use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::Connection;
use tokio::sync::mpsc::{self, Sender, error::SendError, error::TrySendError};

use crate::commands::IngestCommand;
use crate::config::ensure_sxd_seed_at;
use crate::pipeline::{ScanFilters, WriterContext, writer_loop};
use crate::storage::error::StorageError;
use crate::storage::lock::WorkspaceLock;
use crate::storage::migrate::ensure_schema;
use crate::storage::retry::retry_with_backoff;

const PRAGMA_BOOT_SQL: &str = "
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA synchronous = NORMAL;
PRAGMA temp_store = MEMORY;
PRAGMA mmap_size = 268435456;
PRAGMA busy_timeout = 5000;
";

const POOL_MAX_SIZE: u32 = 8;
/// Override r2d2's 30 s default to bound each `pool.get()` attempt so the
/// outer [`retry_with_backoff`] can cycle more than once within a
/// reasonable test runner window when a sibling process is still
/// releasing WAL handles. 10 s is comfortably above the worst observed
/// cold-pool init on slow Windows CI runners (Defender real-time scan
/// of the freshly-spawned `standardoc.exe` plus the new SQLite WAL/SHM
/// files can push the happy path to a few seconds) while keeping the
/// 6-attempt retry ceiling at ~60 s instead of 180 s.
const POOL_CONNECTION_TIMEOUT: Duration = Duration::from_secs(10);
const WRITER_CHANNEL_CAPACITY: usize = 64;

/// Field order is load-bearing: `sender` MUST be dropped before `inner` so
/// that the writer's `blocking_recv` returns `None` (channel closed) before
/// `IndexHandleInner::drop` calls `JoinHandle::join`. Reversing the fields
/// would deadlock the last-handle drop because the writer would still be
/// blocked on a live sender.
#[derive(Clone)]
pub struct IndexHandle {
    sender: Sender<IngestCommand>,
    inner: Arc<IndexHandleInner>,
}

type SharedPool = Arc<RwLock<Option<Pool<SqliteConnectionManager>>>>;

struct IndexHandleInner {
    pool: SharedPool,
    workspace_root: PathBuf,
    /// Transient runtime flag. When `true`, `cold_start::run` aborts at the
    /// next chunk boundary and the watcher dispatch loop no-ops events. Not
    /// persisted across reboots — caller resumes by clearing the flag.
    paused: Arc<AtomicBool>,
    writer_handle: Mutex<Option<JoinHandle<()>>>,
    /// `Some` for primary opens that own the fs4 exclusive lock (LSP
    /// daemon, `standardoc watch`, `standardoc index`). `None` for
    /// SECONDARY opens that coexist with a primary writer (MCP daemon
    /// spawned alongside the LSP, CLI flag `--readonly`).
    ///
    /// Despite the historical name (`open_readonly`, `--readonly`),
    /// secondary handles are R/W under SQLite WAL — they CAN write
    /// (`submit*`, `resolve_external`-driven external ingestion, …).
    /// SQLite serialises concurrent writers from both daemons; the
    /// revision counter persisted in `schema_meta` keeps them in sync.
    /// The `is_none()` check is still meaningful: it tells server code
    /// to skip cold-start + watcher boot (the primary owns those).
    lock: Option<WorkspaceLock>,
}

impl Drop for IndexHandleInner {
    fn drop(&mut self) {
        let handle = match self.writer_handle.lock() {
            Ok(mut g) => g.take(),
            Err(p) => p.into_inner().take(),
        };
        if let Some(h) = handle {
            let _ = h.join();
        }
    }
}

impl IndexHandle {
    pub fn open<P: AsRef<Path>>(workspace_root: P) -> Result<Self, StorageError> {
        let workspace_root = workspace_root.as_ref().canonicalize()?;
        let standardoc_dir = workspace_root.join(".standardoc");
        std::fs::create_dir_all(&standardoc_dir)?;

        let lock_path = standardoc_dir.join("db.lock");
        let db_path = standardoc_dir.join("index.db");

        // The fs4 lock, the SQLite pool open and the first connection are
        // all retried as a unit: on a fast restart any of them can hit a
        // transient race against the prior process still releasing its
        // handles. `retry_with_backoff` only loops on transient kinds
        // (`LockHeld`, `locking protocol`, busy, …) — permanent errors
        // (schema too new, IO, …) propagate immediately.
        let (lock, pool) = retry_with_backoff(|| {
            let lock = WorkspaceLock::acquire(&lock_path)?;
            let pool = build_pool(&db_path)?;
            let conn = pool.get()?;
            ensure_schema(&conn)?;
            seed_runtime_metadata(&conn, &workspace_root)?;
            drop(conn);
            Ok::<_, StorageError>((lock, pool))
        })?;

        ensure_sxd_seed_at(&workspace_root)?;

        Self::wire_handle(pool, workspace_root, Some(lock), "standardoc-writer")
    }

    /// Opens an existing workspace index as a SECONDARY daemon: skips the
    /// fs4 exclusive lock (so it coexists with a primary writer such as
    /// the LSP daemon) but still opens the SQLite pool R/W and spawns a
    /// writer thread. SQLite WAL serialises concurrent writers across
    /// both daemons; the revision counter persisted in `schema_meta`
    /// (v6+) lets the secondary observe the primary's writes.
    ///
    /// The name `open_readonly` (and the CLI flag `--readonly`) is kept
    /// for backward compatibility but the actual semantics are
    /// "secondary" — useful for the MCP daemon spawned alongside the
    /// LSP daemon, which needs to write external content discovered via
    /// `resolve_external` without owning the workspace lock.
    ///
    /// Errors with [`StorageError::ReadOnlyMissingDatabase`] when the
    /// workspace has never been indexed (no `.standardoc/index.db`).
    /// Callers that race a primary writer should poll on disk before
    /// calling this.
    pub fn open_readonly<P: AsRef<Path>>(workspace_root: P) -> Result<Self, StorageError> {
        let workspace_root = workspace_root.as_ref().canonicalize()?;
        let db_path = workspace_root.join(".standardoc").join("index.db");
        if !db_path.exists() {
            return Err(StorageError::ReadOnlyMissingDatabase { path: db_path });
        }

        // Secondary daemons race the primary's WAL handle release on fast
        // restarts (LSP exits → MCP --readonly spawns instantly). Retry
        // the pool build + first connection probe on transient errors.
        let pool = retry_with_backoff(|| {
            let pool = build_pool(&db_path)?;
            // Force the lazy first connection so any `locking protocol`
            // surfaces here and routes through the retry loop instead of
            // bubbling up to the daemon's first real query.
            let _probe = pool.get()?;
            Ok::<_, StorageError>(pool)
        })?;
        Self::wire_handle(pool, workspace_root, None, "standardoc-writer-secondary")
    }

    /// Builds the inner state (pool, writer thread, sender) shared by
    /// both [`open`] (primary, holds fs4 lock) and [`open_readonly`]
    /// (secondary, no fs4 lock but still R/W under SQLite WAL).
    fn wire_handle(
        pool: Pool<SqliteConnectionManager>,
        workspace_root: PathBuf,
        lock: Option<WorkspaceLock>,
        writer_thread_name: &str,
    ) -> Result<Self, StorageError> {
        let pool: SharedPool = Arc::new(RwLock::new(Some(pool)));
        let paused = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel(WRITER_CHANNEL_CAPACITY);

        let writer_pool = Arc::clone(&pool);
        let writer_handle = std::thread::Builder::new()
            .name(writer_thread_name.to_string())
            .spawn(move || {
                writer_loop(receiver, &WriterContext { pool: writer_pool });
            })
            .map_err(StorageError::Io)?;

        Ok(Self {
            inner: Arc::new(IndexHandleInner {
                pool,
                workspace_root,
                paused,
                writer_handle: Mutex::new(Some(writer_handle)),
                lock,
            }),
            sender,
        })
    }

    /// Returns `true` when this handle was opened via
    /// [`IndexHandle::open_readonly`] and therefore does not own the
    /// workspace's fs4 lock (secondary mode). Servers (`serve_mcp`, …)
    /// inspect this to skip cold-start + watcher boot when running
    /// alongside a primary writer. The name is preserved for back-compat
    /// but the handle is NOT read-only at the SQLite layer (v6+).
    pub fn is_readonly(&self) -> bool {
        self.inner.lock.is_none()
    }

    pub fn rescan_from_scratch(&self) -> Result<(), StorageError> {
        let mut guard = match self.inner.pool.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = None;

        let standardoc_dir = self.inner.workspace_root.join(".standardoc");
        let db_path = standardoc_dir.join("index.db");
        remove_if_exists(&db_path)?;
        remove_if_exists(&standardoc_dir.join("index.db-wal"))?;
        remove_if_exists(&standardoc_dir.join("index.db-shm"))?;

        let pool = build_pool(&db_path)?;
        {
            let conn = pool.get()?;
            ensure_schema(&conn)?;
            seed_runtime_metadata(&conn, &self.inner.workspace_root)?;
        }

        *guard = Some(pool);
        drop(guard);
        Ok(())
    }

    #[allow(clippy::result_large_err)]
    pub fn try_submit(&self, cmd: IngestCommand) -> Result<(), TrySendError<IngestCommand>> {
        self.sender.try_send(cmd)
    }

    #[allow(clippy::result_large_err)]
    pub fn submit_blocking(&self, cmd: IngestCommand) -> Result<(), SendError<IngestCommand>> {
        self.sender.blocking_send(cmd)
    }

    /// Async counterpart to [`IndexHandle::submit_blocking`]. Suitable for LSP / MCP
    /// handlers running on a tokio runtime: `await`s back-pressure from the
    /// writer queue without blocking the executor thread.
    #[allow(clippy::result_large_err)]
    pub async fn submit(&self, cmd: IngestCommand) -> Result<(), SendError<IngestCommand>> {
        self.sender.send(cmd).await
    }

    /// Reads the persisted workspace revision counter from
    /// `schema_meta`. Returns `0` when the DB is inaccessible (rescan in
    /// progress, missing row) — these are transient states and the
    /// caller treats them as "no observable writes yet". Persisted in
    /// schema v6+; see migration `v5_to_v6.sql`.
    pub fn revision(&self) -> u64 {
        let pool = match self.pool() {
            Ok(p) => p,
            Err(_) => return 0,
        };
        let conn = match pool.get() {
            Ok(c) => c,
            Err(_) => return 0,
        };
        conn.query_row(
            "SELECT value FROM schema_meta WHERE key = 'revision'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(0)
    }

    /// Atomically increments the persisted revision counter. Used by
    /// non-writer-thread callers (`purge_externals_by_origin`,
    /// `delete_paths`, cold-start cleanup) that perform direct SQL
    /// writes outside the writer-loop's own transaction-bound bump.
    /// SQLite WAL serialises concurrent increments from both LSP and
    /// MCP daemons.
    pub(crate) fn bump_revision(&self) {
        let pool = match self.pool() {
            Ok(p) => p,
            Err(_) => return,
        };
        let conn = match pool.get() {
            Ok(c) => c,
            Err(_) => return,
        };
        let _ = conn.execute(
            "UPDATE schema_meta SET value = CAST((CAST(value AS INTEGER) + 1) AS TEXT) \
             WHERE key = 'revision'",
            [],
        );
    }

    /// Sets the paused flag. `cold_start::run` aborts at the next chunk
    /// boundary and the watcher dispatch loop no-ops events until `resume`.
    /// Transient — not persisted across reboots.
    pub fn pause(&self) {
        self.inner.paused.store(true, Ordering::Release);
    }

    /// Clears the paused flag. The next `cold_start::run` invocation walks
    /// normally; the watcher resumes dispatching events on the next FS event.
    pub fn resume(&self) {
        self.inner.paused.store(false, Ordering::Release);
    }

    pub fn is_paused(&self) -> bool {
        self.inner.paused.load(Ordering::Acquire)
    }

    /// Returns workspace-relative paths whose row matches the current
    /// `.stdignore` filters. Used by `standardoc purge-excluded` to surface
    /// candidate rows for deletion and by the watcher to diff old vs new
    /// stacks on `.stdignore` changes.
    pub fn list_paths_matching_ignore(
        &self,
        filters: &ScanFilters,
    ) -> Result<Vec<String>, StorageError> {
        let pool = self.pool()?;
        let conn = pool.get()?;
        let mut stmt = conn.prepare("SELECT path FROM files")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            let path = row?;
            if filters.is_skipped(&path) {
                out.push(path);
            }
        }
        Ok(out)
    }

    /// Deletes the listed paths from the index in a single transaction.
    /// FK cascade removes symbols, edges, edge_sites, documents, enrichments.
    /// Bumps the revision when at least one row was deleted.
    pub fn delete_paths(&self, paths: &[String]) -> Result<(), StorageError> {
        if paths.is_empty() {
            return Ok(());
        }
        let pool = self.pool()?;
        let mut conn = pool.get()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut removed: usize = 0;
        {
            let mut stmt = tx.prepare("DELETE FROM files WHERE path = ?1")?;
            for path in paths {
                removed += stmt.execute([path])?;
            }
        }
        tx.commit()?;
        if removed > 0 {
            self.bump_revision();
        }
        Ok(())
    }

    /// Returns every workspace-relative path currently stored in the `files`
    /// table. Used by the watcher to diff old vs new `.stdignore` stacks.
    pub(crate) fn list_all_file_paths(&self) -> Result<Vec<String>, StorageError> {
        let pool = self.pool()?;
        let conn = pool.get()?;
        let mut stmt = conn.prepare("SELECT path FROM files")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn cold_start_progress(&self) -> Result<Option<(u64, u64)>, StorageError> {
        let pool = self.pool()?;
        let conn = pool.get()?;
        let value: String = conn.query_row(
            "SELECT value FROM schema_meta WHERE key = 'cold_start_progress'",
            [],
            |r| r.get(0),
        )?;
        parse_cold_start_progress(&value)
    }

    pub(crate) fn pool(&self) -> Result<Pool<SqliteConnectionManager>, StorageError> {
        let guard = self
            .inner
            .pool
            .try_read()
            .map_err(|_| StorageError::RescanInProgress)?;
        guard
            .as_ref()
            .cloned()
            .ok_or(StorageError::RescanInProgress)
    }

    pub fn workspace_root(&self) -> &Path {
        &self.inner.workspace_root
    }

    /// True when a primary writer (LSP daemon / `standardoc watch`)
    /// currently holds this workspace's fs4 lock — i.e. the workspace is
    /// being actively indexed/watched, regardless of whether THIS handle
    /// is that primary. A secondary read-only MCP daemon uses this to
    /// report `watcher.active` truthfully: it never spawns its own
    /// watcher but rides a peer primary's.
    ///
    /// Implementation: if this handle owns the lock (primary), the answer
    /// is trivially yes. Otherwise we PROBE the lock file with a
    /// non-blocking try-acquire — `LockHeld` means a primary is alive;
    /// success means none exists and the probe lock drops immediately
    /// (held for microseconds, and only when no writer is around to race
    /// it, so the probe is non-destructive).
    pub fn workspace_watched(&self) -> bool {
        if self.inner.lock.is_some() {
            return true;
        }
        let lock_path = self
            .inner
            .workspace_root
            .join(".standardoc")
            .join("db.lock");
        matches!(
            WorkspaceLock::acquire(&lock_path),
            Err(StorageError::LockHeld { .. })
        )
    }
}

fn parse_cold_start_progress(value: &str) -> Result<Option<(u64, u64)>, StorageError> {
    if value.is_empty() {
        return Ok(None);
    }
    let (done_str, total_str) =
        value
            .split_once('/')
            .ok_or_else(|| StorageError::InvalidStoredData {
                detail: format!("malformed cold_start_progress: {value}"),
            })?;
    let done: u64 = done_str
        .parse()
        .map_err(|_| StorageError::InvalidStoredData {
            detail: format!("malformed cold_start_progress: {value}"),
        })?;
    let total: u64 = total_str
        .parse()
        .map_err(|_| StorageError::InvalidStoredData {
            detail: format!("malformed cold_start_progress: {value}"),
        })?;
    Ok(Some((done, total)))
}

fn build_pool(db_path: &Path) -> Result<Pool<SqliteConnectionManager>, StorageError> {
    let manager = SqliteConnectionManager::file(db_path)
        .with_init(|conn| conn.execute_batch(PRAGMA_BOOT_SQL));
    // `min_idle = Some(0)` makes `build()` return without eagerly creating
    // `max_size` connections. The default behaviour spins up
    // [`POOL_MAX_SIZE`] connections in parallel at build time, which on
    // slow CI runners (Windows in particular) can blow past the
    // [`POOL_CONNECTION_TIMEOUT`] cap when each connection is doing
    // SQLite open + WAL pragma setup. Lazy init means each `get()`
    // creates at most one connection, and that one has the full 2 s
    // window.
    let pool = Pool::builder()
        .max_size(POOL_MAX_SIZE)
        .min_idle(Some(0))
        .connection_timeout(POOL_CONNECTION_TIMEOUT)
        .build(manager)?;
    Ok(pool)
}

fn remove_if_exists(path: &Path) -> Result<(), StorageError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(StorageError::Io(e)),
    }
}

fn seed_runtime_metadata(conn: &Connection, workspace_root: &Path) -> Result<(), StorageError> {
    let workspace_root_str = workspace_root.to_string_lossy().into_owned();
    conn.execute(
        "UPDATE schema_meta SET value = ?1 WHERE key = 'workspace_root'",
        [&workspace_root_str],
    )?;

    let created_at: String = conn.query_row(
        "SELECT value FROM schema_meta WHERE key = 'created_at'",
        [],
        |row| row.get(0),
    )?;
    if created_at.is_empty() {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis().to_string())
            .map_err(|e| StorageError::Io(std::io::Error::other(e.to_string())))?;
        conn.execute(
            "UPDATE schema_meta SET value = ?1 WHERE key = 'created_at'",
            [now_ms],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;

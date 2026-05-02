use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{Connection, OpenFlags};
use tokio::sync::mpsc::{self, Sender, error::SendError, error::TrySendError};

use crate::commands::IngestCommand;
use crate::pipeline::{ScanFilters, WriterContext, ensure_stdignore_seed_at, writer_loop};
use crate::storage::error::StorageError;
use crate::storage::lock::WorkspaceLock;
use crate::storage::migrate::ensure_schema;

const PRAGMA_BOOT_SQL: &str = "
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA synchronous = NORMAL;
PRAGMA temp_store = MEMORY;
PRAGMA mmap_size = 268435456;
PRAGMA busy_timeout = 5000;
";

const POOL_MAX_SIZE: u32 = 8;
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
    revision: Arc<AtomicU64>,
    /// Transient runtime flag. When `true`, `cold_start::run` aborts at the
    /// next chunk boundary and the watcher dispatch loop no-ops events. Not
    /// persisted across reboots — caller resumes by clearing the flag.
    paused: Arc<AtomicBool>,
    writer_handle: Mutex<Option<JoinHandle<()>>>,
    /// `Some` for read-write opens; `None` for read-only opens that do not
    /// take the fs4 exclusive lock so they can run alongside a primary
    /// writer (e.g. MCP `--readonly` next to an LSP daemon, lock 31a).
    /// The handle reads `Option::is_none` to decide whether it is read-only.
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
        let lock = WorkspaceLock::acquire(&lock_path)?;

        ensure_stdignore_seed_at(&workspace_root)?;

        let db_path = standardoc_dir.join("index.db");
        let pool = build_pool(&db_path)?;
        {
            let conn = pool.get()?;
            ensure_schema(&conn)?;
            seed_runtime_metadata(&conn, &workspace_root)?;
        }

        let pool: SharedPool = Arc::new(RwLock::new(Some(pool)));
        let revision = Arc::new(AtomicU64::new(0));
        let paused = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel(WRITER_CHANNEL_CAPACITY);

        let writer_pool = Arc::clone(&pool);
        let writer_revision = Arc::clone(&revision);
        let writer_handle = std::thread::Builder::new()
            .name("standardoc-writer".into())
            .spawn(move || {
                writer_loop(
                    receiver,
                    &WriterContext {
                        pool: writer_pool,
                        revision: writer_revision,
                    },
                );
            })
            .map_err(StorageError::Io)?;

        Ok(Self {
            inner: Arc::new(IndexHandleInner {
                pool,
                workspace_root,
                revision,
                paused,
                writer_handle: Mutex::new(Some(writer_handle)),
                lock: Some(lock),
            }),
            sender,
        })
    }

    /// Opens an existing workspace index in read-only mode. Skips the fs4
    /// exclusive lock so a read-only handle can coexist with an active
    /// writer (LSP daemon, `standardoc watch`, etc.). The SQLite pool is
    /// configured with `SQLITE_OPEN_READ_ONLY`; any caller-issued write
    /// (`submit*`, `rescan_from_scratch`, ...) will fail at the SQLite layer
    /// or the closed writer channel respectively.
    ///
    /// Errors with [`StorageError::ReadOnlyMissingDatabase`] when the
    /// workspace has never been indexed (no `.standardoc/index.db`). Callers
    /// that race a primary writer should poll on disk before calling this.
    pub fn open_readonly<P: AsRef<Path>>(workspace_root: P) -> Result<Self, StorageError> {
        let workspace_root = workspace_root.as_ref().canonicalize()?;
        let db_path = workspace_root.join(".standardoc").join("index.db");
        if !db_path.exists() {
            return Err(StorageError::ReadOnlyMissingDatabase { path: db_path });
        }

        let pool = build_readonly_pool(&db_path)?;
        let pool: SharedPool = Arc::new(RwLock::new(Some(pool)));
        let revision = Arc::new(AtomicU64::new(0));
        let paused = Arc::new(AtomicBool::new(false));

        // Closed channel: any `submit*` returns SendError immediately, so
        // attempts to write through a read-only handle fail loudly without
        // an extra `mode` field on the public API.
        let (sender, receiver) = mpsc::channel::<IngestCommand>(1);
        drop(receiver);

        Ok(Self {
            inner: Arc::new(IndexHandleInner {
                pool,
                workspace_root,
                revision,
                paused,
                writer_handle: Mutex::new(None),
                lock: None,
            }),
            sender,
        })
    }

    /// Returns `true` when this handle was opened via [`IndexHandle::open_readonly`] and
    /// therefore does not own the workspace's fs4 lock or a writer thread.
    /// Servers (`serve_mcp`, ...) inspect this to skip cold-start and
    /// watcher boot when running alongside a primary writer.
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

    pub fn revision(&self) -> u64 {
        self.inner.revision.load(Ordering::Acquire)
    }

    pub(crate) fn bump_revision(&self) {
        self.inner.revision.fetch_add(1, Ordering::Release);
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
    let pool = Pool::builder().max_size(POOL_MAX_SIZE).build(manager)?;
    Ok(pool)
}

const PRAGMA_READONLY_BOOT_SQL: &str = "
PRAGMA foreign_keys = ON;
PRAGMA temp_store = MEMORY;
PRAGMA mmap_size = 268435456;
PRAGMA busy_timeout = 5000;
";

fn build_readonly_pool(db_path: &Path) -> Result<Pool<SqliteConnectionManager>, StorageError> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI;
    let manager = SqliteConnectionManager::file(db_path)
        .with_flags(flags)
        .with_init(|conn| conn.execute_batch(PRAGMA_READONLY_BOOT_SQL));
    let pool = Pool::builder().max_size(POOL_MAX_SIZE).build(manager)?;
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
mod tests {
    use std::time::{Duration, Instant};

    use standardoc_ir::{
        Blake3Hash, ExtractedFile, Kind, Language, LanguageKind, RawSymbol, SourceOrigin,
        SymbolLocation, Visibility,
    };
    use tempfile::tempdir;

    use super::*;

    fn read_meta(handle: &IndexHandle, key: &str) -> String {
        let conn = handle.pool().unwrap().get().unwrap();
        conn.query_row(
            "SELECT value FROM schema_meta WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn sample_extracted(file: &str, fqdn: &str) -> ExtractedFile {
        ExtractedFile {
            file: file.into(),
            language: Language::Rust,
            source_origin: SourceOrigin::Workspace,
            is_external: false,
            content_hash: Blake3Hash::new([0xab; 32]),
            byte_size: 100,
            symbols: vec![RawSymbol {
                name: fqdn.rsplit("::").next().unwrap_or(fqdn).into(),
                fqdn: fqdn.into(),
                kind: Kind::Function,
                language_kind: LanguageKind::from("fn_item"),
                module: None,
                visibility: Visibility::Public,
                location: SymbolLocation {
                    file: file.into(),
                    start_line: 1,
                    end_line: 5,
                    start_col: 0,
                    end_col: 1,
                },
                signature: None,
                body_hash: Some(Blake3Hash::new([0x01; 32])),
                attributes: vec![],
            }],
            edges: vec![],
            call_sites: vec![],
            documents: vec![],
        }
    }

    fn wait_revision_at_least(handle: &IndexHandle, target: u64, timeout: Duration) {
        let start = Instant::now();
        while handle.revision() < target {
            assert!(
                start.elapsed() <= timeout,
                "revision did not reach {target} within {timeout:?} (was {})",
                handle.revision()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn open_creates_standardoc_dir_and_db_files() {
        let dir = tempdir().unwrap();
        let _handle = IndexHandle::open(dir.path()).unwrap();
        assert!(dir.path().join(".standardoc/index.db").exists());
        assert!(dir.path().join(".standardoc/db.lock").exists());
    }

    #[test]
    fn open_seeds_workspace_root() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        let stored = read_meta(&handle, "workspace_root");
        assert!(!stored.is_empty(), "workspace_root must be seeded");
    }

    #[test]
    fn open_seeds_created_at_with_unix_epoch_ms() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        let stored = read_meta(&handle, "created_at");
        let parsed: u128 = stored.parse().expect("created_at must parse as u128");
        assert!(parsed > 0);
    }

    #[test]
    fn second_open_on_same_workspace_fails_with_lock_held() {
        let dir = tempdir().unwrap();
        let _handle1 = IndexHandle::open(dir.path()).unwrap();
        let result = IndexHandle::open(dir.path());
        assert!(matches!(result, Err(StorageError::LockHeld { .. })));
    }

    #[test]
    fn handle_clone_shares_pool() {
        let dir = tempdir().unwrap();
        let h1 = IndexHandle::open(dir.path()).unwrap();
        let h2 = h1.clone();
        let _c1 = h1.pool().unwrap().get().unwrap();
        let _c2 = h2.pool().unwrap().get().unwrap();
    }

    #[test]
    fn reopen_after_drop_preserves_created_at() {
        let dir = tempdir().unwrap();
        let stored1 = {
            let handle = IndexHandle::open(dir.path()).unwrap();
            read_meta(&handle, "created_at")
        };
        std::thread::sleep(Duration::from_millis(2));
        let handle2 = IndexHandle::open(dir.path()).unwrap();
        let stored2 = read_meta(&handle2, "created_at");
        assert_eq!(stored1, stored2, "created_at must persist across reopens");
    }

    #[test]
    fn open_returns_workspace_root_canonicalized() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        assert!(handle.workspace_root().is_absolute());
    }

    #[test]
    fn rescan_from_scratch_on_fresh_db_succeeds() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        handle.rescan_from_scratch().unwrap();
        let _conn = handle.pool().unwrap().get().unwrap();
    }

    #[test]
    fn rescan_from_scratch_wipes_existing_data() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            conn.execute(
                "INSERT INTO files (path, content_hash, language, last_scanned, byte_size) \
                 VALUES ('src/lib.rs', 'aa', 'rust', 0, 0)",
                [],
            )
            .unwrap();
        }
        handle.rescan_from_scratch().unwrap();
        let conn = handle.pool().unwrap().get().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "rescan must wipe existing data");
    }

    #[test]
    fn rescan_from_scratch_reseeds_created_at_strictly_greater() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        let before: u128 = read_meta(&handle, "created_at").parse().unwrap();

        std::thread::sleep(Duration::from_millis(2));
        handle.rescan_from_scratch().unwrap();

        let after: u128 = read_meta(&handle, "created_at").parse().unwrap();
        assert!(
            after > before,
            "rescan must reseed created_at to a newer epoch (before={before}, after={after})"
        );
    }

    #[test]
    fn try_submit_upsert_then_revision_bumps() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        assert_eq!(handle.revision(), 0);

        handle
            .try_submit(IngestCommand::UpsertFile {
                path: "src/main.rs".into(),
                extracted: sample_extracted("src/main.rs", "crate::foo"),
            })
            .unwrap();
        wait_revision_at_least(&handle, 1, Duration::from_secs(2));

        let conn = handle.pool().unwrap().get().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn submit_blocking_succeeds_on_open_handle() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        handle
            .submit_blocking(IngestCommand::UpsertFile {
                path: "src/main.rs".into(),
                extracted: sample_extracted("src/main.rs", "crate::bar"),
            })
            .unwrap();
        wait_revision_at_least(&handle, 1, Duration::from_secs(2));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn submit_async_reaches_writer_thread() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        handle
            .submit(IngestCommand::UpsertFile {
                path: "src/main.rs".into(),
                extracted: sample_extracted("src/main.rs", "crate::async_baz"),
            })
            .await
            .unwrap();
        wait_revision_at_least(&handle, 1, Duration::from_secs(2));

        let conn = handle.pool().unwrap().get().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn delete_file_via_submit_removes_data() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        handle
            .submit_blocking(IngestCommand::UpsertFile {
                path: "src/main.rs".into(),
                extracted: sample_extracted("src/main.rs", "crate::foo"),
            })
            .unwrap();
        wait_revision_at_least(&handle, 1, Duration::from_secs(2));

        handle
            .submit_blocking(IngestCommand::DeleteFile {
                path: "src/main.rs".into(),
            })
            .unwrap();
        wait_revision_at_least(&handle, 2, Duration::from_secs(2));

        let conn = handle.pool().unwrap().get().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn rescan_command_does_not_bump_revision_in_14a() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        handle
            .submit_blocking(IngestCommand::RescanFromScratch)
            .unwrap();
        handle
            .submit_blocking(IngestCommand::UpsertFile {
                path: "src/main.rs".into(),
                extracted: sample_extracted("src/main.rs", "crate::foo"),
            })
            .unwrap();
        wait_revision_at_least(&handle, 1, Duration::from_secs(2));
        assert_eq!(
            handle.revision(),
            1,
            "RescanFromScratch command is unimplemented in 14a (returns Err)"
        );
    }

    #[test]
    fn cold_start_progress_returns_none_on_empty_meta() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        assert_eq!(handle.cold_start_progress().unwrap(), None);
    }

    #[test]
    fn cold_start_progress_parses_done_over_total() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            conn.execute(
                "UPDATE schema_meta SET value = '42/100' WHERE key = 'cold_start_progress'",
                [],
            )
            .unwrap();
        }
        assert_eq!(handle.cold_start_progress().unwrap(), Some((42, 100)));
    }

    #[test]
    fn cold_start_progress_returns_invalid_stored_data_on_garbage() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            conn.execute(
                "UPDATE schema_meta SET value = 'lol' WHERE key = 'cold_start_progress'",
                [],
            )
            .unwrap();
        }
        let err = handle.cold_start_progress().unwrap_err();
        assert!(matches!(err, StorageError::InvalidStoredData { .. }));
    }

    #[test]
    fn drop_last_handle_joins_writer_thread() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        handle
            .submit_blocking(IngestCommand::UpsertFile {
                path: "src/main.rs".into(),
                extracted: sample_extracted("src/main.rs", "crate::foo"),
            })
            .unwrap();
        wait_revision_at_least(&handle, 1, Duration::from_secs(2));
        drop(handle);
    }

    #[test]
    fn open_seeds_stdignore_when_absent() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();

        let stdignore = handle.workspace_root().join(".stdignore");
        let body = std::fs::read_to_string(&stdignore).unwrap();
        assert!(body.contains(".git/"));
        assert!(body.contains("target/"));
        assert!(body.contains("node_modules/"));
        assert!(
            !body.contains(".standardoc/"),
            ".stdignore seed must not include .standardoc/ (lock 21 Q3)"
        );
    }

    #[test]
    fn open_preserves_existing_stdignore() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".stdignore");
        let custom = "# user authored\nfoo/\n!foo/keep.rs\n";
        std::fs::write(&path, custom).unwrap();

        let _handle = IndexHandle::open(dir.path()).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(body, custom);
    }

    #[test]
    fn is_paused_default_false() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        assert!(!handle.is_paused());
    }

    #[test]
    fn pause_resume_round_trips() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();

        handle.pause();
        assert!(handle.is_paused());

        handle.resume();
        assert!(!handle.is_paused());

        handle.pause();
        handle.pause();
        assert!(handle.is_paused(), "double pause is idempotent");
        handle.resume();
        assert!(!handle.is_paused());
    }

    fn seed_file(handle: &IndexHandle, path: &str) {
        let conn = handle.pool().unwrap().get().unwrap();
        conn.execute(
            "INSERT INTO files (path, content_hash, language, last_scanned, byte_size) \
             VALUES (?1, 'aa', 'rust', 0, 0)",
            [path],
        )
        .unwrap();
    }

    fn count_files(handle: &IndexHandle) -> i64 {
        let conn = handle.pool().unwrap().get().unwrap();
        conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
            .unwrap()
    }

    fn write_stdignore(root: &Path, body: &str) {
        std::fs::write(root.join(".stdignore"), body).unwrap();
    }

    #[test]
    fn list_paths_matching_ignore_returns_only_matched() {
        let dir = tempdir().unwrap();
        write_stdignore(dir.path(), "target/\n");
        let handle = IndexHandle::open(dir.path()).unwrap();
        seed_file(&handle, "src/lib.rs");
        seed_file(&handle, "target/debug/build.rs");
        seed_file(&handle, "target/release/foo.rs");

        let filters = ScanFilters::load(handle.workspace_root());
        let mut matched = handle.list_paths_matching_ignore(&filters).unwrap();
        matched.sort();

        assert_eq!(
            matched,
            vec![
                "target/debug/build.rs".to_string(),
                "target/release/foo.rs".to_string(),
            ]
        );
    }

    #[test]
    fn list_paths_matching_ignore_empty_when_no_match() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        seed_file(&handle, "src/lib.rs");

        let filters = ScanFilters::load(handle.workspace_root());
        assert!(
            handle
                .list_paths_matching_ignore(&filters)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn delete_paths_removes_files_and_bumps_revision() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        seed_file(&handle, "src/a.rs");
        seed_file(&handle, "src/b.rs");
        let revision_before = handle.revision();

        handle
            .delete_paths(&["src/a.rs".into(), "src/b.rs".into()])
            .unwrap();

        assert_eq!(count_files(&handle), 0);
        assert!(handle.revision() > revision_before);
    }

    #[test]
    fn delete_paths_cascades_symbols() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        handle
            .submit_blocking(IngestCommand::UpsertFile {
                path: "src/main.rs".into(),
                extracted: sample_extracted("src/main.rs", "crate::foo"),
            })
            .unwrap();
        wait_revision_at_least(&handle, 1, Duration::from_secs(2));

        handle.delete_paths(&["src/main.rs".into()]).unwrap();

        let conn = handle.pool().unwrap().get().unwrap();
        let symbols: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap();
        assert_eq!(symbols, 0, "FK cascade must drop symbols");
        assert_eq!(count_files(&handle), 0);
    }

    #[test]
    fn delete_paths_no_op_on_empty_input() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        seed_file(&handle, "src/a.rs");
        let revision_before = handle.revision();

        handle.delete_paths(&[]).unwrap();

        assert_eq!(count_files(&handle), 1);
        assert_eq!(
            handle.revision(),
            revision_before,
            "empty input must not bump revision"
        );
    }

    #[test]
    fn delete_paths_does_not_bump_when_nothing_matches() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        let revision_before = handle.revision();

        handle.delete_paths(&["src/missing.rs".into()]).unwrap();

        assert_eq!(handle.revision(), revision_before);
    }

    #[test]
    fn list_all_file_paths_returns_all_rows() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        seed_file(&handle, "src/a.rs");
        seed_file(&handle, "src/b.rs");
        seed_file(&handle, "target/c.rs");

        let mut paths = handle.list_all_file_paths().unwrap();
        paths.sort();

        assert_eq!(
            paths,
            vec![
                "src/a.rs".to_string(),
                "src/b.rs".to_string(),
                "target/c.rs".to_string(),
            ]
        );
    }

    #[test]
    fn open_readonly_returns_err_when_db_missing() {
        let dir = tempdir().unwrap();
        let result = IndexHandle::open_readonly(dir.path());
        assert!(matches!(
            result,
            Err(StorageError::ReadOnlyMissingDatabase { .. })
        ));
    }

    #[test]
    fn open_readonly_coexists_with_writer_without_lock_collision() {
        let dir = tempdir().unwrap();
        let writer = IndexHandle::open(dir.path()).unwrap();
        assert!(!writer.is_readonly());

        let reader = IndexHandle::open_readonly(dir.path()).unwrap();
        assert!(reader.is_readonly());

        let _conn = reader.pool().unwrap().get().unwrap();
        drop(reader);
        drop(writer);
    }

    #[test]
    fn open_readonly_reads_data_written_by_writer_then_dropped() {
        let dir = tempdir().unwrap();
        {
            let handle = IndexHandle::open(dir.path()).unwrap();
            handle
                .submit_blocking(IngestCommand::UpsertFile {
                    path: "src/main.rs".into(),
                    extracted: sample_extracted("src/main.rs", "crate::ro_target"),
                })
                .unwrap();
            wait_revision_at_least(&handle, 1, Duration::from_secs(2));
        }

        let reader = IndexHandle::open_readonly(dir.path()).unwrap();
        let conn = reader.pool().unwrap().get().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "readonly handle must see writer-committed data");

        let submit_err = reader
            .submit_blocking(IngestCommand::UpsertFile {
                path: "src/other.rs".into(),
                extracted: sample_extracted("src/other.rs", "crate::ro_blocked"),
            })
            .unwrap_err();
        let _ = submit_err;
    }
}

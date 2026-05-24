use std::sync::{Arc, RwLock};

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::TransactionBehavior;
use tokio::sync::mpsc::Receiver;

use crate::commands::IngestCommand;
use crate::pipeline::batch::{apply_delete_file, apply_upsert_file, record_parse_error};
use crate::pipeline::peer_extract::{peer_path, scope_extracted_paths};
use crate::storage::error::StorageError;
use crate::storage::module_lookup::PRIMARY_WORKSPACE_ID;

pub(crate) struct WriterContext {
    pub(crate) pool: Arc<RwLock<Option<Pool<SqliteConnectionManager>>>>,
}

pub(crate) fn writer_loop(mut rx: Receiver<IngestCommand>, ctx: &WriterContext) {
    while let Some(cmd) = rx.blocking_recv() {
        let _ = process_command(ctx, cmd);
    }
}

fn process_command(ctx: &WriterContext, cmd: IngestCommand) -> Result<(), StorageError> {
    let pool = acquire_pool(ctx).ok_or(StorageError::RescanInProgress)?;
    let mut conn = pool.get()?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Read the current revision from `schema_meta` (persisted v6+) and
    // tag rows touched by this command with `revision + 1`. The same
    // transaction commits the row writes AND the bumped revision, so
    // any reader (LSP + MCP daemons sharing the DB under WAL) sees the
    // pair atomically.
    let current_revision: u64 = read_revision(&tx)?;
    let next_revision = current_revision.saturating_add(1);
    let res = dispatch(&tx, cmd, next_revision);
    match res {
        Ok(()) => {
            tx.execute(
                "UPDATE schema_meta SET value = ?1 WHERE key = 'revision'",
                [next_revision.to_string()],
            )?;
            tx.commit()?;
            Ok(())
        }
        Err(e) => {
            drop(tx);
            Err(e)
        }
    }
}

fn read_revision(tx: &rusqlite::Transaction<'_>) -> Result<u64, StorageError> {
    let raw: String = tx.query_row(
        "SELECT value FROM schema_meta WHERE key = 'revision'",
        [],
        |row| row.get(0),
    )?;
    raw.parse::<u64>()
        .map_err(|_| StorageError::InvalidSchemaMetadata {
            key: "revision".into(),
            value: raw,
        })
}

fn dispatch(
    conn: &rusqlite::Connection,
    cmd: IngestCommand,
    revision: u64,
) -> Result<(), StorageError> {
    match cmd {
        IngestCommand::UpsertFile { extracted, .. } => {
            apply_upsert_file(conn, &extracted, revision, PRIMARY_WORKSPACE_ID)
        }
        IngestCommand::DeleteFile { path } => apply_delete_file(conn, &path),
        IngestCommand::RecordParseError {
            path,
            language,
            detail,
        } => record_parse_error(conn, &path, language, &detail),
        IngestCommand::UpsertPeerFile {
            workspace_id,
            extracted,
            ..
        } => {
            let mut scoped = extracted;
            scope_extracted_paths(&mut scoped, &workspace_id);
            apply_upsert_file(conn, &scoped, revision, &workspace_id)
        }
        IngestCommand::DeletePeerFile { workspace_id, path } => {
            let scoped = peer_path(&workspace_id, &path);
            apply_delete_file(conn, &scoped)
        }
        IngestCommand::RecordPeerParseError {
            workspace_id,
            path,
            language,
            detail,
        } => {
            let scoped = peer_path(&workspace_id, &path);
            record_parse_error(conn, &scoped, language, &detail)
        }
        IngestCommand::RescanFromScratch => Err(StorageError::RescanInProgress),
    }
}

fn acquire_pool(ctx: &WriterContext) -> Option<Pool<SqliteConnectionManager>> {
    let guard = ctx
        .pool
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.as_ref().cloned()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, RwLock};
    use std::time::{Duration, Instant};

    use r2d2_sqlite::SqliteConnectionManager;
    use standardoc_ir::{
        Blake3Hash, ExtractedFile, Kind, Language, LanguageKind, RawSymbol, SourceOrigin,
        SymbolLocation, Visibility,
    };
    use tempfile::tempdir;
    use tokio::sync::mpsc;

    use super::*;
    use crate::commands::IngestCommand;

    fn boot_pool(path: &std::path::Path) -> Pool<SqliteConnectionManager> {
        let manager = SqliteConnectionManager::file(path).with_init(|c| {
            c.execute_batch(
                "PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; \
                 PRAGMA synchronous = NORMAL; PRAGMA temp_store = MEMORY;",
            )
        });
        let pool = Pool::builder().max_size(2).build(manager).unwrap();
        let conn = pool.get().unwrap();
        crate::storage::migrate::ensure_schema(&conn).unwrap();
        drop(conn);
        pool
    }

    fn read_revision_via_pool(pool: &Pool<SqliteConnectionManager>) -> u64 {
        let conn = pool.get().unwrap();
        let raw: String = conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key = 'revision'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        raw.parse().unwrap()
    }

    fn wait_revision_via_pool(
        pool: &Arc<RwLock<Option<Pool<SqliteConnectionManager>>>>,
        target: u64,
        timeout: Duration,
    ) {
        let start = Instant::now();
        loop {
            let inner = pool.read().unwrap().as_ref().cloned();
            let current = inner.map_or(0, |p| read_revision_via_pool(&p));
            if current >= target {
                return;
            }
            assert!(
                start.elapsed() <= timeout,
                "writer revision did not reach {target} within {timeout:?} (was {current})"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn sample_extracted() -> ExtractedFile {
        ExtractedFile {
            file: "src/main.rs".into(),
            language: Language::Rust,
            source_origin: SourceOrigin::Workspace,
            is_external: false,
            content_hash: Blake3Hash::new([0xab; 32]),
            byte_size: 100,
            symbols: vec![RawSymbol {
                decl_kind: None,
                implements_trait: None,
                receiver_type: None,
                entry_point: None,
                name: "foo".into(),
                fqdn: "crate::foo".into(),
                kind: Kind::Callable,
                language_kind: LanguageKind::from("fn_item"),
                module: None,
                visibility: Visibility::Public,
                location: SymbolLocation {
                    file: "src/main.rs".into(),
                    start_line: 1,
                    end_line: 5,
                    start_col: 0,
                    end_col: 1,
                },
                signature: None,
                body_hash: Some(Blake3Hash::new([0x01; 32])),
                attributes: vec![],
                flags: vec![],
            }],
            edges: vec![],
            call_sites: vec![],
            documents: vec![],
            ffi_bindings: vec![],
            module_lookup: None,
        }
    }

    #[test]
    fn writer_processes_upsert_and_bumps_revision() {
        let dir = tempdir().unwrap();
        let pool = boot_pool(&dir.path().join("index.db"));
        let pool_lock = Arc::new(RwLock::new(Some(pool)));
        let (tx, rx) = mpsc::channel(8);

        let ctx = WriterContext {
            pool: Arc::clone(&pool_lock),
        };
        let handle = std::thread::spawn(move || writer_loop(rx, &ctx));

        tx.blocking_send(IngestCommand::UpsertFile {
            path: "src/main.rs".into(),
            extracted: sample_extracted(),
        })
        .unwrap();
        wait_revision_via_pool(&pool_lock, 1, Duration::from_secs(2));

        drop(tx);
        handle.join().unwrap();
    }

    #[test]
    fn writer_does_not_bump_revision_on_rescan_command_in_14a() {
        let dir = tempdir().unwrap();
        let pool = boot_pool(&dir.path().join("index.db"));
        let pool_lock = Arc::new(RwLock::new(Some(pool)));
        let (tx, rx) = mpsc::channel(8);

        let ctx = WriterContext {
            pool: Arc::clone(&pool_lock),
        };
        let handle = std::thread::spawn(move || writer_loop(rx, &ctx));

        tx.blocking_send(IngestCommand::RescanFromScratch).unwrap();
        tx.blocking_send(IngestCommand::UpsertFile {
            path: "src/main.rs".into(),
            extracted: sample_extracted(),
        })
        .unwrap();
        wait_revision_via_pool(&pool_lock, 1, Duration::from_secs(2));

        drop(tx);
        handle.join().unwrap();

        let final_revision = read_revision_via_pool(pool_lock.read().unwrap().as_ref().unwrap());
        assert_eq!(
            final_revision, 1,
            "rescan command must not bump revision in 14a (returns Err)"
        );
    }

    #[test]
    fn writer_dispatch_peer_upsert_scopes_path_and_workspace_id() {
        // L3d-1: an `UpsertPeerFile` round-trips through the writer queue,
        // gets path-scoped via `peer_path`, and its symbol row carries the
        // peer's workspace_id (NOT the 'primary' default).
        let dir = tempdir().unwrap();
        let pool = boot_pool(&dir.path().join("index.db"));
        let pool_lock = Arc::new(RwLock::new(Some(pool)));
        let (tx, rx) = mpsc::channel(8);

        let ctx = WriterContext {
            pool: Arc::clone(&pool_lock),
        };
        let handle = std::thread::spawn(move || writer_loop(rx, &ctx));

        tx.blocking_send(IngestCommand::UpsertPeerFile {
            workspace_id: "peer-w1".into(),
            path: "src/main.rs".into(),
            extracted: sample_extracted(),
        })
        .unwrap();
        wait_revision_via_pool(&pool_lock, 1, Duration::from_secs(2));

        let conn = pool_lock.read().unwrap().as_ref().unwrap().get().unwrap();
        let file_path: String = conn
            .query_row(
                "SELECT path FROM files WHERE path LIKE 'ws:peer-w1:%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(file_path, "ws:peer-w1:src/main.rs");

        let sym_ws_id: String = conn
            .query_row(
                "SELECT workspace_id FROM symbols WHERE fqdn = 'crate::foo'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sym_ws_id, "peer-w1");

        drop(tx);
        handle.join().unwrap();
    }

    #[test]
    fn writer_dispatch_peer_delete_scopes_path() {
        // L3d-1: an `UpsertPeerFile` then `DeletePeerFile` on the same rel
        // path must remove the scoped row (and only that row) — primary's
        // unrelated `src/main.rs` would be untouched if present.
        let dir = tempdir().unwrap();
        let pool = boot_pool(&dir.path().join("index.db"));
        let pool_lock = Arc::new(RwLock::new(Some(pool)));
        let (tx, rx) = mpsc::channel(8);

        let ctx = WriterContext {
            pool: Arc::clone(&pool_lock),
        };
        let handle = std::thread::spawn(move || writer_loop(rx, &ctx));

        tx.blocking_send(IngestCommand::UpsertPeerFile {
            workspace_id: "peer-w2".into(),
            path: "src/main.rs".into(),
            extracted: sample_extracted(),
        })
        .unwrap();
        wait_revision_via_pool(&pool_lock, 1, Duration::from_secs(2));

        tx.blocking_send(IngestCommand::DeletePeerFile {
            workspace_id: "peer-w2".into(),
            path: "src/main.rs".into(),
        })
        .unwrap();
        wait_revision_via_pool(&pool_lock, 2, Duration::from_secs(2));

        let conn = pool_lock.read().unwrap().as_ref().unwrap().get().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE path = 'ws:peer-w2:src/main.rs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "peer delete must remove the scoped row");

        drop(tx);
        handle.join().unwrap();
    }

    #[test]
    fn writer_dispatch_peer_parse_error_scopes_path() {
        // L3d-1: a `RecordPeerParseError` on an existing peer file row
        // writes `last_scan_error` keyed by the scoped path. The peer file
        // must be seeded first (record_parse_error UPDATEs by path).
        let dir = tempdir().unwrap();
        let pool = boot_pool(&dir.path().join("index.db"));
        let pool_lock = Arc::new(RwLock::new(Some(pool)));
        let (tx, rx) = mpsc::channel(8);

        let ctx = WriterContext {
            pool: Arc::clone(&pool_lock),
        };
        let handle = std::thread::spawn(move || writer_loop(rx, &ctx));

        tx.blocking_send(IngestCommand::UpsertPeerFile {
            workspace_id: "peer-w3".into(),
            path: "src/main.rs".into(),
            extracted: sample_extracted(),
        })
        .unwrap();
        wait_revision_via_pool(&pool_lock, 1, Duration::from_secs(2));

        tx.blocking_send(IngestCommand::RecordPeerParseError {
            workspace_id: "peer-w3".into(),
            path: "src/main.rs".into(),
            language: Language::Rust,
            detail: "boom".into(),
        })
        .unwrap();
        // record_parse_error does NOT bump revision; let the writer drain
        // by closing the channel and joining the loop.
        drop(tx);
        handle.join().unwrap();

        let conn = pool_lock.read().unwrap().as_ref().unwrap().get().unwrap();
        let last_error: Option<String> = conn
            .query_row(
                "SELECT last_scan_error FROM files WHERE path = 'ws:peer-w3:src/main.rs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(last_error.as_deref(), Some("boom"));
    }

    #[test]
    fn writer_exits_when_sender_drops() {
        let dir = tempdir().unwrap();
        let pool = boot_pool(&dir.path().join("index.db"));
        let pool_lock = Arc::new(RwLock::new(Some(pool)));
        let (tx, rx) = mpsc::channel::<IngestCommand>(4);

        let ctx = WriterContext {
            pool: Arc::clone(&pool_lock),
        };
        let handle = std::thread::spawn(move || writer_loop(rx, &ctx));

        drop(tx);
        handle.join().unwrap();
    }
}

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::TransactionBehavior;
use tokio::sync::mpsc::Receiver;

use crate::commands::IngestCommand;
use crate::pipeline::batch::{apply_delete_file, apply_upsert_file, record_parse_error};
use crate::storage::error::StorageError;

pub(crate) struct WriterContext {
    pub(crate) pool: Arc<RwLock<Option<Pool<SqliteConnectionManager>>>>,
    pub(crate) revision: Arc<AtomicU64>,
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
    let res = dispatch(&tx, cmd);
    match res {
        Ok(()) => {
            tx.commit()?;
            ctx.revision.fetch_add(1, Ordering::Release);
            Ok(())
        }
        Err(e) => {
            drop(tx);
            Err(e)
        }
    }
}

fn dispatch(conn: &rusqlite::Connection, cmd: IngestCommand) -> Result<(), StorageError> {
    match cmd {
        IngestCommand::UpsertFile { extracted, .. } => apply_upsert_file(conn, &extracted),
        IngestCommand::DeleteFile { path } => apply_delete_file(conn, &path),
        IngestCommand::RecordParseError {
            path,
            language,
            detail,
        } => record_parse_error(conn, &path, language, &detail),
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
    use std::sync::atomic::AtomicU64;
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
        crate::storage::init::run_init_schema(&conn).unwrap();
        drop(conn);
        pool
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
                name: "foo".into(),
                fqdn: "crate::foo".into(),
                kind: Kind::Function,
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
            }],
            edges: vec![],
            call_sites: vec![],
        }
    }

    fn wait_revision_at_least(revision: &AtomicU64, target: u64, timeout: Duration) {
        let start = Instant::now();
        while revision.load(Ordering::Acquire) < target {
            assert!(
                start.elapsed() <= timeout,
                "writer revision did not reach {target} within {timeout:?} (was {})",
                revision.load(Ordering::Acquire)
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn writer_processes_upsert_and_bumps_revision() {
        let dir = tempdir().unwrap();
        let pool = boot_pool(&dir.path().join("index.db"));
        let pool_lock = Arc::new(RwLock::new(Some(pool)));
        let revision = Arc::new(AtomicU64::new(0));
        let (tx, rx) = mpsc::channel(8);

        let ctx = WriterContext {
            pool: Arc::clone(&pool_lock),
            revision: Arc::clone(&revision),
        };
        let handle = std::thread::spawn(move || writer_loop(rx, &ctx));

        tx.blocking_send(IngestCommand::UpsertFile {
            path: "src/main.rs".into(),
            extracted: sample_extracted(),
        })
        .unwrap();
        wait_revision_at_least(&revision, 1, Duration::from_secs(2));

        drop(tx);
        handle.join().unwrap();
    }

    #[test]
    fn writer_does_not_bump_revision_on_rescan_command_in_14a() {
        let dir = tempdir().unwrap();
        let pool = boot_pool(&dir.path().join("index.db"));
        let pool_lock = Arc::new(RwLock::new(Some(pool)));
        let revision = Arc::new(AtomicU64::new(0));
        let (tx, rx) = mpsc::channel(8);

        let ctx = WriterContext {
            pool: Arc::clone(&pool_lock),
            revision: Arc::clone(&revision),
        };
        let handle = std::thread::spawn(move || writer_loop(rx, &ctx));

        tx.blocking_send(IngestCommand::RescanFromScratch).unwrap();
        tx.blocking_send(IngestCommand::UpsertFile {
            path: "src/main.rs".into(),
            extracted: sample_extracted(),
        })
        .unwrap();
        wait_revision_at_least(&revision, 1, Duration::from_secs(2));

        drop(tx);
        handle.join().unwrap();

        assert_eq!(
            revision.load(Ordering::Acquire),
            1,
            "rescan command must not bump revision in 14a (returns Err)"
        );
    }

    #[test]
    fn writer_exits_when_sender_drops() {
        let dir = tempdir().unwrap();
        let pool = boot_pool(&dir.path().join("index.db"));
        let pool_lock = Arc::new(RwLock::new(Some(pool)));
        let revision = Arc::new(AtomicU64::new(0));
        let (tx, rx) = mpsc::channel::<IngestCommand>(4);

        let ctx = WriterContext {
            pool: Arc::clone(&pool_lock),
            revision: Arc::clone(&revision),
        };
        let handle = std::thread::spawn(move || writer_loop(rx, &ctx));

        drop(tx);
        handle.join().unwrap();
    }
}

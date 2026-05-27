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
mod tests;

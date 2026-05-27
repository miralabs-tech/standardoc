//! Workspace catalog operations — the registry of linked workspaces
//! that the primary daemon resolves cross-workspace imports against.
//!
//! Each linked workspace gets a UUID v4 `workspace_id` recorded in
//! `workspace_catalog`. Module lookups and import records keyed by
//! that id can then be persisted via `storage::module_lookup` and
//! cross-joined to resolve imports between workspaces (Stage 3b-4).
//!
//! The primary workspace itself does NOT get a catalog row — it uses
//! the `'primary'` sentinel `workspace_id` everywhere.

use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use standardoc_ir::{IndexingMode, LinkDirection, LinkedWorkspaceStatus};
use uuid::Uuid;

use crate::storage::error::StorageError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinkedWorkspace {
    pub workspace_id: String,
    pub root_path: String,
    pub link_direction: LinkDirection,
    pub linked_at_epoch_ms: i64,
    pub last_indexed_at_epoch_ms: Option<i64>,
    pub status: LinkedWorkspaceStatus,
    /// Stage 3b-7-b Layer 3c: which extraction pipeline cold_start
    /// (and explicit refresh hooks) routes this peer through —
    /// `BlobImport` (3b-7-a, cheap blob copy) or `Extract` (3b-7-b,
    /// autonomous source walk via `pipeline::peer_extract`).
    pub indexing_mode: IndexingMode,
}

fn epoch_ms_now() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis()),
    )
    .unwrap_or(i64::MAX)
}

#[allow(clippy::needless_pass_by_value)]
fn row_to_linked_workspace(
    workspace_id: String,
    root_path: String,
    direction_raw: i64,
    linked_at: i64,
    last_indexed_at: Option<i64>,
    status_raw: String,
    indexing_mode_raw: String,
) -> Result<LinkedWorkspace, StorageError> {
    let link_direction =
        LinkDirection::from_i64(direction_raw).ok_or_else(|| StorageError::InvalidStoredData {
            detail: format!("unknown link_direction value {direction_raw}"),
        })?;
    let status = LinkedWorkspaceStatus::from_str(&status_raw).ok_or_else(|| {
        StorageError::InvalidStoredData {
            detail: format!("unknown workspace status '{status_raw}'"),
        }
    })?;
    let indexing_mode = IndexingMode::from_str(&indexing_mode_raw).ok_or_else(|| {
        StorageError::InvalidStoredData {
            detail: format!("unknown indexing_mode '{indexing_mode_raw}'"),
        }
    })?;
    Ok(LinkedWorkspace {
        workspace_id,
        root_path,
        link_direction,
        linked_at_epoch_ms: linked_at,
        last_indexed_at_epoch_ms: last_indexed_at,
        status,
        indexing_mode,
    })
}

/// Register a new linked workspace and return its freshly-generated
/// UUID v4 id. The same `root_path` may be registered multiple times
/// (yielding different ids) — callers should de-dup via
/// `find_by_root_path` first if that's the intent.
pub(crate) fn register_linked_workspace(
    conn: &Connection,
    root_path: &str,
    direction: LinkDirection,
    indexing_mode: IndexingMode,
) -> Result<String, StorageError> {
    let workspace_id = Uuid::new_v4().to_string();
    let now = epoch_ms_now();
    conn.execute(
        "INSERT INTO workspace_catalog \
		 (workspace_id, root_path, link_direction, linked_at, last_indexed_at, status, indexing_mode) \
		 VALUES (?1, ?2, ?3, ?4, NULL, 'active', ?5)",
        params![
            &workspace_id,
            root_path,
            direction.as_i64(),
            now,
            indexing_mode.as_str()
        ],
    )?;
    Ok(workspace_id)
}

pub(crate) fn get_linked_workspace(
    conn: &Connection,
    workspace_id: &str,
) -> Result<Option<LinkedWorkspace>, StorageError> {
    let row: Option<(String, i64, i64, Option<i64>, String, String)> = conn
        .query_row(
            "SELECT root_path, link_direction, linked_at, last_indexed_at, status, indexing_mode \
			 FROM workspace_catalog WHERE workspace_id = ?1",
            params![workspace_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?;
    row.map(|(root_path, dir, linked, indexed, status, mode)| {
        row_to_linked_workspace(
            workspace_id.to_string(),
            root_path,
            dir,
            linked,
            indexed,
            status,
            mode,
        )
    })
    .transpose()
}

pub(crate) fn list_linked_workspaces(
    conn: &Connection,
) -> Result<Vec<LinkedWorkspace>, StorageError> {
    let mut stmt = conn.prepare(
		"SELECT workspace_id, root_path, link_direction, linked_at, last_indexed_at, status, indexing_mode \
		 FROM workspace_catalog ORDER BY linked_at ASC",
	)?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, root, dir, linked, indexed, status, mode) = row?;
        out.push(row_to_linked_workspace(
            id, root, dir, linked, indexed, status, mode,
        )?);
    }
    Ok(out)
}

/// Find existing workspace_catalog rows by exact `root_path` match.
/// Multiple matches are possible if the same path was registered
/// repeatedly — caller decides which one to use.
pub(crate) fn find_by_root_path(
    conn: &Connection,
    root_path: &str,
) -> Result<Vec<LinkedWorkspace>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT workspace_id, link_direction, linked_at, last_indexed_at, status, indexing_mode \
		 FROM workspace_catalog WHERE root_path = ?1 ORDER BY linked_at ASC",
    )?;
    let rows = stmt.query_map(params![root_path], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, dir, linked, indexed, status, mode) = row?;
        out.push(row_to_linked_workspace(
            id,
            root_path.to_string(),
            dir,
            linked,
            indexed,
            status,
            mode,
        )?);
    }
    Ok(out)
}

pub(crate) fn unregister_linked_workspace(
    conn: &Connection,
    workspace_id: &str,
) -> Result<(), StorageError> {
    // Clean up dependent tables first to keep cross-workspace queries
    // consistent. The schema lacks FK cascade by design (workspace_id
    // is a free TEXT column shared with the 'primary' sentinel), so
    // the caller orchestrates the cleanup explicitly.
    conn.execute(
        "DELETE FROM workspace_imports WHERE workspace_id = ?1",
        params![workspace_id],
    )?;
    conn.execute(
        "DELETE FROM module_lookups WHERE workspace_id = ?1",
        params![workspace_id],
    )?;
    conn.execute(
        "DELETE FROM workspace_catalog WHERE workspace_id = ?1",
        params![workspace_id],
    )?;
    Ok(())
}

pub(crate) fn set_link_direction(
    conn: &Connection,
    workspace_id: &str,
    direction: LinkDirection,
) -> Result<(), StorageError> {
    conn.execute(
        "UPDATE workspace_catalog SET link_direction = ?1 WHERE workspace_id = ?2",
        params![direction.as_i64(), workspace_id],
    )?;
    Ok(())
}

pub(crate) fn set_status(
    conn: &Connection,
    workspace_id: &str,
    status: LinkedWorkspaceStatus,
) -> Result<(), StorageError> {
    conn.execute(
        "UPDATE workspace_catalog SET status = ?1 WHERE workspace_id = ?2",
        params![status.as_str(), workspace_id],
    )?;
    Ok(())
}

pub(crate) fn set_indexing_mode(
    conn: &Connection,
    workspace_id: &str,
    mode: IndexingMode,
) -> Result<(), StorageError> {
    conn.execute(
        "UPDATE workspace_catalog SET indexing_mode = ?1 WHERE workspace_id = ?2",
        params![mode.as_str(), workspace_id],
    )?;
    Ok(())
}

pub(crate) fn touch_last_indexed(
    conn: &Connection,
    workspace_id: &str,
) -> Result<(), StorageError> {
    let now = epoch_ms_now();
    conn.execute(
        "UPDATE workspace_catalog SET last_indexed_at = ?1 WHERE workspace_id = ?2",
        params![now, workspace_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;

//! Stage 3e-3 — typed access to the `schema_meta` key-value table for
//! workspace-scope singleton metadata. Keys already in use elsewhere
//! (`schema_version`, `workspace_root`, `created_at`,
//! `cold_start_progress`, `watcher_debounce_ms`, `external_*_hash`,
//! `revision`) are touched directly by their respective owners; this
//! module owns the `workspace_kind` slot specifically.
//!
//! Why `schema_meta` and not `workspace_catalog`: the latter is the
//! peer-workspace registry (linked workspaces), and the primary
//! workspace uses the `'primary'` sentinel `workspace_id` everywhere —
//! it never owns a `workspace_catalog` row. `schema_meta` is the
//! existing escape hatch for primary-workspace singletons, so the new
//! key lives there. Zero schema migration required.

use rusqlite::{Connection, OptionalExtension, params};
use standardoc_ir::WorkspaceKind;

use crate::storage::error::StorageError;

const WORKSPACE_KIND_KEY: &str = "workspace_kind";

/// Read the primary workspace's persisted [`WorkspaceKind`]. Returns
/// `Ok(None)` when the key has never been written (legacy databases
/// pre-3e-3 or first cold-start in progress). Callers wanting a
/// non-`Option` default to [`WorkspaceKind::Single`].
pub fn read_workspace_kind(conn: &Connection) -> Result<Option<WorkspaceKind>, StorageError> {
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM schema_meta WHERE key = ?1",
            params![WORKSPACE_KIND_KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(StorageError::from)?;
    Ok(value.map(|s| WorkspaceKind::from_str(&s)))
}

/// Upsert the primary workspace's [`WorkspaceKind`] in `schema_meta`.
/// Idempotent: re-running with the same kind is a no-op write. Called
/// at cold-start after project discovery (and again whenever the
/// manifest watcher Stage 3d-5 re-runs detection on root-manifest
/// changes).
pub fn write_workspace_kind(conn: &Connection, kind: &WorkspaceKind) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO schema_meta (key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![WORKSPACE_KIND_KEY, kind.as_str().as_ref()],
    )
    .map_err(StorageError::from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrate::ensure_schema;

    fn fresh_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        ensure_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn read_returns_none_when_key_absent() {
        let conn = fresh_db();
        let kind = read_workspace_kind(&conn).unwrap();
        assert!(kind.is_none(), "fresh DB must not have workspace_kind");
    }

    #[test]
    fn write_then_read_roundtrips_builtin_variant() {
        let conn = fresh_db();
        write_workspace_kind(&conn, &WorkspaceKind::Cargo).unwrap();
        let kind = read_workspace_kind(&conn).unwrap();
        assert_eq!(kind, Some(WorkspaceKind::Cargo));
    }

    #[test]
    fn write_overwrites_existing_value() {
        let conn = fresh_db();
        write_workspace_kind(&conn, &WorkspaceKind::Npm).unwrap();
        write_workspace_kind(&conn, &WorkspaceKind::Cargo).unwrap();
        let kind = read_workspace_kind(&conn).unwrap();
        assert_eq!(kind, Some(WorkspaceKind::Cargo));
    }

    #[test]
    fn write_then_read_roundtrips_custom_variant() {
        let conn = fresh_db();
        let custom = WorkspaceKind::Custom("bazel".into());
        write_workspace_kind(&conn, &custom).unwrap();
        let kind = read_workspace_kind(&conn).unwrap();
        assert_eq!(kind, Some(custom));
    }

    #[test]
    fn write_then_read_roundtrips_single() {
        let conn = fresh_db();
        write_workspace_kind(&conn, &WorkspaceKind::Single).unwrap();
        let kind = read_workspace_kind(&conn).unwrap();
        assert_eq!(kind, Some(WorkspaceKind::Single));
    }
}

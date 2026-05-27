//! Stage 3d — per-project metadata persistence.
//!
//! The cold-start detection layer (via `standarbuild-detect`) populates
//! this table once at boot; the watcher refreshes it when manifest files
//! change. Every indexed file then carries an optional `project_id`
//! FK so consumers can scope queries by project ownership.
//!
//! `kind` stores `ProjectKind::as_str` (`"rust"`, `"node"`, …, or
//! `"custom:<tag>"` for UST-extended variants). No CHECK constraint —
//! the parser handles validation on read.

use rusqlite::{Connection, OptionalExtension, params};
use standardoc_ir::{ProjectInfo, ProjectKind};

use crate::storage::error::StorageError;

#[allow(clippy::needless_pass_by_value)]
fn row_to_project(
    project_id: i64,
    label: String,
    kind_raw: String,
    root_path: String,
    rel_path: String,
) -> Result<ProjectInfo, StorageError> {
    let kind = ProjectKind::from_str(&kind_raw).ok_or_else(|| StorageError::InvalidStoredData {
        detail: format!("unknown project kind slug `{kind_raw}`"),
    })?;
    let project_id = u32::try_from(project_id).map_err(|_| StorageError::InvalidStoredData {
        detail: format!("project_id {project_id} out of u32 range"),
    })?;
    Ok(ProjectInfo {
        project_id,
        label,
        kind,
        root_path,
        rel_path,
    })
}

/// Insert a project row and return its freshly-assigned `project_id`.
/// Caller is responsible for canonicalising `root_path` upstream — the
/// UNIQUE constraint trips on raw-string mismatch otherwise.
pub(crate) fn insert_project(
    conn: &Connection,
    label: &str,
    kind: &ProjectKind,
    root_path: &str,
    rel_path: &str,
) -> Result<u32, StorageError> {
    let kind_slug = kind.as_str().into_owned();
    conn.execute(
        "INSERT INTO projects (label, kind, root_path, rel_path) \
         VALUES (?1, ?2, ?3, ?4)",
        params![label, kind_slug, root_path, rel_path],
    )?;
    let id = conn.last_insert_rowid();
    u32::try_from(id).map_err(|_| StorageError::InvalidStoredData {
        detail: format!("project_id {id} out of u32 range"),
    })
}

/// UPSERT helper used by the cold-start sweep: if a project already
/// exists at `root_path`, return its id (update label/kind/rel_path if
/// they drifted). Otherwise insert. Idempotent across re-runs.
pub(crate) fn upsert_project(
    conn: &Connection,
    label: &str,
    kind: &ProjectKind,
    root_path: &str,
    rel_path: &str,
) -> Result<u32, StorageError> {
    if let Some(existing) = find_by_root_path(conn, root_path)? {
        let kind_slug = kind.as_str().into_owned();
        conn.execute(
            "UPDATE projects SET label = ?1, kind = ?2, rel_path = ?3 \
             WHERE project_id = ?4",
            params![label, kind_slug, rel_path, existing.project_id],
        )?;
        Ok(existing.project_id)
    } else {
        insert_project(conn, label, kind, root_path, rel_path)
    }
}

pub(crate) fn get_project(
    conn: &Connection,
    project_id: u32,
) -> Result<Option<ProjectInfo>, StorageError> {
    let row: Option<(String, String, String, String)> = conn
        .query_row(
            "SELECT label, kind, root_path, rel_path FROM projects WHERE project_id = ?1",
            params![project_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?;
    row.map(|(label, kind, root_path, rel_path)| {
        row_to_project(i64::from(project_id), label, kind, root_path, rel_path)
    })
    .transpose()
}

pub(crate) fn find_by_root_path(
    conn: &Connection,
    root_path: &str,
) -> Result<Option<ProjectInfo>, StorageError> {
    let row: Option<(i64, String, String, String)> = conn
        .query_row(
            "SELECT project_id, label, kind, rel_path FROM projects WHERE root_path = ?1",
            params![root_path],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?;
    row.map(|(id, label, kind, rel_path)| {
        row_to_project(id, label, kind, root_path.to_string(), rel_path)
    })
    .transpose()
}

pub(crate) fn list_projects(conn: &Connection) -> Result<Vec<ProjectInfo>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT project_id, label, kind, root_path, rel_path \
         FROM projects ORDER BY rel_path ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, label, kind, root, rel) = row?;
        out.push(row_to_project(id, label, kind, root, rel)?);
    }
    Ok(out)
}

/// Resolve a file's owning project by walking up the path looking for
/// the deepest project whose `root_path` is an ancestor (or equal).
/// `file_abs_path` must be canonical (matches what was stored).
///
/// Implementation note: SQL filter `?1 LIKE root_path || '%'` is fast
/// but order-by length to prefer the deepest (most-specific) match.
pub(crate) fn find_for_file_path(
    conn: &Connection,
    file_abs_path: &str,
) -> Result<Option<ProjectInfo>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT project_id, label, kind, root_path, rel_path \
         FROM projects \
         WHERE ?1 = root_path OR ?1 LIKE root_path || '/%' OR ?1 LIKE root_path || '\\%' \
         ORDER BY length(root_path) DESC LIMIT 1",
    )?;
    let row: Option<(i64, String, String, String, String)> = stmt
        .query_row(params![file_abs_path], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .optional()?;
    row.map(|(id, label, kind, root, rel)| row_to_project(id, label, kind, root, rel))
        .transpose()
}

pub(crate) fn delete_project(conn: &Connection, project_id: u32) -> Result<(), StorageError> {
    // Files referencing this project_id keep the column but it goes
    // NULL via the SET NULL semantics we'd need to add. Without
    // CASCADE/SET NULL declared, we manually NULL them first.
    conn.execute(
        "UPDATE files SET project_id = NULL WHERE project_id = ?1",
        params![project_id],
    )?;
    conn.execute(
        "DELETE FROM projects WHERE project_id = ?1",
        params![project_id],
    )?;
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
    fn insert_then_get_roundtrips() {
        let conn = fresh_db();
        let id = insert_project(
            &conn,
            "standardoc-core",
            &ProjectKind::Rust,
            "/r/crates/core",
            "./crates/core",
        )
        .unwrap();
        let got = get_project(&conn, id).unwrap().unwrap();
        assert_eq!(got.project_id, id);
        assert_eq!(got.label, "standardoc-core");
        assert_eq!(got.kind, ProjectKind::Rust);
        assert_eq!(got.root_path, "/r/crates/core");
        assert_eq!(got.rel_path, "./crates/core");
    }

    #[test]
    fn upsert_returns_existing_id_when_root_path_matches() {
        let conn = fresh_db();
        let first = upsert_project(&conn, "old-label", &ProjectKind::Rust, "/r", ".").unwrap();
        let second = upsert_project(&conn, "new-label", &ProjectKind::Rust, "/r", ".").unwrap();
        assert_eq!(first, second, "same root_path must reuse id");
        let row = get_project(&conn, first).unwrap().unwrap();
        assert_eq!(row.label, "new-label", "label must be updated on upsert");
    }

    #[test]
    fn list_orders_by_rel_path() {
        let conn = fresh_db();
        insert_project(&conn, "z", &ProjectKind::Rust, "/r/z", "./z").unwrap();
        insert_project(&conn, "a", &ProjectKind::Bun, "/r/a", "./a").unwrap();
        let rows = list_projects(&conn).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label, "a", "rel_path ordering");
        assert_eq!(rows[1].label, "z");
    }

    #[test]
    fn find_for_file_path_picks_deepest_ancestor() {
        let conn = fresh_db();
        let _root_id = insert_project(&conn, "root", &ProjectKind::Rust, "/r", ".").unwrap();
        let inner_id = insert_project(
            &conn,
            "ext-vscode",
            &ProjectKind::Bun,
            "/r/ext/vscode",
            "./ext/vscode",
        )
        .unwrap();
        let hit = find_for_file_path(&conn, "/r/ext/vscode/src/foo.ts")
            .unwrap()
            .expect("hit");
        assert_eq!(
            hit.project_id, inner_id,
            "deepest ancestor must win, got root instead"
        );
    }

    #[test]
    fn find_for_file_path_returns_none_when_unrelated() {
        let conn = fresh_db();
        insert_project(&conn, "root", &ProjectKind::Rust, "/r", ".").unwrap();
        let miss = find_for_file_path(&conn, "/different/path/foo.rs").unwrap();
        assert!(miss.is_none());
    }

    #[test]
    fn find_by_root_path_returns_none_when_missing() {
        let conn = fresh_db();
        assert!(find_by_root_path(&conn, "/nope").unwrap().is_none());
    }

    #[test]
    fn duplicate_root_path_insert_violates_unique() {
        let conn = fresh_db();
        insert_project(&conn, "a", &ProjectKind::Rust, "/r", ".").unwrap();
        let err = insert_project(&conn, "b", &ProjectKind::Rust, "/r", ".");
        assert!(err.is_err(), "UNIQUE on root_path must reject duplicate");
    }

    #[test]
    fn custom_kind_roundtrips_through_storage() {
        let conn = fresh_db();
        let id = insert_project(
            &conn,
            "gpu",
            &ProjectKind::Custom("wgsl".into()),
            "/r/gpu",
            "./gpu",
        )
        .unwrap();
        let got = get_project(&conn, id).unwrap().unwrap();
        assert_eq!(got.kind, ProjectKind::Custom("wgsl".into()));
    }

    #[test]
    fn delete_nulls_dependent_files_project_id() {
        let conn = fresh_db();
        let id = insert_project(&conn, "x", &ProjectKind::Rust, "/r/x", "./x").unwrap();
        // Seed a files row referencing the project.
        conn.execute(
            "INSERT INTO files (path, content_hash, language, last_scanned, byte_size, project_id) \
             VALUES ('src/lib.rs', 'h', 'rust', 0, 0, ?1)",
            params![id],
        )
        .unwrap();
        delete_project(&conn, id).unwrap();
        let proj_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0))
            .unwrap();
        assert_eq!(proj_count, 0);
        let orphan_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE project_id IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphan_count, 1, "file's project_id must be NULLed");
    }
}

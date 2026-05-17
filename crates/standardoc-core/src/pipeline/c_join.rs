//! Stage 1c — cross-file `.h ↔ .c` join pass for the C provider.
//!
//! After cold-start has finished walking every file and inserting raw
//! symbols, this pass walks every `fn_decl` row in the primary workspace
//! and tries to match it with a `fn` definition by name. On a unique
//! match it:
//!
//!   1. Upserts a `symbol_decl_location` row keyed by the def's id with
//!      the decl's source location, so consumers can reach both ends.
//!   2. Deletes the now-redundant `fn_decl` row. `ON DELETE CASCADE`
//!      removes any dependent edges / call_sites that pointed at it.
//!
//! Matches with 0 candidates are kept as standalone decls (header-only
//! API), matches with >1 candidates are skipped (ambiguous — likely a
//! prototype repeated in multiple translation units, or a static
//! mis-classified). The pass is idempotent: re-running it after the
//! decls have been deleted is a no-op.
//!
//! Scope: primary workspace only (Stage 1c MVP). Peers carry their own
//! workspace_id and would need a separate invocation; deferred until we
//! have a real cross-workspace FFI scenario to validate against.

use rusqlite::Connection;

use crate::storage::error::StorageError;
use crate::storage::handle::IndexHandle;
use crate::storage::module_lookup::PRIMARY_WORKSPACE_ID;
use crate::storage::symbol_decl_location::upsert_decl_location;
use standardoc_ir::SymbolLocation;

/// Counters produced by a join run. Surfaced to callers (cold_start) as
/// a debugging aid — currently logged at trace level only.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CJoinReport {
    pub joined: usize,
    pub orphans_kept: usize,
    pub ambiguous_skipped: usize,
}

fn apply_c_join_primary(handle: &IndexHandle) -> Result<CJoinReport, StorageError> {
    let pool = handle.pool()?;
    let conn = pool.get().map_err(StorageError::from)?;
    run_pass(&conn, PRIMARY_WORKSPACE_ID)
}

/// Best-effort variant used by `cold_start::run`. Errors are logged to
/// stderr and swallowed so a join failure never aborts the cold start
/// of an otherwise-healthy workspace.
pub(crate) fn apply_c_join_quietly(handle: &IndexHandle) {
    if let Err(e) = apply_c_join_primary(handle) {
        eprintln!("standardoc c_join: best-effort pass failed: {e}");
    }
}

fn run_pass(conn: &Connection, workspace_id: &str) -> Result<CJoinReport, StorageError> {
    let decls = collect_decls(conn, workspace_id)?;
    let mut report = CJoinReport::default();

    for d in decls {
        let defs = find_defs_by_name(conn, workspace_id, &d.name)?;
        match defs.len() {
            0 => report.orphans_kept += 1,
            1 => {
                let def_id = defs[0];
                let loc = SymbolLocation {
                    file: d.file_path,
                    start_line: d.start_line,
                    end_line: d.end_line,
                    start_col: d.start_col,
                    end_col: d.end_col,
                };
                upsert_decl_location(conn, def_id, &loc)?;
                conn.execute("DELETE FROM symbols WHERE id = ?1", [d.id])?;
                report.joined += 1;
            }
            _ => report.ambiguous_skipped += 1,
        }
    }
    Ok(report)
}

struct DeclRow {
    id: i64,
    name: String,
    file_path: String,
    start_line: u32,
    end_line: u32,
    start_col: u32,
    end_col: u32,
}

fn collect_decls(conn: &Connection, workspace_id: &str) -> Result<Vec<DeclRow>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, file_path, start_line, end_line, start_col, end_col \
         FROM symbols \
         WHERE language = 'c' AND language_kind = 'fn_decl' AND workspace_id = ?1",
    )?;
    let mapped = stmt.query_map([workspace_id], |r| {
        Ok(DeclRow {
            id: r.get(0)?,
            name: r.get(1)?,
            file_path: r.get(2)?,
            start_line: r.get(3)?,
            end_line: r.get(4)?,
            start_col: r.get(5)?,
            end_col: r.get(6)?,
        })
    })?;
    let mut out = Vec::new();
    for row in mapped {
        out.push(row?);
    }
    Ok(out)
}

fn find_defs_by_name(
    conn: &Connection,
    workspace_id: &str,
    name: &str,
) -> Result<Vec<i64>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT id FROM symbols \
         WHERE language = 'c' AND language_kind = 'fn' AND name = ?1 AND workspace_id = ?2",
    )?;
    let mapped = stmt.query_map(rusqlite::params![name, workspace_id], |r| {
        r.get::<_, i64>(0)
    })?;
    let mut out = Vec::new();
    for row in mapped {
        out.push(row?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrate::ensure_schema;
    use crate::storage::symbol_decl_location::read_decl_location;

    fn fresh_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        ensure_schema(&conn).unwrap();
        conn
    }

    fn seed_file(conn: &Connection, path: &str) {
        conn.execute(
            "INSERT OR IGNORE INTO files (path, content_hash, language, last_scanned, byte_size, is_external) \
             VALUES (?1, 'h', 'c', 0, 0, 0)",
            [path],
        )
        .unwrap();
    }

    fn insert_c_symbol(
        conn: &Connection,
        fqdn: &str,
        name: &str,
        language_kind: &str,
        file_path: &str,
        start_line: u32,
    ) -> i64 {
        seed_file(conn, file_path);
        conn.query_row(
            "INSERT INTO symbols (\
                fqdn, name, kind, language_kind, language, module, visibility, \
                file_path, start_line, end_line, start_col, end_col, \
                signature_json, body_hash, is_external, source_origin, \
                last_modified_revision, flags, workspace_id\
             ) VALUES (?1, ?2, 'function', ?3, 'c', NULL, 'public', \
                ?4, ?5, ?5, 0, 1, NULL, NULL, 0, 'workspace', 0, '[]', 'primary') \
             RETURNING id",
            rusqlite::params![fqdn, name, language_kind, file_path, start_line],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    }

    #[test]
    fn unique_decl_def_pair_is_joined() {
        let conn = fresh_conn();
        let decl_id = insert_c_symbol(
            &conn,
            "lurlang::include::vm::lur_vm_new",
            "lur_vm_new",
            "fn_decl",
            "include/vm.h",
            10,
        );
        let def_id = insert_c_symbol(
            &conn,
            "lurlang::runtime::vm::lur_vm_new",
            "lur_vm_new",
            "fn",
            "runtime/vm.c",
            20,
        );

        let report = run_pass(&conn, "primary").unwrap();
        assert_eq!(report.joined, 1);
        assert_eq!(report.orphans_kept, 0);
        assert_eq!(report.ambiguous_skipped, 0);

        // Decl row is gone.
        let decl_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM symbols WHERE id = ?1",
                [decl_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(decl_count, 0);

        // Decl location now stored against the def.
        let loc = read_decl_location(&conn, def_id).unwrap().unwrap();
        assert_eq!(loc.file, "include/vm.h");
        assert_eq!(loc.start_line, 10);
    }

    #[test]
    fn orphan_decl_is_kept_when_no_matching_def() {
        let conn = fresh_conn();
        let decl_id = insert_c_symbol(
            &conn,
            "lurlang::include::extern_only::strlen",
            "strlen",
            "fn_decl",
            "include/extern_only.h",
            5,
        );

        let report = run_pass(&conn, "primary").unwrap();
        assert_eq!(report.joined, 0);
        assert_eq!(report.orphans_kept, 1);
        assert_eq!(report.ambiguous_skipped, 0);

        // Decl row preserved.
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM symbols WHERE id = ?1",
                [decl_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn ambiguous_decl_with_two_defs_is_skipped() {
        let conn = fresh_conn();
        let decl_id = insert_c_symbol(
            &conn,
            "lurlang::include::shared::log",
            "log",
            "fn_decl",
            "include/shared.h",
            1,
        );
        let def_a = insert_c_symbol(
            &conn,
            "lurlang::projA::a::log",
            "log",
            "fn",
            "projA/a.c",
            10,
        );
        let def_b = insert_c_symbol(
            &conn,
            "lurlang::projB::b::log",
            "log",
            "fn",
            "projB/b.c",
            20,
        );

        let report = run_pass(&conn, "primary").unwrap();
        assert_eq!(report.joined, 0);
        assert_eq!(report.orphans_kept, 0);
        assert_eq!(report.ambiguous_skipped, 1);

        for id in [decl_id, def_a, def_b] {
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM symbols WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(count, 1, "id {id} must still exist");
        }
        // No decl_location persisted for either def.
        assert!(read_decl_location(&conn, def_a).unwrap().is_none());
        assert!(read_decl_location(&conn, def_b).unwrap().is_none());
    }

    #[test]
    fn idempotent_second_run_is_noop() {
        let conn = fresh_conn();
        let _ = insert_c_symbol(
            &conn,
            "lurlang::include::vm::foo",
            "foo",
            "fn_decl",
            "include/vm.h",
            10,
        );
        let _ = insert_c_symbol(
            &conn,
            "lurlang::runtime::vm::foo",
            "foo",
            "fn",
            "runtime/vm.c",
            20,
        );
        let r1 = run_pass(&conn, "primary").unwrap();
        let r2 = run_pass(&conn, "primary").unwrap();
        assert_eq!(r1.joined, 1);
        assert_eq!(r2.joined, 0);
        assert_eq!(r2.orphans_kept, 0);
    }

    #[test]
    fn other_workspace_decls_are_ignored() {
        let conn = fresh_conn();
        seed_file(&conn, "peer/x.h");
        // Insert a fn_decl under 'peer-uuid', not 'primary'.
        conn.execute(
            "INSERT INTO symbols (\
                fqdn, name, kind, language_kind, language, module, visibility, \
                file_path, start_line, end_line, start_col, end_col, \
                signature_json, body_hash, is_external, source_origin, \
                last_modified_revision, flags, workspace_id\
             ) VALUES ('peer::x::foo', 'foo', 'function', 'fn_decl', 'c', NULL, 'public', \
                'peer/x.h', 1, 1, 0, 1, NULL, NULL, 0, 'workspace', 0, '[]', 'peer-uuid')",
            [],
        )
        .unwrap();
        let report = run_pass(&conn, "primary").unwrap();
        assert_eq!(report.joined, 0);
        assert_eq!(report.orphans_kept, 0);
    }
}

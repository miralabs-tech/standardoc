//! Stage 2 — cross-language FFI binding resolution pass.
//!
//! After extraction, every `Export` and `Import` binding lives in
//! `symbol_ffi_binding`. This pass groups them by `(abi, abi_name)` and,
//! for each pair where exactly one export matches exactly one import
//! (and they sit on different symbols), emits an `IMPORTS` edge from the
//! importer to the exporter tagged with `ffi:<abi>`.
//!
//! Why exactly-1-to-exactly-1: ambiguous matches (two exports or two
//! imports sharing the same flat name across providers) almost always
//! mean a real architecture decision is needed — e.g. two crates define
//! `static int g_log_level` and link-time name collision is going to
//! bite. Emitting a "best-guess" edge would paper over that. The pass
//! skips ambiguous tuples and surfaces them as a counter; consumers
//! who care can query `symbol_ffi_binding` directly.
//!
//! Edges produced are de-duplicated against existing rows by the
//! standard storage insert path (composite UNIQUE on
//! `edges (from_symbol_id, kind, to_symbol_id)` keeps re-runs idempotent).
//!
//! Best-effort wiring — failures log to stderr and let cold_start
//! finish.

use std::collections::HashMap;

use rusqlite::Connection;

use rusqlite::OptionalExtension;

use crate::storage::edges::insert_edge;
use crate::storage::error::StorageError;
use crate::storage::handle::IndexHandle;
use crate::storage::symbol_ffi_binding::{BindingSide, fetch_all_sides};
use standardoc_ir::{
    EdgeConfidence, EdgeKind, FfiDirection, RawEdge, ResolvedOrUnresolved,
};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FfiResolveReport {
    pub matched: usize,
    pub ambiguous_skipped: usize,
    pub orphans_skipped: usize,
}

pub(crate) fn apply_ffi_resolve_quietly(handle: &IndexHandle) {
    match apply_ffi_resolve(handle) {
        Ok(_report) => {}
        Err(e) => eprintln!("standardoc ffi_resolve: best-effort pass failed: {e}"),
    }
}

fn apply_ffi_resolve(handle: &IndexHandle) -> Result<FfiResolveReport, StorageError> {
    let pool = handle.pool()?;
    let conn = pool.get().map_err(StorageError::from)?;
    run_pass(&conn)
}

fn run_pass(conn: &Connection) -> Result<FfiResolveReport, StorageError> {
    let exports = fetch_all_sides(conn, FfiDirection::Export)?;
    let imports = fetch_all_sides(conn, FfiDirection::Import)?;

    let mut report = FfiResolveReport::default();

    // Group exports by (abi, abi_name) → Vec<BindingSide>.
    let mut export_idx: HashMap<(String, String), Vec<BindingSide>> = HashMap::new();
    for side in exports {
        export_idx
            .entry((side.abi.clone(), side.abi_name.clone()))
            .or_default()
            .push(side);
    }
    let mut import_idx: HashMap<(String, String), Vec<BindingSide>> = HashMap::new();
    for side in imports {
        import_idx
            .entry((side.abi.clone(), side.abi_name.clone()))
            .or_default()
            .push(side);
    }

    for (key, importers) in import_idx {
        let Some(exporters) = export_idx.get(&key) else {
            report.orphans_skipped += importers.len();
            continue;
        };
        if exporters.len() != 1 {
            report.ambiguous_skipped += importers.len();
            continue;
        }
        let exporter = &exporters[0];

        for importer in &importers {
            if importer.symbol_id == exporter.symbol_id {
                // Same symbol exposing both sides (e.g. a Rust fn
                // declared `#[no_mangle] pub extern "C"` AND referenced
                // by a sibling `extern "C" { fn foo; }` block in the
                // same crate). Not an interesting cross-language edge.
                continue;
            }
            // Pre-check existence so re-running the pass after a
            // previous match is a no-op. The `edges` table has no
            // composite UNIQUE constraint we could rely on for
            // ON CONFLICT — we do the dedup at this layer.
            if ffi_edge_exists(conn, importer.symbol_id, exporter.symbol_id, &key.0)? {
                continue;
            }
            let edge = RawEdge {
                from_fqdn: importer.fqdn.clone(),
                kind: EdgeKind::Imports,
                to: ResolvedOrUnresolved::Resolved {
                    fqdn: exporter.fqdn.clone(),
                },
                sites: vec![],
                attributes: vec![format!("ffi:{}", key.0)],
                confidence: EdgeConfidence::Inferred,
            };
            insert_edge(conn, importer.symbol_id, &edge)?;
            report.matched += 1;
        }
    }
    Ok(report)
}

/// Returns true if an existing `IMPORTS` edge already links the two
/// symbols with an `ffi:<abi>` attribute. We rely on a `LIKE` over the
/// JSON-array `attributes` column because the storage shape is a TEXT
/// payload, not a normalised side table.
fn ffi_edge_exists(
    conn: &Connection,
    from_symbol_id: i64,
    to_symbol_id: i64,
    abi: &str,
) -> Result<bool, StorageError> {
    let needle = format!("%\"ffi:{abi}\"%");
    let hit: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM edges \
             WHERE from_symbol_id = ?1 \
               AND to_symbol_id = ?2 \
               AND kind = 'IMPORTS' \
               AND attributes LIKE ?3 \
             LIMIT 1",
            rusqlite::params![from_symbol_id, to_symbol_id, needle],
            |r| r.get(0),
        )
        .optional()?;
    Ok(hit.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrate::ensure_schema;
    use crate::storage::symbol_ffi_binding::upsert_binding;
    use standardoc_ir::{FfiAbi, RawFfiBinding};

    fn fresh_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        ensure_schema(&conn).unwrap();
        conn
    }

    fn seed_file(conn: &Connection, path: &str) {
        conn.execute(
            "INSERT OR IGNORE INTO files (path, content_hash, language, last_scanned, byte_size, is_external) \
             VALUES (?1, 'h', 'rust', 0, 0, 0)",
            [path],
        )
        .unwrap();
    }

    fn seed_sym(conn: &Connection, fqdn: &str, name: &str, file: &str, lang: &str) -> i64 {
        seed_file(conn, file);
        conn.query_row(
            "INSERT INTO symbols (\
                fqdn, name, kind, language_kind, language, module, visibility, \
                file_path, start_line, end_line, start_col, end_col, \
                signature_json, body_hash, is_external, source_origin, \
                last_modified_revision, flags, workspace_id\
             ) VALUES (?1, ?2, 'function', 'fn', ?3, NULL, 'public', \
                ?4, 1, 5, 0, 1, NULL, NULL, 0, 'workspace', 0, '[]', 'primary') \
             RETURNING id",
            rusqlite::params![fqdn, name, lang, file],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    }

    fn binding(direction: FfiDirection, name: &str) -> RawFfiBinding {
        RawFfiBinding {
            symbol_fqdn: String::new(),
            abi: FfiAbi::C,
            direction,
            abi_name: name.into(),
            convention: None,
        }
    }

    fn count_imports_edges(conn: &Connection) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM edges WHERE kind = 'IMPORTS'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn unique_export_unique_import_emits_one_edge() {
        let conn = fresh_conn();
        let exporter = seed_sym(&conn, "lurlang::vm::lur_init", "lur_init", "vm.c", "c");
        let importer = seed_sym(&conn, "lurlang::lib::lur_init", "lur_init", "lib.rs", "rust");
        upsert_binding(&conn, exporter, &binding(FfiDirection::Export, "lur_init")).unwrap();
        upsert_binding(&conn, importer, &binding(FfiDirection::Import, "lur_init")).unwrap();

        let report = run_pass(&conn).unwrap();
        assert_eq!(report.matched, 1);
        assert_eq!(report.ambiguous_skipped, 0);
        assert_eq!(report.orphans_skipped, 0);
        assert_eq!(count_imports_edges(&conn), 1);
    }

    #[test]
    fn ambiguous_two_exports_one_import_skips() {
        let conn = fresh_conn();
        let e1 = seed_sym(&conn, "a::foo", "foo", "a.c", "c");
        let e2 = seed_sym(&conn, "b::foo", "foo", "b.c", "c");
        let imp = seed_sym(&conn, "lib::foo", "foo", "lib.rs", "rust");
        upsert_binding(&conn, e1, &binding(FfiDirection::Export, "foo")).unwrap();
        upsert_binding(&conn, e2, &binding(FfiDirection::Export, "foo")).unwrap();
        upsert_binding(&conn, imp, &binding(FfiDirection::Import, "foo")).unwrap();

        let report = run_pass(&conn).unwrap();
        assert_eq!(report.matched, 0);
        assert_eq!(report.ambiguous_skipped, 1);
        assert_eq!(count_imports_edges(&conn), 0);
    }

    #[test]
    fn orphan_import_with_no_export_is_skipped() {
        let conn = fresh_conn();
        let imp = seed_sym(&conn, "lib::ghost", "ghost", "lib.rs", "rust");
        upsert_binding(&conn, imp, &binding(FfiDirection::Import, "ghost")).unwrap();

        let report = run_pass(&conn).unwrap();
        assert_eq!(report.matched, 0);
        assert_eq!(report.orphans_skipped, 1);
        assert_eq!(count_imports_edges(&conn), 0);
    }

    #[test]
    fn same_symbol_export_plus_import_does_not_self_loop() {
        let conn = fresh_conn();
        let s = seed_sym(&conn, "x::self", "self_x", "x.rs", "rust");
        upsert_binding(&conn, s, &binding(FfiDirection::Export, "self_x")).unwrap();
        upsert_binding(&conn, s, &binding(FfiDirection::Import, "self_x")).unwrap();
        let report = run_pass(&conn).unwrap();
        assert_eq!(report.matched, 0, "self-loops are filtered out");
        assert_eq!(count_imports_edges(&conn), 0);
    }

    #[test]
    fn second_run_does_not_duplicate_edges() {
        let conn = fresh_conn();
        let exporter = seed_sym(&conn, "vm::foo", "foo", "vm.c", "c");
        let importer = seed_sym(&conn, "lib::foo", "foo", "lib.rs", "rust");
        upsert_binding(&conn, exporter, &binding(FfiDirection::Export, "foo")).unwrap();
        upsert_binding(&conn, importer, &binding(FfiDirection::Import, "foo")).unwrap();

        run_pass(&conn).unwrap();
        let r2 = run_pass(&conn).unwrap();
        // matched counter increments per attempted insert; what matters
        // is the DB stays at exactly one row.
        assert_eq!(count_imports_edges(&conn), 1);
        let _ = r2;
    }
}

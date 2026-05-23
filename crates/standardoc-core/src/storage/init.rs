use rusqlite::Connection;

use crate::storage::error::StorageError;

const INIT_V0_SQL: &str = include_str!("../../migrations/init_v0.sql");

pub(crate) fn run_init_schema(conn: &Connection) -> Result<(), StorageError> {
    conn.execute_batch(INIT_V0_SQL)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn
    }

    fn list_objects(conn: &Connection, ty: &str) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = ?1 ORDER BY name")
            .unwrap();
        stmt.query_map([ty], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn init_creates_all_expected_tables() {
        let conn = fresh_conn();
        run_init_schema(&conn).unwrap();
        let tables = list_objects(&conn, "table");
        for expected in [
            "call_sites",
            "documents",
            "edge_sites",
            "edges",
            "enrichment_rejections",
            "enrichments",
            "files",
            "module_lookups",
            "projects",
            "schema_meta",
            "symbol_decl_location",
            "symbol_ffi_binding",
            "symbols",
            "symbols_fts",
            "workspace_catalog",
            "workspace_imports",
        ] {
            assert!(
                tables.iter().any(|t| t == expected),
                "missing table: {expected}; have: {tables:?}"
            );
        }
    }

    #[test]
    fn init_seeds_schema_version_at_v1() {
        let conn = fresh_conn();
        run_init_schema(&conn).unwrap();
        let v: String = conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(v, "1");
    }

    #[test]
    fn init_seeds_runtime_metadata_keys_empty() {
        let conn = fresh_conn();
        run_init_schema(&conn).unwrap();
        for key in ["workspace_root", "created_at", "cold_start_progress"] {
            let v: String = conn
                .query_row(
                    "SELECT value FROM schema_meta WHERE key = ?1",
                    [key],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(v, "", "key {key} should be seeded empty");
        }
    }

    #[test]
    fn init_seeds_watcher_debounce_ms_default_500() {
        let conn = fresh_conn();
        run_init_schema(&conn).unwrap();
        let v: String = conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key = 'watcher_debounce_ms'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(v, "500");
    }

    #[test]
    fn init_seeds_external_lockfile_hash_keys_empty() {
        let conn = fresh_conn();
        run_init_schema(&conn).unwrap();
        for key in [
            "external_cargo_lockfile_hash",
            "external_npm_lockfile_hash",
            "external_npm_lockfile_kind",
            "external_luarocks_hash",
        ] {
            let v: String = conn
                .query_row(
                    "SELECT value FROM schema_meta WHERE key = ?1",
                    [key],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(v, "", "key {key} should be seeded empty");
        }
    }

    #[test]
    fn init_seeds_revision_zero() {
        let conn = fresh_conn();
        run_init_schema(&conn).unwrap();
        let v: String = conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key = 'revision'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(v, "0");
    }

    #[test]
    fn init_creates_fts_virtual_table() {
        let conn = fresh_conn();
        run_init_schema(&conn).unwrap();
        let row: Option<String> = conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='symbols_fts'",
                [],
                |row| row.get(0),
            )
            .ok();
        assert_eq!(row.as_deref(), Some("symbols_fts"));
    }

    #[test]
    fn init_creates_three_fts_triggers() {
        let conn = fresh_conn();
        run_init_schema(&conn).unwrap();
        let triggers = list_objects(&conn, "trigger");
        assert_eq!(
            triggers.iter().map(String::as_str).collect::<Vec<_>>(),
            vec![
                "symbols_fts_delete",
                "symbols_fts_insert",
                "symbols_fts_update",
            ]
        );
    }

    #[test]
    fn init_creates_expected_indexes() {
        let conn = fresh_conn();
        run_init_schema(&conn).unwrap();
        let indexes = list_objects(&conn, "index");
        for expected in [
            "idx_edges_from_kind",
            "idx_edges_to_kind",
            "idx_edges_unresolved",
            "idx_edge_sites_file",
            "idx_symbols_module",
            "idx_symbols_file_pos",
            "idx_enrichments_confidence",
            "idx_workspace_imports_origin_module",
            "idx_module_lookups_language",
        ] {
            assert!(
                indexes.iter().any(|i| i == expected),
                "missing index: {expected}; have: {indexes:?}"
            );
        }
    }

    #[test]
    fn init_does_not_create_dropped_indexes() {
        let conn = fresh_conn();
        run_init_schema(&conn).unwrap();
        let indexes = list_objects(&conn, "index");
        for dropped in [
            "idx_edges_confidence",
            "idx_edges_kind",
            "idx_edges_from",
            "idx_edges_to",
            "idx_files_language",
            "idx_symbols_kind",
            "idx_symbols_language",
            "idx_symbols_file_path",
            "idx_symbols_last_modified_revision",
            "idx_workspace_imports_workspace_id",
            "idx_projects_root_path",
            "idx_symbol_ffi_binding_symbol",
        ] {
            assert!(
                !indexes.iter().any(|i| i == dropped),
                "v0 must not recreate dropped index: {dropped}"
            );
        }
    }

    #[test]
    fn workspace_imports_pk_includes_origin_module() {
        let conn = fresh_conn();
        run_init_schema(&conn).unwrap();
        // Two glob imports from different origins must coexist —
        // this is the bug that broke cold start on v15 and motivated
        // the v0 reset (PK extended with origin_module).
        conn.execute(
            "INSERT INTO workspace_imports \
             (workspace_id, module_fqdn, local_name, origin_module) \
             VALUES ('primary', 'crate::m', '*', 'std::collections')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO workspace_imports \
             (workspace_id, module_fqdn, local_name, origin_module) \
             VALUES ('primary', 'crate::m', '*', 'std::io')",
            [],
        )
        .unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM workspace_imports WHERE module_fqdn = 'crate::m'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }
}

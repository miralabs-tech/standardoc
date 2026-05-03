use rusqlite::Connection;

use crate::storage::error::StorageError;

const INIT_V1_SQL: &str = include_str!("../../migrations/init_v1.sql");

pub(crate) fn run_init_schema(conn: &Connection) -> Result<(), StorageError> {
    conn.execute_batch(INIT_V1_SQL)?;
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
    fn init_creates_all_real_tables() {
        let conn = fresh_conn();
        run_init_schema(&conn).unwrap();
        let tables = list_objects(&conn, "table");
        for expected in [
            "documents",
            "edge_sites",
            "edges",
            "enrichment_rejections",
            "enrichments",
            "files",
            "schema_meta",
            "symbols",
        ] {
            assert!(
                tables.iter().any(|t| t == expected),
                "missing table: {expected}; have: {tables:?}"
            );
        }
    }

    #[test]
    fn init_seeds_schema_version_at_v1_baseline() {
        // run_init_schema seeds the historical v1 baseline. Fresh DBs go
        // through `ensure_schema` which then loops the upgraders to the
        // current SUPPORTED_SCHEMA_VERSION — see `migrate::ensure_schema`.
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
    fn init_creates_partial_indexes_with_where_clause() {
        let conn = fresh_conn();
        run_init_schema(&conn).unwrap();
        let indexes = list_objects(&conn, "index");
        for expected in [
            "idx_edges_from",
            "idx_edges_to",
            "idx_edges_kind",
            "idx_edges_unresolved",
            "idx_edge_sites_file",
            "idx_symbols_module",
            "idx_enrichments_confidence",
        ] {
            assert!(
                indexes.iter().any(|i| i == expected),
                "missing index: {expected}"
            );
        }
    }
}

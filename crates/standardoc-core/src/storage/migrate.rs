use rusqlite::{Connection, OptionalExtension};

use crate::storage::error::StorageError;
use crate::storage::init::run_init_schema;

pub const SUPPORTED_SCHEMA_VERSION: u32 = 15;

const V1_TO_V2_SQL: &str = include_str!("../../migrations/v1_to_v2.sql");
const V2_TO_V3_SQL: &str = include_str!("../../migrations/v2_to_v3.sql");
const V3_TO_V4_SQL: &str = include_str!("../../migrations/v3_to_v4.sql");
const V4_TO_V5_SQL: &str = include_str!("../../migrations/v4_to_v5.sql");
const V5_TO_V6_SQL: &str = include_str!("../../migrations/v5_to_v6.sql");
const V6_TO_V7_SQL: &str = include_str!("../../migrations/v6_to_v7.sql");
const V7_TO_V8_SQL: &str = include_str!("../../migrations/v7_to_v8.sql");
const V8_TO_V9_SQL: &str = include_str!("../../migrations/v8_to_v9.sql");
const V9_TO_V10_SQL: &str = include_str!("../../migrations/v9_to_v10.sql");
const V10_TO_V11_SQL: &str = include_str!("../../migrations/v10_to_v11.sql");
const V11_TO_V12_SQL: &str = include_str!("../../migrations/v11_to_v12.sql");
const V12_TO_V13_SQL: &str = include_str!("../../migrations/v12_to_v13.sql");
const V13_TO_V14_SQL: &str = include_str!("../../migrations/v13_to_v14.sql");
const V14_TO_V15_SQL: &str = include_str!("../../migrations/v14_to_v15.sql");

pub(crate) fn ensure_schema(conn: &Connection) -> Result<(), StorageError> {
    if !table_exists(conn, "schema_meta")? {
        run_init_schema(conn)?;
    }

    loop {
        let version = read_schema_version(conn)?;
        if version == SUPPORTED_SCHEMA_VERSION {
            return Ok(());
        }
        if version > SUPPORTED_SCHEMA_VERSION {
            return Err(StorageError::SchemaVersionTooNew {
                db: version,
                supported: SUPPORTED_SCHEMA_VERSION,
            });
        }
        apply_upgrade(conn, version)?;
    }
}

fn apply_upgrade(conn: &Connection, from: u32) -> Result<(), StorageError> {
    match from {
        1 => conn.execute_batch(V1_TO_V2_SQL).map_err(StorageError::from),
        2 => conn.execute_batch(V2_TO_V3_SQL).map_err(StorageError::from),
        3 => conn.execute_batch(V3_TO_V4_SQL).map_err(StorageError::from),
        4 => conn.execute_batch(V4_TO_V5_SQL).map_err(StorageError::from),
        5 => conn.execute_batch(V5_TO_V6_SQL).map_err(StorageError::from),
        6 => conn.execute_batch(V6_TO_V7_SQL).map_err(StorageError::from),
        7 => conn.execute_batch(V7_TO_V8_SQL).map_err(StorageError::from),
        8 => conn.execute_batch(V8_TO_V9_SQL).map_err(StorageError::from),
        9 => conn.execute_batch(V9_TO_V10_SQL).map_err(StorageError::from),
        10 => conn.execute_batch(V10_TO_V11_SQL).map_err(StorageError::from),
        11 => conn.execute_batch(V11_TO_V12_SQL).map_err(StorageError::from),
        12 => conn.execute_batch(V12_TO_V13_SQL).map_err(StorageError::from),
        13 => conn.execute_batch(V13_TO_V14_SQL).map_err(StorageError::from),
        14 => conn.execute_batch(V14_TO_V15_SQL).map_err(StorageError::from),
        other => Err(StorageError::InvalidSchemaMetadata {
            key: "schema_version".into(),
            value: format!("no upgrade path from version {other}"),
        }),
    }
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool, StorageError> {
    let row: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [name],
            |row| row.get(0),
        )
        .optional()?;
    Ok(row.is_some())
}

fn read_schema_version(conn: &Connection) -> Result<u32, StorageError> {
    let raw: String = conn.query_row(
        "SELECT value FROM schema_meta WHERE key = 'schema_version'",
        [],
        |row| row.get(0),
    )?;
    raw.parse::<u32>()
        .map_err(|_| StorageError::InvalidSchemaMetadata {
            key: "schema_version".into(),
            value: raw,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn
    }

    #[test]
    fn ensure_on_fresh_db_runs_init() {
        let conn = fresh_conn();
        ensure_schema(&conn).unwrap();
        let version = read_schema_version(&conn).unwrap();
        assert_eq!(version, SUPPORTED_SCHEMA_VERSION);
    }

    #[test]
    fn ensure_on_initialised_db_is_idempotent() {
        let conn = fresh_conn();
        ensure_schema(&conn).unwrap();
        ensure_schema(&conn).unwrap();
        ensure_schema(&conn).unwrap();
    }

    #[test]
    fn ensure_on_newer_version_aborts() {
        let conn = fresh_conn();
        ensure_schema(&conn).unwrap();
        conn.execute(
            "UPDATE schema_meta SET value = '99' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
        let err = ensure_schema(&conn).unwrap_err();
        assert!(matches!(
            err,
            StorageError::SchemaVersionTooNew {
                db: 99,
                supported: SUPPORTED_SCHEMA_VERSION,
            }
        ));
    }

    #[test]
    fn upgrade_adds_last_modified_revision_column_to_legacy_v3_db() {
        let conn = fresh_conn();
        run_init_schema(&conn).unwrap();
        conn.execute_batch(V1_TO_V2_SQL).unwrap();
        conn.execute_batch(V2_TO_V3_SQL).unwrap();
        let pre: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('symbols')")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            !pre.iter().any(|c| c == "last_modified_revision"),
            "v3 must NOT have the last_modified_revision column"
        );

        ensure_schema(&conn).unwrap();

        let post: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('symbols')")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            post.iter().any(|c| c == "last_modified_revision"),
            "v3→v4 upgrade must add the last_modified_revision column"
        );
        assert_eq!(
            read_schema_version(&conn).unwrap(),
            SUPPORTED_SCHEMA_VERSION
        );
    }

    #[test]
    fn upgrade_seeds_external_lockfile_hash_keys_on_legacy_v4_db() {
        let conn = fresh_conn();
        run_init_schema(&conn).unwrap();
        conn.execute_batch(V1_TO_V2_SQL).unwrap();
        conn.execute_batch(V2_TO_V3_SQL).unwrap();
        conn.execute_batch(V3_TO_V4_SQL).unwrap();
        let pre: Vec<String> = conn
            .prepare("SELECT key FROM schema_meta")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for key in [
            "external_cargo_lockfile_hash",
            "external_npm_lockfile_hash",
            "external_npm_lockfile_kind",
            "external_luarocks_hash",
        ] {
            assert!(
                !pre.iter().any(|k| k == key),
                "v4 must NOT have the {key} schema_meta row"
            );
        }

        ensure_schema(&conn).unwrap();

        let post: Vec<(String, String)> = conn
            .prepare("SELECT key, value FROM schema_meta")
            .unwrap()
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for key in [
            "external_cargo_lockfile_hash",
            "external_npm_lockfile_hash",
            "external_npm_lockfile_kind",
            "external_luarocks_hash",
        ] {
            let row = post
                .iter()
                .find(|(k, _)| k == key)
                .unwrap_or_else(|| panic!("v4→v5 must seed {key}"));
            assert_eq!(
                row.1, "",
                "{key} must default to blank (unset sentinel), got `{}`",
                row.1
            );
        }
        assert_eq!(
            read_schema_version(&conn).unwrap(),
            SUPPORTED_SCHEMA_VERSION
        );
    }

    #[test]
    fn upgrade_adds_confidence_column_to_legacy_v2_db() {
        let conn = fresh_conn();
        run_init_schema(&conn).unwrap();
        conn.execute_batch(V1_TO_V2_SQL).unwrap();
        let pre: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('edges')")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            !pre.iter().any(|c| c == "confidence"),
            "v2 must NOT have the confidence column"
        );

        ensure_schema(&conn).unwrap();

        let post: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('edges')")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            post.iter().any(|c| c == "confidence"),
            "v2→v3 upgrade must add the confidence column"
        );
        assert_eq!(
            read_schema_version(&conn).unwrap(),
            SUPPORTED_SCHEMA_VERSION
        );
    }

    #[test]
    fn upgrade_creates_call_sites_table_for_legacy_v9_db() {
        // IR-4-f: v9 has no `call_sites` table. After v9→v10 it must
        // exist with the documented columns + 3 indexes. Tested
        // against a fresh-then-rolled-back-to-v9 db so the upgrade
        // path runs in isolation.
        let conn = fresh_conn();
        run_init_schema(&conn).unwrap();
        conn.execute_batch(V1_TO_V2_SQL).unwrap();
        conn.execute_batch(V2_TO_V3_SQL).unwrap();
        conn.execute_batch(V3_TO_V4_SQL).unwrap();
        conn.execute_batch(V4_TO_V5_SQL).unwrap();
        conn.execute_batch(V5_TO_V6_SQL).unwrap();
        conn.execute_batch(V6_TO_V7_SQL).unwrap();
        conn.execute_batch(V7_TO_V8_SQL).unwrap();
        conn.execute_batch(V8_TO_V9_SQL).unwrap();
        assert!(
            !table_exists(&conn, "call_sites").unwrap(),
            "v9 must NOT have the call_sites table"
        );

        ensure_schema(&conn).unwrap();

        assert!(
            table_exists(&conn, "call_sites").unwrap(),
            "v9→v10 upgrade must create the call_sites table"
        );
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('call_sites')")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for expected in [
            "id",
            "from_fqdn",
            "callee_text",
            "args_json",
            "receiver_chain_json",
            "file_path",
            "line",
            "col",
        ] {
            assert!(
                cols.iter().any(|c| c == expected),
                "v9→v10 must add column {expected}, got {cols:?}"
            );
        }
        let indexes: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'call_sites'")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for expected in [
            "idx_call_sites_from_fqdn",
            "idx_call_sites_callee_text",
            "idx_call_sites_file_path",
        ] {
            assert!(
                indexes.iter().any(|i| i == expected),
                "v9→v10 must create index {expected}, got {indexes:?}"
            );
        }
        assert_eq!(
            read_schema_version(&conn).unwrap(),
            SUPPORTED_SCHEMA_VERSION
        );
    }

    #[test]
    fn upgrade_adds_attributes_column_to_legacy_v1_db() {
        let conn = fresh_conn();
        // Bootstrap the historical v1 schema directly (no `attributes` column).
        run_init_schema(&conn).unwrap();
        let pre: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('edges')")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            !pre.iter().any(|c| c == "attributes"),
            "v1 init must NOT seed the attributes column"
        );

        ensure_schema(&conn).unwrap();

        let post: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('edges')")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            post.iter().any(|c| c == "attributes"),
            "v1→v2 upgrade must add the attributes column"
        );
        assert_eq!(
            read_schema_version(&conn).unwrap(),
            SUPPORTED_SCHEMA_VERSION
        );
    }

    #[test]
    fn upgrade_adds_module_lookups_workspace_imports_and_catalog_on_legacy_v6_db() {
        let conn = fresh_conn();
        run_init_schema(&conn).unwrap();
        conn.execute_batch(V1_TO_V2_SQL).unwrap();
        conn.execute_batch(V2_TO_V3_SQL).unwrap();
        conn.execute_batch(V3_TO_V4_SQL).unwrap();
        conn.execute_batch(V4_TO_V5_SQL).unwrap();
        conn.execute_batch(V5_TO_V6_SQL).unwrap();
        for t in ["module_lookups", "workspace_imports", "workspace_catalog"] {
            assert!(
                !table_exists(&conn, t).unwrap(),
                "v6 must NOT have the {t} table"
            );
        }

        ensure_schema(&conn).unwrap();

        for t in ["module_lookups", "workspace_imports", "workspace_catalog"] {
            assert!(
                table_exists(&conn, t).unwrap(),
                "v6\u{2192}v7 must add the {t} table"
            );
        }
        // Verify the link_direction CHECK constraint is enforced.
        let invalid_direction = conn.execute(
            "INSERT INTO workspace_catalog (workspace_id, root_path, link_direction, linked_at) \
             VALUES ('uuid-test', '/some/path', 9, 0)",
            [],
        );
        assert!(
            invalid_direction.is_err(),
            "link_direction CHECK must reject values outside (0, 1, 2)"
        );
        assert_eq!(
            read_schema_version(&conn).unwrap(),
            SUPPORTED_SCHEMA_VERSION
        );
    }

    #[test]
    fn upgrade_adds_projects_table_and_files_project_id_column_on_legacy_v7_db() {
        let conn = fresh_conn();
        run_init_schema(&conn).unwrap();
        conn.execute_batch(V1_TO_V2_SQL).unwrap();
        conn.execute_batch(V2_TO_V3_SQL).unwrap();
        conn.execute_batch(V3_TO_V4_SQL).unwrap();
        conn.execute_batch(V4_TO_V5_SQL).unwrap();
        conn.execute_batch(V5_TO_V6_SQL).unwrap();
        conn.execute_batch(V6_TO_V7_SQL).unwrap();
        assert!(
            !table_exists(&conn, "projects").unwrap(),
            "v7 must NOT have the projects table"
        );
        let pre_files_cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('files')")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            !pre_files_cols.iter().any(|c| c == "project_id"),
            "v7 files table must NOT have the project_id column"
        );

        ensure_schema(&conn).unwrap();

        assert!(
            table_exists(&conn, "projects").unwrap(),
            "v7→v8 must add the projects table"
        );
        let post_files_cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('files')")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            post_files_cols.iter().any(|c| c == "project_id"),
            "v7→v8 must add files.project_id column, got {post_files_cols:?}"
        );

        // Confirm UNIQUE constraint on projects.root_path is enforced.
        conn.execute(
            "INSERT INTO projects (label, kind, root_path, rel_path) \
             VALUES ('foo', 'rust', '/a/b', '.')",
            [],
        )
        .unwrap();
        let dup = conn.execute(
            "INSERT INTO projects (label, kind, root_path, rel_path) \
             VALUES ('bar', 'rust', '/a/b', './sub')",
            [],
        );
        assert!(
            dup.is_err(),
            "UNIQUE on projects.root_path must reject duplicate insert"
        );
        assert_eq!(
            read_schema_version(&conn).unwrap(),
            SUPPORTED_SCHEMA_VERSION
        );
    }

    #[test]
    fn upgrade_adds_flags_column_to_symbols_on_legacy_v8_db() {
        let conn = fresh_conn();
        run_init_schema(&conn).unwrap();
        conn.execute_batch(V1_TO_V2_SQL).unwrap();
        conn.execute_batch(V2_TO_V3_SQL).unwrap();
        conn.execute_batch(V3_TO_V4_SQL).unwrap();
        conn.execute_batch(V4_TO_V5_SQL).unwrap();
        conn.execute_batch(V5_TO_V6_SQL).unwrap();
        conn.execute_batch(V6_TO_V7_SQL).unwrap();
        conn.execute_batch(V7_TO_V8_SQL).unwrap();
        let pre: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('symbols')")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            !pre.iter().any(|c| c == "flags"),
            "v8 must NOT have the flags column"
        );

        ensure_schema(&conn).unwrap();

        let post: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('symbols')")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            post.iter().any(|c| c == "flags"),
            "v8→v9 upgrade must add the flags column, got {post:?}"
        );
        // Default for legacy rows: '[]' (empty JSON array).
        conn.execute(
            "INSERT INTO files (path, content_hash, language, last_scanned, byte_size, is_external) \
             VALUES ('src/x.rs', 'h', 'rust', 0, 0, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO symbols (\
                fqdn, name, kind, language_kind, language, module, visibility, \
                file_path, start_line, end_line, start_col, end_col, \
                signature_json, body_hash, is_external, source_origin, \
                last_modified_revision\
             ) VALUES ('x::f', 'f', 'function', 'fn', 'rust', NULL, 'public', \
                'src/x.rs', 0, 0, 0, 0, NULL, NULL, 0, 'workspace', 0)",
            [],
        )
        .unwrap();
        let default_flags: String = conn
            .query_row("SELECT flags FROM symbols WHERE fqdn = 'x::f'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(default_flags, "[]", "legacy-row insertion must default flags to '[]'");
        assert_eq!(
            read_schema_version(&conn).unwrap(),
            SUPPORTED_SCHEMA_VERSION
        );
    }

    #[test]
    fn upgrade_adds_workspace_id_column_to_symbols_on_legacy_v10_db() {
        // Stage 3b-7-b Layer 1: v10 has symbols without workspace_id.
        // After v10→v11 the column must exist, default to 'primary' for
        // legacy rows, and the (workspace_id, fqdn) composite index
        // must be present for Layer-2 scope-aware queries.
        let conn = fresh_conn();
        run_init_schema(&conn).unwrap();
        conn.execute_batch(V1_TO_V2_SQL).unwrap();
        conn.execute_batch(V2_TO_V3_SQL).unwrap();
        conn.execute_batch(V3_TO_V4_SQL).unwrap();
        conn.execute_batch(V4_TO_V5_SQL).unwrap();
        conn.execute_batch(V5_TO_V6_SQL).unwrap();
        conn.execute_batch(V6_TO_V7_SQL).unwrap();
        conn.execute_batch(V7_TO_V8_SQL).unwrap();
        conn.execute_batch(V8_TO_V9_SQL).unwrap();
        conn.execute_batch(V9_TO_V10_SQL).unwrap();
        let pre: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('symbols')")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            !pre.iter().any(|c| c == "workspace_id"),
            "v10 must NOT have the workspace_id column on symbols"
        );

        // Seed a legacy v10 symbol row to verify the default lands.
        conn.execute(
            "INSERT INTO files (path, content_hash, language, last_scanned, byte_size, is_external) \
             VALUES ('src/x.rs', 'h', 'rust', 0, 0, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO symbols (\
                fqdn, name, kind, language_kind, language, module, visibility, \
                file_path, start_line, end_line, start_col, end_col, \
                signature_json, body_hash, is_external, source_origin, \
                last_modified_revision, flags\
             ) VALUES ('x::f', 'f', 'function', 'fn', 'rust', NULL, 'public', \
                'src/x.rs', 0, 0, 0, 0, NULL, NULL, 0, 'workspace', 0, '[]')",
            [],
        )
        .unwrap();

        ensure_schema(&conn).unwrap();

        let post: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('symbols')")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            post.iter().any(|c| c == "workspace_id"),
            "v10→v11 must add the workspace_id column, got {post:?}"
        );

        let workspace_id: String = conn
            .query_row(
                "SELECT workspace_id FROM symbols WHERE fqdn = 'x::f'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            workspace_id, "primary",
            "legacy v10 rows must default to workspace_id='primary' post-upgrade"
        );

        // Note: v10→v11 originally added an explicit
        // `idx_symbols_workspace_id_fqdn` index, but the subsequent
        // v11→v12 rebuild drops it in favour of the composite UNIQUE's
        // implicit `sqlite_autoindex_symbols_*` — equivalent lookup
        // path with less storage. Since `ensure_schema` runs the full
        // chain up to SUPPORTED_SCHEMA_VERSION, we assert on what
        // survives at v12 (the composite UNIQUE constraint behaviour
        // is covered by the dedicated v11→v12 rebuild test).
        assert_eq!(
            read_schema_version(&conn).unwrap(),
            SUPPORTED_SCHEMA_VERSION
        );
    }

    #[test]
    fn upgrade_rebuilds_symbols_table_with_composite_unique_on_legacy_v11_db() {
        // Stage 3b-7-b Layer 3a: v11 has `UNIQUE (fqdn)` alone. The
        // v11→v12 rebuild swaps it for `UNIQUE (workspace_id, fqdn)`
        // while preserving every column, every existing row's id (FK
        // integrity), the FTS triggers, and the AUTOINCREMENT counter.
        let conn = fresh_conn();
        run_init_schema(&conn).unwrap();
        conn.execute_batch(V1_TO_V2_SQL).unwrap();
        conn.execute_batch(V2_TO_V3_SQL).unwrap();
        conn.execute_batch(V3_TO_V4_SQL).unwrap();
        conn.execute_batch(V4_TO_V5_SQL).unwrap();
        conn.execute_batch(V5_TO_V6_SQL).unwrap();
        conn.execute_batch(V6_TO_V7_SQL).unwrap();
        conn.execute_batch(V7_TO_V8_SQL).unwrap();
        conn.execute_batch(V8_TO_V9_SQL).unwrap();
        conn.execute_batch(V9_TO_V10_SQL).unwrap();
        conn.execute_batch(V10_TO_V11_SQL).unwrap();

        // Seed v11 fixtures: a file row, two symbols (assigned ids 1
        // and 2 by AUTOINCREMENT), and an edge referencing symbol 1
        // — the edge is the FK-integrity canary.
        conn.execute(
            "INSERT INTO files (path, content_hash, language, last_scanned, byte_size, is_external) \
             VALUES ('src/x.rs', 'h', 'rust', 0, 0, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO symbols (\
                fqdn, name, kind, language_kind, language, module, visibility, \
                file_path, start_line, end_line, start_col, end_col, \
                signature_json, body_hash, is_external, source_origin, \
                last_modified_revision, flags, workspace_id\
             ) VALUES ('x::caller', 'caller', 'function', 'fn', 'rust', NULL, 'public', \
                'src/x.rs', 0, 0, 0, 0, NULL, NULL, 0, 'workspace', 0, '[]', 'primary')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO symbols (\
                fqdn, name, kind, language_kind, language, module, visibility, \
                file_path, start_line, end_line, start_col, end_col, \
                signature_json, body_hash, is_external, source_origin, \
                last_modified_revision, flags, workspace_id\
             ) VALUES ('x::callee', 'callee', 'function', 'fn', 'rust', NULL, 'public', \
                'src/x.rs', 0, 0, 0, 0, NULL, NULL, 0, 'workspace', 0, '[]', 'primary')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO edges (from_symbol_id, kind, to_symbol_id) \
             VALUES (1, 'CALLS', 2)",
            [],
        )
        .unwrap();

        // Verify the v11 invariant we want to relax: same fqdn can't
        // be inserted twice regardless of workspace_id.
        let pre = conn.execute(
            "INSERT INTO symbols (\
                fqdn, name, kind, language_kind, language, module, visibility, \
                file_path, start_line, end_line, start_col, end_col, \
                signature_json, body_hash, is_external, source_origin, \
                last_modified_revision, flags, workspace_id\
             ) VALUES ('x::caller', 'caller', 'function', 'fn', 'rust', NULL, 'public', \
                'src/x.rs', 0, 0, 0, 0, NULL, NULL, 0, 'workspace', 0, '[]', 'peer-uuid-1')",
            [],
        );
        assert!(
            pre.is_err(),
            "v11 must reject same-fqdn insert regardless of workspace_id"
        );

        ensure_schema(&conn).unwrap();

        // 1. Schema version bumped.
        assert_eq!(read_schema_version(&conn).unwrap(), SUPPORTED_SCHEMA_VERSION);

        // 2. Existing rows preserved with same ids — the FK canary
        //    (edge) still resolves both endpoints.
        let preserved_ids: Vec<i64> = conn
            .prepare("SELECT id FROM symbols ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get::<_, i64>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            preserved_ids,
            vec![1, 2],
            "ids must be preserved exactly through the rebuild"
        );
        let edge_endpoints: (i64, i64) = conn
            .query_row(
                "SELECT from_symbol_id, to_symbol_id FROM edges WHERE from_symbol_id = 1",
                [],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
            )
            .unwrap();
        assert_eq!(edge_endpoints, (1, 2), "edge FK refs must survive rebuild");

        // 3. PRAGMA foreign_key_check returns zero violation rows.
        let fk_violations: Vec<(String, i64)> = conn
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            fk_violations.is_empty(),
            "PRAGMA foreign_key_check must return no violations, got {fk_violations:?}"
        );

        // 4. UNIQUE constraint is now COMPOSITE — same fqdn under a
        //    different workspace_id is allowed.
        conn.execute(
            "INSERT INTO symbols (\
                fqdn, name, kind, language_kind, language, module, visibility, \
                file_path, start_line, end_line, start_col, end_col, \
                signature_json, body_hash, is_external, source_origin, \
                last_modified_revision, flags, workspace_id\
             ) VALUES ('x::caller', 'caller', 'function', 'fn', 'rust', NULL, 'public', \
                'src/x.rs', 0, 0, 0, 0, NULL, NULL, 0, 'workspace', 0, '[]', 'peer-uuid-1')",
            [],
        )
        .expect("v12: same fqdn, different workspace_id must be allowed");

        // 5. UNIQUE still enforces against duplicate (workspace_id, fqdn).
        let dup = conn.execute(
            "INSERT INTO symbols (\
                fqdn, name, kind, language_kind, language, module, visibility, \
                file_path, start_line, end_line, start_col, end_col, \
                signature_json, body_hash, is_external, source_origin, \
                last_modified_revision, flags, workspace_id\
             ) VALUES ('x::caller', 'caller', 'function', 'fn', 'rust', NULL, 'public', \
                'src/x.rs', 0, 0, 0, 0, NULL, NULL, 0, 'workspace', 0, '[]', 'peer-uuid-1')",
            [],
        );
        assert!(
            dup.is_err(),
            "v12 must reject duplicate (workspace_id, fqdn) — composite UNIQUE"
        );

        // 6. FTS triggers were recreated — inserting a new symbol
        //    pushes it into the FTS index, and an MATCH query finds it.
        conn.execute(
            "INSERT INTO symbols (\
                fqdn, name, kind, language_kind, language, module, visibility, \
                file_path, start_line, end_line, start_col, end_col, \
                signature_json, body_hash, is_external, source_origin, \
                last_modified_revision, flags, workspace_id\
             ) VALUES ('x::distinctive_marker', 'distinctive_marker', 'function', 'fn', 'rust', NULL, 'public', \
                'src/x.rs', 0, 0, 0, 0, NULL, NULL, 0, 'workspace', 0, '[]', 'primary')",
            [],
        )
        .unwrap();
        let fts_hit: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM symbols_fts WHERE symbols_fts MATCH 'distinctive_marker'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            fts_hit, 1,
            "post-rebuild FTS triggers must still index new inserts"
        );

        // 7. AUTOINCREMENT state sane — next INSERT lands at MAX(id)+1
        //    rather than restarting from 1 or jumping wildly.
        let next_id: i64 = conn
            .query_row(
                "SELECT MAX(id) FROM symbols",
                [],
                |r| r.get(0),
            )
            .unwrap();
        // We inserted: 2 v11 rows, 1 peer-uuid clone, 1 distinctive_marker
        // → ids 1, 2, 3, 4. The sqlite_sequence must reflect this so
        // the NEXT INSERT goes to 5 (not 1, not 2).
        assert_eq!(next_id, 4, "AUTOINCREMENT bookkeeping must reflect every inserted row");
        let seq: Option<i64> = conn
            .query_row(
                "SELECT seq FROM sqlite_sequence WHERE name = 'symbols'",
                [],
                |r| r.get(0),
            )
            .ok();
        assert_eq!(
            seq,
            Some(4),
            "sqlite_sequence must be re-keyed to 'symbols' with the correct counter"
        );

        // 8. All the expected indexes are present after rebuild (the
        //    explicit idx_symbols_workspace_id_fqdn is dropped on
        //    purpose: the composite UNIQUE auto-index supersedes it).
        let indexes: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'symbols'")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for expected in [
            "idx_symbols_language",
            "idx_symbols_kind",
            "idx_symbols_name",
            "idx_symbols_file_path",
            "idx_symbols_module",
            "idx_symbols_is_external",
            "idx_symbols_last_modified_revision",
        ] {
            assert!(
                indexes.iter().any(|i| i == expected),
                "v11→v12 rebuild must recreate {expected}, got {indexes:?}"
            );
        }
    }

    #[test]
    fn upgrade_adds_indexing_mode_column_to_workspace_catalog_on_legacy_v12_db() {
        // Stage 3b-7-b Layer 3c: v12 has workspace_catalog without
        // indexing_mode. The v12→v13 migration adds the column with
        // CHECK ('blob_import', 'extract') and default 'blob_import'
        // so legacy rows preserve 3b-7-a behaviour until users opt
        // a peer into autonomous extraction.
        let conn = fresh_conn();
        run_init_schema(&conn).unwrap();
        conn.execute_batch(V1_TO_V2_SQL).unwrap();
        conn.execute_batch(V2_TO_V3_SQL).unwrap();
        conn.execute_batch(V3_TO_V4_SQL).unwrap();
        conn.execute_batch(V4_TO_V5_SQL).unwrap();
        conn.execute_batch(V5_TO_V6_SQL).unwrap();
        conn.execute_batch(V6_TO_V7_SQL).unwrap();
        conn.execute_batch(V7_TO_V8_SQL).unwrap();
        conn.execute_batch(V8_TO_V9_SQL).unwrap();
        conn.execute_batch(V9_TO_V10_SQL).unwrap();
        conn.execute_batch(V10_TO_V11_SQL).unwrap();
        conn.execute_batch(V11_TO_V12_SQL).unwrap();

        // Seed a legacy v12 workspace_catalog row.
        conn.execute(
            "INSERT INTO workspace_catalog (workspace_id, root_path, link_direction, linked_at) \
             VALUES ('legacy-peer-uuid', '/some/path', 0, 0)",
            [],
        )
        .unwrap();

        let pre: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('workspace_catalog')")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            !pre.iter().any(|c| c == "indexing_mode"),
            "v12 must NOT have the indexing_mode column"
        );

        ensure_schema(&conn).unwrap();

        let post: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('workspace_catalog')")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            post.iter().any(|c| c == "indexing_mode"),
            "v12→v13 must add the indexing_mode column, got {post:?}"
        );

        // Legacy row defaults to blob_import.
        let mode: String = conn
            .query_row(
                "SELECT indexing_mode FROM workspace_catalog WHERE workspace_id = ?1",
                ["legacy-peer-uuid"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            mode, "blob_import",
            "legacy v12 rows must default to 'blob_import' post-upgrade"
        );

        // CHECK constraint rejects unknown modes.
        let bad = conn.execute(
            "INSERT INTO workspace_catalog (workspace_id, root_path, link_direction, linked_at, indexing_mode) \
             VALUES ('bad-peer', '/other/path', 0, 0, 'magic')",
            [],
        );
        assert!(
            bad.is_err(),
            "indexing_mode CHECK must reject values outside ('blob_import', 'extract')"
        );

        // 'extract' is accepted.
        conn.execute(
            "INSERT INTO workspace_catalog (workspace_id, root_path, link_direction, linked_at, indexing_mode) \
             VALUES ('opt-in-peer', '/other/path', 0, 0, 'extract')",
            [],
        )
        .expect("'extract' must be accepted by the CHECK constraint");

        assert_eq!(read_schema_version(&conn).unwrap(), SUPPORTED_SCHEMA_VERSION);
    }

    #[test]
    fn upgrade_creates_symbol_decl_location_table_on_legacy_v13_db() {
        // Stage 1c: v13 has no `symbol_decl_location` table. After
        // v13→v14, the table must exist with the documented 1:1
        // PRIMARY KEY shape + ON DELETE CASCADE FK to symbols, and a
        // `file` index to support watcher reverse-lookup.
        let conn = fresh_conn();
        run_init_schema(&conn).unwrap();
        conn.execute_batch(V1_TO_V2_SQL).unwrap();
        conn.execute_batch(V2_TO_V3_SQL).unwrap();
        conn.execute_batch(V3_TO_V4_SQL).unwrap();
        conn.execute_batch(V4_TO_V5_SQL).unwrap();
        conn.execute_batch(V5_TO_V6_SQL).unwrap();
        conn.execute_batch(V6_TO_V7_SQL).unwrap();
        conn.execute_batch(V7_TO_V8_SQL).unwrap();
        conn.execute_batch(V8_TO_V9_SQL).unwrap();
        conn.execute_batch(V9_TO_V10_SQL).unwrap();
        conn.execute_batch(V10_TO_V11_SQL).unwrap();
        conn.execute_batch(V11_TO_V12_SQL).unwrap();
        conn.execute_batch(V12_TO_V13_SQL).unwrap();
        assert!(
            !table_exists(&conn, "symbol_decl_location").unwrap(),
            "v13 must NOT have the symbol_decl_location table"
        );

        ensure_schema(&conn).unwrap();

        assert!(
            table_exists(&conn, "symbol_decl_location").unwrap(),
            "v13→v14 must create the symbol_decl_location table"
        );

        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('symbol_decl_location')")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for expected in ["symbol_id", "file", "start_line", "end_line", "start_col", "end_col"] {
            assert!(
                cols.iter().any(|c| c == expected),
                "v13→v14 must add column {expected}, got {cols:?}"
            );
        }

        let indexes: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'symbol_decl_location'",
            )
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            indexes.iter().any(|i| i == "idx_symbol_decl_location_file"),
            "v13→v14 must create idx_symbol_decl_location_file, got {indexes:?}"
        );

        // FK CASCADE: inserting a row referencing a symbol, then
        // deleting the symbol, must drop the decl_location row too.
        conn.execute(
            "INSERT INTO files (path, content_hash, language, last_scanned, byte_size, is_external) \
             VALUES ('src/x.c', 'h', 'c', 0, 0, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO symbols (\
                fqdn, name, kind, language_kind, language, module, visibility, \
                file_path, start_line, end_line, start_col, end_col, \
                signature_json, body_hash, is_external, source_origin, \
                last_modified_revision, flags, workspace_id\
             ) VALUES ('x::foo', 'foo', 'function', 'fn', 'c', NULL, 'public', \
                'src/x.c', 1, 5, 0, 1, NULL, NULL, 0, 'workspace', 0, '[]', 'primary')",
            [],
        )
        .unwrap();
        let sym_id: i64 = conn
            .query_row("SELECT id FROM symbols WHERE fqdn = 'x::foo'", [], |r| {
                r.get(0)
            })
            .unwrap();
        conn.execute(
            "INSERT INTO symbol_decl_location (symbol_id, file, start_line, end_line, start_col, end_col) \
             VALUES (?1, 'include/x.h', 10, 10, 0, 30)",
            [sym_id],
        )
        .unwrap();
        let count_before: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbol_decl_location", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count_before, 1);

        // Enable FK enforcement (off by default in rusqlite test conn).
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute("DELETE FROM symbols WHERE id = ?1", [sym_id])
            .unwrap();
        let count_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbol_decl_location", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            count_after, 0,
            "ON DELETE CASCADE must drop the decl_location row when its symbol is deleted"
        );

        assert_eq!(read_schema_version(&conn).unwrap(), SUPPORTED_SCHEMA_VERSION);
    }

    #[test]
    fn upgrade_creates_symbol_ffi_binding_table_on_legacy_v14_db() {
        // Stage 2: v14 has no symbol_ffi_binding table. After v14→v15
        // it exists with the composite PK, FK CASCADE, CHECK on
        // direction, and two indexes.
        let conn = fresh_conn();
        run_init_schema(&conn).unwrap();
        for sql in [
            V1_TO_V2_SQL,
            V2_TO_V3_SQL,
            V3_TO_V4_SQL,
            V4_TO_V5_SQL,
            V5_TO_V6_SQL,
            V6_TO_V7_SQL,
            V7_TO_V8_SQL,
            V8_TO_V9_SQL,
            V9_TO_V10_SQL,
            V10_TO_V11_SQL,
            V11_TO_V12_SQL,
            V12_TO_V13_SQL,
            V13_TO_V14_SQL,
        ] {
            conn.execute_batch(sql).unwrap();
        }
        assert!(
            !table_exists(&conn, "symbol_ffi_binding").unwrap(),
            "v14 must NOT have the symbol_ffi_binding table"
        );

        ensure_schema(&conn).unwrap();

        assert!(
            table_exists(&conn, "symbol_ffi_binding").unwrap(),
            "v14→v15 must create the symbol_ffi_binding table"
        );

        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('symbol_ffi_binding')")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for expected in ["symbol_id", "abi", "direction", "abi_name", "convention"] {
            assert!(
                cols.iter().any(|c| c == expected),
                "v14→v15 must add column {expected}, got {cols:?}"
            );
        }

        let indexes: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'symbol_ffi_binding'",
            )
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for expected in [
            "idx_symbol_ffi_binding_lookup",
            "idx_symbol_ffi_binding_symbol",
        ] {
            assert!(
                indexes.iter().any(|i| i == expected),
                "v14→v15 must create index {expected}, got {indexes:?}"
            );
        }

        // CHECK constraint rejects unknown direction.
        conn.execute(
            "INSERT INTO files (path, content_hash, language, last_scanned, byte_size, is_external) \
             VALUES ('src/x.c', 'h', 'c', 0, 0, 0)",
            [],
        )
        .unwrap();
        let sid: i64 = conn
            .query_row(
                "INSERT INTO symbols (\
                    fqdn, name, kind, language_kind, language, module, visibility, \
                    file_path, start_line, end_line, start_col, end_col, \
                    signature_json, body_hash, is_external, source_origin, \
                    last_modified_revision, flags, workspace_id\
                 ) VALUES ('x::foo', 'foo', 'function', 'fn', 'c', NULL, 'public', \
                    'src/x.c', 1, 5, 0, 1, NULL, NULL, 0, 'workspace', 0, '[]', 'primary') \
                 RETURNING id",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap();
        let bad = conn.execute(
            "INSERT INTO symbol_ffi_binding (symbol_id, abi, direction, abi_name) \
             VALUES (?1, 'c', 'sideways', 'foo')",
            [sid],
        );
        assert!(
            bad.is_err(),
            "direction CHECK must reject values outside ('export', 'import')"
        );

        // Valid insert + FK CASCADE on parent deletion.
        conn.execute(
            "INSERT INTO symbol_ffi_binding (symbol_id, abi, direction, abi_name) \
             VALUES (?1, 'c', 'export', 'foo')",
            [sid],
        )
        .expect("'export' direction must be accepted");
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute("DELETE FROM symbols WHERE id = ?1", [sid]).unwrap();
        let count_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbol_ffi_binding", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            count_after, 0,
            "ON DELETE CASCADE must drop ffi binding when its parent symbol is deleted"
        );

        assert_eq!(read_schema_version(&conn).unwrap(), SUPPORTED_SCHEMA_VERSION);
    }

    #[test]
    fn ensure_on_garbage_version_errors() {
        let conn = fresh_conn();
        ensure_schema(&conn).unwrap();
        conn.execute(
            "UPDATE schema_meta SET value = 'banana' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
        let err = ensure_schema(&conn).unwrap_err();
        assert!(matches!(err, StorageError::InvalidSchemaMetadata { .. }));
    }

    #[test]
    fn table_exists_returns_true_after_init() {
        let conn = fresh_conn();
        run_init_schema(&conn).unwrap();
        assert!(table_exists(&conn, "schema_meta").unwrap());
        assert!(table_exists(&conn, "symbols").unwrap());
        assert!(!table_exists(&conn, "no_such_table").unwrap());
    }
}

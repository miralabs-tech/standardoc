use rusqlite::{Connection, OptionalExtension};

use crate::storage::error::StorageError;
use crate::storage::init::run_init_schema;

pub const SUPPORTED_SCHEMA_VERSION: u32 = 9;

const V1_TO_V2_SQL: &str = include_str!("../../migrations/v1_to_v2.sql");
const V2_TO_V3_SQL: &str = include_str!("../../migrations/v2_to_v3.sql");
const V3_TO_V4_SQL: &str = include_str!("../../migrations/v3_to_v4.sql");
const V4_TO_V5_SQL: &str = include_str!("../../migrations/v4_to_v5.sql");
const V5_TO_V6_SQL: &str = include_str!("../../migrations/v5_to_v6.sql");
const V6_TO_V7_SQL: &str = include_str!("../../migrations/v6_to_v7.sql");
const V7_TO_V8_SQL: &str = include_str!("../../migrations/v7_to_v8.sql");
const V8_TO_V9_SQL: &str = include_str!("../../migrations/v8_to_v9.sql");

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

use rusqlite::{Connection, OptionalExtension};

use crate::storage::error::StorageError;
use crate::storage::init::run_init_schema;

pub(crate) const SUPPORTED_SCHEMA_VERSION: u32 = 2;

const V1_TO_V2_SQL: &str = include_str!("../../migrations/v1_to_v2.sql");

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
                supported: 2,
            }
        ));
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
        assert_eq!(read_schema_version(&conn).unwrap(), 2);
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

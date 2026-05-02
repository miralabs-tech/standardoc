use std::cmp::Ordering;

use rusqlite::{Connection, OptionalExtension};

use crate::storage::error::StorageError;
use crate::storage::init::run_init_schema;

pub(crate) const SUPPORTED_SCHEMA_VERSION: u32 = 1;

pub(crate) fn ensure_schema(conn: &Connection) -> Result<(), StorageError> {
    if !table_exists(conn, "schema_meta")? {
        return run_init_schema(conn);
    }

    let version = read_schema_version(conn)?;
    match version.cmp(&SUPPORTED_SCHEMA_VERSION) {
        Ordering::Equal | Ordering::Less => Ok(()),
        Ordering::Greater => Err(StorageError::SchemaVersionTooNew {
            db: version,
            supported: SUPPORTED_SCHEMA_VERSION,
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
                supported: 1,
            }
        ));
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

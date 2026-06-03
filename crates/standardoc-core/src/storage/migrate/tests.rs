use super::*;
use crate::storage::schema_meta::read_schema_version;

fn fresh_conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    conn
}

fn table_count(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )
    .unwrap()
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
    let count_before = table_count(&conn);
    ensure_schema(&conn).unwrap();
    ensure_schema(&conn).unwrap();
    let count_after = table_count(&conn);
    assert_eq!(count_before, count_after);
}

#[test]
fn ensure_resets_db_on_older_version() {
    let conn = fresh_conn();
    ensure_schema(&conn).unwrap();
    // Pretend the DB was written by a pre-v0 binary.
    conn.execute(
        "UPDATE schema_meta SET value = '99' WHERE key = 'schema_version'",
        [],
    )
    .unwrap();
    // Insert a row to verify the reset wipes it.
    conn.execute(
        "INSERT INTO projects (label, kind, root_path, rel_path) \
             VALUES ('stale', 'test', '/x', '.')",
        [],
    )
    .unwrap();
    ensure_schema(&conn).unwrap();
    assert_eq!(
        read_schema_version(&conn).unwrap(),
        SUPPORTED_SCHEMA_VERSION
    );
    let leftover: i64 = conn
        .query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))
        .unwrap();
    assert_eq!(leftover, 0, "reset must wipe user data");
}

#[test]
fn ensure_resets_db_when_only_schema_meta_table_exists() {
    let conn = fresh_conn();
    conn.execute_batch(
        "CREATE TABLE schema_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL); \
             INSERT INTO schema_meta (key, value) VALUES ('schema_version', '7');",
    )
    .unwrap();
    ensure_schema(&conn).unwrap();
    assert_eq!(
        read_schema_version(&conn).unwrap(),
        SUPPORTED_SCHEMA_VERSION
    );
    // Real v0 has many tables — proxy for "reset happened".
    assert!(table_count(&conn) > 5);
}

#[test]
fn reset_handles_unknown_indexes_and_triggers() {
    let conn = fresh_conn();
    ensure_schema(&conn).unwrap();
    // Sprinkle a stray index + trigger that v0 doesn't define.
    conn.execute_batch(
        "CREATE INDEX idx_stray ON projects(label); \
             CREATE TRIGGER trig_stray AFTER INSERT ON projects BEGIN \
               SELECT 1; END;",
    )
    .unwrap();
    conn.execute(
        "UPDATE schema_meta SET value = '99' WHERE key = 'schema_version'",
        [],
    )
    .unwrap();
    ensure_schema(&conn).unwrap();
    let stray: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master \
                 WHERE name IN ('idx_stray', 'trig_stray')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stray, 0, "reset must drop unknown objects too");
}

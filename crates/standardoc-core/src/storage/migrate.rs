use rusqlite::{Connection, OptionalExtension};

use crate::storage::error::StorageError;
use crate::storage::init::run_init_schema;

/// Single supported schema version. Beta-era reboot — the v1→v15
/// chain inherited from earlier iterations was consolidated into
/// `init_v0.sql`. Bumping this constant requires either a forward
/// migration script OR the destructive reset path in `ensure_schema`
/// (today the latter; the index is a derived cache).
pub const SUPPORTED_SCHEMA_VERSION: u32 = 4;

/// Idempotent schema bootstrap. Behaviour by initial DB state:
///   - empty DB                              → run `init_v0.sql`
///   - DB at SUPPORTED_SCHEMA_VERSION        → no-op
///   - DB at a different (older OR newer) v → DROP every object,
///                                            re-run `init_v0.sql`
///
/// The reset path is destructive on purpose: this index is rebuilt
/// from the workspace on next cold-start, so we trade write cost for
/// zero migration-chain maintenance.
pub(crate) fn ensure_schema(conn: &Connection) -> Result<(), StorageError> {
    if !table_exists(conn, "schema_meta")? {
        run_init_schema(conn)?;
        return Ok(());
    }

    let version = read_schema_version(conn)?;
    if version == SUPPORTED_SCHEMA_VERSION {
        return Ok(());
    }

    eprintln!(
        "standardoc: schema version mismatch (db={version}, supported={SUPPORTED_SCHEMA_VERSION}) — destroying and rebuilding index; full reindex required on next cold-start"
    );
    reset_database(conn)?;
    run_init_schema(conn)?;
    Ok(())
}

/// Drop every user-visible object (tables, indexes, triggers, FTS
/// shadows). The set of objects is read from `sqlite_master` so this
/// works against any prior schema layout.
fn reset_database(conn: &Connection) -> Result<(), StorageError> {
    conn.execute("PRAGMA foreign_keys = OFF", [])?;

    let objects: Vec<(String, String)> = conn
        .prepare(
            "SELECT type, name FROM sqlite_master \
             WHERE name NOT LIKE 'sqlite_%' \
               AND type IN ('table', 'index', 'trigger', 'view') \
             ORDER BY CASE type \
                 WHEN 'trigger' THEN 0 \
                 WHEN 'view'    THEN 1 \
                 WHEN 'index'   THEN 2 \
                 WHEN 'table'   THEN 3 END",
        )?
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    for (ty, name) in objects {
        let stmt = match ty.as_str() {
            "table" => format!("DROP TABLE IF EXISTS \"{name}\""),
            "index" => format!("DROP INDEX IF EXISTS \"{name}\""),
            "trigger" => format!("DROP TRIGGER IF EXISTS \"{name}\""),
            "view" => format!("DROP VIEW IF EXISTS \"{name}\""),
            _ => continue,
        };
        conn.execute(&stmt, [])?;
    }

    conn.execute("PRAGMA foreign_keys = ON", [])?;
    Ok(())
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
            "UPDATE schema_meta SET value = '5' WHERE key = 'schema_version'",
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
}

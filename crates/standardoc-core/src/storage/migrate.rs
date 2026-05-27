use rusqlite::{Connection, OptionalExtension};

use crate::storage::error::StorageError;
use crate::storage::init::run_init_schema;

/// Single supported schema version. Beta-era reboot — the v1→v15
/// chain inherited from earlier iterations was consolidated into
/// `init_v0.sql`. Bumping this constant requires either a forward
/// migration script OR the destructive reset path in `ensure_schema`
/// (today the latter; the index is a derived cache).
pub const SUPPORTED_SCHEMA_VERSION: u32 = 5;

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
mod tests;

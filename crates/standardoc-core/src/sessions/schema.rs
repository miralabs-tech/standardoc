use rusqlite::Connection;

use super::SessionsError;

pub(super) const SUPPORTED_VERSION: u32 = 1;

const INIT_SQL: &str = "
CREATE TABLE IF NOT EXISTS schema_meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

INSERT OR IGNORE INTO schema_meta (key, value) VALUES ('schema_version', '1');

CREATE TABLE IF NOT EXISTS sessions (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  slug         TEXT    NOT NULL UNIQUE,
  body_md      TEXT    NOT NULL,
  supersedes   TEXT,
  status       TEXT    NOT NULL DEFAULT 'active'
                 CHECK (status IN ('active', 'superseded')),
  created_at   INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sessions_status     ON sessions(status);
CREATE INDEX IF NOT EXISTS idx_sessions_created_at ON sessions(created_at);
CREATE INDEX IF NOT EXISTS idx_sessions_supersedes ON sessions(supersedes) WHERE supersedes IS NOT NULL;
";

pub(super) fn ensure_schema(conn: &Connection) -> Result<(), SessionsError> {
    conn.execute_batch(INIT_SQL)?;
    let version = read_version(conn)?;
    if version > SUPPORTED_VERSION {
        return Err(SessionsError::SchemaVersionTooNew {
            db: version,
            supported: SUPPORTED_VERSION,
        });
    }
    // Future migrations will run here (v1 → v2, ...).
    Ok(())
}

fn read_version(conn: &Connection) -> Result<u32, SessionsError> {
    let raw: String = conn.query_row(
        "SELECT value FROM schema_meta WHERE key = 'schema_version'",
        [],
        |row| row.get(0),
    )?;
    raw.parse::<u32>()
        .map_err(|_| SessionsError::InvalidSchemaMetadata {
            key: "schema_version".into(),
            value: raw,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_schema_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_schema(&conn).unwrap();
        ensure_schema(&conn).unwrap();
        let version = read_version(&conn).unwrap();
        assert_eq!(version, SUPPORTED_VERSION);
    }

    #[test]
    fn schema_version_too_new_returns_error() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_schema(&conn).unwrap();
        conn.execute(
            "UPDATE schema_meta SET value = '99' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
        let err = ensure_schema(&conn).unwrap_err();
        assert!(matches!(
            err,
            SessionsError::SchemaVersionTooNew { db: 99, supported: 1 }
        ));
    }
}

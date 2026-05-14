use rusqlite::Connection;

use super::SessionsError;

pub(super) const SUPPORTED_VERSION: u32 = 3;

const INIT_SQL: &str = "
CREATE TABLE IF NOT EXISTS schema_meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

INSERT OR IGNORE INTO schema_meta (key, value) VALUES ('schema_version', '3');

CREATE TABLE IF NOT EXISTS sessions (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  slug         TEXT    NOT NULL UNIQUE,
  body_md      TEXT    NOT NULL,
  supersedes   TEXT,
  status       TEXT    NOT NULL DEFAULT 'active'
                 CHECK (status IN ('active', 'superseded')),
  kind         TEXT    NOT NULL DEFAULT 'session'
                 CHECK (kind IN ('session', 'feedback', 'profile', 'lock')),
  created_at   INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sessions_status     ON sessions(status);
CREATE INDEX IF NOT EXISTS idx_sessions_created_at ON sessions(created_at);
CREATE INDEX IF NOT EXISTS idx_sessions_supersedes ON sessions(supersedes) WHERE supersedes IS NOT NULL;

CREATE TABLE IF NOT EXISTS usage_stats (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  tool_name       TEXT    NOT NULL,
  fqdn            TEXT,
  bytes_out       INTEGER NOT NULL,
  baseline_bytes  INTEGER NOT NULL,
  ts_unix         INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_usage_stats_ts_unix   ON usage_stats(ts_unix);
CREATE INDEX IF NOT EXISTS idx_usage_stats_tool_name ON usage_stats(tool_name);
";

pub(super) fn ensure_schema(conn: &Connection) -> Result<(), SessionsError> {
    let version = read_or_init_version(conn)?;
    if version > SUPPORTED_VERSION {
        return Err(SessionsError::SchemaVersionTooNew {
            db: version,
            supported: SUPPORTED_VERSION,
        });
    }
    if version < 3 {
        // v2 → v3 : `kind` column on `sessions` for memory-sync infrastructure
        // (session | feedback | profile | lock).
        conn.execute(
            "ALTER TABLE sessions ADD COLUMN kind TEXT NOT NULL DEFAULT 'session' \
             CHECK (kind IN ('session', 'feedback', 'profile', 'lock'))",
            [],
        )?;
    }
    conn.execute_batch(INIT_SQL)?;
    if version < SUPPORTED_VERSION {
        conn.execute(
            "UPDATE schema_meta SET value = '3' WHERE key = 'schema_version'",
            [],
        )?;
    }
    Ok(())
}

fn read_or_init_version(conn: &Connection) -> Result<u32, SessionsError> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        [],
    )?;
    let existing: Option<String> = conn
        .query_row(
            "SELECT value FROM schema_meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .ok();
    match existing {
        Some(raw) => raw
            .parse::<u32>()
            .map_err(|_| SessionsError::InvalidSchemaMetadata {
                key: "schema_version".into(),
                value: raw,
            }),
        None => Ok(SUPPORTED_VERSION),
    }
}

#[cfg(test)]
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
            SessionsError::SchemaVersionTooNew {
                db: 99,
                supported: 3
            }
        ));
    }

    #[test]
    fn fresh_db_lands_on_v3() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_schema(&conn).unwrap();
        assert_eq!(read_version(&conn).unwrap(), 3);
    }

    #[test]
    fn v1_db_is_migrated_to_v3() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO schema_meta (key, value) VALUES ('schema_version', '1');
             CREATE TABLE sessions (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               slug TEXT NOT NULL UNIQUE,
               body_md TEXT NOT NULL,
               supersedes TEXT,
               status TEXT NOT NULL DEFAULT 'active',
               created_at INTEGER NOT NULL
             );",
        )
        .unwrap();
        ensure_schema(&conn).unwrap();
        assert_eq!(read_version(&conn).unwrap(), 3);
        // usage_stats now exists.
        conn.execute(
            "INSERT INTO usage_stats (tool_name, fqdn, bytes_out, baseline_bytes, ts_unix) \
             VALUES ('get_context', 'crate::x', 100, 1000, 1700000000)",
            [],
        )
        .unwrap();
        let cnt: i64 = conn
            .query_row("SELECT COUNT(*) FROM usage_stats", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cnt, 1);
        // kind column now exists with default 'session'.
        conn.execute(
            "INSERT INTO sessions (slug, body_md, created_at) VALUES ('post-migrate', 'x', 1)",
            [],
        )
        .unwrap();
        let kind: String = conn
            .query_row(
                "SELECT kind FROM sessions WHERE slug = 'post-migrate'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kind, "session");
    }

    #[test]
    fn v2_db_is_migrated_to_v3() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO schema_meta (key, value) VALUES ('schema_version', '2');
             CREATE TABLE sessions (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               slug TEXT NOT NULL UNIQUE,
               body_md TEXT NOT NULL,
               supersedes TEXT,
               status TEXT NOT NULL DEFAULT 'active',
               created_at INTEGER NOT NULL
             );
             CREATE TABLE usage_stats (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               tool_name TEXT NOT NULL,
               fqdn TEXT,
               bytes_out INTEGER NOT NULL,
               baseline_bytes INTEGER NOT NULL,
               ts_unix INTEGER NOT NULL
             );",
        )
        .unwrap();
        ensure_schema(&conn).unwrap();
        assert_eq!(read_version(&conn).unwrap(), 3);
        // Existing rows survive the migration with default kind = 'session'.
        conn.execute(
            "INSERT INTO sessions (slug, body_md, created_at) VALUES ('legacy', 'body', 1)",
            [],
        )
        .unwrap();
        let kind: String = conn
            .query_row("SELECT kind FROM sessions WHERE slug = 'legacy'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(kind, "session");
        // Each of the 4 day-1 kinds is accepted.
        for k in ["session", "feedback", "profile", "lock"] {
            conn.execute(
                "INSERT INTO sessions (slug, body_md, kind, created_at) VALUES (?, 'x', ?, 1)",
                rusqlite::params![format!("slug-{k}"), k],
            )
            .unwrap();
        }
        // An unknown kind is rejected by the CHECK constraint.
        let err = conn.execute(
            "INSERT INTO sessions (slug, body_md, kind, created_at) VALUES ('bad', 'x', 'decision', 1)",
            [],
        );
        assert!(err.is_err());
    }
}

use rusqlite::{Connection, OptionalExtension};
use standardoc_ir::SymbolLocation;

use crate::storage::error::StorageError;

/// Upsert the declaration-site location for a symbol. Replaces any prior
/// row for `symbol_id` — used by the C provider's `.h ↔ .c` join pass
/// when a function definition has been matched with a header prototype.
pub(crate) fn upsert_decl_location(
    conn: &Connection,
    symbol_id: i64,
    location: &SymbolLocation,
) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO symbol_decl_location (\
            symbol_id, file, start_line, end_line, start_col, end_col\
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT(symbol_id) DO UPDATE SET \
            file       = excluded.file, \
            start_line = excluded.start_line, \
            end_line   = excluded.end_line, \
            start_col  = excluded.start_col, \
            end_col    = excluded.end_col",
        rusqlite::params![
            symbol_id,
            &location.file,
            location.start_line,
            location.end_line,
            location.start_col,
            location.end_col,
        ],
    )?;
    Ok(())
}

/// Fetch the declaration-site location for a symbol, or `None` if the
/// symbol has no separate declaration (header-less, or never joined).
pub(crate) fn read_decl_location(
    conn: &Connection,
    symbol_id: i64,
) -> Result<Option<SymbolLocation>, StorageError> {
    let row = conn
        .query_row(
            "SELECT file, start_line, end_line, start_col, end_col \
             FROM symbol_decl_location WHERE symbol_id = ?1",
            [symbol_id],
            |r| {
                Ok(SymbolLocation {
                    file: r.get(0)?,
                    start_line: r.get(1)?,
                    end_line: r.get(2)?,
                    start_col: r.get(3)?,
                    end_col: r.get(4)?,
                })
            },
        )
        .optional()?;
    Ok(row)
}

/// Drop the declaration-site row for a symbol. Cheap because the table
/// is keyed by `symbol_id` PRIMARY KEY. Also runs automatically via
/// `ON DELETE CASCADE` when the parent `symbols` row is removed; the
/// explicit helper is exposed so the pipeline can purge stale joins
/// when a header file is unlinked from a workspace.
pub(crate) fn delete_decl_location(
    conn: &Connection,
    symbol_id: i64,
) -> Result<(), StorageError> {
    conn.execute(
        "DELETE FROM symbol_decl_location WHERE symbol_id = ?1",
        [symbol_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrate::ensure_schema;

    fn fresh_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        ensure_schema(&conn).unwrap();
        conn
    }

    fn seed_symbol(conn: &Connection, fqdn: &str, name: &str) -> i64 {
        conn.execute(
            "INSERT OR IGNORE INTO files (path, content_hash, language, last_scanned, byte_size, is_external) \
             VALUES ('src/x.c', 'h', 'c', 0, 0, 0)",
            [],
        )
        .unwrap();
        conn.query_row(
            "INSERT INTO symbols (\
                fqdn, name, kind, language_kind, language, module, visibility, \
                file_path, start_line, end_line, start_col, end_col, \
                signature_json, body_hash, is_external, source_origin, \
                last_modified_revision, flags, workspace_id\
             ) VALUES (?1, ?2, 'function', 'fn', 'c', NULL, 'public', \
                'src/x.c', 1, 5, 0, 1, NULL, NULL, 0, 'workspace', 0, '[]', 'primary') \
             RETURNING id",
            rusqlite::params![fqdn, name],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    }

    fn sample_loc() -> SymbolLocation {
        SymbolLocation {
            file: "include/x.h".into(),
            start_line: 10,
            end_line: 10,
            start_col: 0,
            end_col: 30,
        }
    }

    #[test]
    fn upsert_creates_row_when_absent() {
        let conn = fresh_conn();
        let sid = seed_symbol(&conn, "x::foo", "foo");
        upsert_decl_location(&conn, sid, &sample_loc()).unwrap();
        let got = read_decl_location(&conn, sid).unwrap().unwrap();
        assert_eq!(got, sample_loc());
    }

    #[test]
    fn upsert_overwrites_existing_row() {
        let conn = fresh_conn();
        let sid = seed_symbol(&conn, "x::foo", "foo");
        upsert_decl_location(&conn, sid, &sample_loc()).unwrap();
        let updated = SymbolLocation {
            file: "include/other.h".into(),
            start_line: 42,
            end_line: 42,
            start_col: 4,
            end_col: 12,
        };
        upsert_decl_location(&conn, sid, &updated).unwrap();
        let got = read_decl_location(&conn, sid).unwrap().unwrap();
        assert_eq!(got, updated);
    }

    #[test]
    fn read_returns_none_when_no_row() {
        let conn = fresh_conn();
        let sid = seed_symbol(&conn, "x::orphan", "orphan");
        assert!(read_decl_location(&conn, sid).unwrap().is_none());
    }

    #[test]
    fn delete_drops_row() {
        let conn = fresh_conn();
        let sid = seed_symbol(&conn, "x::foo", "foo");
        upsert_decl_location(&conn, sid, &sample_loc()).unwrap();
        delete_decl_location(&conn, sid).unwrap();
        assert!(read_decl_location(&conn, sid).unwrap().is_none());
    }

    #[test]
    fn cascade_drops_row_when_symbol_deleted() {
        let conn = fresh_conn();
        let sid = seed_symbol(&conn, "x::foo", "foo");
        upsert_decl_location(&conn, sid, &sample_loc()).unwrap();
        conn.execute("DELETE FROM symbols WHERE id = ?1", [sid])
            .unwrap();
        assert!(read_decl_location(&conn, sid).unwrap().is_none());
    }
}

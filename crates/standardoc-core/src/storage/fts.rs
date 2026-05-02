use rusqlite::{Connection, ErrorCode};

use crate::storage::error::StorageError;

pub(crate) fn reindex_fts(conn: &Connection) -> Result<(), StorageError> {
    conn.execute("INSERT INTO symbols_fts(symbols_fts) VALUES('rebuild')", [])?;
    Ok(())
}

pub(crate) fn fts_integrity_check(conn: &Connection) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO symbols_fts(symbols_fts) VALUES('integrity-check')",
        [],
    )
    .map_err(map_fts_integrity)?;
    Ok(())
}

pub(crate) fn assert_fts_counts_match(conn: &Connection) -> Result<(), StorageError> {
    let symbols: i64 = conn.query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get(0))?;
    let fts: i64 = conn.query_row(
        "SELECT COUNT(*) FROM symbols_fts_docsize",
        [],
        |row| row.get(0),
    )?;
    if symbols != fts {
        return Err(StorageError::FtsCountMismatch { symbols, fts });
    }
    Ok(())
}

fn map_fts_integrity(err: rusqlite::Error) -> StorageError {
    if let rusqlite::Error::SqliteFailure(ref ffi_err, ref msg) = err
        && ffi_err.code == ErrorCode::DatabaseCorrupt
    {
        let detail = msg
            .clone()
            .unwrap_or_else(|| String::from("FTS5 integrity-check failed"));
        return StorageError::FtsCorrupted { detail };
    }
    StorageError::Sqlite(err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::symbols::{delete_symbol, insert_symbol};
    use crate::storage::test_utils::{fresh_conn, sample_symbol, seed_file, symbol_ctx};

    fn seed_two_symbols(conn: &Connection) -> (i64, i64) {
        seed_file(conn, "src/main.rs");
        let a = insert_symbol(
            conn,
            &sample_symbol("alpha", "crate::alpha"),
            symbol_ctx("src/main.rs"),
        )
        .unwrap();
        let b = insert_symbol(
            conn,
            &sample_symbol("beta", "crate::beta"),
            symbol_ctx("src/main.rs"),
        )
        .unwrap();
        (a, b)
    }

    #[test]
    fn reindex_fts_idempotent() {
        let conn = fresh_conn();
        let _ids = seed_two_symbols(&conn);
        reindex_fts(&conn).unwrap();
        reindex_fts(&conn).unwrap();
        assert_fts_counts_match(&conn).unwrap();
    }

    #[test]
    fn reindex_fts_rebuilds_after_raw_truncate() {
        let conn = fresh_conn();
        let _ids = seed_two_symbols(&conn);
        conn.execute("DELETE FROM symbols_fts", []).unwrap();

        let mismatch = assert_fts_counts_match(&conn).unwrap_err();
        assert!(matches!(
            mismatch,
            StorageError::FtsCountMismatch {
                symbols: 2,
                fts: 0
            }
        ));

        reindex_fts(&conn).unwrap();
        assert_fts_counts_match(&conn).unwrap();
    }

    #[test]
    fn assert_fts_counts_match_when_synced() {
        let conn = fresh_conn();
        let _ids = seed_two_symbols(&conn);
        assert_fts_counts_match(&conn).unwrap();
    }

    #[test]
    fn assert_fts_counts_match_returns_mismatch_when_unsynced() {
        let conn = fresh_conn();
        let (a, _b) = seed_two_symbols(&conn);
        conn.execute("DELETE FROM symbols_fts WHERE rowid = ?1", [a])
            .unwrap();

        match assert_fts_counts_match(&conn) {
            Err(StorageError::FtsCountMismatch { symbols, fts }) => {
                assert_eq!(symbols, 2);
                assert_eq!(fts, 1);
            }
            other => panic!("expected FtsCountMismatch, got {other:?}"),
        }
    }

    #[test]
    fn fts_integrity_check_passes_on_synced_db() {
        let conn = fresh_conn();
        let _ids = seed_two_symbols(&conn);
        fts_integrity_check(&conn).unwrap();
    }

    #[test]
    fn update_trigger_propagates_name_change_to_fts() {
        let conn = fresh_conn();
        seed_file(&conn, "src/main.rs");
        let id = insert_symbol(
            &conn,
            &sample_symbol("alpha", "crate::alpha"),
            symbol_ctx("src/main.rs"),
        )
        .unwrap();

        let id2 = insert_symbol(
            &conn,
            &sample_symbol("omega", "crate::alpha"),
            symbol_ctx("src/main.rs"),
        )
        .unwrap();
        assert_eq!(id, id2, "UPSERT by fqdn must preserve id");

        let hit: String = conn
            .query_row(
                "SELECT name FROM symbols_fts WHERE symbols_fts MATCH 'omega'",
                [],
                |r| r.get(0),
            )
            .expect("AFTER UPDATE trigger should re-index renamed symbol");
        assert_eq!(hit, "omega");

        let row_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols_fts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            row_count, 1,
            "AFTER UPDATE trigger must DELETE old then INSERT new (single row)"
        );
    }

    #[test]
    fn delete_trigger_removes_row_from_fts() {
        let conn = fresh_conn();
        seed_file(&conn, "src/main.rs");
        let id = insert_symbol(
            &conn,
            &sample_symbol("alpha", "crate::alpha"),
            symbol_ctx("src/main.rs"),
        )
        .unwrap();

        let count_before: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols_fts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count_before, 1);

        delete_symbol(&conn, id).unwrap();

        let count_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols_fts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count_after, 0, "AFTER DELETE trigger must remove the FTS entry");
    }
}

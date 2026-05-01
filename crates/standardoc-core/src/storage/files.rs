use rusqlite::{Connection, OptionalExtension, Row};
use standardoc_ir::{Blake3Hash, Language};

use crate::storage::conv::{language_from_sql_text, language_to_sql_text};
use crate::storage::error::StorageError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileInput {
    pub(crate) path: String,
    pub(crate) content_hash: Blake3Hash,
    pub(crate) language: Language,
    pub(crate) byte_size: u64,
    pub(crate) last_scanned: i64,
    pub(crate) last_scan_error: Option<String>,
}

pub(crate) fn upsert_file(conn: &Connection, file: &FileInput) -> Result<(), StorageError> {
    let byte_size_i64 =
        i64::try_from(file.byte_size).map_err(|_| StorageError::InvalidStoredData {
            detail: format!("byte_size {} exceeds i64::MAX", file.byte_size),
        })?;
    conn.execute(
        "INSERT INTO files (path, content_hash, language, last_scanned, byte_size, last_scan_error) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT(path) DO UPDATE SET \
            content_hash    = excluded.content_hash, \
            language        = excluded.language, \
            last_scanned    = excluded.last_scanned, \
            byte_size       = excluded.byte_size, \
            last_scan_error = excluded.last_scan_error",
        rusqlite::params![
            file.path,
            file.content_hash.to_hex(),
            language_to_sql_text(file.language),
            file.last_scanned,
            byte_size_i64,
            file.last_scan_error,
        ],
    )?;
    Ok(())
}

pub(crate) fn get_file(conn: &Connection, path: &str) -> Result<Option<FileInput>, StorageError> {
    let raw = conn
        .query_row(
            "SELECT path, content_hash, language, last_scanned, byte_size, last_scan_error \
             FROM files WHERE path = ?1",
            [path],
            from_row,
        )
        .optional()?;
    raw.map(build_file_input).transpose()
}

pub(crate) fn delete_file(conn: &Connection, path: &str) -> Result<(), StorageError> {
    conn.execute("DELETE FROM files WHERE path = ?1", [path])?;
    Ok(())
}

struct RawFileRow {
    path: String,
    content_hash_hex: String,
    language_text: String,
    last_scanned: i64,
    byte_size_i64: i64,
    last_scan_error: Option<String>,
}

fn from_row(row: &Row<'_>) -> rusqlite::Result<RawFileRow> {
    Ok(RawFileRow {
        path: row.get(0)?,
        content_hash_hex: row.get(1)?,
        language_text: row.get(2)?,
        last_scanned: row.get(3)?,
        byte_size_i64: row.get(4)?,
        last_scan_error: row.get(5)?,
    })
}

fn build_file_input(raw: RawFileRow) -> Result<FileInput, StorageError> {
    let content_hash =
        Blake3Hash::from_hex(&raw.content_hash_hex).map_err(|e| StorageError::InvalidStoredData {
            detail: format!("files.content_hash: {e}"),
        })?;
    let language = language_from_sql_text(&raw.language_text)?;
    let byte_size =
        u64::try_from(raw.byte_size_i64).map_err(|_| StorageError::InvalidStoredData {
            detail: format!("files.byte_size negative: {}", raw.byte_size_i64),
        })?;
    Ok(FileInput {
        path: raw.path,
        content_hash,
        language,
        byte_size,
        last_scanned: raw.last_scanned,
        last_scan_error: raw.last_scan_error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_utils::fresh_conn;

    fn sample(path: &str) -> FileInput {
        FileInput {
            path: path.into(),
            content_hash: Blake3Hash::new([0xab; 32]),
            language: Language::Rust,
            byte_size: 1234,
            last_scanned: 1_700_000_000_000,
            last_scan_error: None,
        }
    }

    #[test]
    fn upsert_then_get_round_trip() {
        let conn = fresh_conn();
        let f = sample("src/main.rs");
        upsert_file(&conn, &f).unwrap();
        let back = get_file(&conn, "src/main.rs").unwrap().unwrap();
        assert_eq!(back, f);
    }

    #[test]
    fn upsert_on_conflict_updates_all_columns() {
        let conn = fresh_conn();
        let mut f = sample("src/main.rs");
        upsert_file(&conn, &f).unwrap();

        f.content_hash = Blake3Hash::new([0xcd; 32]);
        f.language = Language::TypeScript;
        f.byte_size = 9999;
        f.last_scanned = 1_700_000_999_999;
        f.last_scan_error = Some("parse error".into());
        upsert_file(&conn, &f).unwrap();

        let back = get_file(&conn, "src/main.rs").unwrap().unwrap();
        assert_eq!(back, f);
    }

    #[test]
    fn get_missing_returns_none() {
        let conn = fresh_conn();
        assert!(get_file(&conn, "no/such/file.rs").unwrap().is_none());
    }

    #[test]
    fn delete_removes_row() {
        let conn = fresh_conn();
        upsert_file(&conn, &sample("src/main.rs")).unwrap();
        delete_file(&conn, "src/main.rs").unwrap();
        assert!(get_file(&conn, "src/main.rs").unwrap().is_none());
    }

    #[test]
    fn delete_missing_is_noop() {
        let conn = fresh_conn();
        delete_file(&conn, "ghost.rs").unwrap();
    }

    #[test]
    fn delete_file_cascades_to_symbols() {
        let conn = fresh_conn();
        upsert_file(&conn, &sample("src/main.rs")).unwrap();
        conn.execute(
            "INSERT INTO symbols \
             (fqdn, name, kind, language_kind, language, file_path, \
              start_line, end_line, start_col, end_col) \
             VALUES ('crate::main', 'main', 'function', 'fn_item', 'rust', \
                     'src/main.rs', 1, 3, 0, 0)",
            [],
        )
        .unwrap();
        let count_before: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count_before, 1);

        delete_file(&conn, "src/main.rs").unwrap();

        let count_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count_after, 0, "FK ON DELETE CASCADE should wipe symbols");
    }

    #[test]
    fn get_with_corrupted_hash_returns_invalid_stored_data() {
        let conn = fresh_conn();
        conn.execute(
            "INSERT INTO files (path, content_hash, language, last_scanned, byte_size) \
             VALUES ('src/x.rs', 'too-short', 'rust', 0, 0)",
            [],
        )
        .unwrap();
        let err = get_file(&conn, "src/x.rs").unwrap_err();
        assert!(matches!(err, StorageError::InvalidStoredData { .. }));
    }

    #[test]
    fn get_with_unknown_language_returns_invalid_stored_data() {
        let conn = fresh_conn();
        conn.execute(
            "INSERT INTO files (path, content_hash, language, last_scanned, byte_size) \
             VALUES ('src/x.rs', ?1, 'banana', 0, 0)",
            [Blake3Hash::default().to_hex()],
        )
        .unwrap();
        let err = get_file(&conn, "src/x.rs").unwrap_err();
        assert!(matches!(err, StorageError::InvalidStoredData { .. }));
    }
}

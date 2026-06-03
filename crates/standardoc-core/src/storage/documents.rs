use rusqlite::{Connection, OptionalExtension, Row};

use crate::storage::error::StorageError;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DocumentInput {
    pub(crate) symbol_id: i64,
    pub(crate) description: Option<String>,
    pub(crate) examples_json: Option<String>,
    pub(crate) tags_json: Option<String>,
    pub(crate) params_json: Option<String>,
    pub(crate) returns_json: Option<String>,
    pub(crate) ai_summary: Option<String>,
    pub(crate) last_updated: i64,
}

pub(crate) fn upsert_document(conn: &Connection, doc: &DocumentInput) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO documents \
         (symbol_id, description, examples_json, tags_json, params_json, \
          returns_json, ai_summary, last_updated) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT(symbol_id) DO UPDATE SET \
            description   = excluded.description, \
            examples_json = excluded.examples_json, \
            tags_json     = excluded.tags_json, \
            params_json   = excluded.params_json, \
            returns_json  = excluded.returns_json, \
            ai_summary    = excluded.ai_summary, \
            last_updated  = excluded.last_updated",
        rusqlite::params![
            doc.symbol_id,
            doc.description,
            doc.examples_json,
            doc.tags_json,
            doc.params_json,
            doc.returns_json,
            doc.ai_summary,
            doc.last_updated,
        ],
    )?;
    Ok(())
}

pub(crate) fn get_document(
    conn: &Connection,
    symbol_id: i64,
) -> Result<Option<DocumentInput>, StorageError> {
    Ok(conn
        .query_row(
            "SELECT symbol_id, description, examples_json, tags_json, params_json, \
                    returns_json, ai_summary, last_updated \
             FROM documents WHERE symbol_id = ?1",
            [symbol_id],
            from_row,
        )
        .optional()?)
}

pub(crate) fn delete_document(conn: &Connection, symbol_id: i64) -> Result<(), StorageError> {
    conn.execute("DELETE FROM documents WHERE symbol_id = ?1", [symbol_id])?;
    Ok(())
}

fn from_row(row: &Row<'_>) -> rusqlite::Result<DocumentInput> {
    Ok(DocumentInput {
        symbol_id: row.get(0)?,
        description: row.get(1)?,
        examples_json: row.get(2)?,
        tags_json: row.get(3)?,
        params_json: row.get(4)?,
        returns_json: row.get(5)?,
        ai_summary: row.get(6)?,
        last_updated: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_utils::fresh_conn;

    fn insert_dummy_symbol(conn: &Connection, fqdn: &str) -> i64 {
        conn.execute(
            "INSERT INTO files (path, content_hash, language, last_scanned, byte_size) \
             VALUES ('src/x.rs', ?1, 'rust', 0, 0) \
             ON CONFLICT(path) DO NOTHING",
            ["0".repeat(64)],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO symbols \
             (fqdn, name, kind, language_kind, language, file_path, \
              start_line, end_line, start_col, end_col) \
             VALUES (?1, ?1, 'callable', 'fn_item', 'rust', 'src/x.rs', 0, 0, 0, 0)",
            [fqdn],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn sample(symbol_id: i64) -> DocumentInput {
        DocumentInput {
            symbol_id,
            description: Some("Creates a new user.".into()),
            examples_json: Some("[\"createUser('a@b.c')\"]".into()),
            tags_json: Some("{\"since\":\"1.2\"}".into()),
            params_json: Some("[{\"name\":\"email\",\"description\":\"user email\"}]".into()),
            returns_json: Some("{\"description\":\"the new user\"}".into()),
            ai_summary: Some("AI-generated summary.".into()),
            last_updated: 1_700_000_000_000,
        }
    }

    #[test]
    fn upsert_then_get_round_trip() {
        let conn = fresh_conn();
        let id = insert_dummy_symbol(&conn, "crate::a");
        let doc = sample(id);
        upsert_document(&conn, &doc).unwrap();
        let back = get_document(&conn, id).unwrap().unwrap();
        assert_eq!(back, doc);
    }

    #[test]
    fn upsert_on_conflict_updates_all_columns() {
        let conn = fresh_conn();
        let id = insert_dummy_symbol(&conn, "crate::a");
        upsert_document(&conn, &sample(id)).unwrap();

        let updated = DocumentInput {
            symbol_id: id,
            description: Some("Now does X.".into()),
            examples_json: None,
            tags_json: None,
            params_json: None,
            returns_json: None,
            ai_summary: None,
            last_updated: 1_700_000_999_999,
        };
        upsert_document(&conn, &updated).unwrap();

        let back = get_document(&conn, id).unwrap().unwrap();
        assert_eq!(back, updated);
    }

    #[test]
    fn get_missing_returns_none() {
        let conn = fresh_conn();
        assert!(get_document(&conn, 9_999).unwrap().is_none());
    }

    #[test]
    fn upsert_with_no_matching_symbol_violates_fk() {
        let conn = fresh_conn();
        let err = upsert_document(&conn, &sample(42)).unwrap_err();
        assert!(matches!(err, StorageError::Sqlite(_)));
    }

    #[test]
    fn delete_symbol_cascades_to_document() {
        let conn = fresh_conn();
        let id = insert_dummy_symbol(&conn, "crate::a");
        upsert_document(&conn, &sample(id)).unwrap();
        conn.execute("DELETE FROM symbols WHERE id = ?1", [id])
            .unwrap();
        assert!(get_document(&conn, id).unwrap().is_none());
    }

    #[test]
    fn upsert_with_all_options_none_persists_nulls() {
        let conn = fresh_conn();
        let id = insert_dummy_symbol(&conn, "crate::a");
        let bare = DocumentInput {
            symbol_id: id,
            last_updated: 42,
            ..DocumentInput::default()
        };
        upsert_document(&conn, &bare).unwrap();
        let back = get_document(&conn, id).unwrap().unwrap();
        assert_eq!(back.symbol_id, id);
        assert_eq!(back.description, None);
        assert_eq!(back.examples_json, None);
        assert_eq!(back.tags_json, None);
        assert_eq!(back.params_json, None);
        assert_eq!(back.returns_json, None);
        assert_eq!(back.ai_summary, None);
        assert_eq!(back.last_updated, 42);
    }
}

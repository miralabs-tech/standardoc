use rusqlite::{Connection, OptionalExtension, Row};

use crate::storage::error::{StorageError, map_constraint};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ConfidenceLevel {
    Low,
    Medium,
    High,
}

impl ConfidenceLevel {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    pub(crate) fn from_sql_text(s: &str) -> Result<Self, StorageError> {
        match s {
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            other => Err(StorageError::InvalidStoredData {
                detail: format!("unknown confidence: {other:?}"),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EnrichmentInput {
    pub(crate) symbol_id: i64,
    pub(crate) description: Option<String>,
    pub(crate) params_json: Option<String>,
    pub(crate) returns_json: Option<String>,
    pub(crate) modifiers_json: Option<String>,
    pub(crate) confidence: ConfidenceLevel,
    pub(crate) sources_json: String,
    pub(crate) last_updated: i64,
}

pub(crate) fn upsert_enrichment(
    conn: &Connection,
    enr: &EnrichmentInput,
) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO enrichments \
         (symbol_id, description, params_json, returns_json, modifiers_json, \
          confidence, sources_json, last_updated) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT(symbol_id) DO UPDATE SET \
            description    = excluded.description, \
            params_json    = excluded.params_json, \
            returns_json   = excluded.returns_json, \
            modifiers_json = excluded.modifiers_json, \
            confidence     = excluded.confidence, \
            sources_json   = excluded.sources_json, \
            last_updated   = excluded.last_updated",
        rusqlite::params![
            enr.symbol_id,
            enr.description,
            enr.params_json,
            enr.returns_json,
            enr.modifiers_json,
            enr.confidence.as_str(),
            enr.sources_json,
            enr.last_updated,
        ],
    )
    .map_err(map_constraint)?;
    Ok(())
}

pub(crate) fn get_enrichment(
    conn: &Connection,
    symbol_id: i64,
) -> Result<Option<EnrichmentInput>, StorageError> {
    let raw = conn
        .query_row(
            "SELECT symbol_id, description, params_json, returns_json, modifiers_json, \
                    confidence, sources_json, last_updated \
             FROM enrichments WHERE symbol_id = ?1",
            [symbol_id],
            from_row,
        )
        .optional()?;
    raw.map(build_enrichment_input).transpose()
}

struct RawEnrichmentRow {
    symbol_id: i64,
    description: Option<String>,
    params_json: Option<String>,
    returns_json: Option<String>,
    modifiers_json: Option<String>,
    confidence_text: String,
    sources_json: String,
    last_updated: i64,
}

fn from_row(row: &Row<'_>) -> rusqlite::Result<RawEnrichmentRow> {
    Ok(RawEnrichmentRow {
        symbol_id: row.get(0)?,
        description: row.get(1)?,
        params_json: row.get(2)?,
        returns_json: row.get(3)?,
        modifiers_json: row.get(4)?,
        confidence_text: row.get(5)?,
        sources_json: row.get(6)?,
        last_updated: row.get(7)?,
    })
}

fn build_enrichment_input(raw: RawEnrichmentRow) -> Result<EnrichmentInput, StorageError> {
    let confidence = ConfidenceLevel::from_sql_text(&raw.confidence_text)?;
    Ok(EnrichmentInput {
        symbol_id: raw.symbol_id,
        description: raw.description,
        params_json: raw.params_json,
        returns_json: raw.returns_json,
        modifiers_json: raw.modifiers_json,
        confidence,
        sources_json: raw.sources_json,
        last_updated: raw.last_updated,
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

    fn sample(symbol_id: i64) -> EnrichmentInput {
        EnrichmentInput {
            symbol_id,
            description: Some("Inferred description.".into()),
            params_json: Some(
                "[{\"name\":\"x\",\"description\":\"x\",\"source\":\"name\"}]".into(),
            ),
            returns_json: Some("{\"description\":\"r\",\"source\":\"return-name\"}".into()),
            modifiers_json: Some("{\"async\":true}".into()),
            confidence: ConfidenceLevel::High,
            sources_json: "[\"predicate-is\",\"verb-create\"]".into(),
            last_updated: 1_700_000_000_000,
        }
    }

    #[test]
    fn confidence_level_round_trip_via_str() {
        for c in [
            ConfidenceLevel::Low,
            ConfidenceLevel::Medium,
            ConfidenceLevel::High,
        ] {
            assert_eq!(ConfidenceLevel::from_sql_text(c.as_str()).unwrap(), c);
        }
    }

    #[test]
    fn confidence_level_unknown_is_invalid_stored_data() {
        let err = ConfidenceLevel::from_sql_text("very-high").unwrap_err();
        assert!(matches!(err, StorageError::InvalidStoredData { .. }));
    }

    #[test]
    fn upsert_then_get_round_trip() {
        let conn = fresh_conn();
        let id = insert_dummy_symbol(&conn, "crate::a");
        let enr = sample(id);
        upsert_enrichment(&conn, &enr).unwrap();
        let back = get_enrichment(&conn, id).unwrap().unwrap();
        assert_eq!(back, enr);
    }

    #[test]
    fn upsert_on_conflict_updates_all_columns() {
        let conn = fresh_conn();
        let id = insert_dummy_symbol(&conn, "crate::a");
        upsert_enrichment(&conn, &sample(id)).unwrap();

        let updated = EnrichmentInput {
            symbol_id: id,
            description: None,
            params_json: None,
            returns_json: None,
            modifiers_json: None,
            confidence: ConfidenceLevel::Low,
            sources_json: "[]".into(),
            last_updated: 1_700_000_999_999,
        };
        upsert_enrichment(&conn, &updated).unwrap();

        let back = get_enrichment(&conn, id).unwrap().unwrap();
        assert_eq!(back, updated);
    }

    #[test]
    fn get_missing_returns_none() {
        let conn = fresh_conn();
        assert!(get_enrichment(&conn, 9_999).unwrap().is_none());
    }

    #[test]
    fn upsert_with_no_matching_symbol_violates_fk() {
        let conn = fresh_conn();
        let err = upsert_enrichment(&conn, &sample(42)).unwrap_err();
        assert!(matches!(err, StorageError::Sqlite(_)));
    }

    #[test]
    fn delete_symbol_cascades_to_enrichment() {
        let conn = fresh_conn();
        let id = insert_dummy_symbol(&conn, "crate::a");
        upsert_enrichment(&conn, &sample(id)).unwrap();
        conn.execute("DELETE FROM symbols WHERE id = ?1", [id])
            .unwrap();
        assert!(get_enrichment(&conn, id).unwrap().is_none());
    }

    #[test]
    fn insert_with_invalid_confidence_string_routes_to_check_violated() {
        let conn = fresh_conn();
        let id = insert_dummy_symbol(&conn, "crate::a");
        let err = conn
            .execute(
                "INSERT INTO enrichments \
                 (symbol_id, confidence, sources_json, last_updated) \
                 VALUES (?1, 'very-high', '[]', 0)",
                [id],
            )
            .unwrap_err();
        let mapped = map_constraint(err);
        assert!(matches!(
            mapped,
            StorageError::CheckConstraintViolated { .. }
        ));
    }
}

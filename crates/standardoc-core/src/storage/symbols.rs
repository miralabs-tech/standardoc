use rusqlite::Connection;
use standardoc_ir::{Language, RawSymbol, SourceOrigin, SymbolLocation};

use crate::storage::conv::{
    kind_to_sql_text, language_to_sql_text, signature_to_json, source_origin_to_sql_text,
    visibility_to_sql_text,
};
use crate::storage::error::{StorageError, map_constraint};

#[derive(Debug, Clone, Copy)]
pub(crate) struct SymbolInsertContext<'a> {
    pub file_path: &'a str,
    pub language: Language,
    pub is_external: bool,
    pub source_origin: SourceOrigin,
}

pub(crate) fn insert_symbol(
    conn: &Connection,
    symbol: &RawSymbol,
    ctx: SymbolInsertContext<'_>,
) -> Result<i64, StorageError> {
    let signature_json = symbol.signature.as_ref().map(signature_to_json).transpose()?;
    let body_hash_hex = symbol.body_hash.as_ref().map(standardoc_ir::Blake3Hash::to_hex);

    let id = conn
        .query_row(
            "INSERT INTO symbols (\
                fqdn, name, kind, language_kind, language, module, visibility, \
                file_path, start_line, end_line, start_col, end_col, \
                signature_json, body_hash, is_external, source_origin\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16) \
             ON CONFLICT(fqdn) DO UPDATE SET \
                name           = excluded.name, \
                kind           = excluded.kind, \
                language_kind  = excluded.language_kind, \
                language       = excluded.language, \
                module         = excluded.module, \
                visibility     = excluded.visibility, \
                file_path      = excluded.file_path, \
                start_line     = excluded.start_line, \
                end_line       = excluded.end_line, \
                start_col      = excluded.start_col, \
                end_col        = excluded.end_col, \
                signature_json = excluded.signature_json, \
                body_hash      = excluded.body_hash, \
                is_external    = excluded.is_external, \
                source_origin  = excluded.source_origin \
             RETURNING id",
            rusqlite::params![
                symbol.fqdn,
                symbol.name,
                kind_to_sql_text(symbol.kind),
                symbol.language_kind.as_str(),
                language_to_sql_text(ctx.language),
                symbol.module,
                visibility_to_sql_text(symbol.visibility),
                ctx.file_path,
                symbol.location.start_line,
                symbol.location.end_line,
                symbol.location.start_col,
                symbol.location.end_col,
                signature_json,
                body_hash_hex,
                ctx.is_external,
                source_origin_to_sql_text(ctx.source_origin),
            ],
            |row| row.get::<_, i64>(0),
        )
        .map_err(map_constraint)?;
    Ok(id)
}

pub(crate) fn update_symbol_positions(
    conn: &Connection,
    id: i64,
    location: &SymbolLocation,
) -> Result<(), StorageError> {
    conn.execute(
        "UPDATE symbols SET \
            start_line = ?1, end_line = ?2, start_col = ?3, end_col = ?4 \
         WHERE id = ?5",
        rusqlite::params![
            location.start_line,
            location.end_line,
            location.start_col,
            location.end_col,
            id,
        ],
    )?;
    Ok(())
}

/// Encapsulates the reverse-promote step (DDL §3.4) before deleting a symbol.
///
/// Atomicity is the caller's responsibility: this helper assumes the pipeline's
/// per-batch transaction (lock pipeline #4) wraps the two statements. Outside
/// that wrap, a partial failure between UPDATE and DELETE leaves entrant edges
/// pointing at `to_unresolved = <fqdn>` while the symbol still exists — a state
/// that self-heals via `promote_unresolved_batch` on the next batch but is
/// easier to reason about as atomic.
pub(crate) fn delete_symbol(conn: &Connection, id: i64) -> Result<(), StorageError> {
    conn.execute(
        "UPDATE edges \
         SET to_unresolved = (SELECT fqdn FROM symbols WHERE id = ?1), \
             to_symbol_id  = NULL \
         WHERE to_symbol_id = ?1",
        [id],
    )?;
    conn.execute("DELETE FROM symbols WHERE id = ?1", [id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_utils::{fresh_conn, sample_symbol, seed_file, symbol_ctx};
    use standardoc_ir::{Blake3Hash, Modifiers, Param, Signature, SymbolLocation, TypeRef};

    #[test]
    fn insert_symbol_returns_id_and_persists_row() {
        let conn = fresh_conn();
        seed_file(&conn, "src/main.rs");
        let id = insert_symbol(
            &conn,
            &sample_symbol("foo", "crate::foo"),
            symbol_ctx("src/main.rs"),
        )
        .unwrap();
        assert!(id > 0);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn insert_symbol_upsert_preserves_id_and_updates_columns() {
        let conn = fresh_conn();
        seed_file(&conn, "src/main.rs");
        let id1 = insert_symbol(
            &conn,
            &sample_symbol("foo", "crate::foo"),
            symbol_ctx("src/main.rs"),
        )
        .unwrap();

        let mut updated = sample_symbol("foo_renamed", "crate::foo");
        updated.location.start_line = 42;
        updated.body_hash = Some(Blake3Hash::new([0xcd; 32]));
        let id2 = insert_symbol(&conn, &updated, symbol_ctx("src/main.rs")).unwrap();

        assert_eq!(id1, id2, "id must be preserved across upsert by fqdn");
        let (name, start_line): (String, i64) = conn
            .query_row(
                "SELECT name, start_line FROM symbols WHERE id = ?1",
                [id1],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(name, "foo_renamed");
        assert_eq!(start_line, 42);
    }

    #[test]
    fn insert_symbol_external_with_no_body_hash() {
        let conn = fresh_conn();
        seed_file(&conn, ".cargo/registry/serde-1.0/src/lib.rs");
        let mut ext = sample_symbol("Deserialize", "external::serde::Deserialize");
        ext.body_hash = None;
        let mut ext_ctx = symbol_ctx(".cargo/registry/serde-1.0/src/lib.rs");
        ext_ctx.is_external = true;
        ext_ctx.source_origin = SourceOrigin::CargoRegistry;

        let id = insert_symbol(&conn, &ext, ext_ctx).unwrap();
        let (is_external, body_hash, source_origin): (i64, Option<String>, String) = conn
            .query_row(
                "SELECT is_external, body_hash, source_origin FROM symbols WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(is_external, 1);
        assert_eq!(body_hash, None);
        assert_eq!(source_origin, "cargo_registry");
    }

    #[test]
    fn insert_symbol_with_signature_persists_json() {
        let conn = fresh_conn();
        seed_file(&conn, "src/main.rs");
        let mut s = sample_symbol("foo", "crate::foo");
        s.signature = Some(Signature {
            params: vec![Param {
                name: "x".into(),
                ty: TypeRef::new("u32"),
                default: None,
            }],
            returns: Some(TypeRef::new("u32")),
            modifiers: Modifiers {
                is_async: true,
                ..Default::default()
            },
            ..Default::default()
        });
        let id = insert_symbol(&conn, &s, symbol_ctx("src/main.rs")).unwrap();
        let json: String = conn
            .query_row(
                "SELECT signature_json FROM symbols WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(json.contains("\"async\":true"));
        assert!(json.contains("\"name\":\"x\""));
    }

    #[test]
    fn update_symbol_positions_only_touches_positions() {
        let conn = fresh_conn();
        seed_file(&conn, "src/main.rs");
        let id = insert_symbol(
            &conn,
            &sample_symbol("foo", "crate::foo"),
            symbol_ctx("src/main.rs"),
        )
        .unwrap();

        let new_loc = SymbolLocation {
            file: "src/main.rs".into(),
            start_line: 100,
            end_line: 110,
            start_col: 4,
            end_col: 8,
        };
        update_symbol_positions(&conn, id, &new_loc).unwrap();

        let (name, start, end_line, body_hash): (String, i64, i64, Option<String>) = conn
            .query_row(
                "SELECT name, start_line, end_line, body_hash FROM symbols WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(name, "foo", "name must NOT change");
        assert_eq!(start, 100);
        assert_eq!(end_line, 110);
        assert!(body_hash.is_some(), "body_hash must NOT be reset");
    }

    #[test]
    fn delete_symbol_removes_row() {
        let conn = fresh_conn();
        seed_file(&conn, "src/main.rs");
        let id = insert_symbol(
            &conn,
            &sample_symbol("foo", "crate::foo"),
            symbol_ctx("src/main.rs"),
        )
        .unwrap();
        delete_symbol(&conn, id).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn delete_symbol_reverse_promotes_incoming_resolved_edges() {
        let conn = fresh_conn();
        seed_file(&conn, "src/main.rs");
        let target_id = insert_symbol(
            &conn,
            &sample_symbol("foo", "crate::foo"),
            symbol_ctx("src/main.rs"),
        )
        .unwrap();
        let caller_id = insert_symbol(
            &conn,
            &sample_symbol("bar", "crate::bar"),
            symbol_ctx("src/main.rs"),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO edges (from_symbol_id, kind, to_symbol_id, to_unresolved) \
             VALUES (?1, 'CALLS', ?2, NULL)",
            [caller_id, target_id],
        )
        .unwrap();

        delete_symbol(&conn, target_id).unwrap();

        let (to_id, to_unresolved): (Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT to_symbol_id, to_unresolved FROM edges WHERE from_symbol_id = ?1",
                [caller_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(to_id, None);
        assert_eq!(
            to_unresolved.as_deref(),
            Some("crate::foo"),
            "incoming edge should fall back to the deleted symbol's fqdn"
        );
    }

    #[test]
    fn delete_symbol_cascades_to_outgoing_edges() {
        let conn = fresh_conn();
        seed_file(&conn, "src/main.rs");
        let from_id = insert_symbol(
            &conn,
            &sample_symbol("foo", "crate::foo"),
            symbol_ctx("src/main.rs"),
        )
        .unwrap();
        let target_id = insert_symbol(
            &conn,
            &sample_symbol("bar", "crate::bar"),
            symbol_ctx("src/main.rs"),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO edges (from_symbol_id, kind, to_symbol_id) \
             VALUES (?1, 'CALLS', ?2)",
            [from_id, target_id],
        )
        .unwrap();

        delete_symbol(&conn, from_id).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM edges WHERE from_symbol_id = ?1",
                [from_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 0,
            "outgoing edges must cascade-delete with the source symbol"
        );
    }

    #[test]
    fn insert_symbol_check_violation_on_invalid_kind_via_raw_sql() {
        let conn = fresh_conn();
        seed_file(&conn, "src/main.rs");
        let err = conn
            .execute(
                "INSERT INTO symbols \
                 (fqdn, name, kind, language_kind, language, file_path, \
                  start_line, end_line, start_col, end_col) \
                 VALUES ('crate::x', 'x', 'banana', 'k', 'rust', 'src/main.rs', \
                         0, 0, 0, 0)",
                [],
            )
            .unwrap_err();
        assert!(matches!(
            map_constraint(err),
            StorageError::CheckConstraintViolated { .. }
        ));
    }

    #[test]
    fn insert_symbol_smoke_fts_match() {
        let conn = fresh_conn();
        seed_file(&conn, "src/main.rs");
        insert_symbol(
            &conn,
            &sample_symbol("create_user", "crate::user::create_user"),
            symbol_ctx("src/main.rs"),
        )
        .unwrap();

        let hit: String = conn
            .query_row(
                "SELECT name FROM symbols_fts WHERE symbols_fts MATCH 'create_user'",
                [],
                |r| r.get(0),
            )
            .expect("FTS trigger should have indexed the new symbol");
        assert_eq!(hit, "create_user");
    }
}

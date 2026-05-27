use rusqlite::Connection;
use standardoc_ir::{Language, RawSymbol, SourceOrigin, SymbolLocation};

use crate::storage::conv::{
    decl_kind_to_sql_text, entry_point_to_sql_text, kind_to_sql_text, language_to_sql_text,
    signature_to_json, source_origin_to_sql_text, visibility_to_sql_text,
};
use crate::storage::error::{StorageError, map_constraint};

#[derive(Debug, Clone, Copy)]
pub(crate) struct SymbolInsertContext<'a> {
    pub file_path: &'a str,
    pub language: Language,
    pub is_external: bool,
    pub source_origin: SourceOrigin,
    pub revision: u64,
    /// Stage 3b-7-b: which workspace owns this row. Primary writers pass
    /// `PRIMARY_WORKSPACE_ID`; the autonomous peer indexer (Layer 3)
    /// will pass a peer's catalog UUID. Stamped explicitly into the
    /// INSERT — relying on the v11 column default ('primary') would
    /// silently misclassify peer rows the day Layer 3 lands.
    pub workspace_id: &'a str,
}

pub(crate) fn insert_symbol(
    conn: &Connection,
    symbol: &RawSymbol,
    ctx: SymbolInsertContext<'_>,
) -> Result<i64, StorageError> {
    let signature_json = symbol
        .signature
        .as_ref()
        .map(signature_to_json)
        .transpose()?;
    let body_hash_hex = symbol
        .body_hash
        .as_ref()
        .map(standardoc_ir::Blake3Hash::to_hex);
    let revision_i64 = i64::try_from(ctx.revision).unwrap_or(i64::MAX);
    // Stage 3e-1b: persist `RawSymbol.flags` as a JSON-array TEXT
    // column so queries can `LIKE`/`json_each` filter without an
    // extra join. `serde_json::to_string` on `&Vec<String>` always
    // succeeds — the `expect` never fires.
    let flags_json = serde_json::to_string(&symbol.flags).expect("Vec<String> serializes to JSON");
    let decl_kind_text = symbol.decl_kind.as_ref().map(decl_kind_to_sql_text);
    let receiver_type_text = symbol.receiver_type.as_ref().map(|t| t.display.clone());
    let entry_point_text = symbol.entry_point.map(entry_point_to_sql_text);

    let id = conn
        .query_row(
            "INSERT INTO symbols (\
                fqdn, name, kind, language_kind, language, module, visibility, \
                file_path, start_line, end_line, start_col, end_col, \
                signature_json, body_hash, is_external, source_origin, \
                last_modified_revision, flags, workspace_id, decl_kind, \
                implements_trait, receiver_type, entry_point\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23) \
             ON CONFLICT(workspace_id, fqdn) DO UPDATE SET \
                name                    = excluded.name, \
                kind                    = excluded.kind, \
                language_kind           = excluded.language_kind, \
                language                = excluded.language, \
                module                  = excluded.module, \
                visibility              = excluded.visibility, \
                file_path               = excluded.file_path, \
                start_line              = excluded.start_line, \
                end_line                = excluded.end_line, \
                start_col               = excluded.start_col, \
                end_col                 = excluded.end_col, \
                signature_json          = excluded.signature_json, \
                body_hash               = excluded.body_hash, \
                is_external             = excluded.is_external, \
                source_origin           = excluded.source_origin, \
                last_modified_revision  = excluded.last_modified_revision, \
                flags                   = excluded.flags, \
                workspace_id            = excluded.workspace_id, \
                decl_kind               = excluded.decl_kind, \
                implements_trait        = excluded.implements_trait, \
                receiver_type           = excluded.receiver_type, \
                entry_point             = excluded.entry_point \
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
                revision_i64,
                flags_json,
                ctx.workspace_id,
                decl_kind_text,
                symbol.implements_trait,
                receiver_type_text,
                entry_point_text,
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
    revision: u64,
) -> Result<(), StorageError> {
    let revision_i64 = i64::try_from(revision).unwrap_or(i64::MAX);
    conn.execute(
        "UPDATE symbols SET \
            start_line              = ?1, \
            end_line                = ?2, \
            start_col               = ?3, \
            end_col                 = ?4, \
            last_modified_revision  = ?5 \
         WHERE id = ?6",
        rusqlite::params![
            location.start_line,
            location.end_line,
            location.start_col,
            location.end_col,
            revision_i64,
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
mod tests;

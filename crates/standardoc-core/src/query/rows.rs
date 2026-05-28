//! Raw-row SQL helpers for the read-side query layer.
//!
//! Extracted from `query.rs` (Phase 3.2+ structure split). Holds the
//! SELECT column list shared by every `symbols` query (`SYMBOL_COLUMNS`),
//! the row-extractor structs (`SymbolRowRaw`, `EdgeRowRaw`) and their
//! companion read / build helpers. All items are `pub(super)` so the
//! parent `query` module sees them via the `use rows::*` glob in
//! `query.rs`.

use rusqlite::{Connection, OptionalExtension, Row};
use standardoc_ir::{
    Blake3Hash, LanguageKind, RawEdge, RawSymbol, ResolvedOrUnresolved, Site, SymbolLocation,
    TypeRef,
};

use crate::storage::conv::{
    decl_kind_from_sql_text, edge_confidence_from_sql_text, edge_kind_from_sql_text,
    entry_point_from_sql_text, json_to_signature, kind_from_sql_text, visibility_from_sql_text,
};
use crate::storage::error::StorageError;

pub(super) const SYMBOL_COLUMNS: &str = "fqdn, name, kind, language_kind, module, visibility, \
     file_path, start_line, end_line, start_col, end_col, \
     signature_json, body_hash, flags, decl_kind, implements_trait, receiver_type, entry_point";

pub(super) struct SymbolRowRaw {
    pub(super) fqdn: String,
    pub(super) name: String,
    pub(super) kind_text: String,
    pub(super) language_kind_text: String,
    pub(super) module: Option<String>,
    pub(super) visibility_text: String,
    pub(super) file_path: String,
    pub(super) start_line: i64,
    pub(super) end_line: i64,
    pub(super) start_col: i64,
    pub(super) end_col: i64,
    pub(super) signature_json: Option<String>,
    pub(super) body_hash_hex: Option<String>,
    pub(super) flags_json: String,
    pub(super) decl_kind_text: Option<String>,
    pub(super) implements_trait: Option<String>,
    pub(super) receiver_type_text: Option<String>,
    pub(super) entry_point_text: Option<String>,
}

pub(super) fn read_symbol_row(row: &Row<'_>) -> rusqlite::Result<SymbolRowRaw> {
    Ok(SymbolRowRaw {
        fqdn: row.get(0)?,
        name: row.get(1)?,
        kind_text: row.get(2)?,
        language_kind_text: row.get(3)?,
        module: row.get(4)?,
        visibility_text: row.get(5)?,
        file_path: row.get(6)?,
        start_line: row.get(7)?,
        end_line: row.get(8)?,
        start_col: row.get(9)?,
        end_col: row.get(10)?,
        signature_json: row.get(11)?,
        body_hash_hex: row.get(12)?,
        flags_json: row.get(13)?,
        decl_kind_text: row.get(14)?,
        implements_trait: row.get(15)?,
        receiver_type_text: row.get(16)?,
        entry_point_text: row.get(17)?,
    })
}

pub(super) fn build_symbol(raw: SymbolRowRaw) -> Result<RawSymbol, StorageError> {
    let kind = kind_from_sql_text(&raw.kind_text)?;
    let visibility = visibility_from_sql_text(&raw.visibility_text)?;
    let signature = raw
        .signature_json
        .as_deref()
        .map(json_to_signature)
        .transpose()?;
    let body_hash = raw
        .body_hash_hex
        .as_deref()
        .map(Blake3Hash::from_hex)
        .transpose()
        .map_err(|e| StorageError::InvalidStoredData {
            detail: format!("symbols.body_hash: {e}"),
        })?;
    let decl_kind = raw
        .decl_kind_text
        .as_deref()
        .map(decl_kind_from_sql_text)
        .transpose()?;
    let entry_point = raw
        .entry_point_text
        .as_deref()
        .map(entry_point_from_sql_text)
        .transpose()?;
    let location = SymbolLocation {
        file: raw.file_path,
        start_line: position_to_u32("start_line", raw.start_line)?,
        end_line: position_to_u32("end_line", raw.end_line)?,
        start_col: position_to_u32("start_col", raw.start_col)?,
        end_col: position_to_u32("end_col", raw.end_col)?,
    };
    Ok(RawSymbol {
        name: raw.name,
        fqdn: raw.fqdn,
        kind,
        language_kind: LanguageKind::from(raw.language_kind_text),
        decl_kind,
        implements_trait: raw.implements_trait,
        receiver_type: raw.receiver_type_text.map(TypeRef::new),
        entry_point,
        module: raw.module,
        visibility,
        location,
        signature,
        body_hash,
        attributes: Vec::new(),
        flags: parse_flags_json(&raw.flags_json),
    })
}

/// Best-effort decode of the `symbols.flags` TEXT column (JSON array of
/// strings). Returns an empty vec on any parse error — schema-level
/// guarantees the column is never NULL, so this only triggers on a
/// genuinely corrupted row.
pub(super) fn parse_flags_json(raw: &str) -> Vec<String> {
    serde_json::from_str(raw).unwrap_or_default()
}

pub(super) fn position_to_u32(field: &str, value: i64) -> Result<u32, StorageError> {
    u32::try_from(value).map_err(|_| StorageError::InvalidStoredData {
        detail: format!("symbols.{field} out of u32 range: {value}"),
    })
}

pub(super) struct EdgeRowRaw {
    pub(super) id: i64,
    pub(super) kind_text: String,
    pub(super) to_symbol_id: Option<i64>,
    pub(super) to_unresolved: Option<String>,
    pub(super) attributes_json: String,
    pub(super) confidence_text: String,
    pub(super) receiver_type: Option<String>,
}

pub(super) fn read_edge_row(row: &Row<'_>) -> rusqlite::Result<EdgeRowRaw> {
    Ok(EdgeRowRaw {
        id: row.get(0)?,
        kind_text: row.get(1)?,
        to_symbol_id: row.get(2)?,
        to_unresolved: row.get(3)?,
        attributes_json: row.get(4)?,
        confidence_text: row.get(5)?,
        receiver_type: row.get(6)?,
    })
}

pub(super) fn collect_edge_rows(
    conn: &Connection,
    sql: &str,
    params: &[&dyn rusqlite::ToSql],
) -> Result<Vec<EdgeRowRaw>, StorageError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(params, read_edge_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub(super) fn build_edge(
    conn: &Connection,
    raw: EdgeRowRaw,
    from_fqdn: String,
) -> Result<RawEdge, StorageError> {
    let kind = edge_kind_from_sql_text(&raw.kind_text)?;
    let to = match (raw.to_symbol_id, raw.to_unresolved) {
        (Some(id), None) => {
            let fqdn =
                lookup_fqdn_by_id(conn, id)?.ok_or_else(|| StorageError::InvalidStoredData {
                    detail: format!("edges.to_symbol_id={id} points to deleted symbol"),
                })?;
            ResolvedOrUnresolved::Resolved { fqdn }
        }
        (None, Some(name)) => ResolvedOrUnresolved::Unresolved { name },
        _ => {
            return Err(StorageError::InvalidStoredData {
                detail: format!(
                    "edges.id={} violates XOR (to_symbol_id, to_unresolved)",
                    raw.id
                ),
            });
        }
    };
    let sites = load_edge_sites(conn, raw.id)?;
    let attributes: Vec<String> = serde_json::from_str(&raw.attributes_json).map_err(|e| {
        StorageError::InvalidStoredData {
            detail: format!("edges.id={} has malformed attributes JSON: {e}", raw.id),
        }
    })?;
    let confidence = edge_confidence_from_sql_text(&raw.confidence_text)?;
    Ok(RawEdge {
        from_fqdn,
        kind,
        to,
        sites,
        attributes,
        confidence,
        receiver_type: raw.receiver_type,
    })
}

pub(super) fn lookup_fqdn_by_id(
    conn: &Connection,
    id: i64,
) -> Result<Option<String>, StorageError> {
    let fqdn = conn
        .query_row("SELECT fqdn FROM symbols WHERE id = ?1", [id], |r| {
            r.get::<_, String>(0)
        })
        .optional()?;
    Ok(fqdn)
}

/// Resolve an fqdn → row id within the primary workspace. The
/// `UNIQUE(workspace_id, fqdn)` constraint permits the same fqdn in
/// multiple workspaces; the public `edges_from` / `edges_to` queries
/// answer about MY workspace, so the lookup is scoped accordingly.
pub(super) fn lookup_id_by_fqdn(
    conn: &Connection,
    fqdn: &str,
) -> Result<Option<i64>, StorageError> {
    let id = conn
        .query_row(
            "SELECT id FROM symbols WHERE workspace_id = ?1 AND fqdn = ?2",
            rusqlite::params![crate::storage::module_lookup::PRIMARY_WORKSPACE_ID, fqdn],
            |r| r.get::<_, i64>(0),
        )
        .optional()?;
    Ok(id)
}

pub(super) fn load_edge_sites(conn: &Connection, edge_id: i64) -> Result<Vec<Site>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT file_path, line, col FROM edge_sites WHERE edge_id = ?1 \
         ORDER BY file_path ASC, line ASC, col ASC",
    )?;
    let rows = stmt
        .query_map([edge_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|(file, line, col)| {
            Ok(Site {
                file,
                line: position_to_u32("edge_sites.line", line)?,
                col: position_to_u32("edge_sites.col", col)?,
            })
        })
        .collect()
}

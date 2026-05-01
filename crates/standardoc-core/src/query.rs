//! Read-only queries over the workspace index.
//!
//! Each function blocks while it borrows a connection from the writer pool
//! and returns owned IR types so callers can drop the result without holding
//! read locks. `RawSymbol::attributes` is reconstructed empty: attributes are
//! a provider artefact that is not persisted in v1 (cf. SCHEMA §2.4 — no
//! attributes column on `symbols`). Bridge-encoded edge targets fall back to
//! `Unresolved { name }` because the stored `to_unresolved` text discards the
//! `BridgeKind` distinction (cf. storage::conv::unresolved_to_storage).

use rusqlite::{Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use standardoc_ir::{
    Blake3Hash, EdgeKind, Language, LanguageKind, RawEdge, RawSymbol, ResolvedOrUnresolved, Site,
    SymbolLocation,
};

use crate::storage::conv::{
    edge_kind_from_sql_text, json_to_signature, kind_from_sql_text, language_from_sql_text,
    visibility_from_sql_text,
};
use crate::storage::error::StorageError;
use crate::storage::handle::IndexHandle;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo {
    pub path: String,
    pub content_hash: String,
    pub language: Language,
    pub last_scanned_ms: i64,
    pub byte_size: i64,
    pub last_scan_error: Option<String>,
}

/// Aggregated read-side context for a single symbol — the shape consumed by
/// LSP `hover` and (later) MCP `get_context`. Day-1 surface stays minimal:
/// the inferred enrichment description plus the user-authored document
/// description. Other enrichment / document fields (params, examples, ...)
/// can be surfaced additively post-beta.1 without breaking callers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolContext {
    pub symbol: RawSymbol,
    pub enrichment_description: Option<String>,
    pub document_description: Option<String>,
}

/// One neighbor in a [`SymbolContextWithNeighbors`] graph view: the edge kind
/// that links us to it, the (resolved or unresolved) target reference, and
/// the resolved [`RawSymbol`] when one exists in the index. `resolved_symbol`
/// is `None` for the cheap shape (`depth = 1`) regardless of resolution
/// status; at `depth = 2` it carries the looked-up `RawSymbol` for `Resolved`
/// targets and stays `None` for `Unresolved` ones (external / not yet indexed)
/// or when the row was deleted between the edge load and the symbol load.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NeighborSymbol {
    pub edge_kind: EdgeKind,
    pub target: ResolvedOrUnresolved,
    pub resolved_symbol: Option<RawSymbol>,
}

/// Composite shape consumed by the MCP `get_context` tool. Wraps the LSP-side
/// [`SymbolContext`] (signature + descriptions) with four pre-grouped neighbor
/// vectors selected for LLM consumption (cf. VISION.md L251-262 "chunk parfait
/// pour LLM"):
///
/// - `callers`     — `edges_to(fqdn)`   filtered to [`EdgeKind::Calls`]
/// - `callees`     — `edges_from(fqdn)` filtered to [`EdgeKind::Calls`]
/// - `imports`     — `edges_from(fqdn)` filtered to [`EdgeKind::Imports`]
/// - `imported_by` — `edges_to(fqdn)`   filtered to [`EdgeKind::Imports`]
///
/// Other [`EdgeKind`] variants (Implements / Extends / References / UsesType
/// / Defines / ExposesApi) are intentionally skipped day-1 (VISION L97 — fewer
/// fields = clearer LLM contract). They can be added additively post-beta.1
/// without breaking callers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolContextWithNeighbors {
    pub context: SymbolContext,
    pub callers: Vec<NeighborSymbol>,
    pub callees: Vec<NeighborSymbol>,
    pub imports: Vec<NeighborSymbol>,
    pub imported_by: Vec<NeighborSymbol>,
}

const SYMBOL_COLUMNS: &str = "fqdn, name, kind, language_kind, module, visibility, \
     file_path, start_line, end_line, start_col, end_col, \
     signature_json, body_hash";

fn with_conn<F, R>(handle: &IndexHandle, f: F) -> Result<R, StorageError>
where
    F: FnOnce(&Connection) -> Result<R, StorageError>,
{
    let pool = handle.pool()?;
    let conn = pool.get()?;
    f(&conn)
}

pub fn symbol_by_fqdn(handle: &IndexHandle, fqdn: &str) -> Result<Option<RawSymbol>, StorageError> {
    with_conn(handle, |conn| {
        let raw = conn
            .query_row(
                &format!("SELECT {SYMBOL_COLUMNS} FROM symbols WHERE fqdn = ?1"),
                [fqdn],
                read_symbol_row,
            )
            .optional()?;
        raw.map(build_symbol).transpose()
    })
}

pub fn symbols_by_name(
    handle: &IndexHandle,
    name: &str,
    limit: usize,
) -> Result<Vec<RawSymbol>, StorageError> {
    let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
    with_conn(handle, |conn| {
        let sql = format!(
            "SELECT {SYMBOL_COLUMNS} FROM symbols WHERE name = ?1 \
             ORDER BY fqdn ASC LIMIT ?2"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(rusqlite::params![name, limit_i64], read_symbol_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(build_symbol).collect()
    })
}

pub fn symbols_by_file(handle: &IndexHandle, path: &str) -> Result<Vec<RawSymbol>, StorageError> {
    with_conn(handle, |conn| {
        let sql = format!(
            "SELECT {SYMBOL_COLUMNS} FROM symbols WHERE file_path = ?1 \
             ORDER BY start_line ASC, start_col ASC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map([path], read_symbol_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(build_symbol).collect()
    })
}

pub fn edges_from(handle: &IndexHandle, fqdn: &str) -> Result<Vec<RawEdge>, StorageError> {
    with_conn(handle, |conn| {
        let Some(id) = lookup_id_by_fqdn(conn, fqdn)? else {
            return Ok(Vec::new());
        };
        let rows = collect_edge_rows(
            conn,
            "SELECT id, from_symbol_id, kind, to_symbol_id, to_unresolved \
             FROM edges WHERE from_symbol_id = ?1 ORDER BY id ASC",
            rusqlite::params![id],
        )?;
        rows.into_iter()
            .map(|row| build_edge(conn, row, fqdn.to_owned()))
            .collect()
    })
}

pub fn edges_to(handle: &IndexHandle, fqdn: &str) -> Result<Vec<RawEdge>, StorageError> {
    with_conn(handle, |conn| {
        let target_id = lookup_id_by_fqdn(conn, fqdn)?;
        let rows = collect_edge_rows(
            conn,
            "SELECT id, from_symbol_id, kind, to_symbol_id, to_unresolved \
             FROM edges \
             WHERE (?1 IS NOT NULL AND to_symbol_id = ?1) \
                OR to_unresolved = ?2 \
             ORDER BY id ASC",
            rusqlite::params![target_id, fqdn],
        )?;
        rows.into_iter()
            .map(|row| {
                let from_fqdn = lookup_fqdn_by_id(conn, row.from_symbol_id)?.ok_or_else(|| {
                    StorageError::InvalidStoredData {
                        detail: format!(
                            "edges.from_symbol_id={} points to deleted symbol",
                            row.from_symbol_id
                        ),
                    }
                })?;
                build_edge(conn, row, from_fqdn)
            })
            .collect()
    })
}

pub fn search_text(
    handle: &IndexHandle,
    query: &str,
    limit: usize,
) -> Result<Vec<RawSymbol>, StorageError> {
    let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
    with_conn(handle, |conn| {
        let mut stmt = conn.prepare(
            "SELECT s.fqdn, s.name, s.kind, s.language_kind, s.module, s.visibility, \
                    s.file_path, s.start_line, s.end_line, s.start_col, s.end_col, \
                    s.signature_json, s.body_hash \
             FROM symbols_fts f \
             JOIN symbols s ON s.id = f.rowid \
             WHERE symbols_fts MATCH ?1 \
             ORDER BY rank \
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![query, limit_i64], read_symbol_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(build_symbol).collect()
    })
}

/// Returns the symbol whose source range contains `(line, col)` in `file_path`.
///
/// When several symbols nest at that point (module > impl > fn), the smallest
/// containing range wins ("deepest"). Used by LSP `goto_definition`,
/// `references` and `hover` to resolve a cursor position to a symbol id.
pub fn symbol_at_position(
    handle: &IndexHandle,
    file_path: &str,
    line: u32,
    col: u32,
) -> Result<Option<RawSymbol>, StorageError> {
    let line_i64 = i64::from(line);
    let col_i64 = i64::from(col);
    with_conn(handle, |conn| {
        let raw = conn
            .query_row(
                &format!(
                    "SELECT {SYMBOL_COLUMNS} FROM symbols \
                     WHERE file_path = ?1 \
                       AND start_line <= ?2 AND end_line >= ?2 \
                       AND (start_line < ?2 OR start_col <= ?3) \
                       AND (end_line   > ?2 OR end_col   >= ?3) \
                     ORDER BY (end_line - start_line) * 1000 + (end_col - start_col) ASC \
                     LIMIT 1"
                ),
                rusqlite::params![file_path, line_i64, col_i64],
                read_symbol_row,
            )
            .optional()?;
        raw.map(build_symbol).transpose()
    })
}

/// Aggregates the symbol row with its optional enrichment + document
/// descriptions in a single SQL round-trip. Returns `None` if no symbol
/// matches `fqdn`. Used by LSP `hover` (renders all three fields as
/// markdown) and reused by MCP `get_context` (post-beta.1).
pub fn context_for_symbol(
    handle: &IndexHandle,
    fqdn: &str,
) -> Result<Option<SymbolContext>, StorageError> {
    with_conn(handle, |conn| {
        let raw = conn
            .query_row(
                &format!(
                    "SELECT {SYMBOL_COLUMNS}, e.description, d.description \
                     FROM symbols s \
                     LEFT JOIN enrichments e ON e.symbol_id = s.id \
                     LEFT JOIN documents   d ON d.symbol_id = s.id \
                     WHERE s.fqdn = ?1"
                ),
                [fqdn],
                |row| {
                    Ok((
                        read_symbol_row(row)?,
                        row.get::<_, Option<String>>(13)?,
                        row.get::<_, Option<String>>(14)?,
                    ))
                },
            )
            .optional()?;
        let Some((symbol_raw, enrichment_description, document_description)) = raw else {
            return Ok(None);
        };
        let symbol = build_symbol(symbol_raw)?;
        Ok(Some(SymbolContext {
            symbol,
            enrichment_description,
            document_description,
        }))
    })
}

/// MCP-side composition: load the [`SymbolContext`] for `fqdn` and attach its
/// 1-hop neighborhood pre-grouped into callers / callees / imports /
/// imported_by (see [`SymbolContextWithNeighbors`]). Returns `None` when the
/// root symbol is absent from the index.
///
/// `depth` is hard-clamped to `1..=2` and selects the shape's richness:
///
/// - `depth = 1` (cheap) — every [`NeighborSymbol::resolved_symbol`] stays
///   `None`. Output carries only target FQDNs + edge kinds. Suited to graph
///   exploration ("who touches this symbol?") without paying any per-neighbor
///   `symbol_by_fqdn` lookup.
/// - `depth = 2` (rich) — `resolved_symbol` is filled via [`symbol_by_fqdn`]
///   for every [`ResolvedOrUnresolved::Resolved`] target. `Unresolved` targets
///   stay `None` since their `RawSymbol` is, by definition, absent. Suited to
///   reasoning ("give me the exploitable LLM chunk").
///
/// Multi-hop graph traversal (`depth >= 3`) is not yet supported: the day-1
/// shape exposes a single neighbor list per direction, and recursing requires
/// extending [`NeighborSymbol`] with a nested context field — additive
/// post-beta.1.
#[allow(clippy::similar_names)]
pub fn context_for_symbol_with_neighbors(
    handle: &IndexHandle,
    fqdn: &str,
    depth: u8,
) -> Result<Option<SymbolContextWithNeighbors>, StorageError> {
    let depth = depth.clamp(1, 2);
    let Some(context) = context_for_symbol(handle, fqdn)? else {
        return Ok(None);
    };

    let mut callees = Vec::new();
    let mut imports = Vec::new();
    for edge in edges_from(handle, fqdn)? {
        match edge.kind {
            EdgeKind::Calls => callees.push(neighbor_outbound(handle, edge, depth)?),
            EdgeKind::Imports => imports.push(neighbor_outbound(handle, edge, depth)?),
            _ => {}
        }
    }

    let mut callers = Vec::new();
    let mut imported_by = Vec::new();
    for edge in edges_to(handle, fqdn)? {
        match edge.kind {
            EdgeKind::Calls => callers.push(neighbor_inbound(handle, edge, depth)?),
            EdgeKind::Imports => imported_by.push(neighbor_inbound(handle, edge, depth)?),
            _ => {}
        }
    }

    Ok(Some(SymbolContextWithNeighbors {
        context,
        callers,
        callees,
        imports,
        imported_by,
    }))
}

fn neighbor_outbound(
    handle: &IndexHandle,
    edge: RawEdge,
    depth: u8,
) -> Result<NeighborSymbol, StorageError> {
    let resolved_symbol = if depth >= 2 {
        match &edge.to {
            ResolvedOrUnresolved::Resolved { fqdn } => symbol_by_fqdn(handle, fqdn)?,
            _ => None,
        }
    } else {
        None
    };
    Ok(NeighborSymbol {
        edge_kind: edge.kind,
        target: edge.to,
        resolved_symbol,
    })
}

fn neighbor_inbound(
    handle: &IndexHandle,
    edge: RawEdge,
    depth: u8,
) -> Result<NeighborSymbol, StorageError> {
    let resolved_symbol = if depth >= 2 {
        symbol_by_fqdn(handle, &edge.from_fqdn)?
    } else {
        None
    };
    Ok(NeighborSymbol {
        edge_kind: edge.kind,
        target: ResolvedOrUnresolved::Resolved {
            fqdn: edge.from_fqdn,
        },
        resolved_symbol,
    })
}

pub fn file_info(handle: &IndexHandle, path: &str) -> Result<Option<FileInfo>, StorageError> {
    with_conn(handle, |conn| {
        let raw = conn
            .query_row(
                "SELECT path, content_hash, language, last_scanned, byte_size, last_scan_error \
                 FROM files WHERE path = ?1",
                [path],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((p, content_hash, language_text, last_scanned_ms, byte_size, last_scan_error)) =
            raw
        else {
            return Ok(None);
        };
        let language = language_from_sql_text(&language_text)?;
        Ok(Some(FileInfo {
            path: p,
            content_hash,
            language,
            last_scanned_ms,
            byte_size,
            last_scan_error,
        }))
    })
}

struct SymbolRowRaw {
    fqdn: String,
    name: String,
    kind_text: String,
    language_kind_text: String,
    module: Option<String>,
    visibility_text: String,
    file_path: String,
    start_line: i64,
    end_line: i64,
    start_col: i64,
    end_col: i64,
    signature_json: Option<String>,
    body_hash_hex: Option<String>,
}

fn read_symbol_row(row: &Row<'_>) -> rusqlite::Result<SymbolRowRaw> {
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
    })
}

fn build_symbol(raw: SymbolRowRaw) -> Result<RawSymbol, StorageError> {
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
        module: raw.module,
        visibility,
        location,
        signature,
        body_hash,
        attributes: Vec::new(),
    })
}

fn position_to_u32(field: &str, value: i64) -> Result<u32, StorageError> {
    u32::try_from(value).map_err(|_| StorageError::InvalidStoredData {
        detail: format!("symbols.{field} out of u32 range: {value}"),
    })
}

struct EdgeRowRaw {
    id: i64,
    from_symbol_id: i64,
    kind_text: String,
    to_symbol_id: Option<i64>,
    to_unresolved: Option<String>,
}

fn read_edge_row(row: &Row<'_>) -> rusqlite::Result<EdgeRowRaw> {
    Ok(EdgeRowRaw {
        id: row.get(0)?,
        from_symbol_id: row.get(1)?,
        kind_text: row.get(2)?,
        to_symbol_id: row.get(3)?,
        to_unresolved: row.get(4)?,
    })
}

fn collect_edge_rows(
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

fn build_edge(
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
    Ok(RawEdge {
        from_fqdn,
        kind,
        to,
        sites,
    })
}

fn lookup_fqdn_by_id(conn: &Connection, id: i64) -> Result<Option<String>, StorageError> {
    let fqdn = conn
        .query_row("SELECT fqdn FROM symbols WHERE id = ?1", [id], |r| {
            r.get::<_, String>(0)
        })
        .optional()?;
    Ok(fqdn)
}

fn lookup_id_by_fqdn(conn: &Connection, fqdn: &str) -> Result<Option<i64>, StorageError> {
    let id = conn
        .query_row("SELECT id FROM symbols WHERE fqdn = ?1", [fqdn], |r| {
            r.get::<_, i64>(0)
        })
        .optional()?;
    Ok(id)
}

fn load_edge_sites(conn: &Connection, edge_id: i64) -> Result<Vec<Site>, StorageError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::IngestCommand;
    use crate::storage::edges::insert_edge;
    use crate::storage::edge_sites::insert_edge_sites;
    use crate::storage::files::{FileInput, upsert_file};
    use crate::storage::symbols::{SymbolInsertContext, insert_symbol};
    use rusqlite::Connection;
    use standardoc_ir::{
        Blake3Hash, EdgeKind, ExtractedFile, Kind, LanguageKind, Modifiers, Param, RawEdge,
        RawSymbol, Signature, Site, SourceOrigin, SymbolLocation, TypeRef, Visibility,
    };
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    fn open_handle() -> (TempDir, IndexHandle) {
        let dir = tempfile::tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        (dir, handle)
    }

    fn wait_revision_at_least(handle: &IndexHandle, target: u64) {
        let start = Instant::now();
        while handle.revision() < target {
            assert!(
                start.elapsed() <= Duration::from_secs(5),
                "revision did not reach {target} (was {})",
                handle.revision()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn seed_file(conn: &Connection, path: &str) {
        upsert_file(
            conn,
            &FileInput {
                path: path.into(),
                content_hash: Blake3Hash::default(),
                language: Language::Rust,
                byte_size: 100,
                last_scanned: 1_700_000_000_000,
                last_scan_error: None,
            },
        )
        .unwrap();
    }

    fn seed_symbol(
        conn: &Connection,
        file: &str,
        name: &str,
        fqdn: &str,
        line: u32,
    ) -> (i64, RawSymbol) {
        let sym = RawSymbol {
            name: name.into(),
            fqdn: fqdn.into(),
            kind: Kind::Function,
            language_kind: LanguageKind::from("fn_item"),
            module: None,
            visibility: Visibility::Public,
            location: SymbolLocation {
                file: file.into(),
                start_line: line,
                end_line: line + 5,
                start_col: 0,
                end_col: 1,
            },
            signature: None,
            body_hash: Some(Blake3Hash::new([0xab; 32])),
            attributes: vec![],
        };
        let id = insert_symbol(
            conn,
            &sym,
            SymbolInsertContext {
                file_path: file,
                language: Language::Rust,
                is_external: false,
                source_origin: SourceOrigin::Workspace,
            },
        )
        .unwrap();
        (id, sym)
    }

    #[test]
    fn symbol_by_fqdn_returns_some_when_present() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol(&conn, "src/main.rs", "foo", "crate::foo", 10);
        }
        let got = symbol_by_fqdn(&handle, "crate::foo").unwrap().unwrap();
        assert_eq!(got.name, "foo");
        assert_eq!(got.fqdn, "crate::foo");
        assert_eq!(got.kind, Kind::Function);
        assert_eq!(got.location.start_line, 10);
    }

    #[test]
    fn symbol_by_fqdn_returns_none_when_absent() {
        let (_dir, handle) = open_handle();
        assert_eq!(symbol_by_fqdn(&handle, "crate::ghost").unwrap(), None);
    }

    #[test]
    fn symbol_by_fqdn_round_trips_signature_and_body_hash() {
        let (_dir, handle) = open_handle();
        let body = Blake3Hash::new([0xcd; 32]);
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            let sym = RawSymbol {
                name: "f".into(),
                fqdn: "crate::f".into(),
                kind: Kind::Function,
                language_kind: LanguageKind::from("fn_item"),
                module: Some("m".into()),
                visibility: Visibility::Crate,
                location: SymbolLocation {
                    file: "src/main.rs".into(),
                    start_line: 1,
                    end_line: 2,
                    start_col: 0,
                    end_col: 1,
                },
                signature: Some(Signature {
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
                }),
                body_hash: Some(body),
                attributes: vec![],
            };
            insert_symbol(
                &conn,
                &sym,
                SymbolInsertContext {
                    file_path: "src/main.rs",
                    language: Language::Rust,
                    is_external: false,
                    source_origin: SourceOrigin::Workspace,
                },
            )
            .unwrap();
        }
        let got = symbol_by_fqdn(&handle, "crate::f").unwrap().unwrap();
        assert_eq!(got.module.as_deref(), Some("m"));
        assert_eq!(got.visibility, Visibility::Crate);
        assert_eq!(got.body_hash, Some(body));
        let sig = got.signature.expect("signature must round-trip");
        assert!(sig.modifiers.is_async);
        assert_eq!(sig.params[0].name, "x");
    }

    #[test]
    fn symbols_by_name_returns_matches_ordered_by_fqdn_with_limit() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/a.rs");
            seed_file(&conn, "src/b.rs");
            seed_symbol(&conn, "src/b.rs", "tick", "crate::b::tick", 1);
            seed_symbol(&conn, "src/a.rs", "tick", "crate::a::tick", 1);
            seed_symbol(&conn, "src/a.rs", "other", "crate::a::other", 2);
        }
        let got = symbols_by_name(&handle, "tick", 50).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].fqdn, "crate::a::tick");
        assert_eq!(got[1].fqdn, "crate::b::tick");

        let got_limited = symbols_by_name(&handle, "tick", 1).unwrap();
        assert_eq!(got_limited.len(), 1);
        assert_eq!(got_limited[0].fqdn, "crate::a::tick");
    }

    #[test]
    fn symbols_by_name_empty_when_no_match() {
        let (_dir, handle) = open_handle();
        let got = symbols_by_name(&handle, "ghost", 50).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn symbols_by_file_orders_by_position() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol(&conn, "src/main.rs", "c", "crate::c", 30);
            seed_symbol(&conn, "src/main.rs", "a", "crate::a", 1);
            seed_symbol(&conn, "src/main.rs", "b", "crate::b", 15);
        }
        let got = symbols_by_file(&handle, "src/main.rs").unwrap();
        let fqdns: Vec<_> = got.iter().map(|s| s.fqdn.as_str()).collect();
        assert_eq!(fqdns, vec!["crate::a", "crate::b", "crate::c"]);
    }

    #[test]
    fn edges_from_returns_resolved_and_unresolved_targets() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            let (caller_id, _) = seed_symbol(&conn, "src/main.rs", "caller", "crate::caller", 1);
            seed_symbol(&conn, "src/main.rs", "callee", "crate::callee", 10);
            insert_edge(
                &conn,
                caller_id,
                &RawEdge {
                    from_fqdn: "crate::caller".into(),
                    kind: EdgeKind::Calls,
                    to: ResolvedOrUnresolved::Resolved {
                        fqdn: "crate::callee".into(),
                    },
                    sites: vec![],
                },
            )
            .unwrap();
            insert_edge(
                &conn,
                caller_id,
                &RawEdge {
                    from_fqdn: "crate::caller".into(),
                    kind: EdgeKind::Calls,
                    to: ResolvedOrUnresolved::Unresolved {
                        name: "external::thing".into(),
                    },
                    sites: vec![],
                },
            )
            .unwrap();
        }
        let edges = edges_from(&handle, "crate::caller").unwrap();
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0].from_fqdn, "crate::caller");
        assert!(matches!(
            &edges[0].to,
            ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "crate::callee"
        ));
        assert!(matches!(
            &edges[1].to,
            ResolvedOrUnresolved::Unresolved { name } if name == "external::thing"
        ));
    }

    #[test]
    fn edges_from_empty_when_symbol_unknown() {
        let (_dir, handle) = open_handle();
        let got = edges_from(&handle, "crate::ghost").unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn edges_from_loads_sites_ordered() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            let (caller_id, _) = seed_symbol(&conn, "src/main.rs", "caller", "crate::caller", 1);
            let edge_id = insert_edge(
                &conn,
                caller_id,
                &RawEdge {
                    from_fqdn: "crate::caller".into(),
                    kind: EdgeKind::Calls,
                    to: ResolvedOrUnresolved::Unresolved {
                        name: "thing".into(),
                    },
                    sites: vec![],
                },
            )
            .unwrap();
            insert_edge_sites(
                &conn,
                edge_id,
                &[
                    Site {
                        file: "src/main.rs".into(),
                        line: 20,
                        col: 4,
                    },
                    Site {
                        file: "src/main.rs".into(),
                        line: 5,
                        col: 0,
                    },
                ],
            )
            .unwrap();
        }
        let edges = edges_from(&handle, "crate::caller").unwrap();
        assert_eq!(edges.len(), 1);
        let sites = &edges[0].sites;
        assert_eq!(sites.len(), 2);
        assert_eq!(sites[0].line, 5);
        assert_eq!(sites[1].line, 20);
    }

    #[test]
    fn edges_to_finds_resolved_inbound() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            let (caller_id, _) = seed_symbol(&conn, "src/main.rs", "caller", "crate::caller", 1);
            seed_symbol(&conn, "src/main.rs", "callee", "crate::callee", 10);
            insert_edge(
                &conn,
                caller_id,
                &RawEdge {
                    from_fqdn: "crate::caller".into(),
                    kind: EdgeKind::Calls,
                    to: ResolvedOrUnresolved::Resolved {
                        fqdn: "crate::callee".into(),
                    },
                    sites: vec![],
                },
            )
            .unwrap();
        }
        let edges = edges_to(&handle, "crate::callee").unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from_fqdn, "crate::caller");
        assert!(matches!(
            &edges[0].to,
            ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "crate::callee"
        ));
    }

    #[test]
    fn edges_to_finds_unresolved_inbound_for_unknown_fqdn() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            let (caller_id, _) = seed_symbol(&conn, "src/main.rs", "caller", "crate::caller", 1);
            insert_edge(
                &conn,
                caller_id,
                &RawEdge {
                    from_fqdn: "crate::caller".into(),
                    kind: EdgeKind::Calls,
                    to: ResolvedOrUnresolved::Unresolved {
                        name: "external::thing".into(),
                    },
                    sites: vec![],
                },
            )
            .unwrap();
        }
        let edges = edges_to(&handle, "external::thing").unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from_fqdn, "crate::caller");
    }

    #[test]
    fn search_text_returns_match_via_fts() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol(&conn, "src/main.rs", "create_user", "crate::user::create_user", 1);
            seed_symbol(&conn, "src/main.rs", "delete_user", "crate::user::delete_user", 5);
            seed_symbol(&conn, "src/main.rs", "noise", "crate::noise", 10);
        }
        let got = search_text(&handle, "create_user", 50).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "create_user");
    }

    #[test]
    fn search_text_respects_limit() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol(&conn, "src/main.rs", "tick_one", "crate::tick_one", 1);
            seed_symbol(&conn, "src/main.rs", "tick_two", "crate::tick_two", 2);
        }
        let got = search_text(&handle, "tick_one OR tick_two", 1).unwrap();
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn file_info_returns_some_with_data() {
        let (_dir, handle) = open_handle();
        let hash_hex = Blake3Hash::new([0xab; 32]).to_hex();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            upsert_file(
                &conn,
                &FileInput {
                    path: "src/main.rs".into(),
                    content_hash: Blake3Hash::new([0xab; 32]),
                    language: Language::TypeScript,
                    byte_size: 4096,
                    last_scanned: 1_700_000_000_000,
                    last_scan_error: Some("boom".into()),
                },
            )
            .unwrap();
        }
        let got = file_info(&handle, "src/main.rs").unwrap().unwrap();
        assert_eq!(got.path, "src/main.rs");
        assert_eq!(got.content_hash, hash_hex);
        assert_eq!(got.language, Language::TypeScript);
        assert_eq!(got.byte_size, 4096);
        assert_eq!(got.last_scanned_ms, 1_700_000_000_000);
        assert_eq!(got.last_scan_error.as_deref(), Some("boom"));
    }

    #[test]
    fn file_info_returns_none_when_absent() {
        let (_dir, handle) = open_handle();
        assert_eq!(file_info(&handle, "no/such.rs").unwrap(), None);
    }

    #[test]
    fn query_observes_writer_thread_upsert() {
        let (_dir, handle) = open_handle();
        let extracted = ExtractedFile {
            file: "src/main.rs".into(),
            language: Language::Rust,
            source_origin: SourceOrigin::Workspace,
            is_external: false,
            content_hash: Blake3Hash::new([0xee; 32]),
            byte_size: 100,
            symbols: vec![RawSymbol {
                name: "boot".into(),
                fqdn: "crate::boot".into(),
                kind: Kind::Function,
                language_kind: LanguageKind::from("fn_item"),
                module: None,
                visibility: Visibility::Public,
                location: SymbolLocation {
                    file: "src/main.rs".into(),
                    start_line: 1,
                    end_line: 2,
                    start_col: 0,
                    end_col: 1,
                },
                signature: None,
                body_hash: Some(Blake3Hash::new([0x02; 32])),
                attributes: vec![],
            }],
            edges: vec![],
            call_sites: vec![],
        };
        handle
            .submit_blocking(IngestCommand::UpsertFile {
                path: "src/main.rs".into(),
                extracted,
            })
            .unwrap();
        wait_revision_at_least(&handle, 1);

        let got = symbol_by_fqdn(&handle, "crate::boot").unwrap().unwrap();
        assert_eq!(got.name, "boot");
    }

    fn ranged_loc(
        file: &str,
        start_line: u32,
        end_line: u32,
        start_col: u32,
        end_col: u32,
    ) -> SymbolLocation {
        SymbolLocation {
            file: file.into(),
            start_line,
            end_line,
            start_col,
            end_col,
        }
    }

    fn insert_ranged_symbol(conn: &Connection, fqdn: &str, kind: Kind, location: SymbolLocation) {
        let file = location.file.clone();
        let sym = RawSymbol {
            name: fqdn.rsplit("::").next().unwrap_or(fqdn).into(),
            fqdn: fqdn.into(),
            kind,
            language_kind: LanguageKind::from("fn_item"),
            module: None,
            visibility: Visibility::Public,
            location,
            signature: None,
            body_hash: None,
            attributes: vec![],
        };
        insert_symbol(
            conn,
            &sym,
            SymbolInsertContext {
                file_path: &file,
                language: Language::Rust,
                is_external: false,
                source_origin: SourceOrigin::Workspace,
            },
        )
        .unwrap();
    }

    #[test]
    fn symbol_at_position_returns_match_when_in_range() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            insert_ranged_symbol(
                &conn,
                "crate::foo",
                Kind::Function,
                ranged_loc("src/main.rs", 10, 20, 0, 1),
            );
        }
        let got = symbol_at_position(&handle, "src/main.rs", 15, 0)
            .unwrap()
            .expect("position 15:0 lies inside the function body");
        assert_eq!(got.fqdn, "crate::foo");
    }

    #[test]
    fn symbol_at_position_picks_smallest_range_when_nested() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/lib.rs");
            insert_ranged_symbol(
                &conn,
                "crate::outer",
                Kind::Module,
                ranged_loc("src/lib.rs", 1, 100, 0, 0),
            );
            insert_ranged_symbol(
                &conn,
                "crate::outer::inner",
                Kind::Function,
                ranged_loc("src/lib.rs", 10, 20, 4, 1),
            );
        }
        let got = symbol_at_position(&handle, "src/lib.rs", 15, 0)
            .unwrap()
            .expect("position lies inside both module and inner fn");
        assert_eq!(got.fqdn, "crate::outer::inner");
    }

    #[test]
    fn symbol_at_position_returns_none_when_out_of_range() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            insert_ranged_symbol(
                &conn,
                "crate::foo",
                Kind::Function,
                ranged_loc("src/main.rs", 10, 20, 0, 1),
            );
        }
        assert_eq!(
            symbol_at_position(&handle, "src/main.rs", 100, 0).unwrap(),
            None
        );
    }

    #[test]
    fn context_for_symbol_returns_none_when_unknown() {
        let (_dir, handle) = open_handle();
        assert_eq!(context_for_symbol(&handle, "crate::ghost").unwrap(), None);
    }

    #[test]
    fn context_for_symbol_returns_symbol_only_when_no_metadata() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol(&conn, "src/main.rs", "foo", "crate::foo", 1);
        }
        let ctx = context_for_symbol(&handle, "crate::foo")
            .unwrap()
            .expect("symbol exists");
        assert_eq!(ctx.symbol.fqdn, "crate::foo");
        assert_eq!(ctx.enrichment_description, None);
        assert_eq!(ctx.document_description, None);
    }

    #[test]
    fn context_for_symbol_aggregates_enrichment_and_document() {
        use crate::storage::documents::{DocumentInput, upsert_document};
        use crate::storage::enrichments::{ConfidenceLevel, EnrichmentInput, upsert_enrichment};

        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            let (id, _) = seed_symbol(&conn, "src/main.rs", "foo", "crate::foo", 1);
            upsert_enrichment(
                &conn,
                &EnrichmentInput {
                    symbol_id: id,
                    description: Some("inferred summary".into()),
                    params_json: None,
                    returns_json: None,
                    modifiers_json: None,
                    confidence: ConfidenceLevel::High,
                    sources_json: "[]".into(),
                    last_updated: 0,
                },
            )
            .unwrap();
            upsert_document(
                &conn,
                &DocumentInput {
                    symbol_id: id,
                    description: Some("user-authored doc".into()),
                    ..DocumentInput::default()
                },
            )
            .unwrap();
        }
        let ctx = context_for_symbol(&handle, "crate::foo")
            .unwrap()
            .expect("symbol exists");
        assert_eq!(ctx.symbol.fqdn, "crate::foo");
        assert_eq!(
            ctx.enrichment_description.as_deref(),
            Some("inferred summary")
        );
        assert_eq!(
            ctx.document_description.as_deref(),
            Some("user-authored doc")
        );
    }

    fn seed_call_edge(conn: &Connection, from_id: i64, from_fqdn: &str, to: ResolvedOrUnresolved) {
        insert_edge(
            conn,
            from_id,
            &RawEdge {
                from_fqdn: from_fqdn.into(),
                kind: EdgeKind::Calls,
                to,
                sites: vec![],
            },
        )
        .unwrap();
    }

    fn seed_import_edge(
        conn: &Connection,
        from_id: i64,
        from_fqdn: &str,
        to: ResolvedOrUnresolved,
    ) {
        insert_edge(
            conn,
            from_id,
            &RawEdge {
                from_fqdn: from_fqdn.into(),
                kind: EdgeKind::Imports,
                to,
                sites: vec![],
            },
        )
        .unwrap();
    }

    #[test]
    fn context_with_neighbors_returns_none_when_unknown() {
        let (_dir, handle) = open_handle();
        assert_eq!(
            context_for_symbol_with_neighbors(&handle, "crate::ghost", 1).unwrap(),
            None
        );
    }

    #[test]
    fn context_with_neighbors_groups_edges_by_kind_and_direction() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            let (foo_id, _) = seed_symbol(&conn, "src/main.rs", "foo", "crate::foo", 1);
            let (bar_id, _) = seed_symbol(&conn, "src/main.rs", "bar", "crate::bar", 10);
            seed_symbol(&conn, "src/main.rs", "baz", "crate::baz", 20);
            seed_call_edge(
                &conn,
                foo_id,
                "crate::foo",
                ResolvedOrUnresolved::Resolved {
                    fqdn: "crate::baz".into(),
                },
            );
            seed_call_edge(
                &conn,
                foo_id,
                "crate::foo",
                ResolvedOrUnresolved::Unresolved {
                    name: "external::thing".into(),
                },
            );
            seed_import_edge(
                &conn,
                foo_id,
                "crate::foo",
                ResolvedOrUnresolved::Resolved {
                    fqdn: "crate::baz".into(),
                },
            );
            seed_call_edge(
                &conn,
                bar_id,
                "crate::bar",
                ResolvedOrUnresolved::Resolved {
                    fqdn: "crate::foo".into(),
                },
            );
            seed_import_edge(
                &conn,
                bar_id,
                "crate::bar",
                ResolvedOrUnresolved::Resolved {
                    fqdn: "crate::foo".into(),
                },
            );
        }
        let ctx = context_for_symbol_with_neighbors(&handle, "crate::foo", 1)
            .unwrap()
            .expect("foo exists");
        assert_eq!(ctx.context.symbol.fqdn, "crate::foo");
        assert_eq!(ctx.callees.len(), 2, "callees include resolved + unresolved");
        assert_eq!(ctx.imports.len(), 1);
        assert_eq!(ctx.callers.len(), 1);
        assert_eq!(ctx.imported_by.len(), 1);
        assert!(matches!(
            &ctx.callers[0].target,
            ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "crate::bar"
        ));
        assert!(matches!(
            &ctx.imported_by[0].target,
            ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "crate::bar"
        ));
    }

    #[test]
    fn context_with_neighbors_depth_one_omits_resolved_symbol() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            let (foo_id, _) = seed_symbol(&conn, "src/main.rs", "foo", "crate::foo", 1);
            seed_symbol(&conn, "src/main.rs", "baz", "crate::baz", 20);
            seed_call_edge(
                &conn,
                foo_id,
                "crate::foo",
                ResolvedOrUnresolved::Resolved {
                    fqdn: "crate::baz".into(),
                },
            );
        }
        let ctx = context_for_symbol_with_neighbors(&handle, "crate::foo", 1)
            .unwrap()
            .unwrap();
        assert_eq!(ctx.callees.len(), 1);
        assert!(
            ctx.callees[0].resolved_symbol.is_none(),
            "depth=1 must keep resolved_symbol = None even for Resolved targets"
        );
    }

    #[test]
    fn context_with_neighbors_depth_two_populates_resolved_symbol_for_resolved_only() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            let (foo_id, _) = seed_symbol(&conn, "src/main.rs", "foo", "crate::foo", 1);
            seed_symbol(&conn, "src/main.rs", "baz", "crate::baz", 20);
            seed_call_edge(
                &conn,
                foo_id,
                "crate::foo",
                ResolvedOrUnresolved::Resolved {
                    fqdn: "crate::baz".into(),
                },
            );
            seed_call_edge(
                &conn,
                foo_id,
                "crate::foo",
                ResolvedOrUnresolved::Unresolved {
                    name: "external::thing".into(),
                },
            );
        }
        let ctx = context_for_symbol_with_neighbors(&handle, "crate::foo", 2)
            .unwrap()
            .unwrap();
        assert_eq!(ctx.callees.len(), 2);
        let (resolved, unresolved): (Vec<_>, Vec<_>) = ctx
            .callees
            .iter()
            .partition(|n| matches!(n.target, ResolvedOrUnresolved::Resolved { .. }));
        let baz = resolved.first().expect("Resolved neighbor present");
        assert_eq!(
            baz.resolved_symbol.as_ref().map(|s| s.fqdn.as_str()),
            Some("crate::baz")
        );
        let external = unresolved.first().expect("Unresolved neighbor present");
        assert!(
            external.resolved_symbol.is_none(),
            "Unresolved targets stay None even at depth=2"
        );
    }

    #[test]
    fn context_with_neighbors_clamps_depth_above_two() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            let (foo_id, _) = seed_symbol(&conn, "src/main.rs", "foo", "crate::foo", 1);
            seed_symbol(&conn, "src/main.rs", "baz", "crate::baz", 20);
            seed_call_edge(
                &conn,
                foo_id,
                "crate::foo",
                ResolvedOrUnresolved::Resolved {
                    fqdn: "crate::baz".into(),
                },
            );
        }
        let ctx_clamped = context_for_symbol_with_neighbors(&handle, "crate::foo", 99)
            .unwrap()
            .unwrap();
        let ctx_two = context_for_symbol_with_neighbors(&handle, "crate::foo", 2)
            .unwrap()
            .unwrap();
        assert_eq!(ctx_clamped, ctx_two, "depth >= 2 must collapse to depth=2");
    }

    #[test]
    fn context_with_neighbors_skips_other_edge_kinds() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            let (foo_id, _) = seed_symbol(&conn, "src/main.rs", "foo", "crate::foo", 1);
            seed_symbol(&conn, "src/main.rs", "trait_t", "crate::T", 30);
            insert_edge(
                &conn,
                foo_id,
                &RawEdge {
                    from_fqdn: "crate::foo".into(),
                    kind: EdgeKind::Implements,
                    to: ResolvedOrUnresolved::Resolved {
                        fqdn: "crate::T".into(),
                    },
                    sites: vec![],
                },
            )
            .unwrap();
        }
        let ctx = context_for_symbol_with_neighbors(&handle, "crate::foo", 2)
            .unwrap()
            .unwrap();
        assert!(ctx.callees.is_empty());
        assert!(ctx.imports.is_empty());
        assert!(ctx.callers.is_empty());
        assert!(ctx.imported_by.is_empty());
    }
}

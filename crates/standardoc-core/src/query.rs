//! Read-only queries over the workspace index.
//!
//! Each function blocks while it borrows a connection from the writer pool
//! and returns owned IR types so callers can drop the result without holding
//! read locks. `RawSymbol::attributes` is reconstructed empty: attributes are
//! a provider artefact that is not persisted in v1 (cf. SCHEMA §2.4 — no
//! attributes column on `symbols`). Bridge-encoded edge targets fall back to
//! `Unresolved { name }` because the stored `to_unresolved` text discards the
//! `BridgeKind` distinction (cf. storage::conv::unresolved_to_storage).

pub mod call_sites;
pub mod graph;
pub mod projects;
mod similarity;
pub mod workspace;

use rusqlite::{Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use standardoc_ir::{
    Blake3Hash, EdgeKind, Kind, Language, LanguageKind, RawEdge, RawSymbol, ResolvedOrUnresolved,
    Site, SymbolLocation, TypeRef, Visibility,
};

use crate::storage::conv::{
    decl_kind_from_sql_text, edge_confidence_from_sql_text, edge_kind_from_sql_text,
    json_to_signature, kind_from_sql_text, kind_to_sql_text, language_from_sql_text,
    visibility_from_sql_text, visibility_to_sql_text,
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
/// [`SymbolContext`] (signature + descriptions) with six pre-grouped neighbor
/// vectors selected for LLM consumption (cf. VISION.md L251-262 "chunk parfait
/// pour LLM"):
///
/// - `callers`     — `edges_to(fqdn)`   filtered to [`EdgeKind::Calls`]
/// - `callees`     — `edges_from(fqdn)` filtered to [`EdgeKind::Calls`]
/// - `imports`     — `edges_from(fqdn)` filtered to [`EdgeKind::Imports`]
/// - `imported_by` — `edges_to(fqdn)`   filtered to [`EdgeKind::Imports`]
/// - `dependents`  — `edges_to(fqdn)`   for all OTHER kinds (Extends,
///   Implements, References, UsesType).
///   "Anything that breaks if this symbol changes shape".
/// - `tests`       — subset of `callers ∪ dependents` whose source FQDN
///   matches a test naming convention (contains `::tests::`, `::test::`,
///   or ends in `_test` / `_tests`). Coarse heuristic; treats false
///   positives as acceptable noise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolContextWithNeighbors {
    pub context: SymbolContext,
    pub callers: Vec<NeighborSymbol>,
    pub callees: Vec<NeighborSymbol>,
    pub imports: Vec<NeighborSymbol>,
    pub imported_by: Vec<NeighborSymbol>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependents: Vec<NeighborSymbol>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<NeighborSymbol>,
}

const SYMBOL_COLUMNS: &str = "fqdn, name, kind, language_kind, module, visibility, \
     file_path, start_line, end_line, start_col, end_col, \
     signature_json, body_hash, flags, decl_kind, implements_trait, receiver_type";

fn with_conn<F, R>(handle: &IndexHandle, f: F) -> Result<R, StorageError>
where
    F: FnOnce(&Connection) -> Result<R, StorageError>,
{
    let pool = handle.pool()?;
    let conn = pool.get()?;
    f(&conn)
}

pub fn symbol_by_fqdn(handle: &IndexHandle, fqdn: &str) -> Result<Option<RawSymbol>, StorageError> {
    // Implicit primary-workspace scoping. Peer-workspace rows with the
    // same fqdn (`UNIQUE(workspace_id, fqdn)`) require the explicit
    // `symbol_by_fqdn_in_workspace` API.
    symbol_by_fqdn_in_workspace(handle, fqdn, crate::storage::module_lookup::PRIMARY_WORKSPACE_ID)
}

/// Stage 3b-7-b Layer 2: scope-aware variant of [`symbol_by_fqdn`].
///
/// Matches `(workspace_id, fqdn)` exactly rather than `fqdn` alone — the
/// foundation for cross-workspace lookups once Layer 3 lands peer rows in
/// the same `symbols` table. Today every row carries
/// `workspace_id = 'primary'`, so passing `PRIMARY_WORKSPACE_ID` returns
/// the same answer as `symbol_by_fqdn`; the call shape is here so
/// pipeline code that already knows its target workspace can express it
/// without waiting for the broader sibling sweep (find_symbol /
/// find_symbols_by_pattern / list_symbols) which is bundled with Layer 3.
pub fn symbol_by_fqdn_in_workspace(
    handle: &IndexHandle,
    fqdn: &str,
    workspace_id: &str,
) -> Result<Option<RawSymbol>, StorageError> {
    with_conn(handle, |conn| {
        let raw = conn
            .query_row(
                &format!(
                    "SELECT {SYMBOL_COLUMNS} FROM symbols \
                     WHERE workspace_id = ?1 AND fqdn = ?2"
                ),
                rusqlite::params![workspace_id, fqdn],
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
            "SELECT id, kind, to_symbol_id, to_unresolved, attributes, confidence \
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
        // Single round-trip: JOIN symbols on from_symbol_id to fetch
        // the from_fqdn alongside the edge row. Eliminates the prior
        // N+1 of one `lookup_fqdn_by_id` per row.
        let mut stmt = conn.prepare(
            "SELECT e.id, e.kind, e.to_symbol_id, e.to_unresolved, \
                    e.attributes, e.confidence, f.fqdn \
             FROM edges e \
             JOIN symbols f ON f.id = e.from_symbol_id \
             WHERE (?1 IS NOT NULL AND e.to_symbol_id = ?1) \
                OR e.to_unresolved = ?2 \
             ORDER BY e.id ASC",
        )?;
        let rows: Vec<(EdgeRowRaw, String)> = stmt
            .query_map(rusqlite::params![target_id, fqdn], |row| {
                Ok((read_edge_row(row)?, row.get::<_, String>(6)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(row, from_fqdn)| build_edge(conn, row, from_fqdn))
            .collect()
    })
}

/// Symbol-shape filters reused by `search_text`, `list_symbols`,
/// `find_by_pattern` and `find_similar`. Every field is optional;
/// `None` means "no constraint on that column". `module` is matched
/// exactly against `symbols.module` — pattern-style module matching
/// belongs to `find_by_pattern` via wildcards in the pattern argument
/// itself.
///
/// `include_external` toggles whether `symbols.is_external = 1` rows
/// participate in the result. Defaults to `true` so external symbols
/// (Cargo crates, npm `.d.ts`, luarocks) show up alongside workspace
/// symbols by default — set to `false` to scope a query to the
/// workspace-only namespace ("find every public fn I authored").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolFilter {
    pub kind: Option<Kind>,
    pub visibility: Option<Visibility>,
    pub module: Option<String>,
    pub include_external: bool,
    /// L3e: scope the result to a single workspace. `None` resolves to
    /// the primary workspace via [`Self::effective_workspace_id`] —
    /// matches the "give me MY symbols" mental model. Pass
    /// `Some(workspace_id)` to scope a query to a specific linked peer.
    /// There is no "all workspaces" mode — call once per workspace.
    pub workspace_id: Option<String>,
}

impl Default for SymbolFilter {
    fn default() -> Self {
        Self {
            kind: None,
            visibility: None,
            module: None,
            include_external: true,
            workspace_id: None,
        }
    }
}

impl SymbolFilter {
    /// Returns the workspace_id string to use in SQL. `None` → primary.
    pub fn effective_workspace_id(&self) -> &str {
        self.workspace_id
            .as_deref()
            .unwrap_or(crate::storage::module_lookup::PRIMARY_WORKSPACE_ID)
    }
}

pub fn search_text(
    handle: &IndexHandle,
    query: &str,
    limit: usize,
    filter: &SymbolFilter,
) -> Result<Vec<RawSymbol>, StorageError> {
    let sanitized = sanitize_fts5_query(query);
    if sanitized.is_empty() {
        return Ok(Vec::new());
    }
    let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
    let kind_text = filter.kind.map(kind_to_sql_text);
    let vis_text = filter.visibility.map(visibility_to_sql_text);
    let module = filter.module.as_deref();
    let include_external = filter.include_external;
    let workspace_id = filter.effective_workspace_id();
    with_conn(handle, |conn| {
        let mut stmt = conn.prepare(
            "SELECT s.fqdn, s.name, s.kind, s.language_kind, s.module, s.visibility, \
                    s.file_path, s.start_line, s.end_line, s.start_col, s.end_col, \
                    s.signature_json, s.body_hash, s.flags, s.decl_kind, \
                    s.implements_trait, s.receiver_type \
             FROM symbols_fts f \
             JOIN symbols s ON s.id = f.rowid \
             WHERE symbols_fts MATCH ?1 \
               AND (?2 IS NULL OR s.kind       = ?2) \
               AND (?3 IS NULL OR s.visibility = ?3) \
               AND (?4 IS NULL OR s.module     = ?4) \
               AND (?5 = 1 OR s.is_external = 0) \
               AND s.workspace_id = ?6 \
             ORDER BY rank \
             LIMIT ?7",
        )?;
        let rows = stmt
            .query_map(
                rusqlite::params![
                    sanitized,
                    kind_text,
                    vis_text,
                    module,
                    include_external,
                    workspace_id,
                    limit_i64
                ],
                read_symbol_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(build_symbol).collect()
    })
}

/// Strips FTS5 syntactic chars (`-` NOT prefix, `"` phrase quoting,
/// `*` prefix wildcard, `:` column filter, `^` initial-token op, `()`
/// grouping, `+` NEAR/B etween) from the user's query and replaces them
/// with spaces. Multiple resulting tokens become an implicit-AND match.
///
/// Required because `find_symbol("standardoc-cli")` would otherwise be
/// parsed by FTS5 as `standardoc NOT cli` — excluding the very thing
/// the caller asked for. Standardoc's `find_symbol` is a high-level
/// search API, not an FTS5 console ; users expect "match all tokens".
fn sanitize_fts5_query(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_was_space = true;
    for c in raw.chars() {
        let keep = c.is_alphanumeric() || c == '_';
        if keep {
            out.push(c);
            last_was_space = false;
        } else if !last_was_space {
            out.push(' ');
            last_was_space = true;
        }
    }
    out.truncate(out.trim_end().len());
    out
}

/// One page of [`list_symbols`] output. `next_cursor` carries the
/// last fqdn returned when the page filled to `limit` — clients call
/// back with `cursor = Some(next_cursor)` to fetch the next slice.
/// When the page is short (or empty), `next_cursor` is `None` and
/// the walk is done. Cursor format is intentionally transparent (the
/// fqdn itself) since results are always ordered by `s.fqdn`; if we
/// ever change the ordering we'll bump it to an opaque token.
#[derive(Debug, Clone)]
pub struct ListSymbolsPage {
    pub items: Vec<RawSymbol>,
    pub next_cursor: Option<String>,
}

/// Returns symbols matching `filter` ordered by canonical fqdn. No
/// query string, no pattern — pure server-side filter listing useful
/// for audits like "list every private function in module X". Pass
/// `cursor = Some(last_fqdn_from_previous_page)` to walk past the
/// `limit` cap and stream the full result set without server-side
/// state.
pub fn list_symbols(
    handle: &IndexHandle,
    filter: &SymbolFilter,
    limit: usize,
    cursor: Option<&str>,
) -> Result<ListSymbolsPage, StorageError> {
    let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
    let kind_text = filter.kind.map(kind_to_sql_text);
    let vis_text = filter.visibility.map(visibility_to_sql_text);
    let module = filter.module.as_deref();
    let include_external = filter.include_external;
    let workspace_id = filter.effective_workspace_id();
    let cursor_param = cursor;
    let items: Vec<RawSymbol> = with_conn(handle, |conn| {
        let mut stmt = conn.prepare(
            "SELECT s.fqdn, s.name, s.kind, s.language_kind, s.module, s.visibility, \
                    s.file_path, s.start_line, s.end_line, s.start_col, s.end_col, \
                    s.signature_json, s.body_hash, s.flags, s.decl_kind, \
                    s.implements_trait, s.receiver_type \
             FROM symbols s \
             WHERE (?1 IS NULL OR s.kind       = ?1) \
               AND (?2 IS NULL OR s.visibility = ?2) \
               AND (?3 IS NULL OR s.module     = ?3) \
               AND (?4 = 1 OR s.is_external = 0) \
               AND (?5 IS NULL OR s.fqdn       > ?5) \
               AND s.workspace_id = ?6 \
             ORDER BY s.fqdn \
             LIMIT ?7",
        )?;
        let rows = stmt
            .query_map(
                rusqlite::params![
                    kind_text,
                    vis_text,
                    module,
                    include_external,
                    cursor_param,
                    workspace_id,
                    limit_i64
                ],
                read_symbol_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(build_symbol).collect()
    })?;

    // A short page proves we hit the end. A full page MAY have more
    // — we emit the last fqdn as the cursor and let the client decide
    // whether to fetch again. Empty page (cursor went past the last
    // matching fqdn) also terminates the walk.
    let next_cursor = if items.len() == limit && !items.is_empty() {
        items.last().map(|s| s.fqdn.clone())
    } else {
        None
    };
    Ok(ListSymbolsPage { items, next_cursor })
}

/// Glob-pattern search over `symbols.name` and `symbols.fqdn`. Uses
/// SQLite's `GLOB` operator (`*`, `?`, `[abc]` wildcards — case-sensitive,
/// distinct semantics from `LIKE`). A symbol matches when EITHER its
/// name OR its fqdn satisfies the pattern. Results ordered by fqdn for
/// stability.
///
/// Typical usage: detect cross-module duplications (`strip_*_extension`
/// → catches `strip_rs_extension`, `strip_ts_extension`, ...).
pub fn find_by_pattern(
    handle: &IndexHandle,
    pattern: &str,
    filter: &SymbolFilter,
    limit: usize,
) -> Result<Vec<RawSymbol>, StorageError> {
    let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
    let kind_text = filter.kind.map(kind_to_sql_text);
    let vis_text = filter.visibility.map(visibility_to_sql_text);
    let module = filter.module.as_deref();
    let include_external = filter.include_external;
    let workspace_id = filter.effective_workspace_id();
    with_conn(handle, |conn| {
        let mut stmt = conn.prepare(
            "SELECT s.fqdn, s.name, s.kind, s.language_kind, s.module, s.visibility, \
                    s.file_path, s.start_line, s.end_line, s.start_col, s.end_col, \
                    s.signature_json, s.body_hash, s.flags, s.decl_kind, \
                    s.implements_trait, s.receiver_type \
             FROM symbols s \
             WHERE (s.name GLOB ?1 OR s.fqdn GLOB ?1) \
               AND (?2 IS NULL OR s.kind       = ?2) \
               AND (?3 IS NULL OR s.visibility = ?3) \
               AND (?4 IS NULL OR s.module     = ?4) \
               AND (?5 = 1 OR s.is_external = 0) \
               AND s.workspace_id = ?6 \
             ORDER BY s.fqdn \
             LIMIT ?7",
        )?;
        let rows = stmt
            .query_map(
                rusqlite::params![
                    pattern,
                    kind_text,
                    vis_text,
                    module,
                    include_external,
                    workspace_id,
                    limit_i64
                ],
                read_symbol_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(build_symbol).collect()
    })
}

/// Returns symbols whose `name` is similar to `reference`, ranked by score
/// descending. The score is a hybrid `max(jaro_winkler, jaccard_tokens)` over
/// lowercased names — see `query::similarity` for the algorithm.
///
/// `reference` is treated as raw text: no symbol lookup, no canonicalisation.
/// Pass either a known name (`strip_rs_extension`) or a hypothetical one
/// (`strip_extension`) to anchor the search. Symbols whose name matches
/// `reference` case-insensitively are skipped (self-skip semantics — anchor
/// is not its own neighbor; collisions across modules are skipped uniformly).
///
/// `threshold` is hard-clamped to `[0.0, 1.0]` by the caller; rows with
/// `score < threshold` are dropped. `filter` narrows the candidate pool
/// BEFORE scoring (idiomatic with the other query helpers — keeps the
/// scorer's input bounded). `limit` truncates the final ranked list.
///
/// Comparison is on `name` only, not `fqdn`: the module-path string would
/// dominate identifier characters and drown out the templated-name signal
/// (`a::b::strip_rs_extension` vs `c::d::e::strip_ts_extension` look
/// different even though they're the cluster we want to detect).
pub fn find_similar(
    handle: &IndexHandle,
    reference: &str,
    threshold: f32,
    filter: &SymbolFilter,
    limit: usize,
) -> Result<Vec<(RawSymbol, f32)>, StorageError> {
    let reference_lc = reference.to_lowercase();
    let kind_text = filter.kind.map(kind_to_sql_text);
    let vis_text = filter.visibility.map(visibility_to_sql_text);
    let module = filter.module.as_deref();
    let include_external = filter.include_external;
    with_conn(handle, |conn| {
        let mut stmt = conn.prepare(
            "SELECT s.fqdn, s.name, s.kind, s.language_kind, s.module, s.visibility, \
                    s.file_path, s.start_line, s.end_line, s.start_col, s.end_col, \
                    s.signature_json, s.body_hash, s.flags, s.decl_kind, \
                    s.implements_trait, s.receiver_type \
             FROM symbols s \
             WHERE (?1 IS NULL OR s.kind       = ?1) \
               AND (?2 IS NULL OR s.visibility = ?2) \
               AND (?3 IS NULL OR s.module     = ?3) \
               AND (?4 = 1 OR s.is_external = 0)",
        )?;
        let rows = stmt
            .query_map(
                rusqlite::params![kind_text, vis_text, module, include_external],
                read_symbol_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let symbols = rows
            .into_iter()
            .map(build_symbol)
            .collect::<Result<Vec<_>, _>>()?;
        let mut scored: Vec<(RawSymbol, f32)> = symbols
            .into_iter()
            .filter(|s| s.name.to_lowercase() != reference_lc)
            .filter_map(|s| {
                let score = similarity::score(reference, &s.name);
                (score >= threshold).then_some((s, score))
            })
            .collect();
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.fqdn.cmp(&b.0.fqdn))
        });
        scored.truncate(limit);
        Ok(scored)
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
                        row.get::<_, Option<String>>(17)?,
                        row.get::<_, Option<String>>(18)?,
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
    let mut dependents = Vec::new();
    for edge in edges_to(handle, fqdn)? {
        match edge.kind {
            EdgeKind::Calls => callers.push(neighbor_inbound(handle, edge, depth)?),
            EdgeKind::Imports => imported_by.push(neighbor_inbound(handle, edge, depth)?),
            EdgeKind::Extends
            | EdgeKind::Implements
            | EdgeKind::References
            | EdgeKind::UsesType => dependents.push(neighbor_inbound(handle, edge, depth)?),
        }
    }

    let tests: Vec<NeighborSymbol> = callers
        .iter()
        .chain(dependents.iter())
        .filter(|n| neighbor_looks_like_test(n))
        .cloned()
        .collect();

    Ok(Some(SymbolContextWithNeighbors {
        context,
        callers,
        callees,
        imports,
        imported_by,
        dependents,
        tests,
    }))
}

/// Coarse, fqdn-based detection of test sites for the blast-radius view.
/// Captures Rust's `mod tests { ... }` / `#[cfg(test)]` convention, TS's
/// `*.test.ts` / `*.spec.ts` patterns when surfaced as a `::test` /
/// `::tests` module segment, and trailing `_test` / `_tests` names.
/// False positives are accepted — the field is a hint, not a contract.
fn neighbor_looks_like_test(n: &NeighborSymbol) -> bool {
    let from = match &n.target {
        ResolvedOrUnresolved::Resolved { fqdn } => fqdn.as_str(),
        ResolvedOrUnresolved::Unresolved { name }
        | ResolvedOrUnresolved::UnresolvedBridge { name, .. } => name.as_str(),
    };
    // `.test` and `.spec` are part of an FQDN convention (the symbol's
    // module fqdn includes the trailing `.test`/`.spec` segment carried
    // from the source filename), not a file extension. The
    // `case_sensitive_file_extension_comparisons` lint targets path
    // comparisons; here the input is workspace-canonical text.
    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    let ext_match = from.ends_with(".test") || from.ends_with(".spec");
    from.contains("::tests::")
        || from.contains("::test::")
        || from.ends_with("_test")
        || from.ends_with("_tests")
        || ext_match
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

/// Aggregated result of [`body_for_fqdn`]: the raw source slice covering a
/// symbol's declared `start_line..=end_line` plus enough metadata for the
/// caller to know what was returned and whether a `max_lines` cap or any of
/// the noise-stripping options kicked in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BodySlice {
    pub fqdn: String,
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub body: String,
    pub truncated: bool,
    pub total_body_lines: u32,
    /// Number of leading "noise" lines (doc comments, attributes, blank
    /// lines between them) dropped by `BodyOptions::strip_attrs`. Zero
    /// when stripping is disabled or the body had no leading noise.
    #[serde(default)]
    pub stripped_lines: u32,
    /// `true` when `BodyOptions::signature_only` truncated the body at
    /// the first line containing `{`. Independent of `truncated` (which
    /// only reflects the `max_lines` cap).
    #[serde(default)]
    pub signature_only: bool,
    /// Number of leading-whitespace bytes shared by every non-blank line
    /// of the returned slice that were stripped to dedent the body. Zero
    /// when the body had no common indent (or only one non-blank line at
    /// column 0). Pure compaction signal — the original column positions
    /// can be recovered by re-reading the file at `start_line`.
    #[serde(default)]
    pub dedented_prefix_len: u32,
    /// What one indent level in the returned `body` looks like. `"\t"`
    /// when leading 4-space (or 2-space) runs were converted to tabs OR
    /// the source already used tabs. Empty when the body has no indented
    /// line, or when the residual indent is too irregular to canonicalize
    /// (mixed tabs+spaces, non-power-of-2 widths) — in that case the
    /// body is returned verbatim post-dedent.
    #[serde(default)]
    pub indent_unit: String,
}

/// Knobs controlling the slice returned by [`body_for_fqdn`]. Defaults give
/// the legacy "verbatim slice" behavior.
#[derive(Debug, Default, Clone, Copy)]
pub struct BodyOptions {
    /// Cap on the number of returned lines. When the slice exceeds this
    /// count, the response is truncated and `BodySlice.truncated = true`.
    pub max_lines: Option<u32>,
    /// Drop leading doc comments (`///`, `//!`, `//`, `/* … */`, `/** … */`)
    /// AND attribute lines (`#[…]`, `#![…]` — including their multi-line
    /// continuations) AND any blank lines interleaved between them.
    /// Stops at the first line that is neither comment, attribute, nor
    /// blank. Massive shrink for handlers buried under verbose
    /// `#[tool(description = "…")]` blocks.
    pub strip_attrs: bool,
    /// Truncate the body just after the first line containing `{` (the
    /// opening brace of the function block). For Rust / TS / JS / C-like
    /// targets this returns the full multi-line signature without the
    /// implementation. Combine with `strip_attrs` to get the cleanest
    /// signature view. No-op for languages without `{` (Python, Lua —
    /// punted to a future per-language handler).
    pub signature_only: bool,
    /// Strip C-style inline comments from the returned body: `// …\n`
    /// line comments and `/* … */` block comments. Operates after
    /// `strip_attrs` / `signature_only` / `max_lines`, so the leading-
    /// noise paragraph drop and the dedent stay independent knobs.
    /// String-literal safe for `"…"` double-quoted strings (with `\`
    /// escapes), Rust raw strings (`r"…"`, `r#"…"#`, …) and TS
    /// template literals (`` `…` ``). Single-quoted strings are
    /// passed through verbatim — this is correct for Rust char
    /// literals + lifetimes but means TS `'string with // inside'`
    /// would still have the `//` consumed (rare; flag it if it bites).
    /// Lines fully consumed by a stripped comment become blank rather
    /// than disappearing, so line-number correspondence is preserved.
    pub strip_inline_comments: bool,
}

/// Returns the raw source text of the symbol at `fqdn`, sliced from the file
/// on disk between its `start_line` and `end_line`. Returns `None` when no
/// symbol matches the FQDN. See [`BodyOptions`] for the ways the slice can
/// be trimmed.
///
/// File I/O is anchored at `IndexHandle::workspace_root()`; the indexed
/// `location.file` is assumed workspace-relative (the IR contract).
pub fn body_for_fqdn(
    handle: &IndexHandle,
    fqdn: &str,
    opts: &BodyOptions,
) -> Result<Option<BodySlice>, StorageError> {
    let Some(symbol) = symbol_by_fqdn(handle, fqdn)? else {
        return Ok(None);
    };
    let workspace_root = handle.workspace_root();
    let file_abs = workspace_root.join(&symbol.location.file);
    let content = std::fs::read_to_string(&file_abs)?;
    let all_lines: Vec<&str> = content.lines().collect();

    let start_zero = symbol.location.start_line.saturating_sub(1) as usize;
    let end_inclusive = (symbol.location.end_line as usize).min(all_lines.len());
    if start_zero >= end_inclusive {
        return Ok(Some(BodySlice {
            fqdn: symbol.fqdn.clone(),
            file: symbol.location.file.clone(),
            start_line: symbol.location.start_line,
            end_line: symbol.location.end_line,
            body: String::new(),
            truncated: false,
            total_body_lines: 0,
            stripped_lines: 0,
            signature_only: false,
            dedented_prefix_len: 0,
            indent_unit: String::new(),
        }));
    }
    let raw_slice: &[&str] = &all_lines[start_zero..end_inclusive];
    let total = u32::try_from(raw_slice.len()).unwrap_or(u32::MAX);

    let stripped_count = if opts.strip_attrs {
        count_leading_noise_lines(raw_slice)
    } else {
        0
    };
    let after_strip: &[&str] = &raw_slice[stripped_count..];

    let (after_signature, signature_truncated) = if opts.signature_only {
        match after_strip.iter().position(|l| l.contains('{')) {
            Some(i) => (&after_strip[..=i], true),
            None => (after_strip, false),
        }
    } else {
        (after_strip, false)
    };

    let (taken, truncated) = match opts.max_lines {
        Some(cap) if (cap as usize) < after_signature.len() => {
            (&after_signature[..cap as usize], true)
        }
        _ => (after_signature, false),
    };
    let compact = compact_body_indent(taken);
    let final_body = if opts.strip_inline_comments {
        strip_inline_comments_in_body(&compact.body)
    } else {
        compact.body
    };
    Ok(Some(BodySlice {
        fqdn: symbol.fqdn.clone(),
        file: symbol.location.file.clone(),
        start_line: symbol.location.start_line,
        end_line: symbol.location.end_line,
        body: final_body,
        truncated,
        total_body_lines: total,
        stripped_lines: u32::try_from(stripped_count).unwrap_or(u32::MAX),
        signature_only: signature_truncated,
        dedented_prefix_len: compact.dedented_prefix_len,
        indent_unit: compact.indent_unit,
    }))
}

/// Strip C-style inline comments from `body` while leaving `"…"` string
/// literals, Rust raw strings (`r"…"`, `r#"…"#`), and TS template
/// literals (`` `…` ``) untouched.
///
/// `//` strips to end of line. `/* … */` strips through the closing
/// `*/`, preserving newlines so line-number alignment is intact when
/// the caller pairs the body with diagnostics. Single-quoted spans
/// (`'…'`) are walked as plain code — correct for Rust lifetimes /
/// char literals; a TS `'string'` with `//` inside is a documented
/// edge case (the comment characters get stripped).
///
/// Walk is byte-level but only ASCII tokens (`/ * " ` r # \ \n`) drive
/// state transitions — every non-token byte is included via
/// `out.push_str(&body[copy_from..i])` slice copies, so multi-byte
/// UTF8 sequences stay intact.
fn strip_inline_comments_in_body(body: &str) -> String {
    enum St {
        Code,
        LineComment,
        BlockComment(u32),
        DqString,
        Template,
        RawString(usize),
    }
    let bytes = body.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut state = St::Code;
    let mut copy_from = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        match state {
            St::Code => {
                // Rust raw string opener: `r"` or `r#"` / `r##"` …
                if b == b'r' {
                    let mut j = i + 1;
                    let mut hashes = 0usize;
                    while j < bytes.len() && bytes[j] == b'#' {
                        hashes += 1;
                        j += 1;
                    }
                    if j < bytes.len() && bytes[j] == b'"' {
                        state = St::RawString(hashes);
                        i = j + 1;
                        continue;
                    }
                }
                if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                    out.push_str(&body[copy_from..i]);
                    state = St::LineComment;
                    i += 2;
                    continue;
                }
                if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                    out.push_str(&body[copy_from..i]);
                    state = St::BlockComment(1);
                    i += 2;
                    continue;
                }
                if b == b'"' {
                    state = St::DqString;
                } else if b == b'`' {
                    state = St::Template;
                }
                i += 1;
            }
            St::LineComment => {
                if b == b'\n' {
                    copy_from = i;
                    state = St::Code;
                }
                i += 1;
            }
            St::BlockComment(depth) => {
                if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                    state = St::BlockComment(depth + 1);
                    i += 2;
                    continue;
                }
                if b == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                    if depth == 1 {
                        state = St::Code;
                        copy_from = i + 2;
                    } else {
                        state = St::BlockComment(depth - 1);
                    }
                    i += 2;
                    continue;
                }
                if b == b'\n' {
                    out.push('\n');
                }
                i += 1;
            }
            St::DqString => {
                if b == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if b == b'"' {
                    state = St::Code;
                }
                i += 1;
            }
            St::Template => {
                if b == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if b == b'`' {
                    state = St::Code;
                }
                i += 1;
            }
            St::RawString(hashes) => {
                if b == b'"' {
                    let end = i + 1 + hashes;
                    if end <= bytes.len()
                        && bytes[i + 1..end].iter().all(|&c| c == b'#')
                    {
                        state = St::Code;
                        i = end;
                        continue;
                    }
                }
                i += 1;
            }
        }
    }
    if copy_from < bytes.len() {
        out.push_str(&body[copy_from..]);
    }
    out
}

/// Counts how many leading lines of `slice` look like noise:
/// `///` / `//!` / `//` doc comments, `/* … */` block comments, `#[…]` /
/// `#![…]` attributes (with multi-line continuations balanced via paren
/// depth), and blank lines interleaved between them. Stops at the first
/// non-noise line. Pure function — easy to test in isolation.
fn count_leading_noise_lines(slice: &[&str]) -> usize {
    let mut i = 0usize;
    let mut paren_depth: i32 = 0;
    let mut in_block_comment = false;
    while i < slice.len() {
        let raw = slice[i];
        let line = raw.trim_start();
        if in_block_comment {
            if line.contains("*/") {
                in_block_comment = false;
            }
            i += 1;
            continue;
        }
        if paren_depth > 0 {
            paren_depth += paren_depth_delta(raw);
            i += 1;
            continue;
        }
        if line.is_empty() {
            i += 1;
            continue;
        }
        if line.starts_with("///") || line.starts_with("//!") || line.starts_with("//") {
            i += 1;
            continue;
        }
        if line.starts_with("/**") || line.starts_with("/*") {
            if !line.contains("*/") {
                in_block_comment = true;
            }
            i += 1;
            continue;
        }
        if line.starts_with("#[") || line.starts_with("#![") {
            paren_depth += paren_depth_delta(raw);
            i += 1;
            continue;
        }
        // `*` continuation of a `/* …` block comment we missed by starting
        // mid-block (shouldn't happen with well-formed slices, but cheap
        // to handle).
        if line.starts_with('*') && !line.starts_with("*/") {
            i += 1;
            continue;
        }
        break;
    }
    i
}

#[inline]
fn paren_depth_delta(line: &str) -> i32 {
    let mut d: i32 = 0;
    for c in line.chars() {
        match c {
            '(' | '[' => d += 1,
            ')' | ']' => d -= 1,
            _ => {}
        }
    }
    d
}

/// Output of [`compact_body_indent`]: a body string ready to serialize
/// plus enough metadata for the caller (and downstream tools) to know
/// what was done.
struct CompactedBody {
    body: String,
    dedented_prefix_len: u32,
    indent_unit: String,
}

/// Compacts the indentation of a body slice for over-the-wire transport.
///
/// Two passes:
///   1. **Dedent common prefix.** Find the longest leading-whitespace
///      sequence shared by every non-blank line and strip it. A method
///      body indented at 8 spaces inside an impl block becomes flush-left
///      — multi-KB savings on long bodies.
///   2. **Tab-convert residual leading runs.** If every remaining
///      non-blank line is indented with a uniform-width space run (every
///      width a multiple of 4, or every width a multiple of 2), each
///      such run is converted to `\t`. Sources that already use tabs
///      pass through unchanged.
///
/// Mixed or irregular indent (tabs + spaces in the same line, or
/// non-power-of-2 widths) skips pass 2 and returns the dedented body
/// verbatim with `indent_unit = ""`. The line *content* is never altered
/// beyond leading whitespace — `taken.join("\n")` semantics are preserved
/// when no compaction is applicable.
fn compact_body_indent(lines: &[&str]) -> CompactedBody {
    if lines.is_empty() {
        return CompactedBody {
            body: String::new(),
            dedented_prefix_len: 0,
            indent_unit: String::new(),
        };
    }

    let common = longest_common_leading_ws(lines);
    let prefix_len = common.len();

    let stripped: Vec<String> = lines
        .iter()
        .map(|l| {
            if l.starts_with(common) {
                l[prefix_len..].to_string()
            } else {
                String::new()
            }
        })
        .collect();

    let unit = detect_indent_unit(&stripped);
    let (final_lines, indent_unit) = match unit {
        Some(width) if width > 0 => {
            let converted: Vec<String> = stripped
                .iter()
                .map(|l| convert_leading_spaces_to_tabs(l, width))
                .collect();
            (converted, "\t".to_string())
        }
        Some(_) => (stripped, String::new()),
        None => (stripped, "\t".to_string()),
    };

    CompactedBody {
        body: final_lines.join("\n"),
        dedented_prefix_len: u32::try_from(prefix_len).unwrap_or(u32::MAX),
        indent_unit,
    }
}

fn leading_ws(s: &str) -> &str {
    let end = s.bytes().take_while(|b| matches!(*b, b' ' | b'\t')).count();
    &s[..end]
}

fn longest_common_leading_ws<'a>(lines: &[&'a str]) -> &'a str {
    let mut iter = lines.iter().filter(|l| !l.trim().is_empty());
    let Some(first) = iter.next() else {
        return "";
    };
    let mut prefix = leading_ws(first);
    for line in iter {
        let lw = leading_ws(line);
        let n = prefix
            .bytes()
            .zip(lw.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        prefix = &prefix[..n];
        if prefix.is_empty() {
            break;
        }
    }
    prefix
}

/// Returns:
/// - `None` when leading whitespace is already entirely tabs — no
///   conversion is necessary; the output uses tabs natively.
/// - `Some(width)` with `width > 0` when every non-blank line's leading
///   whitespace is a multiple of `width` spaces (try 4 first, fall back
///   to 2) — conversion is applicable.
/// - `Some(0)` when the residual is irregular (mixed tabs+spaces on the
///   same line, or non-multiple widths) — leave the body verbatim and
///   report `indent_unit = ""`.
fn detect_indent_unit(lines: &[String]) -> Option<usize> {
    let mut has_tab_only = false;
    let mut space_widths: Vec<usize> = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let lw = leading_ws(line);
        if lw.is_empty() {
            continue;
        }
        let has_tab = lw.bytes().any(|b| b == b'\t');
        let has_space = lw.bytes().any(|b| b == b' ');
        if has_tab && has_space {
            return Some(0);
        }
        if has_tab {
            has_tab_only = true;
        } else {
            space_widths.push(lw.len());
        }
    }
    if has_tab_only && space_widths.is_empty() {
        return None;
    }
    if has_tab_only {
        return Some(0);
    }
    if space_widths.is_empty() {
        return Some(0);
    }
    if space_widths.iter().all(|n| *n % 4 == 0) {
        return Some(4);
    }
    if space_widths.iter().all(|n| *n % 2 == 0) {
        return Some(2);
    }
    Some(0)
}

fn convert_leading_spaces_to_tabs(line: &str, width: usize) -> String {
    let bytes = line.as_bytes();
    let mut tabs = 0;
    let mut i = 0;
    while i + width <= bytes.len() && bytes[i..i + width].iter().all(|b| *b == b' ') {
        tabs += 1;
        i += width;
    }
    if tabs == 0 {
        return line.to_string();
    }
    let mut out = String::with_capacity(tabs + bytes.len() - i);
    for _ in 0..tabs {
        out.push('\t');
    }
    out.push_str(&line[i..]);
    out
}

/// Reads `schema_meta.schema_version` from the connected workspace index.
/// Useful for the CLI pre-flight schema check: a client can compare the on-disk
/// version against `SUPPORTED_SCHEMA_VERSION` before spawning daemons.
pub fn schema_version(handle: &IndexHandle) -> Result<u32, StorageError> {
    with_conn(handle, |conn| {
        let raw: String = conn.query_row(
            "SELECT value FROM schema_meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )?;
        raw.parse::<u32>()
            .map_err(|_| StorageError::InvalidStoredData {
                detail: format!("schema_meta.schema_version is not a u32: {raw}"),
            })
    })
}

/// Bulk-lookup the `last_modified_revision` for each FQDN. Missing FQDNs are
/// simply absent from the returned map — the MCP staleness check treats that
/// as "symbol no longer indexed", which the agent surfaces back to the user.
/// Empty input returns an empty map without touching the database.
pub fn last_modified_revisions_for_fqdns(
    handle: &IndexHandle,
    fqdns: &[&str],
) -> Result<std::collections::HashMap<String, u64>, StorageError> {
    if fqdns.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    with_conn(handle, |conn| {
        // Scoped to PRIMARY_WORKSPACE_ID so peer-workspace rows that
        // happen to share an fqdn (`UNIQUE(workspace_id, fqdn)`) can't
        // bleed into the staleness check.
        let placeholders = (1..=fqdns.len())
            .map(|i| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT fqdn, last_modified_revision FROM symbols \
             WHERE workspace_id = ?1 AND fqdn IN ({placeholders})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let primary = crate::storage::module_lookup::PRIMARY_WORKSPACE_ID;
        let mut all_params: Vec<&str> = Vec::with_capacity(fqdns.len() + 1);
        all_params.push(primary);
        all_params.extend(fqdns.iter().copied());
        let rows = stmt.query_map(rusqlite::params_from_iter(all_params.iter()), |row| {
            let fqdn: String = row.get(0)?;
            let rev: i64 = row.get(1)?;
            #[allow(clippy::cast_sign_loss)]
            Ok((fqdn, rev.max(0) as u64))
        })?;
        let mut map = std::collections::HashMap::with_capacity(fqdns.len());
        for r in rows {
            let (fqdn, rev) = r?;
            map.insert(fqdn, rev);
        }
        Ok(map)
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
    flags_json: String,
    decl_kind_text: Option<String>,
    implements_trait: Option<String>,
    receiver_type_text: Option<String>,
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
        flags_json: row.get(13)?,
        decl_kind_text: row.get(14)?,
        implements_trait: row.get(15)?,
        receiver_type_text: row.get(16)?,
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
    let decl_kind = raw
        .decl_kind_text
        .as_deref()
        .map(decl_kind_from_sql_text)
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
fn parse_flags_json(raw: &str) -> Vec<String> {
    serde_json::from_str(raw).unwrap_or_default()
}

fn position_to_u32(field: &str, value: i64) -> Result<u32, StorageError> {
    u32::try_from(value).map_err(|_| StorageError::InvalidStoredData {
        detail: format!("symbols.{field} out of u32 range: {value}"),
    })
}

struct EdgeRowRaw {
    id: i64,
    kind_text: String,
    to_symbol_id: Option<i64>,
    to_unresolved: Option<String>,
    attributes_json: String,
    confidence_text: String,
}

fn read_edge_row(row: &Row<'_>) -> rusqlite::Result<EdgeRowRaw> {
    Ok(EdgeRowRaw {
        id: row.get(0)?,
        kind_text: row.get(1)?,
        to_symbol_id: row.get(2)?,
        to_unresolved: row.get(3)?,
        attributes_json: row.get(4)?,
        confidence_text: row.get(5)?,
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

/// Resolve an fqdn → row id within the primary workspace. The
/// `UNIQUE(workspace_id, fqdn)` constraint permits the same fqdn in
/// multiple workspaces; the public `edges_from` / `edges_to` queries
/// answer about MY workspace, so the lookup is scoped accordingly.
fn lookup_id_by_fqdn(conn: &Connection, fqdn: &str) -> Result<Option<i64>, StorageError> {
    let id = conn
        .query_row(
            "SELECT id FROM symbols WHERE workspace_id = ?1 AND fqdn = ?2",
            rusqlite::params![crate::storage::module_lookup::PRIMARY_WORKSPACE_ID, fqdn],
            |r| r.get::<_, i64>(0),
        )
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
mod body_helper_tests {
    use super::count_leading_noise_lines;

    #[test]
    fn count_leading_noise_lines_returns_zero_when_first_line_is_code() {
        let lines = vec!["pub fn foo() {", "    do_thing();", "}"];
        assert_eq!(count_leading_noise_lines(&lines), 0);
    }

    #[test]
    fn count_leading_noise_lines_strips_doc_comments() {
        let lines = vec![
            "/// This is the doc.",
            "/// Second doc line.",
            "pub fn foo() {",
        ];
        assert_eq!(count_leading_noise_lines(&lines), 2);
    }

    #[test]
    fn count_leading_noise_lines_strips_simple_attribute() {
        let lines = vec!["#[inline]", "pub fn foo() {"];
        assert_eq!(count_leading_noise_lines(&lines), 1);
    }

    #[test]
    fn count_leading_noise_lines_strips_multi_line_attribute_via_paren_depth() {
        let lines = vec![
            "#[tool(",
            "    description = \"long\"",
            ")]",
            "async fn handler() {",
        ];
        assert_eq!(count_leading_noise_lines(&lines), 3);
    }

    #[test]
    fn count_leading_noise_lines_strips_doc_then_attr_then_blank() {
        let lines = vec!["/// A function.", "#[allow(dead_code)]", "", "fn f() {"];
        assert_eq!(count_leading_noise_lines(&lines), 3);
    }

    #[test]
    fn count_leading_noise_lines_strips_block_comment_spanning_lines() {
        let lines = vec!["/*", " * Multi-line block.", " */", "fn f() {"];
        assert_eq!(count_leading_noise_lines(&lines), 3);
    }

    #[test]
    fn count_leading_noise_lines_handles_indented_attributes() {
        let lines = vec!["    /// indented doc", "    #[inline]", "    fn inner() {"];
        assert_eq!(count_leading_noise_lines(&lines), 2);
    }

    #[test]
    fn count_leading_noise_lines_stops_at_first_non_noise_line() {
        let lines = vec![
            "/// doc",
            "fn first() {",
            "/// doc on inner",
            "fn nested() {",
        ];
        // Only the first /// is leading noise; everything after the `fn first()`
        // is body and must be preserved.
        assert_eq!(count_leading_noise_lines(&lines), 1);
    }

    use super::compact_body_indent;

    #[test]
    fn compact_body_indent_dedents_common_4_space_prefix_and_converts_to_tabs() {
        // A method body indented 4 spaces inside an impl block.
        let lines = vec!["    fn foo(&self) -> u32 {", "        self.x + 1", "    }"];
        let out = compact_body_indent(&lines);
        assert_eq!(out.dedented_prefix_len, 4);
        assert_eq!(out.indent_unit, "\t");
        assert_eq!(out.body, "fn foo(&self) -> u32 {\n\tself.x + 1\n}");
    }

    #[test]
    fn compact_body_indent_dedents_8_space_then_tab_compacts_residual() {
        let lines = vec![
            "        fn deep() {",
            "            inner();",
            "                more();",
            "        }",
        ];
        let out = compact_body_indent(&lines);
        assert_eq!(out.dedented_prefix_len, 8);
        assert_eq!(out.indent_unit, "\t");
        assert_eq!(out.body, "fn deep() {\n\tinner();\n\t\tmore();\n}");
    }

    #[test]
    fn compact_body_indent_preserves_tab_source_verbatim() {
        let lines = vec!["fn foo() {", "\tinner();", "\t\tnested();", "}"];
        let out = compact_body_indent(&lines);
        assert_eq!(out.dedented_prefix_len, 0);
        assert_eq!(out.indent_unit, "\t");
        assert_eq!(out.body, "fn foo() {\n\tinner();\n\t\tnested();\n}");
    }

    #[test]
    fn compact_body_indent_converts_2_space_indent_to_tabs() {
        // TypeScript-style 2-space indent.
        let lines = vec![
            "export function foo() {",
            "  if (x) {",
            "    bar();",
            "  }",
            "}",
        ];
        let out = compact_body_indent(&lines);
        assert_eq!(out.dedented_prefix_len, 0);
        assert_eq!(out.indent_unit, "\t");
        assert_eq!(
            out.body,
            "export function foo() {\n\tif (x) {\n\t\tbar();\n\t}\n}"
        );
    }

    #[test]
    fn compact_body_indent_blank_lines_do_not_break_dedent() {
        let lines = vec!["    fn foo() {", "", "        let x = 1;", "    }"];
        let out = compact_body_indent(&lines);
        assert_eq!(out.dedented_prefix_len, 4);
        assert_eq!(out.indent_unit, "\t");
        // Blank line stays empty (its leading-ws was shorter than common prefix).
        assert_eq!(out.body, "fn foo() {\n\n\tlet x = 1;\n}");
    }

    #[test]
    fn compact_body_indent_skips_conversion_on_mixed_indent() {
        // One line uses tabs, another uses 3 spaces — non-uniform residual.
        let lines = vec!["fn foo() {", "\tinner();", "   weird();", "}"];
        let out = compact_body_indent(&lines);
        assert_eq!(out.dedented_prefix_len, 0);
        // Tabs present but spaces are non-multiple-of-2 / 4 → unit empty,
        // body returned verbatim post-(no-op) dedent.
        assert_eq!(out.indent_unit, "");
        assert_eq!(out.body, "fn foo() {\n\tinner();\n   weird();\n}");
    }

    #[test]
    fn compact_body_indent_empty_input_returns_empty() {
        let lines: Vec<&str> = vec![];
        let out = compact_body_indent(&lines);
        assert_eq!(out.dedented_prefix_len, 0);
        assert_eq!(out.indent_unit, "");
        assert_eq!(out.body, "");
    }

    #[test]
    fn compact_body_indent_single_line_no_indent_no_op() {
        let lines = vec!["fn foo() {}"];
        let out = compact_body_indent(&lines);
        assert_eq!(out.dedented_prefix_len, 0);
        assert_eq!(out.indent_unit, "");
        assert_eq!(out.body, "fn foo() {}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::IngestCommand;

    #[test]
    fn strip_inline_comments_rust_line() {
        let input = "let x = 1; // trailing comment\n";
        assert_eq!(strip_inline_comments_in_body(input), "let x = 1; \n");
    }

    #[test]
    fn strip_inline_comments_rust_block_inline() {
        let input = "fn foo(/* x */) {}";
        assert_eq!(strip_inline_comments_in_body(input), "fn foo() {}");
    }

    #[test]
    fn strip_inline_comments_block_preserves_newlines() {
        let input = "fn foo() {\n    /* this is\n       a block */\n    x\n}";
        // Two newlines from the block contents survive so line numbers stay
        // aligned with the unstripped source.
        assert_eq!(
            strip_inline_comments_in_body(input),
            "fn foo() {\n    \n\n    x\n}"
        );
    }

    #[test]
    fn strip_inline_comments_skips_inside_double_quoted_string() {
        let input = "let s = \"// not a comment\"; // real one\n";
        assert_eq!(
            strip_inline_comments_in_body(input),
            "let s = \"// not a comment\"; \n"
        );
    }

    #[test]
    fn strip_inline_comments_skips_inside_raw_string() {
        let input = "let s = r#\"// not a comment\"#; // real one\n";
        assert_eq!(
            strip_inline_comments_in_body(input),
            "let s = r#\"// not a comment\"#; \n"
        );
    }

    #[test]
    fn strip_inline_comments_skips_inside_ts_template_literal() {
        let input = "const url = `https://example.com`; // tail\n";
        assert_eq!(
            strip_inline_comments_in_body(input),
            "const url = `https://example.com`; \n"
        );
    }

    #[test]
    fn strip_inline_comments_handles_consecutive_line_comments() {
        let input = "// one\n// two\nlet x = 1;\n";
        assert_eq!(strip_inline_comments_in_body(input), "\n\nlet x = 1;\n");
    }

    use crate::storage::edge_sites::insert_edge_sites;
    use crate::storage::edges::insert_edge;
    use crate::storage::files::{FileInput, upsert_file};
    use crate::storage::module_lookup::PRIMARY_WORKSPACE_ID;
    use crate::storage::symbols::{SymbolInsertContext, insert_symbol};
    use rusqlite::Connection;
    use standardoc_ir::{
        Blake3Hash, EdgeConfidence, EdgeKind, ExtractedFile, Kind, LanguageKind, Modifiers, Param,
        RawEdge, RawSymbol, Signature, Site, SourceOrigin, SymbolLocation, TypeRef, Visibility,
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
                is_external: false,
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
            decl_kind: None,
            implements_trait: None,
            receiver_type: None,
            name: name.into(),
            fqdn: fqdn.into(),
            kind: Kind::Callable,
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
            flags: vec![],
        };
        let id = insert_symbol(
            conn,
            &sym,
            SymbolInsertContext {
                file_path: file,
                language: Language::Rust,
                is_external: false,
                source_origin: SourceOrigin::Workspace,
                revision: 0,
                workspace_id: PRIMARY_WORKSPACE_ID,
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
        assert_eq!(got.kind, Kind::Callable);
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
                decl_kind: None,
                implements_trait: None,
                receiver_type: None,
                name: "f".into(),
                fqdn: "crate::f".into(),
                kind: Kind::Callable,
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
                flags: vec![],
            };
            insert_symbol(
                &conn,
                &sym,
                SymbolInsertContext {
                    file_path: "src/main.rs",
                    language: Language::Rust,
                    is_external: false,
                    source_origin: SourceOrigin::Workspace,
                    revision: 0,
                    workspace_id: PRIMARY_WORKSPACE_ID,
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

    // --- Stage 3b-7-b Layer 2: scope-aware lookup ---

    /// Helper: insert a symbol with an explicit workspace_id tag.
    /// Layer-2 tests need this because `seed_symbol` always stamps
    /// `PRIMARY_WORKSPACE_ID`; isolation tests must stamp peer UUIDs.
    fn seed_symbol_in_workspace(
        conn: &Connection,
        file: &str,
        name: &str,
        fqdn: &str,
        line: u32,
        workspace_id: &str,
    ) -> i64 {
        let sym = RawSymbol {
            decl_kind: None,
            implements_trait: None,
            receiver_type: None,
            name: name.into(),
            fqdn: fqdn.into(),
            kind: Kind::Callable,
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
            flags: vec![],
        };
        insert_symbol(
            conn,
            &sym,
            SymbolInsertContext {
                file_path: file,
                language: Language::Rust,
                is_external: false,
                source_origin: SourceOrigin::Workspace,
                revision: 0,
                workspace_id,
            },
        )
        .unwrap()
    }

    #[test]
    fn symbol_by_fqdn_in_workspace_returns_match_for_primary_scope() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol(&conn, "src/main.rs", "foo", "crate::foo", 10);
        }
        let got = symbol_by_fqdn_in_workspace(&handle, "crate::foo", PRIMARY_WORKSPACE_ID)
            .unwrap()
            .unwrap();
        assert_eq!(got.fqdn, "crate::foo");
    }

    #[test]
    fn symbol_by_fqdn_in_workspace_returns_none_for_mismatched_scope() {
        // Primary row exists; lookup under a different workspace_id
        // must NOT see it — that's the whole point of scope-aware queries.
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol(&conn, "src/main.rs", "foo", "crate::foo", 10);
        }
        assert_eq!(
            symbol_by_fqdn_in_workspace(&handle, "crate::foo", "peer-uuid-abc").unwrap(),
            None
        );
    }

    #[test]
    fn symbol_by_fqdn_in_workspace_isolates_peer_from_primary() {
        // Layer-2 isolation smoke: a primary row and a peer row with
        // distinct fqdns must each be visible only under their own
        // workspace scope. (Same-fqdn collision needs Layer 3's
        // UNIQUE(workspace_id, fqdn) — not in scope here.)
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol(&conn, "src/main.rs", "alpha", "primary::alpha", 1);
            seed_symbol_in_workspace(
                &conn,
                "src/main.rs",
                "beta",
                "peer::beta",
                2,
                "peer-uuid-xyz",
            );
        }
        // Primary scope sees alpha, not beta.
        assert!(
            symbol_by_fqdn_in_workspace(&handle, "primary::alpha", PRIMARY_WORKSPACE_ID)
                .unwrap()
                .is_some()
        );
        assert!(
            symbol_by_fqdn_in_workspace(&handle, "peer::beta", PRIMARY_WORKSPACE_ID)
                .unwrap()
                .is_none()
        );
        // Peer scope sees beta, not alpha.
        assert!(
            symbol_by_fqdn_in_workspace(&handle, "peer::beta", "peer-uuid-xyz")
                .unwrap()
                .is_some()
        );
        assert!(
            symbol_by_fqdn_in_workspace(&handle, "primary::alpha", "peer-uuid-xyz")
                .unwrap()
                .is_none()
        );
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
                    attributes: vec![],
                    confidence: EdgeConfidence::default(),
                },
                "primary",
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
                    attributes: vec![],
                    confidence: EdgeConfidence::default(),
                },
                "primary",
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
                    attributes: vec![],
                    confidence: EdgeConfidence::default(),
                },
                "primary",
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
                    attributes: vec![],
                    confidence: EdgeConfidence::default(),
                },
                "primary",
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
                    attributes: vec![],
                    confidence: EdgeConfidence::default(),
                },
                "primary",
            )
            .unwrap();
        }
        let edges = edges_to(&handle, "external::thing").unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from_fqdn, "crate::caller");
    }

    #[test]
    fn sanitize_fts5_query_strips_hyphen_and_joins_with_space() {
        assert_eq!(sanitize_fts5_query("standardoc-cli"), "standardoc cli");
    }

    #[test]
    fn sanitize_fts5_query_replaces_double_colon_with_space() {
        assert_eq!(sanitize_fts5_query("Type::method"), "Type method");
    }

    #[test]
    fn sanitize_fts5_query_collapses_consecutive_specials() {
        assert_eq!(sanitize_fts5_query("foo---bar::baz"), "foo bar baz");
    }

    #[test]
    fn sanitize_fts5_query_preserves_alphanumeric_and_underscore_unchanged() {
        assert_eq!(sanitize_fts5_query("my_func2"), "my_func2");
    }

    #[test]
    fn sanitize_fts5_query_empty_for_only_special_chars() {
        assert_eq!(sanitize_fts5_query("---"), "");
        assert_eq!(sanitize_fts5_query(""), "");
        assert_eq!(sanitize_fts5_query("   "), "");
    }

    #[test]
    fn search_text_matches_hyphenated_query() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol(
                &conn,
                "src/main.rs",
                "cli_entry",
                "standardoc_cli::cli_entry",
                1,
            );
        }
        let results = search_text(&handle, "standardoc-cli", 10, &SymbolFilter::default()).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].fqdn, "standardoc_cli::cli_entry");
    }

    #[test]
    fn search_text_returns_empty_for_only_special_chars_query() {
        let (_dir, handle) = open_handle();
        let results = search_text(&handle, "---", 10, &SymbolFilter::default()).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn search_text_returns_match_via_fts() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol(
                &conn,
                "src/main.rs",
                "create_user",
                "crate::user::create_user",
                1,
            );
            seed_symbol(
                &conn,
                "src/main.rs",
                "delete_user",
                "crate::user::delete_user",
                5,
            );
            seed_symbol(&conn, "src/main.rs", "noise", "crate::noise", 10);
        }
        let got = search_text(&handle, "create_user", 50, &SymbolFilter::default()).unwrap();
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
        let got =
            search_text(&handle, "tick_one OR tick_two", 1, &SymbolFilter::default()).unwrap();
        assert_eq!(got.len(), 1);
    }

    fn seed_symbol_full(
        conn: &Connection,
        file: &str,
        name: &str,
        fqdn: &str,
        kind: Kind,
        visibility: Visibility,
        module: Option<&str>,
    ) -> i64 {
        let sym = RawSymbol {
            decl_kind: None,
            implements_trait: None,
            receiver_type: None,
            name: name.into(),
            fqdn: fqdn.into(),
            kind,
            language_kind: LanguageKind::from("fn_item"),
            module: module.map(str::to_string),
            visibility,
            location: SymbolLocation {
                file: file.into(),
                start_line: 1,
                end_line: 5,
                start_col: 0,
                end_col: 1,
            },
            signature: None,
            body_hash: Some(Blake3Hash::new([0xab; 32])),
            attributes: vec![],
            flags: vec![],
        };
        insert_symbol(
            conn,
            &sym,
            SymbolInsertContext {
                file_path: file,
                language: Language::Rust,
                is_external: false,
                source_origin: SourceOrigin::Workspace,
                revision: 0,
                workspace_id: PRIMARY_WORKSPACE_ID,
            },
        )
        .unwrap()
    }

    #[test]
    fn search_text_filter_by_kind_excludes_other_kinds() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "marker",
                "crate::marker_fn",
                Kind::Callable,
                Visibility::Public,
                None,
            );
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "marker",
                "crate::marker_ty",
                Kind::Type,
                Visibility::Public,
                None,
            );
        }
        let only_types = SymbolFilter {
            kind: Some(Kind::Type),
            ..Default::default()
        };
        let got = search_text(&handle, "marker", 50, &only_types).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].fqdn, "crate::marker_ty");
    }

    #[test]
    fn search_text_filter_by_visibility_excludes_other_vis() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "thing",
                "crate::thing_pub",
                Kind::Callable,
                Visibility::Public,
                None,
            );
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "thing",
                "crate::thing_priv",
                Kind::Callable,
                Visibility::Private,
                None,
            );
        }
        let only_private = SymbolFilter {
            visibility: Some(Visibility::Private),
            ..Default::default()
        };
        let got = search_text(&handle, "thing", 50, &only_private).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].fqdn, "crate::thing_priv");
    }

    #[test]
    fn list_symbols_returns_all_when_filter_empty() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "a",
                "crate::a",
                Kind::Callable,
                Visibility::Public,
                None,
            );
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "b",
                "crate::b",
                Kind::Type,
                Visibility::Private,
                None,
            );
        }
        let got = list_symbols(&handle, &SymbolFilter::default(), 50, None).unwrap();
        assert_eq!(got.items.len(), 2);
        // Ordered by fqdn for stability.
        assert_eq!(got.items[0].fqdn, "crate::a");
        assert_eq!(got.items[1].fqdn, "crate::b");
        // Page wasn't full → no more pages.
        assert!(got.next_cursor.is_none());
    }

    #[test]
    fn list_symbols_filter_by_visibility_private() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "pub_one",
                "crate::pub_one",
                Kind::Callable,
                Visibility::Public,
                None,
            );
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "priv_one",
                "crate::priv_one",
                Kind::Callable,
                Visibility::Private,
                None,
            );
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "priv_two",
                "crate::priv_two",
                Kind::Callable,
                Visibility::Private,
                None,
            );
        }
        let filter = SymbolFilter {
            visibility: Some(Visibility::Private),
            ..Default::default()
        };
        let got = list_symbols(&handle, &filter, 50, None).unwrap();
        assert_eq!(got.items.len(), 2);
        assert!(
            got.items
                .iter()
                .all(|s| s.visibility == Visibility::Private)
        );
        assert!(got.next_cursor.is_none());
    }

    #[test]
    fn list_symbols_filter_by_module_exact_match() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "f1",
                "crate::a::f1",
                Kind::Callable,
                Visibility::Public,
                Some("crate::a"),
            );
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "f2",
                "crate::b::f2",
                Kind::Callable,
                Visibility::Public,
                Some("crate::b"),
            );
        }
        let filter = SymbolFilter {
            module: Some("crate::a".into()),
            ..Default::default()
        };
        let got = list_symbols(&handle, &filter, 50, None).unwrap();
        assert_eq!(got.items.len(), 1);
        assert_eq!(got.items[0].fqdn, "crate::a::f1");
        assert!(got.next_cursor.is_none());
    }

    #[test]
    fn list_symbols_respects_limit() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            for i in 0..5 {
                seed_symbol_full(
                    &conn,
                    "src/main.rs",
                    &format!("f{i}"),
                    &format!("crate::f{i}"),
                    Kind::Callable,
                    Visibility::Public,
                    None,
                );
            }
        }
        let got = list_symbols(&handle, &SymbolFilter::default(), 3, None).unwrap();
        assert_eq!(got.items.len(), 3);
        // Full page → cursor points at the last item, signalling more.
        assert_eq!(got.next_cursor.as_deref(), Some("crate::f2"));
    }

    #[test]
    fn list_symbols_cursor_walks_full_set() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            for i in 0..5 {
                seed_symbol_full(
                    &conn,
                    "src/main.rs",
                    &format!("f{i}"),
                    &format!("crate::f{i}"),
                    Kind::Callable,
                    Visibility::Public,
                    None,
                );
            }
        }
        // Walk every page until the cursor is exhausted, collecting
        // each fqdn exactly once.
        let mut cursor: Option<String> = None;
        let mut seen: Vec<String> = Vec::new();
        let mut iterations = 0_usize;
        loop {
            iterations += 1;
            assert!(iterations < 100, "pagination loop did not terminate");
            let page =
                list_symbols(&handle, &SymbolFilter::default(), 2, cursor.as_deref()).unwrap();
            for s in page.items {
                seen.push(s.fqdn);
            }
            match page.next_cursor {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        assert_eq!(
            seen,
            vec![
                "crate::f0",
                "crate::f1",
                "crate::f2",
                "crate::f3",
                "crate::f4"
            ],
        );
    }

    #[test]
    fn list_symbols_cursor_skips_already_seen() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            for i in 0..4 {
                seed_symbol_full(
                    &conn,
                    "src/main.rs",
                    &format!("f{i}"),
                    &format!("crate::f{i}"),
                    Kind::Callable,
                    Visibility::Public,
                    None,
                );
            }
        }
        // Start past the second item — cursor uses strict `>` so the
        // anchor fqdn itself is NOT included in the next page.
        let page = list_symbols(&handle, &SymbolFilter::default(), 10, Some("crate::f1")).unwrap();
        let fqdns: Vec<&str> = page.items.iter().map(|s| s.fqdn.as_str()).collect();
        assert_eq!(fqdns, vec!["crate::f2", "crate::f3"]);
        assert!(page.next_cursor.is_none());
    }

    #[test]
    fn find_by_pattern_glob_matches_name_wildcard() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "strip_rs_extension",
                "crate::a::strip_rs_extension",
                Kind::Callable,
                Visibility::Private,
                None,
            );
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "strip_ts_extension",
                "crate::b::strip_ts_extension",
                Kind::Callable,
                Visibility::Private,
                None,
            );
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "compute_path",
                "crate::c::compute_path",
                Kind::Callable,
                Visibility::Public,
                None,
            );
        }
        let got =
            find_by_pattern(&handle, "strip_*_extension", &SymbolFilter::default(), 50).unwrap();
        let names: Vec<&str> = got.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["strip_rs_extension", "strip_ts_extension"]);
    }

    #[test]
    fn find_by_pattern_glob_matches_fqdn_path() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "do_a",
                "myapp::utils::do_a",
                Kind::Callable,
                Visibility::Public,
                None,
            );
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "do_b",
                "myapp::utils::do_b",
                Kind::Callable,
                Visibility::Public,
                None,
            );
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "do_c",
                "other::do_c",
                Kind::Callable,
                Visibility::Public,
                None,
            );
        }
        let got =
            find_by_pattern(&handle, "myapp::utils::*", &SymbolFilter::default(), 50).unwrap();
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|s| s.fqdn.starts_with("myapp::utils::")));
    }

    #[test]
    fn find_by_pattern_combines_pattern_and_visibility_filter() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "helper_one",
                "crate::helper_one",
                Kind::Callable,
                Visibility::Private,
                None,
            );
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "helper_two",
                "crate::helper_two",
                Kind::Callable,
                Visibility::Public,
                None,
            );
        }
        let filter = SymbolFilter {
            visibility: Some(Visibility::Private),
            ..Default::default()
        };
        let got = find_by_pattern(&handle, "helper_*", &filter, 50).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "helper_one");
    }

    #[test]
    fn find_by_pattern_no_match_returns_empty() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "foo",
                "crate::foo",
                Kind::Callable,
                Visibility::Public,
                None,
            );
        }
        let got = find_by_pattern(&handle, "nope_*", &SymbolFilter::default(), 50).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn find_similar_ranks_template_family_above_unrelated() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "strip_rs_extension",
                "crate::a::strip_rs_extension",
                Kind::Callable,
                Visibility::Public,
                None,
            );
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "strip_ts_extension",
                "crate::b::strip_ts_extension",
                Kind::Callable,
                Visibility::Public,
                None,
            );
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "strip_lua_extension",
                "crate::c::strip_lua_extension",
                Kind::Callable,
                Visibility::Public,
                None,
            );
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "render_widget",
                "crate::d::render_widget",
                Kind::Callable,
                Visibility::Public,
                None,
            );
        }
        let got = find_similar(
            &handle,
            "strip_rs_extension",
            0.8,
            &SymbolFilter::default(),
            50,
        )
        .unwrap();
        let names: Vec<&str> = got.iter().map(|(s, _)| s.name.as_str()).collect();
        assert!(
            names.contains(&"strip_ts_extension") && names.contains(&"strip_lua_extension"),
            "expected templated family in result, got {names:?}"
        );
        assert!(
            !names.contains(&"render_widget"),
            "unrelated name must be filtered by threshold, got {names:?}"
        );
    }

    #[test]
    fn find_similar_self_skips_anchor_by_name() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "strip_rs_extension",
                "crate::a::strip_rs_extension",
                Kind::Callable,
                Visibility::Public,
                None,
            );
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "strip_rs_extension",
                "crate::b::strip_rs_extension",
                Kind::Callable,
                Visibility::Public,
                None,
            );
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "strip_ts_extension",
                "crate::c::strip_ts_extension",
                Kind::Callable,
                Visibility::Public,
                None,
            );
        }
        let got = find_similar(
            &handle,
            "strip_rs_extension",
            0.5,
            &SymbolFilter::default(),
            50,
        )
        .unwrap();
        // Both `strip_rs_extension` collisions are skipped (case-insensitive
        // self-skip); only the templated cousin remains.
        let names: Vec<&str> = got.iter().map(|(s, _)| s.name.as_str()).collect();
        assert_eq!(names, vec!["strip_ts_extension"]);
    }

    #[test]
    fn find_similar_orders_by_score_descending_then_fqdn() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            // Closest cousin: 1-char-diff
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "strip_ts_extension",
                "crate::a::strip_ts_extension",
                Kind::Callable,
                Visibility::Public,
                None,
            );
            // Slightly weaker cousin: 3-chars-diff
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "strip_lua_extension",
                "crate::b::strip_lua_extension",
                Kind::Callable,
                Visibility::Public,
                None,
            );
        }
        let got = find_similar(
            &handle,
            "strip_rs_extension",
            0.5,
            &SymbolFilter::default(),
            50,
        )
        .unwrap();
        assert_eq!(got.len(), 2);
        assert!(got[0].1 >= got[1].1, "results must be sorted by score desc");
        assert_eq!(got[0].0.name, "strip_ts_extension");
        assert_eq!(got[1].0.name, "strip_lua_extension");
    }

    #[test]
    fn find_similar_threshold_filters_low_scores() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "buy_apple",
                "crate::a::buy_apple",
                Kind::Callable,
                Visibility::Public,
                None,
            );
        }
        let got =
            find_similar(&handle, "render_widget", 0.95, &SymbolFilter::default(), 50).unwrap();
        assert!(got.is_empty(), "high threshold must drop unrelated names");
    }

    #[test]
    fn find_similar_filter_applied_before_scoring() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "strip_ts_extension",
                "crate::a::strip_ts_extension",
                Kind::Callable,
                Visibility::Public,
                None,
            );
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "strip_lua_extension",
                "crate::b::strip_lua_extension",
                Kind::Callable,
                Visibility::Private,
                None,
            );
        }
        let filter = SymbolFilter {
            visibility: Some(Visibility::Public),
            ..Default::default()
        };
        let got = find_similar(&handle, "strip_rs_extension", 0.5, &filter, 50).unwrap();
        let names: Vec<&str> = got.iter().map(|(s, _)| s.name.as_str()).collect();
        assert_eq!(names, vec!["strip_ts_extension"]);
    }

    #[test]
    fn find_similar_respects_limit_after_sort() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            for label in ["ts", "lua", "py", "go", "js"] {
                let name = format!("strip_{label}_extension");
                let fqdn = format!("crate::{label}::strip_{label}_extension");
                seed_symbol_full(
                    &conn,
                    "src/main.rs",
                    &name,
                    &fqdn,
                    Kind::Callable,
                    Visibility::Public,
                    None,
                );
            }
        }
        let got = find_similar(
            &handle,
            "strip_rs_extension",
            0.0,
            &SymbolFilter::default(),
            2,
        )
        .unwrap();
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn find_similar_empty_index_returns_empty() {
        let (_dir, handle) = open_handle();
        let got = find_similar(&handle, "anything", 0.5, &SymbolFilter::default(), 50).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn find_similar_module_filter_scopes_search() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "strip_ts_extension",
                "crate::a::strip_ts_extension",
                Kind::Callable,
                Visibility::Public,
                Some("crate::a"),
            );
            seed_symbol_full(
                &conn,
                "src/main.rs",
                "strip_lua_extension",
                "crate::b::strip_lua_extension",
                Kind::Callable,
                Visibility::Public,
                Some("crate::b"),
            );
        }
        let filter = SymbolFilter {
            module: Some("crate::a".into()),
            ..Default::default()
        };
        let got = find_similar(&handle, "strip_rs_extension", 0.5, &filter, 50).unwrap();
        let names: Vec<&str> = got.iter().map(|(s, _)| s.name.as_str()).collect();
        assert_eq!(names, vec!["strip_ts_extension"]);
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
                    is_external: false,
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
            module_lookup: None,
            symbols: vec![RawSymbol {
                decl_kind: None,
                implements_trait: None,
                receiver_type: None,
                name: "boot".into(),
                fqdn: "crate::boot".into(),
                kind: Kind::Callable,
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
                flags: vec![],
            }],
            edges: vec![],
            call_sites: vec![],
            documents: vec![],
            ffi_bindings: vec![],
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
            decl_kind: None,
            implements_trait: None,
            receiver_type: None,
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
            flags: vec![],
        };
        insert_symbol(
            conn,
            &sym,
            SymbolInsertContext {
                file_path: &file,
                language: Language::Rust,
                is_external: false,
                source_origin: SourceOrigin::Workspace,
                revision: 0,
                workspace_id: PRIMARY_WORKSPACE_ID,
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
                Kind::Callable,
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
                Kind::Callable,
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
                Kind::Callable,
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
                attributes: vec![],
                confidence: EdgeConfidence::default(),
            },
            "primary",
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
                attributes: vec![],
                confidence: EdgeConfidence::default(),
            },
            "primary",
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
        assert_eq!(
            ctx.callees.len(),
            2,
            "callees include resolved + unresolved"
        );
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
                    attributes: vec![],
                    confidence: EdgeConfidence::default(),
                },
                "primary",
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

    // ────────────────────────────────────────────────────────────────
    // L3e-1: scope-aware query layer (workspace_id filter)
    // ────────────────────────────────────────────────────────────────

    #[test]
    fn search_text_defaults_to_primary_scope_when_workspace_id_is_none() {
        // L3e-1: with `workspace_id=None` the FTS query narrows to
        // primary, peer rows are invisible. Matches "give me MY
        // symbols" default.
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol(&conn, "src/main.rs", "alpha", "primary::alpha", 1);
            seed_symbol_in_workspace(
                &conn,
                "src/main.rs",
                "alpha",
                "peer::alpha",
                10,
                "peer-uuid-l3e1",
            );
        }
        let got = search_text(&handle, "alpha", 50, &SymbolFilter::default()).unwrap();
        let fqdns: Vec<&str> = got.iter().map(|s| s.fqdn.as_str()).collect();
        assert_eq!(
            fqdns,
            vec!["primary::alpha"],
            "default scope must hide peer rows"
        );
    }

    #[test]
    fn search_text_explicit_workspace_id_returns_peer_rows_only() {
        // L3e-1: with `workspace_id=Some(peer)`, primary rows are
        // invisible and only the matching peer row surfaces.
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol(&conn, "src/main.rs", "alpha", "primary::alpha", 1);
            seed_symbol_in_workspace(
                &conn,
                "src/main.rs",
                "alpha",
                "peer::alpha",
                10,
                "peer-uuid-l3e2",
            );
        }
        let filter = SymbolFilter {
            workspace_id: Some("peer-uuid-l3e2".into()),
            ..Default::default()
        };
        let got = search_text(&handle, "alpha", 50, &filter).unwrap();
        let fqdns: Vec<&str> = got.iter().map(|s| s.fqdn.as_str()).collect();
        assert_eq!(fqdns, vec!["peer::alpha"], "peer scope must hide primary");
    }

    #[test]
    fn find_by_pattern_defaults_to_primary_scope() {
        // L3e-1: same default-primary semantics for GLOB pattern queries.
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol(&conn, "src/main.rs", "helper_a", "primary::helper_a", 1);
            seed_symbol_in_workspace(
                &conn,
                "src/main.rs",
                "helper_b",
                "peer::helper_b",
                10,
                "peer-uuid-l3e3",
            );
        }
        let got = find_by_pattern(&handle, "helper_*", &SymbolFilter::default(), 50).unwrap();
        let fqdns: Vec<&str> = got.iter().map(|s| s.fqdn.as_str()).collect();
        assert_eq!(fqdns, vec!["primary::helper_a"]);
    }

    #[test]
    fn find_by_pattern_explicit_workspace_id_returns_peer_match_only() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol(&conn, "src/main.rs", "helper_a", "primary::helper_a", 1);
            seed_symbol_in_workspace(
                &conn,
                "src/main.rs",
                "helper_b",
                "peer::helper_b",
                10,
                "peer-uuid-l3e4",
            );
        }
        let filter = SymbolFilter {
            workspace_id: Some("peer-uuid-l3e4".into()),
            ..Default::default()
        };
        let got = find_by_pattern(&handle, "helper_*", &filter, 50).unwrap();
        let fqdns: Vec<&str> = got.iter().map(|s| s.fqdn.as_str()).collect();
        assert_eq!(fqdns, vec!["peer::helper_b"]);
    }

    #[test]
    fn list_symbols_defaults_to_primary_scope() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol(&conn, "src/main.rs", "alpha", "primary::alpha", 1);
            seed_symbol_in_workspace(
                &conn,
                "src/main.rs",
                "beta",
                "peer::beta",
                10,
                "peer-uuid-l3e5",
            );
        }
        let page = list_symbols(&handle, &SymbolFilter::default(), 50, None).unwrap();
        let fqdns: Vec<&str> = page.items.iter().map(|s| s.fqdn.as_str()).collect();
        assert_eq!(fqdns, vec!["primary::alpha"]);
    }

    #[test]
    fn list_symbols_explicit_workspace_id_returns_peer_rows_only() {
        let (_dir, handle) = open_handle();
        {
            let conn = handle.pool().unwrap().get().unwrap();
            seed_file(&conn, "src/main.rs");
            seed_symbol(&conn, "src/main.rs", "alpha", "primary::alpha", 1);
            seed_symbol_in_workspace(
                &conn,
                "src/main.rs",
                "beta",
                "peer::beta",
                10,
                "peer-uuid-l3e6",
            );
        }
        let filter = SymbolFilter {
            workspace_id: Some("peer-uuid-l3e6".into()),
            ..Default::default()
        };
        let page = list_symbols(&handle, &filter, 50, None).unwrap();
        let fqdns: Vec<&str> = page.items.iter().map(|s| s.fqdn.as_str()).collect();
        assert_eq!(fqdns, vec!["peer::beta"]);
    }

    #[test]
    fn symbol_filter_effective_workspace_id_defaults_to_primary() {
        // Pure unit test for the helper — None resolves to the
        // PRIMARY_WORKSPACE_ID sentinel, Some round-trips through.
        let default = SymbolFilter::default();
        assert_eq!(default.effective_workspace_id(), PRIMARY_WORKSPACE_ID);

        let explicit = SymbolFilter {
            workspace_id: Some("abc".into()),
            ..Default::default()
        };
        assert_eq!(explicit.effective_workspace_id(), "abc");
    }
}

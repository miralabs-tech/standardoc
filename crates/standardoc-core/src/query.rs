//! Read-only queries over the workspace index.
//!
//! Each function blocks while it borrows a connection from the writer pool
//! and returns owned IR types so callers can drop the result without holding
//! read locks. `RawSymbol::attributes` is reconstructed empty: attributes are
//! a provider artefact that is not persisted in v1 (cf. SCHEMA §2.4 — no
//! attributes column on `symbols`). Bridge-encoded edge targets fall back to
//! `Unresolved { name }` because the stored `to_unresolved` text discards the
//! `BridgeKind` distinction (cf. storage::conv::unresolved_to_storage).

mod body;
pub mod call_sites;
pub mod graph;
pub mod projects;
mod similarity;
pub mod workspace;

pub use body::{BodyOptions, BodySlice, body_for_fqdn};

use rusqlite::{Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use standardoc_ir::{
    Blake3Hash, EdgeKind, Kind, Language, LanguageKind, RawEdge, RawSymbol, ResolvedOrUnresolved,
    Site, SymbolLocation, TypeRef, Visibility,
};

use crate::storage::conv::{
    decl_kind_from_sql_text, edge_confidence_from_sql_text, edge_kind_from_sql_text,
    entry_point_from_sql_text, json_to_signature, kind_from_sql_text, kind_to_sql_text,
    language_from_sql_text, visibility_from_sql_text, visibility_to_sql_text,
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
    /// Source language slug stored in `symbols.language` (`rust`,
    /// `typescript`, `lua`, `c`, …). Distinct from
    /// `symbol.language_kind`, which is the LANGUAGE'S native name
    /// for the symbol shape (`struct`, `trait`, `class`, `interface`).
    /// The two names look similar but answer different questions —
    /// this field is the one that disambiguates "is this a Rust
    /// struct or a TS class?" at a glance.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub language: String,
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
/// - `depends_on`  — `edges_from(fqdn)` for all OTHER kinds (Extends,
///   Implements, References, UsesType). Mirror of `dependents`:
///   "the traits this symbol implements / types it uses / supertypes
///   it extends". Without this field outgoing IMPLEMENTS edges were
///   silently swallowed — the LuaProvider→LanguageProvider impl was
///   in the DB but never made it to the viz response.
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
    pub depends_on: Vec<NeighborSymbol>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<NeighborSymbol>,
}

const SYMBOL_COLUMNS: &str = "fqdn, name, kind, language_kind, module, visibility, \
     file_path, start_line, end_line, start_col, end_col, \
     signature_json, body_hash, flags, decl_kind, implements_trait, receiver_type, entry_point";

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
    symbol_by_fqdn_in_workspace(
        handle,
        fqdn,
        crate::storage::module_lookup::PRIMARY_WORKSPACE_ID,
    )
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
            "SELECT id, kind, to_symbol_id, to_unresolved, attributes, confidence, receiver_type \
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
                    e.attributes, e.confidence, e.receiver_type, f.fqdn \
             FROM edges e \
             JOIN symbols f ON f.id = e.from_symbol_id \
             WHERE (?1 IS NOT NULL AND e.to_symbol_id = ?1) \
                OR e.to_unresolved = ?2 \
             ORDER BY e.id ASC",
        )?;
        let rows: Vec<(EdgeRowRaw, String)> = stmt
            .query_map(rusqlite::params![target_id, fqdn], |row| {
                Ok((read_edge_row(row)?, row.get::<_, String>(7)?))
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
                    s.implements_trait, s.receiver_type, s.entry_point \
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
                    s.implements_trait, s.receiver_type, s.entry_point \
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
                    s.implements_trait, s.receiver_type, s.entry_point \
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
                    s.implements_trait, s.receiver_type, s.entry_point \
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
                    "SELECT {SYMBOL_COLUMNS}, s.language, e.description, d.description \
                     FROM symbols s \
                     LEFT JOIN enrichments e ON e.symbol_id = s.id \
                     LEFT JOIN documents   d ON d.symbol_id = s.id \
                     WHERE s.fqdn = ?1"
                ),
                [fqdn],
                |row| {
                    Ok((
                        read_symbol_row(row)?,
                        row.get::<_, String>(18)?,
                        row.get::<_, Option<String>>(19)?,
                        row.get::<_, Option<String>>(20)?,
                    ))
                },
            )
            .optional()?;
        let Some((symbol_raw, language, enrichment_description, document_description)) = raw else {
            return Ok(None);
        };
        let symbol = build_symbol(symbol_raw)?;
        Ok(Some(SymbolContext {
            symbol,
            language,
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
    let mut depends_on = Vec::new();
    for edge in edges_from(handle, fqdn)? {
        match edge.kind {
            EdgeKind::Calls => callees.push(neighbor_outbound(handle, edge, depth)?),
            EdgeKind::Imports => imports.push(neighbor_outbound(handle, edge, depth)?),
            EdgeKind::Extends
            | EdgeKind::Implements
            | EdgeKind::References
            | EdgeKind::UsesType => depends_on.push(neighbor_outbound(handle, edge, depth)?),
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
        depends_on,
        tests,
    }))
}

/// Coarse, fqdn + file-path heuristic for detecting test symbols.
/// Used by MCP tools that take an `exclude_tests` opt-in so callers
/// (LLMs, viz) can scope their queries to the production graph and
/// keep the response focused. Catches:
///
///   * Rust: `::tests::` / `::test::` modules, `_test` / `_tests`
///     trailing segments, files under `tests/` / `test/` directories,
///     `*_test.rs` / `*_tests.rs` siblings
///   * TS / JS: `.test.ts` / `.spec.ts` (+ `.tsx`, `.js`, `.jsx`)
///     suffixes, files under `__tests__/`
///
/// False positives accepted: a real symbol named `find_test` or living
/// under a directory called `tests` (the legacy Rust integration-test
/// idiom) is treated as test code. That matches the user's mental
/// model — "show me production".
pub fn symbol_looks_like_test(symbol: &RawSymbol) -> bool {
    fqdn_looks_like_test(&symbol.fqdn) || file_path_looks_like_test(&symbol.location.file)
}

/// Same heuristic as [`symbol_looks_like_test`] but takes only the
/// FQDN. Use when the caller has no `RawSymbol` (e.g. *_fqdns MCP
/// tools that return `{fqdn, kind}` pairs). Misses file-path-based
/// signals (`*.spec.ts`, `__tests__/`); pair with `symbol_looks_like_test`
/// when the full symbol is available.
pub fn fqdn_looks_like_test_only(fqdn: &str) -> bool {
    fqdn_looks_like_test(fqdn)
}

#[allow(clippy::case_sensitive_file_extension_comparisons)]
fn fqdn_looks_like_test(fqdn: &str) -> bool {
    fqdn.contains("::tests::")
        || fqdn.contains("::test::")
        || fqdn.ends_with("::tests")
        || fqdn.ends_with("::test")
        || fqdn.ends_with("_test")
        || fqdn.ends_with("_tests")
        || fqdn.ends_with(".test")
        || fqdn.ends_with(".spec")
}

fn file_path_looks_like_test(file: &str) -> bool {
    // Normalise separators so the heuristic works on Windows-style
    // paths (`\`) the same as POSIX (`/`).
    let norm = file.replace('\\', "/");
    norm.contains("/tests/")
        || norm.contains("/test/")
        || norm.contains("/__tests__/")
        || norm.ends_with("_test.rs")
        || norm.ends_with("_tests.rs")
        || norm.ends_with(".test.ts")
        || norm.ends_with(".test.tsx")
        || norm.ends_with(".test.js")
        || norm.ends_with(".test.jsx")
        || norm.ends_with(".spec.ts")
        || norm.ends_with(".spec.tsx")
        || norm.ends_with(".spec.js")
        || norm.ends_with(".spec.jsx")
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
    entry_point_text: Option<String>,
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
        entry_point_text: row.get(17)?,
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
    receiver_type: Option<String>,
}

fn read_edge_row(row: &Row<'_>) -> rusqlite::Result<EdgeRowRaw> {
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
        receiver_type: raw.receiver_type,
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
mod body_helper_tests;

#[cfg(test)]
mod tests;

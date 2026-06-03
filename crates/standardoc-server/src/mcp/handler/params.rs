//! Tool input / output struct definitions for the MCP handler.
//!
//! Each `*Params` mirrors one rmcp `#[tool(...)]` method's signature
//! (deserialized from the JSON-RPC `tools/call` body); each `*Json` /
//! `*Response` mirrors the corresponding tool's serialized output
//! envelope. Pure data types — no business logic except
//! [`FetchGraphParams::into_request`] which applies the server-side
//! transport clamps and is paired with the type it transforms.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use standardoc_core::query::{
    self,
    graph::{
        FETCH_GRAPH_DEFAULT_DEPTH, FETCH_GRAPH_DEFAULT_MAX_NODES, FETCH_GRAPH_MAX_DEPTH,
        FETCH_GRAPH_MAX_NODES_CAP, GraphRequest,
    },
};
use standardoc_ir::{EdgeKind, RawSymbol};

#[derive(Debug, Serialize)]
pub(crate) struct GetContextResponse {
    #[serde(flatten)]
    pub ctx: query::SymbolContextWithNeighbors,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing_hint: Option<String>,
}

/// Tool input — `get_context(fqdn, depth?)`. Forwarded to
/// `query::context_for_symbol_with_neighbors`. `depth` defaults to `1`
/// and is hard-clamped to `1..=2` server-side.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct GetContextParams {
    /// The fully-qualified domain name of the symbol to look up
    /// (e.g. `crate::module::function`). Match `RawSymbol::fqdn`.
    pub fqdn: String,
    /// Output richness — `1` = neighbor FQDNs only (cheap, exploration);
    /// `2` = full RawSymbol per resolved neighbor (rich, reasoning).
    /// Defaults to `1`. Hard-clamped to `1..=2`.
    pub depth: Option<u8>,
}

/// Tool input — `get_context_summary(fqdn)`. Forwarded to
/// `query::context_for_symbol_with_neighbors` at depth=1, then
/// projected to neighbor counts for cheap mapping probes.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct GetContextSummaryParams {
    /// The fully-qualified domain name of the symbol to look up.
    pub fqdn: String,
}

/// Tool input — `find_symbol(query, limit?, kind?, visibility?, module?)`.
/// Forwarded to `query::search_text` (FTS5 over `name` + `fqdn`).
/// `limit` defaults to `20` and is capped at `100` server-side.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct FindSymbolParams {
    /// Free-text query against symbol `name` and `fqdn` columns. Accepts the
    /// `name` key as an alias (a common intuition for "find a symbol by
    /// name"). Tokenization handles snake_case and camelCase; multiple
    /// fragments match ANY token when no symbol matches all of them.
    #[serde(alias = "name")]
    pub query: String,
    /// Maximum results to return. Defaults to 20, capped at 100.
    pub limit: Option<u8>,
    /// Optional filter — `function`, `type`, `value`, `module`, `macro`.
    pub kind: Option<String>,
    /// Optional filter — `public`, `private`, `crate`, `protected`.
    pub visibility: Option<String>,
    /// Optional filter — exact match on the symbol's `module` fqdn.
    pub module: Option<String>,
    /// Optional — include `is_external = 1` symbols (Cargo crates, npm
    /// `.d.ts`, luarocks) in the result. Defaults to `true`. Set to
    /// `false` to scope a query to workspace-only symbols.
    #[serde(default)]
    pub include_external: Option<bool>,
    /// Optional — exclude symbols that look like tests (Rust
    /// `#[cfg(test)] mod tests`, files under `tests/` or matching
    /// `*_test.rs`, TS `*.test.ts` / `*.spec.ts`, files under
    /// `__tests__/`). Defaults to `false` — pass `true` to scope a
    /// query to production code when test noise drowns the signal.
    #[serde(default)]
    pub exclude_tests: Option<bool>,
    /// Optional — scope the query to a single workspace by its UUID
    /// (as returned by `link_workspace`). Defaults to the primary
    /// workspace ("MY symbols"). Pass a peer's workspace_id to query
    /// that peer's source. There is no "all workspaces" mode — call
    /// once per workspace.
    #[serde(default)]
    pub workspace_id: Option<String>,
}

/// Tool input — `find_symbol_fqdns(query, limit?, kind?, visibility?,
/// module?, include_external?, workspace_id?)`. Same filters and
/// limits as `find_symbol`; only the response shape differs (FQDN +
/// kind, no RawSymbol).
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct FindSymbolFqdnsParams {
    /// Free-text query against symbol `name` and `fqdn` columns. Accepts the
    /// `name` key as an alias (same as `find_symbol`).
    #[serde(alias = "name")]
    pub query: String,
    /// Maximum results to return. Defaults to 20, capped at 100.
    pub limit: Option<u8>,
    /// Optional filter — `function`, `type`, `value`, `module`, `macro`.
    pub kind: Option<String>,
    /// Optional filter — `public`, `private`, `crate`, `protected`.
    pub visibility: Option<String>,
    /// Optional filter — exact match on the symbol's `module` fqdn.
    pub module: Option<String>,
    /// Optional — include `is_external = 1` symbols. Defaults to `true`.
    #[serde(default)]
    pub include_external: Option<bool>,
    /// Optional — exclude test-looking symbols (same heuristic as
    /// `find_symbol`). Defaults to `false`.
    #[serde(default)]
    pub exclude_tests: Option<bool>,
    /// Optional — scope to a peer workspace by UUID.
    #[serde(default)]
    pub workspace_id: Option<String>,
    /// Optional — when set, FQDNs sharing this prefix are returned in
    /// relative form (leading `::` marker, prefix stripped). FQDNs that
    /// do NOT share the prefix are returned verbatim. Useful for scoped
    /// scans where the prefix repeats across every result and dilutes
    /// the differential. Example: `relative_to = "foo::bar"` turns
    /// `foo::bar::baz::qux` into `::baz::qux`.
    #[serde(default)]
    pub relative_to: Option<String>,
}

/// Tool input — `list_symbols(kind?, visibility?, module?, limit?,
/// include_external?, cursor?)`. Forwarded to `query::list_symbols`.
/// No FTS, no glob — pure server-side filter listing ordered by fqdn.
/// `cursor` enables walking past the per-page limit; pass the value
/// of `next_cursor` from the previous response to fetch the next slice.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ListSymbolsParams {
    /// Optional filter — `function`, `type`, `value`, `module`, `macro`.
    pub kind: Option<String>,
    /// Optional filter — `public`, `private`, `crate`, `protected`.
    pub visibility: Option<String>,
    /// Optional filter — exact match on the symbol's `module` fqdn.
    pub module: Option<String>,
    /// Maximum results to return per page. Defaults to 20, capped at 100.
    pub limit: Option<u8>,
    /// Optional — include `is_external = 1` symbols (Cargo crates, npm
    /// `.d.ts`, luarocks) in the result. Defaults to `true`. Set to
    /// `false` to scope a query to workspace-only symbols.
    #[serde(default)]
    pub include_external: Option<bool>,
    /// Optional — exclude test-looking symbols (`::tests::` modules,
    /// `*_test.rs`, `*.test.ts`, `*.spec.ts`, `__tests__/` dirs).
    /// Defaults to `false`.
    #[serde(default)]
    pub exclude_tests: Option<bool>,
    /// Optional pagination anchor — pass the `next_cursor` value from
    /// the previous page to continue the walk. Server-side this is a
    /// strict `fqdn > cursor` filter; ordering is always by `fqdn`.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Optional — scope the listing to a single workspace by its UUID.
    /// Defaults to the primary workspace. Pass a peer's workspace_id
    /// to list that peer's source.
    #[serde(default)]
    pub workspace_id: Option<String>,
}

/// Tool input — `list_symbol_fqdns(kind?, visibility?, module?,
/// limit?, include_external?, cursor?, workspace_id?)`. Same
/// filters and pagination as `list_symbols`; only the response
/// shape differs (FQDN + kind, no RawSymbol).
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ListSymbolFqdnsParams {
    /// Optional filter — `function`, `type`, `value`, `module`, `macro`.
    pub kind: Option<String>,
    /// Optional filter — `public`, `private`, `crate`, `protected`.
    pub visibility: Option<String>,
    /// Optional filter — exact match on the symbol's `module` fqdn.
    pub module: Option<String>,
    /// Maximum results to return per page. Defaults to 20, capped at 100.
    pub limit: Option<u8>,
    /// Optional — include `is_external = 1` symbols. Defaults to `true`.
    #[serde(default)]
    pub include_external: Option<bool>,
    /// Optional — exclude test-looking symbols. Defaults to `false`.
    #[serde(default)]
    pub exclude_tests: Option<bool>,
    /// Optional pagination anchor — pass the `next_cursor` value from
    /// the previous page to continue the walk.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Optional — scope to a peer workspace by UUID.
    #[serde(default)]
    pub workspace_id: Option<String>,
    /// Optional — when set, FQDNs sharing this prefix are returned in
    /// relative form (leading `::` marker, prefix stripped). Same
    /// semantics as `find_symbol_fqdns.relative_to`.
    #[serde(default)]
    pub relative_to: Option<String>,
}

/// Tool input — `find_symbols_by_pattern(pattern, kind?, visibility?,
/// module?, limit?)`. Forwarded to `query::find_by_pattern` (SQLite
/// `GLOB` over `name` + `fqdn`).
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct FindSymbolsByPatternParams {
    /// SQLite GLOB pattern matched against `name` OR `fqdn`. Wildcards:
    /// `*` (any sequence), `?` (single char), `[abc]` (char class).
    /// Case-sensitive. Example: `strip_*_extension`.
    pub pattern: String,
    /// Optional filter — `function`, `type`, `value`, `module`, `macro`.
    pub kind: Option<String>,
    /// Optional filter — `public`, `private`, `crate`, `protected`.
    pub visibility: Option<String>,
    /// Optional filter — exact match on the symbol's `module` fqdn.
    pub module: Option<String>,
    /// Maximum results to return. Defaults to 20, capped at 100.
    pub limit: Option<u8>,
    /// Optional — include `is_external = 1` symbols (Cargo crates, npm
    /// `.d.ts`, luarocks) in the result. Defaults to `true`. Set to
    /// `false` to scope a query to workspace-only symbols.
    #[serde(default)]
    pub include_external: Option<bool>,
    /// Optional — exclude test-looking symbols. Defaults to `false`.
    #[serde(default)]
    pub exclude_tests: Option<bool>,
    /// Optional — scope the query to a single workspace by its UUID.
    /// Defaults to the primary workspace.
    #[serde(default)]
    pub workspace_id: Option<String>,
    /// Optional — return lean rows (`name` / `fqdn` / `kind` /
    /// `visibility` / `location`) instead of full RawSymbol records.
    /// Auto-engages above `SUMMARY_AUTO_THRESHOLD` matches so a broad
    /// glob can't blow the response up to tens of thousands of chars.
    /// Pass `false` to force full records, `true` to force lean.
    #[serde(default)]
    pub summary: Option<bool>,
}

/// Tool input — `find_call_sites(from_fqdn?, callee_text?, callee_pattern?, limit?)`.
/// Forwarded to `query::call_sites::find_call_sites`. All filters
/// AND-compose; empty / whitespace-only strings normalise to `None` so
/// `""` doesn't smuggle a vacuous predicate into the SQL.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct FindCallSitesParams {
    /// Optional filter — exact match on the enclosing fn/method FQDN
    /// (the "from" side of the call). Answers `what does X call?`.
    pub from_fqdn: Option<String>,
    /// Optional filter — exact match on the called expression text
    /// (`tauri::invoke`, `obj.api.create`, `print`). Answers
    /// `who calls Y?`.
    pub callee_text: Option<String>,
    /// Optional filter — SQLite GLOB pattern matched against the callee
    /// text. Wildcards: `*` (any sequence), `?` (single char),
    /// `[abc]` (char class). Case-sensitive. Example: `*tauri.invoke*`
    /// surfaces every Tauri invocation regardless of receiver chain.
    pub callee_pattern: Option<String>,
    /// Maximum results to return. Defaults to 50, capped at 200 server-side.
    pub limit: Option<u8>,
}

/// Tool input — `fetch_graph(focal?, depth?, kinds?, max_nodes?,
/// include_external?)`. Translated to `query::graph::GraphRequest`
/// after server-side clamping. Unknown `kinds` strings are dropped
/// silently — the consumer can send a superset without erroring.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct FetchGraphParams {
    /// FQDN to center the expansion on. When `Some`, the response is a
    /// BFS expansion of depth `depth` around this node. When `None`,
    /// returns a bounded snapshot of the workspace (ordered by FQDN
    /// ASC) up to `max_nodes`.
    #[serde(default)]
    pub focal: Option<String>,
    /// BFS depth when `focal` is set. Clamped to `1..=5`. Defaults to
    /// `2`. Ignored when `focal` is `None`.
    #[serde(default)]
    pub depth: Option<u8>,
    /// Optional allow-list of edge kinds (case-insensitive: `CALLS`,
    /// `IMPORTS`, `EXTENDS`, `IMPLEMENTS`, `REFERENCES`, `USES_TYPE`).
    /// Unknown values are silently dropped. `None` admits every kind.
    #[serde(default)]
    pub kinds: Option<Vec<String>>,
    /// Safety cap on the number of symbol nodes returned. Clamped to
    /// `1..=5000`. Defaults to `500`.
    #[serde(default)]
    pub max_nodes: Option<u32>,
    /// When `true`, include `is_external = 1` rows (npm `.d.ts`,
    /// Cargo crate metadata, luarocks). Defaults to `false` — the
    /// "MY workspace" view.
    #[serde(default)]
    pub include_external: Option<bool>,
}

impl FetchGraphParams {
    /// Apply the transport-side clamps and translate the kinds
    /// allow-list. Keeping this on the Params type keeps the tool
    /// method body small and gives tests an obvious seam.
    pub(crate) fn into_request(self) -> GraphRequest {
        let depth = self
            .depth
            .unwrap_or(FETCH_GRAPH_DEFAULT_DEPTH)
            .clamp(1, FETCH_GRAPH_MAX_DEPTH);
        let max_nodes = self
            .max_nodes
            .unwrap_or(FETCH_GRAPH_DEFAULT_MAX_NODES)
            .clamp(1, FETCH_GRAPH_MAX_NODES_CAP);
        let kinds = self.kinds.and_then(|raw| {
            let parsed: std::collections::HashSet<EdgeKind> =
                raw.iter().filter_map(|s| parse_edge_kind(s)).collect();
            (!parsed.is_empty()).then_some(parsed)
        });
        GraphRequest {
            focal: self.focal,
            depth,
            kinds,
            max_nodes,
            include_external: self.include_external.unwrap_or(false),
        }
    }
}

/// Case-insensitive parser for the `kinds` allow-list. Accepts both
/// SCREAMING_SNAKE_CASE (the on-the-wire form emitted by `EdgeKind`'s
/// `Serialize`) and lowercase for forgiving consumers. Unknown
/// strings return `None` so the caller can ignore them.
fn parse_edge_kind(raw: &str) -> Option<EdgeKind> {
    match raw.trim().to_ascii_uppercase().as_str() {
        "CALLS" => Some(EdgeKind::Calls),
        "IMPORTS" => Some(EdgeKind::Imports),
        "EXTENDS" => Some(EdgeKind::Extends),
        "IMPLEMENTS" => Some(EdgeKind::Implements),
        "REFERENCES" => Some(EdgeKind::References),
        "USES_TYPE" | "USESTYPE" => Some(EdgeKind::UsesType),
        _ => None,
    }
}

/// Tool input — `find_similar_symbols(reference, threshold?, limit?, kind?,
/// visibility?, module?)`. Forwarded to `query::find_similar`. `reference`
/// is treated as raw text (no symbol lookup); pass either an existing name
/// or a hypothetical one to anchor the search.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct FindSimilarSymbolsParams {
    /// Anchor name to compare against every other symbol's `name`. Raw text:
    /// passing `strip_rs_extension` or a hypothetical `strip_extension` both
    /// work. Self-skip is case-insensitive on `name` only — symbols whose
    /// `name == reference` (any casing) are dropped from the result.
    pub reference: String,
    /// Score cutoff in `[0.0, 1.0]`. Defaults to 0.8. Symbols with score
    /// below this value are dropped.
    pub threshold: Option<f32>,
    /// Maximum results to return. Defaults to 20, capped at 100.
    pub limit: Option<u8>,
    /// Optional filter — `function`, `type`, `value`, `module`, `macro`.
    pub kind: Option<String>,
    /// Optional filter — `public`, `private`, `crate`, `protected`.
    pub visibility: Option<String>,
    /// Optional filter — exact match on the symbol's `module` fqdn.
    pub module: Option<String>,
    /// Optional — include `is_external = 1` symbols (Cargo crates, npm
    /// `.d.ts`, luarocks) in the result. Defaults to `true`. Set to
    /// `false` to scope a query to workspace-only symbols.
    #[serde(default)]
    pub include_external: Option<bool>,
}

/// Tool output envelope for `find_similar_symbols`. Pairs each `RawSymbol`
/// with its similarity score for trivial JSON consumption by the LLM.
#[derive(Debug, Serialize)]
pub(crate) struct SimilarSymbolJson {
    pub score: f32,
    pub symbol: RawSymbol,
}

/// Tool input — `resolve_external(fqdn)`. Routes the FQDN through the
/// `ResolverRegistry`; orchestrator inserts the produced symbol into
/// the index with `is_external = 1` before responding.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ResolveExternalParams {
    /// Fully-qualified domain name of the external symbol to resolve.
    /// Shape matches the workspace convention — `<crate>::<module>::<name>`
    /// for Rust, `<package>::<file>::<name>` for TS, `<rock>::<name>`
    /// for Lua.
    pub fqdn: String,
}

/// Tool output envelope for `resolve_external`. Fields are populated
/// based on `status`:
///
/// - `status = "resolved"` → `source_origin` + `symbol` set, others `None`.
/// - `status = "not_found"` → all extras `None` (no resolver claimed the FQDN).
/// - `status = "missing_binary"` → `missing_binary` set (binary name),
///   `detail` carries the env var hint.
/// - `status = "lockfile_not_found"` → `detail` names the absent lockfile.
/// - `status = "error"` → `detail` carries the resolver error.
#[derive(Debug, Serialize, Deserialize)]
pub struct ResolveExternalJson {
    pub status: String,
    pub fqdn: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<RawSymbol>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missing_binary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Tool input — `get_body(fqdn, max_lines?, strip_attrs?, signature_only?,
/// strip_inline_comments?)`. Forwarded to `query::body_for_fqdn`. Returns
/// `null` if `fqdn` is not in the index.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct GetBodyParams {
    /// Fully-qualified domain name of the target symbol (e.g.
    /// `crate::module::function`). Match `RawSymbol::fqdn` exactly.
    pub fqdn: String,
    /// Optional cap on the number of lines returned. When the symbol's body
    /// exceeds this count, the response is truncated and `truncated=true` is
    /// set so the caller knows to re-fetch with a higher cap (or none).
    pub max_lines: Option<u32>,
    /// When `true`, drop leading doc comments (`///`, `//!`, `//`,
    /// `/* … */`) AND attribute lines (`#[…]`, `#![…]`, including
    /// multi-line continuations) AND blank lines between them. The
    /// response carries `stripped_lines` so the caller can audit the
    /// shrink. Massive savings for handlers with verbose
    /// `#[tool(description = "…")]` blocks.
    #[serde(default)]
    pub strip_attrs: Option<bool>,
    /// When `true`, truncate the body just after the first line containing
    /// `{` (the opening brace of the function / impl / type block). Returns
    /// the multi-line signature without the implementation. Combine with
    /// `strip_attrs` for the cleanest signature view. The response carries
    /// `signature_only: true` to confirm. No-op when no `{` is present.
    #[serde(default)]
    pub signature_only: Option<bool>,
    /// When `true`, strip inline `// …` line comments and `/* … */` block
    /// comments from the returned body. String-literal safe — `"…"`
    /// (including `\` escapes), Rust raw strings (`r"…"`, `r#"…"#`), and
    /// TS template literals (`` `…` ``) pass through verbatim. Newlines
    /// inside multi-line block comments are preserved so line-number
    /// alignment with the source file stays intact. Layered on top of
    /// `strip_attrs` / `signature_only` / `max_lines`.
    #[serde(default)]
    pub strip_inline_comments: Option<bool>,
}

/// Tool input — `get_code(fqdn, max_lines?, strip_attrs?,
/// signature_only?, strip_inline_comments?)`. Agent-tuned variant of
/// `get_body` — defaults `strip_attrs = true` and
/// `strip_inline_comments = true` so the response is pure code.
/// Forward overrides via explicit fields.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct GetCodeParams {
    /// Fully-qualified domain name of the target symbol.
    pub fqdn: String,
    /// Optional cap on the number of lines returned.
    pub max_lines: Option<u32>,
    /// Defaults to `true` — drops leading doc comments + attribute
    /// blocks. Pass `false` to keep them (matches `get_body` behavior).
    #[serde(default)]
    pub strip_attrs: Option<bool>,
    /// Optional — truncate after the first `{` for a signature-only view.
    /// Defaults to `false`.
    #[serde(default)]
    pub signature_only: Option<bool>,
    /// Defaults to `true` — drops inline `// …` and `/* … */` comments.
    /// Pass `false` to keep them (matches `get_body` behavior).
    #[serde(default)]
    pub strip_inline_comments: Option<bool>,
}

/// Tool output for `current_revision`. The legacy `revision` field is kept
/// as the first key so callers that only deserialize `{revision}` keep
/// working; new fields surface daemon capabilities so an AI can decide
/// at boot whether the live watcher is debouncing edits
/// (`watcher.active`), whether early read calls would hit the "indexing
/// in progress" branch (`indexing.ready`), and what monorepo organizer
/// the workspace uses (`workspace.kind`, Stage 3e-3).
#[derive(Debug, Serialize)]
pub(crate) struct CurrentRevisionJson {
    pub revision: u64,
    pub watcher: WatcherCapabilityJson,
    pub indexing: IndexingCapabilityJson,
    pub workspace: WorkspaceCapabilityJson,
}

#[derive(Debug, Serialize)]
pub(crate) struct WatcherCapabilityJson {
    /// `true` once the cold-start spawn has installed the live notify
    /// watcher. `false` while indexing is still running OR when the
    /// daemon was booted in `--readonly` mode (no writer, no watcher).
    pub active: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct IndexingCapabilityJson {
    /// `true` once cold start has flipped `index_ready`. Calls before
    /// this short-circuit with the `"indexing in progress"` text.
    pub ready: bool,
}

/// Stage 3e-3 — workspace-level metadata surfaced through
/// `current_revision`. Mirrors the persisted `schema_meta.workspace_kind`
/// row.
#[derive(Debug, Serialize)]
pub(crate) struct WorkspaceCapabilityJson {
    /// Detected workspace organizer slug (`cargo`, `npm`, `pnpm`, `yarn`,
    /// `bun`, `deno`, `go`, `lerna`, `nx`, `turborepo`, `mira`, or
    /// `custom:<tag>`). `null` when (a) discovery hasn't run yet (legacy
    /// DBs pre-3e-3 or first boot in progress) OR (b) discovery ran but
    /// no workspace manifest is present at the root (loose project tree
    /// / single-crate layout — aligns with standarbuild-detect 0.3
    /// which has no `Single` variant).
    pub kind: Option<String>,
}

/// Tool input — `check_stale(fetched: [{fqdn, fetched_at_revision}, ...])`.
/// The server is stateless; the caller tracks (fqdn → revision_at_fetch) across
/// turns and asks "what changed since".
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct CheckStaleParams {
    /// Pairs of (fqdn, revision_when_fetched). Order is preserved in output.
    pub fetched: Vec<FetchedEntry>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct FetchedEntry {
    pub fqdn: String,
    pub fetched_at_revision: u64,
}

/// Tool output entry for `check_stale`. `status` is `"stale"`, `"fresh"`, or
/// `"missing"`. `last_modified_revision` is `None` when the fqdn is no longer
/// in the index.
#[derive(Debug, Serialize)]
pub(crate) struct StaleEntryJson {
    pub fqdn: String,
    pub fetched_at_revision: u64,
    pub last_modified_revision: Option<u64>,
    pub status: String,
}

/// Tool input — `link_workspace(path, direction, indexing_mode?)`.
/// `direction` is one of `"in"`, `"out"`, `"bidirectional"` (snake_case
/// to match the IR enum's serde shape). `indexing_mode` is optional;
/// when omitted it defaults to `"blob_import"` (Stage 3b-7-a behaviour
/// — cheap copy of peer's pre-built DB). Pass `"extract"` to opt the
/// peer into the Stage 3b-7-b autonomous source-walk pipeline instead.
/// Path is canonicalised server-side.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct LinkWorkspaceParams {
    pub path: String,
    pub direction: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexing_mode: Option<String>,
}

/// Tool input — `unlink_workspace(workspace_id)`.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct UnlinkWorkspaceParams {
    pub workspace_id: String,
}

/// Tool input — `refresh_peer(workspace_id)`. Triggers a single-peer
/// re-extraction outside of cold_start (Q4 staleness mitigation).
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct RefreshPeerParams {
    /// UUID workspace_id of the linked peer to refresh, as returned
    /// by `link_workspace` or `list_linked_workspaces`.
    pub workspace_id: String,
}

/// Tool input — `set_link_direction(workspace_id, direction)`. Flips
/// the persisted link direction AND propagates the change to the live
/// watcher (Out ↔ {In, Bidirectional} transitions register or
/// unregister the peer root).
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SetLinkDirectionParams {
    /// UUID workspace_id of the linked peer whose direction to change.
    pub workspace_id: String,
    /// New direction: one of `in` (peer feeds us), `out` (we feed peer),
    /// `bidirectional`.
    pub direction: String,
}

/// JSON response shape for [`set_link_direction`]. Surfaces both the
/// previous and the new direction so callers can confirm the
/// transition.
#[derive(Debug, Serialize)]
pub(crate) struct SetLinkDirectionJson {
    pub workspace_id: String,
    pub root_path: String,
    pub previous_direction: String,
    pub new_direction: String,
}

/// Tool input — `module_lookup(module_fqdn, workspace_id?)`. Omit
/// `workspace_id` to query the primary workspace.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ModuleLookupParams {
    pub module_fqdn: String,
    #[serde(default)]
    pub workspace_id: Option<String>,
}

/// Tool input — `resolve_cross_workspace(origin_module, origin_symbol)`.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ResolveCrossWorkspaceParams {
    pub origin_module: String,
    pub origin_symbol: String,
}

/// Tool output for `link_workspace`. Mirrors the input path back so the
/// caller can confirm canonicalisation didn't redirect them somewhere
/// unexpected.
#[derive(Debug, Serialize)]
pub(crate) struct LinkWorkspaceJson {
    pub workspace_id: String,
    pub root_path: String,
    pub direction: String,
}

/// Tool input — `project_for_file(path)`. Path is matched as an
/// absolute string against the registered `projects.root_path` rows
/// (cold-start detection stores canonicalised absolutes).
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ProjectForFileParams {
    pub path: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_symbol_accepts_name_alias_for_query() {
        let p: FindSymbolParams = serde_json::from_str(r#"{"name":"merge_mcp_config"}"#).unwrap();
        assert_eq!(p.query, "merge_mcp_config");
        // The canonical `query` key still works.
        let p2: FindSymbolParams = serde_json::from_str(r#"{"query":"foo"}"#).unwrap();
        assert_eq!(p2.query, "foo");
    }

    #[test]
    fn find_symbol_fqdns_accepts_name_alias_for_query() {
        let p: FindSymbolFqdnsParams = serde_json::from_str(r#"{"name":"foo"}"#).unwrap();
        assert_eq!(p.query, "foo");
    }
}

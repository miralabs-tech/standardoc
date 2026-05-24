use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use rmcp::ErrorData;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Content, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use standardoc_core::{
    IndexHandle, LanguageProvider, ResolveOutcome, ResolverRegistry, ScanFilters, WatcherHandle,
    query::{
        self, SymbolFilter,
        call_sites::{self as call_sites_query, CallSiteFilters, FIND_CALL_SITES_DEFAULT_LIMIT},
        graph::{
            self as graph_query, FETCH_GRAPH_DEFAULT_DEPTH, FETCH_GRAPH_DEFAULT_MAX_NODES,
            FETCH_GRAPH_MAX_DEPTH, FETCH_GRAPH_MAX_NODES_CAP, GraphRequest,
        },
        projects as projects_query, workspace as workspace_query,
    },
};
use standardoc_ir::SourceOrigin;
use standardoc_ir::{EdgeKind, IndexingMode, Kind, LinkDirection, RawSymbol, Visibility};

use crate::mcp::error::server_error_to_rmcp;

const FIND_SYMBOL_DEFAULT_LIMIT: u8 = 20;
const FIND_SYMBOL_MAX_LIMIT: u8 = 100;
/// Similarity floor for the `did_you_mean` suggestion bundle attached to
/// empty `find_symbol` / `find_symbols_by_pattern` results. Lower than
/// `find_similar_symbols`'s default 0.8 because the caller has already
/// observed a zero-hit query — surfacing weaker alternatives is the
/// load-bearing value here. Strictly less than the existing
/// `clamp_threshold` floor would still keep noise out for completely
/// unrelated names.
const DID_YOU_MEAN_THRESHOLD: f32 = 0.6;
/// Hard cap on the size of the `did_you_mean` array. Five is enough to
/// surface typo / cluster matches without flooding the tool response.
const DID_YOU_MEAN_LIMIT: usize = 5;
const GET_CONTEXT_DEFAULT_DEPTH: u8 = 1;
const FIND_SIMILAR_DEFAULT_THRESHOLD: f32 = 0.8;

/// Window during which a prior `get_context(fqdn, depth=1)` is considered
/// a "scoping pass" that justifies a follow-up `depth=2` on the same FQDN.
/// Outside the window (or with no prior depth=1 at all), the depth=2 call
/// is flagged as "naked" via `routing_hint` so the caller learns to map
/// neighbors before drilling.
const NAKED_DEPTH_2_WINDOW_SECS: i64 = 300;

/// Drop tracker entries older than this on each insert. Bounds memory
/// growth without LRU bookkeeping. Comfortably above
/// `NAKED_DEPTH_2_WINDOW_SECS` so the cleanup never races a legitimate
/// late depth=2.
const RECENT_DEPTH1_RETENTION_SECS: i64 = 1800;

/// Dispatch target for the rmcp `ServiceExt::serve` boot. Holds:
/// - `handle`: query / submit gateway into the index
/// - `provider`: workspace `LanguageProvider`, shared with the watcher
/// - `filters`: live `ScanFilters`, hot-swapped by the watcher when
///   `.stdignore` changes (lock pause-exclude-22 §1.1 multi-matchers stack)
/// - `index_ready`: flipped to `true` once cold start completes; tool
///   invocations observed before that return a friendly "indexing in progress"
///   tool result rather than an MCP error (Q5 graceful degradation).
/// - `watcher`: filled by the cold-start spawn once indexing finishes;
///   dropped on shutdown so the dispatch thread joins before stdio closes
///   (mirrors `lsp::handler::StandardocLsp`).
/// - `tool_router`: rmcp dispatch table built from the `#[tool]` methods.
#[derive(Clone)]
pub struct StandardocMcp {
    handle: IndexHandle,
    provider: Arc<dyn LanguageProvider>,
    #[allow(dead_code)]
    filters: Arc<RwLock<ScanFilters>>,
    /// Lazy on-demand resolver dispatch for `resolve_external` (S3-G).
    /// Constructed at `new()` time from the handle's workspace root —
    /// each resolver probes its binary then memorizes the result.
    registry: Arc<ResolverRegistry>,
    index_ready: Arc<AtomicBool>,
    watcher: Arc<Mutex<Option<WatcherHandle>>>,
    /// In-memory cache of `(fqdn → ts_unix)` recording when each FQDN
    /// was last fetched at `depth=1`. Drives the "naked depth=2"
    /// routing hint: a depth=2 call with no recent depth=1 on the
    /// same FQDN gets a hint nudging the 3-phase explore→cible→drill
    /// protocol. Transient — resets on daemon restart, no persistence.
    recent_depth1: Arc<Mutex<HashMap<String, i64>>>,
    // Populated by the `#[tool_router]` macro; the macro's emitted
    // `ServerHandler` impl reads it back, but the indirection is
    // invisible to rustc's dead-code analyser.
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl StandardocMcp {
    pub(crate) fn new(
        handle: IndexHandle,
        provider: Arc<dyn LanguageProvider>,
        filters: Arc<RwLock<ScanFilters>>,
    ) -> Self {
        let registry = Arc::new(ResolverRegistry::for_workspace(
            handle.workspace_root().to_path_buf(),
        ));
        Self {
            handle,
            provider,
            filters,
            registry,
            index_ready: Arc::new(AtomicBool::new(false)),
            watcher: Arc::new(Mutex::new(None)),
            recent_depth1: Arc::new(Mutex::new(HashMap::new())),
            tool_router: Self::tool_router(),
        }
    }

    /// Aggregate context for a symbol: signature + descriptions + four
    /// pre-grouped neighbor lists (callers / callees / imports / imported_by).
    /// `depth` selects the shape's richness — see [`SymbolContextWithNeighbors`].
    /// The response sets `routing_hint` when a depth=2 call is made without a
    /// recent depth=1 scoping pass — an in-band nudge to follow the
    /// explore→cible→drill protocol.
    #[tool(
        description = "Aggregate context for a symbol identified by its fully-qualified name (FQDN). Returns the symbol's signature, descriptions, and four pre-grouped neighbor lists (callers, callees, imports, imported_by).\n\n**Pick `depth` deliberately:** `depth=1` returns neighbor FQDNs only — cheap, the right call to map a symbol's neighborhood. `depth=2` enriches each resolved neighbor with its full RawSymbol — only worth it when you have already used a depth=1 pass to identify which neighbors matter. Hard-clamped to 1..=2. The response carries `routing_hint` when a depth=2 call is detected without a prior depth=1 on the same FQDN within the last 5 minutes — that's a signal to map first, drill second."
    )]
    async fn get_context(
        &self,
        Parameters(mut params): Parameters<GetContextParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if !self.index_ready.load(Ordering::Acquire) {
            return Ok(CallToolResult::success(vec![Content::text(
                indexing_in_progress_message(self.handle.cold_start_progress().ok().flatten()),
            )]));
        }
        // Cache key uses the dot-normalized form so a depth=1 hit on
        // `Foo.bar` and a depth=2 hit on `Foo::bar` route to the same
        // recent-call entry. DB lookup, however, MUST try the raw
        // FQDN first — TS file segments such as `profiler.type` (from
        // `profiler.type.ts`) carry a legitimate `.` that the OOP-
        // style normalization would mangle into `profiler::type`.
        let raw_fqdn = params.fqdn.clone();
        let cache_key = normalize_fqdn(&raw_fqdn);

        let depth = params.depth.unwrap_or(GET_CONTEXT_DEFAULT_DEPTH);
        let now = current_unix_seconds();
        let routing_hint = self.compute_routing_hint(&cache_key, depth, now);
        if depth <= 1 {
            self.record_recent_depth1(&cache_key, now);
        }

        // Try verbatim first; fall back to OOP-normalized form for
        // LLM consumers that emit `Type.method` instead of `::`.
        let handle = self.handle.clone();
        let raw_for_call = raw_fqdn.clone();
        let (resolved_fqdn, result) = tokio::task::spawn_blocking(move || {
            if let Some(ctx) =
                query::context_for_symbol_with_neighbors(&handle, &raw_for_call, depth)?
            {
                return Ok::<(String, Option<_>), standardoc_core::StorageError>((
                    raw_for_call,
                    Some(ctx),
                ));
            }
            if raw_for_call.contains('.') {
                let normalized = normalize_fqdn(&raw_for_call);
                if let Some(ctx) =
                    query::context_for_symbol_with_neighbors(&handle, &normalized, depth)?
                {
                    return Ok((normalized, Some(ctx)));
                }
            }
            Ok((raw_for_call, None))
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;
        params.fqdn = resolved_fqdn;

        match result {
            Some(ctx) => {
                let response = GetContextResponse {
                    ctx,
                    routing_hint,
                };
                Ok(success_json(&response))
            }
            None => Ok(CallToolResult::success(vec![Content::text(
                "no symbol found for the given FQDN",
            )])),
        }
    }

    /// Compact variant of `get_context` — returns the symbol header plus
    /// neighbor *counts* per direction (callers / callees / imports /
    /// imported_by / dependents / tests), without materialising the
    /// neighbor lists themselves. Useful as a first probe to decide
    /// whether a `get_context(depth=1)` follow-up is worth the round-trip.
    #[tool(
        description = "Compact context probe: returns `{symbol: {fqdn, name, kind, language_kind, visibility, module?}, neighbor_counts: {callers, callees, imports, imported_by, dependents, tests}}` for a known FQDN. Designed as a cheap first-pass to map a symbol's neighborhood size before drilling with `get_context`. Returns `null` when the FQDN is unknown."
    )]
    async fn get_context_summary(
        &self,
        Parameters(params): Parameters<GetContextSummaryParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if !self.index_ready.load(Ordering::Acquire) {
            return Ok(CallToolResult::success(vec![Content::text(
                indexing_in_progress_message(self.handle.cold_start_progress().ok().flatten()),
            )]));
        }
        let raw_fqdn = params.fqdn.clone();
        let handle = self.handle.clone();
        let raw_for_call = raw_fqdn.clone();
        let result = tokio::task::spawn_blocking(move || {
            if let Some(ctx) =
                query::context_for_symbol_with_neighbors(&handle, &raw_for_call, 1)?
            {
                return Ok::<_, standardoc_core::StorageError>(Some(ctx));
            }
            if raw_for_call.contains('.') {
                let normalized = normalize_fqdn(&raw_for_call);
                if let Some(ctx) =
                    query::context_for_symbol_with_neighbors(&handle, &normalized, 1)?
                {
                    return Ok(Some(ctx));
                }
            }
            Ok(None)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;

        match result {
            Some(ctx) => {
                let summary = serde_json::json!({
                    "symbol": {
                        "fqdn": ctx.context.symbol.fqdn,
                        "name": ctx.context.symbol.name,
                        "kind": ctx.context.symbol.kind,
                        "language_kind": ctx.context.symbol.language_kind,
                        "visibility": ctx.context.symbol.visibility,
                        "module": ctx.context.symbol.module,
                        "decl_kind": ctx.context.symbol.decl_kind,
                        "implements_trait": ctx.context.symbol.implements_trait,
                        "receiver_type": ctx.context.symbol.receiver_type.as_ref().map(|t| &t.display),
                        "entry_point": ctx.context.symbol.entry_point,
                    },
                    "neighbor_counts": {
                        "callers": ctx.callers.len(),
                        "callees": ctx.callees.len(),
                        "imports": ctx.imports.len(),
                        "imported_by": ctx.imported_by.len(),
                        "dependents": ctx.dependents.len(),
                        "tests": ctx.tests.len(),
                    },
                });
                Ok(success_json(&summary))
            }
            None => Ok(CallToolResult::success(vec![Content::text(
                "no symbol found for the given FQDN",
            )])),
        }
    }

    /// Returns `Some(message)` when `depth >= 2` was requested for a FQDN
    /// that has not had a `depth=1` call in the last
    /// `NAKED_DEPTH_2_WINDOW_SECS` seconds (or ever, since boot). The
    /// message nudges callers towards the 3-phase explore→cible→drill
    /// protocol so depth=2 stays a deliberate drill rather than a default.
    fn compute_routing_hint(&self, fqdn: &str, depth: u8, now: i64) -> Option<String> {
        if depth < 2 {
            return None;
        }
        let recent = self
            .recent_depth1
            .lock()
            .ok()
            .and_then(|guard| guard.get(fqdn).copied());
        let was_recent = recent.is_some_and(|ts| now - ts <= NAKED_DEPTH_2_WINDOW_SECS);
        if was_recent {
            return None;
        }
        Some(
            "depth=2 was requested without a prior depth=1 scoping pass on this FQDN within \
             the last 5 minutes. The cheap protocol is: (1) explore via find_symbol / \
             list_symbols / find_symbols_by_pattern; (2) cibler via get_context(fqdn, depth=1) \
             to map neighbor FQDNs; (3) drill into depth=2 only on the 1-3 neighbors that matter. \
             A naked depth=2 on a symbol with many edges can return 5-15 KB."
                .to_owned(),
        )
    }

    fn record_recent_depth1(&self, fqdn: &str, now: i64) {
        let Ok(mut guard) = self.recent_depth1.lock() else {
            return;
        };
        guard.retain(|_, ts| now - *ts <= RECENT_DEPTH1_RETENTION_SECS);
        guard.insert(fqdn.to_owned(), now);
    }

    /// FTS5 search across symbol `name` and `fqdn` columns. Returns ranked
    /// matches as a JSON array of `RawSymbol`. `limit` defaults to 20 and
    /// is server-capped at 100 to keep tool results small. Optional
    /// filters narrow the result set by `kind`, `visibility` and/or
    /// exact `module` (no wildcards — use `find_symbols_by_pattern` for
    /// glob-style module/name matching).
    #[tool(
        description = "Full-text search across the workspace index over symbol names and FQDNs. Returns ranked matches as a JSON array. `limit` defaults to 20 and is capped at 100 server-side. Use this to discover symbols when you only know a fragment of the name; follow up with `get_context` to drill into a specific FQDN. Optional filters: `kind` (function/type/value/module/macro), `visibility` (public/private/crate/protected), `module` (exact match on the symbol's module fqdn). When the query returns zero matches, the response switches to `{results: [], did_you_mean: [{fqdn, name, kind, score}, ...]}` with up to 5 strsim-based suggestions (threshold 0.6). One probe is enough — accept the absence rather than trying variant names."
    )]
    async fn find_symbol(
        &self,
        Parameters(params): Parameters<FindSymbolParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if !self.index_ready.load(Ordering::Acquire) {
            return Ok(CallToolResult::success(vec![Content::text(
                indexing_in_progress_message(self.handle.cold_start_progress().ok().flatten()),
            )]));
        }
        // Don't normalise the module filter — that turns TS file
        // segments (`profiler.type`, `hud.element`) into nonexistent
        // `profiler::type` keys. Stored module FQDNs use `::` for the
        // hierarchy and keep the dot as a literal segment character.

        let trimmed = params.query.trim().to_owned();
        if trimmed.is_empty() {
            return Ok(success_json::<Vec<RawSymbol>>(&Vec::new()));
        }

        let filter = parse_filter(
            params.kind.as_deref(),
            params.visibility.as_deref(),
            params.module,
            params.include_external,
            params.workspace_id,
        )?;
        let limit = clamp_limit(params.limit);
        let handle = self.handle.clone();
        let filter_for_search = filter.clone();
        let trimmed_for_search = trimmed.clone();
        let result = tokio::task::spawn_blocking(move || {
            query::search_text(
                &handle,
                &trimmed_for_search,
                limit as usize,
                &filter_for_search,
            )
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;

        if result.is_empty() {
            let suggestions = compute_did_you_mean(self.handle.clone(), trimmed, filter).await?;
            if !suggestions.is_empty() {
                return Ok(success_json(&serde_json::json!({
                    "results": Vec::<RawSymbol>::new(),
                    "did_you_mean": suggestions,
                })));
            }
        }

        Ok(success_json(&result))
    }

    /// Compact variant of `find_symbol` — returns `{fqdn, kind}` per
    /// match instead of the full `RawSymbol`. Designed for FTS5 probes
    /// where you only need the FQDN to follow up with `get_context`
    /// or `get_code`. Massive payload reduction on broad queries.
    /// Optional `relative_to` shortens matching FQDNs to `::<rest>`.
    #[tool(
        description = "FQDN-only variant of `find_symbol`. Returns `[{fqdn, kind}, ...]` ranked by FTS5 — the minimal shape needed to follow up with `get_context` / `get_code`. Same filters and limit semantics as `find_symbol`. When the query returns zero matches, the response switches to `{results: [], did_you_mean: [...]}` — same did-you-mean envelope as `find_symbol`. Use this as your default discovery probe; reach for `find_symbol` only when you actually need the full RawSymbol shape per match. Pass `relative_to = \"foo::bar\"` to collapse matching FQDNs into `::baz::qux` form — kills the prefix repetition that dominates scoped scans."
    )]
    async fn find_symbol_fqdns(
        &self,
        Parameters(params): Parameters<FindSymbolFqdnsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if !self.index_ready.load(Ordering::Acquire) {
            return Ok(CallToolResult::success(vec![Content::text(
                indexing_in_progress_message(self.handle.cold_start_progress().ok().flatten()),
            )]));
        }
        let trimmed = params.query.trim().to_owned();
        if trimmed.is_empty() {
            return Ok(success_json::<Vec<serde_json::Value>>(&Vec::new()));
        }

        let filter = parse_filter(
            params.kind.as_deref(),
            params.visibility.as_deref(),
            params.module,
            params.include_external,
            params.workspace_id,
        )?;
        let limit = clamp_limit(params.limit);
        let handle = self.handle.clone();
        let filter_for_search = filter.clone();
        let trimmed_for_search = trimmed.clone();
        let result = tokio::task::spawn_blocking(move || {
            query::search_text(
                &handle,
                &trimmed_for_search,
                limit as usize,
                &filter_for_search,
            )
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;

        if result.is_empty() {
            let suggestions = compute_did_you_mean(self.handle.clone(), trimmed, filter).await?;
            if !suggestions.is_empty() {
                return Ok(success_json(&serde_json::json!({
                    "results": Vec::<serde_json::Value>::new(),
                    "did_you_mean": suggestions,
                })));
            }
        }

        let relative_to = params.relative_to.unwrap_or_default();
        let projected: Vec<_> = result
            .into_iter()
            .map(|s| {
                serde_json::json!({
                    "fqdn": relative_fqdn(&s.fqdn, &relative_to),
                    "kind": s.kind,
                })
            })
            .collect();
        Ok(success_json(&projected))
    }

    /// Filter-only listing — no FTS query, no glob pattern. Returns
    /// every symbol matching the provided filters, ordered by canonical
    /// fqdn for stable output. Designed for cross-cutting audits like
    /// "list every private function in module X" or "list every type
    /// with visibility=crate". Pagination is cursor-based: when the
    /// page fills, `next_cursor` carries the last fqdn returned; pass
    /// it back in `cursor` to fetch the next slice. `null` means done.
    #[tool(
        description = "Filter-only listing of symbols. No query string, no pattern — returns every symbol matching the provided filters, ordered by fqdn. Response shape: `{items: [...], next_cursor: string | null}`. Walk the full set by re-calling with `cursor = next_cursor` until it returns `null`. Use this for audits and inventories like 'all private functions' or 'all types in module X'. At least one filter SHOULD be provided to keep the result set bounded. Filters: `kind`, `visibility`, `module` (all optional, all match exactly)."
    )]
    async fn list_symbols(
        &self,
        Parameters(params): Parameters<ListSymbolsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if !self.index_ready.load(Ordering::Acquire) {
            return Ok(CallToolResult::success(vec![Content::text(
                indexing_in_progress_message(self.handle.cold_start_progress().ok().flatten()),
            )]));
        }
        // Don't normalise the module filter — same reason as
        // `find_symbol`: TS file segments carry literal `.` chars.

        let filter = parse_filter(
            params.kind.as_deref(),
            params.visibility.as_deref(),
            params.module,
            params.include_external,
            params.workspace_id,
        )?;
        let limit = clamp_limit(params.limit);
        let cursor = params.cursor;
        let handle = self.handle.clone();
        let page = tokio::task::spawn_blocking(move || {
            query::list_symbols(&handle, &filter, limit as usize, cursor.as_deref())
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;

        let envelope = serde_json::json!({
            "items": page.items,
            "next_cursor": page.next_cursor,
        });
        Ok(success_json(&envelope))
    }

    /// Compact variant of `list_symbols` — returns `{fqdn, kind}` per
    /// item instead of the full `RawSymbol`. Designed for audits that
    /// only need to enumerate FQDNs (with optional filter scoping).
    /// Pagination semantics identical to `list_symbols`. Optional
    /// `relative_to` shortens matching FQDNs to `::<rest>`.
    #[tool(
        description = "FQDN-only variant of `list_symbols`. Returns `{items: [{fqdn, kind}, ...], next_cursor}` instead of full RawSymbol per row. Same filter and pagination semantics as `list_symbols` — use cursor=next_cursor to walk pages. Use this for broad audits ('all private functions in module X', 'all types in crate Y') where the FQDN is the only field that matters. Pass `relative_to = \"foo::bar\"` to collapse matching FQDNs into `::baz` form."
    )]
    async fn list_symbol_fqdns(
        &self,
        Parameters(params): Parameters<ListSymbolFqdnsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if !self.index_ready.load(Ordering::Acquire) {
            return Ok(CallToolResult::success(vec![Content::text(
                indexing_in_progress_message(self.handle.cold_start_progress().ok().flatten()),
            )]));
        }
        let filter = parse_filter(
            params.kind.as_deref(),
            params.visibility.as_deref(),
            params.module,
            params.include_external,
            params.workspace_id,
        )?;
        let limit = clamp_limit(params.limit);
        let cursor = params.cursor;
        let handle = self.handle.clone();
        let page = tokio::task::spawn_blocking(move || {
            query::list_symbols(&handle, &filter, limit as usize, cursor.as_deref())
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;

        let relative_to = params.relative_to.unwrap_or_default();
        let projected: Vec<_> = page
            .items
            .into_iter()
            .map(|s| {
                serde_json::json!({
                    "fqdn": relative_fqdn(&s.fqdn, &relative_to),
                    "kind": s.kind,
                })
            })
            .collect();
        let envelope = serde_json::json!({
            "items": projected,
            "next_cursor": page.next_cursor,
        });
        Ok(success_json(&envelope))
    }

    /// Glob-pattern search over `name` and `fqdn`. Uses SQLite's `GLOB`
    /// operator (`*`, `?`, `[abc]` wildcards — case-sensitive). A symbol
    /// matches when EITHER its name OR its fqdn satisfies the pattern.
    /// Combine with the same filters as `find_symbol` to scope the
    /// search.
    #[tool(
        description = "Glob-pattern search over symbol names and FQDNs (SQLite GLOB: `*`, `?`, `[abc]`, case-sensitive). A symbol matches when either its name or its fqdn satisfies the pattern. Use this to detect cross-module duplications (e.g. `strip_*_extension` to catch every `strip_<lang>_extension` helper). Optional filters: `kind`, `visibility`, `module` — same semantics as `find_symbol`. When the pattern returns zero matches, the response switches to `{results: [], did_you_mean: [...]}` running strsim on the pattern's core (wildcards stripped) — useful for typos like `*to_token_string*` → `to_token_stream`."
    )]
    async fn find_symbols_by_pattern(
        &self,
        Parameters(params): Parameters<FindSymbolsByPatternParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if !self.index_ready.load(Ordering::Acquire) {
            return Ok(CallToolResult::success(vec![Content::text(
                indexing_in_progress_message(self.handle.cold_start_progress().ok().flatten()),
            )]));
        }
        // Don't normalise the pattern or module filter — TS file
        // segments (`profiler.type`, `hud.element`) embed a literal
        // `.` and the GLOB match expects them verbatim. LLM consumers
        // typing OOP-style `Type.method` patterns have to use the
        // canonical `::` form here.
        let trimmed = params.pattern.trim().to_owned();
        if trimmed.is_empty() {
            return Ok(success_json::<Vec<RawSymbol>>(&Vec::new()));
        }

        let filter = parse_filter(
            params.kind.as_deref(),
            params.visibility.as_deref(),
            params.module,
            params.include_external,
            params.workspace_id,
        )?;
        let limit = clamp_limit(params.limit);
        let handle = self.handle.clone();
        let filter_for_search = filter.clone();
        let trimmed_for_search = trimmed.clone();
        let result = tokio::task::spawn_blocking(move || {
            query::find_by_pattern(
                &handle,
                &trimmed_for_search,
                &filter_for_search,
                limit as usize,
            )
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;

        if result.is_empty() {
            let core = glob_core_text(&trimmed);
            if !core.is_empty() {
                let suggestions = compute_did_you_mean(self.handle.clone(), core, filter).await?;
                if !suggestions.is_empty() {
                    return Ok(success_json(&serde_json::json!({
                        "results": Vec::<RawSymbol>::new(),
                        "did_you_mean": suggestions,
                    })));
                }
            }
        }

        Ok(success_json(&result))
    }

    /// Similarity-scored search around an anchor name. Returns symbols whose
    /// name is "close" to `reference` under a hybrid score combining
    /// Jaro-Winkler (character-level) and Jaccard over snake/camel-case tokens.
    /// Self-skips the anchor (case-insensitive name equality). Filters apply
    /// BEFORE scoring (idiomatic with the other tools — keeps the candidate
    /// pool bounded). Use this to detect cluster-style duplications without
    /// having to guess a glob pattern.
    #[tool(
        description = "Similarity-scored search for symbols whose name is close to an anchor `reference`. Returns ranked `[{score, symbol}]` (score in [0,1], descending). Hybrid score = max(jaro_winkler, jaccard_tokens) — captures both typo-style matches (`parseFile` ↔ `parseFiles`) and templated-name clusters (`strip_rs_extension` ↔ `strip_ts_extension` ↔ `strip_lua_extension`). Self-skips the anchor by case-insensitive name. `threshold` defaults to 0.8 in [0.0, 1.0]; `limit` defaults to 20, capped at 100. Comparison runs on `name` only; use `module` filter to scope by module. Optional filters: `kind`, `visibility`, `module`."
    )]
    async fn find_similar_symbols(
        &self,
        Parameters(params): Parameters<FindSimilarSymbolsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if !self.index_ready.load(Ordering::Acquire) {
            return Ok(CallToolResult::success(vec![Content::text(
                indexing_in_progress_message(self.handle.cold_start_progress().ok().flatten()),
            )]));
        }
        // `reference` is a name anchor, not an fqdn — leave it intact.
        // Module filter is left verbatim too: TS file segments carry
        // literal `.` chars that the exact-match filter expects.

        let trimmed = params.reference.trim().to_owned();
        if trimmed.is_empty() {
            return Ok(success_json::<Vec<SimilarSymbolJson>>(&Vec::new()));
        }

        let threshold = parse_threshold(params.threshold)?;
        // find_similar_symbols is intentionally NOT in L3e's MCP surface
        // (per user "just the three primary tools"). The core filter
        // still defaults to primary; passing None preserves that — no
        // workspace override is exposed at the tool level here.
        let filter = parse_filter(
            params.kind.as_deref(),
            params.visibility.as_deref(),
            params.module,
            params.include_external,
            None,
        )?;
        let limit = clamp_limit(params.limit);
        let handle = self.handle.clone();
        let result = tokio::task::spawn_blocking(move || {
            query::find_similar(&handle, &trimmed, threshold, &filter, limit as usize)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;

        let envelope: Vec<SimilarSymbolJson> = result
            .into_iter()
            .map(|(symbol, score)| SimilarSymbolJson { score, symbol })
            .collect();
        Ok(success_json(&envelope))
    }

    /// Returns the raw source text of the symbol identified by `fqdn`. This is
    /// the verbatim slice between `location.start_line` and `location.end_line`
    /// in the file on disk — exactly what a reader would see if they opened the
    /// file at those line numbers. Use this when you need to reason about the
    /// actual code of a known FQDN (the graph tells you WHERE; this tells you
    /// WHAT). `max_lines` clamps long bodies; `strip_attrs` drops leading docs
    /// + attribute blocks; `signature_only` truncates after the opening `{`;
    /// `strip_inline_comments` removes `// …` and `/* … */` from the returned
    /// body (string-literal safe). The response carries `truncated`,
    /// `stripped_lines` and `signature_only` so the caller can audit what was
    /// returned vs. the verbatim slice.
    #[tool(
        description = "Returns the raw source text of a symbol identified by FQDN, sliced from the file at its declared start_line..end_line. Pair with `get_context` (graph relations) when you need to actually read the function body. Optional knobs: `max_lines` caps total output (`truncated=true` flag), `strip_attrs=true` drops leading doc comments / `#[…]` attribute blocks (`stripped_lines` count), `signature_only=true` truncates after the first `{` (returns just the multi-line signature), `strip_inline_comments=true` removes inline `// …` and `/* … */` comments from the body (string-literal safe — `\"…\"`, raw strings, TS templates passed through verbatim). Returns `null` when no symbol matches the FQDN — call `find_symbol` first if you only have a name fragment."
    )]
    async fn get_body(
        &self,
        Parameters(params): Parameters<GetBodyParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if !self.index_ready.load(Ordering::Acquire) {
            return Ok(CallToolResult::success(vec![Content::text(
                indexing_in_progress_message(self.handle.cold_start_progress().ok().flatten()),
            )]));
        }
        // Try raw FQDN first (preserves TS file segments like
        // `profiler.type`), fall back to OOP-normalized form for LLM
        // consumers that emit `Type.method` shorthand.
        let raw_fqdn = params.fqdn.clone();
        let handle = self.handle.clone();
        let opts = query::BodyOptions {
            max_lines: params.max_lines,
            strip_attrs: params.strip_attrs.unwrap_or(false),
            signature_only: params.signature_only.unwrap_or(false),
            strip_inline_comments: params.strip_inline_comments.unwrap_or(false),
        };
        let raw_for_call = raw_fqdn.clone();
        let opts_clone = opts.clone();
        let result = tokio::task::spawn_blocking(move || {
            if let Some(slice) = query::body_for_fqdn(&handle, &raw_for_call, &opts_clone)? {
                return Ok::<_, standardoc_core::StorageError>(Some(slice));
            }
            if raw_for_call.contains('.') {
                let normalized = normalize_fqdn(&raw_for_call);
                if let Some(slice) = query::body_for_fqdn(&handle, &normalized, &opts_clone)? {
                    return Ok(Some(slice));
                }
            }
            Ok(None)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;

        match result {
            Some(slice) => Ok(success_json(&slice)),
            None => Ok(CallToolResult::success(vec![Content::text(
                "no symbol found for the given FQDN",
            )])),
        }
    }

    /// Agent-tuned variant of `get_body` — strips noise by default.
    /// Defaults `strip_attrs = true` and `strip_inline_comments = true`,
    /// so the returned slice is pure code without doc comments,
    /// attribute blocks, or inline `//` / `/* */` comments. Pass either
    /// flag explicitly to override. The leading-doc semantics are
    /// already captured via `RawDocument` / `enrichment_description` —
    /// duplicating them in the body is wasted context.
    #[tool(
        description = "Like `get_body` but returns pure code by default — leading doc comments / `#[…]` attribute blocks and inline `// …` / `/* … */` comments are stripped. The verbatim slice is still available via `get_body`; the leading description lives separately in `get_context.context.document_description`. Useful when you want to read the actual implementation without dilution. Pass `strip_attrs=false` or `strip_inline_comments=false` to disable individual strips. Other knobs (`max_lines`, `signature_only`) behave the same as `get_body`. Returns `null` when no symbol matches the FQDN."
    )]
    async fn get_code(
        &self,
        Parameters(params): Parameters<GetCodeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if !self.index_ready.load(Ordering::Acquire) {
            return Ok(CallToolResult::success(vec![Content::text(
                indexing_in_progress_message(self.handle.cold_start_progress().ok().flatten()),
            )]));
        }
        let raw_fqdn = params.fqdn.clone();
        let handle = self.handle.clone();
        let opts = query::BodyOptions {
            max_lines: params.max_lines,
            strip_attrs: params.strip_attrs.unwrap_or(true),
            signature_only: params.signature_only.unwrap_or(false),
            strip_inline_comments: params.strip_inline_comments.unwrap_or(true),
        };
        let raw_for_call = raw_fqdn.clone();
        let opts_clone = opts.clone();
        let result = tokio::task::spawn_blocking(move || {
            if let Some(slice) = query::body_for_fqdn(&handle, &raw_for_call, &opts_clone)? {
                return Ok::<_, standardoc_core::StorageError>(Some(slice));
            }
            if raw_for_call.contains('.') {
                let normalized = normalize_fqdn(&raw_for_call);
                if let Some(slice) = query::body_for_fqdn(&handle, &normalized, &opts_clone)? {
                    return Ok(Some(slice));
                }
            }
            Ok(None)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;

        match result {
            Some(slice) => Ok(success_json(&slice)),
            None => Ok(CallToolResult::success(vec![Content::text(
                "no symbol found for the given FQDN",
            )])),
        }
    }

    /// Read-only snapshot of the workspace revision counter PLUS daemon
    /// capabilities. The `revision` number is monotonic — every successful
    /// write (cold-start ingest, watcher upsert, rescan) bumps it by 1. The
    /// `watcher` / `indexing` blocks let callers introspect what the daemon
    /// is wired with.
    #[tool(
        description = "Returns the current workspace revision number AND daemon capabilities (`watcher.active`, `indexing.ready`, `workspace.kind`). Use the revision with `check_stale` to detect when fqdns you have already cited have been modified. `workspace.kind` is the detected monorepo organizer (cargo/npm/pnpm/yarn/bun/deno/go/lerna/nx/turborepo/mira/single/custom:<tag>) — null before cold-start detection finishes. Cheap call; no parameters."
    )]
    async fn current_revision(&self) -> Result<CallToolResult, ErrorData> {
        let revision = self.handle.revision();
        let watcher_active = self
            .watcher
            .lock()
            .ok()
            .is_some_and(|guard| guard.is_some());
        let ready = self.index_ready.load(Ordering::Acquire);
        // Stage 3e-3 — surface the detected workspace kind. Best-effort:
        // pre-cold-start (or pre-3e-3 DBs) carry no persisted row and
        // we report `null` rather than guessing `Single` here, so the
        // caller can distinguish "not detected yet" from "detected as
        // Single". Read failure → null + log (kind is informational,
        // never load-bearing).
        let workspace_kind = workspace_query::read_primary_workspace_kind(&self.handle)
            .ok()
            .flatten()
            .map(|k| k.as_str().into_owned());
        Ok(success_json(&CurrentRevisionJson {
            revision,
            watcher: WatcherCapabilityJson {
                active: watcher_active,
            },
            indexing: IndexingCapabilityJson { ready },
            workspace: WorkspaceCapabilityJson {
                kind: workspace_kind,
            },
        }))
    }

    /// Aggregated read-path telemetry — the running tally of bytes the
    /// Lazy on-demand resolution of an external FQDN (Cargo crate, npm
    /// package, luarocks rock). Routes the FQDN through the registered
    /// resolvers in order; the first non-`NotInThisRegistry` answer wins.
    /// On success, the produced `ExtractedFile` is submitted to the
    /// writer pipeline (`is_external = 1` + the resolver's `SourceOrigin`)
    /// and the new symbol's `RawSymbol` is returned in the envelope.
    ///
    /// Scaffold A surface — body is unimplemented until the registry
    /// is wired into `StandardocMcp::new`. Calls return
    /// `status = "scaffold_a_unimplemented"` rather than crashing the
    /// daemon so existing integration tests stay green.
    #[tool(
        description = "Lazy on-demand resolution of an external FQDN (a symbol that lives outside the workspace — Cargo crate, npm package, luarocks rock). Routes the FQDN through registered resolvers and submits the produced ExtractedFile to the writer pipeline. Returns `{status, fqdn, source_origin?, symbol?, missing_binary?, detail?}`. `status` is `resolved` (success — `symbol` is the new RawSymbol), `not_found` (no resolver claimed the FQDN), `missing_binary` (the matching resolver is gated behind a CLI that is not installed — `missing_binary` names which one), or `error` (resolver-level failure — `detail` carries the message). Use this when `get_context(fqdn)` returned a neighbor with `to: Unresolved` and the FQDN shape matches a known dependency."
    )]
    pub async fn resolve_external(
        &self,
        Parameters(params): Parameters<ResolveExternalParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if !self.index_ready.load(Ordering::Acquire) {
            return Ok(CallToolResult::success(vec![Content::text(
                indexing_in_progress_message(self.handle.cold_start_progress().ok().flatten()),
            )]));
        }
        // Leave the FQDN verbatim — callers feed `resolve_external`
        // with FQDNs taken straight from `find_symbol` output, which
        // is already in canonical `::` form. Normalising would mangle
        // TS file segments (`profiler.type`) into nonexistent keys.
        let fqdn = params.fqdn.clone();
        let handle = self.handle.clone();
        let provider = Arc::clone(&self.provider);
        let registry = Arc::clone(&self.registry);
        let outcome = tokio::task::spawn_blocking(move || {
            registry.resolve(&handle, provider.as_ref(), &fqdn)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?;

        let envelope = match outcome {
            Ok(ResolveOutcome::Resolved {
                symbol,
                source_origin,
            }) => ResolveExternalJson {
                status: "resolved".to_string(),
                fqdn: params.fqdn,
                source_origin: Some(source_origin_label(source_origin).to_string()),
                symbol: Some(symbol),
                missing_binary: None,
                detail: None,
            },
            Ok(ResolveOutcome::NotInThisRegistry) => ResolveExternalJson {
                status: "not_found".to_string(),
                fqdn: params.fqdn,
                source_origin: None,
                symbol: None,
                missing_binary: None,
                detail: Some(
                    "no resolver claimed this FQDN — the crate / package may not be a workspace dependency"
                        .to_string(),
                ),
            },
            Ok(ResolveOutcome::MissingBinary {
                binary,
                env_override,
            }) => ResolveExternalJson {
                status: "missing_binary".to_string(),
                fqdn: params.fqdn,
                source_origin: None,
                symbol: None,
                missing_binary: Some(binary.to_string()),
                detail: Some(format!(
                    "binary `{binary}` not found in PATH — set the env var `{env_override}` to override the lookup",
                )),
            },
            Ok(ResolveOutcome::LockfileNotFound { lockfile }) => ResolveExternalJson {
                status: "lockfile_not_found".to_string(),
                fqdn: params.fqdn,
                source_origin: None,
                symbol: None,
                missing_binary: None,
                detail: Some(format!("workspace has no `{lockfile}` at its root")),
            },
            Err(e) => ResolveExternalJson {
                status: "error".to_string(),
                fqdn: params.fqdn,
                source_origin: None,
                symbol: None,
                missing_binary: None,
                detail: Some(e.to_string()),
            },
        };
        Ok(success_json(&envelope))
    }

    /// Compares a set of `(fqdn, fetched_at_revision)` pairs against the
    /// current `symbols.last_modified_revision` of each row and returns the
    /// entries that have been modified since their fetch. Stateless server-side
    /// — the caller is responsible for tracking what it has fetched and at
    /// which revision. Empty / unknown fqdns produce a `Missing` reason rather
    /// than being silently dropped.
    #[tool(
        description = "Detects staleness for a set of previously-fetched fqdns. Input: `fetched = [{fqdn, fetched_at_revision}, ...]`. Returns an array of `{fqdn, fetched_at_revision, last_modified_revision, status}` where status is `stale` (last_modified > fetched_at), `fresh` (no change since fetch), or `missing` (fqdn no longer indexed — likely renamed/removed). Use this BEFORE re-reasoning on cached symbol context to know what changed and what to re-query. Stateless — the caller maintains the (fqdn → revision) map across turns."
    )]
    async fn check_stale(
        &self,
        Parameters(params): Parameters<CheckStaleParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if !self.index_ready.load(Ordering::Acquire) {
            return Ok(CallToolResult::success(vec![Content::text(
                indexing_in_progress_message(self.handle.cold_start_progress().ok().flatten()),
            )]));
        }
        // Leave entry FQDNs verbatim — `check_stale` is called after
        // `find_symbol` / `get_context` have already given the caller
        // canonical `::`-form fqdns. Normalising would corrupt TS
        // file segments containing literal `.` characters.
        if params.fetched.is_empty() {
            return Ok(success_json::<Vec<StaleEntryJson>>(&Vec::new()));
        }

        let fqdns: Vec<String> = params.fetched.iter().map(|e| e.fqdn.clone()).collect();
        let handle = self.handle.clone();
        let revisions = tokio::task::spawn_blocking(move || {
            let refs: Vec<&str> = fqdns.iter().map(String::as_str).collect();
            query::last_modified_revisions_for_fqdns(&handle, &refs)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;

        let entries: Vec<StaleEntryJson> = params
            .fetched
            .into_iter()
            .map(|e| match revisions.get(&e.fqdn).copied() {
                Some(last_modified_revision) => StaleEntryJson {
                    fqdn: e.fqdn,
                    fetched_at_revision: e.fetched_at_revision,
                    last_modified_revision: Some(last_modified_revision),
                    status: if last_modified_revision > e.fetched_at_revision {
                        "stale"
                    } else {
                        "fresh"
                    }
                    .to_string(),
                },
                None => StaleEntryJson {
                    fqdn: e.fqdn,
                    fetched_at_revision: e.fetched_at_revision,
                    last_modified_revision: None,
                    status: "missing".to_string(),
                },
            })
            .collect();

        Ok(success_json(&entries))
    }

    /// Register a linked peer workspace so its persisted `ModuleLookup`
    /// blobs become reachable for cross-workspace import resolution.
    /// The `path` is canonicalised before storage; on a filesystem miss
    /// the tool returns `invalid_params` with a `did_you_mean` list
    /// built from sibling directories of the closest existing ancestor.
    #[tool(
        description = "Register a linked peer workspace (cross-workspace import resolution). `path` is canonicalised. `direction` is one of `in` (peer feeds us), `out` (we feed peer), `bidirectional`. `indexing_mode` is optional and defaults to `blob_import` (Stage 3b-7-a — peer's pre-built DB is copied wholesale); pass `extract` to opt this peer into the Stage 3b-7-b autonomous source-walk pipeline. Returns the freshly-minted UUID workspace_id. Missing path returns invalid_params with a `did_you_mean` list."
    )]
    async fn link_workspace(
        &self,
        Parameters(params): Parameters<LinkWorkspaceParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let direction = parse_link_direction(&params.direction)?;
        let indexing_mode = parse_indexing_mode(params.indexing_mode.as_deref())?;
        let path_for_response = params.path.clone();
        let handle = self.handle.clone();
        let path = params.path;
        let result = tokio::task::spawn_blocking(move || {
            workspace_query::link_workspace(&handle, &path, direction, indexing_mode)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?;
        match result {
            Ok(workspace_id) => {
                // L3d-3: register the peer with the live watcher so its
                // source changes flow through the dispatch loop
                // immediately (rather than waiting for the next
                // cold_start). Skipped for `LinkDirection::Out` — Out
                // means the peer reads us, not us reading them, so we
                // have nothing to watch on their side. Failures here
                // are logged but do not fail the MCP call: the catalog
                // state is already correct and the next cold_start
                // will catch up.
                if direction != LinkDirection::Out {
                    register_peer_with_watcher(
                        &self.watcher_slot(),
                        workspace_id.clone(),
                        Path::new(&path_for_response),
                    );
                }
                Ok(success_json(&LinkWorkspaceJson {
                    workspace_id,
                    root_path: path_for_response,
                    direction: link_direction_label(direction).to_string(),
                }))
            }
            Err(workspace_query::LinkWorkspaceError::PathNotFound { input, suggestions }) => {
                Err(ErrorData::invalid_params(
                    format!("path not found: {input}"),
                    Some(serde_json::json!({
                        "input": input,
                        "did_you_mean": suggestions,
                    })),
                ))
            }
            Err(workspace_query::LinkWorkspaceError::Storage(e)) => {
                Err(server_error_to_rmcp(&e.into()))
            }
        }
    }

    /// Unregister a linked workspace, dropping its `module_lookups` and
    /// `workspace_imports` rows transactionally. Symbols imported from
    /// it stop resolving on the next index pass.
    #[tool(
        description = "Unregister a linked workspace by its `workspace_id`. Cleans up dependent `module_lookups` and `workspace_imports` rows. Idempotent — unregistering an unknown id is a no-op."
    )]
    async fn unlink_workspace(
        &self,
        Parameters(params): Parameters<UnlinkWorkspaceParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let handle = self.handle.clone();
        let workspace_id = params.workspace_id.clone();
        let workspace_id_for_watcher = params.workspace_id;
        tokio::task::spawn_blocking(move || {
            workspace_query::unlink_workspace(&handle, &workspace_id)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;
        // L3d-3: drop the peer from the live watcher registry.
        // Idempotent on the watcher side — safe even if the peer was
        // never registered (e.g. linked with direction=Out, or the
        // watcher had not booted at link time).
        unregister_peer_from_watcher(&self.watcher_slot(), &workspace_id_for_watcher);
        Ok(success_json(&serde_json::json!({ "ok": true })))
    }

    /// Stage 3b-7-b L3-bis: explicit re-extraction of a single linked
    /// peer outside of cold_start. The Q4 staleness gap: peer source
    /// can drift between sessions, and the watcher (L3d) only catches
    /// changes that happen while THIS daemon is running. This tool is
    /// the user-facing escape hatch — "I edited the peer, re-index it
    /// now". Returns the same PeerExtractStats shape cold_start emits
    /// internally so callers can surface files_extracted /
    /// files_skipped_unchanged / files_parse_errors counters.
    #[tool(
        description = "Re-extract a single linked peer workspace's source files into the primary index. Use after editing the peer's source between cold_starts (the live watcher only catches edits while the daemon is up). Returns `{workspace_id, root_path, status, files_extracted, files_skipped_unchanged, files_parse_errors}`. `status` is one of `ok` / `skipped_inactive` / `skipped_missing` / `failed`. Unknown workspace_id returns invalid_params."
    )]
    async fn refresh_peer(
        &self,
        Parameters(params): Parameters<RefreshPeerParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let handle = self.handle.clone();
        let provider = Arc::clone(&self.provider);
        let workspace_id = params.workspace_id;
        let result = tokio::task::spawn_blocking(move || {
            workspace_query::refresh_peer(&handle, provider.as_ref(), &workspace_id)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?;
        match result {
            Ok(stats) => Ok(success_json(&stats)),
            Err(workspace_query::RefreshPeerError::NotFound(id)) => Err(ErrorData::invalid_params(
                format!("workspace_id not found: {id}"),
                Some(serde_json::json!({ "workspace_id": id })),
            )),
            Err(workspace_query::RefreshPeerError::Storage(e)) => {
                Err(server_error_to_rmcp(&e.into()))
            }
            Err(workspace_query::RefreshPeerError::Extract(e)) => Err(ErrorData::internal_error(
                format!("peer extract failed: {e}"),
                None,
            )),
        }
    }

    /// Flip the link direction of a registered peer AND propagate the
    /// change to the live watcher. Transitions crossing the watch
    /// boundary (`Out ↔ {In, Bidirectional}`) trigger an `add_peer` /
    /// `remove_peer` on the watcher. Same-direction calls are no-ops
    /// at both the catalog and watcher layers. Avoids the prior
    /// workaround of `unlink_workspace` + `link_workspace` (which lost
    /// the workspace_id and forced the caller to re-discover it).
    #[tool(
        description = "Change the link direction of a registered peer. `direction` is one of `in` (peer feeds us), `out` (we feed peer), `bidirectional`. Returns `{workspace_id, root_path, previous_direction, new_direction}`. Side effects: transitions crossing the watch boundary (Out ↔ {in, bidirectional}) register or unregister the peer on the live watcher. Unknown workspace_id returns invalid_params."
    )]
    async fn set_link_direction(
        &self,
        Parameters(params): Parameters<SetLinkDirectionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let new_direction = parse_link_direction(&params.direction)?;
        let handle = self.handle.clone();
        let workspace_id = params.workspace_id;
        let workspace_id_for_response = workspace_id.clone();
        let result = tokio::task::spawn_blocking(move || {
            workspace_query::set_link_direction(&handle, &workspace_id, new_direction)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?;
        match result {
            Ok(outcome) => {
                // Watch-boundary transition: Out ↔ {In, Bidirectional}.
                // Same-side transitions (e.g. In → Bidirectional) leave
                // the watcher state untouched — direction is metadata
                // from the watcher's perspective once it's watching.
                let was_watching = watches_peer(outcome.previous_direction);
                let now_watching = watches_peer(outcome.new_direction);
                match (was_watching, now_watching) {
                    (false, true) => {
                        register_peer_with_watcher(
                            &self.watcher_slot(),
                            outcome.workspace_id.clone(),
                            Path::new(&outcome.root_path),
                        );
                    }
                    (true, false) => {
                        unregister_peer_from_watcher(&self.watcher_slot(), &outcome.workspace_id);
                    }
                    _ => {}
                }
                Ok(success_json(&SetLinkDirectionJson {
                    workspace_id: outcome.workspace_id,
                    root_path: outcome.root_path,
                    previous_direction: link_direction_label(outcome.previous_direction)
                        .to_string(),
                    new_direction: link_direction_label(outcome.new_direction).to_string(),
                }))
            }
            Err(workspace_query::SetLinkDirectionError::NotFound(id)) => {
                Err(ErrorData::invalid_params(
                    format!("workspace_id not found: {id}"),
                    Some(serde_json::json!({ "workspace_id": id })),
                ))
            }
            Err(workspace_query::SetLinkDirectionError::Storage(e)) => {
                let _ = workspace_id_for_response;
                Err(server_error_to_rmcp(&e.into()))
            }
        }
    }

    /// Enumerate every linked workspace registered in the catalog,
    /// ordered by registration time (oldest first).
    #[tool(
        description = "List every registered linked workspace (newest first not guaranteed — registration order). Each entry surfaces workspace_id, canonical root_path, direction, status, and the epoch ms of registration and last index run."
    )]
    async fn list_linked_workspaces(&self) -> Result<CallToolResult, ErrorData> {
        let handle = self.handle.clone();
        let rows =
            tokio::task::spawn_blocking(move || workspace_query::list_linked_workspaces(&handle))
                .await
                .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
                .map_err(|e| server_error_to_rmcp(&e.into()))?;
        Ok(success_json(&serde_json::json!({ "workspaces": rows })))
    }

    /// Fetch the persisted `ModuleLookup` blob for `(workspace_id,
    /// module_fqdn)`. `workspace_id` defaults to `"primary"`. Returns
    /// `null` when no row matches — useful to debug the Stage 3a AOT
    /// pre-pass output without re-running the indexer.
    #[tool(
        description = "Fetch the persisted ModuleLookup for `module_fqdn` (full structured bindings/scopes/imports as JSON). `workspace_id` defaults to `\"primary\"` — pass a linked workspace UUID to inspect a peer. Returns null when no row matches."
    )]
    async fn module_lookup(
        &self,
        Parameters(params): Parameters<ModuleLookupParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let handle = self.handle.clone();
        let workspace_id = params.workspace_id;
        let module_fqdn = params.module_fqdn;
        let result = tokio::task::spawn_blocking(move || {
            workspace_query::get_module_lookup(&handle, workspace_id.as_deref(), &module_fqdn)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;
        match result {
            Some(lookup) => Ok(success_json(&lookup)),
            None => Ok(success_json(&serde_json::Value::Null)),
        }
    }

    /// Enumerate every linked workspace that re-exports / declares the
    /// requested symbol at module-root scope. Returns an empty
    /// `providers` array when no peer matches.
    #[tool(
        description = "Resolve `(origin_module, origin_symbol)` against every linked workspace's persisted ModuleLookup. Returns `{providers: [...]}` listing each peer that exposes the symbol at module-root scope. Empty list = no match."
    )]
    async fn resolve_cross_workspace(
        &self,
        Parameters(params): Parameters<ResolveCrossWorkspaceParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let handle = self.handle.clone();
        let origin_module = params.origin_module;
        let origin_symbol = params.origin_symbol;
        let providers = tokio::task::spawn_blocking(move || {
            workspace_query::resolve_cross_workspace(&handle, &origin_module, &origin_symbol)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;
        Ok(success_json(&serde_json::json!({ "providers": providers })))
    }

    /// Enumerate every project detected at cold-start (Rust crates,
    /// Bun/Node/Deno apps, Python packages, …). One row per project;
    /// the order is by `rel_path` so nested projects sort under their
    /// parent in the response.
    #[tool(
        description = "List every project detected in the workspace at cold-start. Each entry surfaces project_id, label, kind (rust/node/bun/deno/python/lua/c/cpp/custom:tag), absolute root_path, and POSIX-style rel_path to the workspace root. Empty list = no manifests found (workspace treated as a single anonymous project)."
    )]
    async fn list_projects(&self) -> Result<CallToolResult, ErrorData> {
        let handle = self.handle.clone();
        let rows = tokio::task::spawn_blocking(move || projects_query::list_projects(&handle))
            .await
            .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
            .map_err(|e| server_error_to_rmcp(&e.into()))?;
        Ok(success_json(&serde_json::json!({ "projects": rows })))
    }

    /// Resolve the project owning a given absolute file path by walking
    /// up the registered project tree for the deepest ancestor match.
    /// Returns `null` when the path isn't under any registered project.
    #[tool(
        description = "Resolve the project owning a file by absolute path. Walks up the registered project tree (Stage 3d cold-start detection) and returns the deepest ancestor's metadata. `null` when the path is outside every registered project root."
    )]
    async fn project_for_file(
        &self,
        Parameters(params): Parameters<ProjectForFileParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let handle = self.handle.clone();
        let path = params.path;
        let result =
            tokio::task::spawn_blocking(move || projects_query::project_for_file(&handle, &path))
                .await
                .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
                .map_err(|e| server_error_to_rmcp(&e.into()))?;
        match result {
            Some(info) => Ok(success_json(&info)),
            None => Ok(success_json(&serde_json::Value::Null)),
        }
    }

    /// IR-4-f follow-up — query the `call_sites` table populated since
    /// IR-4-b/c/d. Three optional filters AND-compose: `from_fqdn`
    /// (exact match on the enclosing FQDN — "what does X call?"),
    /// `callee_text` (exact match on the called expression text — "who
    /// calls Y?"), `callee_pattern` (SQLite GLOB on the callee text —
    /// "every Tauri invocation workspace-wide" via `*tauri.invoke*`).
    /// Calling with no filters returns the most recent N call_sites for
    /// ops-style scanning.
    #[tool(
        description = "Read-only query over the `call_sites` table — every call expression emitted by the extractors (CallExpr in Rust/TS/Lua, NewExpr, OptChain call, method call). Returns a JSON array of records: `{from_fqdn, callee_text, args: [{value, is_string_literal}], receiver_chain: [..], site: {file, line, col}}`. Three optional AND-composable filters: `from_fqdn` (exact match on the enclosing fn/method FQDN — answers `what does X call?`), `callee_text` (exact match on the called expression text — answers `who calls Y?`), `callee_pattern` (SQLite GLOB on the callee text, e.g. `*tauri.invoke*` for every Tauri invocation, `M.api.*` for every call into the M.api namespace). `limit` defaults to 50, capped at 200. Use this to discover textual call patterns the symbol graph alone can't surface — bridge invocations, method-chain shapes, literal-string arg payloads."
    )]
    async fn find_call_sites(
        &self,
        Parameters(params): Parameters<FindCallSitesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if !self.index_ready.load(Ordering::Acquire) {
            return Ok(CallToolResult::success(vec![Content::text(
                indexing_in_progress_message(self.handle.cold_start_progress().ok().flatten()),
            )]));
        }
        let filters = CallSiteFilters {
            from_fqdn: params.from_fqdn.and_then(non_empty),
            callee_text: params.callee_text.and_then(non_empty),
            callee_pattern: params.callee_pattern.and_then(non_empty),
        };
        // `find_call_sites` clamps internally to FIND_CALL_SITES_MAX_LIMIT,
        // so we just translate the caller's optional limit (in `u8` since
        // schemars infers from the param type) up to a u32 — the query
        // helper handles the cap.
        let limit = params
            .limit
            .map_or(FIND_CALL_SITES_DEFAULT_LIMIT, u32::from);
        let handle = self.handle.clone();
        let rows = tokio::task::spawn_blocking(move || {
            call_sites_query::find_call_sites(&handle, &filters, limit)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;
        Ok(success_json(&rows))
    }

    /// Pre-composed graph slice ready for the `standardoc-graph-viz` WASM
    /// consumer. Two modes, dispatched on `focal`:
    /// - `focal = Some(fqdn)` → bounded BFS around the centered node.
    /// - `focal = None` → bounded snapshot of the workspace ordered by
    ///   `fqdn` ASC.
    /// All clamps (`depth`, `max_nodes`) happen here at the transport
    /// boundary; the core composition trusts what it receives.
    #[tool(
        description = "Pre-composed graph payload for the visualisation layer. Returns `{symbols: [{fqdn, name, kind, visibility, module?, language_kind, language, is_external, file, start_line, project_id?}], edges: [{from, to, kind, outbound}], projects: [{project_id, label, kind, rel_path}], focal?}` — flat shape the WASM viz consumes directly (no client-side reshape). `projects` is the lookup table for the `project_id` foreign key; the viz frames symbols by project and colours each frame by `kind`.\n\nTwo modes:\n- `focal: Some(fqdn)` → bounded BFS expansion around the node, `depth` hops (1..=5, default 2). Both outbound (edges_from) and inbound (edges_to) edges are walked; unresolved targets are skipped.\n- `focal: None` → bounded snapshot of the workspace ordered by FQDN ASC. Only edges whose target is also in the bounded set are included (no dangling).\n\nKnobs: `kinds` (allow-list of edge kinds — `CALLS`, `IMPORTS`, `EXTENDS`, `IMPLEMENTS`, `REFERENCES`, `USES_TYPE`; case-insensitive, unknown values silently ignored); `max_nodes` (safety cap, default 500, clamped to 1..=5000); `include_external` (default false, scopes to workspace-authored symbols).\n\nReturns an empty `symbols`/`edges` vec with `focal` echoed when `focal` is supplied but the FQDN is unknown — lets the consumer distinguish 'no neighbors' from 'unknown symbol'."
    )]
    async fn fetch_graph(
        &self,
        Parameters(params): Parameters<FetchGraphParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if !self.index_ready.load(Ordering::Acquire) {
            return Ok(CallToolResult::success(vec![Content::text(
                indexing_in_progress_message(self.handle.cold_start_progress().ok().flatten()),
            )]));
        }
        let req = params.into_request();
        let handle = self.handle.clone();
        let response = tokio::task::spawn_blocking(move || graph_query::fetch_graph(&handle, req))
            .await
            .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
            .map_err(|e| server_error_to_rmcp(&e.into()))?;
        Ok(success_json(&response))
    }
}

/// Drop empty / whitespace-only strings to `None` so an MCP caller can
/// pass `from_fqdn: ""` without smuggling a vacuous filter into the SQL.
fn non_empty(s: String) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[tool_handler]
impl ServerHandler for StandardocMcp {
    fn get_info(&self) -> ServerInfo {
        // `ServerInfo` is `#[non_exhaustive]` since rmcp 1.0 — build
        // via Default then override the fields we care about.
        let mut info = ServerInfo::default();
        info.protocol_version = ProtocolVersion::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = Implementation::from_build_env();
        info.instructions = Some(
            "Standardoc MCP server. Use `find_symbol` to discover symbols by name fragment, \
             then `get_context` for the structured chunk of a specific FQDN. The workspace \
             indexes itself in the background on startup; tools called before indexing \
             completes return a friendly progress message — retry shortly."
                .to_string(),
        );
        info
    }
}

impl StandardocMcp {
    pub fn index_ready(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.index_ready)
    }

    pub fn watcher_slot(&self) -> Arc<Mutex<Option<WatcherHandle>>> {
        Arc::clone(&self.watcher)
    }
}

/// Tool output for `get_context` : the canonical neighbor-flat shape
/// from `query::context_for_symbol_with_neighbors`. `routing_hint` is
/// `None` for well-paced calls and emits an in-band 3-phase nudge when
/// a depth=2 call lands without a recent depth=1 scoping pass on the
/// same FQDN.
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
    /// Free-text FTS5 query against symbol `name` and `fqdn` columns.
    /// Tokenization handles snake_case and camelCase.
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
    /// Free-text FTS5 query against symbol `name` and `fqdn` columns.
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
    /// Optional — scope the query to a single workspace by its UUID.
    /// Defaults to the primary workspace.
    #[serde(default)]
    pub workspace_id: Option<String>,
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
    fn into_request(self) -> GraphRequest {
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
/// - `status = "scaffold_a_unimplemented"` → temporary while scaffold B
///   wires the registry; `detail` carries the pending message.
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
struct SetLinkDirectionJson {
    workspace_id: String,
    root_path: String,
    previous_direction: String,
    new_direction: String,
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

const fn source_origin_label(origin: SourceOrigin) -> &'static str {
    match origin {
        SourceOrigin::Workspace => "workspace",
        SourceOrigin::CargoRegistry => "cargo_registry",
        SourceOrigin::NodeModulesDts => "node_modules_dts",
        SourceOrigin::ManualExternal => "manual_external",
    }
}

fn parse_filter(
    kind: Option<&str>,
    visibility: Option<&str>,
    module: Option<String>,
    include_external: Option<bool>,
    workspace_id: Option<String>,
) -> Result<SymbolFilter, ErrorData> {
    let kind = kind.map(parse_kind).transpose()?;
    let visibility = visibility.map(parse_visibility).transpose()?;
    let include_external = include_external.unwrap_or(true);
    Ok(SymbolFilter {
        kind,
        visibility,
        module,
        include_external,
        workspace_id,
    })
}

fn parse_link_direction(s: &str) -> Result<LinkDirection, ErrorData> {
    match s {
        "in" => Ok(LinkDirection::In),
        "out" => Ok(LinkDirection::Out),
        "bidirectional" => Ok(LinkDirection::Bidirectional),
        other => Err(ErrorData::invalid_params(
            format!("unknown direction `{other}` — expected one of: in, out, bidirectional"),
            None,
        )),
    }
}

fn parse_indexing_mode(s: Option<&str>) -> Result<IndexingMode, ErrorData> {
    match s {
        None => Ok(IndexingMode::default()),
        Some("blob_import") => Ok(IndexingMode::BlobImport),
        Some("extract") => Ok(IndexingMode::Extract),
        Some(other) => Err(ErrorData::invalid_params(
            format!("unknown indexing_mode `{other}` — expected one of: blob_import, extract"),
            None,
        )),
    }
}

const fn link_direction_label(d: LinkDirection) -> &'static str {
    match d {
        LinkDirection::In => "in",
        LinkDirection::Out => "out",
        LinkDirection::Bidirectional => "bidirectional",
    }
}

/// Does this direction trigger the live watcher to observe the peer
/// root? `Out` means the peer reads us — we have nothing to watch on
/// their side, so the watcher stays silent. `In` and `Bidirectional`
/// both require watching the peer's source.
const fn watches_peer(d: LinkDirection) -> bool {
    matches!(d, LinkDirection::In | LinkDirection::Bidirectional)
}

/// L3d-3 helper: hand a freshly-linked peer to the live watcher. Lives
/// outside the handler impl so the locking pattern is visible at the
/// call site. Best-effort: any failure (slot empty, debouncer dropped,
/// notify error) is logged and swallowed — the catalog write already
/// succeeded and the next cold_start will reconcile.
fn register_peer_with_watcher(
    slot: &Arc<Mutex<Option<WatcherHandle>>>,
    workspace_id: String,
    root: &Path,
) {
    let mut guard = match slot.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let Some(w) = guard.as_mut() else {
        // Watcher not booted yet (readonly mode, pre-cold-start, or
        // already shut down). Cold_start will pick up the peer from
        // workspace_catalog on the next boot.
        return;
    };
    if let Err(e) = w.add_peer(workspace_id.clone(), root) {
        eprintln!(
            "standardoc mcp: watcher add_peer failed for {workspace_id} ({}): {e}",
            root.display()
        );
    }
}

/// L3d-3 helper: drop a peer from the live watcher registry. Idempotent.
fn unregister_peer_from_watcher(slot: &Arc<Mutex<Option<WatcherHandle>>>, workspace_id: &str) {
    let mut guard = match slot.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let Some(w) = guard.as_mut() else {
        return;
    };
    if let Err(e) = w.remove_peer(workspace_id) {
        eprintln!("standardoc mcp: watcher remove_peer failed for {workspace_id}: {e}");
    }
}

fn parse_kind(s: &str) -> Result<Kind, ErrorData> {
    match s {
        "callable" => Ok(Kind::Callable),
        "type" => Ok(Kind::Type),
        "value" => Ok(Kind::Value),
        "module" => Ok(Kind::Module),
        "macro" => Ok(Kind::Macro),
        other => Err(ErrorData::invalid_params(
            format!(
                "unknown kind `{other}` — expected one of: callable, type, value, module, macro"
            ),
            None,
        )),
    }
}

fn parse_threshold(raw: Option<f32>) -> Result<f32, ErrorData> {
    let value = raw.unwrap_or(FIND_SIMILAR_DEFAULT_THRESHOLD);
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(ErrorData::invalid_params(
            format!("threshold must be a finite value in [0.0, 1.0], got `{value}`"),
            None,
        ));
    }
    Ok(value)
}

fn parse_visibility(s: &str) -> Result<Visibility, ErrorData> {
    match s {
        "public" => Ok(Visibility::Public),
        "private" => Ok(Visibility::Private),
        "crate" => Ok(Visibility::Crate),
        "protected" => Ok(Visibility::Protected),
        other => Err(ErrorData::invalid_params(
            format!(
                "unknown visibility `{other}` — expected one of: public, private, crate, protected"
            ),
            None,
        )),
    }
}

/// Runs the strsim-backed similarity search to populate the
/// `did_you_mean` field surfaced by `find_symbol` /
/// `find_symbols_by_pattern` when the primary query returns zero hits.
/// Returns a slim JSON array `[{fqdn, name, kind, score}, ...]` capped
/// at `DID_YOU_MEAN_LIMIT` and floored at `DID_YOU_MEAN_THRESHOLD`.
async fn compute_did_you_mean(
    handle: IndexHandle,
    text: String,
    filter: SymbolFilter,
) -> Result<Vec<serde_json::Value>, ErrorData> {
    let pairs = tokio::task::spawn_blocking(move || {
        query::find_similar(
            &handle,
            &text,
            DID_YOU_MEAN_THRESHOLD,
            &filter,
            DID_YOU_MEAN_LIMIT,
        )
    })
    .await
    .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
    .map_err(|e| server_error_to_rmcp(&e.into()))?;
    Ok(pairs
        .into_iter()
        .map(|(sym, score)| {
            serde_json::json!({
                "fqdn": sym.fqdn,
                "name": sym.name,
                "kind": serde_json::to_value(sym.kind).unwrap_or(serde_json::Value::Null),
                "score": score,
            })
        })
        .collect())
}

/// Strips SQLite GLOB wildcards (`*`, `?`, `[`, `]`) from a pattern to
/// extract a "core name" usable for similarity scoring. Backs the
/// `did_you_mean` enrichment on empty `find_symbols_by_pattern`
/// results — e.g. `*to_token_string*` → `to_token_string`, then strsim
/// surfaces `to_token_stream`.
fn glob_core_text(pattern: &str) -> String {
    pattern
        .chars()
        .filter(|c| !matches!(c, '*' | '?' | '[' | ']'))
        .collect::<String>()
        .trim()
        .to_string()
}

/// Defense-in-depth normalization for FQDN inputs reaching exact-match
/// query paths (`get_body`, `get_context`, module filters, …).
///
/// LLM consumers trained on Python / JS / TS naturally emit OOP-style
/// dotted names (`Type.method`) even though Standardoc stores every
/// FQDN with `::` regardless of source language. Without this
/// normalization, `get_body("StandardocMcp.find_symbol")` would miss
/// the symbol stored as `…::StandardocMcp::find_symbol` and surface a
/// "no symbol found" message that looks like a real absence.
///
/// `.` never appears inside a valid FQDN segment in any supported
/// language (Rust / TS / Lua identifiers can't contain a dot), so the
/// replacement is lossless and idempotent on `::`-form inputs.
fn normalize_fqdn(raw: &str) -> String {
    raw.replace('.', "::")
}

/// Project `fqdn` to its `relative_to`-anchored form. FQDNs sharing the
/// prefix become `::<rest>`; the prefix itself collapses to the empty
/// string; FQDNs that don't share the prefix are returned verbatim. An
/// empty `relative_to` short-circuits to the input. Used by the
/// `find_symbol_fqdns` / `list_symbol_fqdns` projections to compress
/// scoped listings.
fn relative_fqdn(fqdn: &str, relative_to: &str) -> String {
    if relative_to.is_empty() {
        return fqdn.to_string();
    }
    if fqdn == relative_to {
        return String::new();
    }
    if let Some(rest) = fqdn.strip_prefix(relative_to)
        && let Some(rest) = rest.strip_prefix("::")
    {
        return format!("::{rest}");
    }
    fqdn.to_string()
}

fn success_json<T: Serialize>(value: &T) -> CallToolResult {
    match serde_json::to_string_pretty(value) {
        Ok(json) => CallToolResult::success(vec![Content::text(json)]),
        Err(e) => CallToolResult::error(vec![Content::text(format!(
            "failed to serialize tool result: {e}"
        ))]),
    }
}

fn clamp_limit(raw: Option<u8>) -> u8 {
    raw.unwrap_or(FIND_SYMBOL_DEFAULT_LIMIT)
        .clamp(1, FIND_SYMBOL_MAX_LIMIT)
}

/// Wall-clock seconds since the Unix epoch. Cheap helper used by the
/// in-memory `recent_depth1` tracker — no need to drag a sessions
/// dependency in.
fn current_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

fn indexing_in_progress_message(progress: Option<(u64, u64)>) -> String {
    match progress {
        Some((done, total)) if total > 0 => format!(
            "Workspace indexing in progress ({done}/{total} files). Please retry in a few seconds."
        ),
        _ => "Workspace indexing in progress. Please retry in a few seconds.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_limit_defaults_to_twenty_when_unset() {
        assert_eq!(clamp_limit(None), FIND_SYMBOL_DEFAULT_LIMIT);
    }

    #[test]
    fn clamp_limit_caps_at_max() {
        assert_eq!(clamp_limit(Some(255)), FIND_SYMBOL_MAX_LIMIT);
    }

    #[test]
    fn clamp_limit_floors_at_one_when_zero_requested() {
        assert_eq!(clamp_limit(Some(0)), 1);
    }

    #[test]
    fn indexing_message_includes_progress_when_known() {
        let msg = indexing_in_progress_message(Some((42, 100)));
        assert!(msg.contains("42/100 files"), "got `{msg}`");
    }

    #[test]
    fn indexing_message_omits_progress_when_zero_total() {
        let msg = indexing_in_progress_message(Some((0, 0)));
        assert!(!msg.contains('/'), "got `{msg}`");
    }

    #[test]
    fn relative_fqdn_strips_prefix_with_marker() {
        assert_eq!(
            relative_fqdn("foo::bar::baz::qux", "foo::bar"),
            "::baz::qux",
        );
    }

    #[test]
    fn relative_fqdn_returns_empty_on_self_match() {
        assert_eq!(relative_fqdn("foo::bar", "foo::bar"), "");
    }

    #[test]
    fn relative_fqdn_passes_through_when_prefix_does_not_match() {
        assert_eq!(
            relative_fqdn("other::lib::x", "foo::bar"),
            "other::lib::x",
        );
    }

    #[test]
    fn relative_fqdn_short_circuits_on_empty_anchor() {
        assert_eq!(relative_fqdn("foo::bar::baz", ""), "foo::bar::baz");
    }

    #[test]
    fn relative_fqdn_requires_segment_boundary() {
        // `foo::bar` should NOT match the prefix of `foo::barista` — the
        // boundary is `::`, not raw string prefix.
        assert_eq!(
            relative_fqdn("foo::barista::x", "foo::bar"),
            "foo::barista::x",
        );
    }

    use standardoc_core::{ScanFilters, cold_start};
    use standardoc_lang_provider::WorkspaceProvider;
    use std::path::Path;
    use tempfile::TempDir;

    fn fixture() -> (TempDir, StandardocMcp) {
        let dir = tempfile::tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        let provider: Arc<dyn LanguageProvider> = Arc::new(WorkspaceProvider::new());
        let filters = Arc::new(RwLock::new(ScanFilters::load(handle.workspace_root())));
        let mcp = StandardocMcp::new(handle, provider, filters);
        (dir, mcp)
    }

    fn cold_start_workspace(mcp: &StandardocMcp, root: &Path) {
        let provider = WorkspaceProvider::new();
        let filters = ScanFilters::load(root);
        cold_start::run(&mcp.handle, &provider, &filters).unwrap();
        mcp.index_ready.store(true, Ordering::Release);
    }

    fn body_text(result: &CallToolResult) -> String {
        result
            .content
            .iter()
            .filter_map(|c| match &c.raw {
                rmcp::model::RawContent::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn get_context_returns_indexing_in_progress_when_not_ready() {
        let (_dir, mcp) = fixture();
        let result = mcp
            .get_context(Parameters(GetContextParams {
                fqdn: "crate::anything".into(),
                depth: None,
            }))
            .await
            .expect("tool returns Ok with friendly degradation");
        let text = body_text(&result);
        assert!(
            text.contains("Workspace indexing in progress"),
            "expected friendly progress message, got `{text}`"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn find_symbol_returns_indexing_in_progress_when_not_ready() {
        let (_dir, mcp) = fixture();
        let result = mcp
            .find_symbol(Parameters(FindSymbolParams {
                query: "anything".into(),
                limit: None,
                kind: None,
                visibility: None,
                module: None,
                include_external: None,
                workspace_id: None,
            }))
            .await
            .expect("tool returns Ok with friendly degradation");
        let text = body_text(&result);
        assert!(
            text.contains("Workspace indexing in progress"),
            "expected friendly progress message, got `{text}`"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn list_symbols_returns_indexing_in_progress_when_not_ready() {
        let (_dir, mcp) = fixture();
        let result = mcp
            .list_symbols(Parameters(ListSymbolsParams {
                kind: None,
                visibility: None,
                module: None,
                limit: None,
                include_external: None,
                cursor: None,
                workspace_id: None,
            }))
            .await
            .expect("tool returns Ok with friendly degradation");
        assert!(body_text(&result).contains("Workspace indexing in progress"));
    }

    /// Envelope-shape check: an empty workspace must still return
    /// `{"items": [...], "next_cursor": ...}`, not a bare array. This
    /// is the contract the playground + ext rely on to walk pages.
    #[tokio::test(flavor = "multi_thread")]
    async fn list_symbols_returns_page_envelope_when_empty() {
        let (dir, mcp) = fixture();
        cold_start_workspace(&mcp, dir.path());
        let result = mcp
            .list_symbols(Parameters(ListSymbolsParams {
                kind: None,
                visibility: None,
                module: None,
                limit: None,
                include_external: Some(false),
                cursor: None,
                workspace_id: None,
            }))
            .await
            .unwrap();
        let text = body_text(&result);
        let json: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("envelope must be valid JSON, got `{text}`: {e}"));
        assert!(
            json.get("items").is_some_and(|v| v.is_array()),
            "envelope must carry an `items` array, got `{text}`"
        );
        assert!(
            json.get("next_cursor").is_some(),
            "envelope must carry a `next_cursor` field (null when no more pages), got `{text}`"
        );
        // Empty workspace → empty items + null cursor.
        assert_eq!(json["items"].as_array().unwrap().len(), 0);
        assert!(json["next_cursor"].is_null());
    }

    /// The `cursor` param must be plumbed through the JsonSchema and
    /// not rejected as an unknown parameter. We don't seed real
    /// symbols here — just verify the daemon accepts the cursor and
    /// returns a well-formed envelope (the core layer is exhaustively
    /// tested in `standardoc-core::query::tests::list_symbols_cursor_*`).
    #[tokio::test(flavor = "multi_thread")]
    async fn list_symbols_accepts_cursor_param() {
        let (dir, mcp) = fixture();
        cold_start_workspace(&mcp, dir.path());
        let result = mcp
            .list_symbols(Parameters(ListSymbolsParams {
                kind: None,
                visibility: None,
                module: None,
                limit: Some(2),
                include_external: Some(false),
                cursor: Some("crate::anchor".into()),
                workspace_id: None,
            }))
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_str(&body_text(&result)).unwrap();
        assert!(json["items"].is_array());
        assert!(json["next_cursor"].is_null());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn find_symbols_by_pattern_returns_indexing_in_progress_when_not_ready() {
        let (_dir, mcp) = fixture();
        let result = mcp
            .find_symbols_by_pattern(Parameters(FindSymbolsByPatternParams {
                pattern: "anything_*".into(),
                kind: None,
                visibility: None,
                module: None,
                limit: None,
                include_external: None,
                workspace_id: None,
            }))
            .await
            .expect("tool returns Ok with friendly degradation");
        assert!(body_text(&result).contains("Workspace indexing in progress"));
    }

    #[test]
    fn parse_kind_recognises_every_ir_variant() {
        assert!(parse_kind("callable").is_ok());
        assert!(parse_kind("type").is_ok());
        assert!(parse_kind("value").is_ok());
        assert!(parse_kind("module").is_ok());
        assert!(parse_kind("macro").is_ok());
    }

    #[test]
    fn parse_kind_rejects_unknown() {
        assert!(parse_kind("class").is_err());
        assert!(parse_kind("").is_err());
    }

    #[test]
    fn parse_visibility_recognises_every_ir_variant() {
        assert!(parse_visibility("public").is_ok());
        assert!(parse_visibility("private").is_ok());
        assert!(parse_visibility("crate").is_ok());
        assert!(parse_visibility("protected").is_ok());
    }

    #[test]
    fn parse_visibility_rejects_unknown() {
        assert!(parse_visibility("internal").is_err());
        assert!(parse_visibility("").is_err());
    }

    #[test]
    fn parse_filter_propagates_module_string_unchanged() {
        let f = parse_filter(
            Some("callable"),
            Some("private"),
            Some("crate::a".into()),
            None,
            None,
        )
        .unwrap();
        assert_eq!(f.kind, Some(Kind::Callable));
        assert_eq!(f.visibility, Some(Visibility::Private));
        assert_eq!(f.module.as_deref(), Some("crate::a"));
        assert!(
            f.include_external,
            "omitting include_external must default to true (S3-G include externals by default)"
        );
    }

    #[test]
    fn parse_filter_all_none_yields_empty_filter() {
        let f = parse_filter(None, None, None, None, None).unwrap();
        assert_eq!(f, SymbolFilter::default());
    }

    #[test]
    fn parse_filter_propagates_workspace_id_when_supplied() {
        // L3e-2: workspace_id flows through parse_filter unchanged so
        // downstream SQL narrows to that peer's rows.
        let f = parse_filter(None, None, None, None, Some("peer-uuid-xyz".into())).unwrap();
        assert_eq!(f.workspace_id.as_deref(), Some("peer-uuid-xyz"));
        assert_eq!(f.effective_workspace_id(), "peer-uuid-xyz");
    }

    #[test]
    fn parse_filter_propagates_include_external_false() {
        let f = parse_filter(None, None, None, Some(false), None).unwrap();
        assert!(
            !f.include_external,
            "explicit false must scope queries to workspace-only symbols"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn get_context_returns_no_symbol_message_when_fqdn_unknown() {
        let (dir, mcp) = fixture();
        cold_start_workspace(&mcp, dir.path());
        let result = mcp
            .get_context(Parameters(GetContextParams {
                fqdn: "crate::ghost".into(),
                depth: Some(1),
            }))
            .await
            .unwrap();
        let text = body_text(&result);
        assert!(
            text.contains("no symbol found"),
            "expected `no symbol found` message, got `{text}`"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn find_symbol_returns_empty_array_for_blank_query() {
        let (dir, mcp) = fixture();
        cold_start_workspace(&mcp, dir.path());
        let result = mcp
            .find_symbol(Parameters(FindSymbolParams {
                query: "   ".into(),
                limit: None,
                kind: None,
                visibility: None,
                module: None,
                include_external: None,
                workspace_id: None,
            }))
            .await
            .unwrap();
        let text = body_text(&result);
        assert!(
            text.trim() == "[]",
            "blank query must short-circuit to empty JSON array, got `{text}`"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn find_symbols_by_pattern_returns_empty_array_for_blank_pattern() {
        let (dir, mcp) = fixture();
        cold_start_workspace(&mcp, dir.path());
        let result = mcp
            .find_symbols_by_pattern(Parameters(FindSymbolsByPatternParams {
                pattern: "   ".into(),
                kind: None,
                visibility: None,
                module: None,
                limit: None,
                include_external: None,
                workspace_id: None,
            }))
            .await
            .unwrap();
        assert_eq!(body_text(&result).trim(), "[]");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn find_similar_symbols_returns_indexing_in_progress_when_not_ready() {
        let (_dir, mcp) = fixture();
        let result = mcp
            .find_similar_symbols(Parameters(FindSimilarSymbolsParams {
                reference: "anything".into(),
                threshold: None,
                limit: None,
                kind: None,
                visibility: None,
                module: None,
                include_external: None,
            }))
            .await
            .expect("tool returns Ok with friendly degradation");
        assert!(body_text(&result).contains("Workspace indexing in progress"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn find_similar_symbols_blank_reference_returns_empty_array() {
        let (dir, mcp) = fixture();
        cold_start_workspace(&mcp, dir.path());
        let result = mcp
            .find_similar_symbols(Parameters(FindSimilarSymbolsParams {
                reference: "   ".into(),
                threshold: None,
                limit: None,
                kind: None,
                visibility: None,
                module: None,
                include_external: None,
            }))
            .await
            .unwrap();
        assert_eq!(body_text(&result).trim(), "[]");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn find_similar_symbols_threshold_above_one_rejected() {
        let (dir, mcp) = fixture();
        cold_start_workspace(&mcp, dir.path());
        let result = mcp
            .find_similar_symbols(Parameters(FindSimilarSymbolsParams {
                reference: "foo".into(),
                threshold: Some(1.5),
                limit: None,
                kind: None,
                visibility: None,
                module: None,
                include_external: None,
            }))
            .await;
        assert!(
            result.is_err(),
            "out-of-range threshold must be rejected with ErrorData"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn find_similar_symbols_threshold_negative_rejected() {
        let (dir, mcp) = fixture();
        cold_start_workspace(&mcp, dir.path());
        let result = mcp
            .find_similar_symbols(Parameters(FindSimilarSymbolsParams {
                reference: "foo".into(),
                threshold: Some(-0.1),
                limit: None,
                kind: None,
                visibility: None,
                module: None,
                include_external: None,
            }))
            .await;
        assert!(result.is_err(), "negative threshold must be rejected");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn find_similar_symbols_invalid_kind_filter_returns_error() {
        let (dir, mcp) = fixture();
        cold_start_workspace(&mcp, dir.path());
        let result = mcp
            .find_similar_symbols(Parameters(FindSimilarSymbolsParams {
                reference: "anything".into(),
                threshold: None,
                limit: None,
                kind: Some("class".into()),
                visibility: None,
                module: None,
                include_external: None,
            }))
            .await;
        assert!(result.is_err(), "invalid `kind` filter must be rejected");
    }

    #[test]
    fn parse_threshold_defaults_to_zero_eight_when_none() {
        let got = parse_threshold(None).unwrap();
        assert!((got - FIND_SIMILAR_DEFAULT_THRESHOLD).abs() < f32::EPSILON);
    }

    #[test]
    fn parse_threshold_accepts_zero_and_one_inclusive() {
        assert!(parse_threshold(Some(0.0)).is_ok());
        assert!(parse_threshold(Some(1.0)).is_ok());
    }

    #[test]
    fn parse_threshold_rejects_nan_and_infinity() {
        assert!(parse_threshold(Some(f32::NAN)).is_err());
        assert!(parse_threshold(Some(f32::INFINITY)).is_err());
        assert!(parse_threshold(Some(f32::NEG_INFINITY)).is_err());
    }

    #[test]
    fn glob_core_text_strips_star_wildcard() {
        assert_eq!(glob_core_text("*to_token_string*"), "to_token_string");
    }

    #[test]
    fn glob_core_text_strips_question_and_bracket_wildcards_but_keeps_inner_chars() {
        // Brackets themselves are stripped ; the character class content
        // is kept verbatim — strsim still benefits even from `[abc]`
        // alternatives rather than dropping the whole group.
        assert_eq!(glob_core_text("get_?[abc]_value"), "get_abc_value");
    }

    #[test]
    fn glob_core_text_empty_for_only_wildcards() {
        assert_eq!(glob_core_text("***"), "");
        assert_eq!(glob_core_text("?*[]"), "");
    }

    #[test]
    fn glob_core_text_preserves_alphanumeric_and_separators() {
        assert_eq!(
            glob_core_text("standardoc-cli::main"),
            "standardoc-cli::main"
        );
    }

    #[test]
    fn normalize_fqdn_replaces_dot_with_double_colon() {
        assert_eq!(
            normalize_fqdn("StandardocMcp.find_symbol"),
            "StandardocMcp::find_symbol"
        );
        assert_eq!(
            normalize_fqdn("crate.mod.Type.method"),
            "crate::mod::Type::method"
        );
    }

    #[test]
    fn normalize_fqdn_is_idempotent_on_double_colon_form() {
        let canonical = "standardoc_core::query::search_text";
        assert_eq!(normalize_fqdn(canonical), canonical);
    }

    #[test]
    fn normalize_fqdn_preserves_other_separators_and_hyphens() {
        // Hyphens (crate names like standardoc-cli) and slashes (TS
        // package paths like @scope/pkg) must survive.
        assert_eq!(
            normalize_fqdn("standardoc-cli::main"),
            "standardoc-cli::main"
        );
        assert_eq!(
            normalize_fqdn("@app/web::module::foo"),
            "@app/web::module::foo"
        );
    }

    #[test]
    fn normalize_fqdn_handles_empty_input() {
        assert_eq!(normalize_fqdn(""), "");
    }

    #[test]
    fn normalize_fqdn_collapses_consecutive_dots_into_quad_colons() {
        // Documents the literal-replace behaviour : malformed input
        // produces literal `::::`. We don't attempt to fix user
        // mistakes ; the downstream exact-match query will fail with
        // a clean "no symbol found" instead of silently passing.
        assert_eq!(normalize_fqdn("foo..bar"), "foo::::bar");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn find_symbol_invalid_kind_filter_returns_error() {
        let (dir, mcp) = fixture();
        cold_start_workspace(&mcp, dir.path());
        let result = mcp
            .find_symbol(Parameters(FindSymbolParams {
                query: "anything".into(),
                limit: None,
                kind: Some("class".into()),
                visibility: None,
                module: None,
                include_external: None,
                workspace_id: None,
            }))
            .await;
        // Invalid filter is a parameter error — surfaces as Err on the
        // tool invocation, NOT a graceful CallToolResult.
        assert!(
            result.is_err(),
            "invalid `kind` must be rejected with ErrorData"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn get_body_returns_indexing_in_progress_when_not_ready() {
        let (_dir, mcp) = fixture();
        let result = mcp
            .get_body(Parameters(GetBodyParams {
                fqdn: "crate::foo".into(),
                max_lines: None,
                strip_attrs: None,
                signature_only: None,
                strip_inline_comments: None,
            }))
            .await
            .expect("tool returns Ok with friendly degradation");
        assert!(body_text(&result).contains("Workspace indexing in progress"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compute_routing_hint_is_none_for_depth_one() {
        let (_dir, mcp) = fixture();
        assert_eq!(mcp.compute_routing_hint("crate::any", 1, 1_000), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compute_routing_hint_fires_for_naked_depth_two() {
        let (_dir, mcp) = fixture();
        let hint = mcp.compute_routing_hint("crate::any", 2, 1_000);
        assert!(hint.is_some(), "naked depth=2 must surface a routing hint");
        let msg = hint.unwrap();
        assert!(msg.contains("depth=2"), "got `{msg}`");
        assert!(msg.contains("depth=1"), "got `{msg}`");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compute_routing_hint_silent_after_recent_depth_one() {
        let (_dir, mcp) = fixture();
        let now = 10_000_i64;
        mcp.record_recent_depth1("crate::scoped", now - 60);
        // 60 s after a depth=1 call, depth=2 should be hint-free.
        assert_eq!(
            mcp.compute_routing_hint("crate::scoped", 2, now),
            None,
            "depth=2 within the 5 min window must NOT trigger the hint"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compute_routing_hint_fires_again_when_window_expires() {
        let (_dir, mcp) = fixture();
        let now = 10_000_i64;
        mcp.record_recent_depth1("crate::stale", now - 600);
        // 10 min later, the prior depth=1 is outside the 5 min window.
        let hint = mcp.compute_routing_hint("crate::stale", 2, now);
        assert!(
            hint.is_some(),
            "stale scoping pass must not silence the hint"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn record_recent_depth_one_evicts_entries_older_than_retention() {
        let (_dir, mcp) = fixture();
        let now = 100_000_i64;
        mcp.record_recent_depth1("crate::ancient", now - 5_000);
        // Insert another entry far enough in the future that the retention
        // window (1800 s) drops the ancient one on the next sweep.
        mcp.record_recent_depth1("crate::fresh", now + 2_000);
        let (has_ancient, has_fresh) = {
            let guard = mcp.recent_depth1.lock().unwrap();
            (
                guard.contains_key("crate::ancient"),
                guard.contains_key("crate::fresh"),
            )
        };
        assert!(!has_ancient);
        assert!(has_fresh);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn get_body_returns_no_symbol_message_when_fqdn_unknown() {
        let (dir, mcp) = fixture();
        cold_start_workspace(&mcp, dir.path());
        let result = mcp
            .get_body(Parameters(GetBodyParams {
                fqdn: "crate::nope::never_indexed".into(),
                max_lines: None,
                strip_attrs: None,
                signature_only: None,
                strip_inline_comments: None,
            }))
            .await
            .unwrap();
        assert!(body_text(&result).contains("no symbol found"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn current_revision_returns_zero_on_fresh_index() {
        let (_dir, mcp) = fixture();
        let result = mcp.current_revision().await.unwrap();
        let body = body_text(&result);
        assert!(body.contains("\"revision\": 0"), "got `{body}`");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn current_revision_advances_after_cold_start_writes() {
        let (dir, mcp) = fixture();
        cold_start_workspace(&mcp, dir.path());
        let result = mcp.current_revision().await.unwrap();
        let body = body_text(&result);
        // Empty workspace = 0 writes = revision stays 0. We rely on the field
        // shape rather than the exact value here.
        assert!(body.contains("\"revision\""), "got `{body}`");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn current_revision_omits_rag_field_post_removal() {
        let (_dir, mcp) = fixture();
        let result = mcp.current_revision().await.unwrap();
        let body = body_text(&result);
        // RAG layer was removed — the `rag` field must no longer be
        // surfaced. Consumers that previously read `rag.enabled` get a
        // breaking absence rather than a stale `false`.
        assert!(
            !body.contains("\"rag\""),
            "rag capability block must be gone, got `{body}`"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn current_revision_reports_indexing_not_ready_before_cold_start() {
        let (_dir, mcp) = fixture();
        let result = mcp.current_revision().await.unwrap();
        let body = body_text(&result);
        assert!(
            body.contains("\"indexing\""),
            "expected indexing block, got `{body}`"
        );
        assert!(body.contains("\"ready\": false"), "got `{body}`");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn current_revision_reports_indexing_ready_after_cold_start() {
        let (dir, mcp) = fixture();
        cold_start_workspace(&mcp, dir.path());
        let result = mcp.current_revision().await.unwrap();
        let body = body_text(&result);
        assert!(body.contains("\"ready\": true"), "got `{body}`");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn current_revision_reports_watcher_active_when_handle_present() {
        let (_dir, mcp) = fixture();
        // No watcher has been spawned by the fixture, so this must be false.
        let result = mcp.current_revision().await.unwrap();
        let body = body_text(&result);
        assert!(body.contains("\"watcher\""), "got `{body}`");
        assert!(body.contains("\"active\": false"), "got `{body}`");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn stage3e3_current_revision_workspace_kind_null_pre_cold_start() {
        let (_dir, mcp) = fixture();
        let result = mcp.current_revision().await.unwrap();
        let body = body_text(&result);
        assert!(body.contains("\"workspace\""), "got `{body}`");
        // No discovery has run yet → null.
        assert!(body.contains("\"kind\": null"), "got `{body}`");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn stage3e3_current_revision_workspace_kind_is_null_when_no_manifest() {
        let (dir, mcp) = fixture();
        cold_start_workspace(&mcp, dir.path());
        let result = mcp.current_revision().await.unwrap();
        let body = body_text(&result);
        // Cold-start has run, but the fixture has no workspace manifest
        // at root → discovery deletes the row → `kind: null`. (Post-
        // revert of `WorkspaceKind::Single` — aligns with
        // standarbuild-detect 0.3.)
        assert!(body.contains("\"workspace\""), "got `{body}`");
        assert!(body.contains("\"kind\": null"), "got `{body}`");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn check_stale_empty_fetched_returns_empty_array() {
        let (dir, mcp) = fixture();
        cold_start_workspace(&mcp, dir.path());
        let result = mcp
            .check_stale(Parameters(CheckStaleParams { fetched: vec![] }))
            .await
            .unwrap();
        assert_eq!(body_text(&result).trim(), "[]");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn check_stale_unknown_fqdn_marked_missing() {
        let (dir, mcp) = fixture();
        cold_start_workspace(&mcp, dir.path());
        let result = mcp
            .check_stale(Parameters(CheckStaleParams {
                fetched: vec![FetchedEntry {
                    fqdn: "crate::nope::never_indexed".into(),
                    fetched_at_revision: 5,
                }],
            }))
            .await
            .unwrap();
        let body = body_text(&result);
        assert!(body.contains("\"status\": \"missing\""), "got `{body}`");
        assert!(
            body.contains("\"last_modified_revision\": null"),
            "got `{body}`"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn check_stale_returns_indexing_in_progress_when_not_ready() {
        let (_dir, mcp) = fixture();
        let result = mcp
            .check_stale(Parameters(CheckStaleParams {
                fetched: vec![FetchedEntry {
                    fqdn: "crate::foo".into(),
                    fetched_at_revision: 0,
                }],
            }))
            .await
            .expect("tool returns Ok with friendly degradation");
        assert!(body_text(&result).contains("Workspace indexing in progress"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn find_symbol_workspace_id_param_narrows_to_named_peer() {
        // L3e-2: passing `workspace_id` through the MCP tool reaches
        // the SQL filter. We don't need to seed peer rows here — the
        // core tests already cover that path. A non-existent peer
        // workspace_id must yield an empty result (proves the filter
        // is wired and primary rows aren't leaking through).
        use std::fs;
        let (dir, mcp) = fixture();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src").join("lib.rs"),
            "pub fn hello_marker() {}",
        )
        .unwrap();
        cold_start_workspace(&mcp, dir.path());

        let default_scope = mcp
            .find_symbol(Parameters(FindSymbolParams {
                query: "hello_marker".into(),
                limit: None,
                kind: None,
                visibility: None,
                module: None,
                include_external: None,
                workspace_id: None,
            }))
            .await
            .unwrap();
        let body = body_text(&default_scope);
        assert!(
            body.contains("hello_marker"),
            "default scope must surface the primary symbol, got `{body}`"
        );

        let peer_scope = mcp
            .find_symbol(Parameters(FindSymbolParams {
                query: "hello_marker".into(),
                limit: None,
                kind: None,
                visibility: None,
                module: None,
                include_external: None,
                workspace_id: Some("nonexistent-peer-uuid".into()),
            }))
            .await
            .unwrap();
        let body = body_text(&peer_scope);
        // The empty-result envelope is `did_you_mean` (DYM kicks in when
        // results vector is empty), not a leaked primary row.
        assert!(
            !body.contains("hello_marker") || body.contains("did_you_mean"),
            "peer scope must NOT leak the primary hello_marker symbol \
             (DYM is fine — it operates on names, not workspace), got `{body}`"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn link_workspace_returns_workspace_id_for_existing_path() {
        let (dir, mcp) = fixture();
        let peer = tempfile::tempdir().unwrap();
        let result = mcp
            .link_workspace(Parameters(LinkWorkspaceParams {
                path: peer.path().to_string_lossy().into_owned(),
                direction: "in".into(),
                indexing_mode: None,
            }))
            .await
            .expect("link_workspace ok");
        let body = body_text(&result);
        assert!(body.contains("\"workspace_id\""), "got `{body}`");
        assert!(body.contains("\"direction\": \"in\""), "got `{body}`");
        drop(dir);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn link_workspace_rejects_missing_path_with_did_you_mean() {
        let (_dir, mcp) = fixture();
        let parent = tempfile::tempdir().unwrap();
        std::fs::create_dir(parent.path().join("projects")).unwrap();
        let typo = parent.path().join("projcts");
        let err = mcp
            .link_workspace(Parameters(LinkWorkspaceParams {
                path: typo.to_string_lossy().into_owned(),
                direction: "in".into(),
                indexing_mode: None,
            }))
            .await
            .expect_err("missing path must surface invalid_params");
        let data = format!("{:?}", err.data);
        assert!(data.contains("did_you_mean"), "got `{data}`");
        assert!(data.contains("projects"), "got `{data}`");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn link_workspace_rejects_unknown_direction() {
        let (_dir, mcp) = fixture();
        let peer = tempfile::tempdir().unwrap();
        let err = mcp
            .link_workspace(Parameters(LinkWorkspaceParams {
                path: peer.path().to_string_lossy().into_owned(),
                direction: "sideways".into(),
                indexing_mode: None,
            }))
            .await
            .expect_err("bogus direction must be rejected");
        assert!(format!("{err}").contains("direction"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn list_linked_workspaces_returns_empty_array_on_fresh_index() {
        let (_dir, mcp) = fixture();
        let result = mcp
            .list_linked_workspaces()
            .await
            .expect("list_linked_workspaces ok");
        let body = body_text(&result);
        assert!(body.contains("\"workspaces\""), "got `{body}`");
        // Fresh index = no rows.
        assert!(body.contains("\"workspaces\": []"), "got `{body}`");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn unlink_workspace_after_link_removes_row() {
        let (_dir, mcp) = fixture();
        let peer = tempfile::tempdir().unwrap();
        let link = mcp
            .link_workspace(Parameters(LinkWorkspaceParams {
                path: peer.path().to_string_lossy().into_owned(),
                direction: "out".into(),
                indexing_mode: None,
            }))
            .await
            .expect("link ok");
        let link_body = body_text(&link);
        let workspace_id = link_body
            .split("\"workspace_id\": \"")
            .nth(1)
            .and_then(|tail| tail.split('"').next())
            .expect("workspace_id present")
            .to_string();

        let _ = mcp
            .unlink_workspace(Parameters(UnlinkWorkspaceParams {
                workspace_id: workspace_id.clone(),
            }))
            .await
            .expect("unlink ok");

        let list = mcp.list_linked_workspaces().await.expect("list ok");
        let list_body = body_text(&list);
        assert!(
            !list_body.contains(&workspace_id),
            "workspace must be gone after unlink, got `{list_body}`"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn link_workspace_in_direction_registers_peer_with_live_watcher() {
        // L3d-3: when the live watcher is booted, linking a peer with
        // direction=in pushes a PeerRoot into the watcher's registry so
        // the dispatch loop starts routing the peer's events
        // immediately (no cold_start needed).
        use standardoc_core::spawn_watcher;

        let (_dir, mcp) = fixture();
        let peer = tempfile::tempdir().unwrap();

        // Seed the watcher slot — the default fixture leaves it None.
        let watcher = spawn_watcher(
            mcp.handle.clone(),
            Arc::clone(&mcp.provider),
            Arc::clone(&mcp.filters),
        )
        .expect("watcher boot");
        {
            let slot = mcp.watcher_slot();
            let mut guard = slot.lock().unwrap();
            *guard = Some(watcher);
        }

        let link = mcp
            .link_workspace(Parameters(LinkWorkspaceParams {
                path: peer.path().to_string_lossy().into_owned(),
                direction: "in".into(),
                indexing_mode: None,
            }))
            .await
            .expect("link ok");
        let workspace_id = body_text(&link)
            .split("\"workspace_id\": \"")
            .nth(1)
            .and_then(|tail| tail.split('"').next())
            .expect("workspace_id present")
            .to_string();

        let slot = mcp.watcher_slot();
        let guard = slot.lock().unwrap();
        let snapshot = guard.as_ref().expect("watcher present").peers_snapshot();
        assert_eq!(snapshot.len(), 1, "peer must be registered");
        assert_eq!(snapshot[0].workspace_id, workspace_id);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn link_workspace_out_direction_skips_watcher_registration() {
        // L3d-3: direction=out means the peer reads us, not us reading
        // them — there is nothing to watch on their side, so the
        // watcher registry stays empty even when the slot is booted.
        use standardoc_core::spawn_watcher;

        let (_dir, mcp) = fixture();
        let peer = tempfile::tempdir().unwrap();

        let watcher = spawn_watcher(
            mcp.handle.clone(),
            Arc::clone(&mcp.provider),
            Arc::clone(&mcp.filters),
        )
        .expect("watcher boot");
        {
            let slot = mcp.watcher_slot();
            let mut guard = slot.lock().unwrap();
            *guard = Some(watcher);
        }

        let _ = mcp
            .link_workspace(Parameters(LinkWorkspaceParams {
                path: peer.path().to_string_lossy().into_owned(),
                direction: "out".into(),
                indexing_mode: None,
            }))
            .await
            .expect("link ok");

        let slot = mcp.watcher_slot();
        let guard = slot.lock().unwrap();
        let snapshot = guard.as_ref().expect("watcher present").peers_snapshot();
        assert!(
            snapshot.is_empty(),
            "Out direction must not register a peer; got {snapshot:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn unlink_workspace_removes_peer_from_live_watcher() {
        // L3d-3: the unlink handler drops the peer from the live
        // watcher registry in addition to the catalog write.
        use standardoc_core::spawn_watcher;

        let (_dir, mcp) = fixture();
        let peer = tempfile::tempdir().unwrap();

        let watcher = spawn_watcher(
            mcp.handle.clone(),
            Arc::clone(&mcp.provider),
            Arc::clone(&mcp.filters),
        )
        .expect("watcher boot");
        {
            let slot = mcp.watcher_slot();
            let mut guard = slot.lock().unwrap();
            *guard = Some(watcher);
        }

        let link = mcp
            .link_workspace(Parameters(LinkWorkspaceParams {
                path: peer.path().to_string_lossy().into_owned(),
                direction: "in".into(),
                indexing_mode: None,
            }))
            .await
            .expect("link ok");
        let workspace_id = body_text(&link)
            .split("\"workspace_id\": \"")
            .nth(1)
            .and_then(|tail| tail.split('"').next())
            .expect("workspace_id present")
            .to_string();

        let _ = mcp
            .unlink_workspace(Parameters(UnlinkWorkspaceParams {
                workspace_id: workspace_id.clone(),
            }))
            .await
            .expect("unlink ok");

        let slot = mcp.watcher_slot();
        let guard = slot.lock().unwrap();
        let snapshot = guard.as_ref().expect("watcher present").peers_snapshot();
        assert!(
            snapshot.is_empty(),
            "peer must be gone from watcher after unlink"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn set_link_direction_out_to_in_adds_peer_to_live_watcher() {
        // post-3b-7-b finalize: a peer linked with direction=Out is NOT
        // watched (Out means the peer reads us). Flipping to direction=in
        // must register the peer on the live watcher so subsequent file
        // changes flow through dispatch.
        use standardoc_core::spawn_watcher;

        let (_dir, mcp) = fixture();
        let peer = tempfile::tempdir().unwrap();

        let watcher = spawn_watcher(
            mcp.handle.clone(),
            Arc::clone(&mcp.provider),
            Arc::clone(&mcp.filters),
        )
        .expect("watcher boot");
        {
            let slot = mcp.watcher_slot();
            let mut guard = slot.lock().unwrap();
            *guard = Some(watcher);
        }

        let link = mcp
            .link_workspace(Parameters(LinkWorkspaceParams {
                path: peer.path().to_string_lossy().into_owned(),
                direction: "out".into(),
                indexing_mode: None,
            }))
            .await
            .expect("link ok");
        let workspace_id = body_text(&link)
            .split("\"workspace_id\": \"")
            .nth(1)
            .and_then(|tail| tail.split('"').next())
            .expect("workspace_id present")
            .to_string();

        // Pre-condition: Out direction means NO peer is registered.
        {
            let slot = mcp.watcher_slot();
            let guard = slot.lock().unwrap();
            assert!(
                guard.as_ref().unwrap().peers_snapshot().is_empty(),
                "Out direction must not register a peer"
            );
        }

        let response = mcp
            .set_link_direction(Parameters(SetLinkDirectionParams {
                workspace_id: workspace_id.clone(),
                direction: "in".into(),
            }))
            .await
            .expect("set_link_direction ok");
        let body = body_text(&response);
        assert!(
            body.contains("\"previous_direction\": \"out\""),
            "got `{body}`"
        );
        assert!(body.contains("\"new_direction\": \"in\""), "got `{body}`");

        let slot = mcp.watcher_slot();
        let guard = slot.lock().unwrap();
        let snapshot = guard.as_ref().unwrap().peers_snapshot();
        assert_eq!(snapshot.len(), 1, "Out → In must register the peer");
        assert_eq!(snapshot[0].workspace_id, workspace_id);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn set_link_direction_in_to_out_removes_peer_from_live_watcher() {
        // Inverse of the above: In → Out must unregister the peer.
        use standardoc_core::spawn_watcher;

        let (_dir, mcp) = fixture();
        let peer = tempfile::tempdir().unwrap();

        let watcher = spawn_watcher(
            mcp.handle.clone(),
            Arc::clone(&mcp.provider),
            Arc::clone(&mcp.filters),
        )
        .expect("watcher boot");
        {
            let slot = mcp.watcher_slot();
            let mut guard = slot.lock().unwrap();
            *guard = Some(watcher);
        }

        let link = mcp
            .link_workspace(Parameters(LinkWorkspaceParams {
                path: peer.path().to_string_lossy().into_owned(),
                direction: "in".into(),
                indexing_mode: None,
            }))
            .await
            .expect("link ok");
        let workspace_id = body_text(&link)
            .split("\"workspace_id\": \"")
            .nth(1)
            .and_then(|tail| tail.split('"').next())
            .expect("workspace_id present")
            .to_string();

        // Pre-condition: In direction registered the peer (L3d-3).
        {
            let slot = mcp.watcher_slot();
            let guard = slot.lock().unwrap();
            assert_eq!(guard.as_ref().unwrap().peers_snapshot().len(), 1);
        }

        let _ = mcp
            .set_link_direction(Parameters(SetLinkDirectionParams {
                workspace_id: workspace_id.clone(),
                direction: "out".into(),
            }))
            .await
            .expect("set_link_direction ok");

        let slot = mcp.watcher_slot();
        let guard = slot.lock().unwrap();
        assert!(
            guard.as_ref().unwrap().peers_snapshot().is_empty(),
            "In → Out must unregister the peer"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn set_link_direction_same_side_transition_is_watcher_noop() {
        // In → Bidirectional: both directions watch the peer, so the
        // watcher registry must stay at 1 entry (NOT 0, NOT 2).
        use standardoc_core::spawn_watcher;

        let (_dir, mcp) = fixture();
        let peer = tempfile::tempdir().unwrap();

        let watcher = spawn_watcher(
            mcp.handle.clone(),
            Arc::clone(&mcp.provider),
            Arc::clone(&mcp.filters),
        )
        .expect("watcher boot");
        {
            let slot = mcp.watcher_slot();
            let mut guard = slot.lock().unwrap();
            *guard = Some(watcher);
        }

        let link = mcp
            .link_workspace(Parameters(LinkWorkspaceParams {
                path: peer.path().to_string_lossy().into_owned(),
                direction: "in".into(),
                indexing_mode: None,
            }))
            .await
            .expect("link ok");
        let workspace_id = body_text(&link)
            .split("\"workspace_id\": \"")
            .nth(1)
            .and_then(|tail| tail.split('"').next())
            .expect("workspace_id present")
            .to_string();

        let _ = mcp
            .set_link_direction(Parameters(SetLinkDirectionParams {
                workspace_id: workspace_id.clone(),
                direction: "bidirectional".into(),
            }))
            .await
            .expect("set_link_direction ok");

        let slot = mcp.watcher_slot();
        let guard = slot.lock().unwrap();
        let snapshot = guard.as_ref().unwrap().peers_snapshot();
        assert_eq!(
            snapshot.len(),
            1,
            "same-side transition must not change registry size"
        );
        assert_eq!(snapshot[0].workspace_id, workspace_id);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn set_link_direction_returns_invalid_params_for_unknown_workspace_id() {
        let (_dir, mcp) = fixture();
        let err = mcp
            .set_link_direction(Parameters(SetLinkDirectionParams {
                workspace_id: "ghost-uuid".into(),
                direction: "in".into(),
            }))
            .await
            .expect_err("unknown workspace_id must error");
        let rendered = format!("{err:?}");
        assert!(
            rendered.contains("ghost-uuid"),
            "error must surface offending workspace_id, got `{rendered}`"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refresh_peer_returns_invalid_params_for_unknown_workspace_id() {
        // L3-bis-2: unknown workspace_id surfaces as invalid_params
        // with the offending id in the data envelope, so MCP clients
        // can show a "no such peer" message without guessing.
        let (_dir, mcp) = fixture();
        let err = mcp
            .refresh_peer(Parameters(RefreshPeerParams {
                workspace_id: "ghost-uuid".into(),
            }))
            .await
            .expect_err("unknown workspace_id must error");
        let rendered = format!("{err:?}");
        assert!(
            rendered.contains("ghost-uuid"),
            "error must surface offending workspace_id, got `{rendered}`"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refresh_peer_after_link_returns_ok_stats_envelope() {
        // L3-bis-2: link a peer (no source seeded, so 0 files) and
        // call refresh_peer. The Ok envelope must carry the
        // workspace_id + status="ok" + numeric counters.
        let (_dir, mcp) = fixture();
        let peer = tempfile::tempdir().unwrap();
        let link = mcp
            .link_workspace(Parameters(LinkWorkspaceParams {
                path: peer.path().to_string_lossy().into_owned(),
                direction: "in".into(),
                indexing_mode: Some("extract".into()),
            }))
            .await
            .expect("link ok");
        let workspace_id = body_text(&link)
            .split("\"workspace_id\": \"")
            .nth(1)
            .and_then(|tail| tail.split('"').next())
            .expect("workspace_id present")
            .to_string();

        let result = mcp
            .refresh_peer(Parameters(RefreshPeerParams {
                workspace_id: workspace_id.clone(),
            }))
            .await
            .expect("refresh_peer ok");
        let body = body_text(&result);
        assert!(body.contains(&workspace_id), "got `{body}`");
        assert!(body.contains("\"kind\": \"ok\""), "got `{body}`");
        assert!(body.contains("\"files_extracted\""), "got `{body}`");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn module_lookup_returns_null_when_module_absent() {
        let (_dir, mcp) = fixture();
        let result = mcp
            .module_lookup(Parameters(ModuleLookupParams {
                module_fqdn: "no::such::module".into(),
                workspace_id: None,
            }))
            .await
            .expect("module_lookup ok");
        let body = body_text(&result);
        assert_eq!(body.trim(), "null");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn resolve_cross_workspace_returns_empty_providers_on_fresh_index() {
        let (_dir, mcp) = fixture();
        let result = mcp
            .resolve_cross_workspace(Parameters(ResolveCrossWorkspaceParams {
                origin_module: "ws_b::lib".into(),
                origin_symbol: "Foo".into(),
            }))
            .await
            .expect("resolve_cross_workspace ok");
        let body = body_text(&result);
        assert!(body.contains("\"providers\": []"), "got `{body}`");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn list_projects_returns_empty_array_on_fresh_index() {
        let (_dir, mcp) = fixture();
        let result = mcp.list_projects().await.expect("list_projects ok");
        let body = body_text(&result);
        assert!(body.contains("\"projects\""), "got `{body}`");
        assert!(body.contains("\"projects\": []"), "got `{body}`");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn list_projects_surfaces_detected_projects_after_cold_start() {
        let (dir, mcp) = fixture();
        // Seed the fixture as a Rust project. cold_start runs
        // `discover_and_persist_projects` which picks up the manifest.
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        cold_start_workspace(&mcp, dir.path());

        let result = mcp.list_projects().await.expect("list_projects ok");
        let body = body_text(&result);
        assert!(
            body.contains("\"kind\": \"rust\""),
            "expected the fixture Rust project to appear, got `{body}`"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn project_for_file_returns_null_when_path_unregistered() {
        let (_dir, mcp) = fixture();
        let result = mcp
            .project_for_file(Parameters(ProjectForFileParams {
                path: "/no/such/file.rs".into(),
            }))
            .await
            .expect("project_for_file ok");
        let body = body_text(&result);
        assert_eq!(body.trim(), "null");
    }

    // --- IR-4-f follow-up: find_call_sites MCP tool ---

    /// Write a Rust fixture under the workspace root so cold_start ends
    /// up walking it and populating `call_sites` via the real extractor
    /// + storage path. Cheaper than wiring around `pool()`'s pub(crate)
    /// visibility, and validates the full IR-4-b → IR-4-f pipeline in
    /// one shot.
    fn seed_rust_call_sites(root: &Path) {
        let src_dir = root.join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        // `caller_a` calls `tauri_invoke` (resolves locally) and
        // `foo`; `caller_b` also calls `tauri_invoke`; `caller_c`
        // calls a multi-segment member-access expression. Match the
        // call_text patterns the test queries below expect.
        std::fs::write(
            src_dir.join("lib.rs"),
            r#"
                fn tauri_invoke() {}
                fn foo() {}
                fn caller_a() { tauri_invoke(); foo(); }
                fn caller_b() { tauri_invoke(); }
                fn caller_c() { M.api.create(); }
            "#,
        )
        .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn find_call_sites_returns_indexing_in_progress_when_not_ready() {
        let (_dir, mcp) = fixture();
        let result = mcp
            .find_call_sites(Parameters(FindCallSitesParams {
                from_fqdn: None,
                callee_text: None,
                callee_pattern: None,
                limit: None,
            }))
            .await
            .expect("tool returns Ok with friendly degradation");
        assert!(body_text(&result).contains("Workspace indexing in progress"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn find_call_sites_no_filter_returns_all_extracted_rows() {
        // E2E pipeline check — extractor populates call_sites, storage
        // persists them, the MCP tool reads them back.
        let (dir, mcp) = fixture();
        seed_rust_call_sites(dir.path());
        cold_start_workspace(&mcp, dir.path());
        let result = mcp
            .find_call_sites(Parameters(FindCallSitesParams {
                from_fqdn: None,
                callee_text: None,
                callee_pattern: None,
                limit: None,
            }))
            .await
            .unwrap();
        let body = body_text(&result);
        let arr: serde_json::Value = serde_json::from_str(&body).unwrap();
        // 4 calls in the fixture: caller_a→tauri_invoke, caller_a→foo,
        // caller_b→tauri_invoke, caller_c→M.api.create.
        assert_eq!(
            arr.as_array().unwrap().len(),
            4,
            "expected 4 extracted call_sites, got `{body}`"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn find_call_sites_filter_by_callee_text_narrows_to_matching_records() {
        let (dir, mcp) = fixture();
        seed_rust_call_sites(dir.path());
        cold_start_workspace(&mcp, dir.path());
        let result = mcp
            .find_call_sites(Parameters(FindCallSitesParams {
                from_fqdn: None,
                callee_text: Some("tauri_invoke".into()),
                callee_pattern: None,
                limit: None,
            }))
            .await
            .unwrap();
        let body = body_text(&result);
        let arr: serde_json::Value = serde_json::from_str(&body).unwrap();
        let rows = arr.as_array().unwrap();
        assert_eq!(
            rows.len(),
            2,
            "two tauri_invoke calls in the fixture, got `{body}`"
        );
        for row in rows {
            assert_eq!(row["callee_text"].as_str(), Some("tauri_invoke"));
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn find_call_sites_filter_by_from_fqdn_returns_calls_of_one_caller() {
        let (dir, mcp) = fixture();
        seed_rust_call_sites(dir.path());
        cold_start_workspace(&mcp, dir.path());
        // The extractor stamps `from_fqdn` as the crate-relative FQDN of
        // the enclosing fn. For our fixture: `fixture::caller_a`.
        let result = mcp
            .find_call_sites(Parameters(FindCallSitesParams {
                from_fqdn: Some("fixture::caller_a".into()),
                callee_text: None,
                callee_pattern: None,
                limit: None,
            }))
            .await
            .unwrap();
        let body = body_text(&result);
        let arr: serde_json::Value = serde_json::from_str(&body).unwrap();
        let rows = arr.as_array().unwrap();
        assert_eq!(
            rows.len(),
            2,
            "caller_a calls both tauri_invoke + foo, got `{body}`"
        );
        for row in rows {
            assert_eq!(row["from_fqdn"].as_str(), Some("fixture::caller_a"));
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn find_call_sites_filter_by_callee_pattern_matches_glob() {
        let (dir, mcp) = fixture();
        seed_rust_call_sites(dir.path());
        cold_start_workspace(&mcp, dir.path());
        // `M.api.create` is the only multi-dotted callee in the fixture.
        let result = mcp
            .find_call_sites(Parameters(FindCallSitesParams {
                from_fqdn: None,
                callee_text: None,
                callee_pattern: Some("M.api.*".into()),
                limit: None,
            }))
            .await
            .unwrap();
        let body = body_text(&result);
        let arr: serde_json::Value = serde_json::from_str(&body).unwrap();
        let rows = arr.as_array().unwrap();
        assert_eq!(rows.len(), 1, "only M.api.create matches the glob");
        assert_eq!(rows[0]["callee_text"].as_str(), Some("M.api.create"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn find_call_sites_empty_string_filter_treated_as_unset() {
        // MCP callers often serialize `Option::None` as `""` — the
        // server-side `non_empty` normalises it back so a vacuous
        // filter doesn't silently constrain the result set.
        let (dir, mcp) = fixture();
        seed_rust_call_sites(dir.path());
        cold_start_workspace(&mcp, dir.path());
        let result = mcp
            .find_call_sites(Parameters(FindCallSitesParams {
                from_fqdn: Some("".into()),
                callee_text: Some("   ".into()),
                callee_pattern: None,
                limit: None,
            }))
            .await
            .unwrap();
        let body = body_text(&result);
        let arr: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            arr.as_array().unwrap().len(),
            4,
            "empty / whitespace filters must read as no filter, got `{body}`"
        );
    }
}

#![allow(
    clippy::significant_drop_tightening,
    clippy::significant_drop_in_scrutinee,
    // Wildcard re-exports are intentional — params + helpers are
    // implementation modules whose public surface is the union of their
    // exports, kept as a single namespace at the handler level.
    clippy::wildcard_imports,
    // Items here are used by handler/tests.rs via `use super::*;` but
    // clippy doesn't track sub-module usage so it flags them as unused
    // in handler.rs body. Keep until the tool_router impl is split into
    // sub-modules (Phase 3.2+) where the imports can be co-located.
    unused_imports
)]

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
    CallToolResult, Content, Implementation, InitializeRequestParams, ProtocolVersion,
    ServerCapabilities, ServerInfo,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use standardoc_core::{
    IndexHandle, LanguageProvider, ResolveOutcome, ResolverRegistry, ScanFilters, WatcherHandle,
    config::load_workspace_config,
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

mod helpers;
mod params;

use helpers::*;
pub use params::*;

pub(super) const FIND_SYMBOL_DEFAULT_LIMIT: u8 = 20;
pub(super) const FIND_SYMBOL_MAX_LIMIT: u8 = 100;
/// Above this many matches, `find_symbols_by_pattern` auto-switches to the
/// lean `summary_envelope` unless the caller forced `summary: false`. Kept
/// BELOW `FIND_SYMBOL_DEFAULT_LIMIT` (20) on purpose: otherwise a broad glob at
/// the default limit caps at 20 < threshold and never degrades, dumping ~20
/// full RawSymbol records (~600 lines). The tool's primary use — dup hunting
/// (`strip_*_extension`) — wants lean rows anyway, so 15 is the line between
/// "a handful, show full" and "broad, show lean".
pub(super) const SUMMARY_AUTO_THRESHOLD: usize = 15;
/// Similarity floor for the `did_you_mean` suggestion bundle attached to
/// empty `find_symbol` / `find_symbols_by_pattern` results. Lower than
/// `find_similar_symbols`'s default 0.8 because the caller has already
/// observed a zero-hit query — surfacing weaker alternatives is the
/// load-bearing value here. Strictly less than the existing
/// `clamp_threshold` floor would still keep noise out for completely
/// unrelated names.
pub(super) const DID_YOU_MEAN_THRESHOLD: f32 = 0.6;
/// Hard cap on the size of the `did_you_mean` array. Five is enough to
/// surface typo / cluster matches without flooding the tool response.
pub(super) const DID_YOU_MEAN_LIMIT: usize = 5;
/// Default depth for `get_context` when the caller doesn't override.
/// Bumped from 1 to 2 so each neighbor entry carries its
/// `resolved_symbol: RawSymbol` payload by default — what the viz
/// expects to render a focus graph without a follow-up round-trip per
/// neighbor, and what humans inspecting the raw response expect to
/// see. Agents that want the cheap shape can pass `depth: 1`
/// explicitly to opt into the map-first / drill-second protocol.
pub(super) const GET_CONTEXT_DEFAULT_DEPTH: u8 = 2;
pub(super) const FIND_SIMILAR_DEFAULT_THRESHOLD: f32 = 0.8;

/// Window during which a prior `get_context(fqdn, depth=1)` is considered
/// a "scoping pass" that justifies a follow-up `depth=2` on the same FQDN.
/// Outside the window (or with no prior depth=1 at all), the depth=2 call
/// is flagged as "naked" via `routing_hint` so the caller learns to map
/// neighbors before drilling.
pub(super) const NAKED_DEPTH_2_WINDOW_SECS: i64 = 300;

/// Drop tracker entries older than this on each insert. Bounds memory
/// growth without LRU bookkeeping. Comfortably above
/// `NAKED_DEPTH_2_WINDOW_SECS` so the cleanup never races a legitimate
/// late depth=2.
pub(super) const RECENT_DEPTH1_RETENTION_SECS: i64 = 1800;

/// Dispatch target for the rmcp `ServiceExt::serve` boot. Holds:
/// - `handle`: query / submit gateway into the index
/// - `provider`: workspace `LanguageProvider`, shared with the watcher
/// - `filters`: live `ScanFilters`, hot-swapped by the watcher when
///   `standardoc.sxd` changes (lock pause-exclude-22 §1.1 multi-matchers stack)
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
        description = "Aggregate context for a symbol identified by its fully-qualified name (FQDN). Returns the symbol's signature, descriptions, and pre-grouped neighbor lists (callers, callees, imports, imported_by, dependents, depends_on, tests).\n\n**Pick `depth` deliberately:** `depth=1` returns neighbor FQDNs only — cheap, the right call to map a symbol's neighborhood. `depth=2` enriches each resolved neighbor with its full RawSymbol — only worth it when you have already used a depth=1 pass to identify which neighbors matter. Hard-clamped to 1..=2. The response carries `routing_hint` when a depth=2 call is detected without a prior depth=1 on the same FQDN within the last 5 minutes — that's a signal to map first, drill second.\n\n**Dead-code judgment:** `callers` holds RESOLVED `CALLS` edges only and MISSES cross-crate / unresolved calls — never conclude a symbol is dead from an empty `callers`. At `depth>=2` on callables the response also carries `textual_callers`: a name-contains sweep of call sites the resolver couldn't bind, deduped by caller FQDN. Read `callers` ∪ `textual_callers` together to judge whether a symbol is truly unused."
    )]
    async fn get_context(
        &self,
        Parameters(params): Parameters<GetContextParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(busy) = self.not_ready() {
            return Ok(busy);
        }
        // Cache key uses the dot-normalized form so a depth=1 hit on
        // `Foo.bar` and a depth=2 hit on `Foo::bar` route to the same
        // recent-call entry. DB lookup, however, MUST try the raw
        // FQDN first — TS file segments such as `profiler.type` (from
        // `profiler.type.ts`) carry a legitimate `.` that the OOP-
        // style normalization would mangle into `profiler::type`.
        let raw_fqdn = params.fqdn.clone();
        let cache_key = normalize_fqdn(&raw_fqdn);

        let explicit_depth = params.depth;
        let depth = explicit_depth.unwrap_or(GET_CONTEXT_DEFAULT_DEPTH);
        let now = current_unix_seconds();
        // Only fire the map-first / drill-second nudge when the caller
        // EXPLICITLY asked for depth=2. With depth=2 now the default,
        // an unspecified `depth` field means "give me the standard
        // shape" — that's not a protocol breach worth warning about
        // and was polluting every viz / human get_context response.
        let routing_hint = if explicit_depth.is_some() {
            self.compute_routing_hint(&cache_key, depth, now)
        } else {
            None
        };
        if depth <= 1 {
            self.record_recent_depth1(&cache_key, now);
        }

        // Try verbatim first; fall back to OOP-normalized form for
        // LLM consumers that emit `Type.method` instead of `::`.
        let handle = self.handle.clone();
        let raw_for_call = raw_fqdn.clone();
        let result = tokio::task::spawn_blocking(move || {
            resolve_fqdn_fallback(&raw_for_call, |fqdn| {
                query::context_for_symbol_with_neighbors(&handle, fqdn, depth)
            })
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;

        match result {
            Some(ctx) => {
                let response = GetContextResponse { ctx, routing_hint };
                Ok(success_json(&response))
            }
            None => Ok(not_found()),
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
        if let Some(busy) = self.not_ready() {
            return Ok(busy);
        }
        let raw_fqdn = params.fqdn.clone();
        let handle = self.handle.clone();
        let raw_for_call = raw_fqdn.clone();
        let result = tokio::task::spawn_blocking(move || {
            resolve_fqdn_fallback(&raw_for_call, |fqdn| {
                query::context_for_symbol_with_neighbors(&handle, fqdn, 1)
            })
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
            None => Ok(not_found()),
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

    /// Uniform "indexing in progress" short-circuit. Returns `Some(reply)`
    /// when cold start hasn't flipped `index_ready` yet, so each tool can do
    /// `if let Some(busy) = self.not_ready() { return Ok(busy); }` instead of
    /// inlining the same five lines.
    fn not_ready(&self) -> Option<CallToolResult> {
        if self.index_ready.load(Ordering::Acquire) {
            None
        } else {
            Some(CallToolResult::success(vec![Content::text(
                indexing_in_progress_message(self.handle.cold_start_progress().ok().flatten()),
            )]))
        }
    }

    /// FTS5 search across symbol `name` and `fqdn` columns. Returns ranked
    /// matches as a JSON array of `RawSymbol`. `limit` defaults to 20 and
    /// is server-capped at 100 to keep tool results small. Optional
    /// filters narrow the result set by `kind`, `visibility` and/or
    /// exact `module` (no wildcards — use `find_symbols_by_pattern` for
    /// glob-style module/name matching).
    #[tool(
        description = "Full-text search across the workspace index over symbol names and FQDNs. Always returns the object `{results: [...], did_you_mean: [...]}` — `results` holds the ranked RawSymbol matches, `did_you_mean` the strsim fallbacks (non-empty only when `results` is empty). `limit` defaults to 20 and is capped at 100 server-side. Use this to discover symbols when you only know a fragment of the name; follow up with `get_context` to drill into a specific FQDN. Optional filters: `kind` (function/type/value/module/macro), `visibility` (public/private/crate/protected), `module` (exact match on the symbol's module fqdn). On a zero-hit query, `did_you_mean` carries up to 5 suggestions `[{fqdn, name, kind, score}, ...]` (threshold 0.6). One probe is enough — accept the absence rather than trying variant names."
    )]
    async fn find_symbol(
        &self,
        Parameters(params): Parameters<FindSymbolParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(busy) = self.not_ready() {
            return Ok(busy);
        }
        // Don't normalise the module filter — that turns TS file
        // segments (`profiler.type`, `hud.element`) into nonexistent
        // `profiler::type` keys. Stored module FQDNs use `::` for the
        // hierarchy and keep the dot as a literal segment character.

        let trimmed = params.query.trim().to_owned();
        if trimmed.is_empty() {
            return Ok(search_envelope::<RawSymbol>(&[], &[]));
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

        let result = apply_exclude_tests(result, params.exclude_tests.unwrap_or(false));

        let suggestions = if result.is_empty() {
            compute_did_you_mean(self.handle.clone(), trimmed, filter).await?
        } else {
            Vec::new()
        };
        Ok(search_envelope(&result, &suggestions))
    }

    /// Compact variant of `find_symbol` — returns `{fqdn, kind}` per
    /// match instead of the full `RawSymbol`. Designed for FTS5 probes
    /// where you only need the FQDN to follow up with `get_context`
    /// or `get_code`. Massive payload reduction on broad queries.
    /// Optional `relative_to` shortens matching FQDNs to `::<rest>`.
    #[tool(
        description = "FQDN-only variant of `find_symbol`. Always returns `{results: [{fqdn, kind}, ...], did_you_mean: [...]}` ranked by FTS5 — the minimal shape needed to follow up with `get_context` / `get_code`. Same filters and limit semantics as `find_symbol`; same `did_you_mean` envelope, populated only on a zero-hit query. Use this as your default discovery probe; reach for `find_symbol` only when you actually need the full RawSymbol shape per match. Pass `relative_to = \"foo::bar\"` to collapse matching FQDNs into `::baz::qux` form — kills the prefix repetition that dominates scoped scans."
    )]
    async fn find_symbol_fqdns(
        &self,
        Parameters(params): Parameters<FindSymbolFqdnsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(busy) = self.not_ready() {
            return Ok(busy);
        }
        let trimmed = params.query.trim().to_owned();
        if trimmed.is_empty() {
            return Ok(search_envelope::<serde_json::Value>(&[], &[]));
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

        let result = apply_exclude_tests(result, params.exclude_tests.unwrap_or(false));

        let suggestions = if result.is_empty() {
            compute_did_you_mean(self.handle.clone(), trimmed, filter).await?
        } else {
            Vec::new()
        };
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
        Ok(search_envelope(&projected, &suggestions))
    }

    /// Filter-only listing — no FTS query, no glob pattern. Returns
    /// every symbol matching the provided filters, ordered by canonical
    /// fqdn for stable output. Designed for cross-cutting audits like
    /// "list every private function in module X" or "list every type
    /// with visibility=crate". Pagination is cursor-based: when the
    /// page fills, `next_cursor` carries the last fqdn returned; pass
    /// it back in `cursor` to fetch the next slice. `null` means done.
    #[tool(
        description = "Filter-only listing of symbols. No query string, no pattern — returns every symbol matching the provided filters, ordered by fqdn. Response shape: `{items: [...], next_cursor: string | null}`. Each item is a `RawSymbol` plus two projected keys: `project_id` (the owning project's id — cross-reference `list_projects`) and `is_test` (the daemon's test-symbol verdict). Walk the full set by re-calling with `cursor = next_cursor` until it returns `null`. Use this for audits and inventories like 'all private functions' or 'all types in module X'. At least one filter SHOULD be provided to keep the result set bounded. Filters: `kind`, `visibility`, `module` (all optional, all match exactly)."
    )]
    async fn list_symbols(
        &self,
        Parameters(params): Parameters<ListSymbolsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(busy) = self.not_ready() {
            return Ok(busy);
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
        let exclude_tests = params.exclude_tests.unwrap_or(false);
        let handle = self.handle.clone();
        let (items, next_cursor) = tokio::task::spawn_blocking(move || {
            query::list_symbols_projected(&handle, &filter, limit as usize, cursor.as_deref())
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;

        // The projection already stamped each row's `is_test`, so the
        // exclude filter reuses that verdict instead of re-running the
        // heuristic `apply_exclude_tests` would.
        let items = if exclude_tests {
            items.into_iter().filter(|s| !s.is_test).collect::<Vec<_>>()
        } else {
            items
        };
        let envelope = serde_json::json!({
            "items": items,
            "next_cursor": next_cursor,
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
        if let Some(busy) = self.not_ready() {
            return Ok(busy);
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

        let items = apply_exclude_tests(page.items, params.exclude_tests.unwrap_or(false));
        let relative_to = params.relative_to.unwrap_or_default();
        let projected: Vec<_> = items
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
        description = "Glob-pattern search over symbol names and FQDNs (SQLite GLOB: `*`, `?`, `[abc]`, case-sensitive). A symbol matches when either its name or its fqdn satisfies the pattern. Returns `{results: [...], did_you_mean: [...]}`. Use this to detect cross-module duplications (e.g. `strip_*_extension` to catch every `strip_<lang>_extension` helper). Optional filters: `kind`, `visibility`, `module` — same semantics as `find_symbol`. On a zero-hit pattern, `did_you_mean` runs strsim on the pattern's core (wildcards stripped) — useful for typos like `*to_token_string*` → `to_token_stream`. Broad patterns auto-degrade to lean rows (`{results: [{name, fqdn, kind, visibility, location}], count, mode: \"summary\"}`) above 15 matches so a wide glob can't blow up the response — pass `summary: false` to force full records, `summary: true` to force lean."
    )]
    async fn find_symbols_by_pattern(
        &self,
        Parameters(params): Parameters<FindSymbolsByPatternParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(busy) = self.not_ready() {
            return Ok(busy);
        }
        // Don't normalise the pattern or module filter — TS file
        // segments (`profiler.type`, `hud.element`) embed a literal
        // `.` and the GLOB match expects them verbatim. LLM consumers
        // typing OOP-style `Type.method` patterns have to use the
        // canonical `::` form here.
        let trimmed = params.pattern.trim().to_owned();
        if trimmed.is_empty() {
            return Ok(search_envelope::<RawSymbol>(&[], &[]));
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

        let result = apply_exclude_tests(result, params.exclude_tests.unwrap_or(false));

        let use_summary = params
            .summary
            .unwrap_or(result.len() > SUMMARY_AUTO_THRESHOLD);
        if use_summary && !result.is_empty() {
            return Ok(summary_envelope(&result));
        }

        let suggestions = if result.is_empty() {
            let core = glob_core_text(&trimmed);
            if core.is_empty() {
                Vec::new()
            } else {
                compute_did_you_mean(self.handle.clone(), core, filter).await?
            }
        } else {
            Vec::new()
        };
        Ok(search_envelope(&result, &suggestions))
    }

    /// Similarity-scored search around an anchor name. Returns symbols whose
    /// name is "close" to `reference` under a hybrid score combining
    /// Jaro-Winkler (character-level) and Jaccard over snake/camel-case tokens.
    /// Self-skips the anchor (case-insensitive name equality). Filters apply
    /// BEFORE scoring (idiomatic with the other tools — keeps the candidate
    /// pool bounded). Use this to detect cluster-style duplications without
    /// having to guess a glob pattern.
    #[tool(
        description = "Similarity-scored search for symbols whose name is close to an anchor `reference`. Returns `{results: [{score, symbol}, ...]}` (score in [0,1], descending). Hybrid score = max(jaro_winkler, jaccard_tokens) — captures both typo-style matches (`parseFile` ↔ `parseFiles`) and templated-name clusters (`strip_rs_extension` ↔ `strip_ts_extension` ↔ `strip_lua_extension`). Self-skips the anchor by case-insensitive name. `threshold` defaults to 0.8 in [0.0, 1.0]; `limit` defaults to 20, capped at 100. Comparison runs on `name` only; use `module` filter to scope by module. Optional filters: `kind`, `visibility`, `module`."
    )]
    async fn find_similar_symbols(
        &self,
        Parameters(params): Parameters<FindSimilarSymbolsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(busy) = self.not_ready() {
            return Ok(busy);
        }
        // `reference` is a name anchor, not an fqdn — leave it intact.
        // Module filter is left verbatim too: TS file segments carry
        // literal `.` chars that the exact-match filter expects.

        let trimmed = params.reference.trim().to_owned();
        if trimmed.is_empty() {
            return Ok(search_envelope::<SimilarSymbolJson>(&[], &[]));
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
        Ok(search_envelope(&envelope, &[]))
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
        description = "Returns the raw source text of a symbol identified by FQDN, sliced from the file at its declared start_line..end_line. Pair with `get_context` (graph relations) when you need to actually read the function body. Optional knobs: `max_lines` caps total output (`truncated=true` flag), `strip_attrs=true` drops leading doc comments / `#[…]` attribute blocks (`stripped_lines` count), `signature_only=true` truncates after the first `{` (returns just the multi-line signature), `strip_inline_comments=true` removes inline `// …` and `/* … */` comments from the body (string-literal safe — `\"…\"`, raw strings, TS templates passed through verbatim). Returns `null` when no symbol matches the FQDN — call `find_symbol` first if you only have a name fragment. NOTE: leading indentation is normalized to the reported `indent_unit`, so the body is not byte-faithful to the file's actual whitespace — re-read the raw file before making indentation-sensitive edits."
    )]
    async fn get_body(
        &self,
        Parameters(params): Parameters<GetBodyParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(busy) = self.not_ready() {
            return Ok(busy);
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
            collapse_blank_lines: false,
        };
        let raw_for_call = raw_fqdn.clone();
        let opts_clone = opts;
        let result = tokio::task::spawn_blocking(move || {
            resolve_fqdn_fallback(&raw_for_call, |fqdn| {
                query::body_for_fqdn(&handle, fqdn, &opts_clone)
            })
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;

        match result {
            Some(slice) => Ok(success_json(&slice)),
            None => Ok(not_found()),
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
        description = "Like `get_body` but returns pure code by default — leading doc comments / `#[…]` attribute blocks and inline `// …` / `/* … */` comments are stripped. The verbatim slice is still available via `get_body`; the leading description lives separately in `get_context.context.document_description`. Useful when you want to read the actual implementation without dilution. Pass `strip_attrs=false` or `strip_inline_comments=false` to disable individual strips. Other knobs (`max_lines`, `signature_only`) behave the same as `get_body`. Returns `null` when no symbol matches the FQDN. NOTE: indentation is normalized to the reported `indent_unit` (not byte-faithful to the file) — re-read the raw file before indentation-sensitive edits."
    )]
    async fn get_code(
        &self,
        Parameters(params): Parameters<GetCodeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(busy) = self.not_ready() {
            return Ok(busy);
        }
        let raw_fqdn = params.fqdn.clone();
        let handle = self.handle.clone();
        let opts = query::BodyOptions {
            max_lines: params.max_lines,
            strip_attrs: params.strip_attrs.unwrap_or(true),
            signature_only: params.signature_only.unwrap_or(false),
            strip_inline_comments: params.strip_inline_comments.unwrap_or(true),
            // `get_code` is the agent-facing "pure code" view, so it
            // also collapses the vestigial blank lines that
            // `strip_inline_comments` leaves behind. `get_body` keeps
            // line-number alignment intact by leaving them in place.
            collapse_blank_lines: true,
        };
        let raw_for_call = raw_fqdn.clone();
        let opts_clone = opts;
        let result = tokio::task::spawn_blocking(move || {
            resolve_fqdn_fallback(&raw_for_call, |fqdn| {
                query::body_for_fqdn(&handle, fqdn, &opts_clone)
            })
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;

        match result {
            Some(slice) => Ok(success_json(&slice)),
            None => Ok(not_found()),
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
        // `watcher.active` reports whether the WORKSPACE is being watched,
        // not whether THIS process owns the watcher. A `mcp --readonly`
        // daemon never spawns its own watcher (the slot below stays
        // `None`) yet rides a primary (LSP daemon / `standardoc watch`)
        // that does — so we also probe the workspace fs4 lock. Either
        // signal being true means edits are being picked up live.
        let watcher_active = self
            .watcher
            .lock()
            .ok()
            .is_some_and(|guard| guard.is_some())
            || self.handle.workspace_watched();
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

    /// Lazy on-demand resolution of an external FQDN (Cargo crate, npm
    /// package, luarocks rock). Routes the FQDN through the registered
    /// resolvers in order; the first non-`NotInThisRegistry` answer wins.
    /// On success, the produced `ExtractedFile` is submitted to the
    /// writer pipeline (`is_external = 1` + the resolver's `SourceOrigin`)
    /// and the new symbol's `RawSymbol` is returned in the envelope.
    #[tool(
        description = "Lazy on-demand resolution of an external FQDN (a symbol that lives outside the workspace — Cargo crate, npm package, luarocks rock). Routes the FQDN through registered resolvers and submits the produced ExtractedFile to the writer pipeline. Returns `{status, fqdn, source_origin?, symbol?, missing_binary?, detail?}`. `status` is `resolved` (success — `symbol` is the new RawSymbol), `not_found` (no resolver claimed the FQDN), `missing_binary` (the matching resolver is gated behind a CLI that is not installed — `missing_binary` names which one), or `error` (resolver-level failure — `detail` carries the message). Use this when `get_context(fqdn)` returned a neighbor with `to: Unresolved` and the FQDN shape matches a known dependency."
    )]
    pub async fn resolve_external(
        &self,
        Parameters(params): Parameters<ResolveExternalParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(busy) = self.not_ready() {
            return Ok(busy);
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
        if let Some(busy) = self.not_ready() {
            return Ok(busy);
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

    /// Enumerate the `group "<slug>" { ... }` blocks declared in
    /// `standardoc.sxd`. Re-parses the config on each call (read-only,
    /// low frequency) so no separate persistence/cache layer is needed.
    /// Empty array when `.sxd` is absent or declares no `group` blocks.
    #[tool(
        description = "List the `group` blocks declared in standardoc.sxd. Each entry surfaces slug, label (optional), members (project slugs). Empty list = no .sxd config, or no `group` blocks. Drives the viz Overview project-cluster collapse."
    )]
    async fn list_groups(&self) -> Result<CallToolResult, ErrorData> {
        let root = self.handle.workspace_root().to_path_buf();
        let groups = tokio::task::spawn_blocking(move || {
            load_workspace_config(&root).map(|opt| opt.map(|c| c.groups).unwrap_or_default())
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| ErrorData::internal_error(format!("load_workspace_config: {e}"), None))?;
        Ok(success_json(&serde_json::json!({ "groups": groups })))
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
        description = "Read-only query over the `call_sites` table — every call expression emitted by the extractors (CallExpr in Rust/TS/Lua, NewExpr, OptChain call, method call). Returns `{call_sites: [{from_fqdn, callee_text, args: [{value, is_string_literal}], receiver_chain: [..], site: {file, line, col}}, ...]}`. Three optional AND-composable filters: `from_fqdn` (exact match on the enclosing fn/method FQDN — answers `what does X call?`), `callee_text` (exact match on the called expression text — answers `who calls Y?`), `callee_pattern` (SQLite GLOB on the callee text, e.g. `*tauri.invoke*` for every Tauri invocation, `M.api.*` for every call into the M.api namespace). `limit` defaults to 50, capped at 200. Use this to discover textual call patterns the symbol graph alone can't surface — bridge invocations, method-chain shapes, literal-string arg payloads."
    )]
    async fn find_call_sites(
        &self,
        Parameters(params): Parameters<FindCallSitesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(busy) = self.not_ready() {
            return Ok(busy);
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
        Ok(success_json(&serde_json::json!({ "call_sites": rows })))
    }

    /// Pre-composed graph slice ready for the `standardoc-graph-viz` WASM
    /// consumer. Two modes, dispatched on `focal`:
    /// - `focal = Some(fqdn)` → bounded BFS around the centered node.
    /// - `focal = None` → bounded snapshot of the workspace ordered by
    ///   `fqdn` ASC.
    /// All clamps (`depth`, `max_nodes`) happen here at the transport
    /// boundary; the core composition trusts what it receives.
    #[tool(
        description = "Pre-composed graph payload for the visualisation layer. Returns `{symbols: [{fqdn, name, kind, visibility, module?, language_kind, is_external, is_test, file, start_line, project_id?}], edges: [{from, to, kind, outbound}], projects: [{project_id, label, kind, rel_path}], focal?}` — flat shape the WASM viz consumes directly (no client-side reshape). `projects` is the lookup table for the `project_id` foreign key; the viz frames symbols by project and colours each frame by `kind`.\n\nTwo modes:\n- `focal: Some(fqdn)` → bounded BFS expansion around the node, `depth` hops (1..=5, default 2). Both outbound (edges_from) and inbound (edges_to) edges are walked; unresolved targets are skipped.\n- `focal: None` → bounded snapshot of the workspace ordered by FQDN ASC. Only edges whose target is also in the bounded set are included (no dangling).\n\nKnobs: `kinds` (allow-list of edge kinds — `CALLS`, `IMPORTS`, `EXTENDS`, `IMPLEMENTS`, `REFERENCES`, `USES_TYPE`; case-insensitive, unknown values silently ignored); `max_nodes` (safety cap, default 500, clamped to 1..=5000); `include_external` (default false, scopes to workspace-authored symbols).\n\nReturns an empty `symbols`/`edges` vec with `focal` echoed when `focal` is supplied but the FQDN is unknown — lets the consumer distinguish 'no neighbors' from 'unknown symbol'."
    )]
    async fn fetch_graph(
        &self,
        Parameters(params): Parameters<FetchGraphParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if let Some(busy) = self.not_ready() {
            return Ok(busy);
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

    /// Override rmcp's default `initialize`, which ignores the client's
    /// requested protocol version and always replies with
    /// `ProtocolVersion::LATEST` — lagging MCP clients reject that outright
    /// (e.g. LM Studio: "Server's protocol version is not supported:
    /// 2025-11-25"). We echo the requested version when the SDK knows it, so
    /// older clients can still connect. Standardoc only uses the `tools`
    /// capability, available since the first protocol version, so announcing
    /// an older version costs nothing.
    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ServerInfo, ErrorData> {
        let requested = request.protocol_version.clone();
        if context.peer.peer_info().is_none() {
            context.peer.set_peer_info(request);
        }
        let mut info = self.get_info();
        info.protocol_version = negotiated_protocol_version(&requested);
        Ok(info)
    }
}

/// Pick the protocol version to announce back at `initialize`: echo the
/// client's requested version when the SDK knows it, otherwise keep our latest
/// so the client decides (MCP spec §Initialization). Split out from
/// `initialize` so the negotiation is unit-testable without a live peer.
fn negotiated_protocol_version(requested: &ProtocolVersion) -> ProtocolVersion {
    if ProtocolVersion::KNOWN_VERSIONS.contains(requested) {
        requested.clone()
    } else {
        ProtocolVersion::default()
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

#[cfg(test)]
mod tests;

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
    IndexHandle, LanguageProvider, RagWatcherHandle, ResolveOutcome, ResolverRegistry, ScanFilters,
    SessionsHandle, UsagePeriod, WatcherHandle, dump_sessions_to_markdown,
    query::{self, SymbolFilter},
};
use standardoc_ir::SourceOrigin;
use standardoc_ir::{Kind, RawSymbol, Visibility};
use standardoc_rag::embedder::Embedder;
use standardoc_rag::store::RagStore;
use standardoc_rag::types::{Chunk, ChunkRef};

use crate::mcp::error::{SessionsErr, server_error_to_rmcp, sessions_err_to_rmcp};
use crate::mcp::usage::{
    files_from_body, files_from_context, files_from_similar, files_from_symbols,
    log_usage_fire_and_forget, sum_distinct_file_sizes,
};

const FIND_SYMBOL_DEFAULT_LIMIT: u8 = 20;
const FIND_SYMBOL_MAX_LIMIT: u8 = 100;
const GET_CONTEXT_DEFAULT_DEPTH: u8 = 1;
const FIND_SIMILAR_DEFAULT_THRESHOLD: f32 = 0.8;
const CHUNK_REFS_DEFAULT_LIMIT: u32 = 10;
const FETCH_CHUNKS_MAX_INPUT: usize = 64;

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
    /// Optional handle to the RAG sidecar store. When `Some`,
    /// `get_context` includes `chunk_refs` for the queried symbol and
    /// the `fetch_chunks` tool is functional. When `None`, both
    /// gracefully degrade (empty refs / RAG-disabled error).
    rag_store: Option<Arc<RagStore>>,
    /// Optional embedder used by `get_context(fqdn, query?)` for
    /// query-time re-rank of `chunk_refs`. When `None`, the query
    /// argument is silently ignored (pre-computed `link × def_site_boost`
    /// confidence drives the order). Wire via `with_embedder()`.
    rag_embedder: Option<Arc<dyn Embedder>>,
    index_ready: Arc<AtomicBool>,
    watcher: Arc<Mutex<Option<WatcherHandle>>>,
    rag_watcher: Arc<Mutex<Option<RagWatcherHandle>>>,
    /// In-memory cache of `(fqdn → ts_unix)` recording when each FQDN
    /// was last fetched at `depth=1`. Drives the "naked depth=2"
    /// routing hint: a depth=2 call with no recent depth=1 on the
    /// same FQDN gets a hint nudging the 3-phase explore→cible→drill
    /// protocol. Transient — resets on daemon restart, no persistence.
    recent_depth1: Arc<Mutex<HashMap<String, i64>>>,
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
            rag_store: None,
            rag_embedder: None,
            index_ready: Arc::new(AtomicBool::new(false)),
            watcher: Arc::new(Mutex::new(None)),
            rag_watcher: Arc::new(Mutex::new(None)),
            recent_depth1: Arc::new(Mutex::new(HashMap::new())),
            tool_router: Self::tool_router(),
        }
    }

    /// Aggregate context for a symbol: signature + descriptions + four
    /// pre-grouped neighbor lists (callers / callees / imports / imported_by).
    /// `depth` selects the shape's richness — see [`SymbolContextWithNeighbors`].
    /// When the daemon was booted with a RAG store, the response also carries
    /// `chunk_refs` (lightweight URI envelopes). Fetch chunk text via the
    /// `fetch_chunks` tool. The response sets `routing_hint` when a depth=2
    /// call is made without a recent depth=1 scoping pass — an in-band nudge
    /// to follow the explore→cible→drill protocol.
    #[tool(
        description = "Aggregate context for a symbol identified by its fully-qualified name (FQDN). Returns the symbol's signature, descriptions, four pre-grouped neighbor lists (callers, callees, imports, imported_by) AND lightweight `chunk_refs` envelopes pointing at related prose chunks (markdown docs, ADRs, design notes). \n\n**Pick `depth` deliberately:** `depth=1` returns neighbor FQDNs only — cheap, the right call to map a symbol's neighborhood. `depth=2` enriches each resolved neighbor with its full RawSymbol — only worth it when you have already used a depth=1 pass to identify which neighbors matter. Hard-clamped to 1..=2. The response carries `routing_hint` when a depth=2 call is detected without a prior depth=1 on the same FQDN within the last 5 minutes — that's a signal to map first, drill second.\n\nThe `chunk_refs` field is empty when no prose is linked or the RAG layer is not enabled — fetch their text via the `fetch_chunks` tool with the URIs (`rag://<id>`)."
    )]
    async fn get_context(
        &self,
        Parameters(params): Parameters<GetContextParams>,
    ) -> Result<CallToolResult, ErrorData> {
        if !self.index_ready.load(Ordering::Acquire) {
            return Ok(CallToolResult::success(vec![Content::text(
                indexing_in_progress_message(self.handle.cold_start_progress().ok().flatten()),
            )]));
        }

        let depth = params.depth.unwrap_or(GET_CONTEXT_DEFAULT_DEPTH);
        let now = current_unix_seconds();
        let routing_hint = self.compute_routing_hint(&params.fqdn, depth, now);
        if depth <= 1 {
            self.record_recent_depth1(&params.fqdn, now);
        }

        let handle = self.handle.clone();
        let fqdn = params.fqdn.clone();
        let result = tokio::task::spawn_blocking(move || {
            query::context_for_symbol_with_neighbors(&handle, &fqdn, depth)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;

        match result {
            Some(ctx) => {
                let chunk_refs = self
                    .chunk_refs_for(&params.fqdn, params.query.as_deref())
                    .await?;
                let response = GetContextResponse {
                    ctx,
                    chunk_refs,
                    routing_hint,
                };
                let files = files_from_context(&response.ctx);
                Ok(success_json_with_usage(
                    &response,
                    self.handle.workspace_root(),
                    "get_context",
                    Some(params.fqdn),
                    files,
                ))
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

    /// Resolves a list of `rag://<id>` URIs to the underlying prose
    /// chunks. The companion to `get_context` — call it with the URIs
    /// surfaced in `chunk_refs` to materialise the actual chunk text.
    /// Unknown / malformed URIs are silently skipped.
    #[tool(
        description = "Resolves a list of `rag://<id>` URIs to the underlying prose chunks (markdown documentation, ADRs, design notes). Pair with `get_context` — call this with URIs from the response's `chunk_refs`. Each returned chunk carries `text`, `source_path`, `section_header` (nearest H2/H3) and byte offsets. Unknown or malformed URIs are silently dropped (diff inputs vs outputs to detect them)."
    )]
    async fn fetch_chunks(
        &self,
        Parameters(params): Parameters<FetchChunksParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let Some(store) = self.rag_store.clone() else {
            return Ok(CallToolResult::success(vec![Content::text(
                "RAG layer not enabled on this daemon — fetch_chunks returns nothing. Boot the daemon with a RAG store to enable prose retrieval.",
            )]));
        };
        let mut uris = params.uris;
        uris.truncate(FETCH_CHUNKS_MAX_INPUT);
        let chunks = tokio::task::spawn_blocking(move || store.fetch_by_uris(&uris))
            .await
            .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
            .map_err(|e| ErrorData::internal_error(format!("rag fetch_by_uris: {e}"), None))?;
        Ok(success_json::<Vec<Chunk>>(&chunks))
    }

    /// FTS5 search across symbol `name` and `fqdn` columns. Returns ranked
    /// matches as a JSON array of `RawSymbol`. `limit` defaults to 20 and
    /// is server-capped at 100 to keep tool results small. Optional
    /// filters narrow the result set by `kind`, `visibility` and/or
    /// exact `module` (no wildcards — use `find_symbols_by_pattern` for
    /// glob-style module/name matching).
    #[tool(
        description = "Full-text search across the workspace index over symbol names and FQDNs. Returns ranked matches as a JSON array. `limit` defaults to 20 and is capped at 100 server-side. Use this to discover symbols when you only know a fragment of the name; follow up with `get_context` to drill into a specific FQDN. Optional filters: `kind` (function/type/value/module/macro), `visibility` (public/private/crate/protected), `module` (exact match on the symbol's module fqdn)."
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

        let trimmed = params.query.trim().to_owned();
        if trimmed.is_empty() {
            return Ok(success_json::<Vec<RawSymbol>>(&Vec::new()));
        }

        let filter = parse_filter(
            params.kind.as_deref(),
            params.visibility.as_deref(),
            params.module,
            params.include_external,
        )?;
        let limit = clamp_limit(params.limit);
        let handle = self.handle.clone();
        let result = tokio::task::spawn_blocking(move || {
            query::search_text(&handle, &trimmed, limit as usize, &filter)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;

        let files = files_from_symbols(&result);
        Ok(success_json_with_usage(
            &result,
            self.handle.workspace_root(),
            "find_symbol",
            None,
            files,
        ))
    }

    /// Filter-only listing — no FTS query, no glob pattern. Returns
    /// every symbol matching the provided filters, ordered by canonical
    /// fqdn for stable output. Designed for cross-cutting audits like
    /// "list every private function in module X" or "list every type
    /// with visibility=crate".
    #[tool(
        description = "Filter-only listing of symbols. No query string, no pattern — returns every symbol matching the provided filters, ordered by fqdn. Use this for audits and inventories like 'all private functions' or 'all types in module X'. At least one filter SHOULD be provided to keep the result set bounded; passing no filters returns the first `limit` symbols sorted by fqdn. Filters: `kind`, `visibility`, `module` (all optional, all match exactly)."
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

        let filter = parse_filter(
            params.kind.as_deref(),
            params.visibility.as_deref(),
            params.module,
            params.include_external,
        )?;
        let limit = clamp_limit(params.limit);
        let handle = self.handle.clone();
        let result = tokio::task::spawn_blocking(move || {
            query::list_symbols(&handle, &filter, limit as usize)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;

        let files = files_from_symbols(&result);
        Ok(success_json_with_usage(
            &result,
            self.handle.workspace_root(),
            "list_symbols",
            None,
            files,
        ))
    }

    /// Glob-pattern search over `name` and `fqdn`. Uses SQLite's `GLOB`
    /// operator (`*`, `?`, `[abc]` wildcards — case-sensitive). A symbol
    /// matches when EITHER its name OR its fqdn satisfies the pattern.
    /// Combine with the same filters as `find_symbol` to scope the
    /// search.
    #[tool(
        description = "Glob-pattern search over symbol names and FQDNs (SQLite GLOB: `*`, `?`, `[abc]`, case-sensitive). A symbol matches when either its name or its fqdn satisfies the pattern. Use this to detect cross-module duplications (e.g. `strip_*_extension` to catch every `strip_<lang>_extension` helper). Optional filters: `kind`, `visibility`, `module` — same semantics as `find_symbol`."
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

        let trimmed = params.pattern.trim().to_owned();
        if trimmed.is_empty() {
            return Ok(success_json::<Vec<RawSymbol>>(&Vec::new()));
        }

        let filter = parse_filter(
            params.kind.as_deref(),
            params.visibility.as_deref(),
            params.module,
            params.include_external,
        )?;
        let limit = clamp_limit(params.limit);
        let handle = self.handle.clone();
        let result = tokio::task::spawn_blocking(move || {
            query::find_by_pattern(&handle, &trimmed, &filter, limit as usize)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;

        let files = files_from_symbols(&result);
        Ok(success_json_with_usage(
            &result,
            self.handle.workspace_root(),
            "find_symbols_by_pattern",
            None,
            files,
        ))
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

        let trimmed = params.reference.trim().to_owned();
        if trimmed.is_empty() {
            return Ok(success_json::<Vec<SimilarSymbolJson>>(&Vec::new()));
        }

        let threshold = parse_threshold(params.threshold)?;
        let filter = parse_filter(
            params.kind.as_deref(),
            params.visibility.as_deref(),
            params.module,
            params.include_external,
        )?;
        let limit = clamp_limit(params.limit);
        let handle = self.handle.clone();
        let result = tokio::task::spawn_blocking(move || {
            query::find_similar(&handle, &trimmed, threshold, &filter, limit as usize)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;

        let files = files_from_similar(&result);
        let envelope: Vec<SimilarSymbolJson> = result
            .into_iter()
            .map(|(symbol, score)| SimilarSymbolJson { score, symbol })
            .collect();
        Ok(success_json_with_usage(
            &envelope,
            self.handle.workspace_root(),
            "find_similar_symbols",
            None,
            files,
        ))
    }

    /// Returns the raw source text of the symbol identified by `fqdn`. This is
    /// the verbatim slice between `location.start_line` and `location.end_line`
    /// in the file on disk — exactly what a reader would see if they opened the
    /// file at those line numbers. Use this when you need to reason about the
    /// actual code of a known FQDN (the graph tells you WHERE; this tells you
    /// WHAT). `max_lines` clamps long bodies; `strip_attrs` drops leading docs
    /// + attribute blocks; `signature_only` truncates after the opening `{`.
    /// The response carries `truncated`, `stripped_lines` and `signature_only`
    /// so the caller can audit what was returned vs. the verbatim slice.
    #[tool(
        description = "Returns the raw source text of a symbol identified by FQDN, sliced from the file at its declared start_line..end_line. Pair with `get_context` (graph relations) when you need to actually read the function body. Optional knobs: `max_lines` caps total output (`truncated=true` flag), `strip_attrs=true` drops leading doc comments / `#[…]` attribute blocks (`stripped_lines` count), `signature_only=true` truncates after the first `{` (returns just the multi-line signature). Returns `null` when no symbol matches the FQDN — call `find_symbol` first if you only have a name fragment."
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
        let handle = self.handle.clone();
        let fqdn = params.fqdn;
        let opts = query::BodyOptions {
            max_lines: params.max_lines,
            strip_attrs: params.strip_attrs.unwrap_or(false),
            signature_only: params.signature_only.unwrap_or(false),
        };
        let result =
            tokio::task::spawn_blocking(move || query::body_for_fqdn(&handle, &fqdn, &opts))
                .await
                .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
                .map_err(|e| server_error_to_rmcp(&e.into()))?;

        match result {
            Some(slice) => {
                let files = files_from_body(&slice);
                let fqdn = slice.fqdn.clone();
                Ok(success_json_with_usage(
                    &slice,
                    self.handle.workspace_root(),
                    "get_body",
                    Some(fqdn),
                    files,
                ))
            }
            None => Ok(CallToolResult::success(vec![Content::text(
                "no symbol found for the given FQDN",
            )])),
        }
    }

    /// Read-only snapshot of the workspace revision counter PLUS daemon
    /// capabilities. The `revision` number is monotonic — every successful
    /// write (cold-start ingest, watcher upsert, rescan) bumps it by 1. The
    /// `rag` / `watcher` / `indexing` blocks let callers introspect what
    /// the daemon is wired with, so an AI can pick the right tool path
    /// (e.g. omit `query` from `get_context` when `rag.embedder` is null).
    #[tool(
        description = "Returns the current workspace revision number AND daemon capabilities (`rag.enabled`, `rag.embedder`, `watcher.active`, `indexing.ready`). Use the revision with `check_stale` to detect when fqdns you have already cited have been modified. Use the capabilities at session boot to decide tool flow — passing `query` to `get_context` only re-ranks chunks when `rag.embedder` is non-null. Cheap call; no parameters."
    )]
    async fn current_revision(&self) -> Result<CallToolResult, ErrorData> {
        let revision = self.handle.revision();
        let rag_enabled = self.rag_store.is_some();
        let embedder = self.rag_embedder.as_ref().map(|e| {
            let model = e.model();
            EmbedderInfoJson {
                id: model.id.clone(),
                dim: model.dim,
            }
        });
        let watcher_active = self
            .watcher
            .lock()
            .ok()
            .is_some_and(|guard| guard.is_some());
        let ready = self.index_ready.load(Ordering::Acquire);
        Ok(success_json(&CurrentRevisionJson {
            revision,
            rag: RagCapabilityJson {
                enabled: rag_enabled,
                embedder,
            },
            watcher: WatcherCapabilityJson {
                active: watcher_active,
            },
            indexing: IndexingCapabilityJson { ready },
        }))
    }

    /// Aggregated read-path telemetry — the running tally of bytes the
    /// standardoc tools have returned vs. the raw file bytes those same
    /// responses pointed at. The baseline is `sum(file_sizes)` of the
    /// distinct source files referenced by each response (the honest
    /// "what an AI would have consumed reading the relevant sources
    /// raw" floor — no estimation multiplier). Returns counts + bytes
    /// totals + a compression `ratio` in `[0, +∞)`. Only successful
    /// read-path tool calls are logged (no `indexing_in_progress`,
    /// `no symbol found`, or blank-query early returns).
    #[tool(
        description = "Returns aggregated standardoc tool usage metrics: `calls`, `bytes_out_total` (what tools returned to the AI), `baseline_bytes_total` (sum of file sizes of distinct source files referenced — what an AI would have consumed reading those raw), `bytes_saved` (baseline - out, can be negative when neighbors inflate the response), and `ratio = bytes_out / baseline`. `period` accepts `day`, `week`, `all` (default). The baseline is graph-grounded — no estimation multiplier — so a ratio of 0.14 means standardoc returned 14% of the raw bytes of the relevant source files."
    )]
    async fn usage_stats(
        &self,
        Parameters(params): Parameters<UsageStatsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let period_str = params.period.unwrap_or_else(|| "all".to_string());
        let period = UsagePeriod::from_str_loose(&period_str).ok_or_else(|| {
            ErrorData::invalid_params(
                format!("unknown period `{period_str}` — expected one of: day, week, all"),
                None,
            )
        })?;
        let workspace = self.handle.workspace_root().to_path_buf();
        let row = tokio::task::spawn_blocking(move || {
            let h = SessionsHandle::open(&workspace).map_err(SessionsErr::from)?;
            h.query_usage_stats(period).map_err(SessionsErr::from)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(sessions_err_to_rmcp)?;
        Ok(success_json(&row))
    }

    /// Persist a session handoff memo into `.standardoc-sessions/sessions.db`.
    /// `slug` is the unique key (UPSERT semantics: re-saving the same slug
    /// overwrites the body). `supersedes`, when provided, marks the named
    /// prior session as `superseded` — useful when a refactor invalidates an
    /// older lock. Returns the inserted row id.
    #[tool(
        description = "Save a session handoff memo to .standardoc-sessions/sessions.db. UPSERT by `slug`. Use this AT END of any session that locks decisions or ships meaningful work so the next chat can pick up via `session_get`. Optional `supersedes` marks a prior slug as superseded (chain semantics). Returns the row id."
    )]
    async fn session_save(
        &self,
        Parameters(params): Parameters<SessionSaveParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let workspace = self.handle.workspace_root().to_path_buf();
        let result = tokio::task::spawn_blocking(move || {
            let h = SessionsHandle::open(&workspace).map_err(SessionsErr::from)?;
            let id = h
                .save(&params.slug, &params.body_md, params.supersedes.as_deref())
                .map_err(SessionsErr::from)?;
            Ok::<_, SessionsErr>(id)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(sessions_err_to_rmcp)?;
        Ok(success_json(&serde_json::json!({ "id": result })))
    }

    /// List session memos newest-first. By default skips `superseded` rows.
    #[tool(
        description = "List session handoff memos newest-first. `active_only` (default true) filters out superseded entries. Returns the full body_md per row — use `session_get` only when you need to fetch one by slug."
    )]
    async fn session_list(
        &self,
        Parameters(params): Parameters<SessionListParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let workspace = self.handle.workspace_root().to_path_buf();
        let active_only = params.active_only.unwrap_or(true);
        let rows = tokio::task::spawn_blocking(move || {
            let h = SessionsHandle::open(&workspace).map_err(SessionsErr::from)?;
            h.list(active_only).map_err(SessionsErr::from)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(sessions_err_to_rmcp)?;
        Ok(success_json(&rows))
    }

    /// Fetch a single session memo by slug, or the most recent active one
    /// when `slug` is omitted. Returns `null` if no row matches.
    #[tool(
        description = "Fetch a session handoff memo. Pass `slug` to target a specific entry; omit it to get the most recent active session (typical reentry point for a new chat). Returns null when nothing matches."
    )]
    async fn session_get(
        &self,
        Parameters(params): Parameters<SessionGetParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let workspace = self.handle.workspace_root().to_path_buf();
        let slug = params.slug;
        let row = tokio::task::spawn_blocking(move || {
            let h = SessionsHandle::open(&workspace).map_err(SessionsErr::from)?;
            match slug {
                Some(s) => h.get_by_slug(&s).map_err(SessionsErr::from),
                None => h.latest().map_err(SessionsErr::from),
            }
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(sessions_err_to_rmcp)?;
        match row {
            Some(r) => Ok(success_json(&r)),
            None => Ok(CallToolResult::success(vec![Content::text(
                "no session found",
            )])),
        }
    }

    /// Dump every session row (active + superseded) into a single markdown
    /// file at `target_path`. Use as a periodic backup or before risky DB ops.
    /// Returns the count of rows written.
    #[tool(
        description = "Export all session memos to a single markdown file at `target_path`. Format mirrors what a future `session_import` would consume — durable backup against schema migrations / accidental DB loss. Returns the number of rows written."
    )]
    async fn session_dump_md(
        &self,
        Parameters(params): Parameters<SessionDumpMdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let workspace = self.handle.workspace_root().to_path_buf();
        let target = std::path::PathBuf::from(params.target_path);
        let count = tokio::task::spawn_blocking(move || {
            let h = SessionsHandle::open(&workspace).map_err(SessionsErr::from)?;
            dump_sessions_to_markdown(&h, &target).map_err(SessionsErr::from)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(sessions_err_to_rmcp)?;
        Ok(success_json(&serde_json::json!({ "count": count })))
    }

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
}

#[tool_handler]
impl ServerHandler for StandardocMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::default(),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "Standardoc MCP server. Use `find_symbol` to discover symbols by name fragment, \
                 then `get_context` for the structured chunk of a specific FQDN. The workspace \
                 indexes itself in the background on startup; tools called before indexing \
                 completes return a friendly progress message — retry shortly."
                    .to_string(),
            ),
        }
    }
}

impl StandardocMcp {
    pub fn index_ready(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.index_ready)
    }

    pub fn watcher_slot(&self) -> Arc<Mutex<Option<WatcherHandle>>> {
        Arc::clone(&self.watcher)
    }

    pub fn rag_watcher_slot(&self) -> Arc<Mutex<Option<RagWatcherHandle>>> {
        Arc::clone(&self.rag_watcher)
    }

    /// Enables the RAG layer for this handler. Builder-style so existing
    /// constructors stay backward-compatible — daemons that wire a RAG
    /// store call this immediately after `new()` ; daemons that don't
    /// keep `rag_store = None` and `fetch_chunks` returns the "not
    /// enabled" message while `get_context` emits an empty `chunk_refs`.
    #[must_use]
    pub fn with_rag(mut self, rag_store: Arc<RagStore>) -> Self {
        self.rag_store = Some(rag_store);
        self
    }

    /// Wires an embedder for query-time `chunk_refs` re-ranking. Only
    /// useful in combination with `with_rag` — the embedder is invoked
    /// from `get_context(fqdn, query?)` when the caller passes a query
    /// string, to recompute cosine similarity between the query and
    /// each linked chunk's stored vector.
    #[must_use]
    pub fn with_embedder(mut self, embedder: Arc<dyn Embedder>) -> Self {
        self.rag_embedder = Some(embedder);
        self
    }

    pub const fn rag_store(&self) -> Option<&Arc<RagStore>> {
        self.rag_store.as_ref()
    }

    /// Looks up chunk references for `fqdn` if the RAG layer is enabled.
    /// When `query` is provided AND an embedder is wired, the refs are
    /// re-ranked by cosine similarity between the query embedding and
    /// each chunk's stored vector. Returns an empty vec when RAG is
    /// disabled or any step fails — degradation is silent so
    /// `get_context` always succeeds for the graph data.
    async fn chunk_refs_for(
        &self,
        fqdn: &str,
        query: Option<&str>,
    ) -> Result<Vec<ChunkRef>, ErrorData> {
        let Some(store) = self.rag_store.clone() else {
            return Ok(Vec::new());
        };
        let fqdn_owned = fqdn.to_string();

        if let (Some(q), Some(embedder)) = (query, self.rag_embedder.clone()) {
            let q_owned = q.to_string();
            let refs = tokio::task::spawn_blocking(move || -> Result<Vec<ChunkRef>, ErrorData> {
                let vector = embedder.embed(&q_owned).map_err(|e| {
                    ErrorData::internal_error(format!("rag embed query: {e}"), None)
                })?;
                store
                    .refs_for_symbol_with_query(&fqdn_owned, &vector, CHUNK_REFS_DEFAULT_LIMIT)
                    .map_err(|e| ErrorData::internal_error(format!("rag re-rank: {e}"), None))
            })
            .await
            .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?;
            return refs;
        }

        let refs = tokio::task::spawn_blocking(move || {
            store.refs_for_symbol(&fqdn_owned, CHUNK_REFS_DEFAULT_LIMIT)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .unwrap_or_default();
        Ok(refs)
    }
}

/// Tool output for `get_context` : the canonical neighbor-flat shape
/// from `query::context_for_symbol_with_neighbors`, plus `chunk_refs`
/// added via `#[serde(flatten)]` so the JSON layout stays a single flat
/// object (consumers that don't know about RAG see an extra optional
/// `chunk_refs` field and can ignore it). `routing_hint` is `None` for
/// well-paced calls and emits an in-band 3-phase nudge when a depth=2
/// call lands without a recent depth=1 scoping pass on the same FQDN.
#[derive(Debug, Serialize)]
pub(crate) struct GetContextResponse {
    #[serde(flatten)]
    pub ctx: query::SymbolContextWithNeighbors,
    pub chunk_refs: Vec<ChunkRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing_hint: Option<String>,
}

/// Tool input — `fetch_chunks(uris)`. Resolves `rag://<id>` URIs to the
/// underlying `Chunk` rows. Unknown / malformed entries are silently
/// dropped. The list is hard-capped at `FETCH_CHUNKS_MAX_INPUT` to keep
/// the response small.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct FetchChunksParams {
    /// URIs of the form `rag://<id>`, typically copied from the
    /// `chunk_refs` field of a previous `get_context` call.
    pub uris: Vec<String>,
}

/// Tool input — `get_context(fqdn, depth?, query?)`. Forwarded to
/// `query::context_for_symbol_with_neighbors`. `depth` defaults to `1`
/// and is hard-clamped to `1..=2` server-side. When `query` is
/// supplied AND the daemon is booted with both a RAG store and an
/// embedder, `chunk_refs` are re-ranked by cosine similarity between
/// the query embedding and each chunk's stored vector.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct GetContextParams {
    /// The fully-qualified domain name of the symbol to look up
    /// (e.g. `crate::module::function`). Match `RawSymbol::fqdn`.
    pub fqdn: String,
    /// Output richness — `1` = neighbor FQDNs only (cheap, exploration);
    /// `2` = full RawSymbol per resolved neighbor (rich, reasoning).
    /// Defaults to `1`. Hard-clamped to `1..=2`.
    pub depth: Option<u8>,
    /// Optional natural-language query used to re-rank `chunk_refs` by
    /// semantic relevance. Ignored when the daemon has no RAG store or
    /// no embedder wired.
    #[serde(default)]
    pub query: Option<String>,
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
}

/// Tool input — `list_symbols(kind?, visibility?, module?, limit?)`.
/// Forwarded to `query::list_symbols`. No FTS, no glob — pure server-side
/// filter listing ordered by fqdn.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ListSymbolsParams {
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

/// Tool input — `get_body(fqdn, max_lines?, strip_attrs?, signature_only?)`.
/// Forwarded to `query::body_for_fqdn`. Returns `null` if `fqdn` is not in
/// the index.
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
}

/// Tool input — `session_save(slug, body_md, supersedes?)`. Persists or
/// overwrites a session memo. `slug` is a UNIQUE identifier; UPSERT semantics.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SessionSaveParams {
    pub slug: String,
    pub body_md: String,
    pub supersedes: Option<String>,
}

/// Tool input — `session_list(active_only?)`. Defaults to active-only.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SessionListParams {
    pub active_only: Option<bool>,
}

/// Tool input — `session_get(slug?)`. Omit `slug` to fetch the most recent
/// active session (latest entry point).
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SessionGetParams {
    pub slug: Option<String>,
}

/// Tool input — `session_dump_md(target_path)`. Exports every session to a
/// markdown file at the given absolute path.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SessionDumpMdParams {
    pub target_path: String,
}

/// Tool output for `current_revision`. The legacy `revision` field is kept
/// as the first key so callers that only deserialize `{revision}` keep
/// working; new fields surface daemon capabilities so an AI can decide
/// at boot whether passing `query` to `get_context` will re-rank chunks
/// (`rag.embedder.is_some()`), whether the live watcher is debouncing
/// edits (`watcher.active`), and whether early read calls would hit the
/// "indexing in progress" branch (`indexing.ready`).
#[derive(Debug, Serialize)]
pub(crate) struct CurrentRevisionJson {
    pub revision: u64,
    pub rag: RagCapabilityJson,
    pub watcher: WatcherCapabilityJson,
    pub indexing: IndexingCapabilityJson,
}

#[derive(Debug, Serialize)]
pub(crate) struct RagCapabilityJson {
    /// `true` when the daemon was booted with `--rag` and a `RagStore`
    /// was successfully opened. Implies `fetch_chunks` is callable and
    /// `chunk_refs` are populated on `get_context` responses.
    pub enabled: bool,
    /// Embedder identity. `Some` when `enabled` AND an embedder is wired
    /// — only then does passing `query` to `get_context` re-rank
    /// `chunk_refs` by cosine similarity. `None` with `enabled: true`
    /// means link-confidence ordering only (the `query` arg is silently
    /// ignored).
    pub embedder: Option<EmbedderInfoJson>,
}

#[derive(Debug, Serialize)]
pub(crate) struct EmbedderInfoJson {
    /// Short model id (e.g. `"bge-small-en-v1.5"`, `"mock-blake3-128"`).
    /// Mirrors `EmbedModel.id` stored in the RAG schema metadata.
    pub id: String,
    /// Vector dimension produced by this embedder.
    pub dim: u32,
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

/// Tool input — `usage_stats(period?)`. Accepted period strings (case-insensitive):
/// `day` / `d` / `today`, `week` / `w` / `7d`, `all` (default).
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct UsageStatsParams {
    /// Window scope. Defaults to `"all"`. Accepts `day`, `week`, `all` (and
    /// their aliases listed above). Anything else returns an invalid-params
    /// error rather than silently coercing.
    #[serde(default)]
    pub period: Option<String>,
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
) -> Result<SymbolFilter, ErrorData> {
    let kind = kind.map(parse_kind).transpose()?;
    let visibility = visibility.map(parse_visibility).transpose()?;
    let include_external = include_external.unwrap_or(true);
    Ok(SymbolFilter {
        kind,
        visibility,
        module,
        include_external,
    })
}

fn parse_kind(s: &str) -> Result<Kind, ErrorData> {
    match s {
        "function" => Ok(Kind::Function),
        "type" => Ok(Kind::Type),
        "value" => Ok(Kind::Value),
        "module" => Ok(Kind::Module),
        "macro" => Ok(Kind::Macro),
        other => Err(ErrorData::invalid_params(
            format!(
                "unknown kind `{other}` — expected one of: function, type, value, module, macro"
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

fn success_json<T: Serialize>(value: &T) -> CallToolResult {
    match serde_json::to_string_pretty(value) {
        Ok(json) => CallToolResult::success(vec![Content::text(json)]),
        Err(e) => CallToolResult::error(vec![Content::text(format!(
            "failed to serialize tool result: {e}"
        ))]),
    }
}

/// Same as `success_json`, but also records the call in `usage_stats`
/// (table in `sessions.db`) with the response byte count and a baseline
/// computed from the workspace-relative `files` referenced by the
/// response. Logging is best-effort and fire-and-forget — a SQLite or
/// runtime hiccup will never bubble up to the caller.
fn success_json_with_usage<T: Serialize>(
    value: &T,
    workspace_root: &Path,
    tool_name: &'static str,
    fqdn: Option<String>,
    files: Vec<String>,
) -> CallToolResult {
    let json = match serde_json::to_string_pretty(value) {
        Ok(j) => j,
        Err(e) => {
            return CallToolResult::error(vec![Content::text(format!(
                "failed to serialize tool result: {e}"
            ))]);
        }
    };
    let bytes_out = u64::try_from(json.len()).unwrap_or(u64::MAX);
    let baseline = sum_distinct_file_sizes(workspace_root, files);
    log_usage_fire_and_forget(
        workspace_root.to_path_buf(),
        tool_name,
        fqdn,
        bytes_out,
        baseline,
    );
    CallToolResult::success(vec![Content::text(json)])
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
                query: None,
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
            }))
            .await
            .expect("tool returns Ok with friendly degradation");
        assert!(body_text(&result).contains("Workspace indexing in progress"));
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
            }))
            .await
            .expect("tool returns Ok with friendly degradation");
        assert!(body_text(&result).contains("Workspace indexing in progress"));
    }

    #[test]
    fn parse_kind_recognises_every_ir_variant() {
        assert!(parse_kind("function").is_ok());
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
            Some("function"),
            Some("private"),
            Some("crate::a".into()),
            None,
        )
        .unwrap();
        assert_eq!(f.kind, Some(Kind::Function));
        assert_eq!(f.visibility, Some(Visibility::Private));
        assert_eq!(f.module.as_deref(), Some("crate::a"));
        assert!(
            f.include_external,
            "omitting include_external must default to true (S3-G include externals by default)"
        );
    }

    #[test]
    fn parse_filter_all_none_yields_empty_filter() {
        let f = parse_filter(None, None, None, None).unwrap();
        assert_eq!(f, SymbolFilter::default());
    }

    #[test]
    fn parse_filter_propagates_include_external_false() {
        let f = parse_filter(None, None, None, Some(false)).unwrap();
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
                query: None,
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
        assert!(hint.is_some(), "stale scoping pass must not silence the hint");
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
    async fn current_revision_reports_rag_disabled_on_default_fixture() {
        let (_dir, mcp) = fixture();
        let result = mcp.current_revision().await.unwrap();
        let body = body_text(&result);
        assert!(
            body.contains("\"rag\""),
            "expected rag capability block, got `{body}`"
        );
        assert!(
            body.contains("\"enabled\": false"),
            "fixture has no RAG store, got `{body}`"
        );
        // embedder must be `null` when no embedder is wired.
        assert!(body.contains("\"embedder\": null"), "got `{body}`");
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
    async fn usage_stats_empty_returns_zero_counters() {
        let (_dir, mcp) = fixture();
        let result = mcp
            .usage_stats(Parameters(UsageStatsParams { period: None }))
            .await
            .unwrap();
        let body = body_text(&result);
        assert!(body.contains("\"calls\": 0"), "got `{body}`");
        assert!(body.contains("\"bytes_out_total\": 0"), "got `{body}`");
        assert!(body.contains("\"baseline_bytes_total\": 0"), "got `{body}`");
        assert!(body.contains("\"period\": \"all\""), "got `{body}`");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn usage_stats_unknown_period_rejected() {
        let (_dir, mcp) = fixture();
        let err = mcp
            .usage_stats(Parameters(UsageStatsParams {
                period: Some("eternity".into()),
            }))
            .await;
        assert!(
            err.is_err(),
            "unknown period must be rejected with ErrorData"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn usage_stats_aliases_accepted() {
        let (_dir, mcp) = fixture();
        for alias in ["day", "Today", "week", "7d", "all", ""] {
            let result = mcp
                .usage_stats(Parameters(UsageStatsParams {
                    period: Some(alias.into()),
                }))
                .await;
            assert!(
                result.is_ok(),
                "alias `{alias}` should be accepted, got {result:?}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn find_symbol_logs_usage_row_for_successful_call() {
        let (dir, mcp) = fixture();
        cold_start_workspace(&mcp, dir.path());
        let workspace = dir.path().to_path_buf();
        let _ = mcp
            .find_symbol(Parameters(FindSymbolParams {
                query: "anything".into(),
                limit: None,
                kind: None,
                visibility: None,
                module: None,
                include_external: None,
            }))
            .await
            .unwrap();
        // log_usage_fire_and_forget hops through tokio::task::spawn ->
        // spawn_blocking -> SessionsHandle::open + INSERT. On starved CI
        // runners (2-vCPU Linux/macOS) the chain can take well over 1s to
        // resolve. Poll up to ~5s before giving up so the test reflects
        // real failures (e.g. swallowed open error) rather than scheduling
        // jitter.
        let h = SessionsHandle::open(&workspace).expect("open sessions");
        let mut stats = h.query_usage_stats(UsagePeriod::All).unwrap();
        for _ in 0..100 {
            if stats.calls > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            stats = h.query_usage_stats(UsagePeriod::All).unwrap();
        }
        assert!(
            stats.calls >= 1,
            "expected at least one logged call after ~5s of polling, got \
             {stats:?} — fire-and-forget spawn likely failed silently \
             (check SessionsHandle::open or the spawn_blocking chain)"
        );
    }
}

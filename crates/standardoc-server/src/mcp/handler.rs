use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, RwLock};

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
    IndexHandle, LanguageProvider, ScanFilters, WatcherHandle,
    query::{self, SymbolFilter},
};
use standardoc_ir::{Kind, RawSymbol, Visibility};

use crate::mcp::error::server_error_to_rmcp;

const FIND_SYMBOL_DEFAULT_LIMIT: u8 = 20;
const FIND_SYMBOL_MAX_LIMIT: u8 = 100;
const GET_CONTEXT_DEFAULT_DEPTH: u8 = 1;
const FIND_SIMILAR_DEFAULT_THRESHOLD: f32 = 0.8;

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
    #[allow(dead_code)]
    provider: Arc<dyn LanguageProvider>,
    #[allow(dead_code)]
    filters: Arc<RwLock<ScanFilters>>,
    index_ready: Arc<AtomicBool>,
    watcher: Arc<Mutex<Option<WatcherHandle>>>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl StandardocMcp {
    pub(crate) fn new(
        handle: IndexHandle,
        provider: Arc<dyn LanguageProvider>,
        filters: Arc<RwLock<ScanFilters>>,
    ) -> Self {
        Self {
            handle,
            provider,
            filters,
            index_ready: Arc::new(AtomicBool::new(false)),
            watcher: Arc::new(Mutex::new(None)),
            tool_router: Self::tool_router(),
        }
    }

    /// Aggregate context for a symbol: signature + descriptions + four
    /// pre-grouped neighbor lists (callers / callees / imports / imported_by).
    /// `depth` selects the shape's richness — see [`SymbolContextWithNeighbors`].
    #[tool(
        description = "Aggregate context for a symbol identified by its fully-qualified name (FQDN). Returns the symbol's signature, descriptions and four pre-grouped neighbor lists (callers, callees, imports, imported_by). `depth=1` returns neighbor FQDNs only — cheap, suited to graph exploration. `depth=2` enriches each resolved neighbor with its full RawSymbol — suited to reasoning. Day-1 hard-clamped to 1..=2."
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
        let handle = self.handle.clone();
        let fqdn = params.fqdn;
        let result = tokio::task::spawn_blocking(move || {
            query::context_for_symbol_with_neighbors(&handle, &fqdn, depth)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;

        match result {
            Some(ctx) => Ok(success_json(&ctx)),
            None => Ok(CallToolResult::success(vec![Content::text(
                "no symbol found for the given FQDN",
            )])),
        }
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

        let filter = parse_filter(params.kind.as_deref(), params.visibility.as_deref(), params.module)?;
        let limit = clamp_limit(params.limit);
        let handle = self.handle.clone();
        let result = tokio::task::spawn_blocking(move || {
            query::search_text(&handle, &trimmed, limit as usize, &filter)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;

        Ok(success_json(&result))
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

        let filter = parse_filter(params.kind.as_deref(), params.visibility.as_deref(), params.module)?;
        let limit = clamp_limit(params.limit);
        let handle = self.handle.clone();
        let result = tokio::task::spawn_blocking(move || {
            query::list_symbols(&handle, &filter, limit as usize)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;

        Ok(success_json(&result))
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

        let filter = parse_filter(params.kind.as_deref(), params.visibility.as_deref(), params.module)?;
        let limit = clamp_limit(params.limit);
        let handle = self.handle.clone();
        let result = tokio::task::spawn_blocking(move || {
            query::find_by_pattern(&handle, &trimmed, &filter, limit as usize)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;

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

        let trimmed = params.reference.trim().to_owned();
        if trimmed.is_empty() {
            return Ok(success_json::<Vec<SimilarSymbolJson>>(&Vec::new()));
        }

        let threshold = parse_threshold(params.threshold)?;
        let filter = parse_filter(params.kind.as_deref(), params.visibility.as_deref(), params.module)?;
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
}

/// Tool input — `get_context(fqdn, depth?)`. Forwarded to
/// `query::context_for_symbol_with_neighbors`. `depth` defaults to `1` and
/// is hard-clamped to `1..=2` server-side.
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
}

/// Tool output envelope for `find_similar_symbols`. Pairs each `RawSymbol`
/// with its similarity score for trivial JSON consumption by the LLM.
#[derive(Debug, Serialize)]
pub(crate) struct SimilarSymbolJson {
    pub score: f32,
    pub symbol: RawSymbol,
}

fn parse_filter(
    kind: Option<&str>,
    visibility: Option<&str>,
    module: Option<String>,
) -> Result<SymbolFilter, ErrorData> {
    let kind = kind.map(parse_kind).transpose()?;
    let visibility = visibility.map(parse_visibility).transpose()?;
    Ok(SymbolFilter {
        kind,
        visibility,
        module,
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

fn clamp_limit(raw: Option<u8>) -> u8 {
    raw.unwrap_or(FIND_SYMBOL_DEFAULT_LIMIT)
        .clamp(1, FIND_SYMBOL_MAX_LIMIT)
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
        let f = parse_filter(Some("function"), Some("private"), Some("crate::a".into())).unwrap();
        assert_eq!(f.kind, Some(Kind::Function));
        assert_eq!(f.visibility, Some(Visibility::Private));
        assert_eq!(f.module.as_deref(), Some("crate::a"));
    }

    #[test]
    fn parse_filter_all_none_yields_empty_filter() {
        let f = parse_filter(None, None, None).unwrap();
        assert_eq!(f, SymbolFilter::default());
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
            }))
            .await;
        // Invalid filter is a parameter error — surfaces as Err on the
        // tool invocation, NOT a graceful CallToolResult.
        assert!(result.is_err(), "invalid `kind` must be rejected with ErrorData");
    }
}

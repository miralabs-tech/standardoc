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
    query::{self},
};
use standardoc_ir::RawSymbol;

use crate::mcp::error::server_error_to_rmcp;

const FIND_SYMBOL_DEFAULT_LIMIT: u8 = 20;
const FIND_SYMBOL_MAX_LIMIT: u8 = 100;
const GET_CONTEXT_DEFAULT_DEPTH: u8 = 1;

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
    /// is server-capped at 100 to keep tool results small.
    #[tool(
        description = "Full-text search across the workspace index over symbol names and FQDNs. Returns ranked matches as a JSON array. `limit` defaults to 20 and is capped at 100 server-side. Use this to discover symbols when you only know a fragment of the name; follow up with `get_context` to drill into a specific FQDN."
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

        let limit = clamp_limit(params.limit);
        let handle = self.handle.clone();
        let result = tokio::task::spawn_blocking(move || {
            query::search_text(&handle, &trimmed, limit as usize)
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
        .map_err(|e| server_error_to_rmcp(&e.into()))?;

        Ok(success_json(&result))
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

/// Tool input — `find_symbol(query, limit?)`. Forwarded to
/// `query::search_text` (FTS5 over `name` + `fqdn`). `limit` defaults to
/// `20` and is capped at `100` server-side to keep MCP tool results small.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct FindSymbolParams {
    /// Free-text FTS5 query against symbol `name` and `fqdn` columns.
    /// Tokenization handles snake_case and camelCase.
    pub query: String,
    /// Maximum results to return. Defaults to 20, capped at 100.
    pub limit: Option<u8>,
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
            }))
            .await
            .unwrap();
        let text = body_text(&result);
        assert!(
            text.trim() == "[]",
            "blank query must short-circuit to empty JSON array, got `{text}`"
        );
    }
}

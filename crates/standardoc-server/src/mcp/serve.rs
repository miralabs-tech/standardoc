use std::io;
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};

use rmcp::ServiceExt;
use rmcp::transport::stdio;
use standardoc_core::{IndexHandle, LanguageProvider, ScanFilters, spawn_watcher};

use super::handler::StandardocMcp;
use super::progress::run_cold_start_with_progress;
use crate::ServerError;

/// Run the MCP daemon over stdio. Async; the caller owns the tokio runtime.
///
/// The boot ordering mirrors `serve_lsp` (lock 23 §1 Q6): the server starts
/// answering MCP requests immediately while cold start runs in the
/// background, so the client sees the daemon as live. Tool calls observed
/// before [`StandardocMcp::index_ready`] flips return a friendly
/// "indexing in progress" tool result rather than an MCP error
/// (Q5 graceful degradation).
///
/// Once cold start completes the watcher boots and is stashed inside the
/// handler so it shares lifetime with the running service: dropping the
/// service drops the watcher, which joins its dispatch thread before
/// stdio closes (mirrors `lsp::handler::StandardocLsp`).
///
/// When `handle.is_readonly()` is true (lock 31a), cold start and watcher
/// are skipped and `index_ready` flips immediately: the primary writer
/// (LSP daemon, `standardoc watch`, ...) owns the fs4 lock and feeds the
/// shared `.standardoc/index.db`, while this read-only daemon only serves
/// queries.
pub async fn serve_mcp(
    handle: IndexHandle,
    provider: Arc<dyn LanguageProvider>,
    filters: Arc<RwLock<ScanFilters>>,
) -> Result<(), ServerError> {
    let mcp = StandardocMcp::new(handle.clone(), Arc::clone(&provider), Arc::clone(&filters));
    kick_off_indexing(&mcp, handle, provider, filters);

    let service = mcp
        .serve(stdio())
        .await
        .map_err(|e| ServerError::Io(io::Error::other(format!("rmcp serve: {e}"))))?;
    service
        .waiting()
        .await
        .map_err(|e| ServerError::Io(io::Error::other(format!("rmcp waiting: {e}"))))?;
    Ok(())
}

/// Boot the indexing pipeline behind an MCP handler. Synchronous flip of
/// `index_ready` for read-only handles (a writer owns the workspace lock
/// elsewhere); background `tokio::spawn` of cold-start + watcher otherwise.
/// Exposed `pub` so integration tests can drive the boot side-effect
/// without consuming stdio via `mcp.serve(stdio())`.
pub fn kick_off_indexing(
    mcp: &StandardocMcp,
    handle: IndexHandle,
    provider: Arc<dyn LanguageProvider>,
    filters: Arc<RwLock<ScanFilters>>,
) {
    let index_ready = mcp.index_ready();

    if handle.is_readonly() {
        index_ready.store(true, Ordering::Release);
        return;
    }

    let watcher_slot = mcp.watcher_slot();
    tokio::spawn(async move {
        match run_cold_start_with_progress(
            handle.clone(),
            Arc::clone(&provider),
            Arc::clone(&filters),
        )
        .await
        {
            Ok(()) => {
                index_ready.store(true, Ordering::Release);
                match spawn_watcher(handle, provider, filters) {
                    Ok(w) => {
                        if let Ok(mut g) = watcher_slot.lock() {
                            *g = Some(w);
                        }
                    }
                    Err(e) => eprintln!("standardoc mcp: watcher boot failed: {e}"),
                }
            }
            Err(e) => eprintln!("standardoc mcp: cold start failed: {e}"),
        }
    });
}

/// Build a [`StandardocMcp`] handler without consuming it via
/// `ServiceExt::serve`. Useful for integration tests that drive tool
/// invocations directly instead of going through stdio framing — mirrors
/// `lsp::serve::build_lsp_service` (lock 24 §1.2).
pub fn build_mcp_handler(
    handle: IndexHandle,
    provider: Arc<dyn LanguageProvider>,
    filters: Arc<RwLock<ScanFilters>>,
) -> StandardocMcp {
    StandardocMcp::new(handle, provider, filters)
}

use std::sync::{Arc, RwLock};

use standardoc_core::{ColdStartError, IndexHandle, LanguageProvider, ScanFilters, cold_start};

/// Run cold start in a background-friendly shape: bridges the std::thread
/// writer pipeline (`cold_start::run`) to the tokio world via
/// `spawn_blocking`. Mirrors the LSP shape (lock 24 `lsp::progress`) without
/// the `$/progress workDoneProgress` notification stream — the MCP spec
/// drives progress entirely from the client (via `_meta.progressToken` in
/// `initialize`), and rmcp 0.16's tool macros don't surface that token to
/// the handler day-1. Punted to post-beta.1 (lock mcp-scaffold-25 §5.4) so
/// the day-1 daemon is functionally equivalent to LSP minus client UX
/// progress visibility — `index_ready` is what gates tool invocations and
/// flips once cold start completes.
pub(crate) async fn run_cold_start_with_progress(
    handle: IndexHandle,
    provider: Arc<dyn LanguageProvider>,
    filters: Arc<RwLock<ScanFilters>>,
) -> Result<(), ColdStartError> {
    tokio::task::spawn_blocking(move || {
        let guard = filters
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cold_start::run(&handle, provider.as_ref(), &guard)
    })
    .await
    .unwrap_or_else(|join_err| {
        Err(ColdStartError::Io(std::io::Error::other(format!(
            "spawn_blocking join failed: {join_err}"
        ))))
    })
}

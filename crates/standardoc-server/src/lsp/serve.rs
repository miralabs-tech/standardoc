use std::sync::{Arc, RwLock};

use standardoc_core::{IndexHandle, LanguageProvider, ScanFilters};
use tower_lsp_server::{ClientSocket, LspService, Server};

use super::handler::StandardocLsp;
use crate::ServerError;

/// Run the LSP daemon over stdio. Async; the caller owns the tokio runtime.
///
/// The boot ordering is LSP-aware (vs [`crate::open_workspace`]):
/// 1. Build [`LspService`] with a fresh [`StandardocLsp`].
/// 2. Run [`Server::serve`] on `stdin`/`stdout`. The handler's
///    `initialize` answers immediately, then `initialized` spawns the
///    cold-start + progress + watcher orchestration as a background task.
/// 3. The `serve` future returns when the client sends `shutdown` + `exit`.
///
/// `lifecycle::open_workspace` is preserved for non-LSP paths (`cmd_watch`,
/// future MCP daemon).
pub async fn serve_lsp(
    handle: IndexHandle,
    provider: Arc<dyn LanguageProvider>,
    filters: Arc<RwLock<ScanFilters>>,
) -> Result<(), ServerError> {
    let (service, socket) = build_lsp_service(handle, provider, filters);
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    Server::new(stdin, stdout, socket).serve(service).await;
    Ok(())
}

/// Build the `LspService` and its `ClientSocket` without consuming them via
/// `Server::serve`. Useful for integration tests that drive the service with
/// `tower::Service::call` instead of stdio framing.
pub fn build_lsp_service(
    handle: IndexHandle,
    provider: Arc<dyn LanguageProvider>,
    filters: Arc<RwLock<ScanFilters>>,
) -> (LspService<StandardocLsp>, ClientSocket) {
    LspService::new(move |client| {
        StandardocLsp::new(
            client,
            handle.clone(),
            Arc::clone(&provider),
            Arc::clone(&filters),
        )
    })
}

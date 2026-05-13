use std::sync::{Arc, Mutex, RwLock};

use standardoc_core::rag::RagPipeline;
use standardoc_core::{
    IndexHandle, LanguageProvider, RagWatcherHandle, RevisionRelinkHandle, ScanFilters,
};
use tower_lsp_server::{ClientSocket, LspService, Server};

use super::handler::StandardocLsp;
use crate::ServerError;
use crate::mcp::serve::{
    run_rag_cold_start, run_rag_relink_all, spawn_rag_watcher_into_slot,
    spawn_revision_relink_into_slot,
};

/// Run the LSP daemon over stdio. Async; the caller owns the tokio runtime.
///
/// The boot ordering is LSP-aware (vs [`crate::open_workspace`]):
/// 1. Build [`LspService`] with a fresh [`StandardocLsp`].
/// 2. Run [`Server::serve`] on `stdin`/`stdout`. The handler's
///    `initialize` answers immediately, then `initialized` spawns the
///    cold-start + progress + watcher orchestration as a background task.
/// 3. The `serve` future returns when the client sends `shutdown` + `exit`.
///
/// When `rag_pipeline` is `Some`, the RAG layer is cold-started in a
/// background tokio task that runs in parallel with the LSP handler's AST
/// cold-start. The RAG watcher is then spawned to keep `.md` edits live.
/// The watcher handle is held by an `Arc<Mutex<...>>` rooted in this
/// function's stack — it stays alive until `Server::serve` returns at
/// shutdown.
///
/// `lifecycle::open_workspace` is preserved for non-LSP paths (`cmd_watch`,
/// future MCP daemon).
pub async fn serve_lsp(
    handle: IndexHandle,
    provider: Arc<dyn LanguageProvider>,
    filters: Arc<RwLock<ScanFilters>>,
    rag_pipeline: Option<Arc<RagPipeline>>,
) -> Result<(), ServerError> {
    let rag_watcher_slot: Arc<Mutex<Option<RagWatcherHandle>>> = Arc::new(Mutex::new(None));
    let rag_relink_slot: Arc<Mutex<Option<RevisionRelinkHandle>>> = Arc::new(Mutex::new(None));
    if let Some(pipeline) = rag_pipeline {
        let handle_for_rag = handle.clone();
        let filters_for_rag = Arc::clone(&filters);
        let watcher_slot = Arc::clone(&rag_watcher_slot);
        let relink_slot = Arc::clone(&rag_relink_slot);
        tokio::task::spawn_blocking(move || {
            // On a fresh workspace the AST cold-start may still be running
            // when RAG cold-start fires; the initial `relink_all` runs
            // against an empty workspace_fqdns list and produces
            // frontmatter-only links. The revision-relink watcher
            // catches up automatically once the AST cold-start commits
            // and bumps `handle.revision()`.
            run_rag_relink_all(&pipeline, &handle_for_rag);
            run_rag_cold_start(&pipeline, &handle_for_rag, &filters_for_rag);
            spawn_revision_relink_into_slot(
                &relink_slot,
                handle_for_rag.clone(),
                Arc::clone(&pipeline),
            );
            spawn_rag_watcher_into_slot(&watcher_slot, handle_for_rag, pipeline, filters_for_rag);
        });
    }

    let (service, socket) = build_lsp_service(handle, provider, filters);
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    Server::new(stdin, stdout, socket).serve(service).await;
    drop(rag_watcher_slot);
    drop(rag_relink_slot);
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

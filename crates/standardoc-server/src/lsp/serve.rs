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
            // T-B Phase 2 : wait for the AST cold-start to commit at
            // least once before running the initial relink, so the
            // workspace_fqdns lookup sees the real graph rather than
            // an empty list. The wait is capped at 30 s (graceful
            // timeout — empty workspaces stay at revision 0 forever),
            // and the revision-relink watcher takes over for any
            // subsequent bumps. The cold-start chunker / embedder
            // still run unconditionally so prose-side changes are
            // captured regardless of AST readiness.
            wait_for_first_revision_bump(&handle_for_rag);
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

/// Spins until `handle.revision()` reports a non-zero value or
/// [`AST_COLD_START_WAIT_CAP`] elapses. Returns immediately on any
/// observed bump : the first commit is enough for the relink that
/// follows to see SOME workspace fqdns rather than an empty list.
/// Empty workspaces (no Rust/TS files) stay at revision 0 forever ;
/// the cap prevents the LSP RAG layer from hanging.
fn wait_for_first_revision_bump(handle: &IndexHandle) {
    const POLL: std::time::Duration = std::time::Duration::from_millis(500);
    const CAP: std::time::Duration = std::time::Duration::from_secs(30);
    let start = std::time::Instant::now();
    while start.elapsed() < CAP {
        if handle.revision() > 0 {
            return;
        }
        std::thread::sleep(POLL);
    }
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

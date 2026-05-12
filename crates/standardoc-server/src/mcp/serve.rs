use std::io;
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};

use rmcp::ServiceExt;
use rmcp::transport::stdio;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::tower::{
    StreamableHttpServerConfig, StreamableHttpService,
};
use standardoc_core::{
    IndexHandle, LanguageProvider, RagPipeline, ScanFilters, spawn_rag_watcher, spawn_watcher,
};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

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
/// shared `.standardoc/index.db`, while this secondary daemon only serves
/// queries (and write-through external resolution under SQLite WAL).
pub async fn serve_mcp(
    handle: IndexHandle,
    provider: Arc<dyn LanguageProvider>,
    filters: Arc<RwLock<ScanFilters>>,
    rag_pipeline: Option<Arc<RagPipeline>>,
) -> Result<(), ServerError> {
    let mcp = with_rag_components(
        StandardocMcp::new(handle.clone(), Arc::clone(&provider), Arc::clone(&filters)),
        rag_pipeline.as_ref(),
    );
    kick_off_indexing(&mcp, handle, provider, filters, rag_pipeline);

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

/// Run the MCP daemon over HTTP/SSE (`streamable-http` transport).
///
/// Binds to `bind_addr` (typically `127.0.0.1:<port>`) and serves the MCP
/// protocol at `/mcp`. Each client gets its own session — multiple chat
/// clients (Copilot Chat, Claude Code in VSCode, Claude Code CLI, Cursor,
/// Claude Desktop, …) connect to the same daemon, eliminating the
/// per-chat stdio child-spawn cost of the stdio transport.
///
/// Writes the resolved endpoint URL to
/// `<workspace>/.standardoc/mcp.endpoint` so the VSCode extension and
/// external clients can discover the port (especially useful when the
/// caller passes `port=0` and the kernel allocates a random ephemeral
/// port).
///
/// Cold-start ordering and watcher lifetime mirror [`serve_mcp`]. Both
/// transports share the same [`StandardocMcp`] handler.
pub async fn serve_mcp_http(
    handle: IndexHandle,
    provider: Arc<dyn LanguageProvider>,
    filters: Arc<RwLock<ScanFilters>>,
    bind_addr: &str,
    rag_pipeline: Option<Arc<RagPipeline>>,
) -> Result<(), ServerError> {
    let listener = TcpListener::bind(bind_addr)
        .await
        .map_err(|e| ServerError::Io(io::Error::other(format!("bind {bind_addr}: {e}"))))?;
    let local_addr = listener
        .local_addr()
        .map_err(|e| ServerError::Io(io::Error::other(format!("local_addr: {e}"))))?;

    let endpoint = format!("http://{local_addr}/mcp");
    write_endpoint_file(handle.workspace_root(), &endpoint)?;
    eprintln!("standardoc mcp http: listening on {endpoint}");

    // Build a fresh `StandardocMcp` factory: the streamable-http server
    // calls this per session, but our handler is cheap to clone — we
    // share a single registry / index / watcher across sessions.
    let template = with_rag_components(
        StandardocMcp::new(handle.clone(), Arc::clone(&provider), Arc::clone(&filters)),
        rag_pipeline.as_ref(),
    );
    kick_off_indexing(&template, handle, provider, filters, rag_pipeline);

    let service = StreamableHttpService::new(
        move || Ok(template.clone()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig {
            stateful_mode: true,
            cancellation_token: CancellationToken::new(),
            ..Default::default()
        },
    );

    let router = axum::Router::new().route_service("/mcp", service);
    axum::serve(listener, router)
        .await
        .map_err(|e| ServerError::Io(io::Error::other(format!("axum serve: {e}"))))
}

fn write_endpoint_file(workspace_root: &std::path::Path, endpoint: &str) -> Result<(), ServerError> {
    let path = workspace_root.join(".standardoc").join("mcp.endpoint");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(ServerError::Io)?;
    }
    std::fs::write(&path, endpoint).map_err(ServerError::Io)
}

/// Boot the indexing pipeline behind an MCP handler. Synchronous flip of
/// `index_ready` for secondary handles (a primary writer owns the
/// workspace lock elsewhere); background `tokio::spawn` of cold-start +
/// watcher otherwise. Exposed `pub` so integration tests can drive the
/// boot side-effect without consuming stdio via `mcp.serve(stdio())`.
pub fn kick_off_indexing(
    mcp: &StandardocMcp,
    handle: IndexHandle,
    provider: Arc<dyn LanguageProvider>,
    filters: Arc<RwLock<ScanFilters>>,
    rag_pipeline: Option<Arc<RagPipeline>>,
) {
    let index_ready = mcp.index_ready();

    if handle.is_readonly() {
        index_ready.store(true, Ordering::Release);
        if let Some(pipeline) = rag_pipeline {
            // rag.db is sidecar — no fs4 contention with the primary
            // LSP daemon — so the MCP --readonly daemon runs the RAG
            // cold start itself. Background tokio task so we don't
            // block axum::serve from booting. Note: on a FRESH
            // workspace where the LSP has not yet populated index.db,
            // the linker's workspace_fqdns lookup will return an empty
            // list, producing frontmatter-only links. The watcher will
            // pick up subsequent `.md` saves ; the user can also hit
            // `Standardoc: Rebuild RAG index` once the LSP cold start
            // completes to backfill the auto-fqdn / auto-name links.
            let rag_watcher_slot = mcp.rag_watcher_slot();
            tokio::spawn(async move {
                run_rag_cold_start(&pipeline, &handle, &filters);
                spawn_rag_watcher_into_slot(&rag_watcher_slot, handle, pipeline, filters);
            });
        }
        return;
    }

    let watcher_slot = mcp.watcher_slot();
    let rag_watcher_slot = mcp.rag_watcher_slot();
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
                match spawn_watcher(handle.clone(), provider, Arc::clone(&filters)) {
                    Ok(w) => {
                        if let Ok(mut g) = watcher_slot.lock() {
                            *g = Some(w);
                        }
                    }
                    Err(e) => eprintln!("standardoc mcp: watcher boot failed: {e}"),
                }
                if let Some(pipeline) = rag_pipeline {
                    run_rag_cold_start(&pipeline, &handle, &filters);
                    spawn_rag_watcher_into_slot(&rag_watcher_slot, handle, pipeline, filters);
                }
            }
            Err(e) => eprintln!("standardoc mcp: cold start failed: {e}"),
        }
    });
}

fn run_rag_cold_start(
    pipeline: &Arc<RagPipeline>,
    handle: &IndexHandle,
    filters: &Arc<RwLock<ScanFilters>>,
) {
    let Ok(guard) = filters.read() else {
        eprintln!("standardoc mcp: rag cold start aborted (filters poisoned)");
        return;
    };
    let workspace_root = handle.workspace_root().to_path_buf();
    if let Err(e) = pipeline.run_cold_start(&workspace_root, &guard, handle) {
        eprintln!("standardoc mcp: rag cold start failed: {e}");
    }
}

fn spawn_rag_watcher_into_slot(
    slot: &Arc<std::sync::Mutex<Option<standardoc_core::RagWatcherHandle>>>,
    handle: IndexHandle,
    pipeline: Arc<RagPipeline>,
    filters: Arc<RwLock<ScanFilters>>,
) {
    match spawn_rag_watcher(handle, pipeline, filters) {
        Ok(w) => {
            if let Ok(mut g) = slot.lock() {
                *g = Some(w);
            }
        }
        Err(e) => eprintln!("standardoc mcp: rag watcher boot failed: {e}"),
    }
}

fn with_rag_components(
    mcp: StandardocMcp,
    rag_pipeline: Option<&Arc<RagPipeline>>,
) -> StandardocMcp {
    let Some(pipeline) = rag_pipeline else {
        return mcp;
    };
    mcp.with_rag(pipeline.store_arc())
        .with_embedder(pipeline.embedder_arc())
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

/// RAG-aware variant of [`build_mcp_handler`] for tests / wiring that
/// want the tool surface populated. Pipeline is wired via
/// `with_rag`/`with_embedder` but NO cold-start / watcher is spawned —
/// callers needing the full boot flow should go through [`serve_mcp`]
/// or [`serve_mcp_http`].
pub fn build_mcp_handler_with_rag(
    handle: IndexHandle,
    provider: Arc<dyn LanguageProvider>,
    filters: Arc<RwLock<ScanFilters>>,
    rag_pipeline: &Arc<RagPipeline>,
) -> StandardocMcp {
    StandardocMcp::new(handle, provider, filters)
        .with_rag(rag_pipeline.store_arc())
        .with_embedder(rag_pipeline.embedder_arc())
}

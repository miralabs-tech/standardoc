use std::io;
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};

use rmcp::ServiceExt;
use rmcp::transport::stdio;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::tower::{
    StreamableHttpServerConfig, StreamableHttpService,
};
use standardoc_core::{IndexHandle, LanguageProvider, ScanFilters, spawn_watcher};
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
    let template = StandardocMcp::new(handle.clone(), Arc::clone(&provider), Arc::clone(&filters));
    kick_off_indexing(&template, handle, provider, filters);

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

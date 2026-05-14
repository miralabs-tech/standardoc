use std::io::{self, IsTerminal};
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};

use rmcp::ServiceExt;
use rmcp::transport::stdio;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::tower::{
    StreamableHttpServerConfig, StreamableHttpService,
};
use standardoc_core::{
    IndexHandle, LanguageProvider, RagPipeline, ScanFilters, spawn_rag_watcher,
    spawn_revision_relink_watcher, spawn_watcher,
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
    // When launched as a child process (VSCode extension supervisor, etc.),
    // stdin is a pipe connected to the parent. We don't read protocol data
    // from it — we use it as a death-watch channel: when the parent dies
    // (force-kill, BSOD, OOM-killer, ...), the OS closes its writer end,
    // our read returns Ok(0), and we exit cleanly. fs4 Drop runs, the
    // workspace `.standardoc/db.lock` is released, and the next daemon can
    // open the workspace without manual Task Manager cleanup of orphan
    // standardoc.exe processes.
    //
    // Gated on `!is_terminal()` so a direct `standardoc mcp --http <port>`
    // invocation from a TTY doesn't consume the user's keystrokes (and
    // doesn't exit if the user happens to Ctrl-D the terminal).
    if !io::stdin().is_terminal() {
        spawn_parent_death_watch();
    }

    let listener = match TcpListener::bind(bind_addr).await {
        Ok(l) => l,
        Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
            eprintln!(
                "standardoc mcp http: {bind_addr} already in use, falling back to an ephemeral port"
            );
            TcpListener::bind("127.0.0.1:0").await.map_err(|e| {
                ServerError::Io(io::Error::other(format!(
                    "bind 127.0.0.1:0 (ephemeral fallback): {e}"
                )))
            })?
        }
        Err(e) => {
            return Err(ServerError::Io(io::Error::other(format!(
                "bind {bind_addr}: {e}"
            ))));
        }
    };
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

/// Spawns a background tokio task that reads from stdin until EOF, then
/// exits the process. The supervisor pipes stdin to the child without
/// ever writing to it, so the only way `read` returns `Ok(0)` is when
/// the parent end of the pipe is closed — i.e. the parent process is
/// gone. This is the portable orphan-prevention mechanism: works on
/// Windows (where there is no `PR_SET_PDEATHSIG`) and on Unix without
/// requiring job objects / process groups.
fn spawn_parent_death_watch() {
    tokio::spawn(async {
        use tokio::io::AsyncReadExt;
        let mut stdin = tokio::io::stdin();
        let mut buf = [0u8; 64];
        loop {
            match stdin.read(&mut buf).await {
                Ok(0) => {
                    eprintln!("standardoc mcp http: parent stdin closed, exiting");
                    std::process::exit(0);
                }
                Ok(_) => {
                    // Unexpected data on the watch channel. The supervisor
                    // never writes to stdin, but if a future protocol
                    // appears here we just drain it and keep watching.
                }
                Err(e) => {
                    eprintln!("standardoc mcp http: stdin read error ({e}), exiting");
                    std::process::exit(0);
                }
            }
        }
    });
}

fn write_endpoint_file(
    workspace_root: &std::path::Path,
    endpoint: &str,
) -> Result<(), ServerError> {
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
        // MCP --readonly is the SECONDARY daemon in the dual-daemon
        // setup : the LSP holds the fs4 lock and owns every write —
        // AST AND RAG. rag.db is sidecar so SQLite WAL would
        // technically allow concurrent writes, but two writers racing
        // on the same `chunks` rows produced a `FOREIGN KEY constraint
        // failed` when one daemon's relink read chunk ids that the
        // other daemon's cold-start had already replaced. The pipeline
        // is still wired into [`StandardocMcp`] via `with_rag_components`
        // so query-time `chunk_refs` / `fetch_chunks` keep working ;
        // we just don't kick off any background write tasks here.
        drop(rag_pipeline);
        return;
    }

    let watcher_slot = mcp.watcher_slot();
    let rag_watcher_slot = mcp.rag_watcher_slot();
    let rag_relink_slot = mcp.rag_relink_watcher_slot();
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
                    run_rag_relink_all(&pipeline, &handle);
                    run_rag_cold_start(&pipeline, &handle, &filters);
                    spawn_revision_relink_into_slot(
                        &rag_relink_slot,
                        handle.clone(),
                        Arc::clone(&pipeline),
                    );
                    spawn_rag_watcher_into_slot(&rag_watcher_slot, handle, pipeline, filters);
                }
            }
            Err(e) => eprintln!("standardoc mcp: cold start failed: {e}"),
        }
    });
}

pub(crate) fn run_rag_cold_start(
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

pub(crate) fn spawn_rag_watcher_into_slot(
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

/// One-shot relink pass against the current AST graph. Cheap when the
/// workspace fqdn set hasn't changed since the previous sweep (single
/// `SELECT value FROM schema_meta` + BLAKE3 over the sorted fqdn list).
/// Errors are logged and swallowed — the daemon must boot even when
/// relink fails.
pub(crate) fn run_rag_relink_all(pipeline: &Arc<RagPipeline>, handle: &IndexHandle) {
    if let Err(e) = pipeline.relink_all(handle.workspace_root(), handle) {
        eprintln!("standardoc mcp: rag relink_all failed: {e}");
    }
}

/// Spawns the revision-driven relink watcher and stashes the handle in
/// `slot`. Errors are logged ; the absence of a revision watcher only
/// means the daemon falls back to the boot-time relink — long-running
/// editing sessions may drift until the next restart.
pub(crate) fn spawn_revision_relink_into_slot(
    slot: &Arc<std::sync::Mutex<Option<standardoc_core::RevisionRelinkHandle>>>,
    handle: IndexHandle,
    pipeline: Arc<RagPipeline>,
) {
    match spawn_revision_relink_watcher(handle, pipeline) {
        Ok(w) => {
            if let Ok(mut g) = slot.lock() {
                *g = Some(w);
            }
        }
        Err(e) => eprintln!("standardoc mcp: rag relink watcher boot failed: {e}"),
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

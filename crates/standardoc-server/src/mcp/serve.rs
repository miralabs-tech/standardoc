use std::io::{self, IsTerminal};
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

    // Port-stability shortcut for ephemeral-bind callers (typically the
    // VSCode supervisor that passes `127.0.0.1:0`): when a previous
    // daemon left an `mcp.endpoint` behind, try to bind the SAME port
    // first. If the previous process is properly dead the port is free
    // and we reuse it transparently — Claude Code's cached MCP URL
    // stays valid across restart, no manual reconnect dance. If the
    // port is taken (concurrent daemon, OS hasn't released it yet) we
    // fall through to the caller's requested bind_addr.
    let listener = match maybe_reuse_previous_port(handle.workspace_root(), bind_addr).await {
        Some(l) => {
            eprintln!(
                "standardoc mcp http: reused previous port from mcp.endpoint (CC reconnect-safe)"
            );
            l
        }
        None => match TcpListener::bind(bind_addr).await {
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
        },
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
    let template = StandardocMcp::new(handle.clone(), Arc::clone(&provider), Arc::clone(&filters));
    kick_off_indexing(&template, handle, provider, filters);

    // Stateless + json_response : each MCP request is self-contained,
    // the response is plain `application/json` (no SSE stream to lose).
    // Two reasons for this choice over the session-store path :
    //
    // 1. Multi-client safety. The previous experiment with
    //    `stateful_mode = true` re-introduced the
    //    `ERR_INCOMPLETE_CHUNKED_ENCODING` storm we'd seen when the
    //    playground (Chrome) and Claude Code shared the daemon — the
    //    SSE stream's death evicted live sessions.
    //
    // 2. Reconnect ergonomics. With a long-lived SSE channel, Claude
    //    Code's MCP transport treats the stream's death (daemon
    //    restart, rebuild, migration) as a hard "server down" signal
    //    and refuses to retry until the user manually reconnects from
    //    settings. Stateless mode has no persistent channel — every
    //    tool call is a fresh HTTP request, and the next call lands on
    //    the new daemon transparently. Combined with `mcp.endpoint`
    //    port reuse, daemon restarts become invisible to CC.
    //
    // The `FileSessionStore` lives in this crate as dead code for now
    // — kept around because rmcp's session-store hook is the right
    // primitive if/when we want a stateful transport again (e.g. for
    // server-pushed notifications). Not wired today.
    //
    // `StreamableHttpServerConfig` is `#[non_exhaustive]` since rmcp
    // 1.0 — build via Default then override the fields we care about.
    let mut http_cfg = StreamableHttpServerConfig::default();
    http_cfg.stateful_mode = false;
    http_cfg.json_response = true;
    http_cfg.cancellation_token = CancellationToken::new();
    let service = StreamableHttpService::new(
        move || Ok(template.clone()),
        Arc::new(LocalSessionManager::default()),
        http_cfg,
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

/// When the caller requested an ephemeral port (`...:0`), try to bind the
/// port left behind by the previous daemon process (recorded in
/// `mcp.endpoint`). Returns `Some(listener)` on success — Claude Code's
/// cached MCP URL keeps working across the restart with no manual
/// reconnect needed. Returns `None` when (a) the caller wants a specific
/// port, (b) no previous endpoint exists, (c) the endpoint is malformed,
/// or (d) the previous port is now busy.
async fn maybe_reuse_previous_port(
    workspace_root: &std::path::Path,
    bind_addr: &str,
) -> Option<TcpListener> {
    if !wants_ephemeral_port(bind_addr) {
        return None;
    }
    let port = read_previous_port(workspace_root)?;
    TcpListener::bind(("127.0.0.1", port)).await.ok()
}

fn wants_ephemeral_port(bind_addr: &str) -> bool {
    bind_addr
        .parse::<std::net::SocketAddr>()
        .is_ok_and(|sa| sa.port() == 0)
}

/// Parse the `port` out of `http://<host>:<port>/mcp` recorded in the
/// previous boot's `mcp.endpoint` file. Best-effort — any read / parse
/// failure returns `None` and the caller falls through to a fresh bind.
fn read_previous_port(workspace_root: &std::path::Path) -> Option<u16> {
    let path = workspace_root.join(".standardoc").join("mcp.endpoint");
    let content = std::fs::read_to_string(&path).ok()?;
    let trimmed = content.trim();
    let after_scheme = trimmed.strip_prefix("http://")?;
    let authority = after_scheme.split('/').next()?;
    let (_host, port_str) = authority.rsplit_once(':')?;
    port_str.parse::<u16>().ok()
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

#[cfg(test)]
mod tests;

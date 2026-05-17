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
use super::session_store::FileSessionStore;
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
    let template = with_rag_components(
        StandardocMcp::new(handle.clone(), Arc::clone(&provider), Arc::clone(&filters)),
        rag_pipeline.as_ref(),
    );
    let workspace_root = handle.workspace_root().to_path_buf();
    kick_off_indexing(&template, handle, provider, filters, rag_pipeline);

    // rmcp 1.7 stateful + persisted SessionStore: stateful mode keeps
    // per-session state server-side and lets the streamable-http
    // transport reuse a stable `Mcp-Session-Id` header across calls.
    // Combined with a persisted `SessionStore` rooted at
    // `<workspace>/.standardoc/mcp-sessions.json`, this lets a new
    // daemon process (post-rebuild, post-restart, post-migration)
    // transparently restore Claude Code's session : the next request
    // lands on the new instance with a stale session_id, the in-memory
    // table misses, the store is consulted, the initialize handshake
    // is replayed from disk, and the call proceeds without any
    // re-init dance. The CC user no longer has to manually reconnect
    // the MCP server from settings after every dev rebuild.
    //
    // Trade-off : stateful_mode forces the SSE response path (the
    // `json_response` field is ignored when stateful_mode is true).
    // The pre-3e-1 commit that flipped stateful_mode to false did so
    // because the playground (Chrome) plus CC concurrent connections
    // triggered Chrome's ERR_INCOMPLETE_CHUNKED_ENCODING storm and
    // evicted CC's session. With session persistence in place, that
    // eviction becomes recoverable — the next CC call restores the
    // session from disk transparently. If the eviction storm itself
    // proves disruptive in practice (concurrent CC + playground), a
    // dedicated stateless `/mcp-stateless` endpoint is a clean
    // follow-up — same handler, different config.
    //
    // `StreamableHttpServerConfig` is `#[non_exhaustive]` since rmcp
    // 1.0 — build via Default then override the fields we care about.
    let session_store: Arc<dyn rmcp::transport::streamable_http_server::session::store::SessionStore> =
        Arc::new(FileSessionStore::new(&workspace_root));
    let mut http_cfg = StreamableHttpServerConfig::default();
    http_cfg.stateful_mode = true;
    http_cfg.cancellation_token = CancellationToken::new();
    http_cfg.session_store = Some(session_store);
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
        .map(|sa| sa.port() == 0)
        .unwrap_or(false)
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn wants_ephemeral_port_recognizes_zero() {
        assert!(wants_ephemeral_port("127.0.0.1:0"));
        assert!(wants_ephemeral_port("0.0.0.0:0"));
    }

    #[test]
    fn wants_ephemeral_port_rejects_specific_port() {
        assert!(!wants_ephemeral_port("127.0.0.1:8765"));
        assert!(!wants_ephemeral_port("127.0.0.1:443"));
    }

    #[test]
    fn wants_ephemeral_port_rejects_unparseable() {
        assert!(!wants_ephemeral_port("not-a-socket-addr"));
        assert!(!wants_ephemeral_port(""));
    }

    #[test]
    fn read_previous_port_parses_endpoint_file() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".standardoc");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mcp.endpoint"), "http://127.0.0.1:56831/mcp").unwrap();
        assert_eq!(read_previous_port(tmp.path()), Some(56831));
    }

    #[test]
    fn read_previous_port_trims_trailing_newline() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".standardoc");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mcp.endpoint"), "http://127.0.0.1:9000/mcp\n").unwrap();
        assert_eq!(read_previous_port(tmp.path()), Some(9000));
    }

    #[test]
    fn read_previous_port_missing_file_is_none() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(read_previous_port(tmp.path()), None);
    }

    #[test]
    fn read_previous_port_malformed_is_none() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".standardoc");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mcp.endpoint"), "not a url").unwrap();
        assert_eq!(read_previous_port(tmp.path()), None);
    }

    #[tokio::test]
    async fn maybe_reuse_skips_when_caller_wants_specific_port() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".standardoc");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mcp.endpoint"), "http://127.0.0.1:56831/mcp").unwrap();
        // Caller asked for a SPECIFIC port — reuse must be a no-op so
        // the caller's intent wins.
        let result = maybe_reuse_previous_port(tmp.path(), "127.0.0.1:7777").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn maybe_reuse_returns_listener_when_port_is_free() {
        // First, bind an ephemeral listener and capture its address —
        // dropping it frees the port. Then write that as the previous
        // endpoint and verify the reuse path re-binds it cleanly.
        let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".standardoc");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("mcp.endpoint"),
            format!("http://127.0.0.1:{port}/mcp"),
        )
        .unwrap();
        let listener = maybe_reuse_previous_port(tmp.path(), "127.0.0.1:0")
            .await
            .expect("port should be reusable after probe drop");
        assert_eq!(listener.local_addr().unwrap().port(), port);
    }
}

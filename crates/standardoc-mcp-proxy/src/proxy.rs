#![allow(
    clippy::significant_drop_tightening,
    clippy::significant_drop_in_scrutinee
)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Path as AxPath, Request, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use bytes::Bytes;
use http::HeaderValue;
use notify_debouncer_full::notify::{EventKind, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, new_debouncer};
use std::net::SocketAddr;
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::sync::{RwLock, mpsc};

/// Resolved proxy configuration. Built by the CLI layer from clap args
/// then passed to [`run`].
pub struct ProxyConfig {
    /// Local bind address — typically `127.0.0.1:7700`. Clients connect
    /// to `http://<bind_addr>/mcp` (default workspace, back-compat) or
    /// `http://<bind_addr>/ws/<id>/mcp` (explicit workspace).
    pub bind_addr: String,
    /// One or more workspace roots to serve. Each one gets a
    /// deterministic blake3-derived `id` and its own
    /// `.standardoc/mcp.endpoint` watcher. The FIRST entry is the
    /// default workspace surfaced at `/mcp` for back-compat with
    /// single-workspace clients.
    pub workspaces: Vec<PathBuf>,
    /// How long to keep retrying when the upstream daemon is
    /// unreachable (connection refused, hyper transport error). After
    /// this window the proxy returns `503 Service Unavailable`. Default
    /// 30 s gives a daemon rebuild + cold-start plenty of headroom
    /// without making the MCP client wait forever on a permanent
    /// failure.
    pub upstream_retry_window: Duration,
}

/// Compute the stable short ID for a workspace path. blake3-then-hex,
/// truncated to 8 chars. Deterministic per canonical absolute path so
/// external clients can construct `/ws/<id>/mcp` URLs without a
/// registration round-trip — `standardoc workspace-id <path>` exposes
/// the same function for scripts.
#[must_use]
pub fn workspace_id_for(path: &Path) -> String {
    let canonical = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_string();
    let hash = blake3::hash(canonical.as_bytes()).to_hex();
    hash.as_str()[..8].to_string()
}

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("bind {0}: {1}")]
    Bind(String, std::io::Error),
    #[error("axum serve: {0}")]
    Serve(std::io::Error),
    #[error("watch `.standardoc/mcp.endpoint`: {0}")]
    Watcher(String),
    #[error("no workspaces configured — pass --workspace at least once")]
    NoWorkspaces,
}

/// Bind, attach a per-workspace file watcher, register the routing
/// table, and block until shutdown. Does NOT exit on upstream failures
/// — the whole point of the proxy is to outlive every daemon process
/// it forwards to.
///
/// **Singleton behaviour**: before binding, probes `GET /health` on
/// `bind_addr`. If a standardoc proxy is already listening there
/// (response carries `default_workspace_id`), this invocation skips
/// the bind, registers every requested workspace with that running
/// proxy via `POST /admin/workspaces`, and returns Ok(()). Sibling
/// VSCode windows therefore converge on a single shared proxy
/// instance without coordination — the first one wins the port, the
/// rest just register and exit cleanly.
pub async fn run(cfg: ProxyConfig) -> Result<(), ProxyError> {
    if cfg.workspaces.is_empty() {
        return Err(ProxyError::NoWorkspaces);
    }
    if let Some(existing) = probe_existing_proxy(&cfg.bind_addr).await {
        return register_with_existing(&existing, &cfg.workspaces).await;
    }
    let mut workspaces: HashMap<String, Arc<WorkspaceEntry>> = HashMap::new();
    let mut default_id: Option<String> = None;
    for root in &cfg.workspaces {
        let entry = init_workspace(root.clone())?;
        if default_id.is_none() {
            default_id = Some(entry.id.clone());
        }
        eprintln!(
            "standardoc-mcp-proxy: registered ws {} ← {}",
            entry.id,
            entry.root.display()
        );
        workspaces.insert(entry.id.clone(), Arc::new(entry));
    }
    let default_id = default_id.expect("workspaces non-empty checked above");

    let client = build_forward_client();
    let started_at = SystemTime::now();
    let state = Arc::new(ProxyState {
        workspaces: RwLock::new(workspaces),
        default_id,
        client,
        retry_window: cfg.upstream_retry_window,
        total_requests: AtomicU64::new(0),
        successful_requests: AtomicU64::new(0),
        upstream_503_responses: AtomicU64::new(0),
        last_request_at: RwLock::new(None),
        started_at,
    });

    let listener = TcpListener::bind(&cfg.bind_addr)
        .await
        .map_err(|e| ProxyError::Bind(cfg.bind_addr.clone(), e))?;
    let local = listener
        .local_addr()
        .map_err(|e| ProxyError::Bind(cfg.bind_addr.clone(), e))?;
    eprintln!("standardoc-mcp-proxy: listening on http://{local}/mcp (default ws)");
    for entry in state.workspaces.read().await.values() {
        eprintln!(
            "standardoc-mcp-proxy:   - http://{local}/ws/{}/mcp ← {}",
            entry.id,
            entry.root.display()
        );
    }
    eprintln!("standardoc-mcp-proxy: health endpoint: http://{local}/health");
    eprintln!("standardoc-mcp-proxy: admin: POST/GET/DELETE http://{local}/admin/workspaces");

    let app = Router::new()
        .route("/health", get(health))
        .route(
            "/admin/workspaces",
            get(admin_list_workspaces).post(admin_register_workspace),
        )
        .route(
            "/admin/workspaces/{id}",
            axum::routing::delete(admin_unregister_workspace),
        )
        .route("/ws/{id}/mcp", any(forward_ws))
        .route("/ws/{id}/{*path}", any(forward_ws))
        .route("/mcp", any(forward_default))
        .route("/{*path}", any(forward_default))
        .with_state(state);

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .map_err(ProxyError::Serve)
}

/// Per-workspace tracked state — id, root, and the live upstream URL
/// (updated by the per-workspace endpoint-file watcher).
pub(crate) struct WorkspaceEntry {
    pub(crate) id: String,
    pub(crate) root: PathBuf,
    pub(crate) upstream: Arc<RwLock<String>>,
}

fn init_workspace(root: PathBuf) -> Result<WorkspaceEntry, ProxyError> {
    let id = workspace_id_for(&root);
    let endpoint_path = root.join(".standardoc").join("mcp.endpoint");
    let initial = read_endpoint_file(&endpoint_path).unwrap_or_default();
    let upstream = Arc::new(RwLock::new(initial.clone()));
    spawn_endpoint_watcher(endpoint_path.clone(), Arc::clone(&upstream))?;
    if initial.is_empty() {
        eprintln!(
            "standardoc-mcp-proxy: ws {id}: no endpoint yet at {} — will start forwarding once the daemon writes its URL",
            endpoint_path.display()
        );
    } else {
        eprintln!("standardoc-mcp-proxy: ws {id}: upstream = {initial}");
    }
    Ok(WorkspaceEntry { id, root, upstream })
}

struct ProxyState {
    /// Mutable at runtime so the admin endpoints can register /
    /// unregister workspaces without bouncing the proxy. Reads on the
    /// hot forward path are short-lived (clone the Arc<WorkspaceEntry>
    /// and release) so write-side admin churn doesn't stall traffic.
    workspaces: RwLock<HashMap<String, Arc<WorkspaceEntry>>>,
    /// First entry's id at boot — what `/mcp` (no ws prefix) falls
    /// back to, preserving single-workspace back-compat. Immutable for
    /// the lifetime of the proxy; refusing to delete it keeps the
    /// fallback route stable.
    default_id: String,
    client: reqwest::Client,
    retry_window: Duration,
    /// Lifetime counter of HTTP requests landed on the proxy
    /// (forwarder hits only — `/health` is excluded). Lets users grep
    /// the `/health` response to confirm the MCP client is talking
    /// to the proxy at all.
    total_requests: AtomicU64,
    /// Lifetime counter of forwarder responses with `< 400` status.
    /// Together with `total_requests` gives a quick failure rate.
    successful_requests: AtomicU64,
    /// Lifetime counter of `503` we surfaced because the upstream
    /// stayed unreachable past `retry_window`. Distinct from
    /// `total_requests - successful_requests` (which also counts
    /// upstream 4xx / 5xx that round-tripped fine).
    upstream_503_responses: AtomicU64,
    /// Wall-clock timestamp of the last forwarder hit. `None` means
    /// the proxy has been idle since boot.
    last_request_at: RwLock<Option<SystemTime>>,
    /// Wall-clock timestamp of `run()`'s start. Used by `/health` to
    /// surface an uptime.
    started_at: SystemTime,
}

/// Probe `http://<bind_addr>/health` with a short timeout. Returns
/// the canonical base URL (`http://<bind_addr>`) on a 200 whose body
/// contains the `default_workspace_id` marker — anything else is
/// treated as "no proxy here", letting the caller proceed with the
/// normal bind path (which then surfaces the real error if the port
/// is held by something unrelated).
async fn probe_existing_proxy(bind_addr: &str) -> Option<String> {
    let base = format!("http://{bind_addr}");
    let url = format!("{base}/health");
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_millis(300))
        .timeout(Duration::from_millis(800))
        .build()
        .ok()?;
    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body = resp.text().await.ok()?;
    // Cheap marker check — avoids dragging serde_json into the proxy
    // just to confirm the responder is a standardoc proxy.
    if body.contains("default_workspace_id") {
        Some(base)
    } else {
        None
    }
}

/// POST each workspace path to `<base>/admin/workspaces`. Logs each
/// outcome to stderr and returns Ok(()) once every entry has been
/// attempted — partial failures (already-registered, etc.) are
/// considered non-fatal because admin register is idempotent on the
/// server side. A wholly unreachable proxy mid-way through is
/// surfaced as a `Watcher` error so the CLI fails visibly.
async fn register_with_existing(base_url: &str, workspaces: &[PathBuf]) -> Result<(), ProxyError> {
    eprintln!(
        "standardoc-mcp-proxy: existing proxy detected at {base_url} — registering workspaces"
    );
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| ProxyError::Watcher(format!("singleton client init: {e}")))?;
    let mut any_error: Option<String> = None;
    for root in workspaces {
        let canonical = root
            .canonicalize()
            .unwrap_or_else(|_| root.clone())
            .to_string_lossy()
            .into_owned();
        let body = format!(
            r#"{{"path":"{}"}}"#,
            canonical.replace('\\', "\\\\").replace('"', "\\\"")
        );
        let url = format!("{base_url}/admin/workspaces");
        match client
            .post(&url)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                eprintln!(
                    "standardoc-mcp-proxy: registered {} with existing proxy ({})",
                    root.display(),
                    resp.status()
                );
            }
            Ok(resp) => {
                let status = resp.status();
                let txt = resp.text().await.unwrap_or_default();
                let msg = format!("register {} failed: HTTP {status}: {txt}", root.display());
                eprintln!("standardoc-mcp-proxy: {msg}");
                any_error.get_or_insert(msg);
            }
            Err(e) => {
                let msg = format!("register {} failed: {e}", root.display());
                eprintln!("standardoc-mcp-proxy: {msg}");
                any_error.get_or_insert(msg);
            }
        }
    }
    match any_error {
        Some(msg) => Err(ProxyError::Watcher(msg)),
        None => Ok(()),
    }
}

fn build_forward_client() -> reqwest::Client {
    reqwest::Client::builder()
        // No connection-pool reuse for upstream — keeps things simple
        // when the daemon address changes mid-flight.
        .pool_max_idle_per_host(0)
        // Short connection timeout so the per-attempt retry loop can
        // make progress instead of stalling on a single attempt.
        .connect_timeout(Duration::from_millis(500))
        // Generous overall request timeout — daemon cold-start +
        // expensive tool calls can take several seconds.
        .timeout(Duration::from_mins(1))
        .build()
        .expect("reqwest client builds with vanilla config")
}

/// Read `mcp.endpoint`, trimmed. Returns empty string on any error so
/// the caller can treat "missing" and "unreadable" the same way.
fn read_endpoint_file(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let trimmed = raw.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Spawns a debounced file watcher on `mcp.endpoint`. Whenever the file
/// changes (daemon restart writes a new URL), the shared `upstream`
/// cell is updated. 200 ms debounce avoids reacting to mid-write
/// states when the daemon writes via tmp + rename (it doesn't today
/// but the headroom is free).
fn spawn_endpoint_watcher(
    endpoint_path: PathBuf,
    upstream: Arc<RwLock<String>>,
) -> Result<(), ProxyError> {
    let parent = endpoint_path
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| ProxyError::Watcher("endpoint path has no parent dir".into()))?;
    if !parent.exists()
        && let Err(e) = std::fs::create_dir_all(&parent)
    {
        return Err(ProxyError::Watcher(format!(
            "create {}: {e}",
            parent.display()
        )));
    }
    let (tx, mut rx) = mpsc::unbounded_channel::<DebounceEventResult>();
    let mut debouncer = new_debouncer(Duration::from_millis(200), None, move |res| {
        let _ = tx.send(res);
    })
    .map_err(|e| ProxyError::Watcher(format!("init debouncer: {e}")))?;
    debouncer
        .watch(&parent, RecursiveMode::NonRecursive)
        .map_err(|e| ProxyError::Watcher(format!("watch {}: {e}", parent.display())))?;

    let endpoint_clone = endpoint_path;
    tokio::spawn(async move {
        // Keep the debouncer alive for the lifetime of this task.
        let _keepalive = debouncer;
        while let Some(events) = rx.recv().await {
            let Ok(events) = events else { continue };
            let mut interesting = false;
            for event in events {
                if matches!(
                    event.event.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                ) && event.event.paths.iter().any(|p| p == &endpoint_clone)
                {
                    interesting = true;
                    break;
                }
            }
            if !interesting {
                continue;
            }
            if let Some(new_url) = read_endpoint_file(&endpoint_clone) {
                let mut guard = upstream.write().await;
                if *guard != new_url {
                    eprintln!("standardoc-mcp-proxy: upstream → {new_url}");
                    *guard = new_url;
                }
            } else {
                eprintln!(
                    "standardoc-mcp-proxy: endpoint file gone — will pause forwarding until it reappears"
                );
                upstream.write().await.clear();
            }
        }
    });
    Ok(())
}

/// Back-compat handler — paths without a `/ws/<id>` prefix forward to
/// the default workspace (first registered). Used by single-workspace
/// clients that predate multi-workspace routing.
async fn forward_default(
    State(state): State<Arc<ProxyState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    let default_id = state.default_id.clone();
    let Some(ws) = state.workspaces.read().await.get(&default_id).cloned() else {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "standardoc-mcp-proxy: default workspace missing (bug)".into(),
        );
    };
    forward_to(state, ws, peer, req, None).await
}

/// Multi-workspace handler — extracts the workspace id from the
/// `/ws/{id}/...` path, strips that prefix before composing the
/// upstream URL, and forwards. Returns 404 when the id isn't
/// registered.
async fn forward_ws(
    State(state): State<Arc<ProxyState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    AxPath(id): AxPath<String>,
    req: Request,
) -> Response {
    let Some(ws) = state.workspaces.read().await.get(&id).cloned() else {
        return error_response(
            StatusCode::NOT_FOUND,
            format!("standardoc-mcp-proxy: unknown workspace id `{id}`"),
        );
    };
    forward_to(state, ws, peer, req, Some(id)).await
}

/// Shared forward path — retries against `ws.upstream` while the
/// daemon is restarting, streams the response back. Buffers the
/// request body so retries can replay it (MCP payloads are small JSON
/// — at most a few KB). Logs each incoming call so users can confirm
/// the MCP client (Claude Code, …) is actually talking to the proxy.
///
/// `strip_ws_prefix` carries `Some(id)` for `/ws/{id}/...` requests so
/// the target URL drops the `/ws/{id}` segment when re-composing; the
/// daemon doesn't know its own routing prefix.
async fn forward_to(
    state: Arc<ProxyState>,
    ws: Arc<WorkspaceEntry>,
    peer: SocketAddr,
    req: Request,
    strip_ws_prefix: Option<String>,
) -> Response {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let headers = req.headers().clone();
    let seq = state.total_requests.fetch_add(1, Ordering::Relaxed) + 1;
    *state.last_request_at.write().await = Some(SystemTime::now());
    eprintln!(
        "standardoc-mcp-proxy: incoming #{seq} ws={} {method} {} from {peer}",
        ws.id,
        uri.path()
    );
    let body_bytes = match axum::body::to_bytes(req.into_body(), 8 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                format!("standardoc-mcp-proxy: failed to buffer request body: {e}"),
            );
        }
    };

    let deadline = tokio::time::Instant::now() + state.retry_window;
    let mut backoff = Duration::from_millis(100);
    #[allow(unused_assignments)]
    let mut last_error: Option<String> = None;

    loop {
        let upstream_url = ws.upstream.read().await.clone();
        if upstream_url.is_empty() {
            last_error = Some(format!(
                "upstream endpoint not yet known for ws {} (waiting for daemon)",
                ws.id
            ));
        } else {
            let target = build_target_url(&upstream_url, &uri, strip_ws_prefix.as_deref());
            match forward_once(
                &state.client,
                &method,
                &target,
                &headers,
                body_bytes.clone(),
            )
            .await
            {
                Ok(resp) => {
                    if resp.status().as_u16() < 400 {
                        state.successful_requests.fetch_add(1, Ordering::Relaxed);
                    }
                    return resp;
                }
                Err(e) if e.is_retryable() => last_error = Some(e.into_message()),
                Err(e) => {
                    return error_response(
                        StatusCode::BAD_GATEWAY,
                        format!("standardoc-mcp-proxy: upstream fatal: {}", e.into_message()),
                    );
                }
            }
        }

        if tokio::time::Instant::now() >= deadline {
            state.upstream_503_responses.fetch_add(1, Ordering::Relaxed);
            let detail = last_error.unwrap_or_else(|| "upstream unreachable".into());
            eprintln!(
                "standardoc-mcp-proxy: 503 for #{seq} ws={} — upstream unreachable after {}s: {detail}",
                ws.id,
                state.retry_window.as_secs()
            );
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                format!(
                    "standardoc-mcp-proxy: upstream still unreachable after {}s: {detail}",
                    state.retry_window.as_secs()
                ),
            );
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(2));
    }
}

/// `GET /health` — JSON snapshot of proxy state. Used by users /
/// scripts to confirm the proxy is alive, every registered workspace
/// has a known upstream, and the MCP client has been hitting the
/// proxy. Counts forwarder hits only (this endpoint itself is
/// excluded from the totals).
async fn health(State(state): State<Arc<ProxyState>>) -> Response {
    let last_request_age_ms = match *state.last_request_at.read().await {
        Some(ts) => SystemTime::now()
            .duration_since(ts)
            .ok()
            .map(|d| d.as_millis()),
        None => None,
    };
    let uptime_ms = SystemTime::now()
        .duration_since(state.started_at)
        .ok()
        .map_or(0, |d| d.as_millis());
    let total = state.total_requests.load(Ordering::Relaxed);
    let ok = state.successful_requests.load(Ordering::Relaxed);
    let upstream_503 = state.upstream_503_responses.load(Ordering::Relaxed);

    let workspaces_json = workspaces_to_json(&state.workspaces).await;

    let body = format!(
        "{{\"default_workspace_id\":\"{default}\",\"workspaces\":{workspaces_json},\
         \"uptime_ms\":{uptime_ms},\"total_requests\":{total},\
         \"successful_requests\":{ok},\"upstream_503_responses\":{upstream_503},\
         \"last_request_age_ms\":{age}}}",
        default = state.default_id,
        age = match last_request_age_ms {
            Some(ms) => ms.to_string(),
            None => "null".into(),
        },
    );
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

/// Bare-minimum JSON string escaping for `"` and `\` only — workspace
/// roots can contain backslashes on Windows and we don't want them to
/// produce malformed JSON. Control characters get passed through; if
/// you have a TAB in a path you have bigger problems than `/health`.
fn escape_json(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Render the current `workspaces` map as a JSON array — shared between
/// `/health` and `GET /admin/workspaces` so the shape stays consistent.
async fn workspaces_to_json(workspaces: &RwLock<HashMap<String, Arc<WorkspaceEntry>>>) -> String {
    let map = workspaces.read().await;
    let mut entries: Vec<String> = Vec::with_capacity(map.len());
    for ws in map.values() {
        let upstream = ws.upstream.read().await.clone();
        entries.push(format!(
            "{{\"id\":\"{id}\",\"root\":\"{root}\",\"upstream\":\"{upstream}\",\"upstream_known\":{known}}}",
            id = ws.id,
            root = escape_json(&ws.root.to_string_lossy()),
            upstream = escape_json(&upstream),
            known = !upstream.is_empty(),
        ));
    }
    format!("[{}]", entries.join(","))
}

/// `GET /admin/workspaces` — list every registered workspace plus the
/// id of the default (which `/mcp` routes to). Shape mirrors the
/// `workspaces` array inside `/health` but stripped of traffic
/// counters.
async fn admin_list_workspaces(State(state): State<Arc<ProxyState>>) -> Response {
    let workspaces_json = workspaces_to_json(&state.workspaces).await;
    let body = format!(
        "{{\"default_workspace_id\":\"{default}\",\"workspaces\":{workspaces_json}}}",
        default = state.default_id,
    );
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

/// `POST /admin/workspaces` — register a new workspace at runtime.
/// Body: `{"path": "/abs/workspace"}`. Idempotent — re-registering
/// the same canonical path returns the existing entry. Responds with
/// `{"id":"...","root":"...","upstream":"...","upstream_known":bool,
/// "default":bool}` so the caller can confirm both routing slot and
/// whether the daemon has come up yet.
async fn admin_register_workspace(State(state): State<Arc<ProxyState>>, body: String) -> Response {
    let path = match parse_register_body(&body) {
        Ok(p) => p,
        Err(msg) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                format!("standardoc-mcp-proxy: admin register: {msg}"),
            );
        }
    };
    let id = workspace_id_for(&path);
    {
        // Idempotent: same canonical path → same id, return existing.
        let map = state.workspaces.read().await;
        if let Some(ws) = map.get(&id) {
            let upstream = ws.upstream.read().await.clone();
            return admin_entry_response(StatusCode::OK, ws, &upstream, &state.default_id);
        }
    }
    let entry = match init_workspace(path.clone()) {
        Ok(e) => Arc::new(e),
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("standardoc-mcp-proxy: admin register: {e}"),
            );
        }
    };
    eprintln!(
        "standardoc-mcp-proxy: admin: registered ws {} ← {}",
        entry.id,
        entry.root.display()
    );
    let upstream = entry.upstream.read().await.clone();
    let ws_for_response = Arc::clone(&entry);
    state
        .workspaces
        .write()
        .await
        .insert(entry.id.clone(), entry);
    admin_entry_response(
        StatusCode::CREATED,
        &ws_for_response,
        &upstream,
        &state.default_id,
    )
}

/// `DELETE /admin/workspaces/{id}` — unregister. The default
/// workspace cannot be deleted (would orphan the `/mcp` back-compat
/// route) — returns 409 in that case. The endpoint-file watcher for
/// the removed entry keeps running silently until process exit; the
/// associated RAM is bounded and small.
async fn admin_unregister_workspace(
    State(state): State<Arc<ProxyState>>,
    AxPath(id): AxPath<String>,
) -> Response {
    if id == state.default_id {
        return error_response(
            StatusCode::CONFLICT,
            format!(
                "standardoc-mcp-proxy: admin unregister: ws `{id}` is the default workspace \
                 and cannot be removed (would orphan /mcp back-compat route)"
            ),
        );
    }
    let removed = state.workspaces.write().await.remove(&id);
    if removed.is_none() {
        return error_response(
            StatusCode::NOT_FOUND,
            format!("standardoc-mcp-proxy: admin unregister: unknown workspace id `{id}`"),
        );
    }
    eprintln!("standardoc-mcp-proxy: admin: unregistered ws {id}");
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    response
}

/// Parse `{"path":"..."}` from the request body. Tolerant of
/// whitespace and trailing newlines ; rejects empty paths, malformed
/// JSON, and missing fields.
fn parse_register_body(body: &str) -> Result<PathBuf, String> {
    // Minimal hand-rolled parse to stay serde-free. The body shape is
    // a tight contract (`{"path": "..."}`) so a regex-grade scan is
    // enough — saves a serde_json dep + 1 transitive crate.
    let trimmed = body.trim();
    let stripped = trimmed
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .ok_or_else(|| "body must be a JSON object".to_string())?;
    let mut path: Option<String> = None;
    for entry in stripped.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (key, value) = entry
            .split_once(':')
            .ok_or_else(|| format!("malformed entry `{entry}` (expected `key:value`)"))?;
        let key = key.trim().trim_matches('"');
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .ok_or_else(|| format!("value for `{key}` must be a quoted string"))?;
        if key == "path" {
            path = Some(value.replace("\\\\", "\\").replace("\\\"", "\""));
        }
    }
    let path = path.ok_or_else(|| "missing `path` field".to_string())?;
    if path.is_empty() {
        return Err("`path` must not be empty".to_string());
    }
    Ok(PathBuf::from(path))
}

fn admin_entry_response(
    status: StatusCode,
    ws: &WorkspaceEntry,
    upstream: &str,
    default_id: &str,
) -> Response {
    let body = format!(
        "{{\"id\":\"{id}\",\"root\":\"{root}\",\"upstream\":\"{upstream}\",\
         \"upstream_known\":{known},\"default\":{is_default}}}",
        id = ws.id,
        root = escape_json(&ws.root.to_string_lossy()),
        upstream = escape_json(upstream),
        known = !upstream.is_empty(),
        is_default = ws.id == default_id,
    );
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

/// Re-build the target URL by replacing the upstream's path with the
/// incoming request's path + query. The upstream URL written by the
/// daemon is `http://host:port/mcp`; clients hitting the proxy on
/// `/anything` get forwarded preserving that suffix.
///
/// When `strip_ws_id` is `Some("abc123")`, the leading `/ws/abc123`
/// segment is removed from the path before composing — the daemon
/// listens at `/mcp`, not `/ws/<id>/mcp`, and doesn't know about the
/// proxy's routing prefix.
fn build_target_url(upstream: &str, incoming: &Uri, strip_ws_id: Option<&str>) -> String {
    // Find the scheme+authority part of `upstream`. Everything after
    // the third `/` (or end-of-string) is the upstream's own path —
    // we discard it and use the incoming request's path+query.
    let prefix = upstream
        .find("://")
        .and_then(|idx| {
            upstream[idx + 3..]
                .find('/')
                .map(|p| &upstream[..idx + 3 + p])
        })
        .unwrap_or(upstream);
    let path_and_query = incoming
        .path_and_query()
        .map_or("/", http::uri::PathAndQuery::as_str);
    let trimmed = match strip_ws_id {
        Some(id) => {
            let needle = format!("/ws/{id}");
            path_and_query
                .strip_prefix(&needle)
                .map_or(path_and_query, |s| if s.is_empty() { "/" } else { s })
        }
        None => path_and_query,
    };
    format!("{prefix}{trimmed}")
}

#[derive(Debug)]
struct ForwardError {
    kind: ForwardErrorKind,
    message: String,
}

#[derive(Debug)]
enum ForwardErrorKind {
    Connect,
    Transport,
    Build,
}

impl ForwardError {
    const fn is_retryable(&self) -> bool {
        matches!(self.kind, ForwardErrorKind::Connect)
    }

    fn into_message(self) -> String {
        self.message
    }
}

async fn forward_once(
    client: &reqwest::Client,
    method: &Method,
    target_url: &str,
    headers: &HeaderMap<HeaderValue>,
    body: Bytes,
) -> Result<Response, ForwardError> {
    let method_owned =
        Method::from_bytes(method.as_str().as_bytes()).map_err(|e| ForwardError {
            kind: ForwardErrorKind::Build,
            message: format!("invalid method `{method}`: {e}"),
        })?;
    let mut builder = client.request(method_owned, target_url);
    for (name, value) in headers {
        // axum-side `host` would route the upstream back to ourselves —
        // strip and let reqwest derive the right host from the URL.
        if name.as_str().eq_ignore_ascii_case("host")
            || name.as_str().eq_ignore_ascii_case("content-length")
        {
            continue;
        }
        builder = builder.header(name.as_str(), value);
    }
    if !body.is_empty() {
        builder = builder.body(body);
    }
    let upstream_resp = builder.send().await.map_err(|e| {
        let kind = if e.is_connect() || e.is_timeout() {
            ForwardErrorKind::Connect
        } else {
            ForwardErrorKind::Transport
        };
        ForwardError {
            kind,
            message: e.to_string(),
        }
    })?;

    let status = upstream_resp.status();
    let mut resp_headers = HeaderMap::new();
    for (name, value) in upstream_resp.headers() {
        resp_headers.insert(name.clone(), value.clone());
    }
    let body_bytes = upstream_resp.bytes().await.map_err(|e| ForwardError {
        kind: ForwardErrorKind::Transport,
        message: format!("read upstream body: {e}"),
    })?;
    let axum_status = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::OK);
    let mut response = Response::builder()
        .status(axum_status)
        .body(Body::from(body_bytes))
        .map_err(|e| ForwardError {
            kind: ForwardErrorKind::Build,
            message: format!("build axum response: {e}"),
        })?;
    *response.headers_mut() = resp_headers;
    Ok(response)
}

fn error_response(status: StatusCode, body: String) -> Response {
    (status, body).into_response()
}

#[cfg(test)]
mod tests;

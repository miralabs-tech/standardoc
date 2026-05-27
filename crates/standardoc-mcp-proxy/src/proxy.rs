use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
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
    /// Local bind address — typically `127.0.0.1:7700`. CC's MCP
    /// server URL points at `http://<bind_addr>/mcp`.
    pub bind_addr: String,
    /// Workspace root used to locate `.standardoc/mcp.endpoint`. The
    /// proxy file-watches that file so daemon port changes are picked
    /// up without polling.
    pub workspace_root: PathBuf,
    /// How long to keep retrying when the upstream daemon is
    /// unreachable (connection refused, hyper transport error). After
    /// this window the proxy returns `503 Service Unavailable`. Default
    /// 30 s gives a daemon rebuild + cold-start plenty of headroom
    /// without making the MCP client wait forever on a permanent
    /// failure.
    pub upstream_retry_window: Duration,
}

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("bind {0}: {1}")]
    Bind(String, std::io::Error),
    #[error("axum serve: {0}")]
    Serve(std::io::Error),
    #[error("watch `.standardoc/mcp.endpoint`: {0}")]
    Watcher(String),
}

/// Bind, attach the file watcher, register the catch-all forwarder, and
/// block until shutdown. Does NOT exit on upstream failures — the whole
/// point of the proxy is to outlive every daemon process it forwards to.
pub async fn run(cfg: ProxyConfig) -> Result<(), ProxyError> {
    let endpoint_path = cfg.workspace_root.join(".standardoc").join("mcp.endpoint");
    let initial = read_endpoint_file(&endpoint_path).unwrap_or_default();
    let upstream = Arc::new(RwLock::new(initial.clone()));
    spawn_endpoint_watcher(endpoint_path.clone(), Arc::clone(&upstream))?;
    if initial.is_empty() {
        eprintln!(
            "standardoc-mcp-proxy: no upstream endpoint yet at {} — will start forwarding once the daemon writes its URL",
            endpoint_path.display()
        );
    } else {
        eprintln!("standardoc-mcp-proxy: upstream = {initial}");
    }

    let client = build_forward_client();
    let started_at = SystemTime::now();
    let state = Arc::new(ProxyState {
        upstream,
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
    eprintln!("standardoc-mcp-proxy: listening on http://{local}/mcp");
    eprintln!("standardoc-mcp-proxy: health endpoint: http://{local}/health");

    let app = Router::new()
        .route("/health", get(health))
        .route("/mcp", any(forward))
        .route("/{*path}", any(forward))
        .with_state(state);

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .map_err(ProxyError::Serve)
}

struct ProxyState {
    upstream: Arc<RwLock<String>>,
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

/// axum handler — receives every incoming HTTP request, forwards to the
/// current upstream URL, retries with exponential backoff while the
/// daemon is restarting, and streams the response back. Buffers the
/// request body in memory so retries can replay it (MCP payloads are
/// small JSON — at most a few KB). Logs each incoming call so users can
/// confirm the MCP client (Claude Code, …) is actually talking to the
/// proxy — silence here means the client side is misconfigured or
/// itself disconnected.
async fn forward(
    State(state): State<Arc<ProxyState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let headers = req.headers().clone();
    let seq = state.total_requests.fetch_add(1, Ordering::Relaxed) + 1;
    *state.last_request_at.write().await = Some(SystemTime::now());
    eprintln!(
        "standardoc-mcp-proxy: incoming #{seq} {method} {} from {peer}",
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
        let upstream_url = state.upstream.read().await.clone();
        if upstream_url.is_empty() {
            last_error = Some("upstream endpoint not yet known (waiting for daemon)".into());
        } else {
            let target = build_target_url(&upstream_url, &uri);
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
                "standardoc-mcp-proxy: 503 for #{seq} — upstream unreachable after {}s: {detail}",
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

/// `GET /health` — JSON snapshot of proxy state. Used by users / scripts
/// to confirm the proxy is alive, the upstream is reachable, and the
/// MCP client has been hitting the proxy. Counts forwarder hits only
/// (this endpoint itself is excluded from the totals).
async fn health(State(state): State<Arc<ProxyState>>) -> Response {
    let upstream = state.upstream.read().await.clone();
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
    let body = format!(
        "{{\"upstream\":\"{upstream}\",\"upstream_known\":{upstream_known},\"uptime_ms\":{uptime_ms},\
         \"total_requests\":{total},\"successful_requests\":{ok},\"upstream_503_responses\":{upstream_503},\
         \"last_request_age_ms\":{age}}}",
        upstream_known = !upstream.is_empty(),
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

/// Re-build the target URL by replacing the upstream's path with the
/// incoming request's path + query. The upstream URL written by the
/// daemon is `http://host:port/mcp`; clients hitting the proxy on
/// `/anything` get forwarded preserving that suffix.
fn build_target_url(upstream: &str, incoming: &Uri) -> String {
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
    format!("{prefix}{path_and_query}")
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
mod tests {
    use super::*;

    #[test]
    fn build_target_strips_upstream_path() {
        let url = build_target_url("http://127.0.0.1:7701/mcp", &"/mcp".parse::<Uri>().unwrap());
        assert_eq!(url, "http://127.0.0.1:7701/mcp");
    }

    #[test]
    fn build_target_preserves_query() {
        let url = build_target_url(
            "http://127.0.0.1:7701/mcp",
            &"/mcp?session=abc".parse::<Uri>().unwrap(),
        );
        assert_eq!(url, "http://127.0.0.1:7701/mcp?session=abc");
    }

    #[test]
    fn build_target_uses_incoming_path_over_upstream_path() {
        let url = build_target_url(
            "http://127.0.0.1:7701/mcp",
            &"/other".parse::<Uri>().unwrap(),
        );
        assert_eq!(url, "http://127.0.0.1:7701/other");
    }

    #[test]
    fn build_target_handles_upstream_without_path() {
        let url = build_target_url("http://127.0.0.1:7701", &"/mcp".parse::<Uri>().unwrap());
        assert_eq!(url, "http://127.0.0.1:7701/mcp");
    }

    #[test]
    fn read_endpoint_file_trims_whitespace() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "  http://127.0.0.1:7701/mcp \n").unwrap();
        let val = read_endpoint_file(tmp.path()).unwrap();
        assert_eq!(val, "http://127.0.0.1:7701/mcp");
    }

    #[test]
    fn read_endpoint_file_returns_none_when_empty() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "   ").unwrap();
        assert!(read_endpoint_file(tmp.path()).is_none());
    }

    #[test]
    fn read_endpoint_file_returns_none_when_missing() {
        let missing = Path::new("/nope/definitely/not/there.endpoint");
        assert!(read_endpoint_file(missing).is_none());
    }

    #[tokio::test]
    async fn health_reports_unknown_upstream_and_zero_counters_at_boot() {
        let state = Arc::new(ProxyState {
            upstream: Arc::new(RwLock::new(String::new())),
            client: build_forward_client(),
            retry_window: Duration::from_secs(1),
            total_requests: AtomicU64::new(0),
            successful_requests: AtomicU64::new(0),
            upstream_503_responses: AtomicU64::new(0),
            last_request_at: RwLock::new(None),
            started_at: SystemTime::now(),
        });
        let resp = health(State(state)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 16 * 1024)
            .await
            .unwrap();
        let s = std::str::from_utf8(&body).unwrap();
        assert!(s.contains("\"upstream\":\"\""), "got {s}");
        assert!(s.contains("\"upstream_known\":false"), "got {s}");
        assert!(s.contains("\"total_requests\":0"), "got {s}");
        assert!(s.contains("\"last_request_age_ms\":null"), "got {s}");
    }

    #[tokio::test]
    async fn health_reports_counters_after_simulated_traffic() {
        let state = Arc::new(ProxyState {
            upstream: Arc::new(RwLock::new("http://127.0.0.1:7701/mcp".into())),
            client: build_forward_client(),
            retry_window: Duration::from_secs(1),
            total_requests: AtomicU64::new(0),
            successful_requests: AtomicU64::new(0),
            upstream_503_responses: AtomicU64::new(0),
            last_request_at: RwLock::new(None),
            started_at: SystemTime::now(),
        });
        // Simulate three forwarder hits, two succeeding.
        state.total_requests.fetch_add(3, Ordering::Relaxed);
        state.successful_requests.fetch_add(2, Ordering::Relaxed);
        state.upstream_503_responses.fetch_add(1, Ordering::Relaxed);
        *state.last_request_at.write().await = Some(SystemTime::now());

        let resp = health(State(state)).await;
        let body = axum::body::to_bytes(resp.into_body(), 16 * 1024)
            .await
            .unwrap();
        let s = std::str::from_utf8(&body).unwrap();
        assert!(s.contains("\"upstream_known\":true"));
        assert!(s.contains("\"total_requests\":3"));
        assert!(s.contains("\"successful_requests\":2"));
        assert!(s.contains("\"upstream_503_responses\":1"));
        // last_request_age_ms must be a numeric (not null) value at this point.
        assert!(!s.contains("\"last_request_age_ms\":null"), "got {s}");
    }
}

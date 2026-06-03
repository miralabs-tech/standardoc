//! `standardoc mcp --connect` — the stdio↔http bridge a Claude Code CLI user
//! (no VSCode extension) wires into `.mcp.json` via `standardoc init`.
//!
//! Claude Code only *spawns* `stdio` MCP servers; for `http` it merely
//! connects to a pre-existing endpoint a bare CLI user doesn't have. This
//! bridge bridges the gap: it is a thin `stdio` process Claude spawns that
//!
//! 1. **ensures a warm daemon** — probes `<root>/.standardoc/mcp.endpoint`
//!    and reuses a reachable daemon (the VSCode extension's, or another
//!    session's bridge); otherwise spawns `standardoc mcp <root> --http`
//!    itself. A read-write daemon runs the file watcher, so a CLI-only user
//!    still gets a live index. If the workspace write lock is held (e.g. the
//!    extension's LSP daemon), it falls back to `--readonly` so queries keep
//!    working against the lock-holder's live index.
//! 2. **relays** newline-delimited JSON-RPC from stdin to the daemon's
//!    `/mcp` endpoint and writes each response back to stdout.
//!
//! The runtime is synchronous (std) except the HTTP forward, which is driven
//! on a current-thread tokio runtime — the workspace tokio build does not
//! enable the `fs` / `process` / `io-util` features, and the relay is a
//! strictly sequential request→response loop anyway.

use std::io::{Error as IoError, Write};
use std::net::ToSocketAddrs;
use std::path::Path;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use standardoc_server::ServerError;

/// Default daemon port the bridge probes/spawns when `--port` is omitted —
/// matches the VSCode extension's `DEFAULT_MCP_HTTP_PORT`.
pub(crate) const DEFAULT_PORT: u16 = 7700;

/// Cap on the wait for the daemon to write its endpoint file.
const ENDPOINT_WAIT: Duration = Duration::from_secs(20);
const ENDPOINT_POLL: Duration = Duration::from_millis(100);
/// Per-attempt TCP connect timeout when checking a daemon is alive.
const PROBE_TIMEOUT: Duration = Duration::from_millis(400);

/// Holds the daemon we spawned (if any) for the bridge's lifetime. Its piped
/// stdin is the daemon's death-watch channel: when the bridge exits and this
/// drops, the pipe closes, the daemon reads EOF and shuts itself down — no
/// orphan process, mirroring the VSCode `McpClient`.
#[derive(Default)]
struct DaemonGuard {
    child: Option<Child>,
}

pub(crate) fn run(workspace_root: &Path, port: u16) -> Result<(), ServerError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(ServerError::Io)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_mins(2))
        .build()
        .map_err(|e| ServerError::Io(IoError::other(format!("reqwest client: {e}"))))?;

    let mut guard = DaemonGuard::default();
    let mut endpoint = ensure_daemon(workspace_root, port, &mut guard)?;
    eprintln!("standardoc mcp --connect: relaying to {endpoint}");

    // `Stdin::read_line` locks per call — fine for this strictly sequential
    // relay, and it sidesteps holding a `StdinLock` across the whole loop.
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut line = String::new();
    loop {
        line.clear();
        let n = stdin.read_line(&mut line).map_err(ServerError::Io)?;
        if n == 0 {
            break; // EOF — Claude closed the connection.
        }
        let request = line.trim();
        if request.is_empty() {
            continue;
        }
        let frame = relay_one(
            &runtime,
            &client,
            request,
            workspace_root,
            port,
            &mut endpoint,
            &mut guard,
        )?;
        if !frame.is_empty() {
            writeln!(stdout, "{frame}").map_err(ServerError::Io)?;
            stdout.flush().map_err(ServerError::Io)?;
        }
    }
    Ok(())
}

/// Forward one request, re-ensuring the daemon once if the connection
/// dropped (the owner session may have exited and taken its daemon along).
fn relay_one(
    runtime: &tokio::runtime::Runtime,
    client: &reqwest::Client,
    request: &str,
    root: &Path,
    port: u16,
    endpoint: &mut String,
    guard: &mut DaemonGuard,
) -> Result<String, ServerError> {
    match runtime.block_on(forward(client, endpoint, request)) {
        Ok(frame) => Ok(frame),
        Err(ForwardError::Connect) => {
            *endpoint = ensure_daemon(root, port, guard)?;
            match runtime.block_on(forward(client, endpoint, request)) {
                Ok(frame) => Ok(frame),
                Err(e) => Ok(error_frame(request, &e.to_string())),
            }
        }
        Err(e) => Ok(error_frame(request, &e.to_string())),
    }
}

fn ensure_daemon(root: &Path, port: u16, guard: &mut DaemonGuard) -> Result<String, ServerError> {
    let endpoint_file = root.join(".standardoc").join("mcp.endpoint");

    // 1. Reuse a reachable daemon (extension's, or another session's bridge).
    if let Some(url) = read_endpoint(&endpoint_file)
        && is_reachable(&url)
    {
        eprintln!("standardoc mcp --connect: reusing daemon at {url}");
        return Ok(url);
    }

    // 2. None reachable — spawn our own. Clear any stale endpoint first so we
    //    wait for the fresh write rather than racing a dead URL.
    let _ = std::fs::remove_file(&endpoint_file);
    if let Ok(url) = spawn_and_wait(root, port, false, &endpoint_file, guard) {
        Ok(url)
    } else {
        // Read-write open fails when another process holds the workspace
        // write lock (typically the VSCode LSP daemon). Retry read-only so
        // queries work against the lock-holder's live index.
        eprintln!(
            "standardoc mcp --connect: read-write daemon unavailable; falling back to --readonly"
        );
        let _ = std::fs::remove_file(&endpoint_file);
        spawn_and_wait(root, port, true, &endpoint_file, guard)
    }
}

fn spawn_and_wait(
    root: &Path,
    port: u16,
    readonly: bool,
    endpoint_file: &Path,
    guard: &mut DaemonGuard,
) -> Result<String, ServerError> {
    let exe = std::env::current_exe().map_err(ServerError::Io)?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("mcp").arg(root).arg("--http").arg(port.to_string());
    if readonly {
        cmd.arg("--readonly");
    }
    // stdin piped = death-watch channel (held by `guard`); stdout nulled so
    // the daemon never pollutes our JSON-RPC channel; stderr inherited so its
    // logs reach Claude's MCP server log.
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let child = cmd.spawn().map_err(ServerError::Io)?;
    guard.child = Some(child);
    wait_for_endpoint(endpoint_file, guard)
}

fn wait_for_endpoint(endpoint_file: &Path, guard: &mut DaemonGuard) -> Result<String, ServerError> {
    let deadline = Instant::now() + ENDPOINT_WAIT;
    loop {
        if let Some(child) = guard.child.as_mut()
            && let Ok(Some(status)) = child.try_wait()
        {
            return Err(ServerError::Io(IoError::other(format!(
                "standardoc mcp daemon exited ({status}) before writing its endpoint"
            ))));
        }
        if let Some(url) = read_endpoint(endpoint_file)
            && is_reachable(&url)
        {
            return Ok(url);
        }
        if Instant::now() >= deadline {
            return Err(ServerError::Io(IoError::other(
                "timed out waiting for the standardoc mcp daemon endpoint",
            )));
        }
        std::thread::sleep(ENDPOINT_POLL);
    }
}

fn read_endpoint(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Cheap liveness check: TCP-connect to the URL's `host:port` with a short
/// timeout. Avoids issuing a protocol request (and hanging on an SSE stream).
fn is_reachable(url: &str) -> bool {
    let Some(authority) = host_port(url) else {
        return false;
    };
    let Ok(addrs) = authority.to_socket_addrs() else {
        return false;
    };
    addrs
        .into_iter()
        .any(|addr| std::net::TcpStream::connect_timeout(&addr, PROBE_TIMEOUT).is_ok())
}

/// Extracts `host:port` from an `http(s)://host:port/path` URL. Returns `None`
/// when the authority carries no explicit port (our daemon always emits one).
fn host_port(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let authority = rest.split('/').next()?;
    if authority.contains(':') {
        Some(authority.to_string())
    } else {
        None
    }
}

enum ForwardError {
    /// The daemon could not be reached — caller should re-ensure and retry.
    Connect,
    Http(String),
}

impl std::fmt::Display for ForwardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connect => write!(f, "daemon connection failed"),
            Self::Http(m) => write!(f, "{m}"),
        }
    }
}

async fn forward(
    client: &reqwest::Client,
    endpoint: &str,
    request: &str,
) -> Result<String, ForwardError> {
    let resp = client
        .post(endpoint)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(request.to_string())
        .send()
        .await
        .map_err(|e| {
            if e.is_connect() || e.is_timeout() {
                ForwardError::Connect
            } else {
                ForwardError::Http(e.to_string())
            }
        })?;
    let status = resp.status();
    // 202 Accepted = the daemon acknowledged a notification/response; nothing
    // to relay back.
    if status == reqwest::StatusCode::ACCEPTED {
        return Ok(String::new());
    }
    let text = resp
        .text()
        .await
        .map_err(|e| ForwardError::Http(e.to_string()))?;
    if !status.is_success() {
        return Err(ForwardError::Http(format!(
            "HTTP {status}: {}",
            text.trim()
        )));
    }
    Ok(extract_json_frame(&text))
}

/// Normalises a daemon response into a single-line JSON-RPC frame. The daemon
/// serves `json_response = true` so the body is plain JSON, but we re-encode
/// compactly (and unwrap an `text/event-stream` `data:` payload defensively)
/// to guarantee one newline-delimited message per line on stdout.
fn extract_json_frame(text: &str) -> String {
    let payload = sse_data(text).unwrap_or_else(|| text.trim().to_string());
    match serde_json::from_str::<Value>(&payload) {
        Ok(v) => serde_json::to_string(&v).unwrap_or(payload),
        Err(_) => payload.replace('\n', " "),
    }
}

fn sse_data(text: &str) -> Option<String> {
    if !text.contains("data:") {
        return None;
    }
    let data: String = text
        .lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .map(str::trim)
        .collect();
    if data.is_empty() { None } else { Some(data) }
}

/// Builds a JSON-RPC error response carrying the request's `id` so Claude
/// surfaces a clean error instead of hanging. Requests without an `id`
/// (notifications) get no response — returns an empty string.
fn error_frame(request: &str, message: &str) -> String {
    let id = serde_json::from_str::<Value>(request)
        .ok()
        .and_then(|v| v.get("id").cloned());
    match id {
        None => String::new(),
        Some(id) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32000, "message": format!("standardoc bridge: {message}") }
        })
        .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_port_extracts_authority_with_port() {
        assert_eq!(
            host_port("http://127.0.0.1:56831/mcp").as_deref(),
            Some("127.0.0.1:56831")
        );
    }

    #[test]
    fn host_port_none_without_explicit_port() {
        assert_eq!(host_port("http://127.0.0.1/mcp"), None);
        assert_eq!(host_port("ftp://nope"), None);
    }

    #[test]
    fn extract_json_frame_compacts_to_one_line() {
        let pretty = "{\n  \"jsonrpc\": \"2.0\",\n  \"id\": 1,\n  \"result\": {}\n}";
        let frame = extract_json_frame(pretty);
        assert!(!frame.contains('\n'));
        assert_eq!(frame, r#"{"id":1,"jsonrpc":"2.0","result":{}}"#);
    }

    #[test]
    fn extract_json_frame_unwraps_sse_data() {
        let sse = "event: message\ndata: {\"id\":2,\"result\":7}\n\n";
        assert_eq!(extract_json_frame(sse), r#"{"id":2,"result":7}"#);
    }

    #[test]
    fn error_frame_carries_request_id() {
        let frame = error_frame(r#"{"jsonrpc":"2.0","id":42,"method":"x"}"#, "boom");
        let v: Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(v["id"], 42);
        assert_eq!(v["error"]["code"], -32000);
        assert!(v["error"]["message"].as_str().unwrap().contains("boom"));
    }

    #[test]
    fn error_frame_empty_for_notification() {
        assert_eq!(
            error_frame(r#"{"jsonrpc":"2.0","method":"note"}"#, "boom"),
            ""
        );
    }
}

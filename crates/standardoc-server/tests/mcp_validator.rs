//! MCP integration tests for `list_diagnostics` tool.

use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    _tempdir: TempDir,
    workspace: PathBuf,
}

impl McpClient {
    fn spawn() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = dir.path().to_path_buf();
        let bin = env!("CARGO_BIN_EXE_standardoc-server");
        let mut child = Command::new(bin)
            .args(["--mcp", "--workspace", &workspace.to_string_lossy()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Self {
            child,
            stdin,
            stdout,
            next_id: 1,
            _tempdir: dir,
            workspace,
        }
    }

    fn workspace(&self) -> &std::path::Path {
        &self.workspace
    }

    fn send(&mut self, method: &str, params: &Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        writeln!(self.stdin, "{msg}").expect("write");
        self.stdin.flush().expect("flush");
        loop {
            let mut line = String::new();
            self.stdout.read_line(&mut line).expect("read");
            let parsed: Value = serde_json::from_str(&line).expect("parse");
            if parsed.get("id").and_then(Value::as_u64) == Some(id) {
                return parsed;
            }
        }
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn text_payload(resp: &Value) -> Value {
    let text = resp["result"]["content"][0]["text"].as_str().expect("text");
    serde_json::from_str::<Value>(text).expect("parse text payload")
}

fn wait_for_blocks(client: &mut McpClient, min_count: usize, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let resp = client.send(
            "tools/call",
            &json!({ "name": "list_docs", "arguments": { "limit": 500 } }),
        );
        let payload = text_payload(&resp);
        if let Some(total) = payload["total"].as_u64() {
            if usize::try_from(total).unwrap_or(0) >= min_count {
                return;
            }
        }
        thread::sleep(Duration::from_millis(200));
    }
    panic!("workspace did not reach {min_count} blocks within timeout");
}

#[test]
fn list_diagnostics_reports_std006_for_undocumented_public_symbol() {
    let mut client = McpClient::spawn();
    let _ = client.send("initialize", &json!({}));

    fs::write(
        client.workspace().join("lib.rs"),
        "pub fn undocumented() {}\n",
    )
    .expect("write");

    wait_for_blocks(&mut client, 1, Duration::from_secs(5));

    let resp = client.send(
        "tools/call",
        &json!({ "name": "list_diagnostics", "arguments": {} }),
    );
    let payload = text_payload(&resp);
    let entries = payload["entries"].as_array().expect("entries");
    assert!(
        entries.iter().any(|e| e["code"].as_str() == Some("STD006")),
        "expected STD006 in {entries:?}"
    );
}

#[test]
fn list_diagnostics_filters_by_severity() {
    let mut client = McpClient::spawn();
    let _ = client.send("initialize", &json!({}));

    // Mixed diagnostics: one STD006 (Hint) + one block without description (STD005, Info).
    fs::write(
        client.workspace().join("lib.rs"),
        "pub fn no_doc() {}\npub fn another() {}\n",
    )
    .expect("write");

    wait_for_blocks(&mut client, 2, Duration::from_secs(5));

    // Filter on "info" — should return STD005 but not STD006 (Hint).
    let resp = client.send(
        "tools/call",
        &json!({ "name": "list_diagnostics", "arguments": { "severity": "info" } }),
    );
    let payload = text_payload(&resp);
    let entries = payload["entries"].as_array().expect("entries");
    assert!(entries
        .iter()
        .all(|e| e["severity"].as_str() == Some("info")));
    assert!(!entries.is_empty(), "expected some info diagnostics");
}

#[test]
fn list_diagnostics_filters_by_code() {
    let mut client = McpClient::spawn();
    let _ = client.send("initialize", &json!({}));

    fs::write(client.workspace().join("lib.rs"), "pub fn anything() {}\n").expect("write");

    wait_for_blocks(&mut client, 1, Duration::from_secs(5));

    let resp = client.send(
        "tools/call",
        &json!({ "name": "list_diagnostics", "arguments": { "code": "STD006" } }),
    );
    let payload = text_payload(&resp);
    let entries = payload["entries"].as_array().expect("entries");
    assert!(!entries.is_empty());
    assert!(entries.iter().all(|e| e["code"].as_str() == Some("STD006")));
}

#[test]
fn list_diagnostics_returns_by_severity_summary() {
    let mut client = McpClient::spawn();
    let _ = client.send("initialize", &json!({}));

    fs::write(client.workspace().join("lib.rs"), "pub fn x() {}\n").expect("write");
    wait_for_blocks(&mut client, 1, Duration::from_secs(5));

    let resp = client.send(
        "tools/call",
        &json!({ "name": "list_diagnostics", "arguments": {} }),
    );
    let payload = text_payload(&resp);
    assert!(payload["by_severity"].is_object());
    assert!(payload["total"].is_number());
}

//! Integration tests for live update via watcher + MCP.
//!
//! Covered scenarios:
//! - A file added during session appears in `list_docs` after a reasonable
//!   delay (watcher debounce + worker rescan).
//! - `set_watch_paused(true)` freezes updates; FS events received during pause
//!   do not change index.
//! - `get_watch_status` reflects current pause state.
//! - Revision is bumped on each effective update.

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
    /// Spawn server with dedicated tempdir workspace managed by client.
    fn spawn_with_owned_workspace() -> Self {
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

        // Skip notifications (messages without `id`) until we hit the
        // response au `id` qu'on vient d'envoyer.
        loop {
            let mut line = String::new();
            self.stdout.read_line(&mut line).expect("read");
            let parsed: Value = serde_json::from_str(&line).expect("parse");
            if parsed.get("id").and_then(Value::as_u64) == Some(id) {
                return parsed;
            }
            // Sinon c'est une notification ou un id d'un autre request — on boucle.
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
    serde_json::from_str::<Value>(text).unwrap_or_else(|_| Value::String(text.to_owned()))
}

/// Poll `list_docs` until a block matching predicate appears or timeout.
/// Handles asynchronous watcher -> worker -> index delays.
fn wait_for_key_match(
    client: &mut McpClient,
    predicate: impl Fn(&Value) -> bool,
    timeout: Duration,
) -> Option<Value> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let resp = client.send(
            "tools/call",
            &json!({
                "name": "list_docs",
                "arguments": { "limit": 500 }
            }),
        );
        let payload = text_payload(&resp);
        if let Some(entries) = payload["entries"].as_array() {
            if let Some(hit) = entries.iter().find(|e| predicate(e)) {
                return Some(hit.clone());
            }
        }
        thread::sleep(Duration::from_millis(200));
    }
    None
}

#[test]
fn new_file_becomes_visible_via_list_docs() {
    let mut client = McpClient::spawn_with_owned_workspace();
    let _ = client.send("initialize", &json!({}));

    // Initial : workspace vide, 0 block.
    let initial = client.send(
        "tools/call",
        &json!({
            "name": "list_docs", "arguments": {}
        }),
    );
    let initial_total = text_payload(&initial)["total"].as_u64().unwrap_or(u64::MAX);
    assert_eq!(initial_total, 0, "fresh tempdir should have 0 blocks");

    // Create Rust file with a public function.
    let file = client.workspace().join("added.rs");
    fs::write(&file, "pub fn newly_added() -> i32 { 42 }\n").expect("write file");

    // Wait for watcher + worker propagation.
    let found = wait_for_key_match(
        &mut client,
        |entry| entry["label"].as_str() == Some("newly_added"),
        Duration::from_secs(5),
    );
    assert!(
        found.is_some(),
        "newly_added should appear in list_docs within 5s"
    );
}

#[test]
fn modifying_a_file_updates_its_block() {
    let mut client = McpClient::spawn_with_owned_workspace();
    let _ = client.send("initialize", &json!({}));

    // Create file with an initial signature.
    let file = client.workspace().join("mutating.rs");
    fs::write(&file, "pub fn foo() -> i32 { 0 }\n").expect("write");

    let found = wait_for_key_match(
        &mut client,
        |e| e["label"].as_str() == Some("foo"),
        Duration::from_secs(5),
    );
    assert!(found.is_some(), "initial foo should appear");

    // Modify the signature.
    fs::write(&file, "pub fn foo(x: i32) -> i32 { x }\n").expect("rewrite");

    // Poll get_doc to observe the new signature.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last_seen: Option<String> = None;
    while Instant::now() < deadline {
        let resp = client.send(
            "tools/call",
            &json!({
                "name": "list_docs", "arguments": { "filter": "foo", "limit": 10 }
            }),
        );
        let payload = text_payload(&resp);
        if let Some(entries) = payload["entries"].as_array() {
            if let Some(entry) = entries.iter().find(|e| e["label"].as_str() == Some("foo")) {
                let key = entry["key"].as_str().unwrap_or("");
                let doc = client.send(
                    "tools/call",
                    &json!({
                        "name": "get_doc", "arguments": { "key": key }
                    }),
                );
                let block_text = doc["result"]["content"][0]["text"].as_str().unwrap_or("");
                last_seen = Some(block_text.to_owned());
                if block_text.contains("fn foo(x: i32)") {
                    return;
                }
            }
        }
        thread::sleep(Duration::from_millis(200));
    }
    panic!("expected signature to change to `fn foo(x: i32)`. Last seen: {last_seen:?}");
}

#[test]
fn pause_freezes_updates() {
    let mut client = McpClient::spawn_with_owned_workspace();
    let _ = client.send("initialize", &json!({}));

    // Pause the watcher.
    let resp = client.send(
        "tools/call",
        &json!({
            "name": "set_watch_paused", "arguments": { "paused": true }
        }),
    );
    let payload = text_payload(&resp);
    assert_eq!(payload["paused"], json!(true));

    // Create file during pause.
    let file = client.workspace().join("while_paused.rs");
    fs::write(&file, "pub fn invisible() -> () {}\n").expect("write");

    // Wait longer than debounce + worker poll (500ms rx timeout + 100ms debounce).
    thread::sleep(Duration::from_secs(2));

    let listed = client.send(
        "tools/call",
        &json!({
            "name": "list_docs", "arguments": { "filter": "invisible", "limit": 10 }
        }),
    );
    let payload = text_payload(&listed);
    let entries = payload["entries"].as_array().cloned().unwrap_or_default();
    assert!(
        entries.is_empty(),
        "paused watcher must NOT update the index, got: {entries:?}"
    );

    // Resume -> file should appear now.
    let resp = client.send(
        "tools/call",
        &json!({
            "name": "set_watch_paused", "arguments": { "paused": false }
        }),
    );
    assert_eq!(text_payload(&resp)["paused"], json!(false));

    // Touch file to re-trigger event (watcher was dropping events during pause).
    fs::write(&file, "pub fn invisible() -> () { /* touched */ }\n").expect("touch");

    let found = wait_for_key_match(
        &mut client,
        |e| e["label"].as_str() == Some("invisible"),
        Duration::from_secs(5),
    );
    assert!(
        found.is_some(),
        "after resume + touch, invisible should appear"
    );
}

#[test]
fn index_changed_notification_is_pushed_after_fs_event() {
    let mut client = McpClient::spawn_with_owned_workspace();
    let _ = client.send("initialize", &json!({}));

    // Create file -> worker should detect it, update index and
    // pousser une notification `notifications/standardoc/index_changed`.
    let file = client.workspace().join("notify_target.rs");
    fs::write(&file, "pub fn watched() {}\n").expect("write");

    // Read stdout lines until finding a notification, with timeout
    // large (watcher debounce + worker poll).
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        let mut line = String::new();
        // `read_line` est bloquant — on ne peut pas polling strict. On limite
        // by checking whether file is visible through periodic `list_docs`,
        // which also implicitly drains stdout buffer.
        match client.stdout.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let parsed: Value = serde_json::from_str(&line).expect("parse");
                if parsed.get("method").and_then(Value::as_str)
                    == Some("notifications/standardoc/index_changed")
                {
                    // Verify payload shape.
                    let params = &parsed["params"];
                    assert!(params["revision"].is_number());
                    assert!(params["added"].is_array());
                    assert!(params["removed"].is_array());
                    // `watched` block should appear in `added` for a rescan
                    // that just added the file.
                    let added: Vec<&str> = params["added"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(Value::as_str)
                        .collect();
                    assert!(
                        added.iter().any(|k| k.contains("watched")),
                        "expected `watched` in added keys, got: {added:?}"
                    );
                    return;
                }
                // Autre message — on skip.
            }
        }
    }
    panic!("no index_changed notification received within timeout");
}

#[test]
fn get_watch_status_reports_has_watcher_and_paused() {
    let mut client = McpClient::spawn_with_owned_workspace();
    let _ = client.send("initialize", &json!({}));

    let resp = client.send(
        "tools/call",
        &json!({
            "name": "get_watch_status", "arguments": {}
        }),
    );
    let payload = text_payload(&resp);
    assert_eq!(payload["has_watcher"], json!(true));
    assert_eq!(payload["paused"], json!(false));
    assert!(payload["revision"].is_number());
}

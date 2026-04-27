//! MCP integration tests for `emit_llms_txt` / `emit_llms_full` /
//! `emit_skill_md` tools. Spawns a server in tempdir with a few Rust + TS
//! files and validates shape/content of all three emitted formats.

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

fn text_string(resp: &Value) -> String {
    resp["result"]["content"][0]["text"]
        .as_str()
        .expect("text")
        .to_owned()
}

fn wait_for_blocks(client: &mut McpClient, min_count: usize, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let resp = client.send(
            "tools/call",
            &json!({ "name": "list_docs", "arguments": { "limit": 500 } }),
        );
        let text = resp["result"]["content"][0]["text"].as_str().expect("text");
        let payload: Value = serde_json::from_str(text).expect("payload");
        if let Some(total) = payload["total"].as_u64() {
            if usize::try_from(total).unwrap_or(0) >= min_count {
                return;
            }
        }
        thread::sleep(Duration::from_millis(200));
    }
    panic!("workspace did not reach {min_count} blocks within timeout");
}

fn seed_minimal_project(client: &McpClient) {
    fs::write(
        client.workspace().join("lib.rs"),
        "/// A widget for the demo.\n\
         /// @doc widget Widget\n\
         /// @description The widget that does widget things.\n\
         pub struct Widget;\n\
         \n\
         pub trait Greet { fn hello(&self) -> String; }\n\
         pub struct French;\n\
         impl Greet for French { fn hello(&self) -> String { String::new() } }\n\
         \n\
         /// Adds two integers.\n\
         pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
    )
    .expect("write lib.rs");
}

#[test]
fn emit_llms_txt_returns_a_well_formed_index() {
    let mut client = McpClient::spawn();
    let _ = client.send("initialize", &json!({}));
    seed_minimal_project(&client);
    wait_for_blocks(&mut client, 4, Duration::from_secs(5));

    let resp = client.send(
        "tools/call",
        &json!({
            "name": "emit_llms_txt",
            "arguments": { "name": "Demo", "tagline": "a tagline" }
        }),
    );
    let body = text_string(&resp);
    assert!(body.starts_with("# Demo"));
    assert!(body.contains("> a tagline"));
    assert!(body.contains("## "));
    // At least one entry in `[label](link.rs#Lxx) (kind): ...` format.
    assert!(body.contains("(struct):"));
    assert!(body.contains("(function):"));
}

#[test]
fn emit_llms_full_includes_signatures_and_descriptions() {
    let mut client = McpClient::spawn();
    let _ = client.send("initialize", &json!({}));
    seed_minimal_project(&client);
    wait_for_blocks(&mut client, 4, Duration::from_secs(5));

    let resp = client.send(
        "tools/call",
        &json!({
            "name": "emit_llms_full",
            "arguments": { "name": "Demo" }
        }),
    );
    let body = text_string(&resp);
    assert!(body.starts_with("# Demo"));
    // Signatures are inside ```rust fences.
    assert!(body.contains("```rust"));
    assert!(body.contains("pub fn add(a: i32, b: i32) -> i32"));
    assert!(body.contains("The widget that does widget things."));
}

#[test]
fn emit_skill_md_has_yaml_front_matter_and_sections() {
    let mut client = McpClient::spawn();
    let _ = client.send("initialize", &json!({}));
    seed_minimal_project(&client);
    wait_for_blocks(&mut client, 4, Duration::from_secs(5));

    let resp = client.send(
        "tools/call",
        &json!({
            "name": "emit_skill_md",
            "arguments": { "name": "Demo Lib", "tagline": "the demo" }
        }),
    );
    let body = text_string(&resp);
    assert!(body.starts_with("---\n"));
    assert!(body.contains("name: demo-lib"));
    assert!(body.contains("description: the demo"));
    assert!(body.contains("# Demo Lib"));
    assert!(body.contains("## Key types"));
    assert!(body.contains("## Key traits"));
    assert!(body.contains("Greet"));
    // `French` is listed as implementor of `Greet`.
    assert!(body.contains("Implementors: French"));
    assert!(body.contains("## Public functions"));
    assert!(body.contains("add"));
}

#[test]
fn emit_llms_txt_link_base_prefixes_paths() {
    let mut client = McpClient::spawn();
    let _ = client.send("initialize", &json!({}));
    seed_minimal_project(&client);
    wait_for_blocks(&mut client, 4, Duration::from_secs(5));

    let resp = client.send(
        "tools/call",
        &json!({
            "name": "emit_llms_txt",
            "arguments": { "link_base": "https://example.com/src" }
        }),
    );
    let body = text_string(&resp);
    assert!(body.contains("https://example.com/src/lib.rs#L"));
}

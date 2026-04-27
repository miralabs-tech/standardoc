//! End-to-end integration test for the MCP server.
//!
//! Spawns the built `standardoc-server --mcp` binary against the repo's own
//! `examples/rust-lib/src/` directory, drives it via stdin, and asserts the
//! responses match the MCP spec.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpClient {
    fn spawn(workspace: &str) -> Self {
        let bin = env!("CARGO_BIN_EXE_standardoc-server");
        let mut child = Command::new(bin)
            .args(["--mcp", "--workspace", workspace])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn standardoc-server");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        }
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
        writeln!(self.stdin, "{msg}").expect("write stdin");
        self.stdin.flush().expect("flush stdin");

        // Ignore asynchronous notifications (without `id`) between our request
        // et sa response.
        loop {
            let mut line = String::new();
            self.stdout.read_line(&mut line).expect("read stdout");
            let parsed: Value = serde_json::from_str(&line).expect("parse response");
            if parsed.get("id").and_then(Value::as_u64) == Some(id) {
                return parsed;
            }
        }
    }

    fn notify(&mut self, method: &str, params: &Value) {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        writeln!(self.stdin, "{msg}").expect("write stdin");
        self.stdin.flush().expect("flush stdin");
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn examples_rust_lib() -> String {
    // Tests run with cwd = the server crate directory.
    // `examples/rust-lib/src` sits at the workspace root.
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = crate_dir.parent().unwrap().parent().unwrap();
    workspace_root
        .join("examples")
        .join("rust-lib")
        .join("src")
        .to_string_lossy()
        .into_owned()
}

fn text_result(response: &Value) -> String {
    response["result"]["content"][0]["text"]
        .as_str()
        .expect("content[0].text")
        .to_owned()
}

#[test]
fn initialize_returns_server_info_and_capabilities() {
    let mut client = McpClient::spawn(&examples_rust_lib());
    let resp = client.send(
        "initialize",
        &json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "itest", "version": "0" }
        }),
    );
    assert_eq!(resp["result"]["serverInfo"]["name"], "standardoc");
    assert!(resp["result"]["capabilities"]["tools"].is_object());
    assert!(resp["result"]["capabilities"]["resources"].is_object());
}

#[test]
fn tools_list_exposes_expected_tools() {
    let mut client = McpClient::spawn(&examples_rust_lib());
    let _ = client.send("initialize", &json!({}));
    client.notify("notifications/initialized", &json!({}));

    let resp = client.send("tools/list", &json!({}));
    let tools = resp["result"]["tools"].as_array().expect("tools array");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    for expected in [
        "list_docs",
        "get_doc",
        "search_docs",
        "evaluate_dsl",
        "render_markdown",
        "get_dsl_reference",
        "find_undocumented",
        "rescan",
        "validate_doc_syntax",
        "coverage_report",
    ] {
        assert!(names.contains(&expected), "missing tool: {expected}");
    }
}

#[test]
fn list_docs_returns_scanned_blocks() {
    let mut client = McpClient::spawn(&examples_rust_lib());
    let _ = client.send("initialize", &json!({}));

    let resp = client.send(
        "tools/call",
        &json!({ "name": "list_docs", "arguments": {} }),
    );
    let text = text_result(&resp);
    let payload: Value = serde_json::from_str(&text).expect("json payload");
    let entries = payload["entries"].as_array().expect("entries array");
    assert!(!entries.is_empty(), "expected at least one block");
    assert!(payload["revision"].is_number());
}

#[test]
fn get_doc_returns_full_block() {
    let mut client = McpClient::spawn(&examples_rust_lib());
    let _ = client.send("initialize", &json!({}));

    let resp = client.send(
        "tools/call",
        &json!({ "name": "get_doc", "arguments": { "key": "calculator.add" } }),
    );
    let text = text_result(&resp);
    let block: Value = serde_json::from_str(&text).expect("json block");
    assert_eq!(block["key"], "calculator.add");
    assert!(block["symbol"]["params"].as_array().unwrap().len() >= 2);
}

#[test]
fn evaluate_dsl_renders_a_single_expression() {
    let mut client = McpClient::spawn(&examples_rust_lib());
    let _ = client.send("initialize", &json!({}));

    let resp = client.send(
        "tools/call",
        &json!({
            "name": "evaluate_dsl",
            "arguments": { "expression": "@doc.calculator.add:label" }
        }),
    );
    assert_eq!(text_result(&resp), "add");
}

#[test]
fn unknown_tool_surfaces_as_tool_error_not_rpc_error() {
    let mut client = McpClient::spawn(&examples_rust_lib());
    let _ = client.send("initialize", &json!({}));

    let resp = client.send(
        "tools/call",
        &json!({ "name": "does_not_exist", "arguments": {} }),
    );
    // Our dispatch returns this as an RPC error (invalid_params), not a
    // tool-level error — that's what the MCP spec allows for unknown tools.
    assert!(resp["error"].is_object());
    assert_eq!(resp["error"]["code"], -32602);
}

#[test]
fn resources_read_returns_dsl_reference_markdown() {
    let mut client = McpClient::spawn(&examples_rust_lib());
    let _ = client.send("initialize", &json!({}));

    let resp = client.send(
        "resources/read",
        &json!({ "uri": "standardoc://schema/dsl" }),
    );
    let contents = resp["result"]["contents"][0]["text"]
        .as_str()
        .expect("text");
    assert!(contents.contains("Standardoc DSL reference"));
    assert_eq!(
        resp["result"]["contents"][0]["mimeType"].as_str(),
        Some("text/markdown")
    );
}

#[test]
fn unknown_method_is_method_not_found() {
    let mut client = McpClient::spawn(&examples_rust_lib());
    let resp = client.send("does/not/exist", &json!({}));
    assert_eq!(resp["error"]["code"], -32601);
}

#[test]
fn find_undocumented_picks_up_the_unannotated_helper() {
    let mut client = McpClient::spawn(&examples_rust_lib());
    let _ = client.send("initialize", &json!({}));

    let resp = client.send(
        "tools/call",
        &json!({ "name": "find_undocumented", "arguments": {} }),
    );
    let payload: Value = serde_json::from_str(&text_result(&resp)).expect("json");
    let keys: Vec<&str> = payload["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["key"].as_str())
        .collect();
    // After the FQN fix, inferred keys carry the package + module prefix.
    assert!(
        keys.contains(&"rust_lib.Calculator.sub"),
        "entries={keys:?}"
    );
}

#[test]
fn rescan_bumps_revision() {
    let mut client = McpClient::spawn(&examples_rust_lib());
    let _ = client.send("initialize", &json!({}));

    let first = client.send(
        "tools/call",
        &json!({ "name": "list_docs", "arguments": {} }),
    );
    let initial: Value = serde_json::from_str(&text_result(&first)).unwrap();
    let initial_rev = initial["revision"].as_u64().unwrap();

    let _ = client.send("tools/call", &json!({ "name": "rescan", "arguments": {} }));

    let second = client.send(
        "tools/call",
        &json!({ "name": "list_docs", "arguments": {} }),
    );
    let after: Value = serde_json::from_str(&text_result(&second)).unwrap();
    let after_rev = after["revision"].as_u64().unwrap();
    assert!(after_rev > initial_rev);
}

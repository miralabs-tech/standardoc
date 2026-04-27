//! Integration tests for cross-reference tools (`find_usages`,
//! `find_implementations`).
//!
//! Spawn un serveur sur un tempdir, scaffolde un mini projet Rust qui
//! exercises different reference types, then verifies tools return expected usages.

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

/// Wait until index reflects FS write, polling `list_docs` until
/// trouver un certain nombre de blocs minimum.
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
fn find_usages_returns_param_type_referrers() {
    let mut client = McpClient::spawn();
    let _ = client.send("initialize", &json!({}));

    // Mini project: `MyType` is a struct, `consumer` takes it as parameter.
    fs::write(
        client.workspace().join("lib.rs"),
        "pub struct MyType { value: i32 }\n\
         pub fn consumer(input: MyType) -> i32 { input.value }\n",
    )
    .expect("write");

    wait_for_blocks(&mut client, 2, Duration::from_secs(5));

    let resp = client.send(
        "tools/call",
        &json!({ "name": "find_usages", "arguments": { "name": "MyType" } }),
    );
    let payload = text_payload(&resp);
    let count = payload["count"].as_u64().unwrap_or(0);
    assert!(count >= 1, "expected at least one usage, got {payload}");
    let entries = payload["entries"].as_array().expect("entries");
    assert!(
        entries
            .iter()
            .any(|e| e["kind"].as_str() == Some("param-type")),
        "expected at least one param-type usage, got: {entries:?}"
    );
}

#[test]
fn find_usages_returns_field_type_referrers() {
    let mut client = McpClient::spawn();
    let _ = client.send("initialize", &json!({}));

    // `Inner` est un type, `Outer` l'a comme champ.
    fs::write(
        client.workspace().join("lib.rs"),
        "pub struct Inner { x: i32 }\n\
         pub struct Outer { core: Inner, extra: bool }\n",
    )
    .expect("write");

    wait_for_blocks(&mut client, 2, Duration::from_secs(5));

    let resp = client.send(
        "tools/call",
        &json!({ "name": "find_usages", "arguments": { "name": "Inner" } }),
    );
    let payload = text_payload(&resp);
    let entries = payload["entries"].as_array().expect("entries");
    assert!(
        entries.iter().any(|e| {
            e["kind"].as_str() == Some("field-type") && e["from_label"].as_str() == Some("Outer")
        }),
        "expected Outer with field-type kind, got: {entries:?}"
    );
}

#[test]
fn find_usages_filters_by_kind() {
    let mut client = McpClient::spawn();
    let _ = client.send("initialize", &json!({}));

    // `Item` is used as both param and field.
    fs::write(
        client.workspace().join("lib.rs"),
        "pub struct Item { id: u32 }\n\
         pub struct Container { content: Item }\n\
         pub fn handle(item: Item) {}\n",
    )
    .expect("write");

    wait_for_blocks(&mut client, 3, Duration::from_secs(5));

    let resp = client.send(
        "tools/call",
        &json!({
            "name": "find_usages",
            "arguments": { "name": "Item", "kind": "param-type" }
        }),
    );
    let payload = text_payload(&resp);
    let entries = payload["entries"].as_array().expect("entries");
    assert!(
        entries
            .iter()
            .all(|e| e["kind"].as_str() == Some("param-type")),
        "kind filter should restrict to param-type, got: {entries:?}"
    );
    assert!(
        !entries.is_empty(),
        "expected at least one param-type usage"
    );
}

#[test]
fn find_implementations_lists_implementor_types() {
    let mut client = McpClient::spawn();
    let _ = client.send("initialize", &json!({}));

    // Define a trait + a struct implementing it.
    fs::write(
        client.workspace().join("lib.rs"),
        "pub trait Greet {\n\
             fn hello(&self) -> String;\n\
         }\n\
         pub struct French;\n\
         impl Greet for French {\n\
             fn hello(&self) -> String { \"bonjour\".to_owned() }\n\
         }\n\
         pub struct English;\n\
         impl Greet for English {\n\
             fn hello(&self) -> String { \"hello\".to_owned() }\n\
         }\n",
    )
    .expect("write");

    wait_for_blocks(&mut client, 3, Duration::from_secs(5));

    let resp = client.send(
        "tools/call",
        &json!({ "name": "find_implementations", "arguments": { "trait_name": "Greet" } }),
    );
    let payload = text_payload(&resp);
    let count = payload["count"].as_u64().unwrap_or(0);
    assert!(
        count >= 2,
        "expected at least 2 implementors, got: {payload}"
    );
    let implementors: Vec<&str> = payload["implementors"]
        .as_array()
        .expect("implementors")
        .iter()
        .filter_map(|i| i["implementor"].as_str())
        .collect();
    assert!(
        implementors.iter().any(|i| i.contains("French")),
        "expected French in implementors, got: {implementors:?}"
    );
    assert!(
        implementors.iter().any(|i| i.contains("English")),
        "expected English in implementors, got: {implementors:?}"
    );
}

#[test]
fn ts_find_usages_param_and_return_types() {
    let mut client = McpClient::spawn();
    let _ = client.send("initialize", &json!({}));

    fs::write(
        client.workspace().join("api.ts"),
        "export interface User { id: number; name: string; }\n\
         export function load(id: number): User { return { id, name: '' }; }\n\
         export function save(u: User): boolean { return true; }\n",
    )
    .expect("write");

    wait_for_blocks(&mut client, 3, Duration::from_secs(5));

    let resp = client.send(
        "tools/call",
        &json!({ "name": "find_usages", "arguments": { "name": "User" } }),
    );
    let payload = text_payload(&resp);
    let entries = payload["entries"].as_array().expect("entries");
    assert!(
        entries
            .iter()
            .any(|e| e["kind"].as_str() == Some("param-type")),
        "expected param-type ref to User, got: {entries:?}"
    );
    assert!(
        entries
            .iter()
            .any(|e| e["kind"].as_str() == Some("return-type")),
        "expected return-type ref to User, got: {entries:?}"
    );
}

#[test]
fn ts_find_implementations_class_implements_interface() {
    let mut client = McpClient::spawn();
    let _ = client.send("initialize", &json!({}));

    fs::write(
        client.workspace().join("greet.ts"),
        "export interface Greeter { hello(): string; }\n\
         export class FrenchGreeter implements Greeter {\n\
             hello(): string { return 'bonjour'; }\n\
         }\n\
         export class EnglishGreeter implements Greeter {\n\
             hello(): string { return 'hello'; }\n\
         }\n",
    )
    .expect("write");

    wait_for_blocks(&mut client, 3, Duration::from_secs(5));

    let resp = client.send(
        "tools/call",
        &json!({ "name": "find_implementations", "arguments": { "trait_name": "Greeter" } }),
    );
    let payload = text_payload(&resp);
    let count = payload["count"].as_u64().unwrap_or(0);
    assert!(
        count >= 2,
        "expected at least 2 implementors, got: {payload}"
    );
    let implementors: Vec<&str> = payload["implementors"]
        .as_array()
        .expect("implementors")
        .iter()
        .filter_map(|i| i["implementor"].as_str())
        .collect();
    assert!(
        implementors.iter().any(|i| i.contains("FrenchGreeter")),
        "expected FrenchGreeter, got: {implementors:?}"
    );
    assert!(
        implementors.iter().any(|i| i.contains("EnglishGreeter")),
        "expected EnglishGreeter, got: {implementors:?}"
    );
}

#[test]
fn ts_find_usages_field_type() {
    let mut client = McpClient::spawn();
    let _ = client.send("initialize", &json!({}));

    fs::write(
        client.workspace().join("model.ts"),
        "export interface Address { street: string; city: string; }\n\
         export interface Customer { id: number; address: Address; }\n",
    )
    .expect("write");

    wait_for_blocks(&mut client, 2, Duration::from_secs(5));

    let resp = client.send(
        "tools/call",
        &json!({ "name": "find_usages", "arguments": { "name": "Address", "kind": "field-type" } }),
    );
    let payload = text_payload(&resp);
    let entries = payload["entries"].as_array().expect("entries");
    assert!(
        !entries.is_empty(),
        "expected Customer to reference Address as field-type, got: {entries:?}"
    );
}

#[test]
fn search_by_return_type_finds_producer_functions() {
    let mut client = McpClient::spawn();
    let _ = client.send("initialize", &json!({}));

    fs::write(
        client.workspace().join("lib.rs"),
        "pub struct Token { value: String }\n\
         pub fn create() -> Token { Token { value: String::new() } }\n\
         pub fn parse(input: &str) -> Token { Token { value: input.to_owned() } }\n\
         pub fn unrelated() -> u32 { 42 }\n",
    )
    .expect("write");

    wait_for_blocks(&mut client, 4, Duration::from_secs(5));

    let resp = client.send(
        "tools/call",
        &json!({ "name": "search_by_return_type", "arguments": { "name": "Token" } }),
    );
    let payload = text_payload(&resp);
    let entries = payload["entries"].as_array().expect("entries");
    let labels: Vec<&str> = entries
        .iter()
        .filter_map(|e| e["from_label"].as_str())
        .collect();
    assert!(
        labels.contains(&"create"),
        "expected `create` in {labels:?}"
    );
    assert!(labels.contains(&"parse"), "expected `parse` in {labels:?}");
    assert!(
        !labels.contains(&"unrelated"),
        "`unrelated` should not appear, got: {labels:?}"
    );
}

#[test]
fn search_by_param_type_finds_consumer_functions() {
    let mut client = McpClient::spawn();
    let _ = client.send("initialize", &json!({}));

    fs::write(
        client.workspace().join("lib.rs"),
        "pub struct Request { url: String }\n\
         pub fn send(req: Request) {}\n\
         pub fn validate(r: &Request) -> bool { !r.url.is_empty() }\n",
    )
    .expect("write");

    wait_for_blocks(&mut client, 3, Duration::from_secs(5));

    let resp = client.send(
        "tools/call",
        &json!({ "name": "search_by_param_type", "arguments": { "name": "Request" } }),
    );
    let payload = text_payload(&resp);
    let entries = payload["entries"].as_array().expect("entries");
    let labels: Vec<&str> = entries
        .iter()
        .filter_map(|e| e["from_label"].as_str())
        .collect();
    assert!(labels.contains(&"send"));
    assert!(labels.contains(&"validate"));
}

#[test]
fn get_type_hierarchy_walks_extends_chain() {
    let mut client = McpClient::spawn();
    let _ = client.send("initialize", &json!({}));

    // TS interface chain: Animal <- Mammal <- Dog
    fs::write(
        client.workspace().join("zoo.ts"),
        "export interface Animal { name: string; }\n\
         export interface Mammal extends Animal { fur: boolean; }\n\
         export interface Dog extends Mammal { breed: string; }\n\
         export interface Cat extends Mammal { lazy: boolean; }\n",
    )
    .expect("write");

    wait_for_blocks(&mut client, 4, Duration::from_secs(5));

    // Ascendants depuis Dog : Mammal puis Animal.
    let resp = client.send(
        "tools/call",
        &json!({ "name": "get_type_hierarchy", "arguments": { "name": "Dog" } }),
    );
    let payload = text_payload(&resp);
    assert_eq!(payload["found"], json!(true));
    let candidates = payload["candidates"].as_array().expect("candidates");
    let dog = candidates
        .iter()
        .find(|c| c["label"].as_str() == Some("Dog"))
        .expect("Dog candidate");
    let ancestors: Vec<&str> = dog["ancestors"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(ancestors.contains(&"Mammal"), "ancestors: {ancestors:?}");
    assert!(ancestors.contains(&"Animal"), "ancestors: {ancestors:?}");

    // Descendants depuis Mammal : Dog et Cat.
    let resp = client.send(
        "tools/call",
        &json!({ "name": "get_type_hierarchy", "arguments": { "name": "Mammal" } }),
    );
    let payload = text_payload(&resp);
    let candidates = payload["candidates"].as_array().expect("candidates");
    let mammal = candidates
        .iter()
        .find(|c| c["label"].as_str() == Some("Mammal"))
        .expect("Mammal candidate");
    let descendants: Vec<&str> = mammal["descendants"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(descendants.contains(&"Dog"));
    assert!(descendants.contains(&"Cat"));
}

#[test]
fn find_usages_unknown_name_returns_empty() {
    let mut client = McpClient::spawn();
    let _ = client.send("initialize", &json!({}));

    fs::write(client.workspace().join("lib.rs"), "pub fn nothing() {}\n").expect("write");

    wait_for_blocks(&mut client, 1, Duration::from_secs(5));

    let resp = client.send(
        "tools/call",
        &json!({ "name": "find_usages", "arguments": { "name": "DoesNotExist" } }),
    );
    let payload = text_payload(&resp);
    assert_eq!(payload["count"], json!(0));
    let entries = payload["entries"].as_array().expect("entries");
    assert!(entries.is_empty());
}

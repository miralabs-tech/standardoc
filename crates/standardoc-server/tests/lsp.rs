use std::sync::{Arc, RwLock};

use serde_json::{Value, json};
use standardoc_core::{IndexHandle, LanguageProvider, ScanFilters};
use standardoc_lang_provider::WorkspaceProvider;
use standardoc_server::build_lsp_service;
use tempfile::TempDir;
use tower::Service;
use tower_lsp_server::jsonrpc::{Request, Response};

fn open_workspace() -> (TempDir, IndexHandle, Arc<dyn LanguageProvider>, Arc<RwLock<ScanFilters>>) {
    let dir = tempfile::tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    let filters = Arc::new(RwLock::new(ScanFilters::load(handle.workspace_root())));
    let provider: Arc<dyn LanguageProvider> = Arc::new(WorkspaceProvider::new());
    (dir, handle, provider, filters)
}

fn build_request(id: i64, method: &str, params: &Value) -> Request {
    serde_json::from_value(json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    }))
    .unwrap()
}

async fn call(
    service: &mut tower_lsp_server::LspService<standardoc_server::StandardocLsp>,
    req: Request,
) -> Response {
    std::future::poll_fn(|cx| service.poll_ready(cx))
        .await
        .expect("service ready");
    service
        .call(req)
        .await
        .expect("service call")
        .expect("response present")
}

fn ok_value(resp: Response) -> Value {
    let (_, body) = resp.into_parts();
    body.expect("response is Ok")
}

#[tokio::test(flavor = "multi_thread")]
async fn initialize_returns_capabilities_advertising_lsp_endpoints() {
    let (_dir, handle, provider, filters) = open_workspace();
    let (mut service, _socket) = build_lsp_service(handle, provider, filters);

    let resp = call(
        &mut service,
        build_request(1, "initialize", &json!({ "capabilities": {} })),
    )
    .await;
    let result = ok_value(resp);
    let caps = &result["capabilities"];

    assert_eq!(caps["hoverProvider"], json!(true));
    assert_eq!(caps["definitionProvider"], json!(true));
    assert_eq!(caps["referencesProvider"], json!(true));
    assert_eq!(caps["documentSymbolProvider"], json!(true));
    assert_eq!(caps["workspaceSymbolProvider"], json!(true));
}

#[tokio::test(flavor = "multi_thread")]
async fn initialize_then_shutdown_succeeds() {
    let (_dir, handle, provider, filters) = open_workspace();
    let (mut service, _socket) = build_lsp_service(handle, provider, filters);

    let _ = call(
        &mut service,
        build_request(1, "initialize", &json!({ "capabilities": {} })),
    )
    .await;
    let resp = call(&mut service, build_request(2, "shutdown", &json!(null))).await;
    let result = ok_value(resp);
    assert_eq!(result, json!(null));
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_symbol_query_on_empty_workspace_returns_empty() {
    let (_dir, handle, provider, filters) = open_workspace();
    let (mut service, _socket) = build_lsp_service(handle, provider, filters);

    let _ = call(
        &mut service,
        build_request(1, "initialize", &json!({ "capabilities": {} })),
    )
    .await;
    let resp = call(
        &mut service,
        build_request(2, "workspace/symbol", &json!({ "query": "anything" })),
    )
    .await;
    let result = ok_value(resp);
    assert_eq!(result, json!([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn document_symbol_on_unindexed_file_returns_empty_list() {
    let (dir, handle, provider, filters) = open_workspace();
    let (mut service, _socket) = build_lsp_service(handle, provider, filters);

    let _ = call(
        &mut service,
        build_request(1, "initialize", &json!({ "capabilities": {} })),
    )
    .await;

    let file_path = dir.path().join("ghost.rs");
    std::fs::write(&file_path, "fn ghost() {}").unwrap();
    let uri = format!("file:///{}", file_path.to_string_lossy().replace('\\', "/"));

    let resp = call(
        &mut service,
        build_request(
            2,
            "textDocument/documentSymbol",
            &json!({ "textDocument": { "uri": uri } }),
        ),
    )
    .await;
    let result = ok_value(resp);
    assert_eq!(result, json!([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn goto_definition_on_unindexed_position_returns_null() {
    let (dir, handle, provider, filters) = open_workspace();
    let (mut service, _socket) = build_lsp_service(handle, provider, filters);

    let _ = call(
        &mut service,
        build_request(1, "initialize", &json!({ "capabilities": {} })),
    )
    .await;

    let file_path = dir.path().join("ghost.rs");
    std::fs::write(&file_path, "fn ghost() {}").unwrap();
    let uri = format!("file:///{}", file_path.to_string_lossy().replace('\\', "/"));

    let resp = call(
        &mut service,
        build_request(
            2,
            "textDocument/definition",
            &json!({
                "textDocument": { "uri": uri },
                "position": { "line": 0, "character": 4 },
            }),
        ),
    )
    .await;
    let result = ok_value(resp);
    assert_eq!(result, json!(null));
}

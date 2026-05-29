use super::*;

#[test]
fn build_target_strips_upstream_path() {
    let url = build_target_url(
        "http://127.0.0.1:7701/mcp",
        &"/mcp".parse::<Uri>().unwrap(),
        None,
    );
    assert_eq!(url, "http://127.0.0.1:7701/mcp");
}

#[test]
fn build_target_preserves_query() {
    let url = build_target_url(
        "http://127.0.0.1:7701/mcp",
        &"/mcp?session=abc".parse::<Uri>().unwrap(),
        None,
    );
    assert_eq!(url, "http://127.0.0.1:7701/mcp?session=abc");
}

#[test]
fn build_target_uses_incoming_path_over_upstream_path() {
    let url = build_target_url(
        "http://127.0.0.1:7701/mcp",
        &"/other".parse::<Uri>().unwrap(),
        None,
    );
    assert_eq!(url, "http://127.0.0.1:7701/other");
}

#[test]
fn build_target_handles_upstream_without_path() {
    let url = build_target_url(
        "http://127.0.0.1:7701",
        &"/mcp".parse::<Uri>().unwrap(),
        None,
    );
    assert_eq!(url, "http://127.0.0.1:7701/mcp");
}

#[test]
fn build_target_strips_ws_prefix_when_provided() {
    // /ws/abc123/mcp → /mcp on the upstream side, daemon doesn't know
    // the proxy's routing prefix.
    let url = build_target_url(
        "http://127.0.0.1:7701/mcp",
        &"/ws/abc123/mcp".parse::<Uri>().unwrap(),
        Some("abc123"),
    );
    assert_eq!(url, "http://127.0.0.1:7701/mcp");
}

#[test]
fn build_target_strips_ws_prefix_preserves_subpath_and_query() {
    let url = build_target_url(
        "http://127.0.0.1:7701/mcp",
        &"/ws/abc123/mcp?session=xyz".parse::<Uri>().unwrap(),
        Some("abc123"),
    );
    assert_eq!(url, "http://127.0.0.1:7701/mcp?session=xyz");
}

#[test]
fn build_target_ws_prefix_strip_collapses_empty_remainder_to_slash() {
    // Edge case: client hits exactly /ws/<id> with no trailing path —
    // shouldn't produce an empty target URL.
    let url = build_target_url(
        "http://127.0.0.1:7701/mcp",
        &"/ws/abc123".parse::<Uri>().unwrap(),
        Some("abc123"),
    );
    assert_eq!(url, "http://127.0.0.1:7701/");
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

#[test]
fn workspace_id_is_deterministic_and_short() {
    let id_a = workspace_id_for(Path::new("/tmp/workspace-a"));
    let id_a2 = workspace_id_for(Path::new("/tmp/workspace-a"));
    let id_b = workspace_id_for(Path::new("/tmp/workspace-b"));
    assert_eq!(id_a, id_a2, "same path → same id");
    assert_ne!(id_a, id_b, "different paths → different ids");
    assert_eq!(id_a.len(), 8, "ids are 8 hex chars");
    assert!(
        id_a.chars().all(|c| c.is_ascii_hexdigit()),
        "ids are pure hex"
    );
}

fn boot_state(workspaces: Vec<(&str, &str)>) -> Arc<ProxyState> {
    let mut map = HashMap::new();
    let mut default_id = None;
    for (id, upstream) in workspaces {
        if default_id.is_none() {
            default_id = Some(id.to_string());
        }
        map.insert(
            id.to_string(),
            Arc::new(WorkspaceEntry {
                id: id.to_string(),
                root: PathBuf::from(format!("/tmp/ws-{id}")),
                upstream: Arc::new(RwLock::new(upstream.to_string())),
            }),
        );
    }
    Arc::new(ProxyState {
        workspaces: RwLock::new(map),
        default_id: default_id.unwrap_or_default(),
        client: build_forward_client(),
        retry_window: Duration::from_secs(1),
        total_requests: AtomicU64::new(0),
        successful_requests: AtomicU64::new(0),
        upstream_503_responses: AtomicU64::new(0),
        last_request_at: RwLock::new(None),
        started_at: SystemTime::now(),
    })
}

#[test]
fn parse_register_body_extracts_path() {
    let p = parse_register_body(r#"{"path":"/abs/dir"}"#).unwrap();
    assert_eq!(p, PathBuf::from("/abs/dir"));
}

#[test]
fn parse_register_body_tolerates_whitespace() {
    let p = parse_register_body("  { \"path\" : \"/abs/dir\" }  \n").unwrap();
    assert_eq!(p, PathBuf::from("/abs/dir"));
}

#[test]
fn parse_register_body_rejects_missing_path() {
    let err = parse_register_body(r#"{"foo":"bar"}"#).unwrap_err();
    assert!(err.contains("missing `path`"), "got {err}");
}

#[test]
fn parse_register_body_rejects_empty_path() {
    let err = parse_register_body(r#"{"path":""}"#).unwrap_err();
    assert!(err.contains("must not be empty"), "got {err}");
}

#[test]
fn parse_register_body_rejects_non_object() {
    let err = parse_register_body(r#""/abs/dir""#).unwrap_err();
    assert!(err.contains("JSON object"), "got {err}");
}

#[tokio::test]
async fn admin_list_workspaces_renders_default_and_entries() {
    let state = boot_state(vec![("ws00", "http://x:1/mcp"), ("ws01", "")]);
    let resp = admin_list_workspaces(State(state)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 16 * 1024)
        .await
        .unwrap();
    let s = std::str::from_utf8(&body).unwrap();
    assert!(s.contains("\"default_workspace_id\":\"ws00\""), "got {s}");
    assert!(s.contains("\"id\":\"ws00\""), "got {s}");
    assert!(s.contains("\"id\":\"ws01\""), "got {s}");
}

#[tokio::test]
async fn admin_unregister_default_rejected_with_409() {
    let state = boot_state(vec![("ws00", "http://x:1/mcp")]);
    let resp = admin_unregister_workspace(State(state), AxPath("ws00".to_string())).await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn admin_unregister_unknown_yields_404() {
    let state = boot_state(vec![("ws00", "")]);
    let resp = admin_unregister_workspace(State(state), AxPath("nope1234".to_string())).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admin_unregister_non_default_returns_204_and_removes() {
    let state = boot_state(vec![("ws00", "http://x:1/mcp"), ("ws01", "http://y:2/mcp")]);
    let resp =
        admin_unregister_workspace(State(Arc::clone(&state)), AxPath("ws01".to_string())).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    // ws01 must be gone from the routing table.
    assert!(!state.workspaces.read().await.contains_key("ws01"));
    assert!(state.workspaces.read().await.contains_key("ws00"));
}

#[tokio::test]
async fn health_reports_zero_counters_and_unknown_upstream_at_boot() {
    let state = boot_state(vec![("ws00", "")]);
    let resp = health(State(state)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 16 * 1024)
        .await
        .unwrap();
    let s = std::str::from_utf8(&body).unwrap();
    assert!(s.contains("\"default_workspace_id\":\"ws00\""), "got {s}");
    assert!(s.contains("\"id\":\"ws00\""), "got {s}");
    assert!(s.contains("\"upstream_known\":false"), "got {s}");
    assert!(s.contains("\"total_requests\":0"), "got {s}");
    assert!(s.contains("\"last_request_age_ms\":null"), "got {s}");
}

#[tokio::test]
async fn health_reports_counters_after_simulated_traffic() {
    let state = boot_state(vec![("ws00", "http://127.0.0.1:7701/mcp")]);
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
    assert!(!s.contains("\"last_request_age_ms\":null"), "got {s}");
}

#[tokio::test]
async fn health_lists_multiple_workspaces() {
    let state = boot_state(vec![
        ("ws00", "http://127.0.0.1:7701/mcp"),
        ("ws01", "http://127.0.0.1:7702/mcp"),
    ]);
    let resp = health(State(state)).await;
    let body = axum::body::to_bytes(resp.into_body(), 16 * 1024)
        .await
        .unwrap();
    let s = std::str::from_utf8(&body).unwrap();
    assert!(s.contains("\"id\":\"ws00\""), "got {s}");
    assert!(s.contains("\"id\":\"ws01\""), "got {s}");
    assert!(s.contains("\"default_workspace_id\":\"ws00\""), "got {s}");
}

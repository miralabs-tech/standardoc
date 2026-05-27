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

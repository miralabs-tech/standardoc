use super::*;
use tempfile::TempDir;

#[test]
fn wants_ephemeral_port_recognizes_zero() {
    assert!(wants_ephemeral_port("127.0.0.1:0"));
    assert!(wants_ephemeral_port("0.0.0.0:0"));
}

#[test]
fn wants_ephemeral_port_rejects_specific_port() {
    assert!(!wants_ephemeral_port("127.0.0.1:8765"));
    assert!(!wants_ephemeral_port("127.0.0.1:443"));
}

#[test]
fn wants_ephemeral_port_rejects_unparseable() {
    assert!(!wants_ephemeral_port("not-a-socket-addr"));
    assert!(!wants_ephemeral_port(""));
}

#[test]
fn read_previous_port_parses_endpoint_file() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join(".standardoc");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("mcp.endpoint"), "http://127.0.0.1:56831/mcp").unwrap();
    assert_eq!(read_previous_port(tmp.path()), Some(56831));
}

#[test]
fn read_previous_port_trims_trailing_newline() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join(".standardoc");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("mcp.endpoint"), "http://127.0.0.1:9000/mcp\n").unwrap();
    assert_eq!(read_previous_port(tmp.path()), Some(9000));
}

#[test]
fn read_previous_port_missing_file_is_none() {
    let tmp = TempDir::new().unwrap();
    assert_eq!(read_previous_port(tmp.path()), None);
}

#[test]
fn read_previous_port_malformed_is_none() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join(".standardoc");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("mcp.endpoint"), "not a url").unwrap();
    assert_eq!(read_previous_port(tmp.path()), None);
}

#[tokio::test]
async fn maybe_reuse_skips_when_caller_wants_specific_port() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join(".standardoc");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("mcp.endpoint"), "http://127.0.0.1:56831/mcp").unwrap();
    // Caller asked for a SPECIFIC port — reuse must be a no-op so
    // the caller's intent wins.
    let result = maybe_reuse_previous_port(tmp.path(), "127.0.0.1:7777").await;
    assert!(result.is_none());
}

#[tokio::test]
async fn maybe_reuse_returns_listener_when_port_is_free() {
    // First, bind an ephemeral listener and capture its address —
    // dropping it frees the port. Then write that as the previous
    // endpoint and verify the reuse path re-binds it cleanly.
    let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join(".standardoc");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("mcp.endpoint"),
        format!("http://127.0.0.1:{port}/mcp"),
    )
    .unwrap();
    let listener = maybe_reuse_previous_port(tmp.path(), "127.0.0.1:0")
        .await
        .expect("port should be reusable after probe drop");
    assert_eq!(listener.local_addr().unwrap().port(), port);
}

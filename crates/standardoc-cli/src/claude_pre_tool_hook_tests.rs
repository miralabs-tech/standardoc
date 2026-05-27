
use super::*;
use std::fs;
use tempfile::TempDir;

fn sentinel_in(tmp: &TempDir) -> PathBuf {
    tmp.path().join("mcp_called_this_session")
}

#[test]
fn mark_writes_sentinel_when_tool_is_standardoc_mcp() {
    let tmp = TempDir::new().unwrap();
    let sentinel = sentinel_in(&tmp);
    let payload = r#"{"tool_name":"mcp__standardoc__find_symbol","cwd":"/anywhere"}"#;
    let out = pre_tool_hook_decide("mark", payload, &sentinel);
    assert!(out.contains(r#""marked":true"#), "out={out}");
    assert!(sentinel.exists(), "sentinel must be written");
}

#[test]
fn mark_skips_non_standardoc_tool() {
    let tmp = TempDir::new().unwrap();
    let sentinel = sentinel_in(&tmp);
    let payload = r#"{"tool_name":"Bash"}"#;
    let out = pre_tool_hook_decide("mark", payload, &sentinel);
    assert!(out.contains("not_standardoc_mcp_tool"), "out={out}");
    assert!(!sentinel.exists(), "sentinel must NOT be written");
}

#[test]
fn mark_skips_when_tool_name_missing() {
    let tmp = TempDir::new().unwrap();
    let sentinel = sentinel_in(&tmp);
    let out = pre_tool_hook_decide("mark", r"{}", &sentinel);
    assert!(out.contains("not_standardoc_mcp_tool"), "out={out}");
    assert!(!sentinel.exists());
}

#[test]
fn mark_creates_parent_dirs() {
    let tmp = TempDir::new().unwrap();
    let nested = tmp.path().join("deep").join(".standardoc");
    let sentinel = nested.join("mcp_called_this_session");
    let payload = r#"{"tool_name":"mcp__standardoc__get_context"}"#;
    let out = pre_tool_hook_decide("mark", payload, &sentinel);
    assert!(out.contains(r#""marked":true"#), "out={out}");
    assert!(sentinel.exists());
}

#[test]
fn check_denies_when_sentinel_absent() {
    let tmp = TempDir::new().unwrap();
    let sentinel = sentinel_in(&tmp);
    let out = pre_tool_hook_decide("check", r"{}", &sentinel);
    assert!(out.contains(r#""permissionDecision":"deny""#), "out={out}");
    assert!(out.contains("MCP-first"));
    assert!(out.contains("find_symbol"));
}

#[test]
fn check_allows_when_sentinel_present() {
    let tmp = TempDir::new().unwrap();
    let sentinel = sentinel_in(&tmp);
    fs::write(&sentinel, b"").unwrap();
    let out = pre_tool_hook_decide("check", r"{}", &sentinel);
    assert_eq!(out, "{}");
}

#[test]
fn check_emits_pretooluse_hook_event_name() {
    // Claude Code requires the hookSpecificOutput.hookEventName to
    // match the firing event, otherwise the JSON is silently
    // ignored. Lock the wire shape.
    let tmp = TempDir::new().unwrap();
    let sentinel = sentinel_in(&tmp);
    let out = pre_tool_hook_decide("check", r"{}", &sentinel);
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        parsed
            .get("hookSpecificOutput")
            .and_then(|v| v.get("hookEventName"))
            .and_then(|v| v.as_str()),
        Some("PreToolUse"),
    );
}

#[test]
fn reset_removes_sentinel() {
    let tmp = TempDir::new().unwrap();
    let sentinel = sentinel_in(&tmp);
    fs::write(&sentinel, b"").unwrap();
    let out = pre_tool_hook_decide("reset", r"{}", &sentinel);
    assert!(out.contains(r#""reset":true"#));
    assert!(!sentinel.exists());
}

#[test]
fn reset_is_idempotent_when_sentinel_absent() {
    let tmp = TempDir::new().unwrap();
    let sentinel = sentinel_in(&tmp);
    let out = pre_tool_hook_decide("reset", r"{}", &sentinel);
    // Must not panic; output is the reset confirmation either way.
    assert!(out.contains(r#""reset":true"#));
}

#[test]
fn invalid_json_does_not_panic_in_any_mode() {
    let tmp = TempDir::new().unwrap();
    let sentinel = sentinel_in(&tmp);
    // Mark with garbage payload — must not panic, must not write
    // the sentinel (no tool name resolvable).
    let out = pre_tool_hook_decide("mark", "not json", &sentinel);
    assert!(out.contains("not_standardoc_mcp_tool"), "out={out}");
    assert!(!sentinel.exists());
    // Check with garbage payload — same as a missing sentinel.
    let out = pre_tool_hook_decide("check", "not json", &sentinel);
    assert!(out.contains(r#""permissionDecision":"deny""#));
    // Reset with garbage payload — no-op (file already absent).
    let out = pre_tool_hook_decide("reset", "not json", &sentinel);
    assert!(out.contains(r#""reset":true"#));
}

#[test]
fn unknown_mode_returns_safe_default() {
    let tmp = TempDir::new().unwrap();
    let sentinel = sentinel_in(&tmp);
    let out = pre_tool_hook_decide("nope", r"{}", &sentinel);
    assert!(out.contains("unknown_mode"));
    // Must not implicitly deny — clap's value_parser already
    // forbids this CLI-side, but a defence-in-depth default is
    // "do not block the agent".
    assert!(!out.contains(r#""permissionDecision":"deny""#));
}

#[test]
fn resolve_sentinel_uses_payload_cwd() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().to_string_lossy().replace('\\', "/");
    let payload = format!(r#"{{"cwd":"{cwd}"}}"#);
    let sentinel = resolve_mcp_first_sentinel(&payload);
    let expected = tmp.path().join(".standardoc").join(MCP_FIRST_SENTINEL);
    assert_eq!(sentinel, expected);
}

#[test]
fn resolve_sentinel_falls_back_to_current_dir_when_payload_lacks_cwd() {
    // The fallback chain is cwd → CLAUDE_PROJECT_DIR → current_dir;
    // we only assert the chain doesn't panic and produces a path
    // ending with the sentinel name + parent `.standardoc`.
    let sentinel = resolve_mcp_first_sentinel(r"{}");
    assert_eq!(
        sentinel.file_name().and_then(|s| s.to_str()),
        Some(MCP_FIRST_SENTINEL),
    );
    assert_eq!(
        sentinel
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str()),
        Some(".standardoc"),
    );
}

//! Idempotent merge of Standardoc's `.mcp.json` server entry.
//!
//! Mirrors `ext/vscode/src/init/mcp-config.ts` in spirit, but the CLI
//! installs the **stdio bridge** entry the agent spawns:
//!
//! ```json
//! { "type": "stdio", "command": "<abs standardoc>", "args": ["mcp", "--connect", "<abs root>"] }
//! ```
//!
//! The bridge (`standardoc mcp --connect`) ensures a warm, watcher-backed
//! http daemon for the workspace and relays JSON-RPC to it over stdio — so
//! a Claude Code CLI user without the VSCode extension still gets the live
//! index. The extension instead writes an `http` entry pointing straight at
//! the daemon it supervises. Both share the `standardoc` server key so a
//! workspace touched by either path converges on one entry.

use serde_json::{Map, Value, json};

const STANDARDOC_SERVER_KEY: &str = "standardoc";

/// Outcome of merging the Standardoc entry into an existing (or absent)
/// `.mcp.json`. The write-bearing variants carry the serialized JSON;
/// `NoOp` means the entry already matched; `Invalid` carries a parse/shape
/// error (the caller warns and leaves the file untouched).
pub(crate) enum MergeOutcome {
    NoOp,
    Invalid(String),
    /// File was absent or had no servers — we wrote a fresh config.
    Create(String),
    /// File had other servers; we added ours alongside them.
    AddFirst(String),
    /// A stale Standardoc entry (e.g. legacy `http` url) was rewritten.
    OverwriteStale(String),
}

/// The canonical stdio bridge entry. `command` is the absolute path to the
/// `standardoc` binary; `args` is `["mcp", "--connect", <abs root>]`.
fn expected_entry(command: &str, args: &[String]) -> Value {
    json!({ "type": "stdio", "command": command, "args": args })
}

/// `raw` is the current file contents (`None` when the file is absent).
pub(crate) fn merge_mcp_config(raw: Option<&str>, command: &str, args: &[String]) -> MergeOutcome {
    let absent = raw.is_none();
    let mut root: Map<String, Value> = match raw {
        None => Map::new(),
        Some(s) if s.trim().is_empty() => Map::new(),
        Some(s) => match serde_json::from_str::<Value>(s) {
            Ok(Value::Object(m)) => m,
            Ok(_) => {
                return MergeOutcome::Invalid(
                    "Root of .mcp.json must be a JSON object".to_string(),
                );
            }
            Err(e) => return MergeOutcome::Invalid(e.to_string()),
        },
    };

    let mut servers = match root.remove("mcpServers") {
        Some(Value::Object(m)) => m,
        Some(_) => {
            return MergeOutcome::Invalid(
                "`mcpServers` in .mcp.json must be a JSON object".to_string(),
            );
        }
        None => Map::new(),
    };

    let expected = expected_entry(command, args);
    let had_servers = !servers.is_empty();
    let current = servers.get(STANDARDOC_SERVER_KEY).cloned();

    if let Some(current) = current {
        if entry_matches(&current, &expected) {
            return MergeOutcome::NoOp;
        }
        // Preserve user-customised sibling fields (e.g. `env`) while pinning
        // the canonical transport fields and dropping any legacy http `url`.
        let mut merged = match current {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        merged.insert("type".to_string(), json!("stdio"));
        merged.insert("command".to_string(), json!(command));
        merged.insert("args".to_string(), json!(args));
        merged.remove("url");
        servers.insert(STANDARDOC_SERVER_KEY.to_string(), Value::Object(merged));
        root.insert("mcpServers".to_string(), Value::Object(servers));
        return MergeOutcome::OverwriteStale(serialize(&Value::Object(root)));
    }

    servers.insert(STANDARDOC_SERVER_KEY.to_string(), expected);
    root.insert("mcpServers".to_string(), Value::Object(servers));
    let serialized = serialize(&Value::Object(root));
    if absent || !had_servers {
        MergeOutcome::Create(serialized)
    } else {
        MergeOutcome::AddFirst(serialized)
    }
}

/// Match on the canonical transport fields only — extra user fields (such as
/// `env`) on the actual entry are tolerated so we never clobber them. `type`
/// defaults to `stdio` on both sides when omitted.
fn entry_matches(actual: &Value, expected: &Value) -> bool {
    let a_type = actual
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("stdio");
    let e_type = expected
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("stdio");
    if a_type != e_type {
        return false;
    }
    if let Some(e_cmd) = expected.get("command").and_then(Value::as_str)
        && actual.get("command").and_then(Value::as_str) != Some(e_cmd)
    {
        return false;
    }
    if let Some(e_args) = expected.get("args").and_then(Value::as_array) {
        let Some(a_args) = actual.get("args").and_then(Value::as_array) else {
            return false;
        };
        if a_args != e_args {
            return false;
        }
    }
    if let Some(e_url) = expected.get("url").and_then(Value::as_str)
        && actual.get("url").and_then(Value::as_str) != Some(e_url)
    {
        return false;
    }
    true
}

fn serialize(value: &Value) -> String {
    let mut s = serde_json::to_string_pretty(value).unwrap_or_default();
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const CMD: &str = "/opt/bin/standardoc";

    fn args() -> Vec<String> {
        vec![
            "mcp".to_string(),
            "--connect".to_string(),
            "/home/me/proj".to_string(),
        ]
    }

    fn rendered(outcome: &MergeOutcome) -> &str {
        match outcome {
            MergeOutcome::Create(s)
            | MergeOutcome::AddFirst(s)
            | MergeOutcome::OverwriteStale(s) => s,
            _ => panic!("expected serialized output"),
        }
    }

    #[test]
    fn creates_when_absent() {
        let out = merge_mcp_config(None, CMD, &args());
        assert!(matches!(out, MergeOutcome::Create(_)));
        let v: Value = serde_json::from_str(rendered(&out)).unwrap();
        assert_eq!(v["mcpServers"]["standardoc"]["type"], "stdio");
        assert_eq!(v["mcpServers"]["standardoc"]["command"], CMD);
        assert_eq!(v["mcpServers"]["standardoc"]["args"][1], "--connect");
    }

    #[test]
    fn empty_string_is_create() {
        // An existing-but-empty file has no servers, so it is a `create`.
        assert!(matches!(
            merge_mcp_config(Some("  \n"), CMD, &args()),
            MergeOutcome::Create(_)
        ));
    }

    #[test]
    fn is_noop_when_matching() {
        let created = rendered(&merge_mcp_config(None, CMD, &args())).to_string();
        assert!(matches!(
            merge_mcp_config(Some(&created), CMD, &args()),
            MergeOutcome::NoOp
        ));
    }

    #[test]
    fn idempotent_across_two_merges() {
        let first = rendered(&merge_mcp_config(None, CMD, &args())).to_string();
        let second = merge_mcp_config(Some(&first), CMD, &args());
        assert!(matches!(second, MergeOutcome::NoOp));
    }

    #[test]
    fn add_first_when_other_servers_present() {
        let existing = serialize(&json!({
            "mcpServers": { "other": { "type": "stdio", "command": "foo", "args": [] } }
        }));
        let out = merge_mcp_config(Some(&existing), CMD, &args());
        assert!(matches!(out, MergeOutcome::AddFirst(_)));
        let v: Value = serde_json::from_str(rendered(&out)).unwrap();
        // Sibling server survives.
        assert_eq!(v["mcpServers"]["other"]["command"], "foo");
        assert_eq!(v["mcpServers"]["standardoc"]["type"], "stdio");
    }

    #[test]
    fn overwrites_stale_http_entry_and_keeps_env() {
        let existing = serialize(&json!({
            "mcpServers": {
                "standardoc": {
                    "type": "http",
                    "url": "http://127.0.0.1:7700/mcp",
                    "env": { "RUST_LOG": "info" }
                }
            }
        }));
        let out = merge_mcp_config(Some(&existing), CMD, &args());
        assert!(matches!(out, MergeOutcome::OverwriteStale(_)));
        let v: Value = serde_json::from_str(rendered(&out)).unwrap();
        let entry = &v["mcpServers"]["standardoc"];
        assert_eq!(entry["type"], "stdio");
        assert_eq!(entry["command"], CMD);
        assert!(
            entry.get("url").is_none(),
            "legacy http url must be dropped"
        );
        // User-authored sibling field is preserved.
        assert_eq!(entry["env"]["RUST_LOG"], "info");
    }

    #[test]
    fn preserves_unknown_top_level_keys() {
        let existing = serialize(&json!({
            "$schema": "https://example.com/mcp.json",
            "mcpServers": { "other": { "type": "stdio", "command": "foo" } }
        }));
        let out = merge_mcp_config(Some(&existing), CMD, &args());
        let v: Value = serde_json::from_str(rendered(&out)).unwrap();
        assert_eq!(v["$schema"], "https://example.com/mcp.json");
    }

    #[test]
    fn rejects_non_object_root() {
        assert!(matches!(
            merge_mcp_config(Some("[1,2,3]"), CMD, &args()),
            MergeOutcome::Invalid(_)
        ));
        assert!(matches!(
            merge_mcp_config(Some("not json"), CMD, &args()),
            MergeOutcome::Invalid(_)
        ));
    }

    #[test]
    fn rejects_non_object_mcp_servers() {
        assert!(matches!(
            merge_mcp_config(Some(r#"{"mcpServers": []}"#), CMD, &args()),
            MergeOutcome::Invalid(_)
        ));
    }

    #[test]
    fn noop_tolerates_extra_user_fields_on_matching_entry() {
        // Same transport fields + a user `env` => still a no-op (we don't
        // strip the env just because it's there).
        let existing = serialize(&json!({
            "mcpServers": {
                "standardoc": {
                    "type": "stdio",
                    "command": CMD,
                    "args": args(),
                    "env": { "FOO": "bar" }
                }
            }
        }));
        assert!(matches!(
            merge_mcp_config(Some(&existing), CMD, &args()),
            MergeOutcome::NoOp
        ));
    }
}

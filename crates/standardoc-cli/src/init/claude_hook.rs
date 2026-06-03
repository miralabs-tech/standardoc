//! Idempotent merge of Standardoc's four `.claude/settings.json` hooks.
//!
//! Mirrors `ext/vscode/src/init/claude-hook.ts` — the command strings and
//! grep-stable markers are byte-identical so a workspace initialised by
//! either the CLI or the extension converges on the same settings and a
//! second init never duplicates an entry.
//!
//! Managed hooks:
//! - `UserPromptSubmit`: advisory MCP nudge.
//! - `PreToolUse` (`mcp__standardoc__.*`): marks the MCP-first sentinel.
//! - `PreToolUse` (`Bash|Read|Grep|Glob`): denies when the sentinel is absent.
//! - `SessionStart`: resets the sentinel so each chat starts strict.

use serde_json::{Map, Value, json};

const HOOK_MARKER: &str = "STANDARDOC_MCP_NUDGE";
const HOOK_MESSAGE: &str = "Standardoc live AST index is available via MCP tools (find_symbol, get_context, list_symbols, find_symbols_by_pattern, find_similar_symbols, current_revision, check_stale). Use them BEFORE Read/Grep/Glob for any code task.";

const MCP_FIRST_MARK_MARKER: &str = "pre-tool-hook --mode mark";
const MCP_FIRST_MARK_COMMAND: &str = "standardoc claude pre-tool-hook --mode mark";
const MCP_FIRST_CHECK_MARKER: &str = "pre-tool-hook --mode check";
const MCP_FIRST_CHECK_COMMAND: &str = "standardoc claude pre-tool-hook --mode check";
const MCP_FIRST_RESET_MARKER: &str = "pre-tool-hook --mode reset";
const MCP_FIRST_RESET_COMMAND: &str = "standardoc claude pre-tool-hook --mode reset";

fn nudge_command() -> String {
    format!("echo \"{HOOK_MARKER}: {HOOK_MESSAGE}\"")
}

fn hook_group(matcher: &str, command: &str) -> Value {
    json!({ "matcher": matcher, "hooks": [{ "type": "command", "command": command }] })
}

/// Outcome of merging the Standardoc hooks into an existing (or absent)
/// `.claude/settings.json`. `Created` / `Appended` carry the serialized
/// JSON to write; `NoOp` means every hook was already present; `Invalid`
/// carries a parse/shape error (the caller warns and leaves the file alone).
pub(crate) enum MergeOutcome {
    NoOp,
    Created(String),
    Appended(String),
    Invalid(String),
}

/// `raw` is the current file contents (`None` when the file is absent).
pub(crate) fn merge_claude_hook(raw: Option<&str>) -> MergeOutcome {
    let absent = raw.is_none();
    let mut root: Map<String, Value> = match raw {
        None => Map::new(),
        Some(s) if s.trim().is_empty() => Map::new(),
        Some(s) => match serde_json::from_str::<Value>(s) {
            Ok(Value::Object(m)) => m,
            Ok(_) => {
                return MergeOutcome::Invalid(
                    "Root of .claude/settings.json must be a JSON object".to_string(),
                );
            }
            Err(e) => return MergeOutcome::Invalid(e.to_string()),
        },
    };

    let mut hooks = match root.remove("hooks") {
        Some(Value::Object(m)) => m,
        Some(_) => {
            return MergeOutcome::Invalid(
                "`hooks` in .claude/settings.json must be a JSON object".to_string(),
            );
        }
        None => Map::new(),
    };

    let mut changed = false;
    changed |= ensure_hook(&mut hooks, "UserPromptSubmit", HOOK_MARKER, || {
        hook_group("", &nudge_command())
    });
    changed |= ensure_hook(&mut hooks, "PreToolUse", MCP_FIRST_MARK_MARKER, || {
        hook_group("mcp__standardoc__.*", MCP_FIRST_MARK_COMMAND)
    });
    changed |= ensure_hook(&mut hooks, "PreToolUse", MCP_FIRST_CHECK_MARKER, || {
        hook_group("Bash|Read|Grep|Glob", MCP_FIRST_CHECK_COMMAND)
    });
    changed |= ensure_hook(&mut hooks, "SessionStart", MCP_FIRST_RESET_MARKER, || {
        hook_group("", MCP_FIRST_RESET_COMMAND)
    });

    if !changed {
        return MergeOutcome::NoOp;
    }

    root.insert("hooks".to_string(), Value::Object(hooks));
    let serialized = serialize(&Value::Object(root));
    if absent {
        MergeOutcome::Created(serialized)
    } else {
        MergeOutcome::Appended(serialized)
    }
}

/// Append `build()`'s group to `hooks[event]` when no group there already
/// carries `marker` in a command string. Returns `true` when it appended.
fn ensure_hook(
    hooks: &mut Map<String, Value>,
    event: &str,
    marker: &str,
    build: impl FnOnce() -> Value,
) -> bool {
    let entry = hooks
        .entry(event.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let Value::Array(groups) = entry else {
        // A non-array event slot is user-authored noise we won't clobber;
        // skip rather than risk corrupting it.
        return false;
    };
    if groups.iter().any(|g| group_contains_marker(g, marker)) {
        return false;
    }
    groups.push(build());
    true
}

fn group_contains_marker(group: &Value, marker: &str) -> bool {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| {
            hooks.iter().any(|h| {
                h.get("command")
                    .and_then(Value::as_str)
                    .is_some_and(|c| c.contains(marker))
            })
        })
}

fn serialize(value: &Value) -> String {
    let mut s = serde_json::to_string_pretty(value).unwrap_or_default();
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered(outcome: &MergeOutcome) -> &str {
        match outcome {
            MergeOutcome::Created(s) | MergeOutcome::Appended(s) => s,
            _ => panic!("expected serialized output"),
        }
    }

    #[test]
    fn creates_all_four_hooks_when_absent() {
        let out = merge_claude_hook(None);
        assert!(matches!(out, MergeOutcome::Created(_)));
        let s = rendered(&out);
        for marker in [
            HOOK_MARKER,
            MCP_FIRST_MARK_MARKER,
            MCP_FIRST_CHECK_MARKER,
            MCP_FIRST_RESET_MARKER,
        ] {
            assert!(s.contains(marker), "missing {marker}");
        }
        // Valid JSON, parses back to an object with a hooks object.
        let v: Value = serde_json::from_str(s).unwrap();
        assert!(v["hooks"].is_object());
    }

    #[test]
    fn is_noop_when_all_present() {
        let created = rendered(&merge_claude_hook(None)).to_string();
        assert!(matches!(
            merge_claude_hook(Some(&created)),
            MergeOutcome::NoOp
        ));
    }

    #[test]
    fn appends_only_missing_hooks() {
        // Start with just the advisory nudge already installed.
        let partial = serialize(&json!({
            "hooks": { "UserPromptSubmit": [hook_group("", &nudge_command())] }
        }));
        let out = merge_claude_hook(Some(&partial));
        assert!(matches!(out, MergeOutcome::Appended(_)));
        let s = rendered(&out);
        for marker in [
            MCP_FIRST_MARK_MARKER,
            MCP_FIRST_CHECK_MARKER,
            MCP_FIRST_RESET_MARKER,
        ] {
            assert!(s.contains(marker), "missing {marker}");
        }
        // The pre-existing nudge is not duplicated.
        assert_eq!(s.matches(HOOK_MARKER).count(), 1);
    }

    #[test]
    fn preserves_unknown_keys_and_foreign_hooks() {
        let existing = serialize(&json!({
            "model": "opus",
            "hooks": {
                "UserPromptSubmit": [hook_group("", "echo other-tool")],
            }
        }));
        let s = rendered(&merge_claude_hook(Some(&existing))).to_string();
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["model"], "opus");
        // Foreign UserPromptSubmit hook survives alongside ours.
        assert!(s.contains("echo other-tool"));
        assert!(s.contains(HOOK_MARKER));
    }

    #[test]
    fn rejects_non_object_root() {
        assert!(matches!(
            merge_claude_hook(Some("[1, 2, 3]")),
            MergeOutcome::Invalid(_)
        ));
        assert!(matches!(
            merge_claude_hook(Some("not json")),
            MergeOutcome::Invalid(_)
        ));
    }

    #[test]
    fn empty_string_is_treated_as_empty_object() {
        // Existing-but-empty file parses as `{}`, so the hooks are appended
        // (the `create` verb is reserved for a genuinely absent file).
        let out = merge_claude_hook(Some("   "));
        assert!(matches!(out, MergeOutcome::Appended(_)));
        assert!(rendered(&out).contains(MCP_FIRST_CHECK_MARKER));
    }

    #[test]
    fn idempotent_across_two_merges() {
        let first = rendered(&merge_claude_hook(None)).to_string();
        let second = merge_claude_hook(Some(&first));
        assert!(matches!(second, MergeOutcome::NoOp));
    }
}

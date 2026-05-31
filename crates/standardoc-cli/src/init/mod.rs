//! `standardoc init` — install the Standardoc agent integration into a
//! workspace so a Claude Code (or other SKILL.md-aware) agent discovers the
//! live index. Writes four artefacts, each an idempotent merge that
//! preserves user content: the agent skill, the `.claude/settings.json`
//! MCP-first hooks, the repo-root `AGENTS.md` section, and the `.mcp.json`
//! stdio bridge entry the agent spawns (`standardoc mcp --connect`).

mod agents_md;
mod claude_hook;
mod mcp_config;

use std::path::Path;

use standardoc_server::ServerError;

/// The agent skill body, single-sourced from the shared asset the VSCode
/// extension also embeds (`ext/vscode/src/init/skill-template.ts` imports the
/// same file). One source keeps both emitters byte-identical.
const SKILL_CONTENT: &str = include_str!("../../assets/skill.md");

/// Workspace-relative path of the generated skill — matches the extension's
/// `SKILL_RELATIVE_PATH`.
const SKILL_RELATIVE_PATH: &str = ".claude/skills/standardoc/SKILL.md";

/// Workspace-relative Claude Code settings file the hook merge targets —
/// matches the extension's `CLAUDE_SETTINGS_FILE`.
const CLAUDE_SETTINGS_PATH: &str = ".claude/settings.json";

/// Repo-root cross-agent instructions file (Codex / Cursor / Copilot / Gemini).
const AGENTS_MD_PATH: &str = "AGENTS.md";

/// Repo-root MCP server config Claude Code (and other MCP clients) reads.
const MCP_CONFIG_PATH: &str = ".mcp.json";

pub(crate) fn run(workspace_root: &Path) -> Result<(), ServerError> {
    write_skill(workspace_root)?;
    write_claude_hook(workspace_root)?;
    write_agents_md(workspace_root)?;
    write_mcp_config(workspace_root)?;
    Ok(())
}

fn write_skill(workspace_root: &Path) -> Result<(), ServerError> {
    let target = workspace_root.join(SKILL_RELATIVE_PATH);
    if let Ok(existing) = std::fs::read_to_string(&target)
        && normalize(&existing) == normalize(SKILL_CONTENT)
    {
        println!("[init] agent skill already up to date: {SKILL_RELATIVE_PATH}");
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(ServerError::Io)?;
    }
    std::fs::write(&target, SKILL_CONTENT).map_err(ServerError::Io)?;
    println!("[init] wrote agent skill: {SKILL_RELATIVE_PATH}");
    Ok(())
}

/// Merge the four MCP-first hooks into `.claude/settings.json`,
/// preserving any user-authored content. Idempotent across re-runs. A
/// settings file we cannot parse is reported and left untouched rather than
/// clobbered.
fn write_claude_hook(workspace_root: &Path) -> Result<(), ServerError> {
    let target = workspace_root.join(CLAUDE_SETTINGS_PATH);
    let raw = match std::fs::read_to_string(&target) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(ServerError::Io(e)),
    };
    match claude_hook::merge_claude_hook(raw.as_deref()) {
        claude_hook::MergeOutcome::NoOp => {
            println!("[init] .claude/settings.json already wires the Standardoc hooks");
        }
        claude_hook::MergeOutcome::Invalid(err) => {
            eprintln!(
                "[init] .claude/settings.json could not be parsed ({err}); skipping hook install"
            );
        }
        claude_hook::MergeOutcome::Created(content)
        | claude_hook::MergeOutcome::Appended(content) => {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(ServerError::Io)?;
            }
            std::fs::write(&target, content).map_err(ServerError::Io)?;
            println!("[init] wrote Standardoc hooks to .claude/settings.json");
        }
    }
    Ok(())
}

/// Merge the short marker-delimited Standardoc section into the repo-root
/// `AGENTS.md` so non-Claude agents (Codex, Cursor, Copilot, ...) also learn
/// the index exists. Idempotent; user content outside the markers is kept.
fn write_agents_md(workspace_root: &Path) -> Result<(), ServerError> {
    let target = workspace_root.join(AGENTS_MD_PATH);
    let raw = match std::fs::read_to_string(&target) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(ServerError::Io(e)),
    };
    match agents_md::merge_agents_md(raw.as_deref()) {
        agents_md::MergeOutcome::NoOp => {
            println!("[init] AGENTS.md already references Standardoc");
        }
        agents_md::MergeOutcome::Written(content) => {
            std::fs::write(&target, content).map_err(ServerError::Io)?;
            println!("[init] wrote the Standardoc section to AGENTS.md");
        }
    }
    Ok(())
}

/// Merge the Standardoc stdio bridge entry into the repo-root `.mcp.json` so
/// Claude Code spawns `standardoc mcp --connect <root>` for the live index.
/// Uses the absolute path of the running binary and the absolute workspace
/// root so the entry resolves regardless of the agent's working directory.
/// Idempotent; user-authored siblings (other servers, `env`) are preserved.
fn write_mcp_config(workspace_root: &Path) -> Result<(), ServerError> {
    let target = workspace_root.join(MCP_CONFIG_PATH);
    let command = std::env::current_exe()
        .map_err(ServerError::Io)?
        .to_string_lossy()
        .into_owned();
    let args = vec![
        "mcp".to_string(),
        "--connect".to_string(),
        absolute_root(workspace_root),
    ];
    let raw = match std::fs::read_to_string(&target) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(ServerError::Io(e)),
    };
    match mcp_config::merge_mcp_config(raw.as_deref(), &command, &args) {
        mcp_config::MergeOutcome::NoOp => {
            println!("[init] .mcp.json already wires the Standardoc bridge");
        }
        mcp_config::MergeOutcome::Invalid(err) => {
            eprintln!(
                "[init] .mcp.json could not be parsed ({err}); skipping bridge wiring. \
                 Add it manually: a `standardoc` server of type \"stdio\" running `{command} mcp --connect`."
            );
        }
        mcp_config::MergeOutcome::Create(content)
        | mcp_config::MergeOutcome::AddFirst(content)
        | mcp_config::MergeOutcome::OverwriteStale(content) => {
            std::fs::write(&target, content).map_err(ServerError::Io)?;
            println!(
                "[init] wrote the Standardoc bridge to .mcp.json (machine-specific paths — \
                 consider adding it to .gitignore if collaborating)"
            );
        }
    }
    Ok(())
}

/// Absolutise `workspace_root` without touching symlinks, stripping the
/// Windows `\\?\` verbatim prefix `canonicalize` adds so the path stays clean
/// in `.mcp.json`. Falls back to the path as given if canonicalisation fails.
fn absolute_root(workspace_root: &Path) -> String {
    let abs = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());
    let s = abs.to_string_lossy();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
}

/// CRLF→LF + trailing-whitespace trim, mirroring the extension's `normalize`
/// drift check so a re-init is a no-op regardless of how the editor saved it.
fn normalize(s: &str) -> String {
    s.replace("\r\n", "\n").trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn skill_content_is_embedded_from_the_shared_asset() {
        assert!(SKILL_CONTENT.starts_with("---\n"));
        assert!(SKILL_CONTENT.contains("name: standardoc"));
        assert!(SKILL_CONTENT.contains("3-phase MCP-first protocol"));
    }

    #[test]
    fn write_skill_creates_file_when_absent() {
        let tmp = tempdir().unwrap();
        run(tmp.path()).unwrap();
        let written = std::fs::read_to_string(tmp.path().join(SKILL_RELATIVE_PATH)).unwrap();
        assert_eq!(written, SKILL_CONTENT);
    }

    #[test]
    fn write_skill_is_idempotent_on_matching_content() {
        let tmp = tempdir().unwrap();
        run(tmp.path()).unwrap();
        run(tmp.path()).unwrap();
        let written = std::fs::read_to_string(tmp.path().join(SKILL_RELATIVE_PATH)).unwrap();
        assert_eq!(written, SKILL_CONTENT);
    }

    #[test]
    fn write_skill_regenerates_when_content_differs() {
        let tmp = tempdir().unwrap();
        let target = tmp.path().join(SKILL_RELATIVE_PATH);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "stale\n").unwrap();
        run(tmp.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), SKILL_CONTENT);
    }

    #[test]
    fn run_installs_skill_and_claude_hooks() {
        let tmp = tempdir().unwrap();
        run(tmp.path()).unwrap();
        assert!(tmp.path().join(SKILL_RELATIVE_PATH).exists());
        let settings = std::fs::read_to_string(tmp.path().join(CLAUDE_SETTINGS_PATH)).unwrap();
        assert!(settings.contains("pre-tool-hook --mode check"));
        assert!(settings.contains("STANDARDOC_MCP_NUDGE"));
        let agents = std::fs::read_to_string(tmp.path().join(AGENTS_MD_PATH)).unwrap();
        assert!(agents.contains("## Standardoc"));
    }

    #[test]
    fn run_is_idempotent_for_hooks() {
        let tmp = tempdir().unwrap();
        run(tmp.path()).unwrap();
        let first = std::fs::read_to_string(tmp.path().join(CLAUDE_SETTINGS_PATH)).unwrap();
        run(tmp.path()).unwrap();
        let second = std::fs::read_to_string(tmp.path().join(CLAUDE_SETTINGS_PATH)).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn run_writes_mcp_bridge_entry() {
        let tmp = tempdir().unwrap();
        run(tmp.path()).unwrap();
        let raw = std::fs::read_to_string(tmp.path().join(MCP_CONFIG_PATH)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let entry = &v["mcpServers"]["standardoc"];
        assert_eq!(entry["type"], "stdio");
        assert_eq!(entry["args"][0], "mcp");
        assert_eq!(entry["args"][1], "--connect");
        // The third arg is the absolute workspace root.
        assert!(entry["args"][2].as_str().unwrap().len() > 1);
    }

    #[test]
    fn run_is_idempotent_for_mcp_config() {
        let tmp = tempdir().unwrap();
        run(tmp.path()).unwrap();
        let first = std::fs::read_to_string(tmp.path().join(MCP_CONFIG_PATH)).unwrap();
        run(tmp.path()).unwrap();
        let second = std::fs::read_to_string(tmp.path().join(MCP_CONFIG_PATH)).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn normalize_ignores_crlf_and_trailing_whitespace() {
        assert_eq!(normalize("a\r\nb\n\n\n"), normalize("a\nb"));
        assert_eq!(normalize("x  \n"), "x");
    }
}

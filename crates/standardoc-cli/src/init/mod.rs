//! `standardoc init` — install the Standardoc agent integration into a
//! workspace so a Claude Code (or other SKILL.md-aware) agent discovers the
//! live index. This increment writes the skill file; the `.mcp.json`,
//! `.claude/settings.json` hooks, and `AGENTS.md` merges land next.

use std::path::Path;

use standardoc_server::ServerError;

/// The agent skill body, single-sourced from the shared asset the VSCode
/// extension also embeds (`ext/vscode/src/init/skill-template.ts` imports the
/// same file). One source keeps both emitters byte-identical.
const SKILL_CONTENT: &str = include_str!("../../assets/skill.md");

/// Workspace-relative path of the generated skill — matches the extension's
/// `SKILL_RELATIVE_PATH`.
const SKILL_RELATIVE_PATH: &str = ".claude/skills/standardoc/SKILL.md";

pub(crate) fn run(workspace_root: &Path) -> Result<(), ServerError> {
    write_skill(workspace_root)
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
    fn normalize_ignores_crlf_and_trailing_whitespace() {
        assert_eq!(normalize("a\r\nb\n\n\n"), normalize("a\nb"));
        assert_eq!(normalize("x  \n"), "x");
    }
}

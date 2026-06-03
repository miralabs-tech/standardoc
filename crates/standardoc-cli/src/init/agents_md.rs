//! Idempotent merge of a short, marker-delimited Standardoc section into the
//! repo-root `AGENTS.md` — the cross-agent instructions file read natively by
//! Codex, Cursor, Copilot, Gemini CLI and others. The section only points at
//! the full skill (`.claude/skills/standardoc/SKILL.md`); AGENTS.md stays in
//! every agent's context, so it is kept deliberately short.

const BEGIN: &str = "<!-- standardoc:begin";
const END: &str = "<!-- standardoc:end -->";

const MANAGED_SECTION: &str = "\
<!-- standardoc:begin (managed by `standardoc init` — edits between the markers are overwritten) -->
## Standardoc — code navigation

This workspace has a live Standardoc semantic index, served over MCP. Use it as
your FIRST step for any code task — locating symbols, callers, dependencies —
before falling back to raw file search. The full tool reference and 3-phase
protocol live in `.claude/skills/standardoc/SKILL.md`.
<!-- standardoc:end -->";

/// `NoOp` means the managed section already matches; `Written` carries the
/// full file contents to persist (create / replace-section / append).
pub(crate) enum MergeOutcome {
    NoOp,
    Written(String),
}

/// `raw` is the current `AGENTS.md` contents (`None` when absent).
pub(crate) fn merge_agents_md(raw: Option<&str>) -> MergeOutcome {
    let Some(existing) = raw else {
        return MergeOutcome::Written(format!("{MANAGED_SECTION}\n"));
    };

    if let Some((start, end)) = marker_span(existing) {
        if normalize(&existing[start..end]) == normalize(MANAGED_SECTION) {
            return MergeOutcome::NoOp;
        }
        let mut out = String::with_capacity(existing.len());
        out.push_str(&existing[..start]);
        out.push_str(MANAGED_SECTION);
        out.push_str(&existing[end..]);
        return MergeOutcome::Written(out);
    }

    // No managed block yet: append after the user's content, keeping exactly
    // one blank line between the existing text and our section.
    let sep = if existing.is_empty() || existing.ends_with("\n\n") {
        ""
    } else if existing.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    MergeOutcome::Written(format!("{existing}{sep}{MANAGED_SECTION}\n"))
}

/// Byte range of the managed block: from the `BEGIN` marker to the end of the
/// `END` marker. `None` when no managed block is present.
fn marker_span(s: &str) -> Option<(usize, usize)> {
    let start = s.find(BEGIN)?;
    let end = s[start..].find(END)? + start + END.len();
    Some((start, end))
}

fn normalize(s: &str) -> String {
    s.replace("\r\n", "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(outcome: &MergeOutcome) -> &str {
        match outcome {
            MergeOutcome::Written(s) => s,
            MergeOutcome::NoOp => panic!("expected Written"),
        }
    }

    #[test]
    fn creates_section_when_absent() {
        let out = merge_agents_md(None);
        let s = written(&out);
        assert!(s.contains(BEGIN));
        assert!(s.contains(END));
        assert!(s.contains("## Standardoc"));
        assert!(s.ends_with('\n'));
    }

    #[test]
    fn is_noop_when_section_matches() {
        let created = written(&merge_agents_md(None)).to_string();
        assert!(matches!(
            merge_agents_md(Some(&created)),
            MergeOutcome::NoOp
        ));
    }

    #[test]
    fn appends_after_user_content_without_markers() {
        let existing = "# My project\n\nSome agent notes.\n";
        let s = written(&merge_agents_md(Some(existing))).to_string();
        assert!(s.starts_with("# My project"));
        assert!(s.contains("Some agent notes."));
        assert!(s.contains(BEGIN));
        // Exactly one blank line between user content and the section.
        assert!(s.contains("Some agent notes.\n\n<!-- standardoc:begin"));
    }

    #[test]
    fn replaces_stale_managed_section_in_place() {
        let stale =
            "# Top\n\n<!-- standardoc:begin -->\nOLD BODY\n<!-- standardoc:end -->\n\n## Footer\n";
        let s = written(&merge_agents_md(Some(stale))).to_string();
        assert!(s.starts_with("# Top"));
        assert!(s.contains("## Footer"));
        assert!(!s.contains("OLD BODY"));
        assert!(s.contains("## Standardoc"));
        // Only one managed block remains.
        assert_eq!(s.matches(BEGIN).count(), 1);
    }

    #[test]
    fn idempotent_across_two_merges() {
        let first = written(&merge_agents_md(None)).to_string();
        assert!(matches!(merge_agents_md(Some(&first)), MergeOutcome::NoOp));
    }
}

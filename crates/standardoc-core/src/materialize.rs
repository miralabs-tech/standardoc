//! Convert virtual annotations into source-level `///` comments.
//!
//! `materialize` walks the index, picks blocks that have virtual annotations
//! synthesized by [`crate::virtual_annotation`] but no real `@doc` content,
//! formats them as language-appropriate doc-comments, and inserts them
//! above the symbol declaration. Unmodified files are left untouched.
//!
//! Two-step UX:
//! 1. Build a [`MaterializePlan`] (pure, no I/O) — the CLI prints it under
//!    `--dry-run` so the user can see exactly what would change.
//! 2. Call [`apply_to_disk`] to write the files when the user confirms.
//!
//! Matérialisation skips blocks where `origin != Inferred` so real `@doc`
//! authoring always wins. It also respects a confidence threshold so users
//! can stick to high-confidence templates first and revisit the rest later.

use crate::model::{BlockOrigin, DocBlock, VirtualConfidence};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Minimum confidence tier required for a virtual annotation to be eligible
/// for materialization. Default in CLI : `AtLeastMedium`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfidenceFilter {
    AtLeastLow,
    AtLeastMedium,
    AtLeastHigh,
}

impl ConfidenceFilter {
    const fn allows(self, c: VirtualConfidence) -> bool {
        match self {
            Self::AtLeastLow => true,
            Self::AtLeastMedium => matches!(c, VirtualConfidence::Medium | VirtualConfidence::High),
            Self::AtLeastHigh => matches!(c, VirtualConfidence::High),
        }
    }
}

/// Plan grouped by source file. Edits inside one file are sorted by
/// `line_start` descending so successive `Vec::insert` calls don't shift
/// the targets of later edits.
#[derive(Debug, Clone, Default)]
pub struct MaterializePlan {
    pub edits: BTreeMap<PathBuf, Vec<FileEdit>>,
}

#[derive(Debug, Clone)]
pub struct FileEdit {
    pub line_start: u32,
    pub key: String,
    pub confidence: VirtualConfidence,
    /// Comment lines as they should appear in the source, **without** the
    /// leading indentation of the target symbol — the writer applies that
    /// from the matched line.
    pub comment_lines: Vec<String>,
}

/// Build a plan from the current index. Pure (no I/O).
#[must_use]
pub fn plan(blocks: &BTreeMap<String, DocBlock>, filter: ConfidenceFilter) -> MaterializePlan {
    let mut edits: BTreeMap<PathBuf, Vec<FileEdit>> = BTreeMap::new();
    for block in blocks.values() {
        let Some(confidence) = block.virtual_confidence else {
            continue;
        };
        if !filter.allows(confidence) {
            continue;
        }
        if block.virtual_tags.is_empty() {
            continue;
        }
        // Real annotations always win — we only fill gaps for blocks the user
        // hasn't touched at all.
        if block.origin != BlockOrigin::Inferred {
            continue;
        }
        let Some(format) = pick_format(&block.meta.file_ext) else {
            continue;
        };
        let comment_lines = render(format, block);
        if comment_lines.is_empty() {
            continue;
        }
        edits
            .entry(block.meta.path.clone())
            .or_default()
            .push(FileEdit {
                line_start: block.meta.line_start,
                key: block.key.as_str().to_owned(),
                confidence,
                comment_lines,
            });
    }
    for v in edits.values_mut() {
        v.sort_by(|a, b| b.line_start.cmp(&a.line_start));
    }
    MaterializePlan { edits }
}

#[derive(Debug, Clone, Copy)]
enum DocFormat {
    /// Single-line doc comment with a fixed prefix (`///`, `---`, `## `).
    DocSingle(&'static str),
    /// Multi-line block comment (`/** … */` JSDoc style).
    DocMulti,
}

const fn pick_format(file_ext: &str) -> Option<DocFormat> {
    match file_ext.as_bytes() {
        b"rs" => Some(DocFormat::DocSingle("///")),
        b"lua" => Some(DocFormat::DocSingle("---")),
        b"ts" | b"tsx" | b"js" | b"jsx" | b"mjs" | b"cjs" => Some(DocFormat::DocMulti),
        // Python docstrings live INSIDE the function body — needs different
        // placement logic. Skip for now; users can write them manually.
        _ => None,
    }
}

fn render(format: DocFormat, block: &DocBlock) -> Vec<String> {
    let body = render_body_lines(block);
    if body.is_empty() {
        return Vec::new();
    }
    match format {
        DocFormat::DocSingle(prefix) => body
            .into_iter()
            .map(|l| {
                if l.is_empty() {
                    prefix.to_owned()
                } else {
                    format!("{prefix} {l}")
                }
            })
            .collect(),
        DocFormat::DocMulti => {
            let mut out = vec!["/**".to_owned()];
            for l in body {
                out.push(if l.is_empty() {
                    " *".to_owned()
                } else {
                    format!(" * {l}")
                });
            }
            out.push(" */".to_owned());
            out
        }
    }
}

fn render_body_lines(block: &DocBlock) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();

    if let Some(desc) = block
        .virtual_tags
        .get("description")
        .and_then(|v| v.first())
        .and_then(|t| t.first())
    {
        let trimmed = desc.trim();
        if !trimmed.is_empty() {
            lines.push(trimmed.to_owned());
        }
    }

    if let Some(params) = block.virtual_tags.get("param") {
        if !params.is_empty() && !lines.is_empty() {
            lines.push(String::new());
        }
        for fields in params {
            let parts: Vec<&str> = fields.iter().map(String::as_str).collect();
            if parts.is_empty() {
                continue;
            }
            lines.push(format!("@param {}", parts.join(" ")));
        }
    }

    if let Some(returns) = block.virtual_tags.get("returns") {
        for fields in returns {
            let parts: Vec<&str> = fields.iter().map(String::as_str).collect();
            if parts.is_empty() {
                continue;
            }
            lines.push(format!("@returns {}", parts.join(" ")));
        }
    }

    lines
}

/// Apply the plan to disk. Writes are best-effort per file: an I/O error
/// on one file aborts and returns the count of successful writes so far.
///
/// `workspace_root` is needed because plan paths are workspace-relative
/// (consistent with `DocMeta.path`).
///
/// Returns `(files_written, edits_applied)`.
pub fn apply_to_disk(
    plan: &MaterializePlan,
    workspace_root: &Path,
) -> Result<(usize, usize), std::io::Error> {
    let mut files_written = 0;
    let mut edits_applied = 0;
    for (rel_path, edits) in &plan.edits {
        let abs = workspace_root.join(rel_path);
        let original = std::fs::read_to_string(&abs)?;
        let trailing_newline = original.ends_with('\n');
        let mut lines: Vec<String> = original.lines().map(str::to_owned).collect();

        for edit in edits {
            let target_idx = (edit.line_start.saturating_sub(1)) as usize;
            if target_idx >= lines.len() {
                continue;
            }
            let indent: String = lines[target_idx]
                .chars()
                .take_while(|c| c.is_whitespace())
                .collect();
            for (i, raw) in edit.comment_lines.iter().enumerate() {
                let line = if raw.is_empty() {
                    indent.clone()
                } else {
                    format!("{indent}{raw}")
                };
                lines.insert(target_idx + i, line);
            }
            edits_applied += 1;
        }

        let mut output = lines.join("\n");
        if trailing_newline {
            output.push('\n');
        }
        std::fs::write(&abs, output)?;
        files_written += 1;
    }
    Ok((files_written, edits_applied))
}

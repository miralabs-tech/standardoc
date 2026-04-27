//! Generators for the standard agent-facing documentation formats:
//!
//! - **`llms.txt`** — Jeremy Howard's proposed standard. A short,
//!   browseable Markdown index pointing at the detailed content. Designed
//!   for LLMs that follow links.
//! - **`llms-full.txt`** — same content as `llms.txt` but inlined: full
//!   signatures + descriptions in one file. For LLMs that ingest in bulk.
//! - **`skill.md`** — Claude Code skill format with YAML front-matter.
//!   Describes the project as a "skill" an agent can acquire.
//!
//! All three generators read from the canonical `BTreeMap<String, DocBlock>`
//! that the rest of the pipeline produces. They never hit the filesystem
//! themselves — the caller writes the returned string wherever it wants.

mod llms_full;
mod llms_txt;
mod openapi;
mod skill_md;

pub use llms_full::emit_llms_full;
pub use llms_txt::emit_llms_txt;
pub use openapi::{emit_openapi, OpenApiOptions};
pub use skill_md::emit_skill_md;

/// Tunables shared by the three generators. None of these are required —
/// reasonable defaults are inferred when fields are `None`.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmitOptions {
    /// Project name. Falls back to `"Project"` if `None`.
    #[serde(default)]
    pub project_name: Option<String>,
    /// One-line description shown right after the project header.
    #[serde(default)]
    pub tagline: Option<String>,
    /// Optional URL prefix for hyperlinks in `llms.txt`. If `None`, the
    /// generator falls back to relative links to source files.
    #[serde(default)]
    pub link_base: Option<String>,
}

impl EmitOptions {
    pub fn project_name_or_default(&self) -> &str {
        self.project_name.as_deref().unwrap_or("Project")
    }
}

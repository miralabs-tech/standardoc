//! Stage 3d — per-project metadata for polyglot monorepos.
//!
//! A single workspace usually contains multiple distinct projects
//! (a Rust crate, a Bun-powered VSCode extension, a Deno script, …).
//! `ProjectInfo` lets the index attribute every file to the project
//! that owns it, so consumers can scope queries (`list_symbols in
//! project X`), resolve imports against the right manifest, and
//! display the graph layered by project ownership.
//!
//! Detection is delegated to the `standarbuild-detect` crate (consumed
//! in `standardoc-core`); this module only carries the IR shape.

use serde::{Deserialize, Serialize};

/// Ecosystem a project belongs to. Mirrors `standarbuild_detect::ProjectKind`
/// with one addition: [`ProjectKind::Custom`] for UST extensibility — future
/// providers (WGSL, Move, Solidity, …) can register custom kinds without
/// recompiling the IR.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "kind", content = "tag")]
pub enum ProjectKind {
    Rust,
    Node,
    Bun,
    Deno,
    Python,
    Lua,
    C,
    Cpp,
    Unknown,
    Custom(String),
}

impl ProjectKind {
    /// Canonical lowercase slug stored in the SQLite `projects.kind`
    /// column. `Custom` variants render as `"custom:<tag>"` so they
    /// survive round-trip through the DB.
    pub fn as_str(&self) -> std::borrow::Cow<'_, str> {
        match self {
            Self::Rust => std::borrow::Cow::Borrowed("rust"),
            Self::Node => std::borrow::Cow::Borrowed("node"),
            Self::Bun => std::borrow::Cow::Borrowed("bun"),
            Self::Deno => std::borrow::Cow::Borrowed("deno"),
            Self::Python => std::borrow::Cow::Borrowed("python"),
            Self::Lua => std::borrow::Cow::Borrowed("lua"),
            Self::C => std::borrow::Cow::Borrowed("c"),
            Self::Cpp => std::borrow::Cow::Borrowed("cpp"),
            Self::Unknown => std::borrow::Cow::Borrowed("unknown"),
            Self::Custom(tag) => std::borrow::Cow::Owned(format!("custom:{tag}")),
        }
    }

    /// Inverse of [`Self::as_str`]. Accepts the lowercase slugs above
    /// plus `"c++"` as a Cpp alias.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "rust" => Some(Self::Rust),
            "node" => Some(Self::Node),
            "bun" => Some(Self::Bun),
            "deno" => Some(Self::Deno),
            "python" => Some(Self::Python),
            "lua" => Some(Self::Lua),
            "c" => Some(Self::C),
            "cpp" | "c++" => Some(Self::Cpp),
            "unknown" => Some(Self::Unknown),
            other => other
                .strip_prefix("custom:")
                .map(|tag| Self::Custom(tag.to_string())),
        }
    }
}

impl std::fmt::Display for ProjectKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.as_str())
    }
}

/// Per-project metadata persisted in the `projects` table. The
/// `project_id` is assigned by SQLite (`INTEGER PRIMARY KEY`); zero
/// is a valid placeholder for not-yet-inserted rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub project_id: u32,
    /// Human-readable name. Derived from the manifest (`Cargo.toml`'s
    /// `[package].name`, `package.json`'s `name`) when available, else
    /// the directory basename. Deduplicated by the detector when the
    /// same label appears twice in a workspace.
    pub label: String,
    /// Ecosystem the project belongs to.
    pub kind: ProjectKind,
    /// Canonical absolute path to the project root (the directory
    /// containing the manifest).
    pub root_path: String,
    /// Path relative to the workspace root, POSIX-style. `"."` for the
    /// workspace-root project, `"./crates/foo"` for a sub-project.
    pub rel_path: String,
}

#[cfg(test)]
mod tests;

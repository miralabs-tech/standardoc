//! Bug E-3 follow-up — `.sxd` workspace config loader.
//!
//! Parses `<workspace>/standardoc.sxd` (DSL via `standarx-dsl`) into a
//! typed [`SxdConfig`]. Lower stage validates the schema (block kinds,
//! required fields, no interpolation in plain strings) and rejects
//! malformed configs early with an [`SxdConfigError`].
//!
//! Schema v0.1:
//!   * `version "<semver>"` — required
//!   * `ignore { patterns ```...``` }` — optional
//!   * `project "<slug>" { label "..." path "..." | paths [...] }`
//!     — optional, repeatable. When at least one `project` block is
//!     present, mechanical detection is short-circuited and only the
//!     declared paths are indexed.
//!   * `group "<slug>" { label "..." members [...] }` — optional, repeatable

use std::fs;
use std::io;
use std::path::Path;

use serde::Serialize;
use standarx_dsl::ast::{Block, Expr, File, StmtNode, StringLit, StringPart};
use standarx_dsl::diag::Spanned;
use standarx_dsl::{Diag, Stmt};

/// Filename of the workspace config (sibling of `Cargo.toml`).
pub const SXD_CONFIG_FILENAME: &str = "standardoc.sxd";

/// Legacy gitignore-syntax filename, kept for back-compat migration only.
/// `ensure_sxd_seed_at` reads this and folds it into `standardoc.sxd`'s
/// `ignore { patterns ... }` block.
const LEGACY_STDIGNORE_FILENAME: &str = ".stdignore";

/// Default `.sxd` content when seeding a fresh workspace. Carries the
/// same defaults as the legacy `.stdignore` seed but inside an
/// `ignore { patterns ```...``` }` block. No `project` blocks — by
/// default standardoc keeps the mechanical detection.
const SXD_SEED_TEMPLATE: &str = "\
# Standardoc workspace config.
# Edit freely — re-running standardoc won't overwrite without --force.
# See https://standardoc.miralabs.tech/docs/sxd for the full schema.

version \"0.1.0\"

ignore {
  patterns ```
.git/
node_modules/
target/
dist/
build/
.old/
*-old/
test-export/
```
}
";

#[derive(Debug, thiserror::Error)]
pub enum SxdConfigError {
    #[error("standardoc.sxd I/O: {0}")]
    Io(#[from] io::Error),
    #[error("standardoc.sxd parse error: {0:?}")]
    Parse(Box<Diag>),
    #[error("standardoc.sxd schema error: {0}")]
    Schema(String),
}

impl From<Diag> for SxdConfigError {
    fn from(d: Diag) -> Self {
        Self::Parse(Box::new(d))
    }
}

/// Typed workspace config — output of [`parse_sxd_source`] / [`load_workspace_config`].
///
/// When `projects` is non-empty, the discovery pipeline short-circuits
/// the mechanical cargo/npm/lua detection and indexes ONLY the listed
/// paths under each named project. Empty `projects` keeps the legacy
/// behaviour.
#[derive(Debug, Clone, Default)]
pub struct SxdConfig {
    pub version: Option<String>,
    pub ignore: Option<IgnoreBlock>,
    pub projects: Vec<ProjectBlock>,
    pub groups: Vec<GroupBlock>,
}

/// `ignore { patterns ```...``` }` block — multi-line gitignore-syntax
/// text exactly as written in the source.
#[derive(Debug, Clone, Default)]
pub struct IgnoreBlock {
    pub patterns: String,
}

/// `project "<slug>" { label "..." path "..." | paths [...] }` block.
///
/// Repeatable. `path` (singular) is a shorthand for a single-entry
/// `paths` array. `paths` is always a workspace-relative directory.
#[derive(Debug, Clone)]
pub struct ProjectBlock {
    pub slug: String,
    pub label: Option<String>,
    pub paths: Vec<String>,
}

/// `group "<slug>" { label "..." members [...] }` block.
///
/// Optional layer ABOVE projects — bundles multiple projects under one
/// label (e.g. one "platform" group with several projects inside).
/// Members must reference declared project slugs.
#[derive(Debug, Clone, Serialize)]
pub struct GroupBlock {
    pub slug: String,
    pub label: Option<String>,
    pub members: Vec<String>,
}

/// Bug E-3 follow-up P2 — replacement for the legacy
/// `ensure_stdignore_seed_at`. Behaviour matrix:
///
///   * `standardoc.sxd` exists → no-op (user-authored config wins, even
///     when `.stdignore` is also present — they may have kept it for
///     other tooling).
///   * `.stdignore` exists, no `.sxd` → migrate : read `.stdignore`,
///     wrap its content in an `ignore { patterns ```...``` }` block,
///     write `standardoc.sxd`, rename `.stdignore` → `.stdignore.bak`.
///   * Neither exists → write the default `.sxd` seed template.
///
/// Idempotent: re-running on the same state produces no changes.
pub fn ensure_sxd_seed_at(workspace_root: &Path) -> io::Result<()> {
    let sxd_path = workspace_root.join(SXD_CONFIG_FILENAME);
    if sxd_path.exists() {
        return Ok(());
    }
    let stdignore_path = workspace_root.join(LEGACY_STDIGNORE_FILENAME);
    if stdignore_path.is_file() {
        let legacy = fs::read_to_string(&stdignore_path)?;
        let sxd = render_sxd_from_stdignore(&legacy);
        fs::write(&sxd_path, sxd)?;
        let backup = workspace_root.join(".stdignore.bak");
        // Best-effort backup: drop existing .bak so re-runs don't fail.
        let _ = fs::remove_file(&backup);
        fs::rename(&stdignore_path, &backup)?;
        return Ok(());
    }
    fs::write(&sxd_path, SXD_SEED_TEMPLATE)
}

/// Build a `.sxd` source string from a legacy `.stdignore` content,
/// preserving the user's patterns verbatim inside an `ignore { patterns
/// ```...``` }` block. Adds a header noting the auto-migration so the
/// user can trace where the content came from.
fn render_sxd_from_stdignore(legacy: &str) -> String {
    let trimmed = legacy.trim_end_matches('\n');
    format!(
        "\
# Standardoc workspace config.
# Auto-migrated from .stdignore on first cold-start (backup at .stdignore.bak).
# Edit freely — re-running standardoc won't overwrite without --force.

version \"0.1.0\"

ignore {{
  patterns ```
{trimmed}
```
}}
"
    )
}

/// Load `standardoc.sxd` from a workspace root. Returns `Ok(None)` when
/// the file is absent.
pub fn load_workspace_config(workspace_root: &Path) -> Result<Option<SxdConfig>, SxdConfigError> {
    let path = workspace_root.join(SXD_CONFIG_FILENAME);
    if !path.exists() {
        return Ok(None);
    }
    let source = fs::read_to_string(&path)?;
    let cfg = parse_sxd_source(&source)?;
    Ok(Some(cfg))
}

/// Parse + lower a `.sxd` source string into a typed [`SxdConfig`].
pub fn parse_sxd_source(source: &str) -> Result<SxdConfig, SxdConfigError> {
    let file = standarx_dsl::parse(source)?;
    lower_file(&file)
}

fn lower_file(file: &File) -> Result<SxdConfig, SxdConfigError> {
    let mut out = SxdConfig::default();
    for stmt in &file.stmts {
        lower_stmt(stmt, &mut out)?;
    }
    Ok(out)
}

fn lower_stmt(stmt: &StmtNode, out: &mut SxdConfig) -> Result<(), SxdConfigError> {
    match &stmt.node {
        Stmt::Assign(a) => {
            let key = a.key.node.as_str();
            if key == "version" {
                out.version = Some(expect_string(&a.value, "version")?);
                Ok(())
            } else {
                Err(SxdConfigError::Schema(format!(
                    "unknown top-level assign `{key}` (only `version` accepted at top level)"
                )))
            }
        }
        Stmt::Block(b) => {
            let kind = b.kind.node.as_str();
            match kind {
                "ignore" => {
                    out.ignore = Some(lower_ignore(b)?);
                    Ok(())
                }
                "project" => {
                    out.projects.push(lower_project(b)?);
                    Ok(())
                }
                "group" => {
                    out.groups.push(lower_group(b)?);
                    Ok(())
                }
                _ => Err(SxdConfigError::Schema(format!(
                    "unknown top-level block `{kind}` \
                     (expected `ignore`, `project`, or `group`)"
                ))),
            }
        }
    }
}

fn lower_ignore(b: &Block) -> Result<IgnoreBlock, SxdConfigError> {
    let mut patterns = String::new();
    for stmt in &b.stmts {
        let Stmt::Assign(a) = &stmt.node else {
            return Err(SxdConfigError::Schema(
                "`ignore` block only accepts `patterns = ...` assignment, not nested blocks".into(),
            ));
        };
        let key = a.key.node.as_str();
        if key != "patterns" {
            return Err(SxdConfigError::Schema(format!(
                "unknown field `{key}` inside `ignore` (only `patterns` accepted)"
            )));
        }
        patterns = expect_string(&a.value, "ignore.patterns")?;
    }
    Ok(IgnoreBlock { patterns })
}

fn lower_project(b: &Block) -> Result<ProjectBlock, SxdConfigError> {
    let slug = b.label.as_ref().map(|s| s.node.clone()).ok_or_else(|| {
        SxdConfigError::Schema(
            "`project` block requires a string slug, e.g. `project \"standardoc\" { ... }`".into(),
        )
    })?;
    let mut label = None;
    let mut path_single: Option<String> = None;
    let mut paths_multi: Option<Vec<String>> = None;
    for stmt in &b.stmts {
        let Stmt::Assign(a) = &stmt.node else {
            return Err(SxdConfigError::Schema(format!(
                "`project \"{slug}\"` block only accepts assignments (`label`, `path`, `paths`)"
            )));
        };
        let key = a.key.node.as_str();
        match key {
            "label" => label = Some(expect_string(&a.value, "project.label")?),
            "path" => path_single = Some(expect_string(&a.value, "project.path")?),
            "paths" => paths_multi = Some(expect_string_list(&a.value, "project.paths")?),
            other => {
                return Err(SxdConfigError::Schema(format!(
                    "unknown field `{other}` inside `project \"{slug}\"` \
                     (expected `label`, `path`, or `paths`)"
                )));
            }
        }
    }
    let paths = match (path_single, paths_multi) {
        (Some(_), Some(_)) => {
            return Err(SxdConfigError::Schema(format!(
                "`project \"{slug}\"` declares both `path` and `paths` — pick one"
            )));
        }
        (Some(p), None) => vec![p],
        (None, Some(ps)) => ps,
        (None, None) => {
            return Err(SxdConfigError::Schema(format!(
                "`project \"{slug}\"` must declare at least one `path \"...\"` or `paths [...]`"
            )));
        }
    };
    if paths.is_empty() {
        return Err(SxdConfigError::Schema(format!(
            "`project \"{slug}\"` has an empty `paths` array"
        )));
    }
    Ok(ProjectBlock { slug, label, paths })
}

fn lower_group(b: &Block) -> Result<GroupBlock, SxdConfigError> {
    let slug = b.label.as_ref().map(|s| s.node.clone()).ok_or_else(|| {
        SxdConfigError::Schema(
            "`group` block requires a string label, e.g. `group \"standardoc\" { ... }`".into(),
        )
    })?;
    let mut label = None;
    let mut members = Vec::new();
    for stmt in &b.stmts {
        let Stmt::Assign(a) = &stmt.node else {
            return Err(SxdConfigError::Schema(format!(
                "`group \"{slug}\"` block only accepts assignments (`label`, `members`)"
            )));
        };
        let key = a.key.node.as_str();
        match key {
            "label" => label = Some(expect_string(&a.value, "group.label")?),
            "members" => members = expect_string_list(&a.value, "group.members")?,
            other => {
                return Err(SxdConfigError::Schema(format!(
                    "unknown field `{other}` inside `group \"{slug}\"` \
                     (expected `label` or `members`)"
                )));
            }
        }
    }
    Ok(GroupBlock {
        slug,
        label,
        members,
    })
}

/// Coerce a single string literal (no interpolation) from an `Expr`.
fn expect_string(value: &Spanned<Expr>, context: &str) -> Result<String, SxdConfigError> {
    let Expr::String(lit) = &value.node else {
        return Err(SxdConfigError::Schema(format!(
            "expected a string value for `{context}`"
        )));
    };
    string_lit_to_plain(lit, context)
}

fn expect_string_list(value: &Spanned<Expr>, context: &str) -> Result<Vec<String>, SxdConfigError> {
    let Expr::List(items) = &value.node else {
        return Err(SxdConfigError::Schema(format!(
            "expected an array for `{context}`, e.g. `[\"a\" \"b\"]`"
        )));
    };
    items
        .iter()
        .map(|item| {
            let Expr::String(lit) = &item.node else {
                return Err(SxdConfigError::Schema(format!(
                    "expected a string element inside `{context}`"
                )));
            };
            string_lit_to_plain(lit, context)
        })
        .collect()
}

/// Flatten a `StringLit` into a plain `String`. Errors on interpolation
/// — standardoc.sxd v0.1 doesn't support `${env.X}` style placeholders
/// (that's a `.sxb` task-runner concern).
fn string_lit_to_plain(lit: &StringLit, context: &str) -> Result<String, SxdConfigError> {
    let mut out = String::new();
    for part in &lit.parts {
        match part {
            StringPart::Lit(s) => out.push_str(s),
            StringPart::Interp(_) => {
                return Err(SxdConfigError::Schema(format!(
                    "string interpolation (`${{...}}`) is not supported in `{context}` \
                     — standardoc.sxd v0.1 expects plain strings"
                )));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;

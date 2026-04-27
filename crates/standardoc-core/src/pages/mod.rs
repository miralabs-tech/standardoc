//! Narrative pages — user-curated `.md` files that complement the
//! auto-discovered code symbols.
//!
//! ## Mental model
//!
//! All pages persist as `.md` files on disk in `.standardoc/pages/` —
//! git-friendly, source of truth, editable through any tool that writes to
//! the filesystem (code editor, script, REST API client, etc.). The core
//! is read-only on the pages tree: it scans, parses frontmatter, and exposes
//! typed `DocPage` values to downstream consumers (validator rules,
//! server state, SSG export). It never creates or modifies pages — that
//! concern belongs to the host (CLI, daemon, web frontend).
//!
//! Two content levels for a given slug:
//!
//! 1. **On-disk page** in `.standardoc/pages/<slug>.md` -> absolute source of
//!    truth. User edited it, so consumers serve it as-is (with DSL eval).
//! 2. **Auto-generated page** from index -> if no file covers the slug but it
//!    maps to a known symbol (`reference/<key>`) or root module
//!    (`modules/<name>`), consumers can generate a DSL template on the fly.
//!
//! ## Slug convention
//!
//! Slug = path relative to pages directory, without extension:
//! - `.standardoc/pages/index.md`               → slug `""` (home)
//! - `.standardoc/pages/getting-started.md`     → slug `"getting-started"`
//! - `.standardoc/pages/reference/foo.bar.md`   → slug `"reference/foo.bar"`
//!
//! Numeric prefix `01-`, `02-`, etc. is stripped from slug and used for order
//! — standard Jekyll/Hugo/Astro convention.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

mod scanner;

pub use scanner::scan_pages;

/// Directory (relative to workspace root) where narrative pages live.
/// Conventional path — could be configurable via `Config.pages.root` later,
/// but currently hardcoded.
pub const PAGES_DIR: &str = ".standardoc/pages";

/// Narrative page loaded from disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocPage {
    /// URL slug — relative to pages directory, no extension, no numeric
    /// order prefix.
    pub slug: String,
    /// Path relatif au workspace root.
    pub path: PathBuf,
    /// Display title. Priority source:
    /// 1. `frontmatter.title`
    /// 2. first `# H1` in body
    /// 3. filename (without extension or `NN-` prefix)
    pub title: String,
    /// Order in its group (section). Priority source:
    /// 1. `frontmatter.order`
    /// 2. `NN-` prefix in filename
    /// 3. `None` (sorted alphabetically after ordered items)
    pub order: Option<i32>,
    /// Hierarchy: path segments without filename. Ex `reference/auth` gives
    /// `["reference"]`.
    pub section: Vec<String>,
    /// Parsed frontmatter (YAML). Kept raw so frontend can pass custom
    /// metadata (icon, color, etc.) without requiring schema awareness.
    #[serde(default)]
    pub frontmatter: BTreeMap<String, serde_json::Value>,
    /// Raw markdown body, after frontmatter strip, before DSL eval.
    pub raw_body: String,
    /// `Mdx` if extension is `.mdx` — client compiles at runtime.
    /// `Md` otherwise — pulldown-cmark + syntect on server side.
    pub kind: PageKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PageKind {
    Md,
    Mdx,
}

impl PageKind {
    /// `true` when rendering should produce server HTML (Md), `false`
    /// when compilation is left to client (Mdx).
    pub const fn renders_to_html_server_side(self) -> bool {
        matches!(self, Self::Md)
    }
}

/// Build slug from file path relative to pages directory.
///
/// - Strip l'extension `.md` ou `.mdx`
/// - Remove `NN-` prefix from each segment (see `strip_order_prefix`)
/// - `index.md` at any level -> that level (parent slug)
///
/// Exemples :
/// - `index.md`                           → `""`  (home)
/// - `01-getting-started.md`              → `"getting-started"`
/// - `reference/02-auth.md`               → `"reference/auth"`
/// - `reference/index.md`                 → `"reference"`
pub fn slug_from_relative(rel: &Path) -> String {
    let with_no_ext = rel.with_extension("");
    let segments: Vec<String> = with_no_ext
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .map(strip_order_prefix)
        .map(ToOwned::to_owned)
        .collect();

    // Trailing `index` = parent slug (or "" if parent is empty).
    if let Some(last) = segments.last() {
        if last == "index" {
            return segments[..segments.len() - 1].join("/");
        }
    }
    segments.join("/")
}

/// Remove numeric prefix like `01-`, `02_`, or `1_` from a path segment.
/// If prefix does not match pattern, return segment unchanged.
pub fn strip_order_prefix(segment: &str) -> &str {
    let bytes = segment.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 || i == bytes.len() {
        return segment;
    }
    let sep = bytes[i];
    if sep == b'-' || sep == b'_' {
        &segment[i + 1..]
    } else {
        segment
    }
}

/// Extract order prefix `NN-` or `NN_` from a segment, when present.
pub fn order_from_prefix(segment: &str) -> Option<i32> {
    let bytes = segment.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 || i == bytes.len() {
        return None;
    }
    let sep = bytes[i];
    if sep == b'-' || sep == b'_' {
        segment[..i].parse::<i32>().ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_index_at_root() {
        assert_eq!(slug_from_relative(Path::new("index.md")), "");
    }

    #[test]
    fn slug_strips_order_prefix() {
        assert_eq!(
            slug_from_relative(Path::new("01-getting-started.md")),
            "getting-started"
        );
    }

    #[test]
    fn slug_nested() {
        assert_eq!(
            slug_from_relative(Path::new("reference/02-auth.mdx")),
            "reference/auth"
        );
    }

    #[test]
    fn slug_index_in_section() {
        assert_eq!(
            slug_from_relative(Path::new("reference/index.md")),
            "reference"
        );
    }

    #[test]
    fn order_prefix_extraction() {
        assert_eq!(order_from_prefix("01-foo"), Some(1));
        assert_eq!(order_from_prefix("12_bar"), Some(12));
        assert_eq!(order_from_prefix("foo"), None);
        assert_eq!(order_from_prefix("01"), None);
        assert_eq!(order_from_prefix("01.foo"), None);
    }

    #[test]
    fn strip_keeps_unprefixed() {
        assert_eq!(strip_order_prefix("foo"), "foo");
        assert_eq!(strip_order_prefix("01-foo"), "foo");
        assert_eq!(strip_order_prefix("123_bar"), "bar");
    }
}

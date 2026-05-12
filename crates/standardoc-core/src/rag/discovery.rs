//! Prose source discovery for the RAG layer.
//!
//! Rules (locked design — Q6) :
//!
//! - **Convention paths** (auto-included) :
//!     * Workspace-root `README.md`
//!     * Every `.md` under `docs/**`
//! - **Frontmatter opt-in** : any other `.md` whose YAML frontmatter
//!   declares `standardoc: rag`. Recognised even when buried under
//!   arbitrary sub-paths.
//! - **Opt-out** : a file matched by a convention path can disable
//!   indexing via `standardoc: false` (or equivalently absent + no rag
//!   directive).
//! - `.stdignore` exclusions are honoured throughout via [`ScanFilters`].

use std::io::{BufRead, BufReader};
use std::path::Path;

use walkdir::WalkDir;

use crate::pipeline::ScanFilters;

/// First N bytes scanned for a frontmatter block. Frontmatter that is
/// not visible in the file's opening 4 KiB is treated as absent.
const FRONTMATTER_SCAN_BYTES: usize = 4 * 1024;

/// Value of the `standardoc:` directive in a markdown file's frontmatter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontmatterDirective {
    /// `standardoc: rag` — the author explicitly opted this file in.
    Rag,
    /// `standardoc: false` — the author explicitly opted this file out
    /// (overrides convention-path inclusion).
    Disabled,
    /// No directive present. Caller decides based on convention paths.
    Absent,
}

/// Walks `workspace_root`, applies `.stdignore` and convention rules,
/// returns workspace-relative `.md` paths (forward-slash separated) to
/// hand to the RAG pipeline.
///
/// The returned paths are deterministic but unordered ; callers needing
/// stable order should sort. Filesystem errors mid-walk are skipped
/// (the walker logs nothing — same policy as `cold_start::run`).
pub fn discover_prose_sources(workspace_root: &Path, filters: &ScanFilters) -> Vec<String> {
    let mut out = Vec::new();
    let walker = WalkDir::new(workspace_root)
        .follow_links(false)
        .same_file_system(true);

    for entry in walker.into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("md"))
        {
            continue;
        }
        let rel = match path.strip_prefix(workspace_root) {
            Ok(p) => p.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };
        if filters.is_skipped(&rel) {
            continue;
        }
        let directive = read_frontmatter_directive(path).unwrap_or(FrontmatterDirective::Absent);
        if matches!(directive, FrontmatterDirective::Disabled) {
            continue;
        }
        let convention_match = is_convention_path(&rel);
        let opted_in = matches!(directive, FrontmatterDirective::Rag);
        if convention_match || opted_in {
            out.push(rel);
        }
    }
    out
}

/// `README.md` at any depth (workspace root + sub-package roots) plus
/// any `docs/**/*.md` qualify as convention paths. Comparison is
/// case-sensitive (matches the actual on-disk path).
///
/// Sub-package README inclusion is intentional : monorepo workspaces
/// commonly keep the user-facing prose next to each crate / package
/// (e.g. `standardoc/README.md`, `frontend/README.md`). Files the user
/// doesn't want indexed can opt out with `--- standardoc: false ---`
/// in their frontmatter ; the `.stdignore` exclusion list also runs
/// before the convention check.
pub fn is_convention_path(rel_path: &str) -> bool {
    if rel_path == "README.md" {
        return true;
    }
    if rel_path.ends_with("/README.md") || rel_path.ends_with("\\README.md") {
        return true;
    }
    rel_path.starts_with("docs/") || rel_path.starts_with("docs\\")
}

/// Reads the opening of `path` and returns the `standardoc:` directive
/// found in its YAML frontmatter. Returns `Ok(Absent)` when the file
/// has no frontmatter or no recognised directive ; returns `Err` only
/// when the file cannot be opened.
pub fn read_frontmatter_directive(path: &Path) -> std::io::Result<FrontmatterDirective> {
    let file = std::fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut head = String::new();
    let mut total = 0usize;
    let mut buf = String::new();
    let mut first_line = true;
    while total < FRONTMATTER_SCAN_BYTES {
        buf.clear();
        let n = reader.read_line(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n;
        if first_line {
            first_line = false;
            if buf.trim_end_matches(['\r', '\n']) != "---" {
                return Ok(FrontmatterDirective::Absent);
            }
            continue;
        }
        let trimmed = buf.trim_end_matches(['\r', '\n']);
        if trimmed == "---" {
            break;
        }
        head.push_str(&buf);
    }
    Ok(parse_directive(&head))
}

fn parse_directive(frontmatter_body: &str) -> FrontmatterDirective {
    for line in frontmatter_body.lines() {
        let Some(rest) = line.trim_start().strip_prefix("standardoc:") else {
            continue;
        };
        let value = rest.trim().trim_matches('"').trim_matches('\'').to_ascii_lowercase();
        return match value.as_str() {
            "rag" | "true" => FrontmatterDirective::Rag,
            "false" | "off" | "no" => FrontmatterDirective::Disabled,
            _ => FrontmatterDirective::Absent,
        };
    }
    FrontmatterDirective::Absent
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::GitignoreStack;

    fn write(root: &Path, rel: &str, content: &str) -> std::path::PathBuf {
        let full = root.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&full, content).unwrap();
        full
    }

    fn fresh_filters(root: &Path) -> ScanFilters {
        ScanFilters::from_stack(GitignoreStack::build(root))
    }

    #[test]
    fn is_convention_path_matches_root_readme_and_docs() {
        assert!(is_convention_path("README.md"));
        assert!(is_convention_path("docs/architecture.md"));
        assert!(is_convention_path("docs/sub/nested.md"));
        assert!(!is_convention_path("notes/random.md"));
        assert!(!is_convention_path("src/lib.rs"));
    }

    #[test]
    fn is_convention_path_picks_up_sub_package_readmes() {
        assert!(is_convention_path("standardoc/README.md"));
        assert!(is_convention_path("frontend/README.md"));
        assert!(is_convention_path("crates/foo/README.md"));
        // Backslash path (Windows-style) also recognised.
        assert!(is_convention_path("standardoc\\README.md"));
        // Other markdowns at the same depth are NOT auto-included.
        assert!(!is_convention_path("standardoc/CHANGELOG.md"));
        assert!(!is_convention_path("standardoc/notes.md"));
    }

    #[test]
    fn parse_directive_recognises_rag_and_false() {
        assert_eq!(parse_directive("standardoc: rag\n"), FrontmatterDirective::Rag);
        assert_eq!(parse_directive("standardoc: \"rag\"\n"), FrontmatterDirective::Rag);
        assert_eq!(parse_directive("standardoc: true\n"), FrontmatterDirective::Rag);
        assert_eq!(parse_directive("standardoc: false\n"), FrontmatterDirective::Disabled);
        assert_eq!(parse_directive("standardoc: no\n"), FrontmatterDirective::Disabled);
        assert_eq!(parse_directive("standardoc: bogus\n"), FrontmatterDirective::Absent);
        assert_eq!(parse_directive("title: hi\n"), FrontmatterDirective::Absent);
    }

    #[test]
    fn read_frontmatter_directive_handles_files_without_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "x.md", "# Hello\n\njust prose.\n");
        let d = read_frontmatter_directive(&p).unwrap();
        assert_eq!(d, FrontmatterDirective::Absent);
    }

    #[test]
    fn read_frontmatter_directive_extracts_rag_opt_in() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(
            dir.path(),
            "notes/x.md",
            "---\ntitle: foo\nstandardoc: rag\n---\nbody\n",
        );
        let d = read_frontmatter_directive(&p).unwrap();
        assert_eq!(d, FrontmatterDirective::Rag);
    }

    #[test]
    fn discover_picks_up_readme_and_docs_by_convention() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "README.md", "# r\n");
        write(root, "docs/a.md", "# a\n");
        write(root, "docs/sub/b.md", "# b\n");
        write(root, "notes/random.md", "# n\n");
        let filters = fresh_filters(root);
        let mut found = discover_prose_sources(root, &filters);
        found.sort();
        assert_eq!(found, vec!["README.md", "docs/a.md", "docs/sub/b.md"]);
    }

    #[test]
    fn discover_picks_up_frontmatter_opt_in_anywhere() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "notes/x.md", "---\nstandardoc: rag\n---\nbody\n");
        write(root, "notes/y.md", "no frontmatter\n");
        let filters = fresh_filters(root);
        let found = discover_prose_sources(root, &filters);
        assert!(found.contains(&"notes/x.md".to_string()));
        assert!(!found.contains(&"notes/y.md".to_string()));
    }

    #[test]
    fn discover_honours_disable_override_on_convention_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "docs/disabled.md",
            "---\nstandardoc: false\n---\nshould be skipped\n",
        );
        write(root, "docs/enabled.md", "# e\n");
        let filters = fresh_filters(root);
        let found = discover_prose_sources(root, &filters);
        assert!(found.contains(&"docs/enabled.md".to_string()));
        assert!(!found.contains(&"docs/disabled.md".to_string()));
    }

    #[test]
    fn discover_honours_stdignore_exclusions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".stdignore"), "docs/private/**\n").unwrap();
        write(root, "docs/public.md", "# p\n");
        write(root, "docs/private/secret.md", "# s\n");
        let filters = fresh_filters(root);
        let found = discover_prose_sources(root, &filters);
        assert!(found.contains(&"docs/public.md".to_string()));
        assert!(!found.iter().any(|p| p.contains("private")));
    }
}

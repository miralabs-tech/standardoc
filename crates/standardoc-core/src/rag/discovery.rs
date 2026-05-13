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

/// A path qualifies as convention prose when EITHER:
/// 1. its filename is in [`ROOT_DOC_FILES`] (at any depth — workspace
///    root, sub-package root, vendored package root) ; OR
/// 2. it lives under one of the [`PROSE_DIR_PREFIXES`] (docs/, notes/,
///    ...) at any depth.
///
/// Comparison is case-sensitive (matches the actual on-disk path).
/// Sub-package inclusion is intentional : monorepo workspaces commonly
/// keep prose next to each crate / package (e.g.
/// `standardoc/README.md`, `frontend/CHANGELOG.md`). Files the user
/// doesn't want indexed can opt out with `--- standardoc: false ---`
/// in their frontmatter ; the `.stdignore` exclusion list also runs
/// before the convention check.
pub fn is_convention_path(rel_path: &str) -> bool {
    matches_root_doc_file(rel_path) || starts_with_prose_dir(rel_path)
}

/// Filenames recognised as prose entry points regardless of nesting
/// depth. README is the historic baseline ; CHANGELOG / ARCHITECTURE /
/// CONTRIBUTING are added so monorepos that keep these next to each
/// sub-package don't need per-file frontmatter opt-in.
const ROOT_DOC_FILES: &[&str] = &[
    "README.md",
    "CHANGELOG.md",
    "ARCHITECTURE.md",
    "CONTRIBUTING.md",
];

/// Directory prefixes whose contents are prose by convention. `docs/`
/// is the historic baseline ; `notes/` covers project memos / session
/// handoffs that the v1.0-beta refactor of standardoc-core actively
/// uses (`notes/locks/`, `notes/done/`).
const PROSE_DIR_PREFIXES: &[&str] = &["docs/", "docs\\", "notes/", "notes\\"];

fn matches_root_doc_file(rel_path: &str) -> bool {
    for name in ROOT_DOC_FILES {
        if rel_path == *name {
            return true;
        }
        if rel_path
            .rsplit_once(['/', '\\'])
            .is_some_and(|(_, last)| last == *name)
        {
            return true;
        }
    }
    false
}

fn starts_with_prose_dir(rel_path: &str) -> bool {
    PROSE_DIR_PREFIXES
        .iter()
        .any(|p| rel_path.starts_with(p))
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
        let value = rest
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_ascii_lowercase();
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
        // Notes are now part of the convention (T-G : project memos /
        // session handoffs / locks).
        assert!(is_convention_path("notes/random.md"));
        assert!(is_convention_path("notes/locks/storage-4a.md"));
        assert!(!is_convention_path("src/lib.rs"));
    }

    #[test]
    fn is_convention_path_picks_up_sub_package_readmes() {
        assert!(is_convention_path("standardoc/README.md"));
        assert!(is_convention_path("frontend/README.md"));
        assert!(is_convention_path("crates/foo/README.md"));
        // Backslash path (Windows-style) also recognised.
        assert!(is_convention_path("standardoc\\README.md"));
        // CHANGELOG / CONTRIBUTING / ARCHITECTURE at any depth are now
        // auto-included alongside README (T-G).
        assert!(is_convention_path("CHANGELOG.md"));
        assert!(is_convention_path("standardoc/CHANGELOG.md"));
        assert!(is_convention_path("CONTRIBUTING.md"));
        assert!(is_convention_path("docs/ARCHITECTURE.md"));
        // Files NOT in the doc-file list and outside prose dirs remain
        // out — opt-in via frontmatter directive if you want them.
        assert!(!is_convention_path("standardoc/notes.md"));
        assert!(!is_convention_path("standardoc/random.md"));
    }

    #[test]
    fn parse_directive_recognises_rag_and_false() {
        assert_eq!(
            parse_directive("standardoc: rag\n"),
            FrontmatterDirective::Rag
        );
        assert_eq!(
            parse_directive("standardoc: \"rag\"\n"),
            FrontmatterDirective::Rag
        );
        assert_eq!(
            parse_directive("standardoc: true\n"),
            FrontmatterDirective::Rag
        );
        assert_eq!(
            parse_directive("standardoc: false\n"),
            FrontmatterDirective::Disabled
        );
        assert_eq!(
            parse_directive("standardoc: no\n"),
            FrontmatterDirective::Disabled
        );
        assert_eq!(
            parse_directive("standardoc: bogus\n"),
            FrontmatterDirective::Absent
        );
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
    fn discover_picks_up_readme_docs_and_notes_by_convention() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "README.md", "# r\n");
        write(root, "docs/a.md", "# a\n");
        write(root, "docs/sub/b.md", "# b\n");
        // T-G : notes/** is now part of the convention.
        write(root, "notes/random.md", "# n\n");
        // Random other .md is NOT auto-included.
        write(root, "random.md", "# x\n");
        let filters = fresh_filters(root);
        let mut found = discover_prose_sources(root, &filters);
        found.sort();
        assert_eq!(
            found,
            vec!["README.md", "docs/a.md", "docs/sub/b.md", "notes/random.md"]
        );
    }

    #[test]
    fn discover_picks_up_frontmatter_opt_in_anywhere() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Frontmatter opt-in is exercised on a non-convention path so
        // the assertion is unambiguous (notes/ became a convention in T-G).
        write(root, "scratch/x.md", "---\nstandardoc: rag\n---\nbody\n");
        write(root, "scratch/y.md", "no frontmatter\n");
        let filters = fresh_filters(root);
        let found = discover_prose_sources(root, &filters);
        assert!(found.contains(&"scratch/x.md".to_string()));
        assert!(!found.contains(&"scratch/y.md".to_string()));
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

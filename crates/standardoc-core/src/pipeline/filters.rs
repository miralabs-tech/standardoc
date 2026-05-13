//! `.stdignore` (gitignore-syntax) exclusion filters shared between
//! `cold_start::run` and `spawn_watcher`. The stack carries one `Gitignore`
//! matcher per source file (each rooted at its parent directory) so nested
//! `.stdignore` files override their parents with proper scope, including
//! `!negation` semantics. The watcher hot-rebuilds the stack on
//! `**/.stdignore` events and swaps it under the shared `RwLock`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use walkdir::WalkDir;

pub const STDIGNORE_FILENAME: &str = ".stdignore";

const STDIGNORE_SEED: &str = "\
# standardoc indexing exclusions (gitignore syntax)
# Edit freely. Lines added here exclude paths from the workspace index.
# Paths removed here trigger an automatic re-index of the affected subtree.

# VCS / package managers
.git/
node_modules/

# Build outputs
target/
dist/
build/

# Legacy / archived code (avoids cross-folder fqdn collisions)
.old/
*-old/

# Test fixtures / generated exports
test-export/
";

/// Aggregated `.stdignore` files from the workspace root down to the deepest
/// subdirectory that contained one. Each layer is a separate `Gitignore`
/// matcher rooted at its `.stdignore`'s parent directory: deeper files
/// override shallower ones (closest-parent-wins, with native `!negation`
/// support inside each layer).
///
/// Built once at `cold_start::run` boot and `spawn_watcher` boot. The watcher
/// hot-rebuilds it when an FS event lands on `**/.stdignore`.
pub struct GitignoreStack {
    /// Sorted by depth: shallowest first, deepest last. Iterating in reverse
    /// gives the closest-parent-wins evaluation order.
    layers: Vec<Layer>,
    workspace_root: PathBuf,
}

struct Layer {
    matcher: Gitignore,
    rooted_at: PathBuf,
}

impl GitignoreStack {
    /// Walks `workspace_root` downward and assembles one matcher per
    /// `.stdignore` found. The root layer is loaded first; nested layers are
    /// discovered through a walk that skips any subtree the root layer already
    /// excludes (avoids descending into `target/` or `node_modules/` just to
    /// look for nested `.stdignore` files).
    ///
    /// Returns an empty stack when no `.stdignore` is present anywhere.
    pub fn build(workspace_root: &Path) -> Self {
        let workspace_root = workspace_root.to_path_buf();
        let layers = collect_layers(&workspace_root);
        Self {
            layers,
            workspace_root,
        }
    }

    /// Tests whether a workspace-relative path is excluded. Forward-slash
    /// separators expected (SCHEMA §2.3 invariant). Parents are tested too:
    /// `target/debug/foo.rs` is ignored when `target/` is.
    ///
    /// Evaluation order is deepest-first; the first layer that has an opinion
    /// (`Ignore` or `Whitelist`) wins, mirroring git's closest-parent-wins
    /// semantics.
    pub fn is_ignored(&self, rel_path: &str) -> bool {
        if rel_path.is_empty() {
            return false;
        }
        let abs = self.workspace_root.join(native_path(rel_path));

        for layer in self.layers.iter().rev() {
            if !abs.starts_with(&layer.rooted_at) {
                continue;
            }
            match layer.matcher.matched_path_or_any_parents(&abs, false) {
                Match::Ignore(_) => return true,
                Match::Whitelist(_) => return false,
                Match::None => {}
            }
        }
        false
    }
}

/// Runtime filters applied during cold start + watcher dispatch. Wraps the
/// `GitignoreStack` so the surface stays extensible for future per-language
/// or extension-specific filters without breaking call sites.
///
/// Shared across threads via `Arc<RwLock<ScanFilters>>` so the watcher can
/// swap in a rebuilt stack on `.stdignore` change without re-spawning.
pub struct ScanFilters {
    pub stack: GitignoreStack,
}

impl ScanFilters {
    pub const fn from_stack(stack: GitignoreStack) -> Self {
        Self { stack }
    }

    /// Loads filters from a workspace root. Equivalent to
    /// `Self::from_stack(GitignoreStack::build(workspace_root))`.
    pub fn load(workspace_root: &Path) -> Self {
        Self::from_stack(GitignoreStack::build(workspace_root))
    }

    /// Convenience wrapper around `self.stack.is_ignored(rel_path)`. Kept as
    /// a method so the filter surface can grow (extension allowlist, custom
    /// language opt-outs, ...) without touching call sites.
    pub fn is_skipped(&self, rel_path: &str) -> bool {
        self.stack.is_ignored(rel_path)
    }
}

/// Writes the seed `.stdignore` at the workspace root when absent. Existing
/// files (even empty ones) are preserved verbatim — we never overwrite a user's
/// authored exclusions. Idempotent.
pub fn ensure_stdignore_seed_at(workspace_root: &Path) -> std::io::Result<()> {
    let path = workspace_root.join(STDIGNORE_FILENAME);
    if path.exists() {
        return Ok(());
    }
    std::fs::write(path, STDIGNORE_SEED)
}

/// Hard cap on the number of filesystem entries scanned by
/// [`preview_pattern_matches`]. A misbehaving pattern (e.g. `**` with
/// no filter) on a huge workspace would otherwise walk every node ;
/// this stops at 50k entries and reports `walk_truncated: true` so
/// callers can surface "scan was capped" in the UI.
pub const PATTERN_PREVIEW_WALK_CAP: usize = 50_000;

/// Aggregated output of [`preview_pattern_matches`]. The `matches`
/// vector is capped at the caller-supplied `limit` ; `total_count`
/// keeps counting beyond the cap so the UI can show "20 shown of 134".
#[derive(Debug, Clone, serde::Serialize)]
pub struct PatternPreview {
    pub pattern: String,
    pub matches: Vec<String>,
    pub total_count: usize,
    pub truncated: bool,
    /// `true` when the walk hit [`PATTERN_PREVIEW_WALK_CAP`] before
    /// completing — `total_count` is a lower bound in that case.
    pub walk_truncated: bool,
}

/// Errors specific to [`preview_pattern_matches`]. The two failure
/// modes are : the gitignore-syntax pattern is malformed, or the
/// matcher build itself blew up (`ignore::Error`).
#[derive(Debug, thiserror::Error)]
pub enum PatternPreviewError {
    #[error("invalid .stdignore pattern: {0}")]
    InvalidPattern(#[from] ignore::Error),
}

/// Walks `workspace_root` and collects every entry that matches a
/// single gitignore-syntax `pattern`. Used by the VSCode extension's
/// `.stdignore` hover provider to surface which paths a line would
/// catch BEFORE the user commits to the edit — a "preview" rather
/// than a guess.
///
/// The matcher built here is independent of any existing `.stdignore`
/// in the workspace : it represents what THIS one pattern in
/// isolation would match. The walk does not skip well-known mega-dirs
/// (a user typing `.git/` should see what falls under it), but caps
/// at [`PATTERN_PREVIEW_WALK_CAP`] entries to bound the worst case.
///
/// `limit` clamps the size of the returned `matches` vector while
/// `total_count` keeps counting unbounded (up to the walk cap).
pub fn preview_pattern_matches(
    workspace_root: &Path,
    pattern: &str,
    limit: usize,
) -> Result<PatternPreview, PatternPreviewError> {
    let trimmed = pattern.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Ok(PatternPreview {
            pattern: pattern.to_string(),
            matches: Vec::new(),
            total_count: 0,
            truncated: false,
            walk_truncated: false,
        });
    }
    let mut builder = GitignoreBuilder::new(workspace_root);
    builder.add_line(None, trimmed)?;
    let matcher = builder.build()?;

    let mut hits: Vec<String> = Vec::new();
    let mut total: usize = 0;
    let mut walk_truncated = false;
    for (scanned, entry) in WalkDir::new(workspace_root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .enumerate()
    {
        if scanned >= PATTERN_PREVIEW_WALK_CAP {
            walk_truncated = true;
            break;
        }
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(workspace_root) else {
            continue;
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let is_dir = entry.file_type().is_dir();
        // `matched_path_or_any_parents` so files INSIDE an ignored
        // directory still report as matched — `target/` should
        // surface every file under it, not just the `target` entry.
        if matches!(
            matcher.matched_path_or_any_parents(rel, is_dir),
            Match::Ignore(_)
        ) {
            total += 1;
            if hits.len() < limit {
                hits.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }

    Ok(PatternPreview {
        pattern: pattern.to_string(),
        matches: hits,
        total_count: total,
        truncated: total > limit,
        walk_truncated,
    })
}

fn collect_layers(workspace_root: &Path) -> Vec<Layer> {
    let mut layers = Vec::new();

    let root_file = workspace_root.join(STDIGNORE_FILENAME);
    if root_file.is_file()
        && let Some(layer) = build_layer(workspace_root, &root_file)
    {
        layers.push(layer);
    }

    let descent_filter = layers.first().map(|l| clone_matcher(&l.matcher));
    add_nested_layers(
        &mut layers,
        workspace_root,
        &root_file,
        descent_filter.as_ref(),
    );
    layers.sort_by_key(|l| l.rooted_at.components().count());
    layers
}

fn add_nested_layers(
    layers: &mut Vec<Layer>,
    workspace_root: &Path,
    root_file: &Path,
    descent_filter: Option<&Gitignore>,
) {
    let walker = WalkDir::new(workspace_root)
        .min_depth(1)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| match descent_filter {
            None => true,
            Some(matcher) => !matcher
                .matched_path_or_any_parents(entry.path(), entry.file_type().is_dir())
                .is_ignore(),
        });

    for entry in walker.flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.file_name() != OsStr::new(STDIGNORE_FILENAME) {
            continue;
        }
        if entry.path() == root_file {
            continue;
        }
        let Some(parent) = entry.path().parent() else {
            continue;
        };
        if let Some(layer) = build_layer(parent, entry.path()) {
            layers.push(layer);
        }
    }
}

fn build_layer(rooted_at: &Path, stdignore_file: &Path) -> Option<Layer> {
    let mut builder = GitignoreBuilder::new(rooted_at);
    if let Some(err) = builder.add(stdignore_file) {
        eprintln!(
            "standardoc: failed to load {}: {err}",
            stdignore_file.display()
        );
        return None;
    }
    match builder.build() {
        Ok(matcher) => Some(Layer {
            matcher,
            rooted_at: rooted_at.to_path_buf(),
        }),
        Err(err) => {
            eprintln!(
                "standardoc: failed to build gitignore matcher for {}: {err}",
                stdignore_file.display()
            );
            None
        }
    }
}

/// Re-builds a fresh `Gitignore` matcher equivalent to `source` so it can be
/// used inside the `WalkDir::filter_entry` closure without borrowing the
/// in-construction `layers` vec. `Gitignore` itself is not `Clone`.
fn clone_matcher(source: &Gitignore) -> Gitignore {
    let mut builder = GitignoreBuilder::new(source.path());
    let stdignore = source.path().join(STDIGNORE_FILENAME);
    if stdignore.is_file()
        && let Some(err) = builder.add(&stdignore)
    {
        eprintln!(
            "standardoc: failed to clone matcher from {}: {err}",
            stdignore.display()
        );
    }
    builder.build().unwrap_or_else(|_| Gitignore::empty())
}

fn native_path(rel_path: &str) -> PathBuf {
    if std::path::MAIN_SEPARATOR == '/' {
        PathBuf::from(rel_path)
    } else {
        PathBuf::from(rel_path.replace('/', std::path::MAIN_SEPARATOR_STR))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn write(root: &Path, rel: &str, body: &str) {
        let abs = root.join(rel);
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(abs, body).unwrap();
    }

    #[test]
    fn is_ignored_returns_false_when_no_layer() {
        let dir = tempdir().unwrap();
        let stack = GitignoreStack::build(dir.path());
        assert!(!stack.is_ignored("src/lib.rs"));
        assert!(!stack.is_ignored("anything"));
    }

    #[test]
    fn gitignore_stack_root_only() {
        let dir = tempdir().unwrap();
        write(dir.path(), ".stdignore", "target/\nbuild/\n");

        let stack = GitignoreStack::build(dir.path());

        assert!(stack.is_ignored("target/debug/foo.rs"));
        assert!(stack.is_ignored("build/output.js"));
        assert!(!stack.is_ignored("src/lib.rs"));
    }

    #[test]
    fn gitignore_stack_nested_layers_extend_root() {
        let dir = tempdir().unwrap();
        write(dir.path(), ".stdignore", "target/\n");
        write(dir.path(), "crates/.stdignore", "vendor/\n");

        let stack = GitignoreStack::build(dir.path());

        assert!(stack.is_ignored("target/debug.rs"));
        assert!(stack.is_ignored("crates/vendor/lib.rs"));
        assert!(!stack.is_ignored("crates/src/lib.rs"));
    }

    #[test]
    fn gitignore_stack_negation_override() {
        let dir = tempdir().unwrap();
        write(dir.path(), ".stdignore", "build/\n");
        write(dir.path(), "crates/.stdignore", "!build/\n");

        let stack = GitignoreStack::build(dir.path());

        assert!(stack.is_ignored("build/output.js"));
        assert!(!stack.is_ignored("crates/build/output.js"));
    }

    #[test]
    fn is_ignored_matches_glob_patterns() {
        let dir = tempdir().unwrap();
        write(dir.path(), ".stdignore", "*.lock\n**/generated/**\n");

        let stack = GitignoreStack::build(dir.path());

        assert!(stack.is_ignored("Cargo.lock"));
        assert!(stack.is_ignored("crates/foo/generated/api.ts"));
        assert!(!stack.is_ignored("Cargo.toml"));
    }

    #[test]
    fn build_skips_descent_into_excluded_subtrees() {
        let dir = tempdir().unwrap();
        write(dir.path(), ".stdignore", "target/\n");
        write(dir.path(), "target/.stdignore", "!debug/\n");

        let stack = GitignoreStack::build(dir.path());

        assert!(
            stack.is_ignored("target/debug/foo.rs"),
            "nested .stdignore inside an excluded subtree must not be discovered"
        );
    }

    #[test]
    fn ensure_stdignore_seed_writes_when_absent() {
        let dir = tempdir().unwrap();
        ensure_stdignore_seed_at(dir.path()).unwrap();

        let path = dir.path().join(STDIGNORE_FILENAME);
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains(".git/"));
        assert!(body.contains("target/"));
        assert!(body.contains("node_modules/"));
        assert!(body.contains("dist/"));
        assert!(body.contains("build/"));
        assert!(body.contains(".old/"));
        assert!(body.contains("*-old/"));
        assert!(body.contains("test-export/"));
        assert!(
            !body.contains(".standardoc/"),
            "seed must not include .standardoc/ (user decision lock 21 Q3)"
        );
    }

    #[test]
    fn ensure_stdignore_seed_preserves_existing_file() {
        let dir = tempdir().unwrap();
        let existing = "# my own exclusions\nfoo/\n";
        write(dir.path(), STDIGNORE_FILENAME, existing);

        ensure_stdignore_seed_at(dir.path()).unwrap();

        let body = fs::read_to_string(dir.path().join(STDIGNORE_FILENAME)).unwrap();
        assert_eq!(body, existing);
    }

    #[test]
    fn ensure_stdignore_seed_preserves_empty_file() {
        let dir = tempdir().unwrap();
        write(dir.path(), STDIGNORE_FILENAME, "");

        ensure_stdignore_seed_at(dir.path()).unwrap();

        let body = fs::read_to_string(dir.path().join(STDIGNORE_FILENAME)).unwrap();
        assert!(
            body.is_empty(),
            "an existing empty .stdignore must stay empty"
        );
    }

    #[test]
    fn scan_filters_load_constructs_from_workspace_root() {
        let dir = tempdir().unwrap();
        write(dir.path(), ".stdignore", "target/\n");

        let filters = ScanFilters::load(dir.path());

        assert!(filters.is_skipped("target/debug.rs"));
        assert!(!filters.is_skipped("src/lib.rs"));
    }

    #[test]
    fn is_ignored_handles_root_path_safely() {
        let dir = tempdir().unwrap();
        let stack = GitignoreStack::build(dir.path());
        assert!(!stack.is_ignored(""));
    }

    #[test]
    fn preview_pattern_matches_returns_paths_under_target_directory() {
        let dir = tempdir().unwrap();
        write(dir.path(), "src/lib.rs", "");
        write(dir.path(), "target/debug/foo.rs", "");
        write(dir.path(), "target/release/bar.rs", "");
        let preview = preview_pattern_matches(dir.path(), "target/", 20).unwrap();
        assert_eq!(preview.pattern, "target/");
        assert!(preview.matches.iter().any(|p| p == "target"
            || p == "target/debug"
            || p == "target/debug/foo.rs"));
        // Three entries under target/ (target dir + debug + release + 2 files)
        // — count is whatever the walker enumerates, all of which match.
        assert!(preview.total_count >= 3);
        assert!(!preview.matches.iter().any(|p| p == "src/lib.rs"));
        assert!(!preview.walk_truncated);
    }

    #[test]
    fn preview_pattern_matches_returns_empty_for_blank_or_comment() {
        let dir = tempdir().unwrap();
        write(dir.path(), "src/lib.rs", "");
        for blank in &["", "   ", "# comment line", "  # indented comment"] {
            let preview = preview_pattern_matches(dir.path(), blank, 20).unwrap();
            assert!(preview.matches.is_empty());
            assert_eq!(preview.total_count, 0);
        }
    }

    #[test]
    fn preview_pattern_matches_truncates_at_limit_while_counting_total() {
        let dir = tempdir().unwrap();
        for i in 0..10 {
            write(dir.path(), &format!("logs/file{i}.log"), "");
        }
        let preview = preview_pattern_matches(dir.path(), "*.log", 3).unwrap();
        assert_eq!(preview.matches.len(), 3);
        assert_eq!(preview.total_count, 10);
        assert!(preview.truncated);
    }

}

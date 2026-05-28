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

/// Legacy `.stdignore` filename, kept so the nested-cascade behaviour
/// (per-subfolder excludes) survives the migration to `standardoc.sxd`.
/// The workspace-root `.stdignore` is no longer auto-seeded — see
/// `config::ensure_sxd_seed_at` for the new path that migrates content
/// into `standardoc.sxd` on first cold-start.
pub const STDIGNORE_FILENAME: &str = ".stdignore";

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
        let layers = collect_layers(&workspace_root, None);
        Self {
            layers,
            workspace_root,
        }
    }

    /// Bug E-3 follow-up — same as [`Self::build`] but seeds the root
    /// layer from the supplied patterns string (the `ignore.patterns`
    /// block of `standardoc.sxd`) instead of reading `.stdignore`.
    /// Nested `.stdignore` files are still cascaded for back-compat
    /// during the migration window.
    pub fn build_with_root_patterns(workspace_root: &Path, patterns: &str) -> Self {
        let workspace_root = workspace_root.to_path_buf();
        let layers = collect_layers(&workspace_root, Some(patterns));
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

    /// Loads filters from a workspace root.
    ///
    /// Bug E-3 follow-up — checks `standardoc.sxd` first ; when an
    /// `ignore.patterns` block is present it overrides the root
    /// `.stdignore` (nested `.stdignore` files still cascade for the
    /// migration window). Falls back to the legacy `.stdignore`-only
    /// path when no `.sxd` is present (or it carries no ignore block).
    pub fn load(workspace_root: &Path) -> Self {
        if let Ok(Some(cfg)) = crate::config::load_workspace_config(workspace_root)
            && let Some(ignore) = cfg.ignore
        {
            return Self::from_stack(GitignoreStack::build_with_root_patterns(
                workspace_root,
                &ignore.patterns,
            ));
        }
        Self::from_stack(GitignoreStack::build(workspace_root))
    }

    /// Convenience wrapper around `self.stack.is_ignored(rel_path)`. Kept as
    /// a method so the filter surface can grow (extension allowlist, custom
    /// language opt-outs, ...) without touching call sites.
    pub fn is_skipped(&self, rel_path: &str) -> bool {
        self.stack.is_ignored(rel_path)
    }
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

fn collect_layers(workspace_root: &Path, root_patterns_override: Option<&str>) -> Vec<Layer> {
    let mut layers = Vec::new();

    let root_file = workspace_root.join(STDIGNORE_FILENAME);
    if let Some(patterns) = root_patterns_override {
        if let Some(layer) = build_layer_from_patterns(workspace_root, patterns) {
            layers.push(layer);
        }
    } else if root_file.is_file()
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

/// Bug E-3 follow-up — build a root layer from a patterns string
/// (e.g. the `ignore.patterns` block of `standardoc.sxd`). Empty or
/// blank lines are skipped ; lines starting with `#` are treated as
/// comments. Behaves identically to a `.stdignore` file otherwise.
fn build_layer_from_patterns(rooted_at: &Path, patterns: &str) -> Option<Layer> {
    let mut builder = GitignoreBuilder::new(rooted_at);
    for line in patterns.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Err(err) = builder.add_line(None, trimmed) {
            eprintln!(
                "standardoc: failed to load standardoc.sxd ignore pattern `{trimmed}`: {err}"
            );
            return None;
        }
    }
    match builder.build() {
        Ok(matcher) => Some(Layer {
            matcher,
            rooted_at: rooted_at.to_path_buf(),
        }),
        Err(err) => {
            eprintln!("standardoc: failed to build gitignore matcher from standardoc.sxd: {err}");
            None
        }
    }
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
mod tests;

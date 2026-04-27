//! Parallel workspace scanner.
//!
//! Walks the workspace, dispatches each file to the matching language provider,
//! and yields a flat list of discovered symbols. The extractor and index
//! wiring happen above this layer — the scanner is deliberately dumb.
//!
//! The walk respects `.gitignore`, `.ignore`, and custom `.stdocignore` by
//! default — this avoids scanning `node_modules/`, `target/`, `dist/`, etc.
//! without requiring explicit user configuration. `ScanOptions` can disable
//! each ignore source and add extra gitignore-style patterns from
//! `.standardoc.json`.

use crate::extractor::comment_scan::{self, CommentSpan};
use crate::lang::{DiscoveredSymbol, LanguageProvider, ParseError};
use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Registry of language providers keyed by file extension.
///
/// Cheap to clone (`Arc` under the hood) so each scanner worker can carry
/// its own reference without locking.
#[derive(Clone, Default)]
pub struct Registry {
    // Extension without leading dot -> provider.
    // An extension in multiple providers is resolved first-wins at registration time.
    providers: Arc<HashMap<String, Arc<dyn LanguageProvider>>>,
}

impl Registry {
    pub fn builder() -> RegistryBuilder {
        RegistryBuilder::default()
    }

    pub fn resolve(&self, path: &Path) -> Option<Arc<dyn LanguageProvider>> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        self.providers.get(&ext).cloned()
    }

    pub fn len(&self) -> usize {
        self.providers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}

#[derive(Default)]
pub struct RegistryBuilder {
    providers: HashMap<String, Arc<dyn LanguageProvider>>,
}

impl RegistryBuilder {
    #[must_use]
    pub fn with<P: LanguageProvider + 'static>(mut self, provider: P) -> Self {
        let provider: Arc<dyn LanguageProvider> = Arc::new(provider);
        for ext in provider.extensions() {
            let normalized = ext.trim_start_matches('.').to_ascii_lowercase();
            self.providers
                .entry(normalized)
                .or_insert_with(|| provider.clone());
        }
        self
    }

    pub fn build(self) -> Registry {
        Registry {
            providers: Arc::new(self.providers),
        }
    }
}

/// Outcome of scanning a single file.
#[derive(Debug)]
pub struct FileScan {
    pub path: PathBuf,
    pub symbols: Vec<DiscoveredSymbol>,
    /// Every comment block found in the file (free-floating included).
    /// The extractor scans these for `@doc-extend K` satellite directives —
    /// see `extractor::extract_satellite_blocks`.
    pub comment_spans: Vec<CommentSpan>,
}

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("parse error in {path}: {source}")]
    Parse { path: PathBuf, source: ParseError },
}

/// Result of a full workspace scan: successful file scans plus non-fatal errors.
#[derive(Debug, Default)]
pub struct ScanReport {
    pub files: Vec<FileScan>,
    pub errors: Vec<ScanError>,
}

/// Scanner walk knobs. All values are opt-in:
/// `Default::default()` gives sensible behavior (gitignore on,
/// hidden files skipped, `.standardoc-ignore` recognized).
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Respect `.gitignore` files found while walking up/down tree.
    /// Disable to scan generated code ignored by git but still desired in index.
    pub respect_gitignore: bool,
    /// Patterns gitignore-style additionnels (`node_modules/`, `target/`,
    /// `**/*.generated.ts`, ...). Applied after `.gitignore` —
    /// use `!` prefix to re-include.
    pub exclude_files: Vec<String>,
    /// Custom ignore filenames to recognize next to `.gitignore`. By default
    /// we support `.stdocignore` for projects that want a separate ignore list
    /// from the git stack (same gitignore-style rules, different filename).
    pub custom_ignore_filenames: Vec<String>,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            respect_gitignore: true,
            exclude_files: Vec::new(),
            custom_ignore_filenames: vec![".stdocignore".to_owned()],
        }
    }
}

/// Walks `root` and dispatches each matching file to the registered provider.
///
/// Files whose extension has no provider are silently skipped. Parse errors
/// and read errors are collected into `ScanReport::errors` rather than
/// aborting the scan — the caller decides whether to treat them as fatal.
///
/// After a provider returns its `DiscoveredSymbol`s (whose FQNs are local to
/// the file), the scanner prepends a **module prefix** derived from the file's
/// location in the workspace — see [`derive_module_prefix`]. This is what
/// distinguishes `standardoc_core::dsl::parser::ParseError` from
/// `standardoc_core::lang::ParseError` that would otherwise collide.
pub fn scan_workspace(root: &Path, registry: &Registry, options: &ScanOptions) -> ScanReport {
    let files: Vec<(PathBuf, Arc<dyn LanguageProvider>)> = build_walker(root, options)
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .filter_map(|e| {
            let path = e.into_path();
            let provider = registry.resolve(&path)?;
            Some((path, provider))
        })
        .collect();

    let (files, errors): (Vec<_>, Vec<_>) = files
        .into_par_iter()
        .map(|(path, provider)| scan_file(&path, provider.as_ref()))
        .partition(Result::is_ok);

    ScanReport {
        files: files.into_iter().map(Result::unwrap).collect(),
        errors: errors.into_iter().map(Result::unwrap_err).collect(),
    }
}

/// Build a `WalkBuilder` configured from `ScanOptions`.
///
/// `require_git(false)` is essential: otherwise `ignore` applies `.gitignore`
/// only in initialized git repos. We want identical scan behavior both in
/// git repos and exported folders.
///
/// `exclude_files` are compiled into a single `Override` added to walker —
/// they override `.gitignore` (`!` prefix to re-include).
fn build_walker(root: &Path, options: &ScanOptions) -> WalkBuilder {
    let mut walker = WalkBuilder::new(root);
    walker
        .follow_links(false)
        .git_ignore(options.respect_gitignore)
        .git_global(options.respect_gitignore)
        .git_exclude(options.respect_gitignore)
        .require_git(false)
        .ignore(true)
        .hidden(true)
        .parents(true);

    for filename in &options.custom_ignore_filenames {
        walker.add_custom_ignore_filename(filename);
    }

    if !options.exclude_files.is_empty() {
        let mut overrides = OverrideBuilder::new(root);
        for pattern in &options.exclude_files {
            // `OverrideBuilder` treats each entry as an **include** pattern by
            // default; exclusion requires a `!` prefix. If user writes
            // `node_modules/` in `.standardoc.json`, we convert it to
            // `!node_modules/` to match expected semantics
            // (gitignore = exclude by default).
            let inverted = pattern
                .strip_prefix('!')
                .map_or_else(|| format!("!{pattern}"), str::to_owned);
            // Best effort: malformed patterns are logged, not fatal — scan
            // continues without that rule instead of aborting.
            if let Err(err) = overrides.add(&inverted) {
                eprintln!(
                    "standardoc scanner: ignoring invalid exclude pattern {pattern:?}: {err}"
                );
            }
        }
        if let Ok(o) = overrides.build() {
            walker.overrides(o);
        }
    }

    walker
}

fn scan_file(path: &Path, provider: &dyn LanguageProvider) -> Result<FileScan, ScanError> {
    let content = fs::read_to_string(path).map_err(|source| ScanError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    scan_file_content(path, &content, provider)
}

/// Scan a single file from already-loaded content. Shared core between the
/// disk-walking `scan_workspace` and the in-memory `scan_in_memory` paths
/// — the only difference between them is who reads the bytes.
fn scan_file_content(
    path: &Path,
    content: &str,
    provider: &dyn LanguageProvider,
) -> Result<FileScan, ScanError> {
    let mut symbols = provider
        .discover_symbols(content, path)
        .map_err(|source| ScanError::Parse {
            path: path.to_path_buf(),
            source,
        })?;

    let prefix = derive_module_prefix(path);
    if !prefix.is_empty() {
        for sym in &mut symbols {
            let mut full = prefix.clone();
            full.append(&mut sym.fqn);
            sym.fqn = full;
        }
    }

    let comment_spans = comment_scan::scan(content, provider.comment_styles());

    Ok(FileScan {
        path: path.to_path_buf(),
        symbols,
        comment_spans,
    })
}

/// Scan an explicit set of in-memory files (already read by the caller).
/// Built for hosts where the host filesystem isn't directly accessible —
/// typically the WASM build the VSCode extension links against, where file
/// enumeration goes through `vscode.workspace.findFiles` and content is
/// read via `vscode.workspace.fs.readFile`.
///
/// Files whose extension has no matching provider are silently skipped
/// (same semantics as `scan_workspace`). The caller is expected to have
/// applied any gitignore / `.stdocignore` filtering on its side — there is
/// no `WalkBuilder` here.
pub fn scan_in_memory(files: Vec<(PathBuf, String)>, registry: &Registry) -> ScanReport {
    let prepared: Vec<(PathBuf, String, Arc<dyn LanguageProvider>)> = files
        .into_iter()
        .filter_map(|(path, content)| {
            let provider = registry.resolve(&path)?;
            Some((path, content, provider))
        })
        .collect();

    let (files, errors): (Vec<_>, Vec<_>) = prepared
        .into_par_iter()
        .map(|(path, content, provider)| scan_file_content(&path, &content, provider.as_ref()))
        .partition(Result::is_ok);

    ScanReport {
        files: files.into_iter().map(Result::unwrap).collect(),
        errors: errors.into_iter().map(Result::unwrap_err).collect(),
    }
}

/// Derives a module path from a file's absolute location.
///
/// Convention (works for both Cargo and npm/TS package layouts):
/// - Look for the last `src/` component in the path.
/// - The last directory **before** `src/` is the package name (dashes → underscores).
/// - The path **after** `src/` is the module path, with:
///   - file extensions stripped
///   - canonical module-root names dropped: `lib`, `main`, `mod`, `index`
///
/// Examples:
/// - `.../crates/standardoc-core/src/dsl/parser.rs` → `[standardoc_core, dsl, parser]`
/// - `.../crates/standardoc-core/src/lib.rs`        → `[standardoc_core]`
/// - `.../crates/standardoc-core/src/model/mod.rs`  → `[standardoc_core, model]`
/// - `.../packages/users/src/index.ts`              → `[users]`
/// - `.../packages/users/src/api/create.ts`         → `[users, api, create]`
///
/// When no `src/` is present (flat layouts, scripts, examples), falls back to
/// the file stem alone — rather than dragging every parent directory into the
/// prefix, which would be noise.
#[must_use]
pub fn derive_module_prefix(path: &Path) -> Vec<String> {
    use std::path::Component;
    let components: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str().map(str::to_owned),
            _ => None,
        })
        .collect();

    let Some(src_idx) = components.iter().rposition(|c| c == "src") else {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|s| !is_module_root(s))
            .map(sanitize_segment);
        return stem.into_iter().collect();
    };

    let before = &components[..src_idx];
    let after = &components[src_idx + 1..];
    let mut prefix: Vec<String> = Vec::with_capacity(1 + after.len());

    if let Some(pkg) = before.last() {
        prefix.push(sanitize_segment(pkg));
    }

    for (i, comp) in after.iter().enumerate() {
        let is_last = i == after.len() - 1;
        if is_last {
            let stem = Path::new(comp)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(comp);
            if is_module_root(stem) {
                break;
            }
            prefix.push(sanitize_segment(stem));
        } else {
            prefix.push(sanitize_segment(comp));
        }
    }

    prefix
}

fn sanitize_segment(s: &str) -> String {
    s.replace('-', "_")
}

fn is_module_root(stem: &str) -> bool {
    matches!(stem, "lib" | "main" | "mod" | "index")
}

#[cfg(test)]
mod prefix_tests {
    use super::*;

    fn p(parts: &[&str]) -> PathBuf {
        parts.iter().collect()
    }

    #[test]
    fn rust_file_in_submodule() {
        let file = p(&[
            "workspace",
            "crates",
            "standardoc-core",
            "src",
            "dsl",
            "parser.rs",
        ]);
        assert_eq!(
            derive_module_prefix(&file),
            vec!["standardoc_core", "dsl", "parser"]
        );
    }

    #[test]
    fn rust_lib_root() {
        let file = p(&["workspace", "crates", "standardoc-core", "src", "lib.rs"]);
        assert_eq!(derive_module_prefix(&file), vec!["standardoc_core"]);
    }

    #[test]
    fn rust_mod_rs_points_to_parent() {
        let file = p(&[
            "workspace",
            "crates",
            "standardoc-core",
            "src",
            "model",
            "mod.rs",
        ]);
        assert_eq!(
            derive_module_prefix(&file),
            vec!["standardoc_core", "model"]
        );
    }

    #[test]
    fn ts_index_points_to_parent() {
        let file = p(&["workspace", "packages", "users", "src", "index.ts"]);
        assert_eq!(derive_module_prefix(&file), vec!["users"]);
    }

    #[test]
    fn ts_nested_file() {
        let file = p(&["workspace", "packages", "users", "src", "api", "create.ts"]);
        assert_eq!(derive_module_prefix(&file), vec!["users", "api", "create"]);
    }

    #[test]
    fn no_src_falls_back_to_file_stem() {
        let file = p(&["workspace", "scripts", "build.ts"]);
        assert_eq!(derive_module_prefix(&file), vec!["build"]);
    }

    #[test]
    fn no_src_with_module_root_name_falls_back_empty() {
        let file = p(&["workspace", "lib.rs"]);
        let empty: Vec<String> = Vec::new();
        assert_eq!(derive_module_prefix(&file), empty);
    }
}

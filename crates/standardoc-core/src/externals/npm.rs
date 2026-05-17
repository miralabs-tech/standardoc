//! npm / pnpm / yarn-PnP resolver — locates a package's source files on
//! disk and parses them via the provided [`LanguageProvider`].
//!
//! Three resolution strategies dispatched by what the workspace ships:
//!
//! - **classic** : `<workspace>/node_modules/<pkg>/`. Reads
//!   `package.json` for the package name verification, then walks the
//!   tree.
//! - **pnpm virtual store** : pnpm symlinks `node_modules/<pkg>` to
//!   `node_modules/.pnpm/<pkg>@<ver>/node_modules/<pkg>`. `canonicalize`
//!   on the classic path resolves the symlink transparently, so we do
//!   not need a dedicated pnpm code path — the classic walk hits the
//!   real directory automatically.
//! - **yarn-PnP** : when `.pnp.cjs` exists at the workspace root, the
//!   resolver shells out to
//!   `node -e 'process.stdout.write(require("./.pnp.cjs").resolveRequest(name, cwd) ?? "")'`
//!   to ask yarn for the package's absolute path. Requires the `node`
//!   binary; surfaced as `MissingBinary` when the probe came back
//!   `Missing` at construction time.
//!
//! Stamped `SourceOrigin`: [`SourceOrigin::NodeModulesDts`] for every
//! shape — invalidation purges the bucket on `external_npm_lockfile_hash`
//! change, regardless of which strategy produced the symbols.
//!
//! Day-1 scope (covered by the user's "tout day-1" lock):
//!
//! - `.d.ts` files (TypeScript ambient declarations).
//! - `.ts` sources when the package publishes them (rare).
//! - `.js` non-minified sources (heuristic: every line ≤ 1000 chars).
//!
//! Out: `.js` minified bundles (mangled names = noise for the graph).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use standardoc_ir::{ExtractedFile, RawSymbol, SourceOrigin};
use walkdir::WalkDir;

use super::binary_detection::{self, BinaryAvailability};
use super::{ExternalsError, ResolveOutcome, Resolver};
use crate::commands::IngestCommand;
use crate::pipeline::{ExtractContext, LanguageProvider};
use crate::query;
use crate::storage::handle::IndexHandle;

const SUBMIT_DRAIN_TIMEOUT: Duration = Duration::from_mins(1);
const SUBMIT_DRAIN_POLL: Duration = Duration::from_millis(20);
const MINIFIED_LINE_THRESHOLD: usize = 1000;
const NPM_EXTENSIONS: &[&str] = &["d.ts", "ts", "tsx", "js", "jsx", "mjs", "cjs"];

/// Resolver backed by `node_modules` discovery + optional `node`
/// subprocess for yarn-PnP. Holds the workspace root and the latest
/// `node` binary probe (only meaningful when `.pnp.cjs` is present).
#[derive(Debug)]
pub struct NpmResolver {
    workspace_root: PathBuf,
    /// Set to `NotApplicable` when no `.pnp.cjs` exists at the workspace
    /// root — classic / pnpm strategies do not need `node`. The MCP
    /// `missing_binary` warning is only surfaced when this resolves to
    /// `Missing` AND the workspace actually uses yarn-PnP.
    node_binary: BinaryAvailability,
}

impl NpmResolver {
    /// Constructs the resolver. Probes `node` only when the workspace
    /// holds a `.pnp.cjs` manifest — otherwise stores
    /// [`BinaryAvailability::NotApplicable`].
    #[must_use]
    pub fn new(workspace_root: PathBuf) -> Self {
        let node_binary = if binary_detection::workspace_has_pnp_cjs(&workspace_root) {
            binary_detection::probe_binary("node", binary_detection::ENV_NODE_PATH)
        } else {
            BinaryAvailability::NotApplicable
        };
        Self {
            workspace_root,
            node_binary,
        }
    }
}

impl Resolver for NpmResolver {
    fn name(&self) -> &'static str {
        "npm"
    }

    fn source_origin(&self) -> SourceOrigin {
        SourceOrigin::NodeModulesDts
    }

    fn binary_availability(&self) -> BinaryAvailability {
        self.node_binary.clone()
    }

    fn resolve(
        &self,
        handle: &IndexHandle,
        provider: &dyn LanguageProvider,
        fqdn: &str,
    ) -> Result<ResolveOutcome, ExternalsError> {
        let Some(package_name) = package_name_of_fqdn(fqdn) else {
            return Ok(ResolveOutcome::NotInThisRegistry);
        };

        // Cache hit short-circuit.
        if let Some(sym) = query::symbol_by_fqdn(handle, fqdn)? {
            return Ok(ResolveOutcome::Resolved {
                symbol: sym,
                source_origin: self.source_origin(),
            });
        }

        // Locate the package — classic+pnpm via canonicalize, fallback
        // to yarn-PnP subprocess when `.pnp.cjs` is present.
        let package_dir = match self.find_package_dir(package_name)? {
            FindResult::Found(dir) => dir,
            FindResult::NotFound => return Ok(ResolveOutcome::NotInThisRegistry),
            FindResult::MissingBinary => {
                return Ok(ResolveOutcome::MissingBinary {
                    binary: "node",
                    env_override: binary_detection::ENV_NODE_PATH,
                });
            }
        };

        let submitted = walk_and_submit_package(
            handle,
            provider,
            &package_dir,
            package_name,
            self.source_origin(),
        )?;
        if submitted == 0 {
            return Ok(ResolveOutcome::NotInThisRegistry);
        }

        let origin = self.source_origin();
        poll_for_symbol(handle, fqdn).map(|opt| match opt {
            Some(symbol) => ResolveOutcome::Resolved {
                symbol,
                source_origin: origin,
            },
            None => ResolveOutcome::NotInThisRegistry,
        })
    }
}

enum FindResult {
    Found(PathBuf),
    NotFound,
    MissingBinary,
}

impl NpmResolver {
    fn find_package_dir(&self, package_name: &str) -> Result<FindResult, ExternalsError> {
        // 1. Classic + pnpm symlink: `<ws>/node_modules/<pkg>` canonicalized.
        let classic = self.workspace_root.join("node_modules").join(package_name);
        if classic.is_dir() {
            let canonical = classic.canonicalize().unwrap_or(classic);
            return Ok(FindResult::Found(canonical));
        }

        // 2. Yarn-PnP fallback via `node -e require('./.pnp.cjs').resolveRequest(...)`.
        let pnp_cjs = self.workspace_root.join(".pnp.cjs");
        if !pnp_cjs.is_file() {
            return Ok(FindResult::NotFound);
        }
        let node_path = match &self.node_binary {
            BinaryAvailability::Available { path, .. } => path.clone(),
            BinaryAvailability::Missing { .. } => return Ok(FindResult::MissingBinary),
            BinaryAvailability::NotApplicable => return Ok(FindResult::NotFound),
        };
        match resolve_via_pnp(&node_path, &self.workspace_root, package_name)? {
            Some(dir) if dir.is_dir() => Ok(FindResult::Found(dir)),
            _ => Ok(FindResult::NotFound),
        }
    }
}

fn resolve_via_pnp(
    node_path: &Path,
    workspace_root: &Path,
    package_name: &str,
) -> Result<Option<PathBuf>, ExternalsError> {
    let script = format!(
        "const r = require('./.pnp.cjs').resolveRequest('{escaped}/package.json', process.cwd()); \
         if (r) process.stdout.write(require('path').dirname(r));",
        escaped = package_name.replace('\'', "\\'"),
    );
    let output = Command::new(node_path)
        .arg("-e")
        .arg(&script)
        .current_dir(workspace_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| ExternalsError::Subprocess {
            binary: node_path.display().to_string(),
            exit: None,
            detail: format!("spawn failed: {e}"),
        })?;
    if !output.status.success() {
        // Yarn-PnP failure to resolve = package not in deps. Not a hard
        // error — let the orchestrator decline gracefully.
        return Ok(None);
    }
    let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path_str.is_empty() {
        Ok(None)
    } else {
        Ok(Some(PathBuf::from(path_str)))
    }
}

/// Extracts the leading package name from an fqdn. Handles `@scope/pkg`
/// shape — the `/` is part of the name, the `::` separator is unique.
fn package_name_of_fqdn(fqdn: &str) -> Option<&str> {
    let (head, _rest) = fqdn.split_once("::")?;
    if head.is_empty() { None } else { Some(head) }
}

fn walk_and_submit_package(
    handle: &IndexHandle,
    provider: &dyn LanguageProvider,
    package_dir: &Path,
    package_name: &str,
    origin: SourceOrigin,
) -> Result<usize, ExternalsError> {
    let mut submitted = 0usize;
    let ctx = ExtractContext {
        workspace_root: package_dir,
    };

    for entry in WalkDir::new(package_dir)
        .into_iter()
        .filter_entry(|e| !is_skip_npm_dir(e.file_name().to_str().unwrap_or("")))
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        if !has_npm_extension(entry.path()) {
            continue;
        }

        let content = match std::fs::read_to_string(entry.path()) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => continue,
            Err(e) => {
                return Err(ExternalsError::Io {
                    fqdn: entry.path().display().to_string(),
                    source: e,
                });
            }
        };
        if is_minified_js(entry.path(), &content) {
            continue;
        }

        let rel = entry
            .path()
            .strip_prefix(package_dir)
            .map_err(|_| ExternalsError::Internal {
                detail: format!(
                    "walkdir produced path `{}` outside package dir `{}`",
                    entry.path().display(),
                    package_dir.display()
                ),
            })?
            .to_string_lossy()
            .replace('\\', "/");

        let mut extracted = match provider.extract(&content, &rel, &ctx) {
            Ok(e) => e,
            Err(_) => continue, // parse error → skip file, keep going
        };
        rewrite_for_external(&mut extracted, package_name, &rel, origin);

        let path = extracted.file.clone();
        handle
            .submit_blocking(IngestCommand::UpsertFile { path, extracted })
            .map_err(|e| ExternalsError::Internal {
                detail: format!("writer queue closed: {e}"),
            })?;
        submitted += 1;
    }
    Ok(submitted)
}

fn is_skip_npm_dir(name: &str) -> bool {
    matches!(
        name,
        "node_modules" | ".git" | "test" | "tests" | "__tests__" | "examples" | "example"
    )
}

fn has_npm_extension(path: &Path) -> bool {
    let s = match path.file_name().and_then(|s| s.to_str()) {
        Some(s) => s,
        None => return false,
    };
    NPM_EXTENSIONS.iter().any(|ext| {
        // Use `ends_with` on the file name (not extension) to catch
        // `.d.ts` (a compound extension that `Path::extension` only
        // returns as "ts").
        s.ends_with(&format!(".{ext}"))
    })
}

fn is_minified_js(path: &Path, content: &str) -> bool {
    let is_js = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| matches!(ext, "js" | "jsx" | "mjs" | "cjs"));
    if !is_js {
        return false;
    }
    content
        .lines()
        .any(|line| line.len() > MINIFIED_LINE_THRESHOLD)
}

fn rewrite_for_external(
    extracted: &mut ExtractedFile,
    package_name: &str,
    rel_path: &str,
    origin: SourceOrigin,
) {
    extracted.file = format!("external://npm/{package_name}/{rel_path}");
    extracted.is_external = true;
    extracted.source_origin = origin;
}

fn poll_for_symbol(handle: &IndexHandle, fqdn: &str) -> Result<Option<RawSymbol>, ExternalsError> {
    let deadline = Instant::now() + SUBMIT_DRAIN_TIMEOUT;
    loop {
        if let Some(sym) = query::symbol_by_fqdn(handle, fqdn)? {
            return Ok(Some(sym));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(SUBMIT_DRAIN_POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn new_probes_node_when_pnp_cjs_present() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".pnp.cjs"), "// pnp").unwrap();
        let resolver = NpmResolver::new(dir.path().to_path_buf());
        assert!(!matches!(
            resolver.binary_availability(),
            BinaryAvailability::NotApplicable
        ));
    }

    #[test]
    fn new_marks_node_not_applicable_without_pnp_cjs() {
        let dir = tempdir().unwrap();
        let resolver = NpmResolver::new(dir.path().to_path_buf());
        assert_eq!(
            resolver.binary_availability(),
            BinaryAvailability::NotApplicable
        );
    }

    #[test]
    fn name_is_stable() {
        let dir = tempdir().unwrap();
        let resolver = NpmResolver::new(dir.path().to_path_buf());
        assert_eq!(resolver.name(), "npm");
        assert_eq!(resolver.source_origin(), SourceOrigin::NodeModulesDts);
    }

    #[test]
    fn package_name_of_fqdn_handles_scoped_packages() {
        assert_eq!(
            package_name_of_fqdn("@types/react::Component"),
            Some("@types/react")
        );
        assert_eq!(package_name_of_fqdn("react::useEffect"), Some("react"));
    }

    #[test]
    fn package_name_of_fqdn_returns_none_on_bare_symbol() {
        assert_eq!(package_name_of_fqdn("bare"), None);
        assert_eq!(package_name_of_fqdn(""), None);
    }

    #[test]
    fn has_npm_extension_recognises_d_ts() {
        assert!(has_npm_extension(Path::new("foo.d.ts")));
        assert!(has_npm_extension(Path::new("a/b/index.d.ts")));
    }

    #[test]
    fn has_npm_extension_recognises_classic_extensions() {
        for ext in ["ts", "tsx", "js", "jsx", "mjs", "cjs"] {
            assert!(has_npm_extension(Path::new(&format!("file.{ext}"))));
        }
    }

    #[test]
    fn has_npm_extension_rejects_unrelated() {
        assert!(!has_npm_extension(Path::new("README.md")));
        assert!(!has_npm_extension(Path::new("Cargo.toml")));
        assert!(!has_npm_extension(Path::new("file.css")));
    }

    #[test]
    fn is_minified_js_detects_long_lines() {
        let content = format!("var x={};", "1".repeat(1500));
        assert!(is_minified_js(Path::new("bundle.js"), &content));
    }

    #[test]
    fn is_minified_js_passes_normal_code() {
        let content = "function foo() {\n  return 42;\n}\n";
        assert!(!is_minified_js(Path::new("foo.js"), content));
    }

    #[test]
    fn is_minified_js_only_applies_to_js_extensions() {
        let content = format!("// {}", "x".repeat(1500));
        assert!(!is_minified_js(Path::new("types.d.ts"), &content));
        assert!(!is_minified_js(Path::new("source.ts"), &content));
    }

    #[test]
    fn is_skip_npm_dir_filters_well_known_noise() {
        assert!(is_skip_npm_dir("node_modules"));
        assert!(is_skip_npm_dir("test"));
        assert!(is_skip_npm_dir("tests"));
        assert!(is_skip_npm_dir("__tests__"));
        assert!(is_skip_npm_dir("examples"));
        assert!(is_skip_npm_dir(".git"));
        assert!(!is_skip_npm_dir("src"));
        assert!(!is_skip_npm_dir("dist"));
    }

    #[test]
    fn rewrite_for_external_stamps_sentinel_path_for_npm() {
        let mut extracted = ExtractedFile {
            file: "index.d.ts".into(),
            language: standardoc_ir::Language::TypeScript,
            source_origin: SourceOrigin::Workspace,
            is_external: false,
            content_hash: standardoc_ir::Blake3Hash::default(),
            byte_size: 0,
            symbols: vec![],
            edges: vec![],
            call_sites: vec![],
            documents: vec![],
            ffi_bindings: vec![],
        };
        rewrite_for_external(
            &mut extracted,
            "@types/react",
            "index.d.ts",
            SourceOrigin::NodeModulesDts,
        );
        assert_eq!(extracted.file, "external://npm/@types/react/index.d.ts");
        assert!(extracted.is_external);
        assert_eq!(extracted.source_origin, SourceOrigin::NodeModulesDts);
    }
}

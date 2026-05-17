//! Cargo registry resolver — locates a crate's source files on disk
//! using `cargo metadata --format-version=1` (subprocess) and parses the
//! crate via the provided [`LanguageProvider`].
//!
//! Day-1 path: `<workspace>/Cargo.lock` → `cargo metadata` JSON →
//! `packages[].manifest_path` → walk `<crate>/src/**/*.rs` → parse each
//! via `provider.extract(...)` → rewrite `is_external = true`,
//! `source_origin = CargoRegistry`, sentinel path
//! `external://cargo/<crate>-<version>/<rel>` → submit via writer queue
//! → poll for the requested FQDN to land in the index.
//!
//! Git/path/alt registries are supported transparently because
//! `cargo metadata` emits their resolved `manifest_path` regardless of
//! source. The resolver treats them all the same.
//!
//! Stamped `SourceOrigin`: [`SourceOrigin::CargoRegistry`] for everything
//! this resolver produces. We do not split crates.io vs. git vs. path —
//! the invalidation step purges them as a single bucket via the cached
//! `Cargo.lock` BLAKE3 hash in `schema_meta.external_cargo_lockfile_hash`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;
use standardoc_ir::{ExtractedFile, RawSymbol, SourceOrigin};
use walkdir::WalkDir;

use super::binary_detection::{self, BinaryAvailability};
use super::{ExternalsError, ResolveOutcome, Resolver};
use crate::commands::IngestCommand;
use crate::pipeline::{ExtractContext, LanguageProvider};
use crate::query;
use crate::storage::handle::IndexHandle;

/// Max wall-clock the resolver waits for the writer thread to drain
/// submitted ExtractedFiles into the index before giving up. Long
/// enough for serde-sized crates on slow disks; short enough that a
/// stuck writer thread becomes a visible error rather than a hang.
const SUBMIT_DRAIN_TIMEOUT: Duration = Duration::from_mins(1);

/// Poll interval while waiting for the writer to drain.
const SUBMIT_DRAIN_POLL: Duration = Duration::from_millis(20);

/// Resolver backed by `cargo metadata`. Holds the workspace root (used
/// as `--manifest-path` parent) and the last probed `cargo` binary.
#[derive(Debug)]
pub struct CargoResolver {
    workspace_root: PathBuf,
    binary: BinaryAvailability,
}

impl CargoResolver {
    /// Constructs the resolver and probes the `cargo` binary once. The
    /// caller (typically [`super::ResolverRegistry::for_workspace`])
    /// gates construction behind [`super::find_cargo_lock`] — if no
    /// `Cargo.lock` is reachable from the workspace root, the resolver
    /// is omitted from the registry entirely.
    #[must_use]
    pub fn new(workspace_root: PathBuf) -> Self {
        let binary = binary_detection::probe_binary("cargo", binary_detection::ENV_CARGO_PATH);
        Self {
            workspace_root,
            binary,
        }
    }
}

impl Resolver for CargoResolver {
    fn name(&self) -> &'static str {
        "cargo"
    }

    fn source_origin(&self) -> SourceOrigin {
        SourceOrigin::CargoRegistry
    }

    fn binary_availability(&self) -> BinaryAvailability {
        self.binary.clone()
    }

    fn resolve(
        &self,
        handle: &IndexHandle,
        provider: &dyn LanguageProvider,
        fqdn: &str,
    ) -> Result<ResolveOutcome, ExternalsError> {
        let Some(crate_name) = crate_name_of_fqdn(fqdn) else {
            return Ok(ResolveOutcome::NotInThisRegistry);
        };

        // 1. Cache hit — the fqdn might already be in the index from a
        //    previous resolve of the same crate (or a sibling fqdn of
        //    the same crate that pulled in the whole tree).
        if let Some(sym) = query::symbol_by_fqdn(handle, fqdn)? {
            return Ok(ResolveOutcome::Resolved {
                symbol: sym,
                source_origin: self.source_origin(),
            });
        }

        // 2. Binary probe — fail loud, the agent can configure
        //    STANDARDOC_CARGO_PATH and retry.
        let cargo_path = match &self.binary {
            BinaryAvailability::Available { path, .. } => path.clone(),
            BinaryAvailability::Missing { .. } | BinaryAvailability::NotApplicable => {
                return Ok(ResolveOutcome::MissingBinary {
                    binary: "cargo",
                    env_override: binary_detection::ENV_CARGO_PATH,
                });
            }
        };

        // 3. Locate the workspace `Cargo.lock`. The resolver should not
        //    have been constructed without one, but the file may have
        //    been deleted between boot and call — surface explicitly.
        let workspace_manifest = self.workspace_root.join("Cargo.toml");
        if !self.workspace_root.join("Cargo.lock").is_file() {
            return Ok(ResolveOutcome::LockfileNotFound {
                lockfile: "Cargo.lock",
            });
        }

        // 4. Run `cargo metadata`. With deps included so we see external
        //    crate manifest_paths (`--no-deps` would only return
        //    workspace packages).
        let metadata = run_cargo_metadata(&cargo_path, &workspace_manifest)?;

        // 5. Match the requested crate by name. Take the first matching
        //    package — workspaces with multiple versions of the same
        //    crate are rare day-1 and accept slight over-extraction.
        let Some(package) = metadata.packages.iter().find(|p| p.name == crate_name) else {
            return Ok(ResolveOutcome::NotInThisRegistry);
        };

        let manifest_dir = package
            .manifest_path
            .parent()
            .ok_or_else(|| ExternalsError::Parse {
                name: "cargo metadata manifest_path".into(),
                detail: format!(
                    "manifest_path `{}` has no parent",
                    package.manifest_path.display()
                ),
            })?;

        // 6. Walk `<manifest_dir>/src/**/*.rs`, parse each via the
        //    provider, stamp external + sentinel path, submit.
        let crate_slug = format!("{}-{}", package.name, package.version);
        let submitted = walk_and_submit_crate(
            handle,
            provider,
            manifest_dir,
            &crate_slug,
            self.source_origin(),
        )?;
        if submitted == 0 {
            // No .rs files under src/. Could happen for binary-only or
            // build-script-only packages. Either way the requested
            // fqdn cannot resolve here.
            return Ok(ResolveOutcome::NotInThisRegistry);
        }

        // 7. Poll for the requested symbol to land in the index.
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

/// Extracts the leading crate name from an fqdn. Returns `None` when
/// the fqdn has no `::` separator (workspace-style bare symbols cannot
/// reference external crates).
fn crate_name_of_fqdn(fqdn: &str) -> Option<&str> {
    let (head, _rest) = fqdn.split_once("::")?;
    if head.is_empty() { None } else { Some(head) }
}

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoPackage>,
}

#[derive(Debug, Deserialize)]
struct CargoPackage {
    name: String,
    version: String,
    manifest_path: PathBuf,
}

fn run_cargo_metadata(
    cargo_path: &Path,
    workspace_manifest: &Path,
) -> Result<CargoMetadata, ExternalsError> {
    let output = Command::new(cargo_path)
        .arg("metadata")
        .arg("--format-version=1")
        .arg("--manifest-path")
        .arg(workspace_manifest)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| ExternalsError::Subprocess {
            binary: cargo_path.display().to_string(),
            exit: None,
            detail: format!("spawn failed: {e}"),
        })?;
    if !output.status.success() {
        return Err(ExternalsError::Subprocess {
            binary: cargo_path.display().to_string(),
            exit: output.status.code(),
            detail: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    serde_json::from_slice(&output.stdout).map_err(|e| ExternalsError::Parse {
        name: "cargo metadata".into(),
        detail: e.to_string(),
    })
}

fn walk_and_submit_crate(
    handle: &IndexHandle,
    provider: &dyn LanguageProvider,
    manifest_dir: &Path,
    crate_slug: &str,
    origin: SourceOrigin,
) -> Result<usize, ExternalsError> {
    let src_dir = manifest_dir.join("src");
    if !src_dir.is_dir() {
        return Ok(0);
    }

    let mut submitted = 0usize;
    let ctx = ExtractContext {
        workspace_root: manifest_dir,
    };

    for entry in WalkDir::new(&src_dir).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.path().extension().is_none_or(|ext| ext != "rs") {
            continue;
        }

        let abs = entry.path();
        let rel = abs
            .strip_prefix(manifest_dir)
            .map_err(|_| ExternalsError::Internal {
                detail: format!(
                    "walkdir produced path `{}` outside manifest dir `{}`",
                    abs.display(),
                    manifest_dir.display()
                ),
            })?;
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let content = match std::fs::read_to_string(abs) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => continue, // non-UTF8
            Err(e) => {
                return Err(ExternalsError::Io {
                    fqdn: rel_str,
                    source: e,
                });
            }
        };

        let mut extracted = match provider.extract(&content, &rel_str, &ctx) {
            Ok(e) => e,
            Err(_) => continue, // parse error: skip this file, keep going
        };
        rewrite_for_external(&mut extracted, crate_slug, &rel_str, origin);

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

/// Stamps an [`ExtractedFile`] as external and rewrites its `file`
/// field to the sentinel path used by cold-start cleanup to skip
/// external rows (see `pipeline::cold_start::cleanup_unseen`).
fn rewrite_for_external(
    extracted: &mut ExtractedFile,
    crate_slug: &str,
    rel_path: &str,
    origin: SourceOrigin,
) {
    extracted.file = format!("external://cargo/{crate_slug}/{rel_path}");
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
    fn new_probes_cargo_binary_on_construct() {
        let dir = tempdir().unwrap();
        let resolver = CargoResolver::new(dir.path().to_path_buf());
        assert!(matches!(
            resolver.binary_availability(),
            BinaryAvailability::Available { .. }
        ));
    }

    #[test]
    fn name_is_stable() {
        let dir = tempdir().unwrap();
        let resolver = CargoResolver::new(dir.path().to_path_buf());
        assert_eq!(resolver.name(), "cargo");
        assert_eq!(resolver.source_origin(), SourceOrigin::CargoRegistry);
    }

    #[test]
    fn crate_name_of_fqdn_extracts_first_segment() {
        assert_eq!(crate_name_of_fqdn("serde::Deserialize"), Some("serde"));
        assert_eq!(crate_name_of_fqdn("tokio::runtime::Builder"), Some("tokio"));
    }

    #[test]
    fn crate_name_of_fqdn_returns_none_on_bare_symbol() {
        assert_eq!(crate_name_of_fqdn("bare_symbol"), None);
        assert_eq!(crate_name_of_fqdn(""), None);
    }

    #[test]
    fn crate_name_of_fqdn_returns_none_on_leading_separator() {
        assert_eq!(crate_name_of_fqdn("::leading"), None);
    }

    #[test]
    fn rewrite_for_external_stamps_sentinel_path_and_flags() {
        let mut extracted = ExtractedFile {
            file: "src/lib.rs".into(),
            language: standardoc_ir::Language::Rust,
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
            "serde-1.0.0",
            "src/lib.rs",
            SourceOrigin::CargoRegistry,
        );
        assert_eq!(extracted.file, "external://cargo/serde-1.0.0/src/lib.rs");
        assert!(extracted.is_external);
        assert_eq!(extracted.source_origin, SourceOrigin::CargoRegistry);
    }
}

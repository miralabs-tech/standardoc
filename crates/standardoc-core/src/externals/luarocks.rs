//! luarocks resolver — Lua's de-facto package manager, used opt-in.
//!
//! Day-1 strategy: subprocess `luarocks show --rock-dir <rock>` to ask
//! the installed `luarocks` for the absolute install directory of the
//! requested rock, then walk it for `.lua` files and parse each via
//! the provided [`LanguageProvider`] (TS, Rust, Lua — the `WorkspaceProvider`
//! dispatcher routes `.lua` to `LuaProvider`).
//!
//! Cross-version compatibility note: `luarocks show --rock-dir` lands
//! on every luarocks ≥ 3.0. Older 2.x deployments are out of scope
//! day-1; the resolver gracefully returns `NotInThisRegistry` when the
//! subprocess fails, so a missing/old luarocks does not destabilise the
//! daemon.
//!
//! Stamped `SourceOrigin`: [`SourceOrigin::ManualExternal`] day-1 — the
//! IR enum has no dedicated Lua variant, and luarocks is niche enough
//! that bundling it under `ManualExternal` is fine. Promoted to a
//! dedicated variant if (and only if) actual users complain.
//!
//! Pre-construction gate: [`super::workspace_has_lua_files`] returns
//! `true`. Workspaces with zero `.lua` files do not construct this
//! resolver — no `luarocks --version` probe is ever emitted, so the
//! VSCode extension does not get noise warnings on pure Rust/TS
//! projects.

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

/// Resolver backed by the `luarocks` CLI. Holds the workspace root
/// (used for env-relative config lookups) and the probed binary state.
#[derive(Debug)]
pub struct LuarocksResolver {
    #[allow(dead_code)] // reserved for future env-relative config probes.
    workspace_root: PathBuf,
    binary: BinaryAvailability,
}

impl LuarocksResolver {
    /// Probes `luarocks` once and stores the result. Construction is
    /// gated by [`super::workspace_has_lua_files`] upstream so this is
    /// only ever called on workspaces that actually consume Lua.
    #[must_use]
    pub fn new(workspace_root: PathBuf) -> Self {
        let binary =
            binary_detection::probe_binary("luarocks", binary_detection::ENV_LUAROCKS_PATH);
        Self {
            workspace_root,
            binary,
        }
    }
}

impl Resolver for LuarocksResolver {
    fn name(&self) -> &'static str {
        "luarocks"
    }

    fn source_origin(&self) -> SourceOrigin {
        SourceOrigin::ManualExternal
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
        let Some(rock_name) = rock_name_of_fqdn(fqdn) else {
            return Ok(ResolveOutcome::NotInThisRegistry);
        };

        // Cache hit short-circuit.
        if let Some(sym) = query::symbol_by_fqdn(handle, fqdn)? {
            return Ok(ResolveOutcome::Resolved {
                symbol: sym,
                source_origin: self.source_origin(),
            });
        }

        let luarocks_path = match &self.binary {
            BinaryAvailability::Available { path, .. } => path.clone(),
            BinaryAvailability::Missing { .. } | BinaryAvailability::NotApplicable => {
                return Ok(ResolveOutcome::MissingBinary {
                    binary: "luarocks",
                    env_override: binary_detection::ENV_LUAROCKS_PATH,
                });
            }
        };

        let rock_dir = match run_luarocks_rock_dir(&luarocks_path, rock_name)? {
            Some(dir) if dir.is_dir() => dir,
            _ => return Ok(ResolveOutcome::NotInThisRegistry),
        };

        let submitted =
            walk_and_submit_rock(handle, provider, &rock_dir, rock_name, self.source_origin())?;
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

/// Extracts the leading rock name from an fqdn. Rocks do not have
/// scope/namespace convention so this is identical to the cargo case.
fn rock_name_of_fqdn(fqdn: &str) -> Option<&str> {
    let (head, _rest) = fqdn.split_once("::")?;
    if head.is_empty() { None } else { Some(head) }
}

fn run_luarocks_rock_dir(
    luarocks_path: &Path,
    rock_name: &str,
) -> Result<Option<PathBuf>, ExternalsError> {
    let output = Command::new(luarocks_path)
        .arg("show")
        .arg("--rock-dir")
        .arg(rock_name)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| ExternalsError::Subprocess {
            binary: luarocks_path.display().to_string(),
            exit: None,
            detail: format!("spawn failed: {e}"),
        })?;
    if !output.status.success() {
        // luarocks exits non-zero when the rock is not installed.
        // Treat as a soft decline rather than a hard error.
        return Ok(None);
    }
    let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path_str.is_empty() {
        Ok(None)
    } else {
        Ok(Some(PathBuf::from(path_str)))
    }
}

fn walk_and_submit_rock(
    handle: &IndexHandle,
    provider: &dyn LanguageProvider,
    rock_dir: &Path,
    rock_name: &str,
    origin: SourceOrigin,
) -> Result<usize, ExternalsError> {
    let mut submitted = 0usize;
    let ctx = ExtractContext {
        workspace_root: rock_dir,
        cross_workspace: None,
    };

    for entry in WalkDir::new(rock_dir).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.path().extension().is_none_or(|ext| ext != "lua") {
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

        let rel = entry
            .path()
            .strip_prefix(rock_dir)
            .map_err(|_| ExternalsError::Internal {
                detail: format!(
                    "walkdir produced path `{}` outside rock dir `{}`",
                    entry.path().display(),
                    rock_dir.display()
                ),
            })?
            .to_string_lossy()
            .replace('\\', "/");

        let mut extracted = match provider.extract(&content, &rel, &ctx) {
            Ok(e) => e,
            Err(_) => continue,
        };
        rewrite_for_external(&mut extracted, rock_name, &rel, origin);

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

fn rewrite_for_external(
    extracted: &mut ExtractedFile,
    rock_name: &str,
    rel_path: &str,
    origin: SourceOrigin,
) {
    extracted.file = format!("external://luarocks/{rock_name}/{rel_path}");
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
    fn new_probes_luarocks_binary_on_construct() {
        let dir = tempdir().unwrap();
        let resolver = LuarocksResolver::new(dir.path().to_path_buf());
        assert!(!matches!(
            resolver.binary_availability(),
            BinaryAvailability::NotApplicable
        ));
    }

    #[test]
    fn name_is_stable() {
        let dir = tempdir().unwrap();
        let resolver = LuarocksResolver::new(dir.path().to_path_buf());
        assert_eq!(resolver.name(), "luarocks");
        assert_eq!(resolver.source_origin(), SourceOrigin::ManualExternal);
    }

    #[test]
    fn rock_name_of_fqdn_extracts_first_segment() {
        assert_eq!(rock_name_of_fqdn("lua-cjson::encode"), Some("lua-cjson"));
        assert_eq!(rock_name_of_fqdn("penlight::pl::stringx"), Some("penlight"));
    }

    #[test]
    fn rock_name_of_fqdn_returns_none_on_bare_symbol() {
        assert_eq!(rock_name_of_fqdn("bare_symbol"), None);
        assert_eq!(rock_name_of_fqdn(""), None);
    }

    #[test]
    fn rewrite_for_external_stamps_sentinel_path_for_luarocks() {
        let mut extracted = ExtractedFile {
            file: "init.lua".into(),
            language: standardoc_ir::Language::Lua,
            source_origin: SourceOrigin::Workspace,
            is_external: false,
            content_hash: standardoc_ir::Blake3Hash::default(),
            byte_size: 0,
            symbols: vec![],
            edges: vec![],
            call_sites: vec![],
            documents: vec![],
            ffi_bindings: vec![],
            module_lookup: None,
        };
        rewrite_for_external(
            &mut extracted,
            "lua-cjson",
            "init.lua",
            SourceOrigin::ManualExternal,
        );
        assert_eq!(extracted.file, "external://luarocks/lua-cjson/init.lua");
        assert!(extracted.is_external);
        assert_eq!(extracted.source_origin, SourceOrigin::ManualExternal);
    }
}

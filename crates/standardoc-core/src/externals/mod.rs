//! External-symbol resolvers (S3-G — lazy on-demand).
//!
//! Resolvers parse a single file out of a package manager's local cache
//! when an FQDN that points outside the workspace is explicitly requested
//! (via the MCP `resolve_external` tool). They never pre-index transitive
//! dependencies and never own the workspace writer thread — the
//! orchestrator hands the produced [`standardoc_ir::ExtractedFile`] to the
//! normal pipeline (`IngestCommand::UpsertFile` with `is_external = true`
//! + the resolver's [`SourceOrigin`]).
//!
//! Day-1 implementations (scaffold B):
//!
//! - [`cargo::CargoResolver`] — uses `cargo metadata` to locate
//!   `<crate>-<version>/src/lib.rs` and delegates parsing to `RustProvider`.
//! - [`npm::NpmResolver`] — walks classic `node_modules/`, pnpm virtual
//!   store and yarn-PnP (`.pnp.cjs` via subprocess `node`); reuses
//!   `TsProvider`.
//! - [`luarocks::LuarocksResolver`] — subprocess `luarocks` + walk
//!   `~/.luarocks/share/lua/<ver>/<rock>/`.
//!
//! [`Resolver`] keeps a sync surface because every concrete resolver
//! either does small fs reads or shells out to a CLI; the MCP handler
//! wraps the call in `tokio::task::spawn_blocking` so the async runtime
//! never stalls.

mod binary_detection;
pub mod cargo;
pub mod luarocks;
pub mod npm;

use std::path::PathBuf;
use std::sync::Arc;

use standardoc_ir::{RawSymbol, SourceOrigin};

use crate::pipeline::LanguageProvider;
use crate::storage::handle::IndexHandle;

pub use binary_detection::{
    BinaryAvailability, ENV_CARGO_PATH, ENV_LUAROCKS_PATH, ENV_NODE_PATH, find_cargo_lock,
    find_package_json, probe_binary, workspace_has_lua_files, workspace_has_pnp_cjs,
};

/// Outcome of attempting to resolve a single FQDN against an external
/// registry. The orchestrator iterates the registry's resolvers and
/// short-circuits on the first non-`NotInThisRegistry` answer.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)] // Resolved is the hot path; boxing RawSymbol moves cost from happy case to sad case.
pub enum ResolveOutcome {
    /// The resolver located the symbol, ingested the surrounding sources
    /// into the index with `is_external = 1`, and looked up the requested
    /// FQDN. `symbol` is the one the user asked for; `source_origin`
    /// identifies which resolver produced it (so the MCP envelope can
    /// surface "cargo" / "node_modules" / "luarocks" without depending
    /// on a per-symbol `source_origin` field in `RawSymbol`).
    Resolved {
        symbol: RawSymbol,
        source_origin: SourceOrigin,
    },
    /// This resolver is not responsible for this FQDN (e.g. an
    /// `@scope/pkg::...` FQDN handed to the cargo resolver, or a
    /// `<crate>::...` whose `<crate>` is not in `cargo metadata`'s
    /// package list).
    NotInThisRegistry,
    /// The resolver requires a binary that was either not detected at
    /// boot or vanished at call time. Surfaced as a structured signal so
    /// the MCP tool can return `status: "missing_binary"` without
    /// destabilising the daemon.
    MissingBinary {
        binary: &'static str,
        env_override: &'static str,
    },
    /// The resolver is correctly wired but the workspace lacks the
    /// expected manifest (e.g. cargo resolver run against a workspace
    /// with no `Cargo.lock`).
    LockfileNotFound { lockfile: &'static str },
}

/// Sync trait shared by every external resolver. Concrete impls live in
/// [`cargo`], [`npm`] and [`luarocks`]. Resolvers are stored as
/// `Box<dyn Resolver>` inside the [`ResolverRegistry`] so the trait
/// stays object-safe — no generics on methods.
pub trait Resolver: Send + Sync {
    /// Stable resolver identifier surfaced in logs and the MCP envelope.
    fn name(&self) -> &'static str;

    /// [`SourceOrigin`] stamped on every `RawSymbol` this resolver
    /// produces. Used by the invalidation step to scope a purge to one
    /// resolver's output (see `pipeline::external_invalidation`).
    fn source_origin(&self) -> SourceOrigin;

    /// Latest probe result for the binary backing this resolver. Cached
    /// by the registry — refreshed on demand only when an actual call
    /// reports `MissingBinary`.
    fn binary_availability(&self) -> BinaryAvailability;

    /// Attempt to resolve a single FQDN. The resolver:
    /// 1. Checks the cache (`is_external = 1` rows already in the
    ///    index for the same crate / package).
    /// 2. If a cache miss, locates the relevant source files, parses
    ///    them via the provided [`LanguageProvider`], stamps
    ///    `is_external = true` + the resolver's `source_origin` and
    ///    submits them via `handle.submit_blocking`.
    /// 3. Looks up the requested FQDN in the index and returns the
    ///    [`RawSymbol`] inside [`ResolveOutcome::Resolved`].
    fn resolve(
        &self,
        handle: &IndexHandle,
        provider: &dyn LanguageProvider,
        fqdn: &str,
    ) -> Result<ResolveOutcome, ExternalsError>;
}

/// Aggregates every resolver shipped with a daemon instance. Built once
/// at boot from the workspace root, then handed to the MCP server as an
/// `Arc<ResolverRegistry>` shared across tool invocations.
#[derive(Clone)]
pub struct ResolverRegistry {
    workspace_root: PathBuf,
    resolvers: Arc<Vec<Box<dyn Resolver>>>,
}

impl ResolverRegistry {
    /// Builds the registry from the workspace root. For Cargo and npm,
    /// the manifest probes ([`find_cargo_lock`] / [`find_package_json`])
    /// walk DOWN from `workspace_root` to discover monorepo subprojects;
    /// each resolver is anchored at the directory containing its
    /// manifest, not at the user's workspace root. For Luarocks (no
    /// workspace manifest), [`workspace_has_lua_files`] consults the
    /// workspace's `.stdignore` so a user can opt out of scanning by
    /// ignoring `.lua` files. Probe results are cached on the resolver
    /// instance.
    #[must_use]
    pub fn for_workspace(workspace_root: PathBuf) -> Self {
        let mut resolvers: Vec<Box<dyn Resolver>> = Vec::new();
        if let Some(cargo_root) = find_cargo_lock(&workspace_root) {
            resolvers.push(Box::new(cargo::CargoResolver::new(cargo_root)));
        }
        if let Some(npm_root) = find_package_json(&workspace_root) {
            resolvers.push(Box::new(npm::NpmResolver::new(npm_root)));
        }
        if workspace_has_lua_files(&workspace_root) {
            resolvers.push(Box::new(luarocks::LuarocksResolver::new(
                workspace_root.clone(),
            )));
        }
        Self {
            workspace_root,
            resolvers: Arc::new(resolvers),
        }
    }

    /// Returns the absolute workspace root the registry was built for.
    #[must_use]
    pub fn workspace_root(&self) -> &std::path::Path {
        &self.workspace_root
    }

    /// Snapshot of every resolver's binary probe — surfaced at boot via
    /// `STDOC_WARN` for resolvers in `Missing` state when their
    /// preconditions are present (e.g. `.pnp.cjs` exists but `node` is
    /// missing).
    #[must_use]
    pub fn probe_all(&self) -> Vec<(&'static str, BinaryAvailability)> {
        self.resolvers
            .iter()
            .map(|r| (r.name(), r.binary_availability()))
            .collect()
    }

    /// Iterates resolvers in registration order, short-circuiting on the
    /// first answer that is NOT `NotInThisRegistry`. Returns
    /// `NotInThisRegistry` only when every resolver declined.
    pub fn resolve(
        &self,
        handle: &IndexHandle,
        provider: &dyn LanguageProvider,
        fqdn: &str,
    ) -> Result<ResolveOutcome, ExternalsError> {
        for resolver in self.resolvers.iter() {
            let outcome = resolver.resolve(handle, provider, fqdn)?;
            if !matches!(outcome, ResolveOutcome::NotInThisRegistry) {
                return Ok(outcome);
            }
        }
        Ok(ResolveOutcome::NotInThisRegistry)
    }

    /// Number of resolvers registered. Useful for assertions in tests
    /// and for the boot WARN sweep (zero registered = no externals
    /// possible on this workspace, no need to warn the user).
    #[must_use]
    pub fn len(&self) -> usize {
        self.resolvers.len()
    }

    /// `true` when no resolver was constructed for this workspace.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.resolvers.is_empty()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExternalsError {
    #[error("I/O failure resolving external `{fqdn}`: {source}")]
    Io {
        fqdn: String,
        #[source]
        source: std::io::Error,
    },
    #[error("subprocess `{binary}` failed (exit {exit:?}): {detail}")]
    Subprocess {
        binary: String,
        exit: Option<i32>,
        detail: String,
    },
    #[error("could not parse `{name}`: {detail}")]
    Parse { name: String, detail: String },
    #[error("workspace root `{root}` is not a directory")]
    InvalidWorkspaceRoot { root: PathBuf },
    #[error("storage failure during external resolution: {0}")]
    Storage(#[from] crate::storage::error::StorageError),
    #[error("internal error: {detail}")]
    Internal { detail: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::ExtractContext;
    use standardoc_ir::ExtractedFile;
    use tempfile::tempdir;

    /// Test-local stub provider — every `extract` call returns
    /// `UnsupportedLanguage`. Sufficient for the dispatch tests below
    /// (which only need the trait object to exist; no resolver hits
    /// `provider.extract` until B2b/B3/B4 land).
    struct StubProvider;
    impl LanguageProvider for StubProvider {
        fn extract(
            &self,
            _content: &str,
            path: &str,
            _ctx: &ExtractContext<'_>,
        ) -> Result<ExtractedFile, crate::pipeline::ExtractError> {
            Err(crate::pipeline::ExtractError::UnsupportedLanguage { file: path.into() })
        }
    }

    #[test]
    fn for_workspace_constructs_no_resolvers_on_empty_workspace() {
        let dir = tempdir().unwrap();
        let registry = ResolverRegistry::for_workspace(dir.path().to_path_buf());
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert_eq!(registry.workspace_root(), dir.path());
    }

    #[test]
    fn for_workspace_constructs_cargo_resolver_when_cargo_lock_present() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.lock"), "# v1").unwrap();
        let registry = ResolverRegistry::for_workspace(dir.path().to_path_buf());
        assert_eq!(registry.len(), 1);
        let names: Vec<&str> = registry.probe_all().iter().map(|(n, _)| *n).collect();
        assert_eq!(names, vec!["cargo"]);
    }

    #[test]
    fn for_workspace_constructs_npm_resolver_when_package_json_present() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        let registry = ResolverRegistry::for_workspace(dir.path().to_path_buf());
        let names: Vec<&str> = registry.probe_all().iter().map(|(n, _)| *n).collect();
        assert!(names.contains(&"npm"));
    }

    #[test]
    fn for_workspace_constructs_luarocks_resolver_when_lua_files_present() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("init.lua"), "").unwrap();
        let registry = ResolverRegistry::for_workspace(dir.path().to_path_buf());
        let names: Vec<&str> = registry.probe_all().iter().map(|(n, _)| *n).collect();
        assert!(names.contains(&"luarocks"));
    }

    #[test]
    fn for_workspace_constructs_all_three_when_mixed_workspace() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.lock"), "# v1").unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        std::fs::write(dir.path().join("plugin.lua"), "").unwrap();
        let registry = ResolverRegistry::for_workspace(dir.path().to_path_buf());
        assert_eq!(registry.len(), 3);
        let names: Vec<&str> = registry.probe_all().iter().map(|(n, _)| *n).collect();
        assert!(names.contains(&"cargo"));
        assert!(names.contains(&"npm"));
        assert!(names.contains(&"luarocks"));
    }

    #[test]
    fn resolve_returns_not_in_this_registry_on_empty_workspace() {
        let dir = tempdir().unwrap();
        let registry = ResolverRegistry::for_workspace(dir.path().to_path_buf());
        let handle = IndexHandle::open(dir.path()).unwrap();
        let provider = StubProvider;
        let outcome = registry
            .resolve(&handle, &provider, "serde::Deserialize")
            .unwrap();
        assert!(matches!(outcome, ResolveOutcome::NotInThisRegistry));
    }

    #[test]
    fn resolve_returns_not_in_registry_when_npm_only_workspace_has_no_match() {
        // Workspace ships `package.json` (npm resolver registered) but no
        // `Cargo.lock` (skips Cargo resolver, avoids cargo subprocess) and
        // no `.lua` files (skips Lua resolver, avoids luarocks subprocess
        // whose availability is env-dependent in test envs). Tests the
        // single-resolver dispatch shape end-to-end without depending on
        // any external CLI being installed.
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        let registry = ResolverRegistry::for_workspace(dir.path().to_path_buf());
        assert_eq!(registry.len(), 1);
        let handle = IndexHandle::open(dir.path()).unwrap();
        let provider = StubProvider;
        let outcome = registry
            .resolve(&handle, &provider, "anything::nothing")
            .unwrap();
        assert!(matches!(outcome, ResolveOutcome::NotInThisRegistry));
    }
}

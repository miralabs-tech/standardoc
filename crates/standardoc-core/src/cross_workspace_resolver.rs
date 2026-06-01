#![allow(
    clippy::significant_drop_tightening,
    clippy::significant_drop_in_scrutinee
)]
//! Stage 3 R3 — DB-backed `CrossWorkspaceResolver` for the visitor.
//!
//! The resolver answers `(origin_module, origin_symbol) -> CrossWorkspaceLookup`
//! at extract time so cross-workspace edges materialise in the
//! [`standardoc_ir::ExtractedFile`] payload (Resolved-with-peer-attrs /
//! typed UnresolvedBridge) rather than only at MCP query time.
//!
//! Strategy:
//! 1. Per-resolve lookup against the persisted `module_lookups` table via
//!    [`crate::storage::cross_workspace`].
//! 2. Lazy in-memory cache keyed by `(module, symbol)` — amortises peer
//!    `ModuleLookup` blob decodes across all references inside a
//!    workspace pass (a single peer's exports get decoded once even if
//!    100+ files import from it).
//! 3. Storage errors collapse into [`CrossWorkspaceLookup::Unknown`] —
//!    the visitor must not fail extraction over a transient lookup
//!    failure. The original Stage 3b-4 MCP-time resolver surfaces
//!    errors through its own path; this resolver is best-effort.

use std::collections::HashMap;
use std::sync::Mutex;

use standardoc_ir::{CrossWorkspaceLookup, CrossWorkspaceResolver};

use crate::storage::cross_workspace::{
    peer_workspace_for_module, resolve_cross_workspace_import, resolve_intra_workspace_import,
};
use crate::storage::handle::IndexHandle;

/// DB-backed resolver wired into [`crate::pipeline::ExtractContext`] for
/// each extract pass. Owns the in-memory cache (`Mutex<HashMap>`) and
/// borrows the [`IndexHandle`] for pool access. Cheap to construct: no
/// upfront SQL is issued at `new`.
pub struct DbCrossWorkspaceResolver<'a> {
    handle: &'a IndexHandle,
    cache: Mutex<HashMap<(String, String), CrossWorkspaceLookup>>,
}

impl<'a> DbCrossWorkspaceResolver<'a> {
    #[must_use]
    pub fn new(handle: &'a IndexHandle) -> Self {
        Self {
            handle,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Compute the lookup without touching the cache. Tries the
    /// verbatim `origin_module` first (covers crates whose Cargo.toml
    /// name genuinely uses underscores, e.g. `rust_decimal`); when
    /// that misses, normalises the leftmost segment `_` → `-` and
    /// retries, because Rust source uses `use my_crate::Foo` but the
    /// IR + Cargo metadata key on the hyphenated form `my-crate`.
    /// Storage errors collapse to `Unknown`.
    fn compute(&self, origin_module: &str, origin_symbol: &str) -> CrossWorkspaceLookup {
        let direct = self.compute_with_key(origin_module, origin_symbol);
        if !matches!(direct, CrossWorkspaceLookup::Unknown) {
            return direct;
        }
        let normalized = normalize_crate_prefix_to_hyphen(origin_module);
        if normalized != origin_module {
            let alt = self.compute_with_key(&normalized, origin_symbol);
            if !matches!(alt, CrossWorkspaceLookup::Unknown) {
                return alt;
            }
        }
        direct
    }

    fn compute_with_key(&self, origin_module: &str, origin_symbol: &str) -> CrossWorkspaceLookup {
        let Ok(conn) = self.handle.conn() else {
            return CrossWorkspaceLookup::Unknown;
        };
        // Pass 1 — linked peer workspaces. Cheapest hit path for the
        // historical multi-workspace use case (Stage 3 R3).
        match resolve_cross_workspace_import(&conn, origin_module, origin_symbol) {
            Ok(Some(resolution)) => {
                return CrossWorkspaceLookup::Hit {
                    workspace_id: resolution.workspace_id,
                    fqdn: resolution.resolved_fqdn,
                };
            }
            Ok(None) => {}
            Err(_) => return CrossWorkspaceLookup::Unknown,
        }
        // Pass 2 — primary (this) workspace's sibling crates. Critical
        // for the mono-repo case (no peers linked): without this,
        // edges crossing crate boundaries within a single workspace
        // (e.g. `standardoc-lang-provider` → `standardoc-ir::FfiAbi`)
        // stayed Unresolved. The cross_workspace_post pass has
        // already filtered out edges targeting the file's own crate
        // via its `is_local` check, so a hit here is always a real
        // cross-crate resolution.
        if let Ok(Some(resolution)) =
            resolve_intra_workspace_import(&conn, origin_module, origin_symbol)
        {
            return CrossWorkspaceLookup::Hit {
                workspace_id: resolution.workspace_id,
                fqdn: resolution.resolved_fqdn,
            };
        }
        // Pass 3 — peer-presence probe so the cross_workspace_post
        // pass can stamp the typed `UnresolvedBridge` when a known
        // peer owns the module but doesn't export the symbol.
        match peer_workspace_for_module(&conn, origin_module) {
            Ok(Some(workspace_id)) => CrossWorkspaceLookup::KnownPeerMissingSymbol { workspace_id },
            _ => CrossWorkspaceLookup::Unknown,
        }
    }
}

/// Replace `_` with `-` in the leftmost `::`-segment of a module
/// FQDN. Used by [`DbCrossWorkspaceResolver::compute`] to bridge the
/// Rust source convention (`use my_crate::*`) with the Cargo +
/// IR storage convention (`my-crate`). Deeper segments stay literal
/// because Rust module names are underscore-native (e.g.
/// `standardoc-core::pipeline::provider`).
fn normalize_crate_prefix_to_hyphen(module: &str) -> String {
    let (head, tail) = match module.split_once("::") {
        Some((h, t)) => (h, Some(t)),
        None => (module, None),
    };
    if !head.contains('_') {
        return module.to_string();
    }
    let head_norm = head.replace('_', "-");
    match tail {
        Some(t) => format!("{head_norm}::{t}"),
        None => head_norm,
    }
}

impl CrossWorkspaceResolver for DbCrossWorkspaceResolver<'_> {
    fn resolve(&self, origin_module: &str, origin_symbol: &str) -> CrossWorkspaceLookup {
        let key = (origin_module.to_string(), origin_symbol.to_string());
        if let Ok(guard) = self.cache.lock()
            && let Some(cached) = guard.get(&key)
        {
            return cached.clone();
        }
        let computed = self.compute(origin_module, origin_symbol);
        if let Ok(mut guard) = self.cache.lock() {
            guard.insert(key, computed.clone());
        }
        computed
    }
}

#[cfg(test)]
mod tests;

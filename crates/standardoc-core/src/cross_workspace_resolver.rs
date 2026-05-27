//! Stage 3 R3 — DB-backed `CrossWorkspaceResolver` for the visitor.
//!
//! The resolver answers `(origin_module, origin_symbol) -> CrossWorkspaceLookup`
//! at extract time so cross-workspace edges materialise in the
//! [`ExtractedFile`] payload (Resolved-with-peer-attrs / typed
//! UnresolvedBridge) rather than only at MCP query time.
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
        let Ok(pool) = self.handle.pool() else {
            return CrossWorkspaceLookup::Unknown;
        };
        let Ok(conn) = pool.get() else {
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
mod tests {
    use super::*;
    use crate::storage::module_lookup::put_module_lookup;
    use standardoc_ir::{BindingSource, IdentResolution, Language, LocalDeclKind, ModuleLookup};
    use tempfile::tempdir;

    fn lookup_with_top_level_symbol(module_fqdn: &str, symbol: &str) -> ModuleLookup {
        let mut m = ModuleLookup::new(module_fqdn.into(), Language::Rust);
        m.push_binding(IdentResolution {
            name: symbol.into(),
            source: BindingSource::LocalDecl {
                decl_kind: LocalDeclKind::Struct,
            },
            resolved_fqdn: None,
            aliases_to: None,
            mutability: None,
            scope_idx: ModuleLookup::ROOT_SCOPE,
            attributes: vec![],
            ir_kind: None,
        });
        m
    }

    fn primary_handle() -> (tempfile::TempDir, IndexHandle) {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        (dir, handle)
    }

    #[test]
    fn hit_when_peer_workspace_exports_symbol() {
        let (_dir, handle) = primary_handle();
        let pool = handle.pool().unwrap();
        let conn = pool.get().unwrap();
        let lookup = lookup_with_top_level_symbol("peer_ws::lib", "Foo");
        put_module_lookup(&conn, "peer-uuid", &lookup).unwrap();
        drop(conn);

        let resolver = DbCrossWorkspaceResolver::new(&handle);
        let result = resolver.resolve("peer_ws::lib", "Foo");
        assert_eq!(
            result,
            CrossWorkspaceLookup::Hit {
                workspace_id: "peer-uuid".into(),
                fqdn: "peer_ws::lib::Foo".into(),
            }
        );
    }

    #[test]
    fn known_peer_missing_symbol_when_module_present_but_symbol_absent() {
        let (_dir, handle) = primary_handle();
        let pool = handle.pool().unwrap();
        let conn = pool.get().unwrap();
        let lookup = lookup_with_top_level_symbol("peer_ws::lib", "Bar");
        put_module_lookup(&conn, "peer-uuid", &lookup).unwrap();
        drop(conn);

        let resolver = DbCrossWorkspaceResolver::new(&handle);
        let result = resolver.resolve("peer_ws::lib", "Foo");
        assert_eq!(
            result,
            CrossWorkspaceLookup::KnownPeerMissingSymbol {
                workspace_id: "peer-uuid".into(),
            }
        );
    }

    #[test]
    fn unknown_when_no_peer_owns_module() {
        let (_dir, handle) = primary_handle();
        let resolver = DbCrossWorkspaceResolver::new(&handle);
        let result = resolver.resolve("totally::external", "Foo");
        assert_eq!(result, CrossWorkspaceLookup::Unknown);
    }

    #[test]
    fn hit_when_only_hyphenated_form_matches_peer_module() {
        // Rust source writes `use standardoc_core::Foo` but the peer
        // workspace stores its ModuleLookup keyed on the Cargo-style
        // hyphenated name `standardoc-core`. The resolver must fall
        // back to the hyphenated form on miss.
        let (_dir, handle) = primary_handle();
        let pool = handle.pool().unwrap();
        let conn = pool.get().unwrap();
        let lookup = lookup_with_top_level_symbol("standardoc-core", "Foo");
        put_module_lookup(&conn, "peer-uuid", &lookup).unwrap();
        drop(conn);

        let resolver = DbCrossWorkspaceResolver::new(&handle);
        let result = resolver.resolve("standardoc_core", "Foo");
        assert_eq!(
            result,
            CrossWorkspaceLookup::Hit {
                workspace_id: "peer-uuid".into(),
                fqdn: "standardoc-core::Foo".into(),
            }
        );
    }

    #[test]
    fn hit_returns_canonical_fqdn_when_binding_has_resolved_fqdn() {
        // Re-export binding: `pub use crate::pipeline::provider::Foo`
        // stamps `resolved_fqdn` on the IdentResolution pointing at the
        // canonical FQDN. The resolver must prefer it over the naive
        // `<origin_module>::<origin_symbol>` re-export FQDN so consumers
        // querying the canonical (`...::pipeline::provider::Foo`) see
        // the dependents materialise.
        let (_dir, handle) = primary_handle();
        let pool = handle.pool().unwrap();
        let conn = pool.get().unwrap();
        let mut lookup = ModuleLookup::new("standardoc-core".into(), Language::Rust);
        lookup.push_binding(IdentResolution {
            name: "Foo".into(),
            source: BindingSource::Import {
                module_path: "crate::pipeline::provider".into(),
                original_name: None,
                is_type_only: false,
                is_re_export: true,
            },
            resolved_fqdn: Some("standardoc-core::pipeline::provider::Foo".into()),
            aliases_to: None,
            mutability: None,
            scope_idx: ModuleLookup::ROOT_SCOPE,
            attributes: vec![],
            ir_kind: None,
        });
        put_module_lookup(&conn, "peer-uuid", &lookup).unwrap();
        drop(conn);

        let resolver = DbCrossWorkspaceResolver::new(&handle);
        let result = resolver.resolve("standardoc-core", "Foo");
        assert_eq!(
            result,
            CrossWorkspaceLookup::Hit {
                workspace_id: "peer-uuid".into(),
                fqdn: "standardoc-core::pipeline::provider::Foo".into(),
            }
        );
    }

    #[test]
    fn hit_when_only_primary_workspace_owns_module() {
        // Mono-repo case: no peer workspaces linked, the target module
        // lives in a sibling crate within the same (primary) workspace.
        // Pre-fix this returned Unknown because the resolver only
        // queried `workspace_id != primary`.
        let (_dir, handle) = primary_handle();
        let pool = handle.pool().unwrap();
        let conn = pool.get().unwrap();
        let lookup = lookup_with_top_level_symbol("standardoc-ir", "FfiAbi");
        // Note: put_module_lookup with no explicit workspace defaults to PRIMARY.
        put_module_lookup(&conn, "primary", &lookup).unwrap();
        drop(conn);

        let resolver = DbCrossWorkspaceResolver::new(&handle);
        let result = resolver.resolve("standardoc-ir", "FfiAbi");
        assert_eq!(
            result,
            CrossWorkspaceLookup::Hit {
                workspace_id: "primary".into(),
                fqdn: "standardoc-ir::FfiAbi".into(),
            }
        );
    }

    #[test]
    fn hit_when_underscore_form_resolves_via_primary_after_normalization() {
        // Combined Bug A + intra-workspace: source uses underscored
        // `standardoc_ir`, primary has hyphenated `standardoc-ir`,
        // no peers. Fix chain: normalize in compute() → intra-workspace
        // primary query hits.
        let (_dir, handle) = primary_handle();
        let pool = handle.pool().unwrap();
        let conn = pool.get().unwrap();
        let lookup = lookup_with_top_level_symbol("standardoc-ir", "FfiAbi");
        put_module_lookup(&conn, "primary", &lookup).unwrap();
        drop(conn);

        let resolver = DbCrossWorkspaceResolver::new(&handle);
        let result = resolver.resolve("standardoc_ir", "FfiAbi");
        assert_eq!(
            result,
            CrossWorkspaceLookup::Hit {
                workspace_id: "primary".into(),
                fqdn: "standardoc-ir::FfiAbi".into(),
            }
        );
    }

    #[test]
    fn cache_amortises_repeated_lookups() {
        let (_dir, handle) = primary_handle();
        let pool = handle.pool().unwrap();
        let conn = pool.get().unwrap();
        let lookup = lookup_with_top_level_symbol("peer_ws::lib", "Foo");
        put_module_lookup(&conn, "peer-uuid", &lookup).unwrap();
        drop(conn);

        let resolver = DbCrossWorkspaceResolver::new(&handle);
        // First call populates cache.
        let first = resolver.resolve("peer_ws::lib", "Foo");
        // Hot-cache call returns the same value WITHOUT re-querying the DB.
        let second = resolver.resolve("peer_ws::lib", "Foo");
        assert_eq!(first, second);
        // Cache slot exists.
        let guard = resolver.cache.lock().unwrap();
        assert!(guard.contains_key(&("peer_ws::lib".into(), "Foo".into())));
    }
}

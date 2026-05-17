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
    peer_workspace_for_module, resolve_cross_workspace_import,
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

    /// Compute the lookup without touching the cache — exposed for the
    /// internal trait impl. Storage errors collapse to `Unknown`.
    fn compute(&self, origin_module: &str, origin_symbol: &str) -> CrossWorkspaceLookup {
        let Ok(pool) = self.handle.pool() else {
            return CrossWorkspaceLookup::Unknown;
        };
        let Ok(conn) = pool.get() else {
            return CrossWorkspaceLookup::Unknown;
        };
        match resolve_cross_workspace_import(&conn, origin_module, origin_symbol) {
            Ok(Some(resolution)) => CrossWorkspaceLookup::Hit {
                workspace_id: resolution.workspace_id,
                fqdn: resolution.resolved_fqdn,
            },
            Ok(None) => match peer_workspace_for_module(&conn, origin_module) {
                Ok(Some(workspace_id)) => {
                    CrossWorkspaceLookup::KnownPeerMissingSymbol { workspace_id }
                }
                _ => CrossWorkspaceLookup::Unknown,
            },
            Err(_) => CrossWorkspaceLookup::Unknown,
        }
    }
}

impl CrossWorkspaceResolver for DbCrossWorkspaceResolver<'_> {
    fn resolve(&self, origin_module: &str, origin_symbol: &str) -> CrossWorkspaceLookup {
        let key = (origin_module.to_string(), origin_symbol.to_string());
        if let Ok(guard) = self.cache.lock() {
            if let Some(cached) = guard.get(&key) {
                return cached.clone();
            }
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
    use standardoc_ir::{
        BindingSource, IdentResolution, Language, LocalDeclKind, ModuleLookup,
    };
    use tempfile::tempdir;

    fn lookup_with_top_level_symbol(
        module_fqdn: &str,
        symbol: &str,
    ) -> ModuleLookup {
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

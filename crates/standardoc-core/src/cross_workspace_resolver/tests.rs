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

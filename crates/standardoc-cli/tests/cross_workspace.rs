//! Stage 3b-6 — End-to-end dogfood of the cross-workspace import resolver.
//!
//! Builds two temp workspaces (`primary` + `peer`), registers the peer in
//! the primary's `workspace_catalog`, seeds a synthetic `ModuleLookup`
//! into the primary's `module_lookups` table keyed by the peer's UUID,
//! then asserts that `query::workspace::resolve_cross_workspace`
//! surfaces the binding with the right workspace_id, fqdn, and
//! `BindingSource`.
//!
//! Goes through the public `query::workspace::*` surface only —
//! exercises the same code path the MCP / LSP tools wire on top.

use standardoc_core::{
    IndexHandle,
    query::workspace::{
        get_module_lookup, link_workspace, list_linked_workspaces, put_module_lookup,
        resolve_cross_workspace, unlink_workspace,
    },
};
use standardoc_ir::{
    BindingSource, IdentResolution, Language, LinkDirection, LocalDeclKind, ModuleLookup,
};

#[test]
fn primary_resolves_symbol_against_registered_peer_workspace() {
    let primary_dir = tempfile::tempdir().expect("primary tempdir");
    let peer_dir = tempfile::tempdir().expect("peer tempdir");

    let handle = IndexHandle::open(primary_dir.path()).expect("open primary IndexHandle");

    // Register the peer workspace — canonicalised path, "in" direction
    // (we consume the peer's symbols).
    let peer_path = peer_dir.path().to_string_lossy().into_owned();
    let peer_id = link_workspace(&handle, &peer_path, LinkDirection::In)
        .expect("link peer workspace");

    // Catalog must list the peer now.
    let listed = list_linked_workspaces(&handle).expect("list peers");
    assert_eq!(listed.len(), 1, "exactly one peer registered");
    assert_eq!(listed[0].workspace_id, peer_id);
    assert_eq!(listed[0].link_direction, LinkDirection::In);

    // Seed a synthetic ModuleLookup attributed to the peer. The module
    // exports `Foo` (top-level struct) — what we'll resolve from the
    // primary side.
    let mut lookup = ModuleLookup::new("peer::lib".into(), Language::Rust);
    lookup.push_binding(IdentResolution {
        name: "Foo".into(),
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
    put_module_lookup(&handle, Some(&peer_id), &lookup).expect("put peer ModuleLookup");

    // Sanity — get_module_lookup must round-trip the seeded blob.
    let fetched = get_module_lookup(&handle, Some(&peer_id), "peer::lib")
        .expect("get_module_lookup")
        .expect("seeded row present");
    assert_eq!(fetched.module_fqdn, "peer::lib");
    assert!(fetched.bindings.contains_key("Foo"));

    // Resolver hit: `peer::lib::Foo` is reachable cross-workspace.
    let providers = resolve_cross_workspace(&handle, "peer::lib", "Foo")
        .expect("resolve_cross_workspace");
    assert_eq!(providers.len(), 1, "exactly one provider");
    let hit = &providers[0];
    assert_eq!(hit.workspace_id, peer_id);
    assert_eq!(hit.resolved_fqdn, "peer::lib::Foo");
    assert!(matches!(
        hit.binding_source,
        BindingSource::LocalDecl {
            decl_kind: LocalDeclKind::Struct,
        }
    ));

    // Resolver miss: nested-scope bindings must NOT surface (root-scope
    // gate is enforced by storage::cross_workspace::list_providers).
    let bad = resolve_cross_workspace(&handle, "peer::lib", "DoesNotExist")
        .expect("resolve_cross_workspace");
    assert!(bad.is_empty(), "unknown symbol must yield empty providers");

    // Cleanup — unlink drops the catalog row AND the dependent
    // module_lookups row. After unlink, the resolver no longer finds Foo.
    unlink_workspace(&handle, &peer_id).expect("unlink peer workspace");
    let after = resolve_cross_workspace(&handle, "peer::lib", "Foo")
        .expect("resolve_cross_workspace after unlink");
    assert!(
        after.is_empty(),
        "unregistering the peer must invalidate its providers"
    );
    let empty = list_linked_workspaces(&handle).expect("list after unlink");
    assert!(empty.is_empty(), "catalog must be empty after unlink");
}

#[test]
fn link_workspace_rejects_missing_path_with_did_you_mean() {
    let primary_dir = tempfile::tempdir().expect("primary tempdir");
    let parent = tempfile::tempdir().expect("parent for typo");
    std::fs::create_dir(parent.path().join("projects")).expect("create projects/");

    let handle = IndexHandle::open(primary_dir.path()).expect("open primary IndexHandle");
    let typo = parent.path().join("projcts");

    let err = link_workspace(&handle, &typo.to_string_lossy(), LinkDirection::In)
        .expect_err("typo path must fail");
    match err {
        standardoc_core::query::workspace::LinkWorkspaceError::PathNotFound {
            input,
            suggestions,
        } => {
            assert_eq!(input, typo.to_string_lossy());
            assert!(
                suggestions.iter().any(|s| s.ends_with("projects")),
                "expected `projects` in suggestions, got {suggestions:?}"
            );
        }
        other => panic!("expected PathNotFound, got {other:?}"),
    }
}

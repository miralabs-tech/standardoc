use super::*;

fn fresh() -> ModuleLookup {
    ModuleLookup::new("test::mod".into(), Language::Rust)
}

fn local_decl(name: &str, scope: u32, kind: LocalDeclKind) -> IdentResolution {
    IdentResolution {
        name: name.into(),
        source: BindingSource::LocalDecl { decl_kind: kind },
        resolved_fqdn: None,
        aliases_to: None,
        mutability: None,
        scope_idx: scope,
        attributes: vec![],
        ir_kind: None,
    }
}

#[test]
fn root_scope_present_by_default() {
    let m = fresh();
    assert_eq!(m.scopes.len(), 1);
    assert_eq!(m.scopes[0].parent, None);
    assert!(matches!(m.scopes[0].kind, ScopeKind::Module));
}

#[test]
fn resolve_local_walks_parent_chain() {
    let mut m = fresh();
    let inner = m.push_scope(ScopeRange {
        start_line: 10,
        end_line: 20,
        parent: Some(ModuleLookup::ROOT_SCOPE),
        kind: ScopeKind::Function,
    });
    m.push_binding(local_decl(
        "foo",
        ModuleLookup::ROOT_SCOPE,
        LocalDeclKind::Function,
    ));
    let r = m
        .resolve_local("foo", inner)
        .expect("should find via parent");
    assert_eq!(r.scope_idx, ModuleLookup::ROOT_SCOPE);
}

#[test]
fn resolve_local_prefers_inner_shadow() {
    let mut m = fresh();
    let inner = m.push_scope(ScopeRange {
        start_line: 10,
        end_line: 20,
        parent: Some(ModuleLookup::ROOT_SCOPE),
        kind: ScopeKind::Function,
    });
    m.push_binding(local_decl(
        "foo",
        ModuleLookup::ROOT_SCOPE,
        LocalDeclKind::Const,
    ));
    m.push_binding(local_decl("foo", inner, LocalDeclKind::Let));
    let r = m.resolve_local("foo", inner).expect("inner shadow wins");
    assert_eq!(r.scope_idx, inner);
    assert!(matches!(
        r.source,
        BindingSource::LocalDecl {
            decl_kind: LocalDeclKind::Let
        }
    ));
}

#[test]
fn resolve_local_misses_when_no_binding() {
    let m = fresh();
    assert!(m.resolve_local("nope", ModuleLookup::ROOT_SCOPE).is_none());
}

#[test]
fn resolve_local_misses_when_binding_outside_chain() {
    let mut m = fresh();
    let sibling_a = m.push_scope(ScopeRange {
        start_line: 1,
        end_line: 5,
        parent: Some(ModuleLookup::ROOT_SCOPE),
        kind: ScopeKind::Function,
    });
    let sibling_b = m.push_scope(ScopeRange {
        start_line: 10,
        end_line: 15,
        parent: Some(ModuleLookup::ROOT_SCOPE),
        kind: ScopeKind::Function,
    });
    m.push_binding(local_decl("foo", sibling_a, LocalDeclKind::Let));
    assert!(m.resolve_local("foo", sibling_b).is_none());
}

#[test]
fn custom_variants_round_trip_via_serde_json() {
    let custom_kind = LocalDeclKind::Custom {
        lang: Language::Rust,
        tag: "ust:my-decl".into(),
    };
    let s = serde_json::to_string(&custom_kind).unwrap();
    let back: LocalDeclKind = serde_json::from_str(&s).unwrap();
    assert_eq!(custom_kind, back);

    let custom_tag = BuiltinTag::Custom {
        tag: "ust:net-stream".into(),
    };
    let s = serde_json::to_string(&custom_tag).unwrap();
    let back: BuiltinTag = serde_json::from_str(&s).unwrap();
    assert_eq!(custom_tag, back);

    let custom_substrate = Substrate::Custom {
        tag: "ust:ruby-vm".into(),
    };
    let s = serde_json::to_string(&custom_substrate).unwrap();
    let back: Substrate = serde_json::from_str(&s).unwrap();
    assert_eq!(custom_substrate, back);
}

#[test]
fn ir2_widened_substrate_variants_round_trip_via_serde_json() {
    for v in [
        Substrate::Browser,
        Substrate::Node,
        Substrate::Database,
        Substrate::MessageBus,
        Substrate::Kernel,
        Substrate::Hypervisor,
    ] {
        let s = serde_json::to_string(&v).unwrap();
        let back: Substrate = serde_json::from_str(&s).unwrap();
        assert_eq!(v, back, "round-trip mismatch for {v:?}");
    }
}

#[test]
fn ir2_widened_substrate_variants_serialize_to_snake_case() {
    assert_eq!(
        serde_json::to_string(&Substrate::Browser).unwrap(),
        "\"browser\""
    );
    assert_eq!(serde_json::to_string(&Substrate::Node).unwrap(), "\"node\"");
    assert_eq!(
        serde_json::to_string(&Substrate::Database).unwrap(),
        "\"database\""
    );
    // `MessageBus` is the case that exercises snake_case lowering;
    // a regression here would silently re-tag the variant in stored
    // `SubstrateBridge` JSON.
    assert_eq!(
        serde_json::to_string(&Substrate::MessageBus).unwrap(),
        "\"message_bus\""
    );
    assert_eq!(
        serde_json::to_string(&Substrate::Kernel).unwrap(),
        "\"kernel\""
    );
    assert_eq!(
        serde_json::to_string(&Substrate::Hypervisor).unwrap(),
        "\"hypervisor\""
    );
}

#[test]
fn ir2_widened_substrate_variants_are_distinct_from_custom_tag_form() {
    // First-class variants must NOT compare equal to their `Custom`
    // tag-string equivalents — equality/hash distinguish so the
    // `SubstrateBridge` (from, to) lookup keys don't collide between
    // `Browser` and `Custom { tag: "browser" }`.
    assert_ne!(
        Substrate::Browser,
        Substrate::Custom {
            tag: "browser".into()
        }
    );
    assert_ne!(
        Substrate::Database,
        Substrate::Custom {
            tag: "database".into()
        }
    );
    assert_ne!(
        Substrate::MessageBus,
        Substrate::Custom {
            tag: "message_bus".into()
        }
    );
}

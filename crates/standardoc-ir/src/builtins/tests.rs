use super::*;

#[test]
fn synthetic_fqdn_scheme() {
    assert_eq!(
        make_synthetic_fqdn(Language::JavaScript, "JSON.parse"),
        "<builtin>::js::JSON.parse"
    );
    assert_eq!(
        make_synthetic_fqdn(Language::Rust, "Vec::new"),
        "<builtin>::rust::Vec::new"
    );
    assert_eq!(
        make_synthetic_fqdn(Language::Lua, "table.insert"),
        "<builtin>::lua::table.insert"
    );
}

#[test]
fn synthetic_fqdn_prefix_cannot_be_valid_identifier() {
    // `<` and `>` are invalid in identifiers across every language we
    // extract — so the synthetic fqdn can never collide with a real
    // user-defined symbol.
    let fqdn = make_synthetic_fqdn(Language::TypeScript, "Promise");
    assert!(fqdn.starts_with("<builtin>::"));
    assert!(fqdn.contains('<'));
    assert!(fqdn.contains('>'));
}

#[test]
fn lookup_native_then_user_extension() {
    let mut reg = BuiltinRegistry::new();
    reg.register(BuiltinEntry::new(
        "JSON.parse",
        Language::JavaScript,
        Kind::Callable,
        BuiltinTag::Decode,
        BuiltinTier::Edge,
    ));
    reg.register_user(BuiltinEntry::new(
        "myCustomGlobal",
        Language::JavaScript,
        Kind::Value,
        BuiltinTag::Custom {
            tag: "ust:user-defined".into(),
        },
        BuiltinTier::Edge,
    ));

    let native = reg
        .lookup("JSON.parse", Language::JavaScript)
        .expect("native builtin present");
    assert_eq!(native.synthetic_fqdn, "<builtin>::js::JSON.parse");

    let user = reg
        .lookup("myCustomGlobal", Language::JavaScript)
        .expect("user extension reachable via lookup");
    assert!(matches!(user.tag, BuiltinTag::Custom { .. }));

    assert!(reg.lookup("nope", Language::JavaScript).is_none());
    assert!(reg.lookup("JSON.parse", Language::Rust).is_none());
}

#[test]
fn lookup_bridge_matches_by_substrate_pair_and_source_name() {
    let mut reg = BuiltinRegistry::new();
    reg.register_bridge(SubstrateBridge {
        from: Substrate::native(Language::JavaScript),
        to: Substrate::native(Language::Rust),
        bridge_kind: BridgeKind::new("napi"),
        mappings: vec![BridgeMapping {
            source_name: "fs.readFileSync".into(),
            target_fqdn: "my_crate::fs::read_file_sync".into(),
        }],
    });

    let hit = reg
        .lookup_bridge(
            &Substrate::native(Language::JavaScript),
            &Substrate::native(Language::Rust),
            "fs.readFileSync",
        )
        .expect("bridge mapping reachable");
    assert_eq!(hit.target_fqdn, "my_crate::fs::read_file_sync");

    assert!(
        reg.lookup_bridge(
            &Substrate::native(Language::Rust),
            &Substrate::native(Language::JavaScript),
            "fs.readFileSync",
        )
        .is_none()
    );
}

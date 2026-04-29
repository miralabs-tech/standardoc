use standardoc_ir::{
    Blake3Hash, BridgeKind, EdgeKind, ExtractedFile, Kind, Language, LanguageKind, Modifiers,
    Param, RawAttribute, RawAttributeArg, RawCallArg, RawCallSite, RawEdge, RawSymbol,
    ResolvedOrUnresolved, Signature, SignatureMeta, Site, SourceOrigin, SymbolLocation, TypeRef,
    Visibility,
};

fn fixture() -> ExtractedFile {
    let create_user = RawSymbol {
        name: "create_user".into(),
        fqdn: "crate::auth::create_user".into(),
        kind: Kind::Function,
        language_kind: LanguageKind::from("function"),
        module: Some("auth".into()),
        visibility: Visibility::Public,
        location: SymbolLocation {
            file: "src/auth.rs".into(),
            start_line: 12,
            end_line: 30,
            start_col: 0,
            end_col: 1,
        },
        signature: Some(Signature {
            params: vec![Param {
                name: "email".into(),
                ty: TypeRef::new("&str"),
                default: None,
            }],
            returns: Some(TypeRef::new("Result<User, Error>")),
            modifiers: Modifiers {
                is_async: true,
                deprecated: None,
                generic_params: vec![],
            },
            meta: SignatureMeta {
                exposed_via: Some(BridgeKind::from("tauri")),
            },
        }),
        body_hash: Some(Blake3Hash::new([0x42; 32])),
        attributes: vec![RawAttribute {
            name: "tauri::command".into(),
            args: vec![RawAttributeArg {
                key: None,
                value: "create_user".into(),
                is_string_literal: true,
            }],
            site: Site {
                file: "src/auth.rs".into(),
                line: 11,
                col: 0,
            },
        }],
    };

    let module_root = RawSymbol {
        name: "auth".into(),
        fqdn: "crate::auth".into(),
        kind: Kind::Module,
        language_kind: LanguageKind::from("module"),
        module: None,
        visibility: Visibility::Crate,
        location: SymbolLocation {
            file: "src/auth.rs".into(),
            start_line: 1,
            end_line: 100,
            start_col: 0,
            end_col: 0,
        },
        signature: None,
        body_hash: Some(Blake3Hash::new([0x01; 32])),
        attributes: vec![],
    };

    let calls_edge = RawEdge {
        from_fqdn: "crate::auth::create_user".into(),
        kind: EdgeKind::Calls,
        to: ResolvedOrUnresolved::Resolved {
            fqdn: "crate::db::insert".into(),
        },
        sites: vec![Site {
            file: "src/auth.rs".into(),
            line: 18,
            col: 8,
        }],
    };

    let unresolved_edge = RawEdge {
        from_fqdn: "crate::auth::create_user".into(),
        kind: EdgeKind::UsesType,
        to: ResolvedOrUnresolved::Unresolved {
            name: "User".into(),
        },
        sites: vec![],
    };

    let bridge_edge = RawEdge {
        from_fqdn: "frontend::login".into(),
        kind: EdgeKind::Calls,
        to: ResolvedOrUnresolved::UnresolvedBridge {
            bridge: BridgeKind::from("tauri"),
            name: "create_user".into(),
        },
        sites: vec![],
    };

    let call_site = RawCallSite {
        from_fqdn: "frontend::login".into(),
        callee_text: "tauri::invoke".into(),
        args: vec![RawCallArg {
            value: "create_user".into(),
            is_string_literal: true,
        }],
        site: Site {
            file: "src/login.ts".into(),
            line: 5,
            col: 4,
        },
    };

    ExtractedFile {
        file: "src/auth.rs".into(),
        language: Language::Rust,
        source_origin: SourceOrigin::Workspace,
        is_external: false,
        content_hash: Blake3Hash::new([0x55; 32]),
        byte_size: 1024,
        symbols: vec![module_root, create_user],
        edges: vec![calls_edge, unresolved_edge, bridge_edge],
        call_sites: vec![call_site],
    }
}

#[test]
fn full_extracted_file_round_trip() {
    let f = fixture();
    let json = serde_json::to_string_pretty(&f).unwrap();
    let back: ExtractedFile = serde_json::from_str(&json).unwrap();
    assert_eq!(f, back);
}

#[test]
fn keys_match_storage_conventions() {
    let f = fixture();
    let json = serde_json::to_string(&f).unwrap();

    assert!(json.contains("\"language\":\"rust\""));
    assert!(json.contains("\"source_origin\":\"workspace\""));
    assert!(json.contains("\"kind\":\"function\""));
    assert!(json.contains("\"kind\":\"module\""));
    assert!(json.contains("\"visibility\":\"public\""));
    assert!(json.contains("\"visibility\":\"crate\""));
    assert!(json.contains("\"kind\":\"CALLS\""));
    assert!(json.contains("\"kind\":\"USES_TYPE\""));
    assert!(json.contains("\"kind\":\"resolved\""));
    assert!(json.contains("\"kind\":\"unresolved\""));
    assert!(json.contains("\"kind\":\"unresolved_bridge\""));
    assert!(json.contains("\"async\":true"));
    assert!(!json.contains("is_async"));
}

#[test]
fn missing_optional_vec_fields_default_to_empty() {
    let json = r#"{
        "file": "src/empty.rs",
        "language": "rust",
        "source_origin": "workspace",
        "is_external": false,
        "content_hash": "0000000000000000000000000000000000000000000000000000000000000000",
        "byte_size": 0
    }"#;
    let parsed: ExtractedFile = serde_json::from_str(json).unwrap();
    assert!(parsed.symbols.is_empty());
    assert!(parsed.edges.is_empty());
    assert!(parsed.call_sites.is_empty());
}

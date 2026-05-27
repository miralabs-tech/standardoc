
use super::*;
use standardoc_ir::{BridgeKind, Modifiers, Param, Signature, TypeRef};

#[test]
fn signature_round_trip_through_json() {
    let sig = Signature {
        params: vec![Param {
            name: "x".into(),
            ty: TypeRef::new("u32"),
            default: None,
        }],
        returns: Some(TypeRef::new("u32")),
        modifiers: Modifiers {
            is_async: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let json = signature_to_json(&sig).unwrap();
    let back = json_to_signature(&json).unwrap();
    assert_eq!(sig, back);
}

#[test]
fn signature_default_round_trip() {
    let sig = Signature::default();
    let json = signature_to_json(&sig).unwrap();
    let back = json_to_signature(&json).unwrap();
    assert_eq!(sig, back);
}

#[test]
fn json_to_signature_invalid_returns_json_error() {
    let err = json_to_signature("not json").unwrap_err();
    assert!(matches!(err, StorageError::Json(_)));
}

#[test]
fn unresolved_to_storage_resolved_returns_none() {
    let target = ResolvedOrUnresolved::Resolved {
        fqdn: "crate::a::foo".into(),
    };
    assert_eq!(unresolved_to_storage(&target).unwrap(), None);
}

#[test]
fn unresolved_to_storage_unresolved_returns_name() {
    let target = ResolvedOrUnresolved::Unresolved {
        name: "do_thing".into(),
    };
    assert_eq!(
        unresolved_to_storage(&target).unwrap().as_deref(),
        Some("do_thing")
    );
}

#[test]
fn unresolved_to_storage_bridge_concatenates() {
    let target = ResolvedOrUnresolved::UnresolvedBridge {
        bridge: BridgeKind::from("tauri"),
        name: "create_user".into(),
    };
    assert_eq!(
        unresolved_to_storage(&target).unwrap().as_deref(),
        Some("tauri::create_user")
    );
}

#[test]
fn unresolved_to_storage_bridge_custom_prefix_passes_validation() {
    let target = ResolvedOrUnresolved::UnresolvedBridge {
        bridge: BridgeKind::from("custom:internal-rpc"),
        name: "ping".into(),
    };
    assert_eq!(
        unresolved_to_storage(&target).unwrap().as_deref(),
        Some("custom:internal-rpc::ping")
    );
}

#[test]
fn unresolved_to_storage_rejects_unknown_bridge_slug() {
    // IR-1: a slug outside BUILTIN_BRIDGE_KINDS that lacks the
    // `custom:` prefix must be refused at the storage boundary,
    // not silently persisted.
    let target = ResolvedOrUnresolved::UnresolvedBridge {
        bridge: BridgeKind::from("tauri-v2"),
        name: "create_user".into(),
    };
    let err = unresolved_to_storage(&target).unwrap_err();
    assert!(
        matches!(err, StorageError::BridgeKindInvalid(_)),
        "got `{err:?}`"
    );
}

#[test]
fn signature_to_json_rejects_unknown_exposed_via_slug() {
    let sig = Signature {
        meta: standardoc_ir::SignatureMeta {
            exposed_via: vec![BridgeKind::from("tauri-v2")],
        },
        ..Default::default()
    };
    let err = signature_to_json(&sig).unwrap_err();
    assert!(
        matches!(err, StorageError::BridgeKindInvalid(_)),
        "got `{err:?}`"
    );
}

#[test]
fn signature_to_json_accepts_builtin_exposed_via_slug() {
    let sig = Signature {
        meta: standardoc_ir::SignatureMeta {
            exposed_via: vec![BridgeKind::from("tauri")],
        },
        ..Default::default()
    };
    let json = signature_to_json(&sig).unwrap();
    assert!(json.contains("\"tauri\""), "got `{json}`");
}

#[test]
fn signature_to_json_accepts_multiple_exposed_via_bridges() {
    // IR-3 dual-target shape — same fn surfaced via both Tauri and
    // wasm-bindgen. Both must round-trip into the JSON array.
    let sig = Signature {
        meta: standardoc_ir::SignatureMeta {
            exposed_via: vec![BridgeKind::from("tauri"), BridgeKind::from("wasm-bindgen")],
        },
        ..Default::default()
    };
    let json = signature_to_json(&sig).unwrap();
    assert!(json.contains("\"tauri\""), "got `{json}`");
    assert!(json.contains("\"wasm-bindgen\""), "got `{json}`");
}

#[test]
fn signature_to_json_rejects_when_any_exposed_via_bridge_is_invalid() {
    // Short-circuits on the first invalid slug — but a partial
    // emission (valid + invalid mixed) must still fail rather than
    // landing only the valid slug.
    let sig = Signature {
        meta: standardoc_ir::SignatureMeta {
            exposed_via: vec![
                BridgeKind::from("tauri"),
                BridgeKind::from("totally-made-up"),
            ],
        },
        ..Default::default()
    };
    let err = signature_to_json(&sig).unwrap_err();
    assert!(
        matches!(err, StorageError::BridgeKindInvalid(_)),
        "got `{err:?}`"
    );
}

#[test]
fn language_round_trip_all_variants() {
    for lang in [
        Language::Rust,
        Language::TypeScript,
        Language::JavaScript,
        Language::Lua,
        Language::Vue,
        Language::Svelte,
        Language::C,
    ] {
        let s = language_to_sql_text(lang);
        let back = language_from_sql_text(s).unwrap();
        assert_eq!(lang, back);
    }
}

#[test]
fn language_to_sql_text_known_lowercase() {
    assert_eq!(language_to_sql_text(Language::Rust), "rust");
    assert_eq!(language_to_sql_text(Language::TypeScript), "typescript");
    assert_eq!(language_to_sql_text(Language::JavaScript), "javascript");
    assert_eq!(language_to_sql_text(Language::Lua), "lua");
    assert_eq!(language_to_sql_text(Language::Vue), "vue");
    assert_eq!(language_to_sql_text(Language::Svelte), "svelte");
    assert_eq!(language_to_sql_text(Language::C), "c");
}

#[test]
fn language_from_sql_text_unknown_is_invalid_stored_data() {
    let err = language_from_sql_text("banana").unwrap_err();
    assert!(matches!(err, StorageError::InvalidStoredData { .. }));
}

#[test]
fn language_from_sql_text_uppercase_is_invalid() {
    let err = language_from_sql_text("Rust").unwrap_err();
    assert!(matches!(err, StorageError::InvalidStoredData { .. }));
}

#[test]
fn kind_round_trip_all_variants() {
    for k in [
        Kind::Callable,
        Kind::Type,
        Kind::Value,
        Kind::Module,
        Kind::Macro,
    ] {
        let s = kind_to_sql_text(k);
        assert_eq!(kind_from_sql_text(s).unwrap(), k);
    }
}

#[test]
fn kind_to_sql_text_lowercase() {
    assert_eq!(kind_to_sql_text(Kind::Callable), "callable");
    assert_eq!(kind_to_sql_text(Kind::Macro), "macro");
}

#[test]
fn kind_from_sql_text_unknown_is_invalid() {
    let err = kind_from_sql_text("class").unwrap_err();
    assert!(matches!(err, StorageError::InvalidStoredData { .. }));
}

#[test]
fn decl_kind_round_trip_built_in_variants() {
    for d in [
        DeclKind::Module,
        DeclKind::Namespace,
        DeclKind::Crate,
        DeclKind::Struct,
        DeclKind::Enum,
        DeclKind::Union,
        DeclKind::Class,
        DeclKind::Interface,
        DeclKind::TypeAlias,
        DeclKind::Function,
        DeclKind::Method,
        DeclKind::Constructor,
        DeclKind::Getter,
        DeclKind::Setter,
        DeclKind::Const,
        DeclKind::Static,
        DeclKind::Var,
        DeclKind::Field,
        DeclKind::EnumVariant,
        DeclKind::DeclarativeMacro,
        DeclKind::ProcMacro,
        DeclKind::Decorator,
    ] {
        let s = decl_kind_to_sql_text(&d);
        assert_eq!(decl_kind_from_sql_text(&s).unwrap(), d, "round-trip {s:?}");
    }
}

#[test]
fn decl_kind_custom_round_trip() {
    let d = DeclKind::Custom {
        lang: Language::Rust,
        tag: "macro_rules_call".into(),
    };
    let s = decl_kind_to_sql_text(&d);
    assert_eq!(s, "custom:rust:macro_rules_call");
    assert_eq!(decl_kind_from_sql_text(&s).unwrap(), d);
}

#[test]
fn decl_kind_from_sql_text_unknown_is_invalid() {
    let err = decl_kind_from_sql_text("trait").unwrap_err();
    assert!(matches!(err, StorageError::InvalidStoredData { .. }));
}

#[test]
fn decl_kind_from_sql_text_custom_missing_tag_is_invalid() {
    let err = decl_kind_from_sql_text("custom:rust").unwrap_err();
    assert!(matches!(err, StorageError::InvalidStoredData { .. }));
}

#[test]
fn decl_kind_from_sql_text_custom_unknown_lang_is_invalid() {
    let err = decl_kind_from_sql_text("custom:cobol:x").unwrap_err();
    assert!(matches!(err, StorageError::InvalidStoredData { .. }));
}

#[test]
fn visibility_round_trip_all_variants() {
    for v in [
        Visibility::Public,
        Visibility::Private,
        Visibility::Crate,
        Visibility::Protected,
    ] {
        let s = visibility_to_sql_text(v);
        assert_eq!(visibility_from_sql_text(s).unwrap(), v);
    }
}

#[test]
fn visibility_from_sql_text_unknown_is_invalid() {
    let err = visibility_from_sql_text("internal").unwrap_err();
    assert!(matches!(err, StorageError::InvalidStoredData { .. }));
}

#[test]
fn source_origin_round_trip_all_variants() {
    for o in [
        SourceOrigin::Workspace,
        SourceOrigin::CargoRegistry,
        SourceOrigin::NodeModulesDts,
        SourceOrigin::ManualExternal,
    ] {
        let s = source_origin_to_sql_text(o);
        assert_eq!(source_origin_from_sql_text(s).unwrap(), o);
    }
}

#[test]
fn source_origin_from_sql_text_unknown_is_invalid() {
    let err = source_origin_from_sql_text("vendor").unwrap_err();
    assert!(matches!(err, StorageError::InvalidStoredData { .. }));
}

#[test]
fn edge_kind_round_trip_all_variants() {
    for k in [
        EdgeKind::Calls,
        EdgeKind::Imports,
        EdgeKind::Extends,
        EdgeKind::Implements,
        EdgeKind::References,
        EdgeKind::UsesType,
    ] {
        let s = edge_kind_to_sql_text(k);
        assert_eq!(edge_kind_from_sql_text(s).unwrap(), k);
    }
}

#[test]
fn edge_kind_to_sql_text_screaming() {
    assert_eq!(edge_kind_to_sql_text(EdgeKind::Calls), "CALLS");
    assert_eq!(edge_kind_to_sql_text(EdgeKind::UsesType), "USES_TYPE");
}

#[test]
fn edge_kind_from_sql_text_lowercase_is_invalid() {
    let err = edge_kind_from_sql_text("calls").unwrap_err();
    assert!(matches!(err, StorageError::InvalidStoredData { .. }));
}

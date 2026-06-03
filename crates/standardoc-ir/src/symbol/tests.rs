use super::*;

#[test]
fn round_trip_minimal() {
    let s = RawSymbol {
        name: "foo".into(),
        fqdn: "crate::foo".into(),
        kind: Kind::Callable,
        language_kind: LanguageKind::from("function"),
        decl_kind: None,
        implements_trait: None,
        receiver_type: None,
        entry_point: None,
        module: None,
        visibility: Visibility::Public,
        location: SymbolLocation {
            file: "src/lib.rs".into(),
            start_line: 1,
            end_line: 5,
            start_col: 0,
            end_col: 1,
        },
        signature: None,
        body_hash: Some(Blake3Hash::new([0xab; 32])),
        attributes: vec![],
        flags: vec![],
    };
    let json = serde_json::to_string(&s).unwrap();
    let back: RawSymbol = serde_json::from_str(&json).unwrap();
    assert_eq!(s, back);
}

#[test]
fn round_trip_external_no_body_hash() {
    let s = RawSymbol {
        name: "Deserialize".into(),
        fqdn: "external::serde::Deserialize".into(),
        kind: Kind::Type,
        language_kind: LanguageKind::from("trait"),
        decl_kind: None,
        implements_trait: None,
        receiver_type: None,
        entry_point: None,
        module: Some("serde".into()),
        visibility: Visibility::Public,
        location: SymbolLocation {
            file: ".cargo/registry/.../serde/de.rs".into(),
            start_line: 100,
            end_line: 200,
            start_col: 0,
            end_col: 1,
        },
        signature: None,
        body_hash: None,
        attributes: vec![],
        flags: vec![],
    };
    let json = serde_json::to_string(&s).unwrap();
    let back: RawSymbol = serde_json::from_str(&json).unwrap();
    assert_eq!(s, back);
}

#[test]
fn round_trip_with_flags() {
    let s = RawSymbol {
        name: "fetchUser".into(),
        fqdn: "app::api::fetchUser".into(),
        kind: Kind::Callable,
        language_kind: LanguageKind::from("function"),
        decl_kind: None,
        implements_trait: None,
        receiver_type: None,
        entry_point: None,
        module: Some("app::api".into()),
        visibility: Visibility::Public,
        location: SymbolLocation {
            file: "src/api.ts".into(),
            start_line: 10,
            end_line: 20,
            start_col: 0,
            end_col: 1,
        },
        signature: None,
        body_hash: Some(Blake3Hash::new([0xcd; 32])),
        attributes: vec![],
        flags: vec!["async".into(), "iter".into()],
    };
    let json = serde_json::to_string(&s).unwrap();
    assert!(
        json.contains("\"flags\":[\"async\",\"iter\"]"),
        "flags must serialize as a JSON array, got {json}"
    );
    let back: RawSymbol = serde_json::from_str(&json).unwrap();
    assert_eq!(s, back);
}

#[test]
fn empty_flags_omitted_from_json() {
    let s = RawSymbol {
        name: "plain".into(),
        fqdn: "x::plain".into(),
        kind: Kind::Callable,
        language_kind: LanguageKind::from("function"),
        decl_kind: None,
        implements_trait: None,
        receiver_type: None,
        entry_point: None,
        module: None,
        visibility: Visibility::Public,
        location: SymbolLocation {
            file: "src/x.rs".into(),
            start_line: 1,
            end_line: 1,
            start_col: 0,
            end_col: 1,
        },
        signature: None,
        body_hash: None,
        attributes: vec![],
        flags: vec![],
    };
    let json = serde_json::to_string(&s).unwrap();
    assert!(
        !json.contains("flags"),
        "empty flags must be omitted (skip_serializing_if), got {json}"
    );
}

#[test]
fn decl_kind_round_trip_method() {
    let s = RawSymbol {
        name: "method".into(),
        fqdn: "crate::Type::method".into(),
        kind: Kind::Callable,
        language_kind: LanguageKind::from("impl_fn"),
        decl_kind: Some(DeclKind::Method),
        implements_trait: Some("crate::Trait".into()),
        receiver_type: Some(TypeRef::new("crate::Type")),
        entry_point: None,
        module: Some("crate::Type".into()),
        visibility: Visibility::Public,
        location: SymbolLocation {
            file: "src/lib.rs".into(),
            start_line: 1,
            end_line: 1,
            start_col: 0,
            end_col: 1,
        },
        signature: None,
        body_hash: None,
        attributes: vec![],
        flags: vec![],
    };
    let json = serde_json::to_string(&s).unwrap();
    assert!(json.contains("\"decl_kind\":\"method\""), "got {json}");
    let back: RawSymbol = serde_json::from_str(&json).unwrap();
    assert_eq!(s, back);
}

#[test]
fn missing_decl_kind_defaults_to_none() {
    let legacy_json = r#"{
            "name":"old","fqdn":"x::old","kind":"callable","language_kind":"fn",
            "visibility":"public",
            "location":{"file":"src/x.rs","start_line":1,"end_line":1,"start_col":0,"end_col":1}
        }"#;
    let back: RawSymbol = serde_json::from_str(legacy_json).unwrap();
    assert_eq!(back.decl_kind, None);
}

#[test]
fn missing_flags_defaults_to_empty_on_deserialize() {
    // Forward-compat: rows persisted before Stage 3e-1b have no
    // `flags` field. Deserialization must default to `vec![]`.
    let legacy_json = r#"{
            "name":"old","fqdn":"x::old","kind":"callable","language_kind":"fn",
            "visibility":"public",
            "location":{"file":"src/x.rs","start_line":1,"end_line":1,"start_col":0,"end_col":1}
        }"#;
    let back: RawSymbol = serde_json::from_str(legacy_json).unwrap();
    assert_eq!(back.flags, Vec::<String>::new());
}

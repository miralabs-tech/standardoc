use super::*;

#[test]
fn full_signature_round_trip() {
    let sig = Signature {
        params: vec![
            Param {
                name: "email".into(),
                ty: TypeRef::new("&str"),
                default: None,
            },
            Param {
                name: "limit".into(),
                ty: TypeRef::new("u32"),
                default: Some("10".into()),
            },
        ],
        returns: Some(TypeRef::new("Result<User, Error>")),
        modifiers: Modifiers {
            is_async: true,
            deprecated: Some("use create_user_v2".into()),
            generic_params: vec!["T".into()],
            where_clause: Some("T: Send".into()),
        },
        meta: SignatureMeta {
            exposed_via: vec![BridgeKind::from("tauri")],
        },
    };
    let json = serde_json::to_string(&sig).unwrap();
    let back: Signature = serde_json::from_str(&json).unwrap();
    assert_eq!(sig, back);
}

#[test]
fn ir3_signature_meta_multi_bridge_round_trip() {
    // Dual-target shape — same fn surfaced via both Tauri (Native↔Browser)
    // and wasm-bindgen (Native↔Wasm) bridges. The Vec<BridgeKind> shape
    // preserves order and emits a JSON array.
    let meta = SignatureMeta {
        exposed_via: vec![BridgeKind::from("tauri"), BridgeKind::from("wasm-bindgen")],
    };
    let json = serde_json::to_string(&meta).unwrap();
    assert!(json.contains("\"tauri\""), "got `{json}`");
    assert!(json.contains("\"wasm-bindgen\""), "got `{json}`");
    let back: SignatureMeta = serde_json::from_str(&json).unwrap();
    assert_eq!(meta, back);
}

#[test]
fn ir3_signature_meta_missing_field_deserializes_to_empty_vec() {
    // Pre-IR-3 DB rows that omitted `exposed_via` (the field was
    // `skip_serializing_if = Option::is_none`) must continue to load
    // — `#[serde(default)]` falls back to `Vec::new()`.
    let meta: SignatureMeta = serde_json::from_str("{}").unwrap();
    assert!(meta.exposed_via.is_empty());
}

#[test]
fn ir3_signature_meta_empty_vec_is_skipped_in_serialized_output() {
    // `skip_serializing_if = Vec::is_empty` keeps the on-disk JSON
    // minimal so the average row stays at the same byte cost as
    // pre-IR-3.
    let meta = SignatureMeta::default();
    let json = serde_json::to_string(&meta).unwrap();
    assert_eq!(json, "{}");
}

#[test]
fn async_renamed_in_json() {
    let m = Modifiers {
        is_async: true,
        ..Default::default()
    };
    let json = serde_json::to_string(&m).unwrap();
    assert!(json.contains("\"async\":true"), "json was {json}");
    assert!(!json.contains("is_async"));
}

#[test]
fn empty_signature_default() {
    let sig = Signature::default();
    let json = serde_json::to_string(&sig).unwrap();
    let back: Signature = serde_json::from_str(&json).unwrap();
    assert_eq!(sig, back);
}

#[test]
fn normalize_display_trims_and_collapses_internal_runs() {
    assert_eq!(normalize_display("  foo   bar  "), "foo bar");
    assert_eq!(normalize_display("a\t\tb\nc"), "a b c");
}

#[test]
fn normalize_display_preserves_typescript_union_single_spaces() {
    assert_eq!(normalize_display("string | number"), "string | number");
    assert_eq!(
        normalize_display("Promise < T >"),
        "Promise < T >",
        "single spaces between tokens must survive — only Rust providers should pre-pass via compact_rust_tokens"
    );
}

#[test]
fn type_ref_new_applies_normalize() {
    let t = TypeRef::new("  Result<User, Error>  ");
    assert_eq!(t.display, "Result<User, Error>");
    let t = TypeRef::new("Vec\t<\tT\t>");
    assert_eq!(t.display, "Vec < T >");
}

#[test]
fn compact_rust_tokens_empty_input_returns_empty() {
    assert_eq!(compact_rust_tokens(""), "");
    assert_eq!(compact_rust_tokens("   \t\n"), "");
}

#[test]
fn compact_rust_tokens_already_compact_is_idempotent() {
    let s = "Arc<dyn Logger>";
    assert_eq!(compact_rust_tokens(s), s);
}

#[test]
fn compact_rust_tokens_strips_spaces_around_angle_brackets_and_double_colon() {
    let raw = "& Arc < std :: sync :: Mutex < Option < standardoc_core :: WatcherHandle > > >";
    assert_eq!(
        compact_rust_tokens(raw),
        "&Arc<std::sync::Mutex<Option<standardoc_core::WatcherHandle>>>"
    );
}

#[test]
fn compact_rust_tokens_keeps_space_after_dyn_impl_mut() {
    assert_eq!(compact_rust_tokens("Arc < dyn Logger >"), "Arc<dyn Logger>");
    assert_eq!(
        compact_rust_tokens("impl Trait + Send"),
        "impl Trait + Send"
    );
    assert_eq!(compact_rust_tokens("& mut Foo"), "&mut Foo");
}

#[test]
fn compact_rust_tokens_handles_comma_semicolon_colon() {
    assert_eq!(
        compact_rust_tokens("HashMap < String , u32 >"),
        "HashMap<String, u32>"
    );
    assert_eq!(compact_rust_tokens("[ T ; N ]"), "[T; N]");
    assert_eq!(
        compact_rust_tokens("fn ( x : u32 ) -> u32"),
        "fn(x: u32) -> u32"
    );
}

#[test]
fn compact_rust_tokens_lifetime_attached_to_ident() {
    // In current syn versions `'a` is a single token; this guards the
    // degenerate split case where `'` and ident arrive separated.
    assert_eq!(compact_rust_tokens("& ' a Foo"), "&'a Foo");
    assert_eq!(compact_rust_tokens("& 'a str"), "&'a str");
    assert_eq!(
        compact_rust_tokens("for < 'a > Fn ( & 'a u32 )"),
        "for<'a> Fn(&'a u32)"
    );
}

#[test]
fn compact_rust_tokens_arrow_return_type() {
    assert_eq!(
        compact_rust_tokens("Result < () , Error >"),
        "Result<(), Error>"
    );
    assert_eq!(
        compact_rust_tokens("fn ( ) -> Result < u32 , Error >"),
        "fn() -> Result<u32, Error>"
    );
}

#[test]
fn compact_rust_tokens_preserves_string_literal_inside_attribute_value() {
    // String literals are single Literal token trees — their internal
    // spaces are not split by `split_whitespace` because the surrounding
    // quotes keep the literal as a single chunk.
    assert_eq!(
        compact_rust_tokens(r#"name = "some description""#),
        r#"name = "some description""#
    );
}

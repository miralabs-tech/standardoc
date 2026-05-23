use serde::{Deserialize, Serialize};

use crate::attribute::RawAttribute;
use crate::hash::Blake3Hash;
use crate::kinds::{DeclKind, Kind, Visibility};
use crate::language_kind::LanguageKind;
use crate::location::SymbolLocation;
use crate::signature::{Signature, TypeRef};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RawSymbol {
    pub name: String,
    pub fqdn: String,
    pub kind: Kind,
    pub language_kind: LanguageKind,
    /// Phase 2 K refined declaration kind — populated per language in
    /// K-Step-B+ (Rust/TS/C/Lua). `None` on rows extracted before the
    /// language gained DeclKind coverage, or for languages that have
    /// not been migrated yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decl_kind: Option<DeclKind>,
    /// Phase 2 K-Step-C — when this symbol is a `Method` declared
    /// inside `impl Trait for Type { ... }`, this carries the trait
    /// FQDN. Inherent impl methods and free functions leave this
    /// `None`. The relation is also visible as an `EdgeKind::Implements`
    /// edge from the receiver type to the trait; this field is the
    /// per-method projection of that relationship.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implements_trait: Option<String>,
    /// Phase 2 K-Step-C — for `DeclKind::Method` symbols, the type
    /// that the method dispatches on. Rust: the impl-er (`Bar` for
    /// `impl Foo for Bar { fn baz }`); for trait method definitions
    /// (`trait Foo { fn baz }`) the trait itself, as the receiver is
    /// `Self : Foo`. Free functions leave this `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver_type: Option<TypeRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    pub visibility: Visibility,
    pub location: SymbolLocation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<Signature>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_hash: Option<Blake3Hash>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attributes: Vec<RawAttribute>,
    /// Computed semantic flags surfaced post-extraction. Distinct from
    /// `attributes` (which mirror source-level `#[derive(…)]` /
    /// decorators) and from `signature.modifiers` (which mirror
    /// syntactic keywords like `async`, `const`, `unsafe`). Flags here
    /// are *derived* by the resolver / walker from observed semantics —
    /// e.g. Stage 3e-1b stamps `"async"` when a fn returns
    /// `Promise<T>` / `Future<T>` (regardless of an explicit `async`
    /// keyword) and `"iter"` when an `Iterator` / `Generator` trait or
    /// type is touched. UST languages can register their own builtin
    /// tags (`lua:coroutine-yielding`, …) which surface here as
    /// arbitrary strings without an IR schema change.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_minimal() {
        let s = RawSymbol {
            name: "foo".into(),
            fqdn: "crate::foo".into(),
            kind: Kind::Function,
            language_kind: LanguageKind::from("function"),
            decl_kind: None,
            implements_trait: None,
            receiver_type: None,
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
            kind: Kind::Function,
            language_kind: LanguageKind::from("function"),
            decl_kind: None,
            implements_trait: None,
            receiver_type: None,
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
            kind: Kind::Function,
            language_kind: LanguageKind::from("function"),
            decl_kind: None,
            implements_trait: None,
            receiver_type: None,
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
            kind: Kind::Function,
            language_kind: LanguageKind::from("impl_fn"),
            decl_kind: Some(DeclKind::Method),
            implements_trait: Some("crate::Trait".into()),
            receiver_type: Some(TypeRef::new("crate::Type")),
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
            "name":"old","fqdn":"x::old","kind":"function","language_kind":"fn",
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
            "name":"old","fqdn":"x::old","kind":"function","language_kind":"fn",
            "visibility":"public",
            "location":{"file":"src/x.rs","start_line":1,"end_line":1,"start_col":0,"end_col":1}
        }"#;
        let back: RawSymbol = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(back.flags, Vec::<String>::new());
    }
}

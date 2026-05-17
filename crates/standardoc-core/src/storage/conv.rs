use standardoc_ir::{
    EdgeConfidence, EdgeKind, Kind, Language, ResolvedOrUnresolved, Signature, SourceOrigin,
    Visibility,
};

use crate::storage::error::StorageError;

pub(crate) fn signature_to_json(sig: &Signature) -> Result<String, StorageError> {
    // IR-1 1.0 vocabulary lock: validate every `exposed_via` slug before
    // persisting the signature JSON. Refuses extractor-emitted slugs
    // that are neither built-in nor `custom:`-prefixed, surfacing the
    // error as `StorageError::BridgeKindInvalid` (via
    // `From<BridgeKindError>`).
    //
    // IR-3: `exposed_via` is `Vec<BridgeKind>` (dual-target apps may
    // expose the same symbol via multiple bridges, e.g. Tauri + wasm).
    // Validation short-circuits on the first invalid slug — a second
    // bad slug in the same vec will surface on the next walk after the
    // first is fixed, which is the simplest contract.
    for bridge in &sig.meta.exposed_via {
        bridge.try_validate()?;
    }
    Ok(serde_json::to_string(sig)?)
}

pub(crate) fn json_to_signature(s: &str) -> Result<Signature, StorageError> {
    Ok(serde_json::from_str(s)?)
}

/// Maps a `ResolvedOrUnresolved` to the `to_unresolved` SQL column when the
/// target is not (yet) resolved. Resolved targets return `None` — the caller
/// must then look up the symbol id from the fqdn before binding `to_symbol_id`.
/// `UnresolvedBridge` is encoded as `"<bridge_kind>::<name>"` per bridges lock #3.
///
/// IR-1: returns `Err(BridgeKindInvalid)` when an `UnresolvedBridge` carries
/// a slug outside the 1.0 vocabulary lock (neither built-in nor
/// `custom:`-prefixed). The two non-bridge variants are infallible.
pub(crate) fn unresolved_to_storage(
    target: &ResolvedOrUnresolved,
) -> Result<Option<String>, StorageError> {
    match target {
        ResolvedOrUnresolved::Resolved { .. } => Ok(None),
        ResolvedOrUnresolved::Unresolved { name } => Ok(Some(name.clone())),
        ResolvedOrUnresolved::UnresolvedBridge { bridge, name } => {
            bridge.try_validate()?;
            Ok(Some(format!("{}::{}", bridge.as_str(), name)))
        }
    }
}

pub(crate) const fn language_to_sql_text(lang: Language) -> &'static str {
    match lang {
        Language::Rust => "rust",
        Language::TypeScript => "typescript",
        Language::JavaScript => "javascript",
        Language::Lua => "lua",
        Language::Vue => "vue",
        Language::Svelte => "svelte",
        Language::C => "c",
    }
}

pub(crate) fn language_from_sql_text(s: &str) -> Result<Language, StorageError> {
    match s {
        "rust" => Ok(Language::Rust),
        "typescript" => Ok(Language::TypeScript),
        "javascript" => Ok(Language::JavaScript),
        "lua" => Ok(Language::Lua),
        "vue" => Ok(Language::Vue),
        "svelte" => Ok(Language::Svelte),
        "c" => Ok(Language::C),
        other => Err(StorageError::InvalidStoredData {
            detail: format!("unknown language: {other:?}"),
        }),
    }
}

pub(crate) const fn kind_to_sql_text(k: Kind) -> &'static str {
    match k {
        Kind::Function => "function",
        Kind::Type => "type",
        Kind::Value => "value",
        Kind::Module => "module",
        Kind::Macro => "macro",
    }
}

pub(crate) fn kind_from_sql_text(s: &str) -> Result<Kind, StorageError> {
    match s {
        "function" => Ok(Kind::Function),
        "type" => Ok(Kind::Type),
        "value" => Ok(Kind::Value),
        "module" => Ok(Kind::Module),
        "macro" => Ok(Kind::Macro),
        other => Err(StorageError::InvalidStoredData {
            detail: format!("unknown kind: {other:?}"),
        }),
    }
}

pub(crate) const fn visibility_to_sql_text(v: Visibility) -> &'static str {
    match v {
        Visibility::Public => "public",
        Visibility::Private => "private",
        Visibility::Crate => "crate",
        Visibility::Protected => "protected",
    }
}

pub(crate) fn visibility_from_sql_text(s: &str) -> Result<Visibility, StorageError> {
    match s {
        "public" => Ok(Visibility::Public),
        "private" => Ok(Visibility::Private),
        "crate" => Ok(Visibility::Crate),
        "protected" => Ok(Visibility::Protected),
        other => Err(StorageError::InvalidStoredData {
            detail: format!("unknown visibility: {other:?}"),
        }),
    }
}

pub(crate) const fn source_origin_to_sql_text(o: SourceOrigin) -> &'static str {
    match o {
        SourceOrigin::Workspace => "workspace",
        SourceOrigin::CargoRegistry => "cargo_registry",
        SourceOrigin::NodeModulesDts => "node_modules_dts",
        SourceOrigin::ManualExternal => "manual_external",
    }
}

pub(crate) fn source_origin_from_sql_text(s: &str) -> Result<SourceOrigin, StorageError> {
    match s {
        "workspace" => Ok(SourceOrigin::Workspace),
        "cargo_registry" => Ok(SourceOrigin::CargoRegistry),
        "node_modules_dts" => Ok(SourceOrigin::NodeModulesDts),
        "manual_external" => Ok(SourceOrigin::ManualExternal),
        other => Err(StorageError::InvalidStoredData {
            detail: format!("unknown source_origin: {other:?}"),
        }),
    }
}

pub(crate) const fn edge_kind_to_sql_text(k: EdgeKind) -> &'static str {
    match k {
        EdgeKind::Calls => "CALLS",
        EdgeKind::Imports => "IMPORTS",
        EdgeKind::Extends => "EXTENDS",
        EdgeKind::Implements => "IMPLEMENTS",
        EdgeKind::References => "REFERENCES",
        EdgeKind::Defines => "DEFINES",
        EdgeKind::UsesType => "USES_TYPE",
        EdgeKind::ExposesApi => "EXPOSES_API",
    }
}

pub(crate) fn edge_kind_from_sql_text(s: &str) -> Result<EdgeKind, StorageError> {
    match s {
        "CALLS" => Ok(EdgeKind::Calls),
        "IMPORTS" => Ok(EdgeKind::Imports),
        "EXTENDS" => Ok(EdgeKind::Extends),
        "IMPLEMENTS" => Ok(EdgeKind::Implements),
        "REFERENCES" => Ok(EdgeKind::References),
        "DEFINES" => Ok(EdgeKind::Defines),
        "USES_TYPE" => Ok(EdgeKind::UsesType),
        "EXPOSES_API" => Ok(EdgeKind::ExposesApi),
        other => Err(StorageError::InvalidStoredData {
            detail: format!("unknown edge kind: {other:?}"),
        }),
    }
}

pub(crate) const fn edge_confidence_to_sql_text(c: EdgeConfidence) -> &'static str {
    match c {
        EdgeConfidence::Extracted => "extracted",
        EdgeConfidence::Inferred => "inferred",
        EdgeConfidence::Ambiguous => "ambiguous",
    }
}

pub(crate) fn edge_confidence_from_sql_text(s: &str) -> Result<EdgeConfidence, StorageError> {
    match s {
        "extracted" => Ok(EdgeConfidence::Extracted),
        "inferred" => Ok(EdgeConfidence::Inferred),
        "ambiguous" => Ok(EdgeConfidence::Ambiguous),
        other => Err(StorageError::InvalidStoredData {
            detail: format!("unknown edge confidence: {other:?}"),
        }),
    }
}

#[cfg(test)]
mod tests {
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
                exposed_via: vec![
                    BridgeKind::from("tauri"),
                    BridgeKind::from("wasm-bindgen"),
                ],
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
            Kind::Function,
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
        assert_eq!(kind_to_sql_text(Kind::Function), "function");
        assert_eq!(kind_to_sql_text(Kind::Macro), "macro");
    }

    #[test]
    fn kind_from_sql_text_unknown_is_invalid() {
        let err = kind_from_sql_text("class").unwrap_err();
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
            EdgeKind::Defines,
            EdgeKind::UsesType,
            EdgeKind::ExposesApi,
        ] {
            let s = edge_kind_to_sql_text(k);
            assert_eq!(edge_kind_from_sql_text(s).unwrap(), k);
        }
    }

    #[test]
    fn edge_kind_to_sql_text_screaming() {
        assert_eq!(edge_kind_to_sql_text(EdgeKind::Calls), "CALLS");
        assert_eq!(edge_kind_to_sql_text(EdgeKind::UsesType), "USES_TYPE");
        assert_eq!(edge_kind_to_sql_text(EdgeKind::ExposesApi), "EXPOSES_API");
    }

    #[test]
    fn edge_kind_from_sql_text_lowercase_is_invalid() {
        let err = edge_kind_from_sql_text("calls").unwrap_err();
        assert!(matches!(err, StorageError::InvalidStoredData { .. }));
    }
}

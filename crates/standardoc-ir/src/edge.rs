use serde::{Deserialize, Serialize};

use crate::bridge_kind::BridgeKind;
use crate::kinds::{EdgeConfidence, EdgeKind};
use crate::location::Site;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RawEdge {
    pub from_fqdn: String,
    pub kind: EdgeKind,
    pub to: ResolvedOrUnresolved,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sites: Vec<Site>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attributes: Vec<String>,
    #[serde(default)]
    pub confidence: EdgeConfidence,
    // Bug E-3 Phase 1: nominal type of the call receiver when inferable
    // by the language extractor (e.g. "Vec", "Foo", "self_type"). Used
    // by the resolver post-pass to disambiguate bare-ident method calls.
    // None when not inferable or not applicable (path-form calls, non-Rust).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResolvedOrUnresolved {
    Resolved { fqdn: String },
    Unresolved { name: String },
    UnresolvedBridge { bridge: BridgeKind, name: String },
}

impl ResolvedOrUnresolved {
    /// Default confidence derived purely from the variant: Resolved is
    /// `Extracted`, UnresolvedBridge is `Inferred`, Unresolved is `Ambiguous`.
    /// Call sites that have finer information (e.g. a resolver that knows the
    /// target came from an alias table) should set `RawEdge.confidence`
    /// explicitly instead of relying on this default.
    #[must_use]
    pub const fn default_confidence(&self) -> EdgeConfidence {
        match self {
            Self::Resolved { .. } => EdgeConfidence::Extracted,
            Self::UnresolvedBridge { .. } => EdgeConfidence::Inferred,
            Self::Unresolved { .. } => EdgeConfidence::Ambiguous,
        }
    }
}

impl RawEdge {
    /// Build a `RawEdge` whose `confidence` is derived from `to`'s variant via
    /// `default_confidence()`. Use the struct literal directly when the
    /// caller needs to set a different confidence.
    #[must_use]
    pub const fn with_default_confidence(
        from_fqdn: String,
        kind: EdgeKind,
        to: ResolvedOrUnresolved,
        sites: Vec<Site>,
        attributes: Vec<String>,
    ) -> Self {
        let confidence = to.default_confidence();
        Self {
            from_fqdn,
            kind,
            to,
            sites,
            attributes,
            confidence,
            receiver_type: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_round_trip() {
        let e = RawEdge {
            from_fqdn: "crate::a::foo".into(),
            kind: EdgeKind::Calls,
            to: ResolvedOrUnresolved::Resolved {
                fqdn: "crate::b::bar".into(),
            },
            sites: vec![Site {
                file: "src/a.rs".into(),
                line: 5,
                col: 8,
            }],
            attributes: vec![],
            confidence: EdgeConfidence::Extracted,
            receiver_type: None,
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: RawEdge = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn unresolved_round_trip() {
        let e = RawEdge {
            from_fqdn: "crate::x".into(),
            kind: EdgeKind::Calls,
            to: ResolvedOrUnresolved::Unresolved {
                name: "do_thing".into(),
            },
            sites: vec![],
            attributes: vec![],
            confidence: EdgeConfidence::Ambiguous,
            receiver_type: None,
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: RawEdge = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn unresolved_bridge_round_trip() {
        let e = RawEdge {
            from_fqdn: "frontend::login".into(),
            kind: EdgeKind::Calls,
            to: ResolvedOrUnresolved::UnresolvedBridge {
                bridge: BridgeKind::from("tauri"),
                name: "create_user".into(),
            },
            sites: vec![],
            attributes: vec![],
            confidence: EdgeConfidence::Inferred,
            receiver_type: None,
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(
            json.contains("\"kind\":\"unresolved_bridge\""),
            "json was {json}"
        );
        let back: RawEdge = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn attributes_round_trip_with_values() {
        let e = RawEdge {
            from_fqdn: "src::components::App".into(),
            kind: EdgeKind::References,
            to: ResolvedOrUnresolved::Unresolved {
                name: "handleClick".into(),
            },
            sites: vec![],
            attributes: vec!["template-bind".into(), "template-event".into()],
            confidence: EdgeConfidence::Extracted,
            receiver_type: None,
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"attributes\""), "json was {json}");
        assert!(json.contains("template-bind"), "json was {json}");
        let back: RawEdge = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
        assert_eq!(back.attributes.len(), 2);
    }

    #[test]
    fn defaults_when_absent_in_json() {
        let json = r#"{
            "from_fqdn": "crate::a",
            "kind": "CALLS",
            "to": { "kind": "unresolved", "name": "foo" }
        }"#;
        let back: RawEdge = serde_json::from_str(json).unwrap();
        assert!(
            back.attributes.is_empty(),
            "attributes should default to empty Vec when absent (migration neutrality)"
        );
        assert!(back.sites.is_empty());
        assert_eq!(
            back.confidence,
            EdgeConfidence::Extracted,
            "confidence should default to Extracted for legacy JSON without the field"
        );
    }

    #[test]
    fn confidence_serializes_snake_case() {
        let e = RawEdge {
            from_fqdn: "a".into(),
            kind: EdgeKind::Calls,
            to: ResolvedOrUnresolved::Unresolved { name: "b".into() },
            sites: vec![],
            attributes: vec![],
            confidence: EdgeConfidence::Inferred,
            receiver_type: None,
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(
            json.contains("\"confidence\":\"inferred\""),
            "json was {json}"
        );
    }

    #[test]
    fn confidence_round_trip_all_variants() {
        for conf in [
            EdgeConfidence::Extracted,
            EdgeConfidence::Inferred,
            EdgeConfidence::Ambiguous,
        ] {
            let e = RawEdge {
                from_fqdn: "a".into(),
                kind: EdgeKind::Calls,
                to: ResolvedOrUnresolved::Unresolved { name: "b".into() },
                sites: vec![],
                attributes: vec![],
                confidence: conf,
                receiver_type: None,
            };
            let json = serde_json::to_string(&e).unwrap();
            let back: RawEdge = serde_json::from_str(&json).unwrap();
            assert_eq!(e, back);
        }
    }
}

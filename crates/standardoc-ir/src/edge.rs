use serde::{Deserialize, Serialize};

use crate::bridge_kind::BridgeKind;
use crate::kinds::EdgeKind;
use crate::location::Site;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RawEdge {
    pub from_fqdn: String,
    pub kind: EdgeKind,
    pub to: ResolvedOrUnresolved,
    #[serde(default)]
    pub sites: Vec<Site>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResolvedOrUnresolved {
    Resolved { fqdn: String },
    Unresolved { name: String },
    UnresolvedBridge { bridge: BridgeKind, name: String },
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
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(
            json.contains("\"kind\":\"unresolved_bridge\""),
            "json was {json}"
        );
        let back: RawEdge = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }
}

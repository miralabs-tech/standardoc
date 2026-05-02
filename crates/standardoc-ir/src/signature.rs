use serde::{Deserialize, Serialize};

use crate::bridge_kind::BridgeKind;

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Signature {
    #[serde(default)]
    pub params: Vec<Param>,
    pub returns: Option<TypeRef>,
    #[serde(default)]
    pub modifiers: Modifiers,
    #[serde(default)]
    pub meta: SignatureMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    pub ty: TypeRef,
    pub default: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TypeRef {
    pub display: String,
}

impl TypeRef {
    pub fn new(display: impl Into<String>) -> Self {
        Self {
            display: display.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Modifiers {
    #[serde(default, rename = "async")]
    pub is_async: bool,
    pub deprecated: Option<String>,
    #[serde(default)]
    pub generic_params: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SignatureMeta {
    pub exposed_via: Option<BridgeKind>,
}

#[cfg(test)]
mod tests {
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
            },
            meta: SignatureMeta {
                exposed_via: Some(BridgeKind::from("tauri")),
            },
        };
        let json = serde_json::to_string(&sig).unwrap();
        let back: Signature = serde_json::from_str(&json).unwrap();
        assert_eq!(sig, back);
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
}

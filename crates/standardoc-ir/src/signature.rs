use serde::{Deserialize, Serialize};

use crate::bridge_kind::BridgeKind;

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Signature {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<Param>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returns: Option<TypeRef>,
    #[serde(default, skip_serializing_if = "Modifiers::is_default")]
    pub modifiers: Modifiers,
    #[serde(default, skip_serializing_if = "SignatureMeta::is_default")]
    pub meta: SignatureMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    pub ty: TypeRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default, rename = "async", skip_serializing_if = "std::ops::Not::not")]
    pub is_async: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub generic_params: Vec<String>,
    /// Raw text of the `where T: Foo, U: Bar` clause when present. Inline
    /// generic bounds (`<T: Display>`) are already part of `generic_params`,
    /// so this only captures the trailing `where` extension.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub where_clause: Option<String>,
}

impl Modifiers {
    const fn is_default(&self) -> bool {
        !self.is_async
            && self.deprecated.is_none()
            && self.generic_params.is_empty()
            && self.where_clause.is_none()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SignatureMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exposed_via: Option<BridgeKind>,
}

impl SignatureMeta {
    const fn is_default(&self) -> bool {
        self.exposed_via.is_none()
    }
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
                where_clause: Some("T: Send".into()),
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

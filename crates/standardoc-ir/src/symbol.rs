use serde::{Deserialize, Serialize};

use crate::attribute::RawAttribute;
use crate::hash::Blake3Hash;
use crate::kinds::{Kind, Visibility};
use crate::language_kind::LanguageKind;
use crate::location::SymbolLocation;
use crate::signature::Signature;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RawSymbol {
    pub name: String,
    pub fqdn: String,
    pub kind: Kind,
    pub language_kind: LanguageKind,
    pub module: Option<String>,
    pub visibility: Visibility,
    pub location: SymbolLocation,
    pub signature: Option<Signature>,
    pub body_hash: Option<Blake3Hash>,
    #[serde(default)]
    pub attributes: Vec<RawAttribute>,
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
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: RawSymbol = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }
}

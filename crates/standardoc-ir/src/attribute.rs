use serde::{Deserialize, Serialize};

use crate::location::Site;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RawAttribute {
    pub name: String,
    #[serde(default)]
    pub args: Vec<RawAttributeArg>,
    pub site: Site,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RawAttributeArg {
    pub key: Option<String>,
    pub value: String,
    pub is_string_literal: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let attr = RawAttribute {
            name: "tauri::command".into(),
            args: vec![RawAttributeArg {
                key: Some("name".into()),
                value: "create_user".into(),
                is_string_literal: true,
            }],
            site: Site {
                file: "src/lib.rs".into(),
                line: 12,
                col: 0,
            },
        };
        let json = serde_json::to_string(&attr).unwrap();
        let back: RawAttribute = serde_json::from_str(&json).unwrap();
        assert_eq!(attr, back);
    }

    #[test]
    fn positional_arg() {
        let arg = RawAttributeArg {
            key: None,
            value: "42".into(),
            is_string_literal: false,
        };
        let json = serde_json::to_string(&arg).unwrap();
        let back: RawAttributeArg = serde_json::from_str(&json).unwrap();
        assert_eq!(arg, back);
    }
}

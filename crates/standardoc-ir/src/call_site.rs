use serde::{Deserialize, Serialize};

use crate::location::Site;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RawCallSite {
    pub from_fqdn: String,
    pub callee_text: String,
    #[serde(default)]
    pub args: Vec<RawCallArg>,
    pub site: Site,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RawCallArg {
    pub value: String,
    pub is_string_literal: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let cs = RawCallSite {
            from_fqdn: "crate::auth::login".into(),
            callee_text: "tauri::invoke".into(),
            args: vec![RawCallArg {
                value: "create_user".into(),
                is_string_literal: true,
            }],
            site: Site {
                file: "src/auth.rs".into(),
                line: 42,
                col: 8,
            },
        };
        let json = serde_json::to_string(&cs).unwrap();
        let back: RawCallSite = serde_json::from_str(&json).unwrap();
        assert_eq!(cs, back);
    }
}

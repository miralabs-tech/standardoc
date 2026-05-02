use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BridgeKind(pub String);

impl BridgeKind {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for BridgeKind {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl From<String> for BridgeKind {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl std::fmt::Display for BridgeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let b = BridgeKind::from("tauri");
        let json = serde_json::to_string(&b).unwrap();
        assert_eq!(json, "\"tauri\"");
        let back: BridgeKind = serde_json::from_str(&json).unwrap();
        assert_eq!(b, back);
    }

    #[test]
    fn display_is_inner() {
        assert_eq!(BridgeKind::from("wasm_bindgen").to_string(), "wasm_bindgen");
    }
}

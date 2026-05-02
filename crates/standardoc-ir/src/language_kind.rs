use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LanguageKind(pub String);

impl LanguageKind {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for LanguageKind {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl From<String> for LanguageKind {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl std::fmt::Display for LanguageKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let l = LanguageKind::from("type_alias");
        let json = serde_json::to_string(&l).unwrap();
        assert_eq!(json, "\"type_alias\"");
        let back: LanguageKind = serde_json::from_str(&json).unwrap();
        assert_eq!(l, back);
    }
}

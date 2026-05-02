use serde::{Deserialize, Serialize};

/// User-authored documentation node attached to a symbol via FQDN.
///
/// Day-1 scope: description-only. Tags (`@param`/`@returns`/`@see`/`@since`)
/// are PUNTED post-beta.1 — providers strip lexical markers (`///`/`//!` for
/// Rust, `/**`/`*/`/leading ` * ` for TS JSDoc) but `@tags` remain inline in
/// `description` as Markdown-ish text.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RawDocument {
    pub symbol_fqdn: String,
    pub description: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let d = RawDocument {
            symbol_fqdn: "crate::foo".into(),
            description: "Creates a new user.".into(),
        };
        let json = serde_json::to_string(&d).unwrap();
        let back: RawDocument = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }
}

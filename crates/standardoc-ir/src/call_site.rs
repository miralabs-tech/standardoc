use serde::{Deserialize, Serialize};

use crate::location::Site;

/// A free-form record of a call expression observed in source code.
/// Unlike a `RawEdge { kind: Calls, ... }`, this carries the *textual*
/// shape of the call (callee text, literal arg values, member-access
/// receiver chain) and is the raw substrate the Lua plugin layer reads
/// to build its own attributes/bridges without re-parsing the source.
///
/// IR-4 shape (1.0):
/// - `callee_text` — verbatim text of the callee expression, dotted /
///   whitespace-collapsed (`obj.api.create` for `obj.api.create(...)`).
/// - `args` — left-to-right positional args with their literal value /
///   identifier text and a `is_string_literal` flag (extractors set it
///   when the source was a string literal — not arbitrary expressions).
/// - `receiver_chain` — for member-access call patterns, the list of
///   each receiver segment text in source order. `foo.bar.baz()` yields
///   `["foo", "bar"]` (the final identifier is the called method, not
///   a receiver). Free-fn calls (`foo()`) yield an empty vec. Generics
///   are stripped (`foo<T>().bar()` yields `["foo"]`); computed accesses
///   (`obj["bar"]()`) are stringified verbatim including the brackets.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RawCallSite {
    pub from_fqdn: String,
    pub callee_text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<RawCallArg>,
    /// IR-4: receiver chain for member-access calls. Empty for free-fn
    /// calls. See type docs for shape rules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub receiver_chain: Vec<String>,
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
            receiver_chain: vec!["tauri".into()],
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

    #[test]
    fn ir4_receiver_chain_missing_field_deserializes_to_empty_vec() {
        // Pre-IR-4 DB rows / extractor emissions omitted `receiver_chain`.
        // `#[serde(default)]` must keep them loadable as an empty vec.
        let json = r#"{
            "from_fqdn": "crate::a",
            "callee_text": "foo",
            "site": { "file": "src/a.rs", "line": 1, "col": 0 }
        }"#;
        let cs: RawCallSite = serde_json::from_str(json).unwrap();
        assert!(cs.receiver_chain.is_empty());
        assert!(cs.args.is_empty());
    }

    #[test]
    fn ir4_empty_receiver_chain_is_skipped_in_serialized_output() {
        // `skip_serializing_if = Vec::is_empty` keeps the on-disk JSON
        // minimal for the free-fn case (the common one).
        let cs = RawCallSite {
            from_fqdn: "crate::a".into(),
            callee_text: "foo".into(),
            args: vec![],
            receiver_chain: vec![],
            site: Site {
                file: "src/a.rs".into(),
                line: 1,
                col: 0,
            },
        };
        let json = serde_json::to_string(&cs).unwrap();
        assert!(!json.contains("receiver_chain"), "got `{json}`");
        assert!(!json.contains("args"), "got `{json}`");
    }

    #[test]
    fn ir4_multi_segment_receiver_chain_round_trips() {
        // `foo.bar.baz(x)` extractor shape — final segment is the called
        // method (`baz`), receivers are the preceding ones in source
        // order.
        let cs = RawCallSite {
            from_fqdn: "crate::caller".into(),
            callee_text: "foo.bar.baz".into(),
            args: vec![RawCallArg {
                value: "x".into(),
                is_string_literal: false,
            }],
            receiver_chain: vec!["foo".into(), "bar".into()],
            site: Site {
                file: "src/caller.rs".into(),
                line: 10,
                col: 4,
            },
        };
        let json = serde_json::to_string(&cs).unwrap();
        let back: RawCallSite = serde_json::from_str(&json).unwrap();
        assert_eq!(cs, back);
        // The vec preserves source order through JSON serialization.
        assert!(
            json.find("\"foo\"").unwrap() < json.find("\"bar\"").unwrap(),
            "receiver_chain order regression: `{json}`"
        );
    }
}

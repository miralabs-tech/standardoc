//! Sourcemap protocol types for the preproc<->extractor contract.
//!
//! These structs match the on-wire JSON shape described in `SOURCEMAP_v1.md`.
//! Producers emit `@stdoc:<tag> <json>` Lua comments; consumers parse the JSON
//! payload via `serde_json::from_str::<...>(json)`. Each annotation carries a
//! `v: u32` version field — consumers should reject payloads with
//! `v > supported`, log + skip on parse errors, and tolerate unknown fields.
//!
//! The crate is intentionally minimal (serde derives only) so that both the
//! `standarlua` preproc (producer) and the `standardoc-core` extractor
//! (consumer) can depend on it without dragging in IR or storage code.

use serde::{Deserialize, Serialize};

// --- Match annotations -----------------------------------------------------

/// `@stdoc:match-begin` — emitted before the lowered scrutinee assignment of
/// a `match ... with ... end` cluster.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchBeginAnnotation {
    pub v: u32,
    pub mid: String,
    pub scrut: String,
    pub arm_count: u32,
}

/// `@stdoc:match-arm` — emitted before each lowered arm body of a match
/// cluster. `idx` is the 0-based arm index within the cluster.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchArmAnnotation {
    pub v: u32,
    pub mid: String,
    pub idx: u32,
    pub pattern: PatternNode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard: Option<String>,
}

/// `@stdoc:match-end` — emitted after the closing `end` of a match cluster.
/// `result` is the synthesized local name aggregating the arm results, or
/// `None` for the statement form (no result var).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchEndAnnotation {
    pub v: u32,
    pub mid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

// --- Safe-nav annotations --------------------------------------------------

/// `@stdoc:safe-nav` — emitted alongside the desugared wrapper of a `?.` /
/// `?:` operator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SafeNavAnnotation {
    pub v: u32,
    pub source: String,
    pub target: String,
    pub op: SafeNavOp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SafeNavOp {
    /// `?.` member access.
    Member,
    /// `?:` method call.
    Call,
}

// --- Compound-op annotations -----------------------------------------------

/// `@stdoc:compound-op` — emitted alongside the desugared plain assignment of
/// a compound operator (`+=`, `-=`, `..=`, etc.).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompoundOpAnnotation {
    pub v: u32,
    pub op: String,
    pub lhs: String,
    pub rhs: String,
}

// --- Type-strip annotations ------------------------------------------------

/// `@stdoc:type-strip` — emitted whenever a type annotation is removed from
/// runtime Lua. Carries the structured type info for the extractor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeStripAnnotation {
    pub v: u32,
    pub ident: String,
    pub ty: TypeNode,
    pub site: TypeStripSite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TypeStripSite {
    /// `local x: T = ...`
    Local,
    /// `function f(x: T)`
    Param,
    /// `function f(): T` — the synthetic ident `_return` carries this.
    Return,
    /// `type X = { f: T }`
    Field,
}

// --- Type declaration annotations ------------------------------------------

/// `@stdoc:type-decl` — emitted for every `type X = ...` declaration so the
/// extractor can populate a per-module type table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeDeclAnnotation {
    pub v: u32,
    pub name: String,
    pub ty: TypeNode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub generics: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<SourceSpan>,
}

/// Span carried by `type-decl` annotations when the producer can attach
/// source location info. Line numbers are 1-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSpan {
    pub start_line: u32,
    pub end_line: u32,
}

// --- Structural type tree --------------------------------------------------

/// Structural type tree used by both `TypeStripAnnotation` and
/// `TypeDeclAnnotation`. Recursive; designed for compact wire representation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TypeNode {
    /// Primitives: `"number"`, `"string"`, `"boolean"`, `"nil"`, `"any"`.
    Primitive { name: String },
    /// `T?` shorthand for `T | nil`.
    Optional { inner: Box<TypeNode> },
    /// Heterogeneous union: `T | U`.
    Union { items: Vec<TypeNode> },
    /// Literal-only union: `200 | 201 | 202`. Distinct from `Union` because
    /// the v2 narrowing checker treats this specially (fast-path
    /// exhaustiveness).
    #[serde(rename = "literalunion")]
    LiteralUnion { values: Vec<serde_json::Value> },
    /// Record / object: `{ x: number, y: number }`.
    Record { fields: Vec<TypeField> },
    /// Map: `{ [K]: V }`.
    Map {
        key: Box<TypeNode>,
        value: Box<TypeNode>,
    },
    /// Array: `{T}`.
    Array { inner: Box<TypeNode> },
    /// Function: `fun(P1, P2): R`. `generics` is reserved for inline generic
    /// declarations (`fun<T>(T): T`); v1 preproc may emit empty.
    Function {
        params: Vec<TypeNode>,
        returns: Box<TypeNode>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        generics: Vec<String>,
    },
    /// Named reference, optionally with generic args.
    Named {
        name: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<TypeNode>,
    },
    /// `any` escape hatch.
    Any,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeField {
    pub key: String,
    pub ty: TypeNode,
    /// `f?: T` syntax — VALUE is optional but key is required. Distinct
    /// from `T?` (the value itself is `T | nil`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
}

// --- Pattern tree ----------------------------------------------------------

/// Structured match pattern carried by `MatchArmAnnotation.pattern`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum PatternNode {
    /// Literal: numbers, strings, booleans, `nil` (serialized as JSON `null`).
    Literal { value: serde_json::Value },
    /// Inclusive numeric range `lo..=hi`. `inclusive` is forward-compat for
    /// a future exclusive variant; v1 always emits `true`.
    Range { lo: f64, hi: f64, inclusive: bool },
    /// Type-only check (no binding). Used inside other constructs or for
    /// arms that only need the type-check without capturing the value.
    Type { name: String },
    /// Capture binding. Optional `ty` is the inline type annotation
    /// (`n: number`); `None` is a bare capture (any value).
    Bind {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ty: Option<String>,
    },
    /// Alternation: `100 | 101 | 102`. Or-pattern alternatives MUST bind the
    /// same set of names (producer enforces).
    Or { items: Vec<PatternNode> },
    /// Table destructure: `{ code = c, msg = m }`.
    Table { fields: Vec<TableField> },
    /// Tuple destructure for multi-arg match: `(a, b)` in
    /// `match (x, y) with`. Lowered as multi-local destructure.
    Tuple { items: Vec<PatternNode> },
    /// As-binding: `pat @ name`. Inner pattern matches AND the whole
    /// matched value gets bound to `name`.
    As {
        name: String,
        inner: Box<PatternNode>,
    },
    /// `_` wildcard. Matches anything, binds nothing.
    Wildcard,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableField {
    pub key: String,
    pub pattern: PatternNode,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Round-trip helper — encode T, decode back, assert equality.
    fn roundtrip<T>(value: &T)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        let s = serde_json::to_string(value).expect("serialize");
        let back: T = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(value, &back, "round-trip mismatch via JSON: {s}");
    }

    #[test]
    fn match_begin_roundtrips() {
        let value = MatchBeginAnnotation {
            v: 1,
            mid: "m1".into(),
            scrut: "http_status".into(),
            arm_count: 8,
        };
        roundtrip(&value);
    }

    #[test]
    fn match_arm_with_guard_roundtrips() {
        let value = MatchArmAnnotation {
            v: 1,
            mid: "m1".into(),
            idx: 1,
            pattern: PatternNode::Bind {
                name: "n".into(),
                ty: Some("number".into()),
            },
            guard: Some("n >= 400".into()),
        };
        roundtrip(&value);
    }

    #[test]
    fn match_arm_without_guard_omits_field_in_json() {
        let value = MatchArmAnnotation {
            v: 1,
            mid: "m1".into(),
            idx: 0,
            pattern: PatternNode::Literal { value: json!(200) },
            guard: None,
        };
        let s = serde_json::to_string(&value).unwrap();
        assert!(!s.contains("guard"), "guard=None must skip: {s}");
        roundtrip(&value);
    }

    #[test]
    fn match_end_with_and_without_result_roundtrips() {
        roundtrip(&MatchEndAnnotation {
            v: 1,
            mid: "m1".into(),
            result: Some("_stdoc_r1".into()),
        });
        roundtrip(&MatchEndAnnotation {
            v: 1,
            mid: "m1".into(),
            result: None,
        });
    }

    #[test]
    fn safe_nav_op_serializes_lowercase() {
        let s = serde_json::to_string(&SafeNavAnnotation {
            v: 1,
            source: "ctx?.task".into(),
            target: "_stdoc_sn1".into(),
            op: SafeNavOp::Member,
        })
        .unwrap();
        assert!(s.contains("\"op\":\"member\""), "{s}");
    }

    #[test]
    fn safe_nav_roundtrips_both_ops() {
        roundtrip(&SafeNavAnnotation {
            v: 1,
            source: "ctx?.task".into(),
            target: "_stdoc_sn1".into(),
            op: SafeNavOp::Member,
        });
        roundtrip(&SafeNavAnnotation {
            v: 1,
            source: "obj?:method()".into(),
            target: "_stdoc_sn2".into(),
            op: SafeNavOp::Call,
        });
    }

    #[test]
    fn compound_op_roundtrips() {
        roundtrip(&CompoundOpAnnotation {
            v: 1,
            op: "+=".into(),
            lhs: "x".into(),
            rhs: "5".into(),
        });
    }

    #[test]
    fn type_strip_site_serializes_lowercase() {
        let s = serde_json::to_string(&TypeStripAnnotation {
            v: 1,
            ident: "x".into(),
            ty: TypeNode::Primitive {
                name: "number".into(),
            },
            site: TypeStripSite::Param,
        })
        .unwrap();
        assert!(s.contains("\"site\":\"param\""), "{s}");
    }

    #[test]
    fn type_node_function_with_generics_roundtrips() {
        let ty = TypeNode::Function {
            params: vec![TypeNode::Named {
                name: "T".into(),
                args: vec![],
            }],
            returns: Box::new(TypeNode::Named {
                name: "T".into(),
                args: vec![],
            }),
            generics: vec!["T".into()],
        };
        roundtrip(&ty);
    }

    #[test]
    fn type_node_record_with_optional_field_roundtrips() {
        let ty = TypeNode::Record {
            fields: vec![
                TypeField {
                    key: "x".into(),
                    ty: TypeNode::Primitive {
                        name: "number".into(),
                    },
                    optional: false,
                },
                TypeField {
                    key: "label".into(),
                    ty: TypeNode::Primitive {
                        name: "string".into(),
                    },
                    optional: true,
                },
            ],
        };
        let s = serde_json::to_string(&ty).unwrap();
        // Required field must skip `optional`; optional field must carry it.
        assert!(
            s.contains("{\"key\":\"x\",\"ty\":{\"kind\":\"primitive\",\"name\":\"number\"}}"),
            "{s}"
        );
        assert!(s.contains("\"optional\":true"), "{s}");
        roundtrip(&ty);
    }

    #[test]
    fn type_node_literal_union_with_mixed_types_roundtrips() {
        let ty = TypeNode::LiteralUnion {
            values: vec![json!(200), json!("GET"), json!(true), json!(null)],
        };
        let s = serde_json::to_string(&ty).unwrap();
        assert!(s.contains("\"kind\":\"literalunion\""), "{s}");
        roundtrip(&ty);
    }

    #[test]
    fn type_decl_with_generics_and_location_roundtrips() {
        roundtrip(&TypeDeclAnnotation {
            v: 1,
            name: "Maybe".into(),
            ty: TypeNode::Named {
                name: "T".into(),
                args: vec![],
            },
            generics: vec!["T".into()],
            location: Some(SourceSpan {
                start_line: 10,
                end_line: 14,
            }),
        });
    }

    #[test]
    fn type_decl_without_generics_skips_field() {
        let s = serde_json::to_string(&TypeDeclAnnotation {
            v: 1,
            name: "Point".into(),
            ty: TypeNode::Record { fields: vec![] },
            generics: vec![],
            location: None,
        })
        .unwrap();
        assert!(!s.contains("generics"), "empty generics must skip: {s}");
        assert!(!s.contains("location"), "None location must skip: {s}");
    }

    #[test]
    fn pattern_node_or_with_mixed_alternatives_roundtrips() {
        let pat = PatternNode::Or {
            items: vec![
                PatternNode::Literal { value: json!(100) },
                PatternNode::Literal { value: json!(101) },
                PatternNode::Range {
                    lo: 200.0,
                    hi: 299.0,
                    inclusive: true,
                },
            ],
        };
        roundtrip(&pat);
    }

    #[test]
    fn pattern_node_table_destructure_roundtrips() {
        let pat = PatternNode::Table {
            fields: vec![
                TableField {
                    key: "code".into(),
                    pattern: PatternNode::Bind {
                        name: "c".into(),
                        ty: None,
                    },
                },
                TableField {
                    key: "msg".into(),
                    pattern: PatternNode::Bind {
                        name: "m".into(),
                        ty: None,
                    },
                },
            ],
        };
        roundtrip(&pat);
    }

    #[test]
    fn pattern_node_tuple_with_nested_table_roundtrips() {
        let pat = PatternNode::Tuple {
            items: vec![
                PatternNode::Table {
                    fields: vec![TableField {
                        key: "kind".into(),
                        pattern: PatternNode::Literal {
                            value: json!("click"),
                        },
                    }],
                },
                PatternNode::Literal {
                    value: json!("idle"),
                },
            ],
        };
        roundtrip(&pat);
    }

    #[test]
    fn pattern_node_as_binding_roundtrips() {
        let pat = PatternNode::As {
            name: "n".into(),
            inner: Box::new(PatternNode::Range {
                lo: 200.0,
                hi: 299.0,
                inclusive: true,
            }),
        };
        roundtrip(&pat);
    }

    #[test]
    fn pattern_node_wildcard_serializes_as_kind_only() {
        let pat = PatternNode::Wildcard;
        let s = serde_json::to_string(&pat).unwrap();
        assert_eq!(s, "{\"kind\":\"wildcard\"}", "{s}");
        roundtrip(&pat);
    }

    #[test]
    fn pattern_node_literal_nil_serializes_as_json_null() {
        let pat = PatternNode::Literal {
            value: serde_json::Value::Null,
        };
        let s = serde_json::to_string(&pat).unwrap();
        assert_eq!(s, "{\"kind\":\"literal\",\"value\":null}", "{s}");
        roundtrip(&pat);
    }

    #[test]
    fn parsing_payload_tolerates_unknown_fields() {
        // Forward-compat: producers MAY add fields the v1 consumer doesn't
        // know about. Serde default must ignore unknown keys.
        let json = r#"{"v":1,"mid":"m1","scrut":"x","arm_count":1,"future_field":"ignored"}"#;
        let value: MatchBeginAnnotation = serde_json::from_str(json).expect("tolerate unknown");
        assert_eq!(value.mid, "m1");
    }
}


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

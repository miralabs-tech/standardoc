#![allow(clippy::case_sensitive_file_extension_comparisons)]

use super::super::walk::walk;
use standardoc_ir::{EdgeKind, ResolvedOrUnresolved};

fn parse(src: &str) -> syn::File {
    syn::parse_file(src).expect("test source not parsable")
}

fn calls(edges: &[standardoc_ir::RawEdge]) -> Vec<&standardoc_ir::RawEdge> {
    edges.iter().filter(|e| e.kind == EdgeKind::Calls).collect()
}

#[test]
fn expr_call_local_function_is_resolved_against_defined_fqdn() {
    let parsed = parse("fn bar() {} fn caller() { bar(); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let cs = calls(&edges);
    assert_eq!(cs.len(), 1);
    assert_eq!(cs[0].from_fqdn, "c::caller");
    match &cs[0].to {
        ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "c::bar"),
        other => panic!("expected resolved, got {other:?}"),
    }
}

#[test]
fn expr_call_unknown_external_is_unresolved_canonical() {
    let parsed = parse("fn caller() { std::mem::take(&mut 0); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let cs = calls(&edges);
    assert_eq!(cs.len(), 1);
    match &cs[0].to {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "std::mem::take"),
        other => panic!("expected unresolved as-written, got {other:?}"),
    }
}

#[test]
fn expr_call_via_alias_resolves_to_canonical() {
    let parsed = parse("use foo::bar; fn caller() { bar(); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let cs = calls(&edges);
    assert_eq!(cs.len(), 1);
    match &cs[0].to {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "foo::bar"),
        other => panic!("expected unresolved canonical via alias, got {other:?}"),
    }
}

#[test]
fn expr_method_call_is_always_unresolved_with_method_ident() {
    let parsed = parse("fn caller() { let v = vec![1]; v.iter().count(); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    // Filter out the `vec!` macro edge — this test is about method calls.
    let cs: Vec<_> = calls(&edges)
        .into_iter()
        .filter(|e| !e.attributes.iter().any(|a| a == "macro"))
        .collect();
    // Two ExprMethodCall: .iter() and .count().
    assert_eq!(cs.len(), 2);
    let names: Vec<_> = cs
        .iter()
        .map(|e| match &e.to {
            ResolvedOrUnresolved::Unresolved { name } => name.clone(),
            _ => panic!("expected unresolved method call"),
        })
        .collect();
    assert!(names.contains(&"iter".to_string()));
    assert!(names.contains(&"count".to_string()));
}

#[test]
fn nested_calls_in_arguments_are_captured() {
    let parsed = parse("fn a() {} fn b() {} fn caller() { a(); b(); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let cs = calls(&edges);
    assert_eq!(cs.len(), 2);
}

#[test]
fn impl_fn_body_calls_attributed_to_method_fqdn() {
    let parsed = parse("fn helper() {} struct F; impl F { fn run(&self) { helper(); } }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let cs = calls(&edges);
    assert_eq!(cs.len(), 1);
    assert_eq!(cs[0].from_fqdn, "c::F::run");
}

#[test]
fn trait_default_body_calls_attributed_to_trait_fn_fqdn() {
    let parsed = parse("fn helper() {} trait T { fn run(&self) { helper(); } }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let cs = calls(&edges);
    assert_eq!(cs.len(), 1);
    assert_eq!(cs[0].from_fqdn, "c::T::run");
}

#[test]
fn self_method_in_impl_block_resolves_to_target_type() {
    // `impl Foo { fn new() -> Self {...} fn other(&self) { Self::new() } }`
    // — `Self::new` inside `other()` should resolve to `c::Foo::new`,
    // not stay as the bogus `Self::new` text-as-written.
    let parsed =
        parse("struct Foo; impl Foo { fn new() -> Self { Foo } fn other(&self) { Self::new(); } }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let cs = calls(&edges);
    assert_eq!(cs.len(), 1, "exactly one Self::new call expected");
    match &cs[0].to {
        ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "c::Foo::new"),
        other => panic!("expected Resolved c::Foo::new, got {other:?}"),
    }
    assert_eq!(cs[0].from_fqdn, "c::Foo::other");
}

#[test]
fn self_default_in_impl_block_resolves_via_substitution() {
    // `Self::default()` inside an impl Foo block must rewrite to
    // `c::Foo::default` before resolution. Since `default` isn't
    // defined in this file, the edge stays Unresolved BUT with the
    // composed canonical name (no longer the raw `Self::default`).
    let parsed = parse("struct Foo; impl Foo { fn other(&self) { Self::default(); } }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let cs = calls(&edges);
    assert_eq!(cs.len(), 1);
    match &cs[0].to {
        ResolvedOrUnresolved::Unresolved { name } => {
            assert!(
                name.contains("Foo::default"),
                "Self was not substituted, got {name:?}"
            );
            assert!(!name.contains("Self::"), "Self leaked, got {name:?}");
        }
        other => panic!("expected Unresolved with Self substituted, got {other:?}"),
    }
}

#[test]
fn self_outside_impl_block_stays_unresolved() {
    // `Self::xxx` inside a free fn or trait default body has no
    // concrete self_type to substitute. The CallVisitor's self_type
    // is None, so substitution is a no-op and the path goes through
    // resolve_name verbatim — Unresolved with the original text.
    let parsed = parse("fn outside() { Self::new(); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let cs = calls(&edges);
    // resolve_path's module-local fallback prefixes the current
    // module, so the unresolved name becomes `c::Self::new`. The
    // key invariant is that no spurious Foo-substitution happened.
    assert!(
        cs.iter().any(|e| match &e.to {
            ResolvedOrUnresolved::Unresolved { name } => name.contains("Self::new"),
            _ => false,
        }),
        "Self::new outside impl must stay as Self::new in the edge target, got {cs:?}"
    );
}

#[test]
fn async_block_body_is_walked_for_calls() {
    let parsed = parse("fn outside() {} fn caller() { let _ = async { outside(); }; }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let cs = calls(&edges);
    let names: Vec<_> = cs
        .iter()
        .map(|e| match &e.to {
            ResolvedOrUnresolved::Resolved { fqdn } => fqdn.clone(),
            ResolvedOrUnresolved::Unresolved { name }
            | ResolvedOrUnresolved::UnresolvedBridge { name, .. } => name.clone(),
        })
        .collect();
    assert!(
        names.contains(&"c::outside".to_string()),
        "async block must surface inner calls, got {names:?}"
    );
}

#[test]
fn user_macro_invocation_emits_call_edge_with_macro_attribute() {
    // User-defined macro (not in the Drop-tier builtin registry) still
    // emits a Calls edge so plugins can reach the macro definition.
    let parsed = parse("fn caller() { my_user_macro!(\"hi\"); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let cs = calls(&edges);
    assert_eq!(cs.len(), 1, "exactly one user-macro call expected");
    assert!(
        cs[0].attributes.iter().any(|a| a == "macro"),
        "macro call edge must carry attribute=`macro`, got {:?}",
        cs[0].attributes
    );
    match &cs[0].to {
        ResolvedOrUnresolved::Unresolved { name } => {
            assert!(name.ends_with("my_user_macro"), "macro target = {name:?}");
        }
        other => panic!("expected unresolved macro target, got {other:?}"),
    }
}

#[test]
fn builtin_macro_invocation_drops_calls_edge_keeps_call_site() {
    // Bug E-1: `println!` / `assert!` / `panic!` / `vec!` / etc. are
    // Drop-tier in the builtin registry. The Calls edge is suppressed
    // (graph noise) but the RawCallSite row is still emitted upstream
    // of the registry check so consumers wanting raw macro counts
    // continue to get them.
    let parsed = parse("fn caller() { println!(\"hi\"); assert!(true); panic!(\"x\"); vec![1]; }");
    let (_, edges, _, call_sites) = walk(&parsed, "c", "src/lib.rs", "c");
    let macro_edges: Vec<_> = calls(&edges)
        .into_iter()
        .filter(|e| e.attributes.iter().any(|a| a == "macro"))
        .collect();
    assert!(
        macro_edges.is_empty(),
        "builtin Drop-tier macros must not emit Calls edges, got {macro_edges:?}"
    );
    let macro_callees: Vec<_> = call_sites
        .iter()
        .filter(|c| c.callee_text.ends_with('!'))
        .map(|c| c.callee_text.as_str())
        .collect();
    for expected in ["println!", "assert!", "panic!", "vec!"] {
        assert!(
            macro_callees.contains(&expected),
            "call_site for {expected} must still be emitted, got {macro_callees:?}"
        );
    }
}

#[test]
fn macro_invocation_args_remain_opaque() {
    // Tokens inside `println!(...)` are NOT parsed as Rust; only the path is captured.
    // After Bug E-1 the println! Calls edge itself is dropped (Drop-tier), but the
    // contract under test is that `outside()` doesn't leak out of the opaque token
    // stream — which still holds.
    let parsed = parse("fn outside() {} fn caller() { println!(\"{}\", outside()); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let cs = calls(&edges);
    let outside_walked = cs.iter().any(|e| match &e.to {
        ResolvedOrUnresolved::Resolved { fqdn } => fqdn == "c::outside",
        _ => false,
    });
    assert!(!outside_walked, "macro tokens must remain opaque");
}

#[test]
fn closure_body_is_walked_for_calls() {
    let parsed = parse("fn inner() {} fn caller() { let f = || inner(); f(); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let cs = calls(&edges);
    // ExprCall captures: inner() inside closure + f() outside (f is not a Path,
    // it's an ExprPath to local var → emitted with name "f", not in defined_fqdns).
    let names: Vec<_> = cs
        .iter()
        .map(|e| match &e.to {
            ResolvedOrUnresolved::Resolved { fqdn } => fqdn.clone(),
            ResolvedOrUnresolved::Unresolved { name }
            | ResolvedOrUnresolved::UnresolvedBridge { name, .. } => name.clone(),
        })
        .collect();
    assert!(names.contains(&"c::inner".to_string()));
}

#[test]
fn turbofish_call_drops_generics_in_emitted_path() {
    // Multi-segment unaliased path stays text-as-written. Generic
    // stripping itself is validated here with a non-builtin name so
    // Stage 3e-1 tier gating doesn't interfere (`Vec` is now Drop
    // and would skip the edge entirely — covered separately).
    let parsed = parse("fn caller() { MyType::<u8>::new(); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let cs = calls(&edges);
    assert_eq!(cs.len(), 1);
    match &cs[0].to {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "MyType::new"),
        other => panic!("got {other:?}"),
    }
}

#[test]
fn stage3e1_call_drop_tier_builtin_skipped() {
    // `Vec` is registered as `BuiltinTier::Drop` — `Vec::new()` is
    // structural noise and produces no `Calls` edge. The body still
    // walks for any inner-call recursion (none here).
    let parsed = parse("fn caller() { Vec::<u8>::new(); String::new(); Box::new(0u8); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let cs = calls(&edges);
    assert!(
        cs.is_empty(),
        "Drop-tier builtin calls must not surface, got {cs:?}"
    );
}

#[test]
fn stage3e1_call_drop_skips_but_args_still_walked() {
    // The Drop-tier wrapper (`Some`) is skipped, but the call inside
    // the args (`inner()`) must still surface — the visitor recurses
    // through `syn::visit::visit_expr_call` after the tier decision.
    let parsed = parse("fn inner() {} fn caller() { Some(inner()); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let names: Vec<_> = calls(&edges)
        .into_iter()
        .filter_map(|e| match &e.to {
            ResolvedOrUnresolved::Resolved { fqdn } => Some(fqdn.clone()),
            _ => None,
        })
        .collect();
    assert!(
        names.contains(&"c::inner".to_string()),
        "inner() call must surface even when wrapped in Some(_), got {names:?}"
    );
}

// --- Stage 3e-2: References emit via Expr::Path ---

fn refs(edges: &[standardoc_ir::RawEdge]) -> Vec<&standardoc_ir::RawEdge> {
    edges
        .iter()
        .filter(|e| e.kind == EdgeKind::References)
        .collect()
}

fn value_reads(edges: &[standardoc_ir::RawEdge]) -> Vec<&standardoc_ir::RawEdge> {
    refs(edges)
        .into_iter()
        .filter(|e| e.attributes.iter().any(|a| a == "value-read"))
        .collect()
}

#[test]
fn stage3e2_module_local_fn_pointer_emits_value_read_reference() {
    // `foo` in value position resolves to the module-local fn; emit
    // a References edge with `value-read`.
    let parsed = parse("fn foo() {} fn caller() { let _ = foo; }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let vr = value_reads(&edges);
    assert_eq!(vr.len(), 1, "exactly one value-read expected, got {vr:?}");
    assert_eq!(vr[0].from_fqdn, "c::caller");
    match &vr[0].to {
        ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "c::foo"),
        other => panic!("expected resolved c::foo, got {other:?}"),
    }
}

#[test]
fn stage3e2_call_does_not_double_emit_as_reference() {
    // `bar()` should emit ONE Calls edge and ZERO References edges —
    // the manual arg-only recursion in visit_expr_call must suppress
    // the visit_expr_path fire on `node.func`.
    let parsed = parse("fn bar() {} fn caller() { bar(); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert_eq!(calls(&edges).len(), 1);
    assert!(
        refs(&edges).is_empty(),
        "call-expr func must not surface as a value-read, got {:?}",
        refs(&edges)
    );
}

#[test]
fn stage3e2_unresolved_path_does_not_emit_reference() {
    // Stage-1 safety net: an unindexed identifier in value position
    // produces no References edge (would create noise pointing at
    // unresolved targets).
    let parsed = parse("fn caller() { let _ = unknown_thing; }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert!(
        refs(&edges).is_empty(),
        "unresolved value-reads must be skipped, got {:?}",
        refs(&edges)
    );
}

#[test]
fn stage3e2_local_binding_value_read_is_skipped() {
    // `let x = 0; let _ = x;` — `x` is a local binding (nested
    // scope), so the read must not emit a References edge.
    let parsed = parse("fn caller() { let x = 0; let _ = x; }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert!(
        refs(&edges).is_empty(),
        "local value-read must be skipped, got {:?}",
        refs(&edges)
    );
}

#[test]
fn stage3e2_self_reference_is_skipped() {
    // `fn caller() { let _ = caller; }` — reading own name in value
    // position is dropped (recursion patterns are covered by Calls,
    // value-position self-refs are usually structural).
    let parsed = parse("fn caller() { let _ = caller; }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert!(
        refs(&edges).is_empty(),
        "self-reference must be skipped, got {:?}",
        refs(&edges)
    );
}

#[test]
fn stage3e2_drop_tier_builtin_value_read_is_skipped() {
    // `Vec` in value position (as fn-pointer or generic carrier) is
    // Drop-tier → no References edge.
    let parsed = parse("fn caller() { let _ = Vec::<u8>::new; }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert!(
        refs(&edges).is_empty(),
        "Drop-tier builtin value-read must be skipped, got {:?}",
        refs(&edges)
    );
}

#[test]
fn stage3e2_attribute_tier_builtin_value_read_promotes_flag() {
    // `Iterator` is Attribute-tier — reading it in value position
    // (rare but possible: trait-object construction) must NOT emit
    // an edge but MUST promote `iter` flag onto the enclosing fn.
    let parsed = parse(
        "fn caller() { let _: fn() -> &'static dyn Iterator<Item = u8> = || todo!(); let _ = Iterator::count; }",
    );
    let (symbols, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert!(
        refs(&edges).is_empty(),
        "Attribute-tier builtin value-read must skip the edge, got {:?}",
        refs(&edges)
    );
    let caller = symbols.iter().find(|s| s.fqdn == "c::caller").unwrap();
    assert!(
        caller.flags.iter().any(|f| f == "iter"),
        "Attribute-tier builtin value-read must register `iter` flag, got {:?}",
        caller.flags
    );
}

#[test]
fn stage3e2_multi_segment_resolved_path_emits_value_read() {
    // `MyType::CONST` — multi-segment value-read. The leftmost
    // segment isn't a builtin, so the full path goes through
    // `resolve_path` which canonicalizes against alias_table.
    let parsed = parse("use other::MyType; fn caller() { let _ = MyType::CONST; }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    // `MyType::CONST` resolves to canonical `other::MyType::CONST`
    // which isn't in defined_fqdns → Unresolved → no edge emitted
    // (Stage 1 safety net for unresolved value-reads).
    // The test guards the absence of noise: no References edges
    // emitted in this case, since the canonical target isn't local.
    assert!(
        refs(&edges).is_empty(),
        "unresolved canonical multi-segment path must skip emit, got {:?}",
        refs(&edges)
    );
}

#[test]
fn stage3e2_method_receiver_path_emits_value_read() {
    // `foo.bar()` — `foo` is a legitimate value-read on the receiver.
    // The default visit_expr_method_call recursion walks the receiver
    // via visit_expr → visit_expr_path, so we should see a References
    // edge on `foo`.
    let parsed = parse("fn foo() {} fn caller() { foo.bar(); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let vr = value_reads(&edges);
    // `foo` reads as `c::foo` (Resolved).
    let foo_refs: Vec<_> = vr
        .iter()
        .filter(|e| matches!(&e.to, ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "c::foo"))
        .collect();
    assert_eq!(
        foo_refs.len(),
        1,
        "exactly one value-read on receiver `foo` expected, got {vr:?}"
    );
}

#[test]
fn stage3e2_value_read_dedupes_against_unresolved_module_local() {
    // Bare `bar` resolves to `c::bar` which isn't defined → Unresolved.
    // Per Stage 1 safety net, no References edge.
    let parsed = parse("fn caller() { let _ = bar; }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert!(
        refs(&edges).is_empty(),
        "unresolved bare ident value-read must be skipped, got {:?}",
        refs(&edges)
    );
}

// --- Stage 3e-2-bis: Rust let-binding alias propagation ---

#[test]
fn stage3e2bis_let_binding_propagates_alias_with_via_alias_slug() {
    // `let x = bar;` makes `x` an alias for `bar`. Reading `x`
    // surfaces as a References edge to `c::bar` with `via-alias`.
    let parsed = parse("fn bar() {} fn caller() { let x = bar; let _ = x; }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let vr = value_reads(&edges);
    // One value-read through alias (`x`) + one direct value-read on
    // the RHS (`bar` in `let x = bar;` is also an Expr::Path read).
    // Both must resolve to c::bar.
    let alias_refs: Vec<_> = vr
        .iter()
        .filter(|e| e.attributes.iter().any(|a| a == "via-alias"))
        .collect();
    assert_eq!(
        alias_refs.len(),
        1,
        "exactly one via-alias edge expected, got {vr:?}"
    );
    match &alias_refs[0].to {
        ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "c::bar"),
        other => panic!("expected resolved c::bar, got {other:?}"),
    }
}

#[test]
fn stage3e2bis_let_mut_binding_propagates_alias_with_via_alias_mutable_slug() {
    // `let mut x = bar;` — the binding is mutable, so the alias is
    // tagged `via-alias-mutable` to signal staleness risk.
    let parsed = parse("fn bar() {} fn caller() { let mut x = bar; let _ = x; }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let mutable_refs: Vec<_> = value_reads(&edges)
        .into_iter()
        .filter(|e| e.attributes.iter().any(|a| a == "via-alias-mutable"))
        .collect();
    assert_eq!(
        mutable_refs.len(),
        1,
        "exactly one via-alias-mutable edge expected, got {:?}",
        value_reads(&edges)
    );
    match &mutable_refs[0].to {
        ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "c::bar"),
        other => panic!("expected resolved c::bar, got {other:?}"),
    }
}

#[test]
fn stage3e2bis_non_path_rhs_does_not_create_alias() {
    // `let x = bar();` — RHS is a Call, not an alias-worthy Path.
    // `x` becomes an opaque Local → reading it surfaces nothing.
    let parsed = parse("fn bar() -> u32 { 0 } fn caller() { let x = bar(); let _ = x; }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    // The `bar()` call still emits a Calls edge.
    let cs = calls(&edges);
    assert!(cs.iter().any(|e| matches!(&e.to,
        ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "c::bar"
    )));
    // But `x` read should NOT produce a References edge — x is Local.
    assert!(
        refs(&edges).is_empty(),
        "non-Path RHS must not create alias propagation, got {:?}",
        refs(&edges)
    );
}

#[test]
fn stage3e2bis_type_annotated_let_still_aliases() {
    // `let x: fn() = bar;` — the Pat::Type wrapper around Pat::Ident
    // must be peeled so alias detection still fires.
    let parsed = parse("fn bar() {} fn caller() { let x: fn() = bar; let _ = x; }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let alias_refs: Vec<_> = value_reads(&edges)
        .into_iter()
        .filter(|e| e.attributes.iter().any(|a| a == "via-alias"))
        .collect();
    assert_eq!(
        alias_refs.len(),
        1,
        "type-annotated let must still alias, got {:?}",
        value_reads(&edges)
    );
}

// --- Stage 3e-2-ter: scope-aware visit_expr_call ---

#[test]
fn stage3e2ter_let_alias_call_propagates_via_alias() {
    // `let f = bar; f();` — Stage 3e-2-bis registered `f` as alias
    // for `bar` (Const mutability). Stage 3e-2-ter routes the call
    // through resolve_name → emits Calls to c::bar with `via-alias`.
    let parsed = parse("fn bar() {} fn caller() { let f = bar; f(); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let alias_calls: Vec<_> = calls(&edges)
        .into_iter()
        .filter(|e| e.attributes.iter().any(|a| a == "via-alias"))
        .collect();
    assert_eq!(
        alias_calls.len(),
        1,
        "exactly one via-alias Calls edge expected, got {:?}",
        calls(&edges)
    );
    match &alias_calls[0].to {
        ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "c::bar"),
        other => panic!("expected resolved c::bar, got {other:?}"),
    }
}

#[test]
fn stage3e2ter_let_mut_alias_call_via_alias_mutable() {
    let parsed = parse("fn bar() {} fn caller() { let mut f = bar; f(); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let mutable_calls: Vec<_> = calls(&edges)
        .into_iter()
        .filter(|e| e.attributes.iter().any(|a| a == "via-alias-mutable"))
        .collect();
    assert_eq!(
        mutable_calls.len(),
        1,
        "expected one via-alias-mutable call, got {:?}",
        calls(&edges)
    );
}

#[test]
fn stage3e2ter_fn_pointer_param_call_is_skipped() {
    // `fn caller(cb: fn()) { cb(); }` — `cb` is a Local (Param) in
    // the caller's scope. Calling it emits NO edge (mirror of TS
    // Bug B Stage 2 — locals in call position are skipped).
    let parsed = parse("fn caller(cb: fn()) { cb(); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert!(
        calls(&edges).is_empty(),
        "fn-pointer param call must not emit a Calls edge, got {:?}",
        calls(&edges)
    );
}

#[test]
fn stage3e2ter_closure_typed_local_call_is_skipped() {
    // `let f = || {}; f();` — RHS is a closure (not Expr::Path), so
    // no alias is registered for `f`. Reading `f` in call position
    // resolves to a Local without alias → skip.
    let parsed = parse("fn caller() { let f = || {}; f(); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let non_macro: Vec<_> = calls(&edges)
        .into_iter()
        .filter(|e| !e.attributes.iter().any(|a| a == "macro"))
        .collect();
    assert!(
        non_macro.is_empty(),
        "closure-typed local call must not emit a Calls edge, got {non_macro:?}"
    );
}

#[test]
fn stage3e2ter_module_level_fn_call_still_resolves_post_refactor() {
    // Regression guard: routing through resolve_name MUST preserve
    // the pre-3e-2-ter behavior for plain module-local fn calls.
    let parsed = parse("fn bar() {} fn caller() { bar(); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let cs = calls(&edges);
    assert_eq!(cs.len(), 1);
    // No alias/builtin attrs on a direct module-local call.
    assert!(
        cs[0].attributes.is_empty(),
        "direct call must carry no attrs, got {:?}",
        cs[0].attributes
    );
    match &cs[0].to {
        ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "c::bar"),
        other => panic!("expected resolved c::bar, got {other:?}"),
    }
}

#[test]
fn stage3e2ter_self_recursion_still_emits_calls_edge() {
    // Self-reference exclusion applies to References (value-position),
    // NOT to Calls — `fn foo() { foo(); }` is a legitimate recursion
    // signal that must surface as a Calls edge.
    let parsed = parse("fn foo() { foo(); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let self_calls: Vec<_> = calls(&edges)
        .into_iter()
        .filter(|e| {
            e.from_fqdn == "c::foo"
                && matches!(&e.to, ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "c::foo")
        })
        .collect();
    assert_eq!(
        self_calls.len(),
        1,
        "recursion must emit a self-loop Calls edge, got {:?}",
        calls(&edges)
    );
}

#[test]
fn stage3e2ter_drop_tier_call_still_skipped_via_resolve_name() {
    // Regression: Drop tier dispatch must still work post-refactor.
    let parsed = parse("fn caller() { Vec::<u8>::new(); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert!(
        calls(&edges).is_empty(),
        "Drop-tier builtin call must be skipped, got {:?}",
        calls(&edges)
    );
}

#[test]
fn stage3e2bis_destructuring_pattern_falls_through_to_bind_pat() {
    // `let (x, y) = (bar, baz);` — tuple destructuring isn't an
    // alias (the RHS is a Tuple expr, not a Path), so x and y stay
    // opaque Locals. Reading them produces no References edge.
    let parsed = parse(
        "fn bar() {} fn baz() {} fn caller() { let (x, y) = (bar, baz); let _ = x; let _ = y; }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    // The tuple expr's inner Expr::Path reads on bar and baz DO emit
    // value-reads at the RHS expression itself. But x/y reads on the
    // bottom lines must NOT emit (locals without alias).
    let resolved_targets: Vec<_> = value_reads(&edges)
        .iter()
        .filter_map(|e| match &e.to {
            ResolvedOrUnresolved::Resolved { fqdn } => Some(fqdn.clone()),
            _ => None,
        })
        .collect();
    // Should see c::bar and c::baz from the RHS tuple (2 reads),
    // NOT 4 (no x/y propagation).
    assert_eq!(
        resolved_targets.len(),
        2,
        "destructuring must not create alias propagation, got {resolved_targets:?}"
    );
    assert!(resolved_targets.contains(&"c::bar".to_string()));
    assert!(resolved_targets.contains(&"c::baz".to_string()));
}

// --- IR-4-b: call_sites emission (observational, separate from edges) ---

fn call_sites_of(parsed: &syn::File) -> Vec<standardoc_ir::RawCallSite> {
    let (_, _, _, css) = walk(parsed, "c", "src/lib.rs", "c");
    css
}

#[test]
fn ir4b_path_form_call_emits_call_site_with_classified_args() {
    // `foo("hi", 42, x)` — three positional args: a string literal,
    // a numeric literal, and an identifier. Each must surface with
    // the correct `is_string_literal` flag.
    let parsed = parse("fn caller() { let x = 0; foo(\"hi\", 42, x); }");
    let css = call_sites_of(&parsed);
    let cs = css
        .iter()
        .find(|c| c.callee_text == "foo")
        .unwrap_or_else(|| panic!("expected a call_site for foo(...), got {css:?}"));
    assert_eq!(cs.from_fqdn, "c::caller");
    assert!(cs.receiver_chain.is_empty());
    assert_eq!(cs.args.len(), 3);
    assert_eq!(cs.args[0].value, "hi");
    assert!(cs.args[0].is_string_literal);
    assert_eq!(cs.args[1].value, "42");
    assert!(!cs.args[1].is_string_literal);
    assert_eq!(cs.args[2].value, "x");
    assert!(!cs.args[2].is_string_literal);
}

#[test]
fn ir4b_multi_segment_path_call_keeps_dotted_callee_text() {
    // `tauri::invoke("ping")` — multi-segment path is preserved
    // verbatim in `callee_text`; receiver_chain stays empty (path-
    // form has no receiver concept).
    let parsed = parse("fn caller() { tauri::invoke(\"ping\"); }");
    let css = call_sites_of(&parsed);
    let cs = css
        .iter()
        .find(|c| c.callee_text == "tauri::invoke")
        .unwrap_or_else(|| panic!("expected tauri::invoke call_site, got {css:?}"));
    assert!(cs.receiver_chain.is_empty());
    assert_eq!(cs.args.len(), 1);
    assert_eq!(cs.args[0].value, "ping");
    assert!(cs.args[0].is_string_literal);
}

#[test]
fn ir4b_method_call_emits_receiver_chain_single_segment() {
    // `obj.bar(x)` — single-segment receiver_chain.
    let parsed = parse("fn caller() { let obj = 0; obj.bar(x); }");
    let css = call_sites_of(&parsed);
    let cs = css
        .iter()
        .find(|c| c.callee_text.ends_with(".bar"))
        .unwrap_or_else(|| panic!("expected obj.bar call_site, got {css:?}"));
    assert_eq!(cs.callee_text, "obj.bar");
    assert_eq!(cs.receiver_chain, vec!["obj".to_string()]);
    assert_eq!(cs.args.len(), 1);
    assert_eq!(cs.args[0].value, "x");
}

#[test]
fn ir4b_chained_method_call_walks_field_layers_into_receiver_chain() {
    // `obj.field.bar(x)` — receiver is `ExprField { base: Path("obj"), member: "field" }`.
    // The chain must surface in source order: ["obj", "field"].
    let parsed = parse("fn caller() { let obj = 0; obj.field.bar(x); }");
    let css = call_sites_of(&parsed);
    let cs = css
        .iter()
        .find(|c| c.callee_text.ends_with(".bar"))
        .unwrap_or_else(|| panic!("expected obj.field.bar call_site, got {css:?}"));
    assert_eq!(cs.callee_text, "obj.field.bar");
    assert_eq!(
        cs.receiver_chain,
        vec!["obj".to_string(), "field".to_string()]
    );
}

#[test]
fn ir4b_macro_call_emits_call_site_with_bang_suffix_and_no_args() {
    // `println!("hi")` — macro args are an opaque token stream, so
    // `args` stays empty by design. `callee_text` carries the `!`
    // suffix so consumers can distinguish macros from fn calls.
    let parsed = parse("fn caller() { println!(\"hi\"); }");
    let css = call_sites_of(&parsed);
    let cs = css
        .iter()
        .find(|c| c.callee_text == "println!")
        .unwrap_or_else(|| panic!("expected println! call_site, got {css:?}"));
    assert!(cs.args.is_empty());
    assert!(cs.receiver_chain.is_empty());
}

#[test]
fn ir4b_drop_tier_call_still_emits_call_site() {
    // `Vec::<u8>::new()` — the Drop tier suppresses the graph edge,
    // but the call_site must still surface (observational, plugin-
    // layer reads it regardless of edge-tier decisions). Generics
    // are stripped in `callee_text` via `path_to_string`.
    let parsed = parse("fn caller() { Vec::<u8>::new(); }");
    let (_, edges, _, css) = walk(&parsed, "c", "src/lib.rs", "c");
    assert!(
        calls(&edges).is_empty(),
        "Drop-tier edge suppression must still hold"
    );
    let cs = css
        .iter()
        .find(|c| c.callee_text == "Vec::new")
        .unwrap_or_else(|| panic!("expected Vec::new call_site, got {css:?}"));
    assert!(cs.receiver_chain.is_empty());
}

#[test]
fn ir4b_self_recursive_call_still_emits_call_site() {
    // `fn foo() { foo(); }` — self-recursion preserved as a Calls
    // edge (different from value-position self-refs) AND as a
    // call_site (observational signal of the recursion).
    let parsed = parse("fn foo() { foo(); }");
    let css = call_sites_of(&parsed);
    assert!(
        css.iter().any(|c| c.callee_text == "foo"),
        "self-recursive call must emit a call_site, got {css:?}"
    );
}

#[test]
fn ir4b_call_site_from_fqdn_attributes_to_enclosing_method() {
    // Same as `impl_fn_body_calls_attributed_to_method_fqdn` but
    // checking the call_site path — `from_fqdn` must be the impl
    // method's fqdn, not the module's.
    let parsed = parse("fn helper() {} struct F; impl F { fn run(&self) { helper(); } }");
    let css = call_sites_of(&parsed);
    let cs = css
        .iter()
        .find(|c| c.callee_text == "helper")
        .unwrap_or_else(|| panic!("expected helper call_site, got {css:?}"));
    assert_eq!(cs.from_fqdn, "c::F::run");
}

#[test]
fn ir4b_empty_callee_path_still_falls_through_to_default() {
    // Edge case — `path_to_string` returns an empty string for a
    // path with no segments. The call_site emit short-circuits in
    // that case (the `if !path_str.is_empty()` guard), so no
    // call_site is produced. Guard against regression by checking
    // that pathological inputs don't crash the walk.
    let parsed = parse("fn caller() { (|| {})(); }");
    // `(|| {})()` is a non-path func — we DO emit a call_site with
    // the stringified closure text. Just assert no panic + at
    // least one call_site emitted on the non-path branch.
    let css = call_sites_of(&parsed);
    assert!(
        css.iter().any(|c| !c.callee_text.is_empty()),
        "non-path call must still emit a non-empty call_site"
    );
}

// --- Bug E-3 Phase 1: receiver_type annotation on method-call edges ---

fn method_calls(edges: &[standardoc_ir::RawEdge]) -> Vec<&standardoc_ir::RawEdge> {
    // Method-call edges are the CALLS edges whose `to_unresolved` is a
    // bare ident (no `::`, no `.`). They're what visit_expr_method_call
    // emits — distinct from path-form calls (`Foo::bar()`) and macro calls.
    calls(edges)
        .into_iter()
        .filter(|e| match &e.to {
            ResolvedOrUnresolved::Unresolved { name } => {
                !name.contains("::") && !name.contains('.')
            }
            _ => false,
        })
        .filter(|e| !e.attributes.iter().any(|a| a == "macro"))
        .collect()
}

#[test]
fn method_on_self_attaches_self_type_as_receiver_type() {
    let parsed = parse("struct F; impl F { fn run(&self) { self.helper(); } fn helper(&self) {} }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let mc = method_calls(&edges);
    assert_eq!(mc.len(), 1, "exactly one self.helper() method call");
    assert_eq!(mc[0].receiver_type.as_deref(), Some("c::F"));
}

#[test]
fn method_on_self_field_attaches_field_nominal_type() {
    let parsed = parse(
        "struct Inner; impl Inner { fn ping(&self) {} } \
         struct F { inner: Inner } \
         impl F { fn run(&self) { self.inner.ping(); } }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let mc = method_calls(&edges);
    assert_eq!(mc.len(), 1);
    assert_eq!(mc[0].receiver_type.as_deref(), Some("Inner"));
}

#[test]
fn method_on_typed_param_attaches_param_type() {
    let parsed = parse("fn caller(v: Vec<u8>) { v.iter(); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let mc = method_calls(&edges);
    assert_eq!(mc.len(), 1);
    assert_eq!(mc[0].receiver_type.as_deref(), Some("Vec"));
}

#[test]
fn method_on_annotated_let_attaches_let_type() {
    let parsed = parse("fn caller() { let s: String = String::new(); s.len(); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let mc = method_calls(&edges);
    // s.len() is the only method call (String::new is a path call).
    assert_eq!(mc.len(), 1);
    assert_eq!(mc[0].receiver_type.as_deref(), Some("String"));
}

#[test]
fn method_on_constructor_let_attaches_constructor_type() {
    let parsed = parse("fn caller() { let v = Vec::new(); v.push(1); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let mc = method_calls(&edges);
    // v.push(1) only — `Vec::new` is a path call (Drop tier, filtered).
    assert_eq!(mc.len(), 1);
    assert_eq!(mc[0].receiver_type.as_deref(), Some("Vec"));
}

#[test]
fn method_on_reference_param_strips_reference() {
    let parsed =
        parse("fn caller(x: &mut Foo) { x.run(); } struct Foo; impl Foo { fn run(&mut self) {} }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let mc = method_calls(&edges);
    let on_x: Vec<_> = mc.iter().filter(|e| e.from_fqdn == "c::caller").collect();
    assert_eq!(on_x.len(), 1);
    assert_eq!(on_x[0].receiver_type.as_deref(), Some("Foo"));
}

#[test]
fn method_on_unknown_binding_yields_none() {
    let parsed = parse("fn caller(opaque: impl Trait) { opaque.do_it(); } trait Trait {}");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let mc = method_calls(&edges);
    assert_eq!(mc.len(), 1);
    assert_eq!(mc[0].receiver_type, None);
}

#[test]
fn deep_chain_propagates_via_builtin_returns() {
    // Bug E-3 Phase 3: chained calls inherit receiver_type from the
    // preceding step's registered `returns`. `v.iter()` (Vec) returns
    // Iterator, `.map(...)` keeps Iterator, `.collect(...)`'s receiver
    // is therefore Iterator (collect itself has no registered return —
    // polymorphic — but the edge's receiver_type is still set).
    let parsed =
        parse("fn caller() { let v = Vec::new(); v.iter().map(|x| x).collect::<Vec<_>>(); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let mc = method_calls(&edges);
    assert_eq!(mc.len(), 3, "three method calls in the chain");
    let mut types: Vec<&str> = mc
        .iter()
        .filter_map(|e| e.receiver_type.as_deref())
        .collect();
    types.sort_unstable();
    assert_eq!(types, vec!["Iterator", "Iterator", "Vec"]);
}

#[test]
fn self_field_with_generic_field_type_keeps_nominal() {
    let parsed =
        parse("struct F { items: Vec<u8> } impl F { fn run(&self) { self.items.iter(); } }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let mc = method_calls(&edges);
    assert_eq!(mc.len(), 1);
    assert_eq!(mc[0].receiver_type.as_deref(), Some("Vec"));
}

#[test]
fn path_form_call_carries_no_receiver_type() {
    // Path-form `Foo::bar()` is NOT a method call — receiver_type stays None.
    let parsed = parse("fn caller() { String::new(); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    for e in calls(&edges) {
        assert_eq!(
            e.receiver_type, None,
            "path-form call must have no receiver_type"
        );
    }
}

// --- Bug E-3 Phase 3 chain propagation tests ---

#[test]
fn iterator_adapter_chain_keeps_iterator_type() {
    // Iterator → map → filter → enumerate — every step's receiver_type
    // should resolve to "Iterator" via the builtin returns table.
    let parsed = parse(
        "fn caller() { let v = Vec::new(); \
         v.iter().map(|x| x).filter(|x| true).enumerate(); }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let mc = method_calls(&edges);
    assert_eq!(mc.len(), 4);
    let iter_count = mc
        .iter()
        .filter(|e| e.receiver_type.as_deref() == Some("Iterator"))
        .count();
    // .map, .filter, .enumerate all see Iterator as receiver_type.
    assert_eq!(iter_count, 3, "three adapters after .iter() see Iterator");
}

#[test]
fn str_chars_returns_iterator_for_next_step() {
    let parsed = parse("fn caller(s: &str) { s.chars().count(); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let mc = method_calls(&edges);
    assert_eq!(mc.len(), 2);
    // s.chars() → recv str, count() → recv Iterator
    let count_call = mc
        .iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "count"))
        .expect("count() edge");
    assert_eq!(count_call.receiver_type.as_deref(), Some("Iterator"));
}

#[test]
fn option_map_propagates_option() {
    let parsed = parse("fn caller(x: Option<u8>) { x.map(|v| v + 1).unwrap_or(0); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let mc = method_calls(&edges);
    // x.map → Option (returns Option), .unwrap_or → recv Option
    let unwrap_or = mc
        .iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "unwrap_or"))
        .expect("unwrap_or edge");
    assert_eq!(unwrap_or.receiver_type.as_deref(), Some("Option"));
}

#[test]
fn result_ok_returns_option() {
    let parsed = parse("fn caller(r: Result<u8, ()>) { r.ok().unwrap(); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let mc = method_calls(&edges);
    let unwrap = mc
        .iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "unwrap"))
        .expect("unwrap edge");
    // r.ok() returns Option; .unwrap() receiver = Option.
    assert_eq!(unwrap.receiver_type.as_deref(), Some("Option"));
}

#[test]
fn path_parent_returns_option() {
    let parsed = parse("fn caller(p: &Path) { p.parent().unwrap(); } struct Path;");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let mc = method_calls(&edges);
    let unwrap = mc
        .iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "unwrap"))
        .expect("unwrap edge");
    assert_eq!(unwrap.receiver_type.as_deref(), Some("Option"));
}

#[test]
fn chain_break_on_unknown_step_falls_through() {
    // .custom_method() isn't in the registry. The chain breaks there
    // and downstream receivers go back to None.
    let parsed = parse("fn caller() { let v = Vec::new(); v.iter().custom_method().another(); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let mc = method_calls(&edges);
    // v.iter() → Vec, .custom_method() → Iterator (recv known), .another() → ??? (recv unknown)
    let another = mc
        .iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "another"))
        .expect("another edge");
    assert_eq!(another.receiver_type, None);
}

// --- Bug E-3 ext P-E3.2: closure-arg type inference ---

#[test]
fn option_map_closure_arg_typed_from_receiver_generic() {
    // `opt: Option<Foo>` ; inside `.map(|x| x.helper())`, the closure
    // sees `x` bound to `Foo` via the builtin registry's `closure_arg_type
    // = "T"` annotation on `Option::map`. Calls inside the closure body
    // emit edges with `receiver_type = Some("Foo")`.
    let parsed = parse(
        "struct Foo; impl Foo { fn helper(&self) {} } \
         fn caller(opt: Option<Foo>) { opt.map(|x| x.helper()); }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let helper = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "helper"))
        .expect("helper edge");
    assert_eq!(helper.receiver_type.as_deref(), Some("Foo"));
}

#[test]
fn vec_retain_closure_arg_typed_with_ref_stripped() {
    // `Vec::retain(|x| ...)` — closure_arg_type = "T" (refs stripped),
    // so `x.field()` inside binds `receiver_type = Foo`.
    let parsed = parse(
        "struct Foo; impl Foo { fn check(&self) -> bool { true } } \
         fn caller(xs: Vec<Foo>) { xs.retain(|x| x.check()); }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let check = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "check"))
        .expect("check edge");
    assert_eq!(check.receiver_type.as_deref(), Some("Foo"));
}

#[test]
fn iterator_chain_propagates_generic_via_filter_then_find() {
    // `Vec<Foo>::iter()` returns `Iterator<T>` substituted to
    // `Iterator<Foo>` ; `.filter(|x| ...)` preserves T (still
    // `Iterator<Foo>`) ; `.find(|y| y.matches())` binds `y: Foo`.
    let parsed = parse(
        "struct Foo; impl Foo { fn ok(&self) -> bool { true } fn matches(&self) -> bool { true } } \
         fn caller(xs: Vec<Foo>) { xs.iter().filter(|x| x.ok()).find(|y| y.matches()); }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let mc = method_calls(&edges);
    let ok = mc
        .iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "ok"))
        .expect("ok edge");
    let matches_edge = mc
        .iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "matches"))
        .expect("matches edge");
    assert_eq!(ok.receiver_type.as_deref(), Some("Foo"));
    assert_eq!(matches_edge.receiver_type.as_deref(), Some("Foo"));
}

#[test]
fn nested_closure_inner_shadows_outer_binding() {
    // `outer: Vec<Vec<Foo>>` ; `.iter().map(|inner| inner.iter().for_each(|x| x.run()))`
    // — outer's `inner: Vec<Foo>`, inner's `x: Foo`. Closure scope
    // push/pop stacks so the inner `x.run()` sees `Foo`.
    let parsed = parse(
        "struct Foo; impl Foo { fn run(&self) {} } \
         fn caller(outer: Vec<Vec<Foo>>) { \
             outer.iter().for_each(|inner| inner.iter().for_each(|x| x.run())); \
         }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let run = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "run"))
        .expect("run edge");
    assert_eq!(run.receiver_type.as_deref(), Some("Foo"));
}

#[test]
fn struct_field_parametric_unlocks_closure_through_field_chain() {
    // Bug E-3 ext P-E3.2.1: `self.items.iter().map(|x| x.foo())` —
    // struct_field_table now preserves `Vec<Foo>` parametrically so the
    // closure binding sees `x: Foo`, enabling `x.foo()` to emit
    // receiver_type = Foo (vs receiver_type = None pre-P-E3.2.1).
    let parsed = parse(
        "struct Foo; impl Foo { fn foo(&self) {} } \
         struct Owner { items: Vec<Foo> } \
         impl Owner { fn process(&self) { self.items.iter().map(|x| x.foo()); } }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let foo = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "foo"))
        .expect("foo edge");
    assert_eq!(foo.receiver_type.as_deref(), Some("Foo"));
}

// --- Bug E-3.4.1: if-let / while-let / match-arm pattern binding ---

#[test]
fn if_let_some_binds_inner_for_then_branch() {
    let parsed = parse(
        "struct Foo; impl Foo { fn ping(&self) {} } \
         fn caller(opt: Option<Foo>) { if let Some(x) = opt { x.ping(); } }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let ping = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "ping"))
        .expect("ping edge");
    assert_eq!(ping.receiver_type.as_deref(), Some("Foo"));
}

#[test]
fn if_let_ok_binds_inner_t() {
    let parsed = parse(
        "struct Foo; impl Foo { fn ping(&self) {} } \
         fn caller(r: Result<Foo, ()>) { if let Ok(v) = r { v.ping(); } }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let ping = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "ping"))
        .expect("ping edge");
    assert_eq!(ping.receiver_type.as_deref(), Some("Foo"));
}

#[test]
fn if_let_err_binds_inner_e() {
    let parsed = parse(
        "struct ApiErr; impl ApiErr { fn log(&self) {} } \
         fn caller(r: Result<(), ApiErr>) { if let Err(e) = r { e.log(); } }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let log = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "log"))
        .expect("log edge");
    assert_eq!(log.receiver_type.as_deref(), Some("ApiErr"));
}

#[test]
fn while_let_some_binds_inner_for_body() {
    let parsed = parse(
        "struct Foo; impl Foo { fn ping(&self) {} } \
         fn caller(mut opt: Option<Foo>) { while let Some(x) = opt.take() { x.ping(); } }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let ping = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "ping"))
        .expect("ping edge");
    assert_eq!(ping.receiver_type.as_deref(), Some("Foo"));
}

#[test]
fn match_arm_some_binds_per_arm() {
    let parsed = parse(
        "struct Foo; impl Foo { fn ping(&self) {} } \
         fn caller(opt: Option<Foo>) { match opt { Some(x) => x.ping(), None => () } }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let ping = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "ping"))
        .expect("ping edge");
    assert_eq!(ping.receiver_type.as_deref(), Some("Foo"));
}

#[test]
fn match_arm_ok_err_bind_independently() {
    let parsed = parse(
        "struct Foo; impl Foo { fn ping(&self) {} } \
         struct ApiErr; impl ApiErr { fn log(&self) {} } \
         fn caller(r: Result<Foo, ApiErr>) { \
             match r { Ok(v) => v.ping(), Err(e) => e.log() } \
         }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let ping = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "ping"))
        .expect("ping edge");
    let log = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "log"))
        .expect("log edge");
    assert_eq!(ping.receiver_type.as_deref(), Some("Foo"));
    assert_eq!(log.receiver_type.as_deref(), Some("ApiErr"));
}

// --- Bug E-3.4: for-loop pattern binding ---

#[test]
fn for_loop_over_vec_binds_pat_to_inner_t() {
    // `for x in xs` where `xs: Vec<Foo>` should bind `x: Foo` for the
    // duration of the body, so `x.ping()` carries receiver_type = Foo.
    let parsed = parse(
        "struct Foo; impl Foo { fn ping(&self) {} } \
         fn caller(xs: Vec<Foo>) { for x in xs { x.ping(); } }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let ping = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "ping"))
        .expect("ping edge");
    assert_eq!(ping.receiver_type.as_deref(), Some("Foo"));
}

#[test]
fn for_loop_over_ref_vec_strips_ref_in_binding() {
    let parsed = parse(
        "struct Foo; impl Foo { fn ping(&self) {} } \
         fn caller(xs: Vec<Foo>) { for x in &xs { x.ping(); } }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let ping = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "ping"))
        .expect("ping edge");
    assert_eq!(ping.receiver_type.as_deref(), Some("Foo"));
}

#[test]
fn for_loop_over_iterator_binds_pat() {
    // `for x in xs.iter()` where xs: Vec<Foo> → xs.iter() returns
    // Iterator<Foo> → x binds to Foo.
    let parsed = parse(
        "struct Foo; impl Foo { fn ping(&self) {} } \
         fn caller(xs: Vec<Foo>) { for x in xs.iter() { x.ping(); } }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let ping = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "ping"))
        .expect("ping edge");
    assert_eq!(ping.receiver_type.as_deref(), Some("Foo"));
}

#[test]
fn for_loop_over_struct_field_chain() {
    // `for sym in &plan.inserts` — the audit-identified pattern from
    // batch.rs. `plan.inserts: Vec<RawSymbol>` → `sym: RawSymbol` →
    // `sym.fqdn.as_str()` resolves through struct_fields + builtins.
    let parsed = parse(
        "struct RawSymbol { fqdn: String } \
         struct Plan { inserts: Vec<RawSymbol> } \
         fn touched_fqdns(plan: &Plan) { for sym in &plan.inserts { sym.fqdn.as_str(); } }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let as_str = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "as_str"))
        .expect("as_str edge");
    assert_eq!(as_str.receiver_type.as_deref(), Some("String"));
}

#[test]
fn for_loop_pat_does_not_leak_outside_body() {
    // Outer `x: Outer`; for-loop introduces inner `x: Inner` shadowing.
    // After the loop, `x.outer_method()` should re-see Outer.
    let parsed = parse(
        "struct Outer; impl Outer { fn outer_method(&self) {} } \
         struct Inner; impl Inner { fn inner_method(&self) {} } \
         fn caller(x: Outer, xs: Vec<Inner>) { \
             for x in xs { x.inner_method(); } \
             x.outer_method(); \
         }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let inner = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "inner_method"))
        .expect("inner_method edge");
    let outer = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "outer_method"))
        .expect("outer_method edge");
    assert_eq!(inner.receiver_type.as_deref(), Some("Inner"));
    assert_eq!(outer.receiver_type.as_deref(), Some("Outer"));
}

// --- Bug E-3.3: parametric unwrap (Option<T>::unwrap → T, etc.) ---

#[test]
fn option_unwrap_returns_inner_t_for_chained_method_call() {
    // `Option<Foo>::unwrap()` should propagate as `Foo` so the chained
    // `.ping()` carries `receiver_type = Foo`.
    let parsed = parse(
        "struct Foo; impl Foo { fn ping(&self) {} } \
         fn caller(opt: Option<Foo>) { opt.unwrap().ping(); }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let ping = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "ping"))
        .expect("ping edge");
    assert_eq!(ping.receiver_type.as_deref(), Some("Foo"));
}

#[test]
fn result_unwrap_returns_inner_t() {
    let parsed = parse(
        "struct Foo; impl Foo { fn ping(&self) {} } \
         fn caller(r: Result<Foo, ()>) { r.unwrap().ping(); }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let ping = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "ping"))
        .expect("ping edge");
    assert_eq!(ping.receiver_type.as_deref(), Some("Foo"));
}

#[test]
fn result_unwrap_err_returns_inner_e() {
    let parsed = parse(
        "struct ApiErr; impl ApiErr { fn log(&self) {} } \
         fn caller(r: Result<(), ApiErr>) { r.unwrap_err().log(); }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let log = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "log"))
        .expect("log edge");
    assert_eq!(log.receiver_type.as_deref(), Some("ApiErr"));
}

#[test]
fn vec_get_returns_option_t_then_unwrap_returns_t() {
    // Full chain : `xs.get(0)` → `Option<T>`, `.unwrap()` → `T`,
    // `.ping()` → receiver_type = `Foo`.
    let parsed = parse(
        "struct Foo; impl Foo { fn ping(&self) {} } \
         fn caller(xs: Vec<Foo>) { xs.get(0).unwrap().ping(); }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let ping = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "ping"))
        .expect("ping edge");
    assert_eq!(ping.receiver_type.as_deref(), Some("Foo"));
}

#[test]
fn iterator_next_unwrap_chain() {
    // `vec.iter().next().unwrap().ping()` — chain through
    // `Iterator<T>::next` → `Option<T>` → `T`.
    let parsed = parse(
        "struct Foo; impl Foo { fn ping(&self) {} } \
         fn caller(xs: Vec<Foo>) { xs.iter().next().unwrap().ping(); }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let ping = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "ping"))
        .expect("ping edge");
    assert_eq!(ping.receiver_type.as_deref(), Some("Foo"));
}

#[test]
fn hashmap_get_returns_option_v() {
    let parsed = parse(
        "struct Foo; impl Foo { fn ping(&self) {} } \
         fn caller(m: std::collections::HashMap<String, Foo>) { m.get(\"k\").unwrap().ping(); }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let ping = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "ping"))
        .expect("ping edge");
    assert_eq!(ping.receiver_type.as_deref(), Some("Foo"));
}

#[test]
fn workspace_call_binding_unlocks_closure_chain() {
    // Bug E-3 ext P-E3.2.3: `let extracted = walk(...)` now binds
    // `extracted` to walk's recorded return type via the workspace
    // return-type table fallback. The chain
    // `extracted.symbols.iter().map(|s| s.name.as_str())` then
    // resolves end-to-end.
    let parsed = parse(
        "struct Sym; impl Sym { fn ping(&self) {} } \
         struct Out { items: Vec<Sym> } \
         fn walk() -> Out { Out { items: Vec::new() } } \
         fn caller() { let extracted = walk(); extracted.items.iter().map(|s| s.ping()); }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let ping = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "ping"))
        .expect("ping edge");
    assert_eq!(ping.receiver_type.as_deref(), Some("Sym"));
}

#[test]
fn workspace_method_call_binding_unlocks_chain() {
    // Same as above but the init is a method call instead of a free fn.
    let parsed = parse(
        "struct Sym; impl Sym { fn ping(&self) {} } \
         struct Out { items: Vec<Sym> } \
         struct Repo; impl Repo { fn snapshot(&self) -> Out { Out { items: Vec::new() } } } \
         fn caller(r: Repo) { let extracted = r.snapshot(); extracted.items.iter().map(|s| s.ping()); }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let ping = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "ping"))
        .expect("ping edge");
    assert_eq!(ping.receiver_type.as_deref(), Some("Sym"));
}

#[test]
fn struct_field_chain_via_nominal_owner_resolves_through_side_index() {
    // Bug E-3 ext P-E3.2.2: when the owner is a fn-param binding
    // (`fn process(owner: Owner)`), `owner.items` keys struct_fields
    // with the *nominal* short name "Owner" — the side-index now
    // resolves that to the recorded FQDN `c::Owner`, so closure-arg
    // propagation reaches `x.foo()` with `receiver_type = Foo`.
    let parsed = parse(
        "struct Foo; impl Foo { fn foo(&self) {} } \
         struct Owner { items: Vec<Foo> } \
         fn process(owner: Owner) { owner.items.iter().map(|x| x.foo()); }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let foo = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "foo"))
        .expect("foo edge");
    assert_eq!(foo.receiver_type.as_deref(), Some("Foo"));
}

#[test]
fn await_passes_through_for_chained_method_calls() {
    // P-E3.2.1: `.await` collapses to its base type so async chains
    // reach the subsequent method-call resolution path. Semantically
    // wrong (Future<Output=Vec<T>>.await yields Vec<T>, not the
    // Future), but the workspace return-type table for `async fn`
    // already records the user-typed return — so propagation matches
    // reality often enough.
    let parsed = parse(
        "struct Foo; impl Foo { fn run(&self) {} } \
         fn caller(opt: Option<Foo>) { let _ = async { opt.map(|x| x.run()).unwrap() }; }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let run = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "run"))
        .expect("run edge");
    assert_eq!(run.receiver_type.as_deref(), Some("Foo"));
}

#[test]
fn try_operator_smart_unwraps_result_for_chained_method_calls() {
    // P-E3.2.1: `expr?` smart-unwraps the Ok branch of `Result<T, _>`
    // (and `Option<T>::Some`) so post-`?` chains reach the inner type's
    // method-call resolution. `xs?.iter().map(|x| x.run())` for
    // `xs: Result<Vec<Foo>, ()>` propagates Vec<Foo> through the `?`,
    // then `.iter()` → Iterator<Foo>, then `.map(|x| ...)` binds
    // `x: Foo` via the builtin registry closure_arg_type.
    let parsed = parse(
        "struct Foo; impl Foo { fn run(&self) {} } \
         fn caller(xs: Result<Vec<Foo>, ()>) -> Result<(), ()> { \
             xs?.iter().map(|x| x.run()); Ok(()) \
         }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let run = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "run"))
        .expect("run edge");
    assert_eq!(run.receiver_type.as_deref(), Some("Foo"));
}

#[test]
fn closure_arg_without_generic_args_does_not_pollute_receiver_type() {
    // `let v: Vec<_> = Vec::new();` substitutes `T = "_"` into the
    // closure-arg template. Bug E-3.3 suppresses that info-less binding
    // so `x.foo()` carries receiver_type = None instead of polluting the
    // edge column with `"_"`.
    let parsed = parse("fn caller() { let v: Vec<_> = Vec::new(); v.retain(|x| x.foo()); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let foo = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "foo"))
        .expect("foo edge");
    assert_eq!(foo.receiver_type, None);
}

#[test]
fn field_as_call_on_workspace_struct_is_suppressed() {
    // Bug field-as-CALL: `s.handler()` where `handler` is a nominal-
    // typed field of workspace struct `S` (the typical real-world case:
    // a closure / Box<dyn Fn> / Arc<F> stored as a field). syn parses
    // this as ExprMethodCall, but semantically the call goes through
    // the field value — not a method on `S`. The CALLS edge with
    // `name = "handler"` should be suppressed.
    //
    // V2 (`fn()` extension) extends this guard to non-nominal field
    // types via `has_field` — see
    // `field_as_call_on_bare_fn_field_is_suppressed_v2`.
    let parsed = parse(
        "struct H; struct S { handler: H } fn caller(s: S) { s.handler(); }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let handler_calls: Vec<_> = method_calls(&edges)
        .into_iter()
        .filter(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "handler"))
        .collect();
    assert!(
        handler_calls.is_empty(),
        "field-as-CALL must be suppressed, got: {handler_calls:#?}"
    );
}

#[test]
fn method_call_on_workspace_struct_without_matching_field_is_preserved() {
    // Sanity: a real method call on a workspace struct (no field with
    // the same name) still produces a CALLS edge — the suppression
    // only kicks in when struct_fields.lookup() hits.
    let parsed = parse(
        "struct S { value: i32 } impl S { fn compute(&self) {} } \
         fn caller(s: S) { s.compute(); }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let compute = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "compute"))
        .expect("compute method CALLS edge expected");
    assert_eq!(compute.from_fqdn, "c::caller");
}

#[test]
fn stdlib_method_call_is_preserved_even_with_field_like_name() {
    // Stdlib types are not in struct_fields (workspace-only table), so
    // a call like `path.exists()` where `exists` happens to be a method
    // (not a field) must still emit a CALLS edge. Same for `.iter()`
    // and other common idents that could collide with hypothetical
    // workspace field names but aren't.
    let parsed =
        parse("fn caller(p: std::path::PathBuf) { p.exists(); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let exists = method_calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "exists"))
        .expect("stdlib .exists() CALLS edge must survive");
    assert_eq!(exists.from_fqdn, "c::caller");
}

#[test]
fn field_as_call_on_bare_fn_field_is_suppressed_v2() {
    // V2: `Type::BareFn` (`fn()`) fields are tracked via
    // `StructFieldTable::record_presence` even though `parametric_type`
    // skips them for the typed table. The guard uses `has_field`
    // (presence-only) so `s.bare()` where `bare: fn()` no longer
    // emits a phantom CALLS edge with `name = "bare"`.
    let parsed = parse(
        "struct S { bare: fn() } fn caller(s: S) { s.bare(); }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let bare_calls: Vec<_> = method_calls(&edges)
        .into_iter()
        .filter(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "bare"))
        .collect();
    assert!(
        bare_calls.is_empty(),
        "V2 must suppress bare-fn field-as-CALL, got: {bare_calls:#?}"
    );
}

#[test]
fn field_as_call_on_closure_field_is_suppressed_v2() {
    // V2: closure-typed fields (`Box<dyn Fn(...)>` etc.) are also
    // tracked by `record_presence`. The Box wrapper IS nominal so
    // `record` would catch this too, but the test makes the
    // closure-call intent explicit.
    let parsed = parse(
        "struct S { cb: Box<dyn Fn(u32) -> bool> } fn caller(s: &S) { s.cb(0); }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let cb_calls: Vec<_> = method_calls(&edges)
        .into_iter()
        .filter(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "cb"))
        .collect();
    assert!(
        cb_calls.is_empty(),
        "V2 must suppress Box<dyn Fn> field-as-CALL, got: {cb_calls:#?}"
    );
}

#[test]
fn field_as_call_via_ref_receiver_is_suppressed() {
    // The receiver type comes through as `&S` (or `&mut S`) via the
    // local env. The guard must strip refs before the field lookup
    // so `&S` matches struct `S` in the workspace table.
    let parsed = parse(
        "struct H; struct S { cb: H } fn caller(s: &S) { s.cb(); }",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let cb_calls: Vec<_> = method_calls(&edges)
        .into_iter()
        .filter(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "cb"))
        .collect();
    assert!(
        cb_calls.is_empty(),
        "field-as-CALL through &S must be suppressed, got: {cb_calls:#?}"
    );
}

// --- GRTR Phase 4: workspace-global return-type lookup chain ---

#[test]
fn cross_file_workspace_call_resolves_receiver_type_via_global_registry() {
    // Simulates Pass 0 having already walked `other_crate` and recorded
    // `other_crate::get_user -> User`. The walked file lives in `c` and
    // does `let u = other_crate::get_user(); u.name();`. Without GRTR
    // the `u.name` edge carries `receiver_type = None`; with it the
    // edge resolves to `receiver_type = "User"`.
    use std::sync::Arc;
    use syn::parse_quote;

    use super::super::global_return_type_registry::GlobalReturnTypeRegistry;
    use super::super::walk::walk_with_lookup;

    let mut registry = GlobalReturnTypeRegistry::default();
    let ret_ty: syn::Type = parse_quote!(User);
    registry.record("other_crate::get_user", &ret_ty);
    let registry = Arc::new(registry);

    let parsed = parse(
        "fn caller() { let u = other_crate::get_user(); u.name(); }",
    );
    let (_, edges, _, _, _) = walk_with_lookup(
        &parsed,
        "c",
        "src/lib.rs",
        "c",
        Some(Arc::clone(&registry)),
    );
    let name_edge = calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "name"))
        .expect("name() CALLS edge");
    assert_eq!(
        name_edge.receiver_type.as_deref(),
        Some("User"),
        "GRTR must propagate `User` to receiver_type for `u.name()` via Pass 0 seeded `other_crate::get_user` return"
    );
}

#[test]
fn cross_file_lookup_misses_when_registry_is_none() {
    // Sanity: when GRTR isn't seeded (None) the per-file table alone
    // can't resolve `other_crate::get_user` because the fn is not
    // declared in this file. The `u.name()` edge stays
    // receiver_type=None — matching the pre-GRTR behaviour.
    use super::super::walk::walk_with_lookup;
    let parsed = parse(
        "fn caller() { let u = other_crate::get_user(); u.name(); }",
    );
    let (_, edges, _, _, _) = walk_with_lookup(&parsed, "c", "src/lib.rs", "c", None);
    let name_edge = calls(&edges)
        .into_iter()
        .find(|e| matches!(&e.to, ResolvedOrUnresolved::Unresolved { name } if name == "name"))
        .expect("name() CALLS edge");
    assert_eq!(
        name_edge.receiver_type, None,
        "without GRTR the cross-file fn return type stays unresolved"
    );
}

#![allow(clippy::match_wildcard_for_single_variants)]

use super::*;
use std::path::PathBuf;
use swc_core::common::comments::SingleThreadedComments;
use swc_core::common::{FileName, SourceMap, sync::Lrc};
use swc_core::ecma::ast::{EsVersion, Module};
use swc_core::ecma::parser::{Parser, StringInput, Syntax, TsSyntax, lexer::Lexer};

fn parse_ts(source: &str) -> (Lrc<SourceMap>, Module, SingleThreadedComments) {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(
        Lrc::new(FileName::Custom("test.ts".into())),
        source.to_string(),
    );
    let comments = SingleThreadedComments::default();
    let lexer = Lexer::new(
        Syntax::Typescript(TsSyntax {
            tsx: false,
            decorators: false,
            dts: false,
            no_early_errors: true,
            disallow_ambiguous_jsx_like: false,
        }),
        EsVersion::EsNext,
        StringInput::from(&*fm),
        Some(&comments),
    );
    let mut parser = Parser::new_from(lexer);
    let module = parser.parse_module().expect("parse ok");
    (cm, module, comments)
}

fn parse_tsx(source: &str) -> (Lrc<SourceMap>, Module, SingleThreadedComments) {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(
        Lrc::new(FileName::Custom("test.tsx".into())),
        source.to_string(),
    );
    let comments = SingleThreadedComments::default();
    let lexer = Lexer::new(
        Syntax::Typescript(TsSyntax {
            tsx: true,
            decorators: false,
            dts: false,
            no_early_errors: true,
            disallow_ambiguous_jsx_like: false,
        }),
        EsVersion::EsNext,
        StringInput::from(&*fm),
        Some(&comments),
    );
    let mut parser = Parser::new_from(lexer);
    let module = parser.parse_module().expect("parse ok");
    (cm, module, comments)
}

fn run(source: &str) -> (Vec<standardoc_ir::RawSymbol>, Vec<RawEdge>) {
    let (cm, module, comments) = parse_ts(source);
    let (symbols, edges, _, _) = super::super::walk::walk(
        &module,
        "@app",
        "src/index.ts",
        "src",
        cm,
        &PathBuf::from("/tmp/pkg/src/index.ts"),
        &PathBuf::from("/tmp/pkg"),
        None,
        &comments,
    );
    (symbols, edges)
}

fn run_tsx(source: &str) -> (Vec<standardoc_ir::RawSymbol>, Vec<RawEdge>) {
    let (cm, module, comments) = parse_tsx(source);
    let (symbols, edges, _, _) = super::super::walk::walk(
        &module,
        "@app",
        "src/App.tsx",
        "src/App",
        cm,
        &PathBuf::from("/tmp/pkg/src/App.tsx"),
        &PathBuf::from("/tmp/pkg"),
        None,
        &comments,
    );
    (symbols, edges)
}

fn calls(edges: &[RawEdge]) -> Vec<&RawEdge> {
    edges.iter().filter(|e| e.kind == EdgeKind::Calls).collect()
}

fn references(edges: &[RawEdge]) -> Vec<&RawEdge> {
    edges
        .iter()
        .filter(|e| e.kind == EdgeKind::References)
        .collect()
}

#[test]
fn simple_function_call_is_resolved_against_defined_fqdn() {
    let (_, edges) = run("function bar() {} function caller() { bar(); }");
    let cs = calls(&edges);
    assert_eq!(cs.len(), 1);
    assert_eq!(cs[0].from_fqdn, "src::caller");
    match &cs[0].to {
        ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "src::bar"),
        other => panic!("expected resolved, got {other:?}"),
    }
}

#[test]
fn unknown_call_is_unresolved_module_local() {
    let (_, edges) = run("function caller() { unknown(); }");
    let cs = calls(&edges);
    assert_eq!(cs.len(), 1);
    match &cs[0].to {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "src::unknown"),
        other => panic!("expected unresolved, got {other:?}"),
    }
}

#[test]
fn member_call_is_unresolved_with_method_ident() {
    let (_, edges) = run("function caller() { obj.run(); }");
    let cs = calls(&edges);
    assert_eq!(cs.len(), 1);
    match &cs[0].to {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "run"),
        other => panic!("expected unresolved method ident, got {other:?}"),
    }
}

#[test]
fn nested_calls_in_arguments_are_captured() {
    let (_, edges) = run("function a() {} function b() {} function caller() { a(); b(); }");
    let cs = calls(&edges);
    assert_eq!(cs.len(), 2);
}

#[test]
fn class_method_body_calls_attributed_to_method_fqdn() {
    let (_, edges) = run("function helper() {} class Foo { run(): void { helper(); } }");
    let cs = calls(&edges);
    assert_eq!(cs.len(), 1);
    assert_eq!(cs[0].from_fqdn, "src::Foo::run");
}

#[test]
fn new_expr_emits_calls_edge() {
    let (_, edges) = run("class Foo {} function caller() { new Foo(); }");
    let cs = calls(&edges);
    assert_eq!(cs.len(), 1);
    match &cs[0].to {
        ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "src::Foo"),
        other => panic!("expected resolved Foo, got {other:?}"),
    }
}

#[test]
fn arrow_const_body_calls_attributed_to_var_fqdn() {
    let (_, edges) = run("function helper() {} const run = () => { helper(); };");
    let cs = calls(&edges);
    assert_eq!(cs.len(), 1);
    assert_eq!(cs[0].from_fqdn, "src::run");
}

#[test]
fn optional_chain_call_is_unresolved_method() {
    let (_, edges) = run("function caller() { obj?.run(); }");
    let cs = calls(&edges);
    assert_eq!(cs.len(), 1);
    match &cs[0].to {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "run"),
        other => panic!("expected unresolved method ident, got {other:?}"),
    }
}

#[test]
fn alias_resolves_to_canonical_via_import_table() {
    let (_, edges) = run("import { foo } from './helper'; function caller() { foo(); }");
    let cs = calls(&edges);
    assert_eq!(cs.len(), 1);
    match &cs[0].to {
        ResolvedOrUnresolved::Unresolved { name } => {
            assert_eq!(name, "@app::src::helper::foo");
        }
        other => panic!("expected unresolved canonical via alias, got {other:?}"),
    }
}

// --- JSX template-extraction tests -----------------------------------

fn refs_with_attribute<'a>(edges: &'a [RawEdge], attr: &str) -> Vec<&'a RawEdge> {
    edges
        .iter()
        .filter(|e| e.kind == EdgeKind::References && e.attributes.iter().any(|a| a == attr))
        .collect()
}

fn ref_target_name(edge: &RawEdge) -> &str {
    match &edge.to {
        ResolvedOrUnresolved::Resolved { fqdn } => fqdn.as_str(),
        ResolvedOrUnresolved::Unresolved { name }
        | ResolvedOrUnresolved::UnresolvedBridge { name, .. } => name.as_str(),
    }
}

#[test]
fn jsx_uppercase_tag_emits_component_ref() {
    let (_, edges) = run_tsx("function App() { return <Header />; }");
    let comp = refs_with_attribute(&edges, "template-component-ref");
    assert_eq!(comp.len(), 1);
    assert!(ref_target_name(comp[0]).ends_with("Header"));
}

#[test]
fn jsx_lowercase_tag_does_not_emit_component_ref() {
    let (_, edges) = run_tsx("function App() { return <div />; }");
    let comp = refs_with_attribute(&edges, "template-component-ref");
    assert!(comp.is_empty());
}

#[test]
fn jsx_attribute_expression_emits_template_bind() {
    let (_, edges) = run_tsx("function App() { return <input value={text} />; }");
    let bind = refs_with_attribute(&edges, "template-bind");
    let names: Vec<&str> = bind.iter().map(|e| ref_target_name(e)).collect();
    assert!(names.iter().any(|n| n.ends_with("text")));
}

#[test]
fn jsx_child_expression_emits_template_interpolation() {
    let (_, edges) = run_tsx("function App() { return <p>{message}</p>; }");
    let interp = refs_with_attribute(&edges, "template-interpolation");
    let names: Vec<&str> = interp.iter().map(|e| ref_target_name(e)).collect();
    assert!(names.iter().any(|n| n.ends_with("message")));
}

#[test]
fn jsx_spread_attribute_emits_template_bind() {
    let (_, edges) = run_tsx("function App(props: any) { return <input {...props} />; }");
    let bind = refs_with_attribute(&edges, "template-bind");
    let names: Vec<&str> = bind.iter().map(|e| ref_target_name(e)).collect();
    assert!(names.iter().any(|n| n.ends_with("props")));
}

#[test]
fn jsx_event_handler_call_inside_attribute_also_emits_calls_edge() {
    let (_, edges) =
        run_tsx("function App() { return <button onClick={() => handle(payload)} />; }");
    // Both the handle CALL and the handle/payload REFERENCES (Bind).
    let cs = calls(&edges);
    let bind = refs_with_attribute(&edges, "template-bind");
    let call_names: Vec<&str> = cs.iter().map(|e| ref_target_name(e)).collect();
    let bind_names: Vec<&str> = bind.iter().map(|e| ref_target_name(e)).collect();
    assert!(call_names.iter().any(|n| n.ends_with("handle")));
    assert!(bind_names.iter().any(|n| n.ends_with("handle")));
    assert!(bind_names.iter().any(|n| n.ends_with("payload")));
}

#[test]
fn jsx_member_access_in_child_emits_root_only() {
    let (_, edges) = run_tsx("function App() { return <p>{user.name}</p>; }");
    let interp = refs_with_attribute(&edges, "template-interpolation");
    let names: Vec<&str> = interp.iter().map(|e| ref_target_name(e)).collect();
    assert!(names.iter().any(|n| n.ends_with("user")));
    assert!(!names.iter().any(|n| n.ends_with("name")));
}

#[test]
fn jsx_static_string_attribute_does_not_emit_ref() {
    let (_, edges) = run_tsx(r#"function App() { return <div className="static" />; }"#);
    let bind = refs_with_attribute(&edges, "template-bind");
    assert!(bind.is_empty());
}

#[test]
fn jsx_nested_components_both_emit_component_ref() {
    let (_, edges) = run_tsx("function App() { return <Layout><Header /></Layout>; }");
    let comp = refs_with_attribute(&edges, "template-component-ref");
    let names: Vec<&str> = comp.iter().map(|e| ref_target_name(e)).collect();
    assert!(names.iter().any(|n| n.ends_with("Layout")));
    assert!(names.iter().any(|n| n.ends_with("Header")));
}

#[test]
fn jsx_globals_filtered_in_interpolation() {
    let (_, edges) = run_tsx("function App() { return <p>{Math.max(a, b)}</p>; }");
    let interp = refs_with_attribute(&edges, "template-interpolation");
    let names: Vec<&str> = interp.iter().map(|e| ref_target_name(e)).collect();
    assert!(!names.iter().any(|n| n.ends_with("Math")));
    assert!(names.iter().any(|n| n.ends_with('a')));
    assert!(names.iter().any(|n| n.ends_with('b')));
}

#[test]
fn no_jsx_means_no_template_refs() {
    // Plain TS without JSX should produce zero REFERENCES edges from
    // the visitor (existing call-emission behavior preserved).
    let (_, edges) = run("function caller() { unknown(); }");
    assert!(references(&edges).is_empty());
}

// --- Bug B Stage 2 tests: scope tracking + alias propagation ---

fn refs_with_all_attrs<'a>(edges: &'a [RawEdge], attrs: &[&str]) -> Vec<&'a RawEdge> {
    edges
        .iter()
        .filter(|e| {
            e.kind == EdgeKind::References
                && attrs.iter().all(|a| e.attributes.iter().any(|x| x == a))
        })
        .collect()
}

fn calls_with_attr<'a>(edges: &'a [RawEdge], attr: &str) -> Vec<&'a RawEdge> {
    edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Calls && e.attributes.iter().any(|a| a == attr))
        .collect()
}

#[test]
fn stage2_const_alias_propagates_to_aliased_value_read() {
    let (_, edges) = run("function FOO() {} function takesArg(x) {} \
             function f() { const x = FOO; takesArg(x); }");
    let refs = refs_with_all_attrs(&edges, &["value-read", "via-alias"]);
    let targets: Vec<&str> = refs
        .iter()
        .filter_map(|e| match &e.to {
            ResolvedOrUnresolved::Resolved { fqdn } => Some(fqdn.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        targets.contains(&"src::FOO"),
        "expected via-alias value-read to src::FOO, got {targets:?}",
    );
}

#[test]
fn stage2_let_alias_tagged_via_alias_mutable() {
    let (_, edges) = run("function FOO() {} function takesArg(x) {} \
             function f() { let x = FOO; takesArg(x); }");
    let refs = refs_with_all_attrs(&edges, &["value-read", "via-alias-mutable"]);
    let targets: Vec<&str> = refs
        .iter()
        .filter_map(|e| match &e.to {
            ResolvedOrUnresolved::Resolved { fqdn } => Some(fqdn.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        targets.contains(&"src::FOO"),
        "expected via-alias-mutable value-read to src::FOO, got {targets:?}",
    );
}

#[test]
fn stage2_aliased_call_carries_via_alias_attribute() {
    // Imported FOO → alias `fn` → `fn()` emits Calls{Unresolved{canonical}}
    // tagged `via-alias`.
    let (_, edges) = run("import { FOO } from './m'; function f() { const fn = FOO; fn(); }");
    let cs = calls_with_attr(&edges, "via-alias");
    let names: Vec<&str> = cs
        .iter()
        .filter_map(|e| match &e.to {
            ResolvedOrUnresolved::Unresolved { name } => Some(name.as_str()),
            ResolvedOrUnresolved::Resolved { fqdn } => Some(fqdn.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        names.iter().any(|n| n.ends_with("::m::FOO")),
        "expected via-alias call pointing at imported FOO, got {names:?}",
    );
}

#[test]
fn stage2_local_function_decl_shadows_import_and_suppresses_call() {
    // Inner `function FOO() {}` shadows the import; inner FOO()
    // is a local call → no Calls edge to the imported symbol.
    let (_, edges) = run("import { FOO } from './m'; function f() { function FOO() {} FOO(); }");
    let names: Vec<&str> = calls(&edges)
        .iter()
        .filter_map(|e| match &e.to {
            ResolvedOrUnresolved::Unresolved { name } => Some(name.as_str()),
            ResolvedOrUnresolved::Resolved { fqdn } => Some(fqdn.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        !names.iter().any(|n| n.ends_with("::m::FOO")),
        "expected no call edge to imported FOO, got {names:?}",
    );
}

#[test]
fn stage2_function_param_read_emits_no_reference() {
    let (_, edges) = run("function consume(a) {} function f(arg) { consume(arg); }");
    // The arg → consume call carries arg as an argument. Stage 1
    // would emit a value-read on `arg` (Unresolved → skip filtered).
    // Stage 2's scope binding skips emission too — but with the
    // explicit Local sentinel, no spurious value-read appears.
    let refs = refs_with_all_attrs(&edges, &["value-read"]);
    let on_arg: Vec<&&RawEdge> = refs
        .iter()
        .filter(|e| match &e.to {
            ResolvedOrUnresolved::Resolved { fqdn } => fqdn.ends_with("::arg"),
            ResolvedOrUnresolved::Unresolved { name }
            | ResolvedOrUnresolved::UnresolvedBridge { name, .. } => name.ends_with("::arg"),
        })
        .collect();
    assert!(
        on_arg.is_empty(),
        "expected no value-read on local param `arg`, got {on_arg:?}",
    );
}

#[test]
fn stage2_hoisted_inner_function_call_is_local_no_edge() {
    // `function outer() { inner(); function inner() {} }` — the
    // forward call to `inner` resolves to the hoisted local fn
    // and emits NO module-level edge.
    let (_, edges) = run("function outer() { inner(); function inner() {} }");
    let names: Vec<&str> = calls(&edges)
        .iter()
        .filter_map(|e| match &e.to {
            ResolvedOrUnresolved::Resolved { fqdn } => Some(fqdn.as_str()),
            ResolvedOrUnresolved::Unresolved { name } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        !names.iter().any(|n| n.ends_with("::inner")),
        "expected no call edge for hoisted local `inner`, got {names:?}",
    );
}

#[test]
fn stage2_chained_alias_resolves_through_scope() {
    // const x = FOO; const y = x; takesArg(y)
    //  → y's alias chains through x to src::FOO.
    let (_, edges) = run("function FOO() {} function takesArg(z) {} \
             function f() { const x = FOO; const y = x; takesArg(y); }");
    let refs = refs_with_all_attrs(&edges, &["value-read", "via-alias"]);
    let foo_refs: usize = refs
        .iter()
        .filter(|e| match &e.to {
            ResolvedOrUnresolved::Resolved { fqdn } => fqdn == "src::FOO",
            _ => false,
        })
        .count();
    assert!(
        foo_refs >= 1,
        "expected at least one chained via-alias value-read to src::FOO, got {refs:?}",
    );
}

#[test]
fn stage2_member_chain_alias_resolves_leftmost_base() {
    // `const ns = FOO; consume(ns)` — alias seeded on FOO's
    // resolution. (`FOO.bar.baz` would chain through the same
    // leftmost-base extraction in `resolve_alias_rhs`.)
    let (_, edges) = run("function FOO() {} function consume(z) {} \
             function f() { const ns = FOO; consume(ns); }");
    let refs = refs_with_all_attrs(&edges, &["value-read", "via-alias"]);
    assert!(
        refs.iter().any(|e| matches!(
            &e.to,
            ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "src::FOO"
        )),
        "expected via-alias edge to src::FOO, got {refs:?}",
    );
}

#[test]
fn stage2_block_scope_alias_does_not_leak_outside_block() {
    // `if` block introduces a fresh scope. Alias seeded inside is
    // invisible outside — `consume(x)` in the outer scope should
    // NOT carry a via-alias attribute on the param-named `x`.
    let (_, edges) = run("function FOO() {} function consume(z) {} \
             function f(x) { if (true) { const x = FOO; } consume(x); }");
    // The outer `x` is a function param → local → no edge.
    // The inner `const x = FOO` shadows it inside the if-block,
    // but that scope is popped before consume(x).
    let on_foo = refs_with_all_attrs(&edges, &["value-read", "via-alias"]);
    let leaked: Vec<_> = on_foo
        .iter()
        .filter(|e| {
            matches!(
                &e.to,
                ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "src::FOO"
            )
        })
        .collect();
    assert!(
        leaked.is_empty(),
        "alias leaked out of if-block scope: {leaked:?}",
    );
}

#[test]
fn stage2_call_via_resolved_alias_keeps_via_alias_attribute() {
    // const helper = ACTUAL; helper();
    //   → Calls{Resolved{src::ACTUAL}} tagged via-alias.
    let (_, edges) = run("function ACTUAL() {} function f() { const helper = ACTUAL; helper(); }");
    let cs = calls_with_attr(&edges, "via-alias");
    let resolved_targets: Vec<&str> = cs
        .iter()
        .filter_map(|e| match &e.to {
            ResolvedOrUnresolved::Resolved { fqdn } => Some(fqdn.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        resolved_targets.contains(&"src::ACTUAL"),
        "expected via-alias call to resolved src::ACTUAL, got {resolved_targets:?}",
    );
}

// --- Bug B Stage 2b tests: UsesType edges ---

fn uses_type_edges(edges: &[RawEdge]) -> Vec<&RawEdge> {
    edges
        .iter()
        .filter(|e| e.kind == EdgeKind::UsesType)
        .collect()
}

fn uses_type_with_attrs<'a>(edges: &'a [RawEdge], attrs: &[&str]) -> Vec<&'a RawEdge> {
    edges
        .iter()
        .filter(|e| {
            e.kind == EdgeKind::UsesType
                && attrs.iter().all(|a| e.attributes.iter().any(|x| x == a))
        })
        .collect()
}

fn resolved_fqdns(edges: &[&RawEdge]) -> Vec<String> {
    edges
        .iter()
        .filter_map(|e| match &e.to {
            ResolvedOrUnresolved::Resolved { fqdn } => Some(fqdn.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn stage2b_function_param_type_annotation_emits_uses_type() {
    let (_, edges) = run("interface Foo {} function f(x: Foo) { return x; }");
    let refs = uses_type_with_attrs(&edges, &["via-type", "type-annotation"]);
    let targets = resolved_fqdns(&refs);
    assert!(
        targets.contains(&"src::Foo".to_string()),
        "expected via-type/type-annotation edge to src::Foo, got {targets:?}",
    );
}

#[test]
fn stage2b_function_return_type_annotation_emits_uses_type() {
    let (_, edges) = run("interface Bar {} function f(): Bar { return null as any; }");
    let refs = uses_type_with_attrs(&edges, &["via-type", "type-annotation"]);
    let targets = resolved_fqdns(&refs);
    assert!(
        targets.contains(&"src::Bar".to_string()),
        "expected via-type/type-annotation edge to src::Bar, got {targets:?}",
    );
}

#[test]
fn stage2b_var_type_annotation_emits_uses_type() {
    let (_, edges) = run("interface Foo {} const x: Foo = null as any;");
    let refs = uses_type_with_attrs(&edges, &["via-type", "type-annotation"]);
    let foo_edges: Vec<_> = refs
        .iter()
        .filter(|e| matches!(&e.to, ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "src::Foo"))
        .collect();
    assert!(
        !foo_edges.is_empty(),
        "expected via-type edge on src::Foo from var annotation, got {refs:?}",
    );
    // The edge should originate from `src::x`, not the module.
    assert!(
        foo_edges.iter().any(|e| e.from_fqdn == "src::x"),
        "expected edge from src::x, got {foo_edges:?}",
    );
}

#[test]
fn stage2b_type_alias_body_emits_uses_type() {
    let (_, edges) = run("interface A {} interface B {} type X = A | B;");
    let refs = uses_type_with_attrs(&edges, &["via-type", "type-alias-body"]);
    let targets = resolved_fqdns(&refs);
    assert!(
        targets.contains(&"src::A".to_string()) && targets.contains(&"src::B".to_string()),
        "expected via-type/type-alias-body edges to A and B, got {targets:?}",
    );
    // From-fqdn should be the alias.
    assert!(
        refs.iter().all(|e| e.from_fqdn == "src::X"),
        "expected all alias-body edges originating from src::X, got {refs:?}",
    );
}

#[test]
fn stage2b_interface_member_type_emits_uses_type() {
    // Bug C-1: interface members are now per-member sub-symbols
    // (`src::Bar::x`), so the `via-type/type-interface-member`
    // edge originates from the member fqdn, not the interface.
    let (_, edges) = run("interface Foo {} interface Bar { x: Foo }");
    let refs = uses_type_with_attrs(&edges, &["via-type", "type-interface-member"]);
    let targets = resolved_fqdns(&refs);
    assert!(
        targets.contains(&"src::Foo".to_string()),
        "expected via-type/type-interface-member edge to src::Foo, got {targets:?}",
    );
    assert!(
        refs.iter().any(|e| e.from_fqdn == "src::Bar::x"),
        "expected edge from member src::Bar::x, got {refs:?}",
    );
}

#[test]
fn stage2b_class_prop_type_emits_uses_type() {
    let (_, edges) = run("interface Foo {} class Bar { field: Foo; }");
    let refs = uses_type_with_attrs(&edges, &["via-type", "type-class-prop"]);
    let targets = resolved_fqdns(&refs);
    assert!(
        targets.contains(&"src::Foo".to_string()),
        "expected via-type/type-class-prop edge to src::Foo, got {targets:?}",
    );
    assert!(
        refs.iter().any(|e| e.from_fqdn == "src::Bar::field"),
        "expected edge from src::Bar::field, got {refs:?}",
    );
}

#[test]
fn stage2b_generic_constraint_emits_uses_type() {
    let (_, edges) = run("interface Foo {} function f<T extends Foo>(x: T) { return x; }");
    let refs = uses_type_with_attrs(&edges, &["via-type", "type-constraint"]);
    let targets = resolved_fqdns(&refs);
    assert!(
        targets.contains(&"src::Foo".to_string()),
        "expected via-type/type-constraint edge to src::Foo, got {targets:?}",
    );
}

#[test]
fn stage2b_generic_param_does_not_leak_as_uses_type() {
    // `<T>` is bound as a scope-local; `x: T` resolves Local → no edge.
    let (_, edges) = run("function f<T>(x: T) { return x; }");
    let refs = uses_type_edges(&edges);
    let on_t: Vec<_> = refs
        .iter()
        .filter(|e| match &e.to {
            ResolvedOrUnresolved::Resolved { fqdn } => fqdn.ends_with("::T"),
            ResolvedOrUnresolved::Unresolved { name }
            | ResolvedOrUnresolved::UnresolvedBridge { name, .. } => name.ends_with("::T"),
        })
        .collect();
    assert!(
        on_t.is_empty(),
        "expected no UsesType edge on generic param T, got {on_t:?}",
    );
}

#[test]
fn stage2b_type_cast_emits_uses_type_with_type_cast_attr() {
    let (_, edges) = run("interface Foo {} function f(x: any) { return x as Foo; }");
    let refs = uses_type_with_attrs(&edges, &["via-type", "type-cast"]);
    let targets = resolved_fqdns(&refs);
    assert!(
        targets.contains(&"src::Foo".to_string()),
        "expected via-type/type-cast edge to src::Foo, got {targets:?}",
    );
}

#[test]
fn stage2b_generic_instantiation_emits_uses_type_with_instantiation_attr() {
    let (_, edges) = run(
        "interface Foo {} function generic<T>(): T { return null as any; } \
             function f() { return generic<Foo>(); }",
    );
    let refs = uses_type_with_attrs(&edges, &["via-type", "type-instantiation"]);
    let targets = resolved_fqdns(&refs);
    assert!(
        targets.contains(&"src::Foo".to_string()),
        "expected via-type/type-instantiation edge to src::Foo, got {targets:?}",
    );
}

#[test]
fn stage2b_ts_builtin_wrapper_filtered_inner_args_still_emit() {
    // `Map<Foo, Bar>` — outer Map filtered, inner Foo + Bar emit.
    let (_, edges) = run("interface Foo {} interface Bar {} \
             function f(): Map<Foo, Bar> { return new Map(); }");
    let refs = uses_type_with_attrs(&edges, &["via-type"]);
    let targets = resolved_fqdns(&refs);
    assert!(
        !targets.contains(&"src::Map".to_string()),
        "expected Map filtered as builtin, got {targets:?}",
    );
    assert!(
        targets.contains(&"src::Foo".to_string()),
        "expected via-type edge to src::Foo, got {targets:?}",
    );
    assert!(
        targets.contains(&"src::Bar".to_string()),
        "expected via-type edge to src::Bar, got {targets:?}",
    );
}

#[test]
fn stage2b_class_extends_generic_args_emit_uses_type() {
    let (_, edges) = run("interface Foo {} class Base<T> {} class Derived extends Base<Foo> {}");
    let refs = uses_type_with_attrs(&edges, &["via-type", "type-extends"]);
    let targets = resolved_fqdns(&refs);
    assert!(
        targets.contains(&"src::Foo".to_string()),
        "expected via-type/type-extends edge to src::Foo, got {targets:?}",
    );
}

#[test]
fn stage2b_class_implements_generic_args_emit_uses_type() {
    let (_, edges) = run("interface Foo {} interface Iface<T> {} class C implements Iface<Foo> {}");
    let refs = uses_type_with_attrs(&edges, &["via-type", "type-implements"]);
    let targets = resolved_fqdns(&refs);
    assert!(
        targets.contains(&"src::Foo".to_string()),
        "expected via-type/type-implements edge to src::Foo, got {targets:?}",
    );
}

#[test]
fn stage2b_unresolved_type_carries_unresolved_type_attribute() {
    // `BareUnknown` is not defined locally or imported → Unresolved.
    // Stage 2b emits anyway with `unresolved-type` attribute (debug
    // toggleable in viz).
    let (_, edges) = run("function f(x: BareUnknown) { return x; }");
    let refs = uses_type_with_attrs(&edges, &["via-type", "unresolved-type"]);
    assert!(
        !refs.is_empty(),
        "expected at least one unresolved-type edge, got none",
    );
    // The target should be the canonical Unresolved form.
    let unresolved_names: Vec<&str> = refs
        .iter()
        .filter_map(|e| match &e.to {
            ResolvedOrUnresolved::Unresolved { name } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        unresolved_names
            .iter()
            .any(|n| n.ends_with("::BareUnknown")),
        "expected unresolved target ending in ::BareUnknown, got {unresolved_names:?}",
    );
}

#[test]
fn stage2b_local_var_typed_with_local_class_emits_uses_type() {
    // const x: LocalClass = new LocalClass() — `LocalClass` is a
    // top-level class, so the var's type-annotation resolves
    // cleanly to its fqdn (not a local binding).
    let (_, edges) = run("class LocalClass {} const x: LocalClass = new LocalClass();");
    let refs = uses_type_with_attrs(&edges, &["via-type", "type-annotation"]);
    let targets = resolved_fqdns(&refs);
    assert!(
        targets.contains(&"src::LocalClass".to_string()),
        "expected via-type edge to src::LocalClass, got {targets:?}",
    );
}

#[test]
fn stage2b_qualified_type_name_uses_leftmost_base() {
    // `Foo.Bar` in type position → emit on Foo (the leftmost base).
    let (_, edges) = run("interface Foo {} function f(): Foo.Bar { return null as any; }");
    let refs = uses_type_with_attrs(&edges, &["via-type", "type-annotation"]);
    let targets = resolved_fqdns(&refs);
    assert!(
        targets.contains(&"src::Foo".to_string()),
        "expected via-type edge to leftmost src::Foo, got {targets:?}",
    );
}

// --- Bug C-1 tests: TS interface + enum member granularity ---

fn symbol_fqdns(symbols: &[standardoc_ir::RawSymbol]) -> Vec<&str> {
    symbols.iter().map(|s| s.fqdn.as_str()).collect()
}

#[test]
fn bug_c1_interface_property_is_indexed_as_sub_symbol() {
    let (symbols, _) = run("interface User { id: number; name: string; }");
    let fqdns = symbol_fqdns(&symbols);
    assert!(
        fqdns.contains(&"src::User"),
        "expected User interface in index, got {fqdns:?}",
    );
    assert!(
        fqdns.contains(&"src::User::id"),
        "expected User::id sub-symbol, got {fqdns:?}",
    );
    assert!(
        fqdns.contains(&"src::User::name"),
        "expected User::name sub-symbol, got {fqdns:?}",
    );
    // Sub-symbols must declare their parent via `module`.
    let id_sym = symbols
        .iter()
        .find(|s| s.fqdn == "src::User::id")
        .expect("User::id found");
    assert_eq!(id_sym.module.as_deref(), Some("src::User"));
    assert_eq!(id_sym.language_kind.as_str(), "interface_property");
}

#[test]
fn bug_c1_interface_method_is_indexed_as_sub_symbol() {
    let (symbols, _) = run("interface API { fetch(): void; }");
    let fqdns = symbol_fqdns(&symbols);
    assert!(
        fqdns.contains(&"src::API::fetch"),
        "expected API::fetch sub-symbol, got {fqdns:?}",
    );
    let fetch_sym = symbols
        .iter()
        .find(|s| s.fqdn == "src::API::fetch")
        .unwrap();
    assert_eq!(fetch_sym.language_kind.as_str(), "interface_method");
    assert_eq!(fetch_sym.module.as_deref(), Some("src::API"));
}

#[test]
fn bug_c1_interface_getter_setter_are_indexed() {
    let (symbols, _) = run("interface Box<T> { get value(): T; set value(v: T); }");
    let fqdns = symbol_fqdns(&symbols);
    let getter = symbols
        .iter()
        .find(|s| s.fqdn == "src::Box::value" && s.language_kind.as_str() == "interface_getter")
        .map(|s| s.fqdn.as_str());
    let setter = symbols
        .iter()
        .find(|s| s.fqdn == "src::Box::value" && s.language_kind.as_str() == "interface_setter")
        .map(|s| s.fqdn.as_str());
    assert!(
        getter.is_some() && setter.is_some(),
        "expected both getter+setter for Box::value, got {fqdns:?}",
    );
}

#[test]
fn bug_c1_interface_member_type_edges_originate_from_member_fqdn() {
    let (symbols, edges) = run("interface Foo {} interface Bar {} \
             interface Combo { x: Foo; m(p: Bar): void; }");
    let fqdns = symbol_fqdns(&symbols);
    assert!(fqdns.contains(&"src::Combo::x"));
    assert!(fqdns.contains(&"src::Combo::m"));
    // Per-member edges: x → Foo from `src::Combo::x`.
    let x_edges: Vec<_> = edges
        .iter()
        .filter(|e| e.from_fqdn == "src::Combo::x")
        .collect();
    assert!(
        x_edges.iter().any(|e| matches!(
            &e.to,
            ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "src::Foo"
        )),
        "expected UsesType from Combo::x to Foo, got {x_edges:?}",
    );
    // m → Bar from `src::Combo::m`.
    let m_edges: Vec<_> = edges
        .iter()
        .filter(|e| e.from_fqdn == "src::Combo::m")
        .collect();
    assert!(
        m_edges.iter().any(|e| matches!(
            &e.to,
            ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "src::Bar"
        )),
        "expected UsesType from Combo::m to Bar, got {m_edges:?}",
    );
}

#[test]
fn bug_c1_ts_enum_members_are_indexed_as_sub_symbols() {
    let (symbols, _) = run("enum Status { Active, Inactive, Pending = 5 }");
    let fqdns = symbol_fqdns(&symbols);
    assert!(fqdns.contains(&"src::Status"));
    assert!(fqdns.contains(&"src::Status::Active"));
    assert!(fqdns.contains(&"src::Status::Inactive"));
    assert!(fqdns.contains(&"src::Status::Pending"));
    let active = symbols
        .iter()
        .find(|s| s.fqdn == "src::Status::Active")
        .unwrap();
    assert_eq!(active.language_kind.as_str(), "enum_member");
    assert_eq!(active.module.as_deref(), Some("src::Status"));
}

#[test]
fn bug_c1_interface_computed_key_skipped() {
    // Computed-key members (`[KEY]: T`) can't be assigned a stable
    // FQDN — skipped silently.
    let (symbols, _) = run("const KEY = 'k'; interface Dyn { [KEY]: number; }");
    let fqdns = symbol_fqdns(&symbols);
    assert!(fqdns.contains(&"src::Dyn"));
    // No anonymous sub-symbol pushed.
    let dyn_children: Vec<_> = symbols
        .iter()
        .filter(|s| s.module.as_deref() == Some("src::Dyn"))
        .collect();
    assert!(
        dyn_children.is_empty(),
        "expected no sub-symbols for computed-key interface, got {dyn_children:?}",
    );
}

// --- Stage 3c: class/interface-level generics propagate to inner
// method/member signatures through the lookup's parent-chain walk.
// Pre-3a-6c, the visitor's hand-maintained scope stack handled the
// simple cases but the visitor only added type-params it knew about
// — a class-level `<T>` reachable from a method's signature relied
// on the lookup pre-pass having seeded both the class scope AND the
// method scope's parent chain. Stage 3a-6c made resolve_local walk
// the chain so the propagation is now structural rather than
// duplicated.

#[test]
fn stage3c_class_method_filters_class_level_generic() {
    let (_, edges) = run("class Box<T> { take(x: T): T { return x; } }");
    let refs = uses_type_with_attrs(&edges, &["via-type", "type-annotation"]);
    let leaked: Vec<&RawEdge> = refs
        .iter()
        .copied()
        .filter(|e| match &e.to {
            ResolvedOrUnresolved::Resolved { fqdn } => fqdn.ends_with("::T"),
            ResolvedOrUnresolved::Unresolved { name } => name.ends_with("::T"),
            _ => false,
        })
        .collect();
    assert!(
        leaked.is_empty(),
        "class-level T leaked into Box's method signature: {leaked:?}",
    );
}

#[test]
fn stage3c_class_method_inner_generic_combined_with_outer() {
    let (_, edges) = run("class Box<T> { take<U>(x: T, y: U): T { return x; } }");
    let refs = uses_type_with_attrs(&edges, &["via-type", "type-annotation"]);
    let leaked: Vec<String> = refs
        .iter()
        .filter_map(|e| match &e.to {
            ResolvedOrUnresolved::Resolved { fqdn }
                if fqdn.ends_with("::T") || fqdn.ends_with("::U") =>
            {
                Some(fqdn.clone())
            }
            ResolvedOrUnresolved::Unresolved { name }
                if name.ends_with("::T") || name.ends_with("::U") =>
            {
                Some(name.clone())
            }
            _ => None,
        })
        .collect();
    assert!(
        leaked.is_empty(),
        "neither outer T nor inner U should leak: got {leaked:?}",
    );
}

#[test]
fn stage3c_interface_method_inner_generic_combined_with_outer() {
    let (_, edges) = run("interface Box<T> { take<U>(x: T, y: U): T; }");
    let refs = uses_type_with_attrs(&edges, &["via-type", "type-interface-member"]);
    let leaked: Vec<String> = refs
        .iter()
        .filter_map(|e| match &e.to {
            ResolvedOrUnresolved::Resolved { fqdn }
                if fqdn.ends_with("::T") || fqdn.ends_with("::U") =>
            {
                Some(fqdn.clone())
            }
            ResolvedOrUnresolved::Unresolved { name }
                if name.ends_with("::T") || name.ends_with("::U") =>
            {
                Some(name.clone())
            }
            _ => None,
        })
        .collect();
    assert!(
        leaked.is_empty(),
        "interface-level T + method-level U must both be local: {leaked:?}",
    );
}

// --- IR-4-c: call_sites emission (observational, separate from edges) ---

fn run_with_call_sites(source: &str) -> Vec<RawCallSite> {
    let (cm, module, comments) = parse_ts(source);
    let (_, _, _, css) = super::super::walk::walk(
        &module,
        "@app",
        "src/index.ts",
        "src",
        cm,
        &PathBuf::from("/tmp/pkg/src/index.ts"),
        &PathBuf::from("/tmp/pkg"),
        None,
        &comments,
    );
    css
}

#[test]
fn ir4c_free_fn_call_emits_call_site_with_classified_args() {
    // `foo("hi", 42, x)` — three positional args: a string literal,
    // a numeric literal, and an identifier. Args must classify
    // string-literals vs others correctly.
    let css = run_with_call_sites("function caller() { let x = 0; foo(\"hi\", 42, x); }");
    let cs = css
        .iter()
        .find(|c| c.callee_text == "foo")
        .unwrap_or_else(|| panic!("expected foo(...) call_site, got {css:?}"));
    assert_eq!(cs.from_fqdn, "src::caller");
    assert!(cs.receiver_chain.is_empty());
    assert_eq!(cs.args.len(), 3);
    assert_eq!(cs.args[0].value, "hi");
    assert!(cs.args[0].is_string_literal);
    assert!(!cs.args[1].is_string_literal);
    assert!(cs.args[1].value.contains("42"));
    assert_eq!(cs.args[2].value, "x");
    assert!(!cs.args[2].is_string_literal);
}

#[test]
fn ir4c_member_call_emits_receiver_chain_single_segment() {
    // `obj.bar(x)` — receiver_chain=["obj"], callee_text="obj.bar".
    let css = run_with_call_sites("function caller() { obj.bar(x); }");
    let cs = css
        .iter()
        .find(|c| c.callee_text == "obj.bar")
        .unwrap_or_else(|| panic!("expected obj.bar call_site, got {css:?}"));
    assert_eq!(cs.receiver_chain, vec!["obj".to_string()]);
}

#[test]
fn ir4c_chained_member_call_walks_through_member_layers() {
    // `obj.field.bar(x)` — receiver = ExprMember { obj: Member{obj:Ident("obj"), prop:"field"}, prop:"bar" }
    // Chain segments are pushed inner→outer then reversed, so the
    // result is ["obj", "field"] in source order.
    let css = run_with_call_sites("function caller() { obj.field.bar(x); }");
    let cs = css
        .iter()
        .find(|c| c.callee_text == "obj.field.bar")
        .unwrap_or_else(|| panic!("expected obj.field.bar call_site, got {css:?}"));
    assert_eq!(
        cs.receiver_chain,
        vec!["obj".to_string(), "field".to_string()]
    );
}

#[test]
fn ir4c_optional_chain_call_uses_question_dot_in_callee_text() {
    // `obj?.bar(x)` — optional chain on member. callee_text reflects
    // the optional `.` syntax so plugins can distinguish from a
    // direct member access. receiver_chain is still ["obj"].
    let css = run_with_call_sites("function caller() { obj?.bar(x); }");
    let cs = css
        .iter()
        .find(|c| c.callee_text == "obj?.bar")
        .unwrap_or_else(|| panic!("expected obj?.bar call_site, got {css:?}"));
    assert_eq!(cs.receiver_chain, vec!["obj".to_string()]);
}

#[test]
fn ir4c_new_expression_emits_call_site_with_constructor_text() {
    // `new Foo(x)` — surfaces with the constructor expression as
    // callee_text (no `new` prefix). Receiver chain stays empty.
    let css = run_with_call_sites("function caller() { new Foo(x); }");
    let cs = css
        .iter()
        .find(|c| c.callee_text == "Foo")
        .unwrap_or_else(|| panic!("expected new Foo(...) call_site, got {css:?}"));
    assert!(cs.receiver_chain.is_empty());
    assert_eq!(cs.args.len(), 1);
    assert_eq!(cs.args[0].value, "x");
}

#[test]
fn ir4c_spread_arg_carries_dotdotdot_prefix() {
    // `foo(...rest)` — spread args carry a `...` prefix in `value`
    // and are never tagged as string literals (the spread source
    // isn't evaluated to a string at the call site).
    let css = run_with_call_sites("function caller() { foo(...rest); }");
    let cs = css
        .iter()
        .find(|c| c.callee_text == "foo")
        .unwrap_or_else(|| panic!("expected foo call_site, got {css:?}"));
    assert_eq!(cs.args.len(), 1);
    assert_eq!(cs.args[0].value, "...rest");
    assert!(!cs.args[0].is_string_literal);
}

#[test]
fn ir4c_super_call_emits_call_site_with_super_callee_text() {
    // `super(x)` — `Callee::Super`. callee_text is the literal
    // string "super"; receiver_chain stays empty. Attributed to
    // the constructor's FQDN now that `visit_constructor_body`
    // walks the constructor body in `walk::visit_class_methods`.
    let css = run_with_call_sites("class Foo extends Bar { constructor() { super(\"x\"); } }");
    let cs = css
        .iter()
        .find(|c| c.callee_text == "super")
        .unwrap_or_else(|| panic!("expected super(...) call_site, got {css:?}"));
    assert_eq!(cs.from_fqdn, "src::Foo::constructor");
    assert!(cs.receiver_chain.is_empty());
    assert_eq!(cs.args.len(), 1);
    assert!(cs.args[0].is_string_literal);
    assert_eq!(cs.args[0].value, "x");
}

#[test]
fn ir4c_constructor_body_call_attributed_to_ctor_fqdn() {
    // Free-fn call inside a constructor body must be attributed
    // to `<module>::<Class>::constructor`, not the enclosing
    // module FQDN — proves the new ctor walker passes the right
    // enclosing_fqdn down to the CallVisitor.
    let css = run_with_call_sites("class Foo { constructor() { helper(\"x\"); } }");
    let cs = css
        .iter()
        .find(|c| c.callee_text == "helper")
        .unwrap_or_else(|| panic!("expected helper(...) call_site, got {css:?}"));
    assert_eq!(cs.from_fqdn, "src::Foo::constructor");
    assert!(cs.receiver_chain.is_empty());
}

#[test]
fn ir4c_constructor_body_new_expression_emits_call_site() {
    // `new Bar()` inside a constructor — NewExpr path traversed by
    // the new ctor walker, attributed to the ctor FQDN.
    let css = run_with_call_sites("class Foo { constructor() { new Bar(\"x\"); } }");
    let cs = css
        .iter()
        .find(|c| c.callee_text == "Bar")
        .unwrap_or_else(|| panic!("expected new Bar(...) call_site, got {css:?}"));
    assert_eq!(cs.from_fqdn, "src::Foo::constructor");
    assert_eq!(cs.args.len(), 1);
    assert!(cs.args[0].is_string_literal);
}

#[test]
fn ir4c_constructor_body_method_chain_receiver_walked() {
    // `this.api.create()` inside a constructor — receiver_chain
    // walker should produce `["this", "api"]` and attribute the
    // call to the ctor FQDN. Proves the scope push/pop of the
    // new `visit_constructor` override doesn't break member-walk.
    let css = run_with_call_sites("class Foo { constructor() { this.api.create(); } }");
    let cs = css
        .iter()
        .find(|c| c.callee_text == "this.api.create")
        .unwrap_or_else(|| panic!("expected this.api.create() call_site, got {css:?}"));
    assert_eq!(cs.from_fqdn, "src::Foo::constructor");
    assert_eq!(
        cs.receiver_chain,
        vec!["this".to_string(), "api".to_string()]
    );
}

#[test]
fn ir4c_dynamic_import_emits_call_site_with_import_callee_text() {
    // `import("./mod")` — `Callee::Import`. callee_text="import".
    let css = run_with_call_sites("function caller() { import(\"./mod\"); }");
    let cs = css
        .iter()
        .find(|c| c.callee_text == "import")
        .unwrap_or_else(|| panic!("expected import(...) call_site, got {css:?}"));
    assert_eq!(cs.args.len(), 1);
    assert!(cs.args[0].is_string_literal);
    assert_eq!(cs.args[0].value, "./mod");
}

#[test]
fn ir4c_drop_tier_member_call_still_emits_call_site() {
    // `arr.push(x)` — `Array.push` is a Drop-tier builtin in the
    // edge layer (gets suppressed), but the call_site must still
    // surface so plugins reading textual patterns aren't blinded.
    let css = run_with_call_sites("function caller() { const arr = [1]; arr.push(2); }");
    let cs = css
        .iter()
        .find(|c| c.callee_text == "arr.push")
        .unwrap_or_else(|| panic!("expected arr.push call_site, got {css:?}"));
    assert_eq!(cs.receiver_chain, vec!["arr".to_string()]);
}

#[test]
fn ir4c_call_site_from_fqdn_attributes_to_enclosing_method() {
    // `class Svc { run() { helper(); } }` — call_site's `from_fqdn`
    // must point at `src::Svc::run`, not the module fqdn.
    let css = run_with_call_sites("class Svc { run() { helper(); } }");
    let cs = css
        .iter()
        .find(|c| c.callee_text == "helper")
        .unwrap_or_else(|| panic!("expected helper() call_site, got {css:?}"));
    assert_eq!(cs.from_fqdn, "src::Svc::run");
}

// --- IR-4-e: top-level Stmt::Expr emission (Vue script-setup unblock) ---

#[test]
fn ir4e_top_level_stmt_expr_emits_call_site_with_module_fqdn() {
    // Top-level expression statement at module scope (Vue 3
    // script-setup idiom). No enclosing function/method, so
    // `from_fqdn` is the module fqdn itself.
    let css = run_with_call_sites("foo(\"x\");");
    let cs = css
        .iter()
        .find(|c| c.callee_text == "foo")
        .unwrap_or_else(|| panic!("expected foo(...) call_site, got {css:?}"));
    assert_eq!(cs.from_fqdn, "src");
    assert!(cs.receiver_chain.is_empty());
    assert_eq!(cs.args.len(), 1);
    assert!(cs.args[0].is_string_literal);
    assert_eq!(cs.args[0].value, "x");
}

#[test]
fn ir4e_top_level_new_expression_emits_call_site() {
    // Top-level NewExpr — proves the new Stmt::Expr arm walks
    // every callable shape (NewExpr, not just CallExpr).
    let css = run_with_call_sites("new Foo(\"x\");");
    let cs = css
        .iter()
        .find(|c| c.callee_text == "Foo")
        .unwrap_or_else(|| panic!("expected new Foo(...) call_site, got {css:?}"));
    assert_eq!(cs.from_fqdn, "src");
    assert_eq!(cs.args.len(), 1);
    assert!(cs.args[0].is_string_literal);
}

#[test]
fn ir4e_top_level_method_chain_receiver_walked() {
    // Top-level member-access call — receiver_chain walker must
    // produce `["obj", "api"]` even when the call is at module
    // scope (no enclosing fn). Attribution = module fqdn.
    let css = run_with_call_sites("obj.api.create();");
    let cs = css
        .iter()
        .find(|c| c.callee_text == "obj.api.create")
        .unwrap_or_else(|| panic!("expected obj.api.create() call_site, got {css:?}"));
    assert_eq!(cs.from_fqdn, "src");
    assert_eq!(
        cs.receiver_chain,
        vec!["obj".to_string(), "api".to_string()]
    );
}

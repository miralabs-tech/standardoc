use super::*;
use std::path::PathBuf;
use swc_core::common::{BytePos, FileName, sync::Lrc};
use swc_core::ecma::ast::EsVersion;
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

fn run(
    source: &str,
) -> (
    Vec<RawSymbol>,
    Vec<RawEdge>,
    Vec<RawDocument>,
    Vec<RawCallSite>,
) {
    let (cm, module, comments) = parse_ts(source);
    walk(
        &module,
        "@app",
        "src/index.ts",
        "src",
        cm,
        &PathBuf::from("/tmp/pkg/src/index.ts"),
        &PathBuf::from("/tmp/pkg"),
        None,
        &comments,
    )
}

#[test]
fn function_decl_emits_function_symbol() {
    let (symbols, edges, _docs, _) = run("function foo() {}");
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].kind, Kind::Callable);
    assert_eq!(symbols[0].fqdn, "src::foo");
    assert_eq!(symbols[0].visibility, Visibility::Private);
    assert!(edges.is_empty());
}

#[test]
fn export_function_decl_is_public() {
    let (symbols, _, _, _) = run("export function foo() {}");
    assert_eq!(symbols[0].visibility, Visibility::Public);
}

#[test]
fn function_signature_captures_param_types_and_return() {
    let (symbols, _, _, _) =
        run("export function add(a: number, b: number): number { return a + b; }");
    let sig = symbols[0].signature.as_ref().unwrap();
    assert_eq!(sig.params.len(), 2);
    assert_eq!(sig.params[0].name, "a");
    assert_eq!(sig.params[0].ty.display, "number");
    assert_eq!(sig.params[1].name, "b");
    assert_eq!(sig.returns.as_ref().unwrap().display, "number");
}

#[test]
fn async_function_modifier_set() {
    let (symbols, _, _, _) = run("export async function boot() {}");
    assert!(symbols[0].signature.as_ref().unwrap().modifiers.is_async);
}

#[test]
fn function_default_param_captured() {
    let (symbols, _, _, _) = run("export function f(x: number = 7) {}");
    let p = &symbols[0].signature.as_ref().unwrap().params[0];
    assert_eq!(p.default.as_deref(), Some("7"));
}

#[test]
fn rest_param_prefixed_with_ellipsis() {
    let (symbols, _, _, _) = run("export function f(...args: number[]) {}");
    let p = &symbols[0].signature.as_ref().unwrap().params[0];
    assert_eq!(p.name, "...args");
    assert_eq!(p.ty.display, "number[]");
}

#[test]
fn generic_params_captured_as_strings() {
    let (symbols, _, _, _) = run("export function id<T>(x: T): T { return x; }");
    let g = &symbols[0]
        .signature
        .as_ref()
        .unwrap()
        .modifiers
        .generic_params;
    assert_eq!(g.len(), 1);
    assert_eq!(g[0], "T");
}

#[test]
fn class_emits_type_symbol_and_methods() {
    let (symbols, _, _, _) = run("export class Foo { run(): void {} }");
    let foo = symbols.iter().find(|s| s.fqdn == "src::Foo").unwrap();
    assert_eq!(foo.kind, Kind::Type);
    assert_eq!(foo.language_kind.as_str(), "class");
    let run = symbols.iter().find(|s| s.fqdn == "src::Foo::run").unwrap();
    assert_eq!(run.kind, Kind::Callable);
    assert_eq!(run.language_kind.as_str(), "method");
}

#[test]
fn class_extends_emits_extends_edge() {
    let (_, edges, _, _) = run("class Foo extends Bar {}");
    let ext: Vec<_> = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Extends)
        .collect();
    assert_eq!(ext.len(), 1);
    assert_eq!(ext[0].from_fqdn, "src::Foo");
    match &ext[0].to {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "src::Bar"),
        other => panic!("expected unresolved, got {other:?}"),
    }
}

#[test]
fn class_implements_emits_implements_edge() {
    let (_, edges, _, _) = run("class Foo implements IBar {}");
    let imp: Vec<_> = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Implements)
        .collect();
    assert_eq!(imp.len(), 1);
    assert_eq!(imp[0].from_fqdn, "src::Foo");
    match &imp[0].to {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "src::IBar"),
        other => panic!("expected unresolved, got {other:?}"),
    }
}

#[test]
fn class_implements_namespace_qualified_routes_through_alias() {
    // Bug C: `class Foo implements vscode.Disposable` after
    // `import * as vscode from 'vscode'` must produce an IMPLEMENTS edge
    // targeting `vscode::Disposable`, not the bogus module-local
    // fallback `src::vscode.Disposable`.
    let (_, edges, _, _) =
        run("import * as vscode from 'vscode';\nclass Foo implements vscode.Disposable {}");
    let imp: Vec<_> = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Implements)
        .collect();
    assert_eq!(imp.len(), 1);
    match &imp[0].to {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "vscode::Disposable"),
        other => panic!("expected unresolved namespace-qualified, got {other:?}"),
    }
}

#[test]
fn class_method_accessibility_maps_to_visibility() {
    let (symbols, _, _, _) =
        run("class Foo { public a() {} private b() {} protected c() {} d() {} }");
    let a = symbols.iter().find(|s| s.fqdn == "src::Foo::a").unwrap();
    assert_eq!(a.visibility, Visibility::Public);
    let b = symbols.iter().find(|s| s.fqdn == "src::Foo::b").unwrap();
    assert_eq!(b.visibility, Visibility::Private);
    let c = symbols.iter().find(|s| s.fqdn == "src::Foo::c").unwrap();
    assert_eq!(c.visibility, Visibility::Protected);
    let d = symbols.iter().find(|s| s.fqdn == "src::Foo::d").unwrap();
    assert_eq!(d.visibility, Visibility::Public);
}

#[test]
fn interface_emits_type_symbol() {
    let (symbols, _, _, _) = run("export interface IFoo { x: number }");
    assert_eq!(symbols[0].kind, Kind::Type);
    assert_eq!(symbols[0].language_kind.as_str(), "interface");
    assert_eq!(symbols[0].fqdn, "src::IFoo");
}

#[test]
fn interface_extends_emits_extends_edge() {
    let (_, edges, _, _) = run("interface A extends B {}");
    let ext: Vec<_> = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Extends)
        .collect();
    assert_eq!(ext.len(), 1);
    assert_eq!(ext[0].from_fqdn, "src::A");
}

#[test]
fn type_alias_emits_type_symbol() {
    let (symbols, _, _, _) = run("export type Bytes = Uint8Array;");
    assert_eq!(symbols[0].kind, Kind::Type);
    assert_eq!(symbols[0].language_kind.as_str(), "type_alias");
    assert_eq!(symbols[0].fqdn, "src::Bytes");
}

#[test]
fn enum_emits_type_symbol() {
    let (symbols, _, _, _) = run("export enum Color { Red, Green }");
    assert_eq!(symbols[0].kind, Kind::Type);
    assert_eq!(symbols[0].language_kind.as_str(), "enum");
}

#[test]
fn const_var_emits_value_symbol() {
    let (symbols, _, _, _) = run("export const N = 42;");
    assert_eq!(symbols[0].kind, Kind::Value);
    assert_eq!(symbols[0].language_kind.as_str(), "const");
    assert_eq!(symbols[0].fqdn, "src::N");
}

#[test]
fn arrow_const_emits_function_symbol() {
    let (symbols, _, _, _) = run("export const add = (a: number, b: number): number => a + b;");
    assert_eq!(symbols[0].kind, Kind::Callable);
    assert_eq!(symbols[0].language_kind.as_str(), "function");
    let sig = symbols[0].signature.as_ref().unwrap();
    assert_eq!(sig.params.len(), 2);
    assert_eq!(sig.params[0].name, "a");
    assert_eq!(sig.returns.as_ref().unwrap().display, "number");
}

#[test]
fn import_named_emits_import_edge_and_alias() {
    let (_, edges, _, _) = run("import { foo } from './helper';");
    let imp: Vec<_> = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Imports)
        .collect();
    assert_eq!(imp.len(), 1);
}

#[test]
fn import_default_emits_import_edge() {
    let (_, edges, _, _) = run("import React from 'react';");
    assert!(edges.iter().any(|e| e.kind == EdgeKind::Imports));
}

#[test]
fn import_namespace_emits_import_edge() {
    let (_, edges, _, _) = run("import * as utils from './utils';");
    assert!(edges.iter().any(|e| e.kind == EdgeKind::Imports));
}

#[test]
fn import_side_effect_emits_one_edge_no_alias() {
    let (_, edges, _, _) = run("import 'polyfill';");
    let imp: Vec<_> = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Imports)
        .collect();
    assert_eq!(imp.len(), 1);
}

#[test]
fn export_default_function_named() {
    let (symbols, _, _, _) = run("export default function foo() {}");
    let foo = symbols.iter().find(|s| s.fqdn == "src::foo").unwrap();
    assert_eq!(foo.kind, Kind::Callable);
    assert_eq!(foo.visibility, Visibility::Public);
}

#[test]
fn export_default_function_anonymous_uses_default_name() {
    let (symbols, _, _, _) = run("export default function () {}");
    assert_eq!(symbols[0].fqdn, "src::default");
}

#[test]
fn export_default_class_named() {
    let (symbols, _, _, _) = run("export default class Foo {}");
    let foo = symbols.iter().find(|s| s.fqdn == "src::Foo").unwrap();
    assert_eq!(foo.kind, Kind::Type);
}

#[test]
fn span_locations_are_captured() {
    let (symbols, _, _, _) = run("\n\nexport function foo() {}\n");
    assert_eq!(symbols[0].location.start_line, 3);
}

#[test]
fn body_hash_changes_with_body_content() {
    let (sym_a, _, _, _) = run("export function foo() { return 1; }");
    let (sym_b, _, _, _) = run("export function foo() { return 2; }");
    assert_ne!(sym_a[0].body_hash, sym_b[0].body_hash);
}

fn expect_emit(outcome: ResolutionOutcome) -> CallTarget {
    match outcome {
        ResolutionOutcome::Emit(target) => target,
        other => panic!("expected ResolutionOutcome::Emit, got {other:?}"),
    }
}

#[test]
fn resolve_call_via_alias_table() {
    let (cm, _module, comments) = parse_ts("");
    let mut ctx = TsWalkContext::new(
        "src/index.ts".into(),
        "@app".into(),
        "src".into(),
        cm,
        PathBuf::new(),
        PathBuf::new(),
        None,
        &comments,
    );
    ctx.add_import_alias(
        "Foo".into(),
        ResolvedImport {
            target: ResolvedOrUnresolved::Unresolved {
                name: "@app::src::foo::Foo".into(),
            },
        },
    );
    let target = expect_emit(ctx.resolve_call("Foo", "src"));
    assert!(
        target.via_builtin.is_none(),
        "alias resolution carries no via_builtin"
    );
    match target.to {
        ResolvedOrUnresolved::Unresolved { name } => {
            assert_eq!(name, "@app::src::foo::Foo");
        }
        other => panic!("expected unresolved canonical via alias, got {other:?}"),
    }
}

#[test]
fn resolve_call_dotted_name_routes_through_alias_prefix() {
    // Bug C narrow: `vscode.Disposable` where `vscode` is a namespace
    // import alias resolves to `<aliased_target>::Disposable`, not the
    // bogus module-local fallback.
    let (cm, _module, comments) = parse_ts("");
    let mut ctx = TsWalkContext::new(
        "src/index.ts".into(),
        "@app".into(),
        "src".into(),
        cm,
        PathBuf::new(),
        PathBuf::new(),
        None,
        &comments,
    );
    ctx.add_import_alias(
        "vscode".into(),
        ResolvedImport {
            target: ResolvedOrUnresolved::Unresolved {
                name: "vscode".into(),
            },
        },
    );
    // Single-dot suffix
    let t1 = expect_emit(ctx.resolve_call("vscode.Disposable", "src"));
    match t1.to {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "vscode::Disposable"),
        other => panic!("expected unresolved vscode::Disposable, got {other:?}"),
    }
    // Multi-segment suffix: dots are replaced with `::`
    let t2 = expect_emit(ctx.resolve_call("vscode.commands.executeCommand", "src"));
    match t2.to {
        ResolvedOrUnresolved::Unresolved { name } => {
            assert_eq!(name, "vscode::commands::executeCommand");
        }
        other => panic!("expected unresolved vscode::commands::executeCommand, got {other:?}"),
    }
    // Head not an alias → unchanged module-local fallback
    let t3 = expect_emit(ctx.resolve_call("local.foo", "src"));
    match t3.to {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "src::local.foo"),
        other => panic!("expected module-local fallback, got {other:?}"),
    }
}

#[test]
fn resolve_call_module_local_resolved_when_defined() {
    let (cm, _module, comments) = parse_ts("");
    let mut ctx = TsWalkContext::new(
        "src/index.ts".into(),
        "@app".into(),
        "src".into(),
        cm,
        PathBuf::new(),
        PathBuf::new(),
        None,
        &comments,
    );
    ctx.core.defined_fqdns.insert("src::foo".into());
    let target = expect_emit(ctx.resolve_call("foo", "src"));
    assert!(
        target.via_builtin.is_none(),
        "module-local resolution carries no via_builtin"
    );
    match target.to {
        ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "src::foo"),
        other => panic!("expected resolved, got {other:?}"),
    }
}

#[test]
fn resolve_call_falls_back_to_unresolved_module_local() {
    let (cm, _module, comments) = parse_ts("");
    let ctx = TsWalkContext::new(
        "src/index.ts".into(),
        "@app".into(),
        "src".into(),
        cm,
        PathBuf::new(),
        PathBuf::new(),
        None,
        &comments,
    );
    let target = expect_emit(ctx.resolve_call("nope", "src"));
    assert!(
        target.via_builtin.is_none(),
        "fall-through unresolved carries no via_builtin"
    );
    match target.to {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "src::nope"),
        other => panic!("expected unresolved module-local, got {other:?}"),
    }
}

#[test]
fn stage3e1_resolve_call_builtin_edge_emits_synthetic_with_tag() {
    // `Math` is registered in the TS builtins as `BuiltinTier::Edge`
    // with tag `BuiltinTag::Math` — resolution should produce a
    // synthetic FQDN AND propagate the tag for downstream attribute
    // stamping (`via-builtin` + `builtin-math`).
    let (cm, _module, comments) = parse_ts("");
    let ctx = TsWalkContext::new(
        "src/index.ts".into(),
        "@app".into(),
        "src".into(),
        cm,
        PathBuf::new(),
        PathBuf::new(),
        None,
        &comments,
    );
    let target = expect_emit(ctx.resolve_call("Math", "src"));
    match &target.to {
        ResolvedOrUnresolved::Resolved { fqdn } => {
            assert!(
                fqdn.starts_with("<builtin>::"),
                "expected synthetic FQDN, got {fqdn}"
            );
            assert!(
                fqdn.ends_with("::Math"),
                "expected synthetic to end with ::Math, got {fqdn}"
            );
        }
        other => panic!("expected Resolved synthetic for Edge tier, got {other:?}"),
    }
    match target.via_builtin {
        Some(BuiltinTag::Math) => {}
        other => panic!("expected via_builtin = Some(Math), got {other:?}"),
    }
}

#[test]
fn stage3e1_resolve_call_builtin_drop_returns_none() {
    // `Array` is registered as `BuiltinTier::Drop` in the TS builtins —
    // structural container noise, no edge should be emitted. Resolver
    // returns `None` so callers can skip without further inspection.
    let (cm, _module, comments) = parse_ts("");
    let ctx = TsWalkContext::new(
        "src/index.ts".into(),
        "@app".into(),
        "src".into(),
        cm,
        PathBuf::new(),
        PathBuf::new(),
        None,
        &comments,
    );
    assert!(
        matches!(ctx.resolve_call("Array", "src"), ResolutionOutcome::Drop),
        "Drop-tier builtin must resolve to ResolutionOutcome::Drop"
    );
}

#[test]
fn stage3e1b_resolve_call_attribute_tier_returns_tag() {
    // `Promise` is `BuiltinTier::Attribute` (Async tag). Stage 3e-1b
    // surfaces the tag through `ResolutionOutcome::Attribute(tag)`
    // so callers can flag the enclosing source symbol.
    let (cm, _module, comments) = parse_ts("");
    let ctx = TsWalkContext::new(
        "src/index.ts".into(),
        "@app".into(),
        "src".into(),
        cm,
        PathBuf::new(),
        PathBuf::new(),
        None,
        &comments,
    );
    match ctx.resolve_call("Promise", "src") {
        ResolutionOutcome::Attribute(BuiltinTag::Async) => {}
        other => panic!("expected Attribute(Async) for Promise, got {other:?}"),
    }
}

#[test]
fn stage3e1b_register_attribute_flag_flushes_to_symbol() {
    // End-to-end : register a flag against an FQDN, push a matching
    // symbol, drain via `into_outputs`, assert the symbol picks the
    // flag up.
    let (cm, _module, comments) = parse_ts("");
    let mut ctx = TsWalkContext::new(
        "src/index.ts".into(),
        "@app".into(),
        "src".into(),
        cm,
        PathBuf::new(),
        PathBuf::new(),
        None,
        &comments,
    );
    ctx.push_symbol(RawSymbol {
        decl_kind: None,
        implements_trait: None,
        receiver_type: None,
        entry_point: None,
        name: "doStuff".into(),
        fqdn: "src::doStuff".into(),
        kind: Kind::Callable,
        language_kind: LanguageKind::from("function"),
        module: Some("src".into()),
        visibility: Visibility::Public,
        location: SymbolLocation {
            file: "src/index.ts".into(),
            start_line: 1,
            end_line: 5,
            start_col: 0,
            end_col: 1,
        },
        signature: None,
        body_hash: None,
        attributes: vec![],
        flags: vec![],
    });
    ctx.register_attribute_flag("src::doStuff", &BuiltinTag::Async);
    ctx.register_attribute_flag("src::doStuff", &BuiltinTag::Iter);
    // Duplicate register is a no-op (HashSet dedup).
    ctx.register_attribute_flag("src::doStuff", &BuiltinTag::Async);

    let (symbols, _, _, _) = ctx.into_outputs();
    let s = symbols
        .into_iter()
        .find(|s| s.fqdn == "src::doStuff")
        .expect("symbol must be retained");
    assert!(s.flags.contains(&"async".to_string()), "got {:?}", s.flags);
    assert!(s.flags.contains(&"iter".to_string()), "got {:?}", s.flags);
    assert_eq!(s.flags.len(), 2, "dedup must yield exactly 2 flags");
}

#[test]
fn stage3e1_resolve_call_alias_overrides_builtin() {
    // An explicit import alias shadows a global builtin name. The
    // alias path wins (carries no via_builtin) — guards against the
    // tier dispatch accidentally taking precedence over user code
    // that locally rebinds `Promise`, `Math`, etc.
    let (cm, _module, comments) = parse_ts("");
    let mut ctx = TsWalkContext::new(
        "src/index.ts".into(),
        "@app".into(),
        "src".into(),
        cm,
        PathBuf::new(),
        PathBuf::new(),
        None,
        &comments,
    );
    ctx.add_import_alias(
        "Promise".into(),
        ResolvedImport {
            target: ResolvedOrUnresolved::Resolved {
                fqdn: "@app::src::polyfill::Promise".into(),
            },
        },
    );
    let target = expect_emit(ctx.resolve_call("Promise", "src"));
    assert!(
        target.via_builtin.is_none(),
        "alias-shadowed Promise is NOT a builtin"
    );
    match target.to {
        ResolvedOrUnresolved::Resolved { fqdn } => {
            assert_eq!(fqdn, "@app::src::polyfill::Promise");
        }
        other => panic!("expected aliased Resolved, got {other:?}"),
    }
}

#[test]
fn _bytepos_unused() {
    // Touch BytePos to keep the import warning-free if test setup grows.
    let _ = BytePos(0);
    let _ = Span::default().lo();
}

fn decl_kind_of(symbols: &[RawSymbol], fqdn: &str) -> Option<DeclKind> {
    symbols
        .iter()
        .find(|s| s.fqdn == fqdn)
        .unwrap_or_else(|| panic!("symbol {fqdn} not found in {symbols:?}"))
        .decl_kind
        .clone()
}

#[test]
fn decl_kind_function_for_function_decl() {
    let (symbols, _, _, _) = run("export function foo() {}");
    assert_eq!(decl_kind_of(&symbols, "src::foo"), Some(DeclKind::Function));
}

#[test]
fn decl_kind_class_with_constructor_method_field() {
    let (symbols, _, _, _) = run("export class C { \
               x: number = 1; \
               constructor() {} \
               run() {} \
             }");
    assert_eq!(decl_kind_of(&symbols, "src::C"), Some(DeclKind::Class));
    assert_eq!(
        decl_kind_of(&symbols, "src::C::constructor"),
        Some(DeclKind::Constructor),
    );
    assert_eq!(
        decl_kind_of(&symbols, "src::C::run"),
        Some(DeclKind::Method)
    );
    assert_eq!(decl_kind_of(&symbols, "src::C::x"), Some(DeclKind::Field));
}

#[test]
fn decl_kind_class_getter_and_setter() {
    let (symbols, _, _, _) = run("export class C { \
               get value() { return 1; } \
               set value(v: number) {} \
             }");
    // Getter + setter share the FQDN; just check at least one Getter/Setter exists.
    let getters = symbols
        .iter()
        .filter(|s| s.decl_kind == Some(DeclKind::Getter))
        .count();
    let setters = symbols
        .iter()
        .filter(|s| s.decl_kind == Some(DeclKind::Setter))
        .count();
    assert_eq!(getters, 1, "want one Getter, got {symbols:?}");
    assert_eq!(setters, 1, "want one Setter, got {symbols:?}");
}

#[test]
fn decl_kind_interface_with_members() {
    let (symbols, _, _, _) = run("export interface I { \
               x: number; \
               run(): void; \
               get y(): number; \
               set y(v: number); \
             }");
    assert_eq!(decl_kind_of(&symbols, "src::I"), Some(DeclKind::Interface));
    assert_eq!(decl_kind_of(&symbols, "src::I::x"), Some(DeclKind::Field));
    assert_eq!(
        decl_kind_of(&symbols, "src::I::run"),
        Some(DeclKind::Method)
    );
    let getters = symbols
        .iter()
        .filter(|s| s.decl_kind == Some(DeclKind::Getter))
        .count();
    let setters = symbols
        .iter()
        .filter(|s| s.decl_kind == Some(DeclKind::Setter))
        .count();
    assert_eq!(getters, 1, "want one interface Getter, got {symbols:?}");
    assert_eq!(setters, 1, "want one interface Setter, got {symbols:?}");
}

#[test]
fn decl_kind_enum_with_variants() {
    let (symbols, _, _, _) = run("export enum E { A, B }");
    assert_eq!(decl_kind_of(&symbols, "src::E"), Some(DeclKind::Enum));
    assert_eq!(
        decl_kind_of(&symbols, "src::E::A"),
        Some(DeclKind::EnumVariant),
    );
    assert_eq!(
        decl_kind_of(&symbols, "src::E::B"),
        Some(DeclKind::EnumVariant),
    );
}

#[test]
fn decl_kind_type_alias() {
    let (symbols, _, _, _) = run("export type Id = string;");
    assert_eq!(decl_kind_of(&symbols, "src::Id"), Some(DeclKind::TypeAlias),);
}

#[test]
fn decl_kind_var_const_vs_let() {
    let (symbols, _, _, _) = run("const PI = 3; let counter = 0; var legacy = 1;");
    assert_eq!(decl_kind_of(&symbols, "src::PI"), Some(DeclKind::Const));
    assert_eq!(decl_kind_of(&symbols, "src::counter"), Some(DeclKind::Var));
    assert_eq!(decl_kind_of(&symbols, "src::legacy"), Some(DeclKind::Var));
}

#[test]
fn decl_kind_arrow_function_const_is_function() {
    let (symbols, _, _, _) = run("export const greet = (name: string) => name;");
    assert_eq!(
        decl_kind_of(&symbols, "src::greet"),
        Some(DeclKind::Function),
    );
}

#[test]
fn entry_point_main_at_module_root_tagged_binary_main() {
    let (symbols, _, _, _) = run("export function main() {}");
    let m = symbols.iter().find(|s| s.fqdn == "src::main").unwrap();
    assert_eq!(m.entry_point, Some(EntryPointKind::BinaryMain));
}

#[test]
fn entry_point_default_export_main_tagged_binary_main() {
    let (symbols, _, _, _) = run("export default function main() {}");
    let m = symbols.iter().find(|s| s.fqdn == "src::main").unwrap();
    assert_eq!(m.entry_point, Some(EntryPointKind::BinaryMain));
}

#[test]
fn entry_point_non_main_function_is_none() {
    let (symbols, _, _, _) = run("export function helper() {}");
    let h = symbols.iter().find(|s| s.fqdn == "src::helper").unwrap();
    assert_eq!(h.entry_point, None);
}

#[test]
fn entry_point_main_helper_inside_class_not_tagged() {
    // Class method named `main` is parent_fqdn = `src::Runner`
    // (1 `::`, still under the heuristic cap) BUT methods don't
    // route through `extract_fn_decl` — they hit `extract_method`,
    // which still carries `entry_point: None` by construction.
    // Guards against a future drift where the method path picks
    // up the helper and starts tagging class methods named main.
    let (symbols, _, _, _) = run("class Runner { main() {} }");
    let m = symbols
        .iter()
        .find(|s| s.fqdn == "src::Runner::main")
        .unwrap();
    assert_eq!(m.entry_point, None);
}

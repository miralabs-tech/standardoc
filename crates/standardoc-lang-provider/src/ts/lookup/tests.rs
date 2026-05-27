use super::*;
use swc_core::common::{FileName, SourceMap, sync::Lrc};
use swc_core::ecma::parser::{Parser, StringInput, Syntax, TsSyntax};

fn parse(src: &str) -> Module {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(
        Lrc::new(FileName::Custom("test.ts".into())),
        src.to_string(),
    );
    let mut parser = Parser::new(
        Syntax::Typescript(TsSyntax {
            tsx: false,
            ..Default::default()
        }),
        StringInput::from(&*fm),
        None,
    );
    parser.parse_module().expect("parse ok")
}

#[test]
fn module_lookup_carries_module_fqdn_and_language() {
    let module = parse("export const x = 1;\n");
    let lookup = build_ts_lookup(&module, "pkg::src::index");
    assert_eq!(lookup.module_fqdn, "pkg::src::index");
    assert_eq!(lookup.language, Language::TypeScript);
}

#[test]
fn top_level_function_and_class_hoisted_to_root() {
    let module = parse("function f() {}\nclass C {}\n");
    let lookup = build_ts_lookup(&module, "m");
    let f = lookup
        .bindings
        .get("f")
        .and_then(|v| v.first())
        .expect("f binding");
    assert_eq!(f.scope_idx, ModuleLookup::ROOT_SCOPE);
    assert!(matches!(
        f.source,
        BindingSource::LocalDecl {
            decl_kind: LocalDeclKind::Function
        }
    ));
    let c = lookup
        .bindings
        .get("C")
        .and_then(|v| v.first())
        .expect("C binding");
    assert_eq!(c.scope_idx, ModuleLookup::ROOT_SCOPE);
    assert!(matches!(
        c.source,
        BindingSource::LocalDecl {
            decl_kind: LocalDeclKind::Class
        }
    ));
}

#[test]
fn imports_populate_both_bindings_and_import_records() {
    let src = r#"import { foo, bar as baz } from "other";
import type { T } from "other";
import def from "default-pkg";
import * as ns from "ns-pkg";
"#;
    let module = parse(src);
    let lookup = build_ts_lookup(&module, "m");

    assert!(lookup.bindings.contains_key("foo"));
    assert!(lookup.bindings.contains_key("baz"));
    assert!(lookup.bindings.contains_key("T"));
    assert!(lookup.bindings.contains_key("def"));
    assert!(lookup.bindings.contains_key("ns"));

    // foo: regular import, not type-only
    let foo = lookup.bindings.get("foo").and_then(|v| v.first()).unwrap();
    match &foo.source {
        BindingSource::Import {
            module_path,
            is_type_only,
            ..
        } => {
            assert_eq!(module_path, "other");
            assert!(!is_type_only);
        }
        other => panic!("expected Import, got {other:?}"),
    }

    // T: type-only import
    let t = lookup.bindings.get("T").and_then(|v| v.first()).unwrap();
    match &t.source {
        BindingSource::Import { is_type_only, .. } => {
            assert!(is_type_only);
        }
        other => panic!("expected Import, got {other:?}"),
    }
    assert!(t.attributes.iter().any(|a| a == "type-only"));

    // baz: aliased — original_name should be `bar`
    let baz = lookup.bindings.get("baz").and_then(|v| v.first()).unwrap();
    match &baz.source {
        BindingSource::Import { original_name, .. } => {
            assert_eq!(original_name.as_deref(), Some("bar"));
        }
        other => panic!("expected Import, got {other:?}"),
    }

    // 4 import records (foo, baz, T, def, ns)  =  5 entries flat
    assert_eq!(lookup.imports.len(), 5);
}

#[test]
fn const_alias_captures_leftmost_base() {
    let module = parse("const x = FOO;\nconst y = obj.a.b;\nlet z = mut_target;\n");
    let lookup = build_ts_lookup(&module, "m");

    let x = lookup.bindings.get("x").and_then(|v| v.first()).unwrap();
    assert_eq!(x.aliases_to.as_deref(), Some("FOO"));
    assert_eq!(x.mutability, Some(AliasMutability::Const));

    let y = lookup.bindings.get("y").and_then(|v| v.first()).unwrap();
    assert_eq!(y.aliases_to.as_deref(), Some("obj"));

    let z = lookup.bindings.get("z").and_then(|v| v.first()).unwrap();
    assert_eq!(z.aliases_to.as_deref(), Some("mut_target"));
    assert_eq!(z.mutability, Some(AliasMutability::Mutable));
}

#[test]
fn function_body_var_lands_in_function_scope_not_root() {
    let module = parse("function f() { const inner = 1; }\n");
    let lookup = build_ts_lookup(&module, "m");
    let inner = lookup
        .bindings
        .get("inner")
        .and_then(|v| v.first())
        .expect("inner binding");
    assert_ne!(inner.scope_idx, ModuleLookup::ROOT_SCOPE);
    // Walk parent chain — must reach ROOT.
    let mut cursor = inner.scope_idx;
    while let Some(parent) = lookup.scopes[cursor as usize].parent {
        cursor = parent;
    }
    assert_eq!(cursor, ModuleLookup::ROOT_SCOPE);
}

#[test]
fn type_param_bound_in_function_scope() {
    let module = parse("function f<T>(x: T): T { return x; }\n");
    let lookup = build_ts_lookup(&module, "m");
    let t = lookup
        .bindings
        .get("T")
        .and_then(|v| v.first())
        .expect("T binding");
    assert!(matches!(t.source, BindingSource::TypeParam));
    assert_ne!(t.scope_idx, ModuleLookup::ROOT_SCOPE);
}

#[test]
fn enum_members_bound_inside_enum_scope() {
    let module = parse("enum Color { Red, Green }\n");
    let lookup = build_ts_lookup(&module, "m");
    // Color binding at root.
    assert!(lookup.bindings.contains_key("Color"));
    // Red + Green inside the enum's TypeContainer scope.
    let red = lookup
        .bindings
        .get("Red")
        .and_then(|v| v.first())
        .expect("Red binding");
    assert_ne!(red.scope_idx, ModuleLookup::ROOT_SCOPE);
    assert!(red.attributes.iter().any(|a| a == "enum-member"));
}

#[test]
fn destructuring_tags_attribute_and_still_binds_idents() {
    let module = parse("const { a, b: c } = obj;\nconst [x, y] = arr;\n");
    let lookup = build_ts_lookup(&module, "m");
    for n in ["a", "c", "x", "y"] {
        let b = lookup
            .bindings
            .get(n)
            .and_then(|v| v.first())
            .unwrap_or_else(|| panic!("{n} binding missing"));
        assert!(
            b.attributes.iter().any(|a| a == "unhandled-destructuring"),
            "{n} should carry unhandled-destructuring"
        );
    }
}

#[test]
fn resolve_local_walks_chain_to_root_binding() {
    let module = parse("const outer = 1;\nfunction f() { const inner = outer; }\n");
    let lookup = build_ts_lookup(&module, "m");
    let inner_scope = lookup
        .bindings
        .get("inner")
        .and_then(|v| v.first())
        .unwrap()
        .scope_idx;
    // `outer` is reachable from inside `f`'s scope via parent chain.
    let outer = lookup
        .resolve_local("outer", inner_scope)
        .expect("outer reachable via parent");
    assert_eq!(outer.scope_idx, ModuleLookup::ROOT_SCOPE);
}

// --- Constructor param binding (IR-4-c follow-up) ----------------------

#[test]
fn constructor_plain_param_bound_in_function_scope() {
    // Plain `Param` (not `TsParamProp`) inside a constructor must
    // land in the constructor's Function scope, just like a normal
    // fn param — proves the `ParamOrTsParamProp::Param` branch of
    // `visit_constructor` routes through `bind_pat` correctly.
    let module = parse("class Foo { constructor(x: number) {} }\n");
    let lookup = build_ts_lookup(&module, "m");
    let x = lookup
        .bindings
        .get("x")
        .and_then(|v| v.first())
        .expect("x binding");
    assert!(matches!(x.source, BindingSource::Param));
    assert_ne!(x.scope_idx, ModuleLookup::ROOT_SCOPE);
    assert!(
        !x.attributes.iter().any(|a| a == "param-property"),
        "plain Param must NOT carry param-property"
    );
}

#[test]
fn constructor_ts_param_prop_ident_seeds_binding_with_attribute() {
    // `constructor(private readonly db: Db)` shorthand — `db` is a
    // param-property: SWC encodes it as TsParamProp(Ident). The
    // lookup builder must seed `db` as a Param binding so body refs
    // resolve, AND tag it `param-property` so consumers can recognise
    // the implicit `this.db = db` assignment.
    let module = parse("class Foo { constructor(private readonly db: Db) {} }\n");
    let lookup = build_ts_lookup(&module, "m");
    let db = lookup
        .bindings
        .get("db")
        .and_then(|v| v.first())
        .expect("db binding");
    assert!(matches!(db.source, BindingSource::Param));
    assert_ne!(db.scope_idx, ModuleLookup::ROOT_SCOPE);
    assert!(
        db.attributes.iter().any(|a| a == "param-property"),
        "TsParamProp must carry param-property attribute"
    );
}

#[test]
fn constructor_ts_param_prop_with_default_value_binds_via_assign_pat() {
    // `constructor(public x = 0)` — TsParamProp(Assign) variant.
    // SWC encodes the LHS as a `Pat` (here `Pat::Ident("x")`) inside
    // an AssignPat. The lookup builder must route through `bind_pat`
    // to seed the underlying ident.
    let module = parse("class Foo { constructor(public x = 0) {} }\n");
    let lookup = build_ts_lookup(&module, "m");
    let x = lookup
        .bindings
        .get("x")
        .and_then(|v| v.first())
        .expect("x binding");
    assert!(matches!(x.source, BindingSource::Param));
    assert_ne!(x.scope_idx, ModuleLookup::ROOT_SCOPE);
    assert!(
        x.attributes.iter().any(|a| a == "param-property"),
        "TsParamProp(Assign) must carry param-property attribute"
    );
}

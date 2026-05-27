use super::*;

fn parse(src: &str) -> syn::File {
    syn::parse_file(src).expect("test source not parsable")
}

#[test]
fn walks_simple_fn_emits_function_symbol() {
    let parsed = parse("fn foo() {}");
    let (symbols, edges, _docs, _) = walk(&parsed, "mycrate", "src/lib.rs", "mycrate");
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].kind, Kind::Callable);
    assert_eq!(symbols[0].fqdn, "mycrate::foo");
    assert_eq!(symbols[0].name, "foo");
    assert_eq!(symbols[0].visibility, Visibility::Private);
    assert!(edges.is_empty());
}

#[test]
fn pub_fn_visibility_is_public() {
    let parsed = parse("pub fn foo() {}");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert_eq!(symbols[0].visibility, Visibility::Public);
}

#[test]
fn fn_signature_captures_params_and_return() {
    let parsed = parse("pub fn add(a: u32, b: u32) -> u32 { a + b }");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let sig = symbols[0].signature.as_ref().unwrap();
    assert_eq!(sig.params.len(), 2);
    assert_eq!(sig.params[0].name, "a");
    assert_eq!(sig.params[0].ty.display, "u32");
    assert_eq!(sig.params[1].name, "b");
    assert_eq!(sig.returns.as_ref().unwrap().display, "u32");
}

#[test]
fn async_fn_modifier_set() {
    let parsed = parse("async fn boot() {}");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert!(symbols[0].signature.as_ref().unwrap().modifiers.is_async);
}

#[test]
fn deprecated_attribute_propagates_to_modifier() {
    let parsed = parse("#[deprecated = \"use bar\"] fn foo() {}");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let dep = symbols[0]
        .signature
        .as_ref()
        .unwrap()
        .modifiers
        .deprecated
        .as_deref();
    assert_eq!(dep, Some("\"use bar\""));
}

#[test]
fn self_receiver_renders_as_self_typeref() {
    let parsed = parse("impl Foo {\n  fn a(self) {}\n  fn b(&self) {}\n  fn c(&mut self) {}\n}");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let a = &symbols.iter().find(|s| s.name == "a").unwrap().signature;
    let b = &symbols.iter().find(|s| s.name == "b").unwrap().signature;
    let c = &symbols.iter().find(|s| s.name == "c").unwrap().signature;
    assert_eq!(a.as_ref().unwrap().params[0].ty.display, "Self");
    assert_eq!(b.as_ref().unwrap().params[0].ty.display, "&Self");
    assert_eq!(c.as_ref().unwrap().params[0].ty.display, "&mut Self");
}

#[test]
fn struct_emits_type_symbol_and_field_sub_symbols() {
    // Bug C-2: a struct now pushes the parent type symbol AND one
    // Value-kind sub-symbol per named field.
    let parsed = parse("pub struct Foo { x: u32 }");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let foo = symbols.iter().find(|s| s.fqdn == "c::Foo").unwrap();
    assert_eq!(foo.kind, Kind::Type);
    assert_eq!(foo.language_kind.as_str(), "struct");
    let field = symbols.iter().find(|s| s.fqdn == "c::Foo::x").unwrap();
    assert_eq!(field.kind, Kind::Value);
    assert_eq!(field.language_kind.as_str(), "field");
    assert_eq!(field.module.as_deref(), Some("c::Foo"));
    // Type captured on signature.returns as a TypeRef.
    assert_eq!(
        field
            .signature
            .as_ref()
            .unwrap()
            .returns
            .as_ref()
            .unwrap()
            .display,
        "u32",
    );
}

#[test]
fn tuple_struct_emits_positional_field_sub_symbols() {
    let parsed = parse("pub struct Pair(pub u32, pub String);");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let f0 = symbols.iter().find(|s| s.fqdn == "c::Pair::0").unwrap();
    let f1 = symbols.iter().find(|s| s.fqdn == "c::Pair::1").unwrap();
    assert_eq!(f0.language_kind.as_str(), "tuple_field");
    assert_eq!(f1.language_kind.as_str(), "tuple_field");
    assert_eq!(
        f0.signature
            .as_ref()
            .unwrap()
            .returns
            .as_ref()
            .unwrap()
            .display,
        "u32",
    );
    assert_eq!(
        f1.signature
            .as_ref()
            .unwrap()
            .returns
            .as_ref()
            .unwrap()
            .display,
        "String",
    );
}

#[test]
fn enum_emits_type_symbol_and_variant_sub_symbols() {
    // Bug C-2: enum pushes the parent type symbol AND one Type-kind
    // sub-symbol per variant.
    let parsed = parse("enum E { A, B }");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let e = symbols.iter().find(|s| s.fqdn == "c::E").unwrap();
    assert_eq!(e.kind, Kind::Type);
    assert_eq!(e.language_kind.as_str(), "enum");
    let a = symbols.iter().find(|s| s.fqdn == "c::E::A").unwrap();
    let b = symbols.iter().find(|s| s.fqdn == "c::E::B").unwrap();
    assert_eq!(a.language_kind.as_str(), "enum_variant");
    assert_eq!(a.module.as_deref(), Some("c::E"));
    assert_eq!(b.language_kind.as_str(), "enum_variant");
}

#[test]
fn unit_struct_emits_only_parent_symbol() {
    let parsed = parse("pub struct Marker;");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let marker = symbols.iter().find(|s| s.fqdn == "c::Marker").unwrap();
    assert_eq!(marker.language_kind.as_str(), "struct");
    // No sub-fields for a unit struct.
    let children: Vec<_> = symbols
        .iter()
        .filter(|s| s.module.as_deref() == Some("c::Marker"))
        .collect();
    assert!(
        children.is_empty(),
        "expected no sub-symbols for unit struct, got {children:?}",
    );
}

#[test]
fn trait_emits_type_and_inner_fn_symbols() {
    let parsed = parse("pub trait T { fn foo(&self); fn bar(&self) -> u32 { 0 } }");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert_eq!(symbols.len(), 3);
    assert_eq!(symbols[0].kind, Kind::Type);
    assert_eq!(symbols[0].language_kind.as_str(), "trait");
    assert_eq!(symbols[0].fqdn, "c::T");
    assert_eq!(symbols[1].kind, Kind::Callable);
    assert_eq!(symbols[1].fqdn, "c::T::foo");
    assert_eq!(symbols[1].language_kind.as_str(), "trait_fn");
    assert_eq!(symbols[1].visibility, Visibility::Public);
    assert_eq!(symbols[2].fqdn, "c::T::bar");
}

#[test]
fn inherent_impl_emits_method_symbols() {
    let parsed = parse("struct Foo; impl Foo { pub fn a(&self) {} fn b(&self) {} }");
    let (symbols, edges, _docs, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert!(edges.is_empty(), "no IMPLEMENTS for inherent impl");
    let foo = symbols.iter().find(|s| s.fqdn == "c::Foo").unwrap();
    assert_eq!(foo.kind, Kind::Type);
    let a = symbols.iter().find(|s| s.fqdn == "c::Foo::a").unwrap();
    assert_eq!(a.visibility, Visibility::Public);
    let b = symbols.iter().find(|s| s.fqdn == "c::Foo::b").unwrap();
    assert_eq!(b.visibility, Visibility::Private);
}

#[test]
fn trait_impl_emits_implements_edge() {
    let parsed = parse("struct Foo; impl SomeTrait for Foo { fn run(&self) {} }");
    let (symbols, edges, _docs, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let imp: Vec<_> = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Implements)
        .collect();
    assert_eq!(imp.len(), 1);
    assert_eq!(imp[0].from_fqdn, "c::Foo");
    // No alias/local match → fallback to module-local canonical "c::SomeTrait".
    match &imp[0].to {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "c::SomeTrait"),
        other => panic!("expected unresolved, got {other:?}"),
    }
    assert!(symbols.iter().any(|s| s.fqdn == "c::Foo::run"));
}

#[test]
fn impl_builtin_drop_tier_trait_emits_no_implements_edge() {
    // `impl Drop for Foo` (and Default, Clone, From, ...): pre-fix
    // emitted a bogus `c::Drop` Unresolved IMPLEMENTS target via
    // resolve_path's module-local fallback. Now the builtin
    // registry catches it (tier::Drop) and we skip the edge,
    // mirroring the value-position policy.
    let parsed = parse("struct Foo; impl Drop for Foo { fn drop(&mut self) {} }");
    let (_symbols, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let imp_count = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Implements)
        .count();
    assert_eq!(
        imp_count, 0,
        "tier::Drop builtin trait (Drop) must not emit an IMPLEMENTS edge"
    );
}

#[test]
fn impl_builtin_default_clone_emits_no_implements_edges() {
    let parsed = parse(
        "struct Foo;\n\
             impl Default for Foo { fn default() -> Self { Foo } }\n\
             impl Clone for Foo { fn clone(&self) -> Self { Foo } }",
    );
    let (_symbols, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let imp_count = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Implements)
        .count();
    assert_eq!(
        imp_count, 0,
        "Default + Clone are tier::Drop builtins, no IMPLEMENTS edges expected"
    );
}

#[test]
fn impl_builtin_error_tier_edge_emits_resolved_with_synthetic_fqdn() {
    // `Error` is tier::Edge — implementing it is a semantic
    // "this is an error type" signal worth keeping in the graph.
    // Expect a Resolved IMPLEMENTS target pointing at the
    // synthetic builtin fqdn + a `via-builtin` attribute.
    let parsed =
        parse("struct MyError; impl Error for MyError { fn description(&self) -> &str { \"\" } }");
    let (_symbols, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let imp: Vec<_> = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Implements)
        .collect();
    assert_eq!(imp.len(), 1, "Error tier::Edge must emit one IMPLEMENTS");
    assert_eq!(imp[0].from_fqdn, "c::MyError");
    match &imp[0].to {
        ResolvedOrUnresolved::Resolved { fqdn } => {
            assert!(
                fqdn.starts_with("<builtin>::"),
                "expected synthetic builtin fqdn, got {fqdn}"
            );
            assert!(
                fqdn.ends_with("::Error"),
                "expected ::Error tail, got {fqdn}"
            );
        }
        other => panic!("expected resolved synthetic fqdn, got {other:?}"),
    }
    assert!(imp[0].attributes.iter().any(|a| a == "via-builtin"));
}

#[test]
fn impl_block_on_non_nominal_self_type_emits_nothing() {
    // `impl<T> Iterator for &mut T` — self-type is a reference, not a
    // Path. Methods inside are accessed via trait dispatch; concating
    // `&mut T::method` produces garbage FQDNs.
    let parsed = parse(
        "impl<T> Iterator for &mut T { type Item = (); fn next(&mut self) -> Option<()> { None } }",
    );
    let (symbols, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");

    assert!(
        !symbols.iter().any(|s| s.fqdn.contains('&')),
        "no symbol should reference `&` in its fqdn, got {:?}",
        symbols.iter().map(|s| &s.fqdn).collect::<Vec<_>>()
    );
    let impls: Vec<_> = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Implements)
        .collect();
    assert!(
        impls.is_empty(),
        "impl on non-nominal self-type must emit no IMPLEMENTS edge"
    );
}

#[test]
fn impl_block_on_tuple_self_type_emits_nothing() {
    let parsed = parse("impl SomeTrait for (u32, u32) { fn run(&self) {} }");
    let (symbols, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert!(symbols.is_empty(), "tuple self-type must emit no symbol");
    assert!(edges.is_empty(), "tuple self-type must emit no edge");
}

#[test]
fn trait_impl_with_use_alias_resolves_implements_target() {
    let parsed = parse("use crate::traits::Foo; struct Bar; impl Foo for Bar { fn run(&self) {} }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let imp = edges
        .iter()
        .find(|e| e.kind == EdgeKind::Implements)
        .expect("implements edge");
    match &imp.to {
        ResolvedOrUnresolved::Unresolved { name } => {
            assert_eq!(name, "c::traits::Foo");
        }
        other => panic!("expected unresolved canonical, got {other:?}"),
    }
}

#[test]
fn const_emits_value_symbol() {
    let parsed = parse("const N: u32 = 0;");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert_eq!(symbols[0].kind, Kind::Value);
    assert_eq!(symbols[0].language_kind.as_str(), "const");
    assert_eq!(symbols[0].fqdn, "c::N");
}

#[test]
fn static_emits_value_symbol() {
    let parsed = parse("static GLOBAL: u32 = 0;");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert_eq!(symbols[0].kind, Kind::Value);
    assert_eq!(symbols[0].language_kind.as_str(), "static");
}

#[test]
fn type_alias_emits_type_symbol() {
    let parsed = parse("pub type Bytes = Vec<u8>;");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert_eq!(symbols[0].kind, Kind::Type);
    assert_eq!(symbols[0].language_kind.as_str(), "type_alias");
}

#[test]
fn macro_rules_with_export_is_public() {
    let parsed = parse("#[macro_export] macro_rules! say { () => {}; }");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].kind, Kind::Macro);
    assert_eq!(symbols[0].visibility, Visibility::Public);
}

#[test]
fn macro_rules_without_export_is_private() {
    let parsed = parse("macro_rules! say { () => {}; }");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert_eq!(symbols[0].visibility, Visibility::Private);
}

#[test]
fn inline_mod_pushes_fqdn_without_emitting_module_symbol() {
    let parsed = parse("mod inner { pub fn deep() {} }");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert_eq!(
        symbols.len(),
        1,
        "only the deep fn — no Module symbol for inline mod"
    );
    assert_eq!(symbols[0].kind, Kind::Callable);
    assert_eq!(symbols[0].fqdn, "c::inner::deep");
}

#[test]
fn attributes_are_captured_with_path_name() {
    let parsed = parse("#[derive(Debug, Clone)] pub struct X;");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert_eq!(symbols[0].attributes.len(), 1);
    assert_eq!(symbols[0].attributes[0].name, "derive");
    assert_eq!(symbols[0].attributes[0].args.len(), 1);
    assert_eq!(symbols[0].attributes[0].args[0].value, "Debug, Clone");
}

#[test]
fn generic_params_captured_as_strings() {
    let parsed = parse("fn id<T>(x: T) -> T { x }");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
fn where_clause_captured_as_text_without_leading_keyword() {
    let parsed = parse("fn foo<T>(x: T) where T: Send + Sync {}");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let wc = symbols[0]
        .signature
        .as_ref()
        .unwrap()
        .modifiers
        .where_clause
        .as_deref();
    assert!(wc.is_some(), "where clause must be captured");
    let text = wc.unwrap();
    assert!(
        !text.starts_with("where"),
        "leading `where` must be stripped: `{text}`"
    );
    assert!(text.contains("Send"));
    assert!(text.contains("Sync"));
}

#[test]
fn where_clause_is_none_when_absent() {
    let parsed = parse("fn bar() {}");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let wc = &symbols[0]
        .signature
        .as_ref()
        .unwrap()
        .modifiers
        .where_clause;
    assert!(wc.is_none());
}

#[test]
fn inline_generic_bounds_remain_in_generic_params() {
    let parsed = parse("fn foo<T: Display + Clone>(x: T) {}");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let g = &symbols[0]
        .signature
        .as_ref()
        .unwrap()
        .modifiers
        .generic_params;
    assert_eq!(g.len(), 1);
    assert!(g[0].contains("Display"), "got {g:?}");
    assert!(g[0].contains("Clone"), "got {g:?}");
}

#[test]
fn span_locations_are_captured() {
    // proc-macro2 with span-locations feature gives 1-based lines for parsed source.
    let parsed = parse("\n\nfn foo() {}\n");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert_eq!(symbols[0].location.start_line, 3);
}

#[test]
fn path_to_string_drops_whitespace_and_generics() {
    let p: syn::Path = syn::parse_str("crate::foo::Bar").unwrap();
    assert_eq!(path_to_string(&p), "crate::foo::Bar");
    let p2: syn::Path = syn::parse_str("Vec::<u8>::new").unwrap();
    assert_eq!(path_to_string(&p2), "Vec::new");
}

#[test]
fn canonicalize_crate_keyword_replaces_with_crate_name() {
    let mut ctx = WalkContext::new("src/lib.rs", "mycrate", "mycrate".to_string());
    ctx.alias_table.clear();
    assert_eq!(
        ctx.canonicalize("crate::foo::bar", "mycrate"),
        Some("mycrate::foo::bar".to_string())
    );
    assert_eq!(
        ctx.canonicalize("crate", "mycrate"),
        Some("mycrate".to_string())
    );
}

#[test]
fn canonicalize_self_resolves_to_current_module() {
    let ctx = WalkContext::new("src/foo.rs", "c", "c::foo".to_string());
    assert_eq!(
        ctx.canonicalize("self::bar", "c::foo"),
        Some("c::foo::bar".to_string())
    );
}

#[test]
fn canonicalize_super_pops_one_level() {
    let ctx = WalkContext::new("src/a/b.rs", "c", "c::a::b".to_string());
    assert_eq!(
        ctx.canonicalize("super::x", "c::a::b"),
        Some("c::a::x".to_string())
    );
    // No parent → None.
    assert_eq!(ctx.canonicalize("super::x", "c"), None);
}

#[test]
fn canonicalize_alias_then_remaining_segments() {
    let mut ctx = WalkContext::new("src/lib.rs", "c", "c".to_string());
    ctx.add_alias("HM".into(), "std::collections::HashMap".into());
    assert_eq!(
        ctx.canonicalize("HM::new", "c"),
        Some("std::collections::HashMap::new".to_string())
    );
}

#[test]
fn canonicalize_strict_returns_none_for_unaliased_single_ident() {
    let ctx = WalkContext::new("src/lib.rs", "c", "c::foo".to_string());
    // Strict mode: no module-local fallback. The fallback lives in resolve_path.
    assert_eq!(ctx.canonicalize("bar", "c::foo"), None);
}

#[test]
fn canonicalize_opaque_multi_segment_without_alias_returns_none() {
    let ctx = WalkContext::new("src/lib.rs", "c", "c".to_string());
    assert_eq!(ctx.canonicalize("std::mem::take", "c"), None);
}

#[test]
fn resolve_path_single_ident_falls_back_to_module_local() {
    let mut ctx = WalkContext::new("src/lib.rs", "c", "c::foo".to_string());
    ctx.core.defined_fqdns.insert("c::foo::bar".to_string());
    assert!(matches!(
        ctx.resolve_path("bar", "c::foo"),
        ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "c::foo::bar"
    ));
}

#[test]
fn resolve_path_multi_segment_without_alias_keeps_text_as_written() {
    let ctx = WalkContext::new("src/lib.rs", "c", "c".to_string());
    match ctx.resolve_path("std::mem::take", "c") {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "std::mem::take"),
        other => panic!("expected unresolved as-written, got {other:?}"),
    }
}

#[test]
fn resolve_path_returns_resolved_when_canonical_matches_defined_fqdn() {
    let mut ctx = WalkContext::new("src/lib.rs", "c", "c".to_string());
    ctx.core.defined_fqdns.insert("c::foo".to_string());
    assert!(matches!(
        ctx.resolve_path("self::foo", "c"),
        ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "c::foo"
    ));
}

#[test]
fn resolve_path_single_ident_walks_up_for_parent_fn_via_glob() {
    // `walk(...)` called from `c::walk::tests::test_fn` after
    // `mod tests { use super::*; ... }`. The glob doesn't enumerate
    // the parent fn `walk` into the test scope's bindings ; the
    // ancestor walk finds `c::walk::walk` at the file root.
    let mut ctx = WalkContext::new("src/walk.rs", "c", "c::walk".to_string());
    ctx.core.defined_fqdns.insert("c::walk::walk".into());
    match ctx.resolve_path("walk", "c::walk::tests") {
        ResolvedOrUnresolved::Resolved { fqdn } => {
            assert_eq!(fqdn, "c::walk::walk");
        }
        other => panic!("expected Resolved via ancestor walk, got {other:?}"),
    }
}

#[test]
fn resolve_path_single_ident_unresolved_keeps_module_local_name() {
    // Unknown single ident at any nested scope falls back to the
    // module-local unresolved name (preserves pre-fix shape).
    let ctx = WalkContext::new("src/lib.rs", "c", "c".to_string());
    match ctx.resolve_path("missing", "c::tests") {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "c::tests::missing"),
        other => panic!("expected module-local unresolved, got {other:?}"),
    }
}

#[test]
fn resolve_path_multi_segment_walks_up_to_file_root_for_local_type() {
    // Bug E-2 follow-up : `IndexHandle::open()` called inside a test
    // submodule of the same file (current_module = `c::handle::tests`)
    // must resolve through the file root where `struct IndexHandle`
    // lives. Without the ancestor walk, the path stays text-as-written.
    let mut ctx = WalkContext::new("src/handle.rs", "c", "c::handle".to_string());
    ctx.core
        .defined_fqdns
        .insert("c::handle::IndexHandle".into());
    ctx.core
        .defined_fqdns
        .insert("c::handle::IndexHandle::open".into());
    match ctx.resolve_path("IndexHandle::open", "c::handle::tests") {
        ResolvedOrUnresolved::Resolved { fqdn } => {
            assert_eq!(fqdn, "c::handle::IndexHandle::open");
        }
        other => panic!("expected Resolved via ancestor walk, got {other:?}"),
    }
}

#[test]
fn resolve_path_multi_segment_unresolved_with_canonical_when_method_not_defined() {
    // Same ancestor-walk path but the method itself isn't in
    // defined_fqdns (impl methods are emitted in p2 ; the lookup-time
    // resolve_path runs in p1/p2 boundary). Composing the canonical is
    // still a win — the pipeline edge-resolve step matches against
    // `symbols.fqdn` so the canonical can still tie to a real symbol_id.
    let mut ctx = WalkContext::new("src/handle.rs", "c", "c::handle".to_string());
    ctx.core
        .defined_fqdns
        .insert("c::handle::IndexHandle".into());
    match ctx.resolve_path("IndexHandle::open", "c::handle::tests::nested::fn") {
        ResolvedOrUnresolved::Unresolved { name } => {
            assert_eq!(name, "c::handle::IndexHandle::open");
        }
        other => panic!("expected unresolved-with-canonical, got {other:?}"),
    }
}

#[test]
fn resolve_path_multi_segment_stays_text_when_no_ancestor_defines_leftmost() {
    // External path like `std::mem::take` — no ancestor of the
    // current_module has a `std` symbol, so the walk falls through
    // to the text-as-written branch (same outcome as pre-fix).
    let ctx = WalkContext::new("src/lib.rs", "c", "c".to_string());
    match ctx.resolve_path("std::mem::take", "c::tests") {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "std::mem::take"),
        other => panic!("expected unresolved as-written, got {other:?}"),
    }
}

// --- Stage 3e-2 foundation: resolve_name tests ---

#[test]
fn stage3e2_resolve_name_empty_path_returns_drop() {
    let ctx = WalkContext::new("src/lib.rs", "c", "c".to_string());
    assert!(matches!(
        ctx.resolve_name("", ModuleLookup::ROOT_SCOPE, "c"),
        NameResolution::Drop
    ));
}

#[test]
fn stage3e2_resolve_name_module_local_resolved_when_defined() {
    let mut ctx = WalkContext::new("src/lib.rs", "c", "c".to_string());
    ctx.core.defined_fqdns.insert("c::bar".to_string());
    match ctx.resolve_name("bar", ModuleLookup::ROOT_SCOPE, "c") {
        NameResolution::Target {
            to: ResolvedOrUnresolved::Resolved { fqdn },
            alias_mut: None,
            via_builtin: None,
        } => assert_eq!(fqdn, "c::bar"),
        other => panic!("expected Target Resolved, got {other:?}"),
    }
}

#[test]
fn stage3e2_resolve_name_falls_back_to_unresolved_module_local() {
    let ctx = WalkContext::new("src/lib.rs", "c", "c".to_string());
    match ctx.resolve_name("missing", ModuleLookup::ROOT_SCOPE, "c") {
        NameResolution::Target {
            to: ResolvedOrUnresolved::Unresolved { name },
            ..
        } => assert_eq!(name, "c::missing"),
        other => panic!("expected Target Unresolved, got {other:?}"),
    }
}

#[test]
fn stage3e2_resolve_name_builtin_drop_tier_returns_drop() {
    // `Vec` is Drop-tier on Rust per Stage 3e-1 (structural noise).
    let ctx = WalkContext::new("src/lib.rs", "c", "c".to_string());
    assert!(matches!(
        ctx.resolve_name("Vec", ModuleLookup::ROOT_SCOPE, "c"),
        NameResolution::Drop
    ));
    // Multi-segment with Drop-tier leftmost also drops.
    assert!(matches!(
        ctx.resolve_name("Vec::new", ModuleLookup::ROOT_SCOPE, "c"),
        NameResolution::Drop
    ));
}

#[test]
fn stage3e2_resolve_name_builtin_attribute_tier_returns_attribute() {
    // `Iterator` is Attribute-tier on Rust per Stage 3e-1b (`iter` flag).
    let ctx = WalkContext::new("src/lib.rs", "c", "c".to_string());
    match ctx.resolve_name("Iterator", ModuleLookup::ROOT_SCOPE, "c") {
        NameResolution::Attribute(tag) => {
            assert_eq!(tag.slug(), "iter");
        }
        other => panic!("expected Attribute(iter), got {other:?}"),
    }
}

#[test]
fn stage3e2_resolve_name_via_alias_table_resolves_to_canonical() {
    let mut ctx = WalkContext::new("src/lib.rs", "c", "c".to_string());
    ctx.add_alias("HM".into(), "std::collections::HashMap".into());
    match ctx.resolve_name("HM", ModuleLookup::ROOT_SCOPE, "c") {
        NameResolution::Target {
            to: ResolvedOrUnresolved::Unresolved { name },
            alias_mut: None,
            via_builtin: None,
        } => assert_eq!(name, "std::collections::HashMap"),
        other => panic!("expected Target Unresolved canonical, got {other:?}"),
    }
}

// --- Bug C-3 tests: Rust UsesType emission ---

fn uses_type_edges(edges: &[RawEdge]) -> Vec<&RawEdge> {
    edges
        .iter()
        .filter(|e| e.kind == EdgeKind::UsesType)
        .collect()
}

fn uses_type_with<'a>(edges: &'a [RawEdge], attrs: &[&str]) -> Vec<&'a RawEdge> {
    edges
        .iter()
        .filter(|e| {
            e.kind == EdgeKind::UsesType
                && attrs.iter().all(|a| e.attributes.iter().any(|x| x == a))
        })
        .collect()
}

fn resolved_targets(edges: &[&RawEdge]) -> Vec<String> {
    edges
        .iter()
        .filter_map(|e| match &e.to {
            ResolvedOrUnresolved::Resolved { fqdn } => Some(fqdn.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn bug_c3_fn_param_type_emits_uses_type() {
    let parsed = parse("pub struct Foo; pub fn process(x: Foo) {}");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let refs = uses_type_with(&edges, &["via-type", "type-annotation"]);
    let targets = resolved_targets(&refs);
    assert!(
        targets.contains(&"c::Foo".to_string()),
        "expected UsesType edge to c::Foo, got {targets:?}",
    );
    assert!(
        refs.iter().any(|e| e.from_fqdn == "c::process"),
        "expected edge from c::process, got {refs:?}",
    );
}

#[test]
fn bug_c3_fn_return_type_emits_uses_type() {
    let parsed = parse("pub struct Bar; pub fn make() -> Bar { Bar }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let refs = uses_type_with(&edges, &["via-type", "type-annotation"]);
    let targets = resolved_targets(&refs);
    assert!(targets.contains(&"c::Bar".to_string()));
}

#[test]
fn bug_c3_struct_field_type_emits_uses_type_from_field_fqdn() {
    let parsed = parse("pub struct Foo; pub struct Bar { pub f: Foo }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let refs = uses_type_with(&edges, &["via-type", "type-annotation"]);
    // Per-field provenance: edge originates from c::Bar::f, not c::Bar.
    let from_field: Vec<&RawEdge> = refs
        .iter()
        .copied()
        .filter(|e| e.from_fqdn == "c::Bar::f")
        .collect();
    assert!(
        !from_field.is_empty(),
        "expected UsesType edge from c::Bar::f, got {refs:?}",
    );
    let targets = resolved_targets(&from_field);
    assert!(targets.contains(&"c::Foo".to_string()));
}

#[test]
fn bug_c3_generic_type_param_does_not_leak() {
    let parsed = parse("pub fn id<T>(x: T) -> T { x }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let refs = uses_type_edges(&edges);
    // `T` is fn-level generic → bound as local → no UsesType edge to c::T.
    let leaked: Vec<_> = refs
        .iter()
        .filter(|e| match &e.to {
            ResolvedOrUnresolved::Resolved { fqdn } => fqdn == "c::T",
            ResolvedOrUnresolved::Unresolved { name } => name == "c::T",
            _ => false,
        })
        .collect();
    assert!(
        leaked.is_empty(),
        "generic param T leaked as UsesType edge: {leaked:?}",
    );
}

#[test]
fn bug_c3_struct_generic_param_does_not_leak_in_fields() {
    let parsed = parse("pub struct Box2<T> { pub inner: T }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let refs = uses_type_edges(&edges);
    let leaked: Vec<_> = refs
        .iter()
        .filter(|e| match &e.to {
            ResolvedOrUnresolved::Resolved { fqdn } => fqdn == "c::T",
            ResolvedOrUnresolved::Unresolved { name } => name == "c::T",
            _ => false,
        })
        .collect();
    assert!(leaked.is_empty(), "struct-level T leaked: {leaked:?}");
}

#[test]
fn bug_c3_generic_constraint_emits_type_constraint() {
    let parsed = parse("pub trait Foo {} pub fn process<T: Foo>(x: T) -> T { x }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let refs = uses_type_with(&edges, &["via-type", "type-constraint"]);
    let targets = resolved_targets(&refs);
    assert!(
        targets.contains(&"c::Foo".to_string()),
        "expected UsesType/type-constraint edge to c::Foo, got {targets:?}",
    );
}

#[test]
fn bug_c3_where_clause_emits_type_constraint() {
    let parsed = parse("pub trait Foo {} pub fn process<T>(x: T) where T: Foo { let _ = x; }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let refs = uses_type_with(&edges, &["via-type", "type-constraint"]);
    let targets = resolved_targets(&refs);
    assert!(
        targets.contains(&"c::Foo".to_string()),
        "expected via-type/type-constraint via where-clause to c::Foo, got {targets:?}",
    );
}

#[test]
fn bug_c3_type_alias_body_emits_uses_type() {
    let parsed = parse("pub struct Foo; pub type X = Foo;");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let refs = uses_type_with(&edges, &["via-type", "type-alias-body"]);
    let targets = resolved_targets(&refs);
    assert!(
        targets.contains(&"c::Foo".to_string()),
        "expected UsesType/type-alias-body to c::Foo, got {targets:?}",
    );
    assert!(refs.iter().all(|e| e.from_fqdn == "c::X"));
}

#[test]
fn bug_c3_const_static_type_emits_uses_type() {
    let parsed = parse("pub struct Cfg; pub const K: Cfg = Cfg; pub static M: Cfg = Cfg;");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let refs = uses_type_with(&edges, &["via-type", "type-annotation"]);
    let const_edges: Vec<_> = refs.iter().filter(|e| e.from_fqdn == "c::K").collect();
    let static_edges: Vec<_> = refs.iter().filter(|e| e.from_fqdn == "c::M").collect();
    assert!(!const_edges.is_empty(), "expected const K → Cfg edge");
    assert!(!static_edges.is_empty(), "expected static M → Cfg edge");
}

#[test]
fn bug_c3_trait_supertrait_emits_type_extends() {
    let parsed = parse("pub trait Foo {} pub trait Bar: Foo {}");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let refs = uses_type_with(&edges, &["via-type", "type-extends"]);
    let targets = resolved_targets(&refs);
    assert!(
        targets.contains(&"c::Foo".to_string()),
        "expected UsesType/type-extends from Bar to Foo, got {targets:?}",
    );
}

#[test]
fn bug_c3_impl_trait_generic_arg_emits_type_implements() {
    let parsed = parse(
        "pub struct Foo; pub trait Iface<T> {} pub struct C; \
             impl Iface<Foo> for C {}",
    );
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let refs = uses_type_with(&edges, &["via-type", "type-implements"]);
    let targets = resolved_targets(&refs);
    assert!(
        targets.contains(&"c::Foo".to_string()),
        "expected UsesType/type-implements arg to c::Foo, got {targets:?}",
    );
}

#[test]
fn stage3e1_drop_tier_wrapper_skipped_inner_arg_still_emits() {
    // `Vec<Foo>` — Stage 3e-1: `Vec` is now classified as
    // `BuiltinTier::Drop` (structural noise) and produces no
    // `UsesType` edge. The inner `Foo` still emits normally — the
    // recursion through `visit_type_path` happens regardless of the
    // wrapper's tier decision.
    let parsed = parse("pub struct Foo; pub fn collect() -> Vec<Foo> { vec![] }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let refs = uses_type_with(&edges, &["via-type", "type-annotation"]);
    let targets = resolved_targets(&refs);
    assert!(
        !targets.iter().any(|t| t == "<builtin>::rust::Vec"),
        "Drop-tier Vec must not surface, got {targets:?}",
    );
    assert!(
        targets.contains(&"c::Foo".to_string()),
        "inner Foo must still emit, got {targets:?}",
    );
}

#[test]
fn stage3e1_uses_type_edge_tier_builtin_emits_with_attrs() {
    // `Error` is the lone `BuiltinTier::Edge` entry in the Rust
    // registry. A trait bound `T: Error` should produce a UsesType
    // edge to `<builtin>::rust::Error` carrying `via-builtin` plus
    // the `builtin-<tag>` slug — parity with TS Edge-tier emission.
    let parsed = parse("pub fn boom<T: Error>(e: T) {}");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let refs = uses_type_with(&edges, &["via-type", "via-builtin"]);
    let targets = resolved_targets(&refs);
    assert!(
        targets.iter().any(|t| t == "<builtin>::rust::Error"),
        "expected Edge-tier Error synthetic, got {targets:?}",
    );
    let has_tag_attr = refs.iter().any(|e| {
        e.attributes
            .iter()
            .any(|a| a.starts_with("builtin-") && a != "builtin-")
    });
    assert!(has_tag_attr, "expected builtin-<slug> attr on edge");
}

#[test]
fn stage3e1b_uses_type_attribute_tier_promotes_flag_on_source_symbol() {
    // `Iterator` is `BuiltinTier::Attribute` (`Iter` tag). Stage
    // 3e-1b flushes that into `flags = ["iter"]` on the enclosing
    // fn ; no edge surfaces (the property is a fact about the fn,
    // not a graph neighbor worth a node).
    let parsed = parse("pub fn collect<T: Iterator>(it: T) {}");
    let (symbols, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let refs = uses_type_with(&edges, &["via-type"]);
    let targets = resolved_targets(&refs);
    assert!(
        !targets.iter().any(|t| t.ends_with("::Iterator")),
        "Attribute-tier Iterator must not surface as an edge, got {targets:?}",
    );
    let collect_sym = symbols
        .iter()
        .find(|s| s.fqdn == "c::collect")
        .expect("c::collect must be indexed");
    assert!(
        collect_sym.flags.contains(&"iter".to_string()),
        "expected `iter` flag on c::collect, got {:?}",
        collect_sym.flags
    );
}

#[test]
fn stage3e1b_future_bound_promotes_async_flag() {
    // `Future` is `BuiltinTier::Attribute` (`Async` tag) — same
    // mechanism as Iterator but flagged as `"async"`.
    let parsed = parse("pub fn run<F: Future>(fut: F) {}");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let run_sym = symbols
        .iter()
        .find(|s| s.fqdn == "c::run")
        .expect("c::run must be indexed");
    assert!(
        run_sym.flags.contains(&"async".to_string()),
        "expected `async` flag on c::run, got {:?}",
        run_sym.flags
    );
}

#[test]
fn stage3e1b_attribute_flag_dedupes_across_multiple_hits() {
    // Same Attribute-tier trait touched twice in one fn signature
    // (param bound + return bound) must produce the flag exactly
    // once — `HashSet` dedup happens at the register-time site.
    let parsed = parse("pub fn pipe<I: Iterator>(i: I) -> impl Iterator { i }");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let pipe_sym = symbols
        .iter()
        .find(|s| s.fqdn == "c::pipe")
        .expect("c::pipe must be indexed");
    let iter_count = pipe_sym.flags.iter().filter(|f| *f == "iter").count();
    assert_eq!(
        iter_count, 1,
        "iter flag must dedup, got flags = {:?}",
        pipe_sym.flags
    );
}

#[test]
fn stage3e1_uses_type_primitive_skipped_via_registry() {
    // `u32` / `String` / `bool` are registered as `BuiltinTier::Drop`
    // primitives — no UsesType edge from a parameter / return slot.
    // Validates that the registry is the single source of truth now
    // (previously the deleted `RUST_BUILTIN_TYPES` const lived here).
    let parsed = parse("pub fn add(a: u32, b: u32, name: String) -> bool { true }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let refs = uses_type_with(&edges, &["via-type"]);
    let targets = resolved_targets(&refs);
    assert!(
        targets.is_empty(),
        "primitives + String must skip edges, got {targets:?}",
    );
}

#[test]
fn bug_c3_unresolved_type_carries_unresolved_type_attr() {
    let parsed = parse("pub fn x(p: SomeUnknown) {}");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let refs = uses_type_with(&edges, &["via-type", "unresolved-type"]);
    assert!(
        !refs.is_empty(),
        "expected unresolved-type marker on unknown type ref",
    );
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
            .any(|n| n.ends_with("::SomeUnknown")),
        "expected unresolved canonical name, got {unresolved_names:?}",
    );
}

#[test]
fn bug_c3_enum_variant_inner_field_types_emit_from_variant_fqdn() {
    let parsed = parse("pub struct Foo; pub struct Bar; pub enum E { V(Foo, Bar) }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let refs = uses_type_with(&edges, &["via-type", "type-annotation"]);
    let from_variant: Vec<&RawEdge> = refs
        .iter()
        .copied()
        .filter(|e| e.from_fqdn == "c::E::V")
        .collect();
    let targets = resolved_targets(&from_variant);
    assert!(
        targets.contains(&"c::Foo".to_string()) && targets.contains(&"c::Bar".to_string()),
        "expected variant V → Foo, Bar (got {targets:?})",
    );
}

// --- Stage 3c: class/struct/trait/impl-level generics propagate to
// inner method bodies through the lookup's parent-chain walk. The
// pre-3a-8c `outer_locals` HashSet plumbing handled the simple cases
// but missed scenarios where an impl/trait-level generic was used
// inside an inner method's signature without being explicitly
// re-collected. These tests pin the now-automatic behaviour.

#[test]
fn stage3c_impl_method_filters_impl_level_generic() {
    let parsed =
        parse("pub struct S<T>(T); impl<T> S<T> { pub fn m(&self) -> T { unimplemented!() } }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let leaked: Vec<&RawEdge> = uses_type_edges(&edges)
        .into_iter()
        .filter(|e| {
            e.from_fqdn == "c::S::m"
                && match &e.to {
                    ResolvedOrUnresolved::Resolved { fqdn } => fqdn == "c::T",
                    ResolvedOrUnresolved::Unresolved { name } => name == "c::T",
                    _ => false,
                }
        })
        .collect();
    assert!(
        leaked.is_empty(),
        "impl-level generic T leaked into S::m's signature: {leaked:?}",
    );
}

#[test]
fn stage3c_trait_method_filters_trait_level_generic() {
    let parsed = parse("pub trait Tr<T> { fn m(&self) -> T; }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let leaked: Vec<&RawEdge> = uses_type_edges(&edges)
        .into_iter()
        .filter(|e| {
            e.from_fqdn == "c::Tr::m"
                && match &e.to {
                    ResolvedOrUnresolved::Resolved { fqdn } => fqdn == "c::T",
                    ResolvedOrUnresolved::Unresolved { name } => name == "c::T",
                    _ => false,
                }
        })
        .collect();
    assert!(
        leaked.is_empty(),
        "trait-level generic T leaked into Tr::m: {leaked:?}",
    );
}

#[test]
fn stage3c_impl_method_inner_generic_combined_with_outer_generic() {
    let parsed = parse("pub struct S<T>(T); impl<T> S<T> { pub fn m<U>(_x: T, _y: U) {} }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let m_refs: Vec<&RawEdge> = uses_type_edges(&edges)
        .into_iter()
        .filter(|e| e.from_fqdn == "c::S::m")
        .collect();
    let leaked_names: Vec<String> = m_refs
        .iter()
        .filter_map(|e| match &e.to {
            ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "c::T" || fqdn == "c::U" => {
                Some(fqdn.clone())
            }
            ResolvedOrUnresolved::Unresolved { name } if name == "c::T" || name == "c::U" => {
                Some(name.clone())
            }
            _ => None,
        })
        .collect();
    assert!(
        leaked_names.is_empty(),
        "neither outer T nor inner U should leak as a UsesType: got {leaked_names:?}",
    );
}

#[test]
fn stage3c_trait_method_inner_generic_shadows_trait_generic() {
    // Inner `<T>` shadows the trait-level `<T>`. Either way the
    // resolution lands on `BindingSource::TypeParam` so no phantom
    // `c::T` UsesType edge fires.
    let parsed = parse("pub trait Tr<T> { fn m<T>(_x: T); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let leaked: Vec<&RawEdge> = uses_type_edges(&edges)
        .into_iter()
        .filter(|e| {
            e.from_fqdn == "c::Tr::m"
                && match &e.to {
                    ResolvedOrUnresolved::Resolved { fqdn } => fqdn == "c::T",
                    ResolvedOrUnresolved::Unresolved { name } => name == "c::T",
                    _ => false,
                }
        })
        .collect();
    assert!(
        leaked.is_empty(),
        "shadowed T should still be a local TypeParam: {leaked:?}",
    );
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
fn decl_kind_function_for_top_level_fn() {
    let parsed = parse("pub fn foo() {}");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert_eq!(decl_kind_of(&symbols, "c::foo"), Some(DeclKind::Function));
}

#[test]
fn decl_kind_struct_and_field() {
    let parsed = parse("pub struct S { pub x: u32 }");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert_eq!(decl_kind_of(&symbols, "c::S"), Some(DeclKind::Struct));
    assert_eq!(decl_kind_of(&symbols, "c::S::x"), Some(DeclKind::Field));
}

#[test]
fn decl_kind_enum_and_variant() {
    let parsed = parse("pub enum E { A, B }");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert_eq!(decl_kind_of(&symbols, "c::E"), Some(DeclKind::Enum));
    assert_eq!(
        decl_kind_of(&symbols, "c::E::A"),
        Some(DeclKind::EnumVariant)
    );
}

#[test]
fn decl_kind_union_and_type_alias() {
    let parsed = parse("pub union U { i: u32 } pub type Alias = u32;");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert_eq!(decl_kind_of(&symbols, "c::U"), Some(DeclKind::Union));
    assert_eq!(
        decl_kind_of(&symbols, "c::Alias"),
        Some(DeclKind::TypeAlias)
    );
}

#[test]
fn decl_kind_trait_collapses_to_interface_with_method_items() {
    let parsed = parse("pub trait Tr { fn m(&self); }");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert_eq!(decl_kind_of(&symbols, "c::Tr"), Some(DeclKind::Interface));
    assert_eq!(decl_kind_of(&symbols, "c::Tr::m"), Some(DeclKind::Method));
}

#[test]
fn decl_kind_impl_methods_are_methods_no_impl_symbol() {
    // `impl` blocks do not emit a symbol — only the methods do.
    let parsed = parse("struct F; impl F { pub fn run(&self) {} }");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert!(
        !symbols.iter().any(|s| s.fqdn.contains("::impl")),
        "no impl-block symbol expected: {symbols:?}",
    );
    assert_eq!(decl_kind_of(&symbols, "c::F::run"), Some(DeclKind::Method));
}

#[test]
fn decl_kind_const_and_static() {
    let parsed = parse("pub const C: u32 = 1; pub static S: u32 = 2;");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert_eq!(decl_kind_of(&symbols, "c::C"), Some(DeclKind::Const));
    assert_eq!(decl_kind_of(&symbols, "c::S"), Some(DeclKind::Static));
}

#[test]
fn decl_kind_declarative_macro() {
    let parsed = parse("macro_rules! mac { () => {} }");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    assert_eq!(
        decl_kind_of(&symbols, "c::mac"),
        Some(DeclKind::DeclarativeMacro)
    );
}

fn find_sym<'a>(symbols: &'a [RawSymbol], fqdn: &str) -> &'a RawSymbol {
    symbols
        .iter()
        .find(|s| s.fqdn == fqdn)
        .unwrap_or_else(|| panic!("symbol {fqdn} not found in {symbols:?}"))
}

#[test]
fn receiver_type_set_on_inherent_impl_method() {
    let parsed = parse("struct F; impl F { fn run(&self) {} }");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let m = find_sym(&symbols, "c::F::run");
    assert_eq!(
        m.receiver_type.as_ref().map(|t| t.display.as_str()),
        Some("c::F"),
    );
    assert_eq!(m.implements_trait, None);
}

#[test]
fn implements_trait_and_receiver_type_set_on_trait_impl_method() {
    let parsed = parse("trait Tr { fn run(&self); } struct F; impl Tr for F { fn run(&self) {} }");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let m = find_sym(&symbols, "c::F::run");
    assert_eq!(
        m.receiver_type.as_ref().map(|t| t.display.as_str()),
        Some("c::F"),
    );
    assert_eq!(m.implements_trait.as_deref(), Some("Tr"));
}

#[test]
fn receiver_type_set_on_trait_method_definition() {
    // Trait method definitions carry `Self : Trait` as receiver —
    // expose the trait FQDN so consumers can group by receiver
    // uniformly with impl methods.
    let parsed = parse("trait Tr { fn run(&self); }");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let m = find_sym(&symbols, "c::Tr::run");
    assert_eq!(
        m.receiver_type.as_ref().map(|t| t.display.as_str()),
        Some("c::Tr"),
    );
    assert_eq!(m.implements_trait, None);
}

#[test]
fn free_function_has_no_receiver_or_trait() {
    let parsed = parse("fn foo() {}");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let f = find_sym(&symbols, "c::foo");
    assert_eq!(f.receiver_type, None);
    assert_eq!(f.implements_trait, None);
}

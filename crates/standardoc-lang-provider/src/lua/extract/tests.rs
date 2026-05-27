use super::*;
use std::path::PathBuf;

fn extract(content: &str, workspace_relative: &str, package_relative: &str) -> ExtractedFile {
    extract_file(
        content,
        workspace_relative,
        "myapp",
        package_relative,
        &PathBuf::from(format!("/tmp/pkg/{package_relative}")),
        &PathBuf::from("/tmp/pkg"),
    )
    .expect("extract ok")
}

#[test]
fn empty_file_produces_module_symbol_only() {
    let r = extract("", "main.lua", "main.lua");
    assert_eq!(r.symbols.len(), 1);
    assert!(r.edges.is_empty());
    assert_eq!(r.symbols[0].kind, Kind::Module);
    assert_eq!(r.symbols[0].fqdn, "myapp::main");
}

#[test]
fn module_fqdn_drops_init_segment() {
    let r = extract("", "src/utils/init.lua", "src/utils/init.lua");
    assert_eq!(r.symbols[0].fqdn, "myapp::src::utils");
}

#[test]
fn local_function_extracted_as_private() {
    let src = "local function helper(a, b) return a + b end\n";
    let r = extract(src, "main.lua", "main.lua");
    let sym = r
        .symbols
        .iter()
        .find(|s| s.name == "helper")
        .expect("helper");
    assert_eq!(sym.fqdn, "myapp::main::helper");
    assert_eq!(sym.kind, Kind::Callable);
    assert_eq!(sym.visibility, Visibility::Private);
    let sig = sym.signature.as_ref().expect("sig");
    assert_eq!(sig.params.len(), 2);
    assert_eq!(sig.params[0].name, "a");
}

#[test]
fn global_function_extracted_as_public() {
    let src = "function greet(name) print(name) end\n";
    let r = extract(src, "main.lua", "main.lua");
    let sym = r.symbols.iter().find(|s| s.name == "greet").expect("greet");
    assert_eq!(sym.fqdn, "myapp::main::greet");
    assert_eq!(sym.visibility, Visibility::Public);
}

#[test]
fn dotted_function_decl_yields_nested_fqdn() {
    let src = "function M.foo() end\n";
    let r = extract(src, "lib.lua", "lib.lua");
    let sym = r.symbols.iter().find(|s| s.name == "foo").expect("foo");
    assert_eq!(sym.fqdn, "myapp::lib::M::foo");
    assert_eq!(sym.module.as_deref(), Some("myapp::lib::M"));
}

#[test]
fn method_decl_yields_self_first_param() {
    let src = "function M:bar(x) end\n";
    let r = extract(src, "lib.lua", "lib.lua");
    let sym = r.symbols.iter().find(|s| s.name == "bar").expect("bar");
    assert_eq!(sym.fqdn, "myapp::lib::M::bar");
    let sig = sym.signature.as_ref().expect("sig");
    assert_eq!(sig.params[0].name, "self");
    assert_eq!(sig.params[1].name, "x");
    assert_eq!(sym.language_kind.0, "method");
}

#[test]
fn local_var_extracted_as_value_private() {
    let src = "local count = 0\n";
    let r = extract(src, "main.lua", "main.lua");
    let sym = r.symbols.iter().find(|s| s.name == "count").expect("count");
    assert_eq!(sym.fqdn, "myapp::main::count");
    assert_eq!(sym.kind, Kind::Value);
    assert_eq!(sym.visibility, Visibility::Private);
}

#[test]
fn empty_table_local_marks_module_candidate_then_private_without_return() {
    let src = "local M = {}\nfunction M.foo() end\n";
    let r = extract(src, "lib.lua", "lib.lua");
    let foo = r.symbols.iter().find(|s| s.name == "foo").expect("foo");
    assert_eq!(foo.visibility, Visibility::Private);
}

#[test]
fn module_pattern_promotes_to_public_when_returned() {
    let src = "local M = {}\nfunction M.foo() end\nreturn M\n";
    let r = extract(src, "lib.lua", "lib.lua");
    let foo = r.symbols.iter().find(|s| s.name == "foo").expect("foo");
    assert_eq!(foo.visibility, Visibility::Public);
}

#[test]
fn module_pattern_promotes_method_too() {
    let src = "local M = {}\nfunction M:bar() end\nreturn M\n";
    let r = extract(src, "lib.lua", "lib.lua");
    let bar = r.symbols.iter().find(|s| s.name == "bar").expect("bar");
    assert_eq!(bar.visibility, Visibility::Public);
}

#[test]
fn assignment_with_function_value_extracts_symbol() {
    let src = "local M = {}\nM.alpha = function() end\nreturn M\n";
    let r = extract(src, "lib.lua", "lib.lua");
    let a = r.symbols.iter().find(|s| s.name == "alpha").expect("alpha");
    assert_eq!(a.fqdn, "myapp::lib::M::alpha");
    assert_eq!(a.kind, Kind::Callable);
    assert_eq!(a.visibility, Visibility::Public);
}

#[test]
fn require_with_parens_emits_imports_edge() {
    let src = "local strings = require(\"utils.strings\")\n";
    let r = extract(src, "main.lua", "main.lua");
    let imports: Vec<_> = r
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Imports)
        .collect();
    assert_eq!(imports.len(), 1);
    match &imports[0].to {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "utils.strings"),
        other => panic!("expected Unresolved, got {other:?}"),
    }
}

#[test]
fn require_with_string_arg_no_parens_emits_imports_edge() {
    let src = "local x = require \"json\"\n";
    let r = extract(src, "main.lua", "main.lua");
    let imports: Vec<_> = r
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Imports)
        .collect();
    assert_eq!(imports.len(), 1);
    match &imports[0].to {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "json"),
        other => panic!("expected Unresolved, got {other:?}"),
    }
}

#[test]
fn top_level_function_call_emits_calls_edge() {
    let src = "doStuff()\n";
    let r = extract(src, "main.lua", "main.lua");
    let calls: Vec<_> = r
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Calls)
        .collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].from_fqdn, "myapp::main");
    match &calls[0].to {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "doStuff"),
        other => panic!("expected Unresolved, got {other:?}"),
    }
}

#[test]
fn dotted_call_recorded_with_dotted_name() {
    let src = "M.greet(\"hi\")\n";
    let r = extract(src, "main.lua", "main.lua");
    let calls: Vec<_> = r
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Calls)
        .collect();
    assert_eq!(calls.len(), 1);
    match &calls[0].to {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "M.greet"),
        other => panic!("expected Unresolved, got {other:?}"),
    }
}

#[test]
fn method_call_recorded_with_colon_name() {
    let src = "obj:run(1)\n";
    let r = extract(src, "main.lua", "main.lua");
    let calls: Vec<_> = r
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Calls)
        .collect();
    assert_eq!(calls.len(), 1);
    match &calls[0].to {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "obj:run"),
        other => panic!("expected Unresolved, got {other:?}"),
    }
}

#[test]
fn nested_call_inside_function_body_records_call_from_caller() {
    let src = "local function caller() callee() end\n";
    let r = extract(src, "main.lua", "main.lua");
    let calls: Vec<_> = r
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Calls)
        .collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].from_fqdn, "myapp::main::caller");
    match &calls[0].to {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "callee"),
        other => panic!("expected Unresolved, got {other:?}"),
    }
}

#[test]
fn doc_block_attached_to_local_function() {
    let src = "--- doubles its argument\nlocal function dbl(x) return x*2 end\n";
    let r = extract(src, "main.lua", "main.lua");
    let doc = r
        .documents
        .iter()
        .find(|d| d.symbol_fqdn == "myapp::main::dbl")
        .expect("doc on dbl");
    assert_eq!(doc.description, "doubles its argument");
}

#[test]
fn parse_error_returns_extract_error() {
    let err = extract_file(
        "local x =",
        "broken.lua",
        "myapp",
        "broken.lua",
        &PathBuf::from("/tmp/pkg/broken.lua"),
        &PathBuf::from("/tmp/pkg"),
    )
    .expect_err("must fail");
    match err {
        ExtractError::Parse { file, .. } => assert_eq!(file, "broken.lua"),
        other => panic!("expected Parse, got {other:?}"),
    }
}

#[test]
fn vararg_function_yields_ellipsis_param() {
    let src = "local function f(...) end\n";
    let r = extract(src, "main.lua", "main.lua");
    let sym = r.symbols.iter().find(|s| s.name == "f").expect("f");
    let sig = sym.signature.as_ref().expect("sig");
    assert_eq!(sig.params[0].name, "...");
}

#[test]
fn body_hash_present_for_function() {
    let src = "local function noop() end\n";
    let r = extract(src, "main.lua", "main.lua");
    let sym = r.symbols.iter().find(|s| s.name == "noop").expect("noop");
    assert!(sym.body_hash.is_some());
}

#[test]
fn module_symbol_body_hash_equals_content_hash() {
    let src = "local x = 1\n";
    let r = extract(src, "main.lua", "main.lua");
    let module = &r.symbols[0];
    assert_eq!(module.body_hash, Some(r.content_hash));
}

#[test]
fn language_is_lua() {
    let r = extract("local x = 1\n", "main.lua", "main.lua");
    assert_eq!(r.language, Language::Lua);
}

#[test]
fn nested_table_field_function_only_supported_one_level_day_one() {
    // `function M.sub.foo()` is two-level — fully supported.
    let src = "function M.sub.foo() end\n";
    let r = extract(src, "lib.lua", "lib.lua");
    let foo = r.symbols.iter().find(|s| s.name == "foo").expect("foo");
    assert_eq!(foo.fqdn, "myapp::lib::M::sub::foo");
}

#[test]
fn require_inside_function_body_is_recorded_against_caller() {
    let src = "local function init() local m = require(\"sys\") end\n";
    let r = extract(src, "main.lua", "main.lua");
    let imports: Vec<_> = r
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Imports)
        .collect();
    assert_eq!(imports.len(), 1);
    assert_eq!(imports[0].from_fqdn, "myapp::main::init");
}

#[test]
fn stage3e1_lua_call_top_level_builtin_resolves_to_synthetic() {
    // `print` is registered top-level (Edge tier, Console tag) —
    // the call now produces a `Resolved` edge to the synthetic
    // FQDN with `via-builtin` + `builtin-console` attrs, replacing
    // the pre-3e-1 Unresolved fallthrough.
    let src = "print(\"hi\")\n";
    let r = extract(src, "main.lua", "main.lua");
    let calls: Vec<_> = r
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Calls)
        .collect();
    assert_eq!(calls.len(), 1);
    match &calls[0].to {
        ResolvedOrUnresolved::Resolved { fqdn } => {
            assert!(
                fqdn.starts_with("<builtin>::"),
                "expected synthetic FQDN, got {fqdn}"
            );
            assert!(fqdn.ends_with("::print"), "got {fqdn}");
        }
        other => panic!("expected Resolved synthetic, got {other:?}"),
    }
    assert!(calls[0].attributes.contains(&"via-builtin".to_string()));
    assert!(calls[0].attributes.contains(&"builtin-console".to_string()));
}

#[test]
fn stage3e1_lua_call_dotted_builtin_member_resolves() {
    // `table.insert` is an explicitly enumerated hot member (Iter
    // tag). Full-path lookup hits the registered entry, no fallback
    // needed.
    let src = "local t = {}\ntable.insert(t, 1)\n";
    let r = extract(src, "main.lua", "main.lua");
    let calls: Vec<_> = r
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Calls)
        .collect();
    assert_eq!(calls.len(), 1);
    match &calls[0].to {
        ResolvedOrUnresolved::Resolved { fqdn } => {
            assert!(fqdn.ends_with("::table.insert"), "got {fqdn}");
        }
        other => panic!("expected Resolved synthetic, got {other:?}"),
    }
    assert!(calls[0].attributes.contains(&"builtin-iter".to_string()));
}

#[test]
fn stage3e1_lua_call_unenumerated_module_member_falls_back_to_module() {
    // `os.tmpname()` isn't enumerated as a hot member — the full
    // lookup misses but the leftmost-segment fallback hits the
    // `os` module entry. The edge points at the module synthetic,
    // signalling "this code touches the os stdlib module". (The
    // visitor only enumerates top-level statement calls, so we
    // use the call as a statement rather than an assignment RHS.)
    let src = "os.tmpname()\n";
    let r = extract(src, "main.lua", "main.lua");
    let calls: Vec<_> = r
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Calls)
        .collect();
    assert_eq!(calls.len(), 1);
    match &calls[0].to {
        ResolvedOrUnresolved::Resolved { fqdn } => {
            assert!(fqdn.ends_with("::os"), "got {fqdn}");
        }
        other => panic!("expected Resolved synthetic for os fallback, got {other:?}"),
    }
}

#[test]
fn stage3e1_lua_call_user_dotted_call_stays_unresolved() {
    // `M.greet` is NOT a builtin (neither full nor leftmost) —
    // unchanged from pre-3e-1: emitted as Unresolved canonical.
    // Guards against the leftmost fallback over-matching.
    let src = "M.greet(\"hi\")\n";
    let r = extract(src, "main.lua", "main.lua");
    let calls: Vec<_> = r
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Calls)
        .collect();
    assert_eq!(calls.len(), 1);
    match &calls[0].to {
        ResolvedOrUnresolved::Unresolved { name } => {
            assert_eq!(name, "M.greet");
        }
        other => panic!("user dotted call must stay Unresolved, got {other:?}"),
    }
    assert!(calls[0].attributes.is_empty());
}

// --- IR-4-d: call_sites emission (observational, separate from edges) ---

#[test]
fn ir4d_free_fn_call_emits_call_site_with_classified_args() {
    // `foo("hi", 42, x)` — three positional args. Args must classify
    // string-literals vs identifiers/numbers correctly.
    let src = "local function caller() local x = 0 foo(\"hi\", 42, x) end\n";
    let r = extract(src, "main.lua", "main.lua");
    let cs = r
        .call_sites
        .iter()
        .find(|c| c.callee_text == "foo")
        .unwrap_or_else(|| panic!("expected foo call_site, got {:?}", r.call_sites));
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
fn ir4d_dotted_call_emits_chain_and_dotted_callee_text() {
    // `M.api.create(payload)` — chain=["M","api"], callee_text="M.api.create".
    let src = "local function caller() M.api.create(payload) end\n";
    let r = extract(src, "main.lua", "main.lua");
    let cs = r
        .call_sites
        .iter()
        .find(|c| c.callee_text == "M.api.create")
        .unwrap_or_else(|| panic!("expected M.api.create call_site, got {:?}", r.call_sites));
    assert_eq!(cs.receiver_chain, vec!["M".to_string(), "api".to_string()]);
    assert_eq!(cs.args.len(), 1);
    assert_eq!(cs.args[0].value, "payload");
}

#[test]
fn ir4d_method_colon_call_uses_colon_in_callee_text() {
    // `obj:bar(x)` — Lua method-call syntax. callee_text="obj:bar",
    // receiver_chain=["obj"]. The colon survives into the textual
    // record so plugins can distinguish from `obj.bar(x)`.
    let src = "local function caller() obj:bar(x) end\n";
    let r = extract(src, "main.lua", "main.lua");
    let cs = r
        .call_sites
        .iter()
        .find(|c| c.callee_text == "obj:bar")
        .unwrap_or_else(|| panic!("expected obj:bar call_site, got {:?}", r.call_sites));
    assert_eq!(cs.receiver_chain, vec!["obj".to_string()]);
}

#[test]
fn ir4d_string_call_syntax_carries_single_string_arg() {
    // `require "modname"` — the `foo "literal"` shorthand surfaces
    // as a single string-literal arg. (`require` is special-cased
    // for the Imports edge BEFORE the call_site emit — so we test
    // with a non-require fn to actually see the call_site.)
    let src = "local function caller() greet \"world\" end\n";
    let r = extract(src, "main.lua", "main.lua");
    let cs = r
        .call_sites
        .iter()
        .find(|c| c.callee_text == "greet")
        .unwrap_or_else(|| panic!("expected greet call_site, got {:?}", r.call_sites));
    assert_eq!(cs.args.len(), 1);
    assert!(cs.args[0].is_string_literal);
    assert_eq!(cs.args[0].value, "world");
}

#[test]
fn ir4d_table_constructor_call_carries_single_non_literal_arg() {
    // `foo {1, 2, 3}` — table-constructor shorthand. value carries
    // the table source text; is_string_literal=false.
    let src = "local function caller() foo {1, 2, 3} end\n";
    let r = extract(src, "main.lua", "main.lua");
    let cs = r
        .call_sites
        .iter()
        .find(|c| c.callee_text == "foo")
        .unwrap_or_else(|| panic!("expected foo call_site, got {:?}", r.call_sites));
    assert_eq!(cs.args.len(), 1);
    assert!(!cs.args[0].is_string_literal);
    assert!(cs.args[0].value.contains('1'));
}

#[test]
fn ir4d_builtin_call_still_emits_call_site() {
    // `print(x)` — `print` is an Edge-tier Lua builtin (resolves
    // to a synthetic FQDN with `via-builtin`). The call_site must
    // still surface independently of the edge classification.
    let src = "local function caller() print(\"hi\") end\n";
    let r = extract(src, "main.lua", "main.lua");
    assert!(
        r.call_sites.iter().any(|c| c.callee_text == "print"),
        "print call_site must surface, got {:?}",
        r.call_sites
    );
}

#[test]
fn ir4d_require_emits_import_edge_but_no_call_site() {
    // `require("mod")` — special-cased for the IMPORTS edge before
    // the call_site emit. Plugins reading require usage should
    // walk edges, not call_sites — keeping the textual record
    // empty avoids double-counting.
    let src = "local M = require(\"sibling\")\n";
    let r = extract(src, "main.lua", "main.lua");
    assert!(
        r.call_sites.iter().all(|c| c.callee_text != "require"),
        "require should not emit a call_site, got {:?}",
        r.call_sites
    );
}

#[test]
fn ir4d_call_site_from_fqdn_attributes_to_enclosing_function() {
    // `function svc:run() helper() end` — the call_site for
    // `helper()` must have `from_fqdn = myapp::main::svc::run`,
    // not the module fqdn.
    let src = "function svc:run() helper() end\n";
    let r = extract(src, "main.lua", "main.lua");
    let cs = r
        .call_sites
        .iter()
        .find(|c| c.callee_text == "helper")
        .unwrap_or_else(|| panic!("expected helper call_site, got {:?}", r.call_sites));
    assert_eq!(cs.from_fqdn, "myapp::main::svc::run");
}

fn decl_kind_of(file: &ExtractedFile, fqdn: &str) -> Option<DeclKind> {
    file.symbols
        .iter()
        .find(|s| s.fqdn == fqdn)
        .unwrap_or_else(|| panic!("symbol {fqdn} not found in {:?}", file.symbols))
        .decl_kind
        .clone()
}

#[test]
fn decl_kind_module_for_file_module() {
    let r = extract("", "main.lua", "main.lua");
    assert_eq!(decl_kind_of(&r, "myapp::main"), Some(DeclKind::Module));
}

#[test]
fn decl_kind_function_for_local_function() {
    let src = "local function helper() end\n";
    let r = extract(src, "main.lua", "main.lua");
    assert_eq!(
        decl_kind_of(&r, "myapp::main::helper"),
        Some(DeclKind::Function),
    );
}

#[test]
fn decl_kind_function_for_global_function() {
    let src = "function greet() end\n";
    let r = extract(src, "main.lua", "main.lua");
    assert_eq!(
        decl_kind_of(&r, "myapp::main::greet"),
        Some(DeclKind::Function),
    );
}

#[test]
fn decl_kind_function_for_dotted_function() {
    let src = "function M.foo() end\n";
    let r = extract(src, "lib.lua", "lib.lua");
    assert_eq!(
        decl_kind_of(&r, "myapp::lib::M::foo"),
        Some(DeclKind::Function),
    );
}

#[test]
fn decl_kind_method_for_colon_function() {
    let src = "function M:bar() end\n";
    let r = extract(src, "lib.lua", "lib.lua");
    assert_eq!(
        decl_kind_of(&r, "myapp::lib::M::bar"),
        Some(DeclKind::Method),
    );
}

#[test]
fn decl_kind_var_for_local_assignment() {
    let src = "local counter = 0\n";
    let r = extract(src, "main.lua", "main.lua");
    assert_eq!(
        decl_kind_of(&r, "myapp::main::counter"),
        Some(DeclKind::Var),
    );
}

#[test]
fn decl_kind_function_for_table_assignment_with_function_rhs() {
    // `M.foo = function() end` — extract_assignment emits as Kind::Callable
    // because the RHS is a function literal.
    let src = "local M = {}\nM.foo = function() end\n";
    let r = extract(src, "lib.lua", "lib.lua");
    assert_eq!(
        decl_kind_of(&r, "myapp::lib::M::foo"),
        Some(DeclKind::Function),
    );
}

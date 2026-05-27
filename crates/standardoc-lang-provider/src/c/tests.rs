use super::*;
use standardoc_ir::{
    EdgeKind, FfiAbi, FfiDirection, Kind, Language, ResolvedOrUnresolved, Visibility,
};
use std::path::Path;

fn run(source: &str, path: &str) -> ExtractedFile {
    let provider = CProvider::new();
    let ctx = ExtractContext {
        workspace_root: Path::new("/tmp/lurlang"),
        cross_workspace: None,
    };
    provider.extract(source, path, &ctx).expect("extract ok")
}

fn find<'a>(file: &'a ExtractedFile, fqdn: &str) -> &'a standardoc_ir::RawSymbol {
    file.symbols
        .iter()
        .find(|s| s.fqdn == fqdn)
        .unwrap_or_else(|| panic!("symbol {fqdn} not found, got: {:?}", file.symbols))
}

#[test]
fn extracts_function_definition_as_fn_kind() {
    let src = "int lur_vm_run(int x) { return x + 1; }\n";
    let file = run(src, "runtime/vm.c");
    let s = find(&file, "lurlang::runtime::vm::lur_vm_run");
    assert_eq!(s.kind, Kind::Callable);
    assert_eq!(s.language_kind.as_str(), "fn");
    assert_eq!(s.visibility, Visibility::Public);
    assert!(s.body_hash.is_some());
    assert_eq!(file.language, Language::C);
}

#[test]
fn extract_tags_c_main_as_binary_main_entry_point() {
    use standardoc_ir::EntryPointKind;
    let src = "int main(int argc, char** argv) { return 0; }\nvoid helper(void) {}\n";
    let file = run(src, "runtime/cli.c");
    let main_sym = find(&file, "lurlang::runtime::cli::main");
    assert_eq!(main_sym.entry_point, Some(EntryPointKind::BinaryMain));
    let helper = find(&file, "lurlang::runtime::cli::helper");
    assert_eq!(helper.entry_point, None);
}

#[test]
fn extract_tags_luaopen_prefixed_fn_as_ffi_export() {
    use standardoc_ir::EntryPointKind;
    // Lua C-API contract — `require("foo")` calls a symbol named
    // `luaopen_foo`. The prefix is the entry-point marker.
    let src = "int luaopen_matchigo(void* L) { return 1; }\n";
    let file = run(src, "matchigo.c");
    let s = find(&file, "lurlang::matchigo::luaopen_matchigo");
    assert_eq!(s.entry_point, Some(EntryPointKind::FfiExport));
}

#[test]
fn static_function_is_private_visibility() {
    let src = "static void internal_reset(int* p) { *p = 0; }\n";
    let file = run(src, "runtime/vm.c");
    let s = find(&file, "lurlang::runtime::vm::internal_reset");
    assert_eq!(s.visibility, Visibility::Private);
}

#[test]
fn function_prototype_in_header_is_fn_decl() {
    let src = "int lur_compile(const char* src);\n";
    let file = run(src, "include/lur.h");
    let s = find(&file, "lurlang::include::lur::lur_compile");
    assert_eq!(s.kind, Kind::Callable);
    assert_eq!(s.language_kind.as_str(), "fn_decl");
    assert!(s.body_hash.is_none());
}

#[test]
fn named_struct_emitted_as_type() {
    let src = "struct LurVm { int magic; char* program; };\n";
    let file = run(src, "runtime/vm.h");
    let s = find(&file, "lurlang::runtime::vm::LurVm");
    assert_eq!(s.kind, Kind::Type);
    assert_eq!(s.language_kind.as_str(), "struct");
}

#[test]
fn enum_emits_parent_and_variants_as_sub_symbols() {
    let src = "enum LurColor { RED, GREEN, BLUE };\n";
    let file = run(src, "runtime/colors.h");
    let parent = find(&file, "lurlang::runtime::colors::LurColor");
    assert_eq!(parent.kind, Kind::Type);
    assert_eq!(parent.language_kind.as_str(), "enum");

    for variant in ["RED", "GREEN", "BLUE"] {
        let fqdn = format!("lurlang::runtime::colors::LurColor::{variant}");
        let v = find(&file, &fqdn);
        assert_eq!(v.kind, Kind::Value);
        assert_eq!(v.language_kind.as_str(), "enum_variant");
    }
}

#[test]
fn typedef_emits_type_alias() {
    let src = "typedef struct LurVm LurVm;\n";
    let file = run(src, "runtime/vm.h");
    let s = find(&file, "lurlang::runtime::vm::LurVm");
    // Both the struct decl AND the typedef alias hit here — the typedef
    // wins by name. The point: the symbol exists and is queryable.
    assert_eq!(s.kind, Kind::Type);
}

#[test]
fn macros_emit_with_correct_kind_and_language_kind() {
    let src = "#define MAX_FOO 100\n#define MIN(a, b) ((a) < (b) ? (a) : (b))\n";
    let file = run(src, "include/lur.h");
    let obj = find(&file, "lurlang::include::lur::MAX_FOO");
    assert_eq!(obj.kind, Kind::Macro);
    assert_eq!(obj.language_kind.as_str(), "macro_object");
    let func = find(&file, "lurlang::include::lur::MIN");
    assert_eq!(func.kind, Kind::Macro);
    assert_eq!(func.language_kind.as_str(), "macro_fn");
}

#[test]
fn ifdef_guarded_content_is_still_indexed() {
    // Headers wrap content in `#ifndef X / #define X / ... / #endif`.
    // The walker must descend into preproc_ifdef so the symbols inside
    // are reachable.
    let src = "#ifndef LUR_H\n#define LUR_H\nint lur_init(void);\n#endif\n";
    let file = run(src, "include/lur.h");
    let _ = find(&file, "lurlang::include::lur::lur_init");
}

#[test]
fn module_symbol_is_emitted_first() {
    let src = "int foo(void);\n";
    let file = run(src, "runtime/vm.c");
    assert!(!file.symbols.is_empty());
    let first = &file.symbols[0];
    assert_eq!(first.kind, Kind::Module);
    assert_eq!(first.fqdn, "lurlang::runtime::vm");
}

#[test]
fn system_include_emits_resolved_builtin_edge() {
    let src = "#include <stdio.h>\nint foo(void) { return 0; }\n";
    let file = run(src, "runtime/vm.c");
    let edge = file
        .edges
        .iter()
        .find(|e| e.kind == EdgeKind::Imports)
        .expect("imports edge missing");
    assert_eq!(edge.from_fqdn, "lurlang::runtime::vm");
    match &edge.to {
        ResolvedOrUnresolved::Resolved { fqdn } => {
            assert_eq!(fqdn, "<builtin>::c::stdio");
        }
        other => panic!("expected Resolved builtin, got {other:?}"),
    }
}

#[test]
fn local_include_emits_unresolved_edge_with_path() {
    let src = "#include \"runtime/util.h\"\nint foo(void);\n";
    let file = run(src, "runtime/vm.c");
    let edge = file
        .edges
        .iter()
        .find(|e| e.kind == EdgeKind::Imports)
        .expect("imports edge missing");
    match &edge.to {
        ResolvedOrUnresolved::Unresolved { name } => {
            assert_eq!(name, "runtime/util.h");
        }
        other => panic!("expected Unresolved by name, got {other:?}"),
    }
}

#[test]
fn multiple_includes_all_get_edges() {
    let src = "#include <stdlib.h>\n#include <string.h>\n#include \"foo.h\"\n";
    let file = run(src, "main.c");
    let imports: Vec<_> = file
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Imports)
        .collect();
    assert_eq!(imports.len(), 3);
}

#[test]
fn non_static_fn_emits_c_abi_export_binding() {
    let src = "int lur_vm_init(void) { return 0; }\n";
    let file = run(src, "runtime/vm.c");
    assert_eq!(file.ffi_bindings.len(), 1);
    let b = &file.ffi_bindings[0];
    assert_eq!(b.abi, FfiAbi::C);
    assert_eq!(b.direction, FfiDirection::Export);
    assert_eq!(b.abi_name, "lur_vm_init");
    assert_eq!(b.symbol_fqdn, "lurlang::runtime::vm::lur_vm_init");
}

#[test]
fn static_fn_does_not_emit_ffi_binding() {
    let src = "static int internal(void) { return 0; }\n";
    let file = run(src, "runtime/vm.c");
    assert!(
        file.ffi_bindings.is_empty(),
        "`static` functions are file-local and must not be tagged for FFI"
    );
}

#[test]
fn header_prototype_emits_c_abi_import_binding() {
    let src = "int lur_compile(const char* src);\n";
    let file = run(src, "include/lur.h");
    assert_eq!(file.ffi_bindings.len(), 1);
    let b = &file.ffi_bindings[0];
    assert_eq!(b.direction, FfiDirection::Import);
    assert_eq!(b.abi_name, "lur_compile");
}

#[test]
fn mixed_fn_def_and_static_fn_only_def_is_tagged() {
    let src = "static void helper(void) {}\nint exported(int x) { return x; }\n";
    let file = run(src, "runtime/vm.c");
    let exports: Vec<_> = file
        .ffi_bindings
        .iter()
        .filter(|b| b.direction == FfiDirection::Export)
        .collect();
    assert_eq!(exports.len(), 1);
    assert_eq!(exports[0].abi_name, "exported");
}

// ------------------------------------------------------------------
// G2 — intra-function call_sites
// ------------------------------------------------------------------

fn calls_in<'a>(file: &'a ExtractedFile, from_fqdn: &str) -> Vec<&'a standardoc_ir::RawCallSite> {
    file.call_sites
        .iter()
        .filter(|c| c.from_fqdn == from_fqdn)
        .collect()
}

#[test]
fn call_site_simple_call_inside_function_body() {
    let src = "void caller(void) { callee(); }\n";
    let file = run(src, "runtime/vm.c");
    let calls = calls_in(&file, "lurlang::runtime::vm::caller");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].callee_text, "callee");
    assert!(calls[0].receiver_chain.is_empty());
    assert!(calls[0].args.is_empty());
}

#[test]
fn call_site_emitted_inside_for_and_if_bodies() {
    let src = "void f(int n) {\n\
                   \tfor (int i = 0; i < n; i++) { loop_body(i); }\n\
                   \tif (n > 0) { branch_taken(); } else { branch_not_taken(); }\n\
                   }\n";
    let file = run(src, "runtime/vm.c");
    let callees: Vec<&str> = calls_in(&file, "lurlang::runtime::vm::f")
        .into_iter()
        .map(|c| c.callee_text.as_str())
        .collect();
    assert!(callees.contains(&"loop_body"));
    assert!(callees.contains(&"branch_taken"));
    assert!(callees.contains(&"branch_not_taken"));
}

#[test]
fn call_site_via_dot_member_records_receiver_chain() {
    let src = "void f(struct S obj) { obj.method(1); }\n";
    let file = run(src, "runtime/vm.c");
    let calls = calls_in(&file, "lurlang::runtime::vm::f");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].callee_text, "obj.method");
    assert_eq!(calls[0].receiver_chain, vec!["obj".to_string()]);
}

#[test]
fn call_site_via_arrow_pointer_member_records_receiver_chain() {
    let src = "void f(struct S* p) { p->handler(42); }\n";
    let file = run(src, "runtime/vm.c");
    let calls = calls_in(&file, "lurlang::runtime::vm::f");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].callee_text, "p->handler");
    assert_eq!(calls[0].receiver_chain, vec!["p".to_string()]);
}

#[test]
fn call_site_macro_like_invocation_is_captured() {
    // `printf` parses as `call_expression` in tree-sitter-c whether
    // it is a real fn or a `#define`-style macro. The plugin layer
    // dedups against the symbol table; the extractor stays uniform.
    let src = "void f(int n) { printf(\"got %d\", n); }\n";
    let file = run(src, "runtime/vm.c");
    let calls = calls_in(&file, "lurlang::runtime::vm::f");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].callee_text, "printf");
    assert_eq!(calls[0].args.len(), 2);
    assert!(calls[0].args[0].is_string_literal);
    assert_eq!(calls[0].args[0].value, "\"got %d\"");
    assert!(!calls[0].args[1].is_string_literal);
    assert_eq!(calls[0].args[1].value, "n");
}

#[test]
fn call_site_function_pointer_call_keeps_verbatim_callee_text() {
    let src = "void f(void (*fp)(int)) { (*fp)(7); }\n";
    let file = run(src, "runtime/vm.c");
    let calls = calls_in(&file, "lurlang::runtime::vm::f");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].callee_text, "(*fp)");
    // Function-pointer indirection has no owning receiver.
    assert!(calls[0].receiver_chain.is_empty());
    assert_eq!(calls[0].args.len(), 1);
    assert_eq!(calls[0].args[0].value, "7");
}

#[test]
fn call_site_nested_call_in_argument_emits_both() {
    let src = "void f(void) { outer(inner()); }\n";
    let file = run(src, "runtime/vm.c");
    let callees: Vec<&str> = calls_in(&file, "lurlang::runtime::vm::f")
        .into_iter()
        .map(|c| c.callee_text.as_str())
        .collect();
    assert!(callees.contains(&"outer"));
    assert!(callees.contains(&"inner"));
}

#[test]
fn call_site_static_function_emits_calls_with_static_fqdn() {
    // `static` functions still need call_sites — the plugin layer
    // wants to track intra-file call graphs for refactor warnings,
    // etc. The from_fqdn carries the static fn's module-qualified
    // name regardless of visibility.
    let src = "static void helper(void) { do_work(); }\n";
    let file = run(src, "runtime/vm.c");
    let calls = calls_in(&file, "lurlang::runtime::vm::helper");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].callee_text, "do_work");
}

#[test]
fn call_site_prototype_in_header_emits_nothing() {
    // Function prototypes have no body, so no calls. Sanity check
    // that the body-less branch in `emit_function_definition`
    // doesn't accidentally fire the descent.
    let src = "int compile(const char* src);\n";
    let file = run(src, "include/lur.h");
    assert!(file.call_sites.is_empty());
}

#[test]
fn call_site_records_correct_line_and_file() {
    let src = "void f(void) {\n\
                   \tno_op();\n\
                   \ttarget();\n\
                   }\n";
    let file = run(src, "runtime/vm.c");
    let calls = calls_in(&file, "lurlang::runtime::vm::f");
    let target = calls
        .iter()
        .find(|c| c.callee_text == "target")
        .expect("target call_site");
    assert_eq!(target.site.file, "runtime/vm.c");
    // Source layout: line 1 = signature, 2 = no_op, 3 = target.
    assert_eq!(target.site.line, 3);
}

#[test]
fn decl_kind_function_for_fn_def_and_prototype() {
    let src = "int impl_fn(int x) { return x; }\nint proto_fn(int);\n";
    let file = run(src, "runtime/vm.c");
    assert_eq!(
        find(&file, "lurlang::runtime::vm::impl_fn").decl_kind,
        Some(standardoc_ir::DeclKind::Function),
    );
    assert_eq!(
        find(&file, "lurlang::runtime::vm::proto_fn").decl_kind,
        Some(standardoc_ir::DeclKind::Function),
    );
}

#[test]
fn decl_kind_struct_and_union() {
    let src = "struct S { int x; };\nunion U { int i; float f; };\n";
    let file = run(src, "runtime/vm.c");
    assert_eq!(
        find(&file, "lurlang::runtime::vm::S").decl_kind,
        Some(standardoc_ir::DeclKind::Struct),
    );
    assert_eq!(
        find(&file, "lurlang::runtime::vm::U").decl_kind,
        Some(standardoc_ir::DeclKind::Union),
    );
}

#[test]
fn decl_kind_enum_and_variants() {
    let src = "enum E { A, B };\n";
    let file = run(src, "runtime/vm.c");
    assert_eq!(
        find(&file, "lurlang::runtime::vm::E").decl_kind,
        Some(standardoc_ir::DeclKind::Enum),
    );
    assert_eq!(
        find(&file, "lurlang::runtime::vm::E::A").decl_kind,
        Some(standardoc_ir::DeclKind::EnumVariant),
    );
    assert_eq!(
        find(&file, "lurlang::runtime::vm::E::B").decl_kind,
        Some(standardoc_ir::DeclKind::EnumVariant),
    );
}

#[test]
fn decl_kind_typedef_is_type_alias() {
    let src = "typedef int MyInt;\n";
    let file = run(src, "runtime/vm.c");
    assert_eq!(
        find(&file, "lurlang::runtime::vm::MyInt").decl_kind,
        Some(standardoc_ir::DeclKind::TypeAlias),
    );
}

#[test]
fn decl_kind_macros_are_declarative_macro() {
    let src = "#define MAX_LEN 256\n#define MIN(a,b) ((a)<(b)?(a):(b))\n";
    let file = run(src, "runtime/vm.c");
    assert_eq!(
        find(&file, "lurlang::runtime::vm::MAX_LEN").decl_kind,
        Some(standardoc_ir::DeclKind::DeclarativeMacro),
    );
    assert_eq!(
        find(&file, "lurlang::runtime::vm::MIN").decl_kind,
        Some(standardoc_ir::DeclKind::DeclarativeMacro),
    );
}


use super::*;
use swc_core::common::{FileName, SourceMap, sync::Lrc};
use swc_core::ecma::ast::EsVersion;
use swc_core::ecma::parser::{Parser, StringInput, Syntax, TsSyntax, lexer::Lexer};

fn parse(src: &str) -> (Module, Lrc<SourceMap>) {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(
        Lrc::new(FileName::Custom("test.ts".into())),
        src.to_string(),
    );
    let lexer = Lexer::new(
        Syntax::Typescript(TsSyntax::default()),
        EsVersion::EsNext,
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    let module = parser.parse_module().expect("parse module");
    (module, cm)
}

fn run(src: &str) -> (Vec<RawSymbol>, Vec<RawFfiBinding>) {
    let (module, cm) = parse(src);
    extract_ffi_bindings(&module, "pkg::test", "test.ts", &cm)
}

#[test]
fn bun_ffi_dlopen_emits_symbol_and_binding_per_key() {
    let src = "import { dlopen, FFIType } from \"bun:ffi\";\n\
                   const lib = dlopen(\"./libfoo.so\", {\n\
                   \tfoo: { args: [FFIType.i32], returns: FFIType.i32 },\n\
                   \tbar: { args: [], returns: FFIType.void },\n\
                   });\n";
    let (symbols, bindings) = run(src);

    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"foo"));
    assert!(names.contains(&"bar"));
    for s in &symbols {
        assert_eq!(s.kind, Kind::Value);
        assert_eq!(s.language_kind.as_str(), "ffi_import");
        assert!(s.flags.iter().any(|f| f == "ffi-import"));
    }
    let binding_names: Vec<&str> = bindings.iter().map(|b| b.abi_name.as_str()).collect();
    assert!(binding_names.contains(&"foo"));
    assert!(binding_names.contains(&"bar"));
    for b in &bindings {
        assert_eq!(b.abi, FfiAbi::C);
        assert_eq!(b.direction, FfiDirection::Import);
        assert_eq!(b.convention.as_deref(), Some("bun-dlopen"));
    }
}

#[test]
fn deno_dlopen_emits_with_deno_convention() {
    let src = "const lib = Deno.dlopen(\"./libfoo.so\", {\n\
                   \tfoo: { parameters: [\"i32\"], result: \"i32\" },\n\
                   });\n";
    let (symbols, bindings) = run(src);
    assert_eq!(symbols.len(), 1);
    assert_eq!(bindings.len(), 1);
    assert_eq!(symbols[0].fqdn, "pkg::test::foo");
    assert_eq!(bindings[0].convention.as_deref(), Some("deno-dlopen"));
}

#[test]
fn aliased_bun_dlopen_import_is_tracked() {
    let src = "import { dlopen as bunOpen } from \"bun:ffi\";\n\
                   const lib = bunOpen(\"./libfoo.so\", { foo: {} });\n";
    let (symbols, bindings) = run(src);
    assert_eq!(symbols.len(), 1);
    assert_eq!(bindings.len(), 1);
    assert_eq!(symbols[0].name, "foo");
}

#[test]
fn bare_dlopen_without_bun_ffi_import_is_ignored() {
    // No `bun:ffi` import means `dlopen` could be anything (user-
    // defined fn, FFI from another lib). The tagger plays safe and
    // emits nothing.
    let src = "const lib = dlopen(\"./libfoo.so\", { foo: {} });\n";
    let (symbols, bindings) = run(src);
    assert!(symbols.is_empty());
    assert!(bindings.is_empty());
}

#[test]
fn deno_dlopen_via_globalthis_is_recognised() {
    let src = "const lib = globalThis.Deno.dlopen(\"./libfoo.so\", { foo: {} });\n";
    let (symbols, bindings) = run(src);
    assert_eq!(symbols.len(), 1);
    assert_eq!(bindings[0].convention.as_deref(), Some("deno-dlopen"));
}

#[test]
fn non_object_second_arg_is_skipped_silently() {
    // `dlopen(path, symbols_var)` — we can't enumerate the keys
    // statically. Skip cleanly rather than mis-attribute.
    let src = "import { dlopen } from \"bun:ffi\";\n\
                   const syms = { foo: {} };\n\
                   const lib = dlopen(\"./libfoo.so\", syms);\n";
    let (symbols, bindings) = run(src);
    assert!(symbols.is_empty());
    assert!(bindings.is_empty());
}

#[test]
fn computed_property_keys_are_skipped() {
    let src = "import { dlopen } from \"bun:ffi\";\n\
                   const lib = dlopen(\"./libfoo.so\", { [dynamic]: {}, real: {} });\n";
    let (symbols, _bindings) = run(src);
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["real"]);
}

#[test]
fn string_literal_keys_are_accepted() {
    let src = "import { dlopen } from \"bun:ffi\";\n\
                   const lib = dlopen(\"./libfoo.so\", { \"foo\": {}, \"bar-baz\": {} });\n";
    let (symbols, _bindings) = run(src);
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"foo"));
    assert!(names.contains(&"bar-baz"));
}

#[test]
fn dlopen_inside_function_body_still_emits() {
    // The visitor descends into nested expressions, so a factory
    // pattern `() => dlopen(...)` still produces bindings.
    let src = "import { dlopen } from \"bun:ffi\";\n\
                   function loadLib() {\n\
                   \treturn dlopen(\"./libfoo.so\", { foo: {} });\n\
                   }\n";
    let (symbols, bindings) = run(src);
    assert_eq!(symbols.len(), 1);
    assert_eq!(bindings.len(), 1);
}

#[test]
fn multiple_dlopen_calls_emit_all_keys() {
    // Two distinct dlopen sites both contribute. Today the FQDN
    // scheme collides if both sites import the same name; the
    // visitor still emits both rows, and storage de-dupes on
    // primary key at insert time. Document the behaviour.
    let src = "import { dlopen } from \"bun:ffi\";\n\
                   const a = dlopen(\"./a.so\", { alpha: {} });\n\
                   const b = dlopen(\"./b.so\", { beta: {} });\n";
    let (symbols, bindings) = run(src);
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"alpha"));
    assert!(names.contains(&"beta"));
    assert_eq!(bindings.len(), 2);
}

#[test]
fn span_locations_point_at_the_property_key() {
    let src = "import { dlopen } from \"bun:ffi\";\n\
                   const lib = dlopen(\"./libfoo.so\", { foo: {} });\n";
    let (symbols, _bindings) = run(src);
    let foo = symbols.iter().find(|s| s.name == "foo").unwrap();
    // The property key `foo` is on line 2.
    assert_eq!(foo.location.start_line, 2);
    assert_eq!(foo.location.file, "test.ts");
}

// ------------------------------------------------------------------
// NAPI follow-up — `.node` addon detection (no per-fn granularity)
// ------------------------------------------------------------------

fn napi_binding(bindings: &[RawFfiBinding]) -> &RawFfiBinding {
    bindings
        .iter()
        .find(|b| b.convention.as_deref() == Some("napi"))
        .expect("napi binding")
}

#[test]
fn napi_default_import_of_node_addon_emits_placeholder_binding() {
    let src = "import addon from \"./addon.node\";\n";
    let (symbols, bindings) = run(src);
    let sym = symbols.iter().find(|s| s.name == "addon").unwrap();
    assert!(sym.flags.iter().any(|f| f == "napi"));
    let b = napi_binding(&bindings);
    assert_eq!(b.symbol_fqdn, "pkg::test::addon");
    assert!(matches!(&b.abi, FfiAbi::Other(s) if s == "napi"));
    assert_eq!(b.direction, FfiDirection::Import);
    assert_eq!(b.abi_name, "addon");
}

#[test]
fn napi_require_of_node_path_emits_placeholder_binding() {
    let src = "const native = require(\"./build/native.node\");\n";
    let (_symbols, bindings) = run(src);
    let b = napi_binding(&bindings);
    assert_eq!(b.symbol_fqdn, "pkg::test::native");
    assert_eq!(b.abi_name, "native");
}

#[test]
fn napi_bindings_factory_uses_inner_string_as_abi_name() {
    let src = "const addon = require(\"bindings\")(\"my-addon\");\n";
    let (symbols, bindings) = run(src);
    let sym = symbols.iter().find(|s| s.name == "addon").unwrap();
    assert!(sym.flags.iter().any(|f| f == "napi"));
    let b = napi_binding(&bindings);
    assert_eq!(b.symbol_fqdn, "pkg::test::addon");
    assert_eq!(b.abi_name, "my-addon");
}

#[test]
fn napi_non_addon_require_is_ignored() {
    // `require("./helpers.js")` isn't a NAPI addon — bail.
    let src = "const helpers = require(\"./helpers.js\");\n";
    let (_symbols, bindings) = run(src);
    assert!(
        bindings
            .iter()
            .all(|b| b.convention.as_deref() != Some("napi"))
    );
}

#[test]
fn napi_destructuring_const_is_skipped() {
    // `const { foo } = require(\"./addon.node\")` — destructuring
    // isn't covered; bun:ffi's `{ foo: ... }` object shape is
    // the way to get per-key granularity.
    let src = "const { foo } = require(\"./addon.node\");\n";
    let (_symbols, bindings) = run(src);
    assert!(
        bindings
            .iter()
            .all(|b| b.convention.as_deref() != Some("napi"))
    );
}

#[test]
fn napi_path_strips_dir_prefix_in_abi_name() {
    let src = "const addon = require(\"./build/Release/addon.node\");\n";
    let (_symbols, bindings) = run(src);
    let b = napi_binding(&bindings);
    assert_eq!(b.abi_name, "addon");
}

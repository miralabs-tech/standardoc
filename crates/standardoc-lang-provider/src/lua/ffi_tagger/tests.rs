use super::*;

fn run(src: &str) -> (Vec<RawSymbol>, Vec<RawFfiBinding>) {
    let ast = full_moon::parse(src).expect("parse lua");
    extract_ffi_bindings(&ast, "pkg::test", "test.lua", src)
}

#[test]
fn cdef_block_emits_one_symbol_and_binding_per_function() {
    let src = "local ffi = require(\"ffi\")\n\
                   ffi.cdef[[\n\
                   int add(int a, int b);\n\
                   void log_line(const char* s);\n\
                   ]]\n";
    let (symbols, bindings) = run(src);
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"add"));
    assert!(names.contains(&"log_line"));
    for s in &symbols {
        assert_eq!(s.kind, Kind::Value);
        assert_eq!(s.language_kind.as_str(), "ffi_import");
        assert!(s.flags.iter().any(|f| f == "ffi-import"));
    }
    for b in &bindings {
        assert_eq!(b.abi, FfiAbi::C);
        assert_eq!(b.direction, FfiDirection::Import);
        assert_eq!(b.convention.as_deref(), Some("luajit-ffi"));
    }
}

#[test]
fn aliased_require_is_tracked() {
    let src = "local ffi_alt = require(\"ffi\")\n\
                   ffi_alt.cdef[[ int compute(int n); ]]\n";
    let (symbols, bindings) = run(src);
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].name, "compute");
    assert_eq!(bindings.len(), 1);
}

#[test]
fn cdef_via_parens_string_arg_also_works() {
    let src = "local ffi = require(\"ffi\")\n\
                   ffi.cdef(\"int paren_fn(int);\")\n";
    let (_symbols, bindings) = run(src);
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].abi_name, "paren_fn");
}

#[test]
fn typedef_in_cdef_is_skipped() {
    let src = "local ffi = require(\"ffi\")\n\
                   ffi.cdef[[\n\
                   typedef struct point { int x; int y; } point_t;\n\
                   int distance(point_t a, point_t b);\n\
                   ]]\n";
    let (symbols, bindings) = run(src);
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["distance"]);
    assert_eq!(bindings.len(), 1);
}

#[test]
fn no_ffi_require_means_no_emission() {
    let src = "local x = require(\"other\")\n\
                   x.cdef[[ int foo(int); ]]\n";
    let (symbols, bindings) = run(src);
    assert!(symbols.is_empty());
    assert!(bindings.is_empty());
}

#[test]
fn cdef_with_non_string_arg_emits_nothing() {
    // `ffi.cdef(get_decls())` — body is dynamic, we can't read it.
    let src = "local ffi = require(\"ffi\")\n\
                   ffi.cdef(get_decls())\n";
    let (symbols, bindings) = run(src);
    assert!(symbols.is_empty());
    assert!(bindings.is_empty());
}

#[test]
fn duplicate_function_in_two_cdef_blocks_emits_once() {
    // Same name appearing twice (e.g. multi-cdef refactor) dedups
    // to a single virtual symbol + binding. Avoids the storage
    // insert duplicate-key failure.
    let src = "local ffi = require(\"ffi\")\n\
                   ffi.cdef[[ int twice(int); ]]\n\
                   ffi.cdef[[ int twice(int); ]]\n";
    let (symbols, bindings) = run(src);
    assert_eq!(symbols.len(), 1);
    assert_eq!(bindings.len(), 1);
}

#[test]
fn pointer_return_type_is_recognised() {
    let src = "local ffi = require(\"ffi\")\n\
                   ffi.cdef[[ const char* version(void); ]]\n";
    let (_symbols, bindings) = run(src);
    let names: Vec<&str> = bindings.iter().map(|b| b.abi_name.as_str()).collect();
    assert_eq!(names, vec!["version"]);
}

#[test]
fn cdef_inside_function_body_still_emits() {
    // `ffi.cdef` calls inside a wrapping fn (factory pattern) are
    // still detected by the visitor descending into bodies.
    let src = "local ffi = require(\"ffi\")\n\
                   local function setup()\n\
                   \tffi.cdef[[ int inside(int); ]]\n\
                   end\n";
    let (symbols, _bindings) = run(src);
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].name, "inside");
}

//! Stage 2 — FFI binding extractor for the LuaJIT FFI surface.
//!
//! Detects the LuaJIT idiom:
//!
//!   ```lua
//!   local ffi = require("ffi")
//!   ffi.cdef[[
//!     int   add(int a, int b);
//!     void  log_line(const char* s);
//!     typedef struct point { int x; int y; } point_t;
//!   ]]
//!   local lib = ffi.load("libfoo")
//!   lib.add(1, 2)
//!   ```
//!
//! For each function declaration inside a `cdef` block we emit:
//!
//!   * A `RawSymbol` (`kind = Value`, `language_kind = "ffi_import"`)
//!     anchored at the `cdef` call site so `apply_ffi_bindings` finds
//!     a matching `symbols.id` to attach the binding to.
//!   * A `RawFfiBinding` (`abi = C`, `direction = Import`,
//!     `convention = "luajit-ffi"`) so `ffi_resolve` can pair the
//!     declaration with a C-side `Export` of the same `abi_name`.
//!
//! The cdef body is re-parsed with `tree-sitter-c` — same parser the
//! C provider uses — so any prototype the C extractor recognises is
//! also recognised here. Non-function declarations (typedefs, globals,
//! `#include` directives) are skipped silently.
//!
//! Aliased imports are honoured: `local ffi_alt = require("ffi")` →
//! `ffi_alt.cdef(...)` extracts the same way. The visitor tracks every
//! local name bound to `require("ffi")` so re-aliasing is supported.
//!
//! `ffi.load(name)` and the `lib.foo()` bridge call are intentionally
//! out of scope here — the binding-level emission is enough for the
//! cross-language resolver to match the cdef-declared function against
//! a C `Export` regardless of how it is later loaded.

use full_moon::ast::{Ast, Call, Expression, FunctionArgs, FunctionCall, Prefix, Suffix};
use full_moon::tokenizer::TokenType;
use full_moon::visitors::Visitor;
use standardoc_ir::{
    FfiAbi, FfiDirection, Kind, LanguageKind, RawFfiBinding, RawSymbol, Signature, SymbolLocation,
    Visibility,
};
use tree_sitter::{Node, Parser};

/// `require("ffi")` — the module spec we treat as the LuaJIT FFI hook.
const REQUIRE_FFI_TARGET: &str = "ffi";

/// Convention slug stamped on every binding produced by the cdef
/// extractor. Resolve ignores `convention` on day-one (matches on
/// `abi + abi_name`), but consumers can filter by this hint.
const CONVENTION_LUAJIT_FFI: &str = "luajit-ffi";

/// `language_kind` for the virtual symbols we emit so the consumer
/// can tell a cdef-declared name apart from an authored Lua fn.
const FFI_IMPORT_LANGUAGE_KIND: &str = "ffi_import";

/// Walk `ast` looking for `<alias>.cdef([[...]])` blocks (where
/// `<alias>` is any local name bound to `require("ffi")`), parse
/// the embedded C, and return one virtual symbol + one binding per
/// declared function. Empty when the file has no `require("ffi")`
/// or no parseable cdef declarations.
pub(crate) fn extract_ffi_bindings(
    ast: &Ast,
    module_fqdn: &str,
    file_path: &str,
) -> (Vec<RawSymbol>, Vec<RawFfiBinding>) {
    let aliases = collect_ffi_aliases(ast);
    if aliases.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let mut visitor = FfiVisitor {
        aliases,
        module_fqdn,
        file_path,
        symbols: Vec::new(),
        bindings: Vec::new(),
        seen_names: std::collections::HashSet::new(),
    };
    visitor.visit_ast(ast);
    (visitor.symbols, visitor.bindings)
}

/// Scan top-level + nested statements for `local <name> = require("ffi")`
/// and return every `<name>` we should treat as the ffi handle. Empty
/// when the file never imports ffi — that's the fast-path bail-out.
fn collect_ffi_aliases(ast: &Ast) -> Vec<String> {
    let mut collector = AliasCollector {
        aliases: Vec::new(),
    };
    collector.visit_ast(ast);
    collector.aliases
}

struct AliasCollector {
    aliases: Vec<String>,
}

impl Visitor for AliasCollector {
    fn visit_local_assignment(&mut self, la: &full_moon::ast::LocalAssignment) {
        let mut names = la.names().iter();
        let mut exprs = la.expressions().iter();
        while let (Some(name), Some(expr)) = (names.next(), exprs.next()) {
            if is_require_of(expr, REQUIRE_FFI_TARGET) {
                self.aliases.push(name.token().to_string());
            }
        }
    }

    fn visit_assignment(&mut self, a: &full_moon::ast::Assignment) {
        // Also accept `ffi = require("ffi")` at the module level.
        // The Lua walker doesn't typically emit a symbol for this case
        // (no `local`), but the alias should still work.
        let mut vars = a.variables().iter();
        let mut exprs = a.expressions().iter();
        while let (Some(var), Some(expr)) = (vars.next(), exprs.next()) {
            let full_moon::ast::Var::Name(name) = var else {
                continue;
            };
            if is_require_of(expr, REQUIRE_FFI_TARGET) {
                self.aliases.push(name.token().to_string());
            }
        }
    }
}

/// Returns `true` when `expr` is `require("<target>")` or
/// `require "<target>"` (Lua's adjacency-call sugar).
fn is_require_of(expr: &Expression, target: &str) -> bool {
    let Expression::FunctionCall(fc) = expr else {
        return false;
    };
    let Prefix::Name(name) = fc.prefix() else {
        return false;
    };
    if name.token().to_string() != "require" {
        return false;
    }
    let mut suffixes = fc.suffixes();
    let Some(Suffix::Call(call)) = suffixes.next() else {
        return false;
    };
    if suffixes.next().is_some() {
        return false;
    }
    let Call::AnonymousCall(args) = call else {
        return false;
    };
    let arg_text = string_arg_value(args);
    arg_text.as_deref() == Some(target)
}

/// Returns the literal value of the single string argument inside
/// `args`, supporting both `f("x")` (parentheses) and `f"x"` /
/// `f[[x]]` (adjacency-call sugar).
fn string_arg_value(args: &FunctionArgs) -> Option<String> {
    match args {
        FunctionArgs::Parentheses { arguments, .. } => {
            let first = arguments.iter().next()?;
            literal_string_of(first)
        }
        FunctionArgs::String(token) => {
            let TokenType::StringLiteral { literal, .. } = token.token_type() else {
                return None;
            };
            Some(literal.to_string())
        }
        _ => None,
    }
}

/// Returns the value of `expr` when it is a string literal — handles
/// both quoted strings and Lua long brackets (`[[...]]`).
fn literal_string_of(expr: &Expression) -> Option<String> {
    let Expression::String(token) = expr else {
        return None;
    };
    match token.token_type() {
        TokenType::StringLiteral { literal, .. } => Some(literal.to_string()),
        _ => None,
    }
}

struct FfiVisitor<'a> {
    aliases: Vec<String>,
    module_fqdn: &'a str,
    file_path: &'a str,
    symbols: Vec<RawSymbol>,
    bindings: Vec<RawFfiBinding>,
    seen_names: std::collections::HashSet<String>,
}

impl Visitor for FfiVisitor<'_> {
    fn visit_function_call(&mut self, fc: &FunctionCall) {
        let Some(cdef_body) = self.match_cdef_call(fc) else {
            return;
        };
        let location = self.fc_location(fc);
        for name in extract_function_names_from_cdef(&cdef_body) {
            if !self.seen_names.insert(name.clone()) {
                continue;
            }
            let fqdn = format!("{}::{}", self.module_fqdn, name);
            self.symbols.push(RawSymbol {
                decl_kind: None,
                implements_trait: None,
                receiver_type: None,
                name: name.clone(),
                fqdn: fqdn.clone(),
                kind: Kind::Value,
                language_kind: LanguageKind::from(FFI_IMPORT_LANGUAGE_KIND),
                module: Some(self.module_fqdn.to_owned()),
                visibility: Visibility::Private,
                location: location.clone(),
                signature: None as Option<Signature>,
                body_hash: None,
                attributes: vec![],
                flags: vec!["ffi-import".to_owned()],
            });
            self.bindings.push(RawFfiBinding {
                symbol_fqdn: fqdn,
                abi: FfiAbi::C,
                direction: FfiDirection::Import,
                abi_name: name,
                convention: Some(CONVENTION_LUAJIT_FFI.to_owned()),
            });
        }
    }
}

impl FfiVisitor<'_> {
    /// Returns the cdef body string when `fc` is `<alias>.cdef(<str>)`
    /// for any known ffi alias. `None` otherwise.
    fn match_cdef_call(&self, fc: &FunctionCall) -> Option<String> {
        let Prefix::Name(name) = fc.prefix() else {
            return None;
        };
        let prefix_text = name.token().to_string();
        if !self.aliases.iter().any(|a| *a == prefix_text) {
            return None;
        }
        let mut suffixes = fc.suffixes();
        // First suffix: `.cdef` index.
        let Some(Suffix::Index(index)) = suffixes.next() else {
            return None;
        };
        if !index_is(index, "cdef") {
            return None;
        }
        // Second suffix: the call carrying the C declarations.
        let Some(Suffix::Call(call)) = suffixes.next() else {
            return None;
        };
        // Reject anything after the call — chained suffixes here would
        // be unusual and unsafe to interpret as a cdef.
        if suffixes.next().is_some() {
            return None;
        }
        let Call::AnonymousCall(args) = call else {
            return None;
        };
        string_arg_value(args)
    }

    fn fc_location(&self, fc: &FunctionCall) -> SymbolLocation {
        let start = call_start_position(fc);
        let line = start.map(|(l, _c)| l).unwrap_or(1);
        let col = start.map(|(_l, c)| c).unwrap_or(0);
        SymbolLocation {
            file: self.file_path.to_owned(),
            start_line: line,
            start_col: col,
            end_line: line,
            end_col: col,
        }
    }
}

fn index_is(index: &full_moon::ast::Index, target: &str) -> bool {
    match index {
        full_moon::ast::Index::Dot { name, .. } => name.token().to_string() == target,
        full_moon::ast::Index::Brackets { .. } => false,
        _ => false,
    }
}

fn call_start_position(fc: &FunctionCall) -> Option<(u32, u32)> {
    let Prefix::Name(name) = fc.prefix() else {
        return None;
    };
    let pos = name.token().start_position();
    Some((u32::try_from(pos.line()).unwrap_or(1), {
        // full_moon columns are 1-based; SymbolLocation columns are 0-
        // based half-open per existing helpers, so subtract one when
        // safe.
        let c = pos.character();
        u32::try_from(c.saturating_sub(1)).unwrap_or(0)
    }))
}

/// Parse the cdef body as a C translation unit and return the names
/// of every function-prototype declaration. Non-function declarations
/// (typedefs, globals, struct decls) are dropped silently. The same
/// `tree-sitter-c` grammar the C provider uses is loaded here so the
/// recognition set is identical.
fn extract_function_names_from_cdef(cdef: &str) -> Vec<String> {
    let mut parser = Parser::new();
    let language = tree_sitter_c::LANGUAGE.into();
    if parser.set_language(&language).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(cdef, None) else {
        return Vec::new();
    };
    let root = tree.root_node();
    let mut out: Vec<String> = Vec::new();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        // Top-level cdef content can appear under `declaration` or
        // `linkage_specification` (rare in cdef bodies, but valid).
        match child.kind() {
            "declaration" => {
                if let Some(name) = fn_name_from_declaration(child, cdef) {
                    out.push(name);
                }
            }
            "linkage_specification" => {
                let mut inner = child.walk();
                for grand in child.named_children(&mut inner) {
                    if grand.kind() == "declaration"
                        && let Some(name) = fn_name_from_declaration(grand, cdef)
                    {
                        out.push(name);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn fn_name_from_declaration(node: Node, src: &str) -> Option<String> {
    let declarator = node.child_by_field_name("declarator")?;
    let inner = unwrap_function_declarator(declarator)?;
    let id = name_from_declarator(inner, src)?;
    Some(id.to_owned())
}

fn unwrap_function_declarator(node: Node) -> Option<Node> {
    let mut current = node;
    loop {
        match current.kind() {
            "function_declarator" => return Some(current),
            "pointer_declarator" | "array_declarator" | "parenthesized_declarator" => {
                current = current.child_by_field_name("declarator")?;
            }
            _ => return None,
        }
    }
}

fn name_from_declarator<'a>(node: Node, src: &'a str) -> Option<&'a str> {
    let inner = node.child_by_field_name("declarator")?;
    let mut current = inner;
    loop {
        match current.kind() {
            "identifier" | "field_identifier" | "type_identifier" => {
                return current.utf8_text(src.as_bytes()).ok();
            }
            "parenthesized_declarator" => {
                let mut cursor = current.walk();
                let first = current.named_children(&mut cursor).next()?;
                current = first;
            }
            "pointer_declarator" | "function_declarator" | "array_declarator" => {
                current = current.child_by_field_name("declarator")?;
            }
            _ => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &str) -> (Vec<RawSymbol>, Vec<RawFfiBinding>) {
        let ast = full_moon::parse(src).expect("parse lua");
        extract_ffi_bindings(&ast, "pkg::test", "test.lua")
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
}

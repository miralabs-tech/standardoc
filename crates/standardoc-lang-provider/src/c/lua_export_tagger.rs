//! Stage 2 follow-up — C-side Lua-export FFI tagger.
//!
//! Detects the Lua C API pattern that exposes a C function to the Lua
//! VM under a Lua-visible name:
//!
//!   ```c
//!   void register_module(lua_State* L) {
//!       lua_register(L, "add", lua_add);
//!       lua_register(L, "log_line", lua_log_line);
//!   }
//!   ```
//!
//! For each `lua_register` call we emit one
//! `RawFfiBinding { abi: FfiAbi::Lua, direction: Export, abi_name: "add",
//! convention: "lua-register" }` on the C-implementation symbol
//! (`lua_add` / `lua_log_line` in the example). The binding attaches
//! to a `RawSymbol` produced earlier by `emit_function_definition`
//! when the implementation lives in the same translation unit.
//!
//! `luaL_register` / `luaL_setfuncs` populate an entire `luaL_Reg[]`
//! array at once. The array's contents live in a separate
//! declaration the walker would need to cross-reference, so they
//! are intentionally OUT of scope for v1. The Lua-side cdef path
//! (G6) already covers the dual case where Lua imports via
//! `ffi.cdef` + `ffi.load`, which is independent of `luaL_*`.

use standardoc_ir::{FfiAbi, FfiDirection, RawFfiBinding};
use tree_sitter::Node;

use super::helpers::node_text;

/// Convention slug stamped on every binding produced by this tagger.
const CONVENTION_LUA_REGISTER: &str = "lua-register";

/// The C function name the tagger pattern-matches on. Aliases /
/// macros that wrap `lua_register` are deliberately not chased — the
/// surface stays narrow until the viz starts surfacing these edges
/// and we learn what additional shapes are worth catching.
const LUA_REGISTER_CALLEE: &str = "lua_register";

/// Walk every named descendant of `body` and emit one
/// `RawFfiBinding` per matched `lua_register(L, "name", c_fn)` call.
/// `module_fqdn` anchors the produced FQDNs at the file's module
/// FQDN — this matches the convention `emit_function_definition`
/// uses when emitting the C-impl symbol, so the `apply_ffi_bindings`
/// FQDN→`symbols.id` resolution lands on the same row.
pub(crate) fn extract_lua_exports(
    body: Node,
    src: &str,
    module_fqdn: &str,
    bindings: &mut Vec<RawFfiBinding>,
) {
    visit(body, src, module_fqdn, bindings);
}

fn visit(node: Node, src: &str, module_fqdn: &str, bindings: &mut Vec<RawFfiBinding>) {
    if node.kind() == "call_expression" {
        if let Some(binding) = try_match_lua_register(node, src, module_fqdn) {
            bindings.push(binding);
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        visit(child, src, module_fqdn, bindings);
    }
}

fn try_match_lua_register(call: Node, src: &str, module_fqdn: &str) -> Option<RawFfiBinding> {
    let callee = call.child_by_field_name("function")?;
    if callee.kind() != "identifier" || node_text(callee, src) != LUA_REGISTER_CALLEE {
        return None;
    }
    let args = call.child_by_field_name("arguments")?;
    // `lua_register(L, "name", c_impl)` — exactly three positional
    // args. A wrapper macro that adds extra args would parse with a
    // different shape; bail rather than guess.
    let named_args: Vec<Node> = {
        let mut cursor = args.walk();
        args.named_children(&mut cursor).collect()
    };
    if named_args.len() != 3 {
        return None;
    }
    let name_arg = named_args[1];
    let impl_arg = named_args[2];

    let abi_name = string_literal_value(name_arg, src)?;
    let impl_name = identifier_text(impl_arg, src)?;

    Some(RawFfiBinding {
        symbol_fqdn: format!("{module_fqdn}::{impl_name}"),
        abi: FfiAbi::Lua,
        direction: FfiDirection::Export,
        abi_name,
        convention: Some(CONVENTION_LUA_REGISTER.to_owned()),
    })
}

/// Returns the inner text of a C string literal, stripping the
/// surrounding double-quotes. Returns `None` for anything that isn't
/// a plain `string_literal` (concatenated literals, escape sequences
/// embedded via macros, identifier-keyed lookups).
fn string_literal_value(node: Node, src: &str) -> Option<String> {
    if node.kind() != "string_literal" {
        return None;
    }
    let raw = node_text(node, src);
    let trimmed = raw.trim_start_matches('"').trim_end_matches('"');
    Some(trimmed.to_owned())
}

fn identifier_text<'a>(node: Node, src: &'a str) -> Option<&'a str> {
    if node.kind() != "identifier" {
        return None;
    }
    Some(node_text(node, src))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    fn run(body_src: &str) -> Vec<RawFfiBinding> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_c::LANGUAGE.into())
            .expect("c grammar");
        let wrapped = format!("void wrapper(void) {{ {body_src} }}");
        let tree = parser.parse(&wrapped, None).expect("parse");
        let root = tree.root_node();
        let fn_def = root
            .child(0)
            .expect("function_definition");
        assert_eq!(fn_def.kind(), "function_definition");
        let body = fn_def
            .child_by_field_name("body")
            .expect("compound_statement");
        let mut out = Vec::new();
        extract_lua_exports(body, &wrapped, "pkg::a", &mut out);
        out
    }

    #[test]
    fn lua_register_call_emits_export_binding() {
        let out = run("lua_register(L, \"add\", c_add);");
        assert_eq!(out.len(), 1);
        let b = &out[0];
        assert_eq!(b.symbol_fqdn, "pkg::a::c_add");
        assert_eq!(b.abi, FfiAbi::Lua);
        assert_eq!(b.direction, FfiDirection::Export);
        assert_eq!(b.abi_name, "add");
        assert_eq!(b.convention.as_deref(), Some("lua-register"));
    }

    #[test]
    fn multiple_lua_register_calls_emit_one_binding_each() {
        let out = run(
            "lua_register(L, \"add\", c_add); \
             lua_register(L, \"sub\", c_sub);",
        );
        let names: Vec<&str> = out.iter().map(|b| b.abi_name.as_str()).collect();
        assert_eq!(names, vec!["add", "sub"]);
        let impls: Vec<&str> = out.iter().map(|b| b.symbol_fqdn.as_str()).collect();
        assert_eq!(impls, vec!["pkg::a::c_add", "pkg::a::c_sub"]);
    }

    #[test]
    fn lua_register_nested_in_control_flow_is_captured() {
        let out = run(
            "if (cond) { lua_register(L, \"a\", a_fn); } \
             else { lua_register(L, \"b\", b_fn); }",
        );
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn lua_register_with_non_string_name_is_skipped() {
        // Macro-defined name token: `lua_register(L, NAME, c_fn)` —
        // we can't statically extract the name. Bail rather than
        // emit a binding with an identifier text as `abi_name`.
        let out = run("lua_register(L, NAME, c_fn);");
        assert!(out.is_empty());
    }

    #[test]
    fn lua_register_with_non_identifier_impl_is_skipped() {
        // `lua_register(L, "x", &c_fn)` — the impl arg is a
        // pointer_expression, not a bare identifier. The
        // convention is bare-identifier fn pointers; addressed-of
        // forms are rare in practice and worth skipping rather
        // than emitting a wrong FQDN.
        let out = run("lua_register(L, \"x\", &c_fn);");
        assert!(out.is_empty());
    }

    #[test]
    fn other_three_arg_calls_are_not_misidentified() {
        // Same shape as lua_register (3 args, identifier-first arg,
        // string lit second, identifier third) but a different
        // callee. The callee-name check must filter these out.
        let out = run("not_lua_register(L, \"add\", c_add);");
        assert!(out.is_empty());
    }

    #[test]
    fn lua_register_with_wrong_arity_is_skipped() {
        // 4 args — would be a wrapper macro variant; bail rather
        // than guess which slot carries the name / impl.
        let out = run("lua_register(L, \"add\", c_add, extra);");
        assert!(out.is_empty());
    }
}

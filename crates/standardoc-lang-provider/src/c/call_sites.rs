//! Intra-function call-site collection for the C provider.
//!
//! Tree-sitter native — walks the `compound_statement` subtree of a
//! `function_definition` and emits one `RawCallSite` per
//! `call_expression` node. The walk is recursive through every named
//! descendant so calls nested inside `if` / `for` / `while` / `switch`
//! bodies and inside argument expressions of an outer call all surface.
//!
//! Macro invocations (`printf(...)`, `assert(...)`, user-defined
//! `MY_MACRO(arg)`) parse as `call_expression` in tree-sitter-c and
//! are captured uniformly — the plugin layer that consumes call_sites
//! treats them like regular calls and dedups against `#define` macros
//! in the symbol table.
//!
//! Function-pointer calls (`(*fp)(x)`, `cb->handler(x)`) parse with a
//! `parenthesized_expression` / `field_expression` in the `function`
//! field; the callee text is the verbatim source slice so the consumer
//! can later pattern-match on `->handler`, `(*fp)`, etc.

use standardoc_ir::{RawCallArg, RawCallSite, Site};
use tree_sitter::Node;

use super::helpers::{col_utf16, node_text};
use crate::walk_core::WalkContextCore;

/// Walk every named descendant of `body` and emit a `RawCallSite` for
/// each `call_expression` encountered. `enclosing_fqdn` is stamped on
/// every site so the plugin layer can filter by caller without a
/// separate symbol-table lookup.
///
/// The descent does NOT short-circuit on `call_expression`: nested
/// calls in argument positions (`foo(bar())`) emit both `foo` and
/// `bar` as distinct call_sites.
pub(crate) fn emit_intra_fn_call_sites(
    body: Node,
    src: &str,
    ctx: &mut WalkContextCore,
    enclosing_fqdn: &str,
) {
    visit(body, src, ctx, enclosing_fqdn);
}

fn visit(node: Node, src: &str, ctx: &mut WalkContextCore, enclosing_fqdn: &str) {
    if node.kind() == "call_expression" {
        emit_one(node, src, ctx, enclosing_fqdn);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        visit(child, src, ctx, enclosing_fqdn);
    }
}

fn emit_one(node: Node, src: &str, ctx: &mut WalkContextCore, enclosing_fqdn: &str) {
    let Some(callee_node) = node.child_by_field_name("function") else {
        return;
    };
    let callee_text = node_text(callee_node, src).to_owned();
    if callee_text.is_empty() {
        return;
    }
    let receiver_chain = receiver_chain_of(callee_node, src);
    let args = node
        .child_by_field_name("arguments")
        .map_or_else(Vec::new, |list| args_of(list, src));
    let start = node.start_position();
    ctx.push_call_site(RawCallSite {
        from_fqdn: enclosing_fqdn.to_owned(),
        callee_text,
        args,
        receiver_chain,
        site: Site {
            file: ctx.file_path.clone(),
            line: u32::try_from(start.row + 1).unwrap_or(u32::MAX),
            col: col_utf16(src, start.row, start.column),
        },
    });
}

/// For `obj.field.method(...)` / `obj->api->method(...)` chains, walk
/// the `field_expression` argument side and collect the chain segments
/// (outermost first). Empty when the callee is a bare identifier,
/// parenthesized expression, or anything else without a clear
/// owning receiver.
fn receiver_chain_of(callee: Node, src: &str) -> Vec<String> {
    if callee.kind() != "field_expression" {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    let mut cursor = callee;
    while cursor.kind() == "field_expression" {
        let Some(arg) = cursor.child_by_field_name("argument") else {
            break;
        };
        if arg.kind() == "field_expression" {
            let Some(field) = arg.child_by_field_name("field") else {
                break;
            };
            out.push(node_text(field, src).to_owned());
            cursor = arg;
        } else {
            out.push(node_text(arg, src).to_owned());
            break;
        }
    }
    out.reverse();
    out
}

fn args_of(list: Node, src: &str) -> Vec<RawCallArg> {
    let mut cursor = list.walk();
    list.named_children(&mut cursor)
        .map(|child| RawCallArg {
            value: node_text(child, src).to_owned(),
            is_string_literal: child.kind() == "string_literal",
        })
        .collect()
}

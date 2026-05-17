use standardoc_ir::{
    EdgeKind, FfiAbi, FfiDirection, Kind, Language, LanguageKind, RawEdge, RawFfiBinding,
    RawSymbol, ResolvedOrUnresolved, Site, SymbolLocation, Visibility,
};
use tree_sitter::{Node, TreeCursor};

use crate::utils::{hash_bytes, parent_module};
use crate::walk_core::WalkContextCore;

use super::helpers::{
    declaration_is_function_prototype, declarator_name, location_from_node, node_text,
    storage_class_is_static,
};

/// Per-file walker state for the C provider.
///
// TODO(viz-readiness-G4-c): no `build_c_lookup` AOT pass yet. C symbols
// can't currently be the target of cross-workspace `ModuleLookup`
// resolution from peer workspaces. FFI bindings remain the only
// cross-language bridge for C. If C-side symbols need to be addressable
// from peer workspaces via ModuleLookup (rather than only via FFI), add
// a `c::lookup` module mirroring `rust::lookup` / `ts::lookup` and wire
// it through `extract_file` so `ExtractedFile.module_lookup` is `Some`.
pub(crate) struct CWalkContext {
    pub(crate) core: WalkContextCore,
    pub(crate) ffi_bindings: Vec<RawFfiBinding>,
}

impl CWalkContext {
    pub(crate) fn new(file_path: String, file_module_fqdn: String) -> Self {
        Self {
            core: WalkContextCore::new(file_path, file_module_fqdn, Language::C),
            ffi_bindings: Vec::new(),
        }
    }
}

/// Walk a parsed tree-sitter C tree from the root `translation_unit`,
/// emitting symbols into `ctx`. Top-level only — descends into
/// `preproc_ifdef` / `preproc_if` / `linkage_specification` blocks so
/// guarded content in headers is still indexed.
pub(crate) fn walk_translation_unit(root: Node, src: &str, ctx: &mut CWalkContext) {
    let mut cursor = root.walk();
    walk_block_children(&mut cursor, src, ctx);
}

fn walk_block_children(cursor: &mut TreeCursor, src: &str, ctx: &mut CWalkContext) {
    let parent = cursor.node();
    let mut walker = parent.walk();
    for child in parent.children(&mut walker) {
        visit_top_level(child, src, ctx);
    }
}

fn visit_top_level(node: Node, src: &str, ctx: &mut CWalkContext) {
    match node.kind() {
        "function_definition" => emit_function_definition(node, src, ctx),
        "declaration" => emit_declaration(node, src, ctx),
        "struct_specifier" => emit_struct_like(node, src, ctx, "struct"),
        "union_specifier" => emit_struct_like(node, src, ctx, "union"),
        "enum_specifier" => emit_enum(node, src, ctx),
        "type_definition" => emit_typedef(node, src, ctx),
        "preproc_def" => emit_macro_object(node, src, ctx),
        "preproc_function_def" => emit_macro_fn(node, src, ctx),
        "preproc_include" => emit_include(node, src, ctx),
        "preproc_ifdef"
        | "preproc_if"
        | "preproc_else"
        | "preproc_elif"
        | "linkage_specification" => {
            // Recurse into guarded / extern "C" bodies so their contents
            // are still indexed at top-level granularity.
            let mut cursor = node.walk();
            walk_block_children(&mut cursor, src, ctx);
        }
        _ => {}
    }
}

fn emit_include(node: Node, src: &str, ctx: &mut CWalkContext) {
    let Some(path_node) = node.child_by_field_name("path") else {
        return;
    };
    let raw = node_text(path_node, src);
    let target = match path_node.kind() {
        // `<stdio.h>` → builtin tier. Strip `<>` and `.h`.
        "system_lib_string" => {
            let inner = raw.trim_start_matches('<').trim_end_matches('>');
            let stem = inner.strip_suffix(".h").unwrap_or(inner);
            ResolvedOrUnresolved::Resolved {
                fqdn: format!("<builtin>::c::{stem}"),
            }
        }
        // `"foo.h"` → unresolved by name (storage layer matches via
        // closest-fqdn suffix on the file's module FQDN). Strip the
        // surrounding quotes; keep the rest verbatim so suffix matching
        // works against paths like `runtime/util.h` later.
        "string_literal" => {
            let inner = raw.trim_start_matches('"').trim_end_matches('"');
            ResolvedOrUnresolved::Unresolved {
                name: inner.to_string(),
            }
        }
        _ => return,
    };

    let start = node.start_position();
    let site = Site {
        file: ctx.core.file_path.clone(),
        line: u32::try_from(start.row + 1).unwrap_or(u32::MAX),
        col: u32::try_from(start.column).unwrap_or(u32::MAX),
    };
    let confidence = target.default_confidence();
    ctx.core.push_edge(RawEdge {
        from_fqdn: ctx.core.file_module_fqdn.clone(),
        kind: EdgeKind::Imports,
        to: target,
        sites: vec![site],
        attributes: vec![],
        confidence,
    });
}

// TODO(viz-readiness-G2): emit `RawCallSite` for tree-sitter
// `call_expression` nodes found inside this function's body so the C
// intra-fn call graph materialises in the viz. Today the body is walked
// only for FFI binding emission (export side) — calls between C
// functions stay invisible to `find_call_sites` / graph payloads.
// Needs a `CallVisitor`-style descent through the `compound_statement`
// child, collecting `call_expression` -> RawCallSite { from_fqdn,
// callee_text, args, site } via `ctx.core.push_call_site`.
fn emit_function_definition(node: Node, src: &str, ctx: &mut CWalkContext) {
    let Some(declarator) = node.child_by_field_name("declarator") else {
        return;
    };
    let Some(name) = declarator_name(declarator, src) else {
        return;
    };
    let is_static = storage_class_is_static(node, src);
    let visibility = if is_static {
        Visibility::Private
    } else {
        Visibility::Public
    };
    let body_hash = node
        .child_by_field_name("body")
        .map(|b| hash_bytes(node_text(b, src).as_bytes()));

    let fqdn = format!("{}::{}", ctx.core.file_module_fqdn, name);
    push_symbol(
        ctx,
        name,
        Kind::Function,
        LanguageKind::from("fn"),
        visibility,
        location_from_node(&ctx.core.file_path.clone(), node),
        body_hash,
    );

    // Stage 2 — C ABI export. Every non-`static` function in a C
    // translation unit is exposed to the linker under its source name.
    // `static` functions stay file-local and are intentionally NOT
    // tagged: they cannot participate in cross-language FFI by C ABI
    // semantics.
    if !is_static {
        ctx.ffi_bindings.push(RawFfiBinding {
            symbol_fqdn: fqdn,
            abi: FfiAbi::C,
            direction: FfiDirection::Export,
            abi_name: name.to_string(),
            convention: None,
        });
    }
}

fn emit_declaration(node: Node, src: &str, ctx: &mut CWalkContext) {
    // A top-level declaration in C is either:
    //   * a function prototype (`void foo(int);`) → emit as fn_decl
    //   * a global variable (`extern int g;`, `static const int x = 0;`) → emit as global
    //   * a struct/union/enum forward-decl with no declarator → handled by emit_struct_like
    //     elsewhere; skip here.
    let Some(declarator) = node.child_by_field_name("declarator") else {
        return;
    };
    let Some(name) = declarator_name(declarator, src) else {
        return;
    };
    let is_static = storage_class_is_static(node, src);
    let visibility = if is_static {
        Visibility::Private
    } else {
        Visibility::Public
    };
    let loc = location_from_node(&ctx.core.file_path.clone(), node);

    if declaration_is_function_prototype(node) {
        push_symbol(
            ctx,
            name,
            Kind::Function,
            LanguageKind::from("fn_decl"),
            visibility,
            loc,
            None,
        );
        // Stage 2 — a `.h` prototype that does NOT match a `.c`
        // definition in the same workspace is an Import (the linker
        // expects the symbol to come from somewhere else: a sibling
        // language's compilation unit, a system library, …). The
        // post-extraction `c_join` pass deletes the fn_decl row when
        // a local match exists — its FFI binding cascades away with
        // it. Surviving fn_decl rows therefore correctly stand as
        // imports at resolve time.
        if !is_static {
            let fqdn = format!("{}::{}", ctx.core.file_module_fqdn, name);
            ctx.ffi_bindings.push(RawFfiBinding {
                symbol_fqdn: fqdn,
                abi: FfiAbi::C,
                direction: FfiDirection::Import,
                abi_name: name.to_string(),
                convention: None,
            });
        }
    } else {
        push_symbol(
            ctx,
            name,
            Kind::Value,
            LanguageKind::from("global"),
            visibility,
            loc,
            None,
        );
    }
}

fn emit_struct_like(node: Node, src: &str, ctx: &mut CWalkContext, lang_kind: &str) {
    // Anonymous structs/unions (no `name` field) have no FQDN of their own —
    // they only exist as part of a typedef, which is handled by emit_typedef.
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = node_text(name_node, src);
    push_symbol(
        ctx,
        name,
        Kind::Type,
        LanguageKind::from(lang_kind),
        Visibility::Public,
        location_from_node(&ctx.core.file_path.clone(), node),
        None,
    );
}

fn emit_enum(node: Node, src: &str, ctx: &mut CWalkContext) {
    let enum_name_opt = node
        .child_by_field_name("name")
        .map(|n| node_text(n, src).to_string());

    // Push the enum itself when named (anonymous enums are emitted only as
    // their enumerator sub-symbols at typedef level).
    let parent_fqdn = if let Some(ref enum_name) = enum_name_opt {
        let enum_fqdn = format!("{}::{}", ctx.core.file_module_fqdn, enum_name);
        ctx.core.push_symbol(RawSymbol {
            name: enum_name.clone(),
            fqdn: enum_fqdn.clone(),
            kind: Kind::Type,
            language_kind: LanguageKind::from("enum"),
            module: Some(ctx.core.file_module_fqdn.clone()),
            visibility: Visibility::Public,
            location: location_from_node(&ctx.core.file_path, node),
            signature: None,
            body_hash: None,
            attributes: vec![],
            flags: vec![],
        });
        enum_fqdn
    } else {
        return;
    };

    // Sub-symbols per enumerator (cf. Bug C-3 granularity).
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        if child.kind() != "enumerator" {
            continue;
        }
        let Some(name_node) = child.child_by_field_name("name") else {
            continue;
        };
        let name = node_text(name_node, src).to_string();
        let fqdn = format!("{parent_fqdn}::{name}");
        ctx.core.push_symbol(RawSymbol {
            name,
            fqdn,
            kind: Kind::Value,
            language_kind: LanguageKind::from("enum_variant"),
            module: Some(parent_fqdn.clone()),
            visibility: Visibility::Public,
            location: location_from_node(&ctx.core.file_path, child),
            signature: None,
            body_hash: None,
            attributes: vec![],
            flags: vec![],
        });
    }
}

fn emit_typedef(node: Node, src: &str, ctx: &mut CWalkContext) {
    // typedef shape: `typedef <type> <declarator> ;`
    // The bound name is in `declarator` (can be a function_declarator for
    // function pointer typedefs).
    let Some(declarator) = node.child_by_field_name("declarator") else {
        return;
    };
    let Some(name) = declarator_name(declarator, src) else {
        return;
    };
    push_symbol(
        ctx,
        name,
        Kind::Type,
        LanguageKind::from("type_alias"),
        Visibility::Public,
        location_from_node(&ctx.core.file_path.clone(), node),
        None,
    );

    // If the typedef wraps an inline `enum { ... }`, emit its enumerators
    // under the typedef name so users can reach them via FQDN.
    if let Some(type_node) = node.child_by_field_name("type")
        && type_node.kind() == "enum_specifier"
        && let Some(body) = type_node.child_by_field_name("body")
    {
        let parent_fqdn = format!("{}::{}", ctx.core.file_module_fqdn, name);
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            if child.kind() != "enumerator" {
                continue;
            }
            let Some(var_name_node) = child.child_by_field_name("name") else {
                continue;
            };
            let var_name = node_text(var_name_node, src).to_string();
            let fqdn = format!("{parent_fqdn}::{var_name}");
            ctx.core.push_symbol(RawSymbol {
                name: var_name,
                fqdn,
                kind: Kind::Value,
                language_kind: LanguageKind::from("enum_variant"),
                module: Some(parent_fqdn.clone()),
                visibility: Visibility::Public,
                location: location_from_node(&ctx.core.file_path, child),
                signature: None,
                body_hash: None,
                attributes: vec![],
                flags: vec![],
            });
        }
    }
}

fn emit_macro_object(node: Node, src: &str, ctx: &mut CWalkContext) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = node_text(name_node, src);
    push_symbol(
        ctx,
        name,
        Kind::Macro,
        LanguageKind::from("macro_object"),
        Visibility::Public,
        location_from_node(&ctx.core.file_path.clone(), node),
        None,
    );
}

fn emit_macro_fn(node: Node, src: &str, ctx: &mut CWalkContext) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = node_text(name_node, src);
    push_symbol(
        ctx,
        name,
        Kind::Macro,
        LanguageKind::from("macro_fn"),
        Visibility::Public,
        location_from_node(&ctx.core.file_path.clone(), node),
        None,
    );
}

#[allow(clippy::too_many_arguments)]
fn push_symbol(
    ctx: &mut CWalkContext,
    name: &str,
    kind: Kind,
    language_kind: LanguageKind,
    visibility: Visibility,
    location: SymbolLocation,
    body_hash: Option<standardoc_ir::Blake3Hash>,
) {
    let fqdn = format!("{}::{}", ctx.core.file_module_fqdn, name);
    let module = parent_module(&fqdn);
    ctx.core.push_symbol(RawSymbol {
        name: name.to_string(),
        fqdn,
        kind,
        language_kind,
        module,
        visibility,
        location,
        signature: None,
        body_hash,
        attributes: vec![],
        flags: vec![],
    });
}

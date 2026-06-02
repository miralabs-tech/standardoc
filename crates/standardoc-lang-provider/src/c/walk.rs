use standardoc_ir::{
    DeclKind, EdgeKind, EntryPointKind, FfiAbi, FfiDirection, Kind, Language, LanguageKind,
    RawEdge, RawFfiBinding, RawSymbol, ResolvedOrUnresolved, Site, SymbolLocation, Visibility,
};
use tree_sitter::{Node, TreeCursor};

use std::collections::HashMap;

use crate::utils::{hash_bytes, parent_module};
use crate::walk_core::WalkContextCore;

use super::helpers::{
    declaration_is_function_prototype, declarator_name, location_from_node, node_text,
    storage_class_is_static,
};

/// Per-file walker state for the C provider.
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
        "struct_specifier" => emit_struct_like(node, src, ctx, "struct", DeclKind::Struct),
        "union_specifier" => emit_struct_like(node, src, ctx, "union", DeclKind::Union),
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
        // `<stdio.h>` → builtin tier. Strip `<>` but keep `.h` so the
        // header namespace stays distinct from same-named functions
        // (`<time.h>` vs `time()`); see `builtins::c::register_all`.
        "system_lib_string" => {
            let inner = raw.trim_start_matches('<').trim_end_matches('>');
            ResolvedOrUnresolved::Resolved {
                fqdn: format!("<builtin>::c::{inner}"),
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
        receiver_type: None,
    });
}

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
    let body_node = node.child_by_field_name("body");
    let body_hash = body_node.map(|b| hash_bytes(node_text(b, src).as_bytes()));

    let fqdn = format!("{}::{}", ctx.core.file_module_fqdn, name);
    let entry_point = classify_c_fn_entry_point(name, is_static);
    push_symbol_with_entry(
        ctx,
        name,
        Kind::Callable,
        LanguageKind::from("fn"),
        DeclKind::Function,
        visibility,
        location_from_node(&ctx.core.file_path.clone(), node),
        body_hash,
        entry_point,
    );

    // Stage 2 — C ABI export. Every non-`static` function in a C
    // translation unit is exposed to the linker under its source name.
    // `static` functions stay file-local and are intentionally NOT
    // tagged: they cannot participate in cross-language FFI by C ABI
    // semantics.
    if !is_static {
        ctx.ffi_bindings.push(RawFfiBinding {
            symbol_fqdn: fqdn.clone(),
            abi: FfiAbi::C,
            direction: FfiDirection::Export,
            abi_name: name.to_string(),
            convention: None,
        });
    }

    // G2 — observational call_sites for the plugin layer / viz.
    // Walks every `call_expression` inside the function body
    // (compound_statement) and pushes one RawCallSite per call. Macro
    // invocations parse as call_expression in tree-sitter-c and are
    // captured uniformly — dedup against `#define` rows is the
    // consumer's job.
    if let Some(body) = body_node {
        super::call_sites::emit_intra_fn_call_sites(body, src, &mut ctx.core, &fqdn);
        // G6 follow-up — Lua C API export tagger. Walks the same
        // body for `lua_register(L, "name", c_impl)` calls and
        // emits one `RawFfiBinding { abi: Lua, direction: Export }`
        // per match, attached to the C-impl symbol. Closes the
        // Lua-side cdef import bridge (G6) on the C export side.
        super::lua_export_tagger::extract_lua_exports(
            body,
            src,
            &ctx.core.file_module_fqdn,
            &mut ctx.ffi_bindings,
        );
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
            Kind::Callable,
            LanguageKind::from("fn_decl"),
            DeclKind::Function,
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
            DeclKind::Static,
            visibility,
            loc,
            None,
        );
    }
}

fn emit_struct_like(
    node: Node,
    src: &str,
    ctx: &mut CWalkContext,
    lang_kind: &str,
    decl_kind: DeclKind,
) {
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
        decl_kind,
        Visibility::Public,
        location_from_node(&ctx.core.file_path.clone(), node),
        None,
    );

    // Field granularity (cf. Bug C-3 for Rust): one Field sub-symbol per
    // named member, parented at the struct/union FQDN.
    if let Some(body) = node.child_by_field_name("body") {
        let parent_fqdn = format!("{}::{}", ctx.core.file_module_fqdn, name);
        emit_aggregate_fields(body, src, ctx, &parent_fqdn);
    }
}

/// Emit one `DeclKind::Field` sub-symbol per named member of a
/// struct/union `field_declaration_list`. `int x, y;` yields two
/// fields (one declarator each). Anonymous members (a nested
/// `struct {...}` / `union {...}` with no field name) have no FQDN of
/// their own and are skipped — flattening their inner fields up into
/// the parent is deferred.
fn emit_aggregate_fields(body: Node, src: &str, ctx: &mut CWalkContext, parent_fqdn: &str) {
    let file = ctx.core.file_path.clone();
    let mut collected: Vec<(String, SymbolLocation)> = Vec::new();
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        if child.kind() != "field_declaration" {
            continue;
        }
        let mut dcursor = child.walk();
        for decl in child.children_by_field_name("declarator", &mut dcursor) {
            if let Some(name) = declarator_name(decl, src) {
                collected.push((name.to_string(), location_from_node(&file, decl)));
            }
        }
    }
    for (name, location) in collected {
        let fqdn = format!("{parent_fqdn}::{name}");
        ctx.core.push_symbol(RawSymbol {
            decl_kind: Some(DeclKind::Field),
            implements_trait: None,
            receiver_type: None,
            entry_point: None,
            name,
            fqdn,
            kind: Kind::Value,
            language_kind: LanguageKind::from("field"),
            module: Some(parent_fqdn.to_string()),
            visibility: Visibility::Public,
            location,
            signature: None,
            body_hash: None,
            attributes: vec![],
            flags: vec![],
        });
    }
}

/// Emit one `DeclKind::EnumVariant` sub-symbol per enumerator in an
/// `enumerator_list` body, parented at `parent_fqdn`. Shared by the
/// named-enum and typedef-enum paths.
fn emit_enumerators(body: Node, src: &str, ctx: &mut CWalkContext, parent_fqdn: &str) {
    let file = ctx.core.file_path.clone();
    let mut collected: Vec<(String, SymbolLocation)> = Vec::new();
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        if child.kind() != "enumerator" {
            continue;
        }
        let Some(name_node) = child.child_by_field_name("name") else {
            continue;
        };
        collected.push((
            node_text(name_node, src).to_string(),
            location_from_node(&file, child),
        ));
    }
    for (name, location) in collected {
        let fqdn = format!("{parent_fqdn}::{name}");
        ctx.core.push_symbol(RawSymbol {
            decl_kind: Some(DeclKind::EnumVariant),
            implements_trait: None,
            receiver_type: None,
            entry_point: None,
            name,
            fqdn,
            kind: Kind::Value,
            language_kind: LanguageKind::from("enum_variant"),
            module: Some(parent_fqdn.to_string()),
            visibility: Visibility::Public,
            location,
            signature: None,
            body_hash: None,
            attributes: vec![],
            flags: vec![],
        });
    }
}

fn emit_enum(node: Node, src: &str, ctx: &mut CWalkContext) {
    // Push the enum itself when named (anonymous top-level enums are
    // rare and carry no FQDN; their enumerators are only reachable via
    // the typedef path, handled in emit_typedef).
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let enum_name = node_text(name_node, src).to_string();
    let enum_fqdn = format!("{}::{}", ctx.core.file_module_fqdn, enum_name);
    ctx.core.push_symbol(RawSymbol {
        decl_kind: Some(DeclKind::Enum),
        implements_trait: None,
        receiver_type: None,
        entry_point: None,
        name: enum_name,
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

    // Sub-symbols per enumerator (cf. Bug C-3 granularity).
    if let Some(body) = node.child_by_field_name("body") {
        emit_enumerators(body, src, ctx, &enum_fqdn);
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

    let type_node = node.child_by_field_name("type");
    let inline_body = type_node.and_then(|t| t.child_by_field_name("body"));

    // When the typedef wraps an INLINE aggregate (`typedef struct {...}
    // Foo;` / `typedef struct Tag {...} Foo;` and union/enum variants),
    // the typedef name IS that nominal type — emit it with the matching
    // DeclKind so it reads as a struct/union/enum (not a bare alias) and
    // carries its members. `typedef struct Foo Foo;` (a reference, no
    // body) stays a plain TypeAlias, as do scalar typedefs.
    let (lang_kind, decl_kind) = match (type_node.map(|t| t.kind()), inline_body) {
        (Some("struct_specifier"), Some(_)) => ("struct", DeclKind::Struct),
        (Some("union_specifier"), Some(_)) => ("union", DeclKind::Union),
        (Some("enum_specifier"), Some(_)) => ("enum", DeclKind::Enum),
        _ => ("type_alias", DeclKind::TypeAlias),
    };
    push_symbol(
        ctx,
        name,
        Kind::Type,
        LanguageKind::from(lang_kind),
        decl_kind,
        Visibility::Public,
        location_from_node(&ctx.core.file_path.clone(), node),
        None,
    );

    // Emit the inline aggregate's members under the typedef name so
    // users reach them via FQDN (the name they actually reference in
    // source).
    if let (Some(tn), Some(body)) = (type_node, inline_body) {
        let parent_fqdn = format!("{}::{}", ctx.core.file_module_fqdn, name);
        match tn.kind() {
            "struct_specifier" | "union_specifier" => {
                emit_aggregate_fields(body, src, ctx, &parent_fqdn);
            }
            "enum_specifier" => emit_enumerators(body, src, ctx, &parent_fqdn),
            _ => {}
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
        DeclKind::DeclarativeMacro,
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
        DeclKind::DeclarativeMacro,
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
    decl_kind: DeclKind,
    visibility: Visibility,
    location: SymbolLocation,
    body_hash: Option<standardoc_ir::Blake3Hash>,
) {
    push_symbol_with_entry(
        ctx,
        name,
        kind,
        language_kind,
        decl_kind,
        visibility,
        location,
        body_hash,
        None,
    );
}

#[allow(clippy::too_many_arguments)]
fn push_symbol_with_entry(
    ctx: &mut CWalkContext,
    name: &str,
    kind: Kind,
    language_kind: LanguageKind,
    decl_kind: DeclKind,
    visibility: Visibility,
    location: SymbolLocation,
    body_hash: Option<standardoc_ir::Blake3Hash>,
    entry_point: Option<EntryPointKind>,
) {
    let fqdn = format!("{}::{}", ctx.core.file_module_fqdn, name);
    let module = parent_module(&fqdn);
    ctx.core.push_symbol(RawSymbol {
        decl_kind: Some(decl_kind),
        implements_trait: None,
        receiver_type: None,
        entry_point,
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

/// Phase 3 (Flow) — first-pass entry-point classification for C
/// function DEFINITIONS (not prototypes — `.h` declarations are not
/// the actual flow root). Conservative coverage:
///
///   - `BinaryMain`: `main` — the kernel-launched entry of every
///     C binary. We don't check the signature shape (`int(int,
///     char**)` is the convention but freestanding / hosted variants
///     differ); the name at file scope is the canonical marker.
///   - `FfiExport`: `luaopen_*` — the Lua C-API contract for native
///     module loading. `static luaopen_*` would be a code smell but
///     is technically valid; we still tag it as the contract is
///     name-based.
///
/// `PublicApi` for plain non-`static` C functions is **deferred**.
/// Almost every non-static C function in a multi-file project is
/// also non-static for intra-project cross-TU calls — marking them
/// all as entry-points would drown the signal. A future pass can
/// refine this with header-visibility / call-graph analysis.
fn classify_c_fn_entry_point(name: &str, _is_static: bool) -> Option<EntryPointKind> {
    if name == "main" {
        return Some(EntryPointKind::BinaryMain);
    }
    if name.starts_with("luaopen_") {
        return Some(EntryPointKind::FfiExport);
    }
    None
}

/// Resolve the per-function `call_sites` collected during the walk into
/// `EdgeKind::Calls` edges (mirrors the Lua provider's `emit_call_edges`).
/// Without this pass C had call_sites but no CALLS edges, so the focus
/// graph — which draws relations from edges, not call_sites — showed C
/// nodes with no connections.
///
/// A callee whose bare name matches a function / macro DEFINED in this
/// file resolves to it (intra-file). Everything else stays
/// `Unresolved` by bare name so the `.h ↔ .c` join, the FFI resolve
/// pass, and the cross-workspace `resolve_unresolved` sweep can rewrite
/// it once sibling files are in the DB. Member / function-pointer calls
/// (`p->handler`, `(*fp)`) reduce to their trailing field name; when
/// that doesn't match a top-level symbol they remain unresolved (no
/// false positives — they just don't draw an intra-file edge).
pub(crate) fn emit_call_edges(ctx: &mut CWalkContext) {
    let by_name: HashMap<&str, String> = ctx
        .core
        .symbols
        .iter()
        .filter(|s| matches!(s.kind, Kind::Callable | Kind::Macro))
        .map(|s| (s.name.as_str(), s.fqdn.clone()))
        .collect();

    let mut edges: Vec<RawEdge> = Vec::with_capacity(ctx.core.call_sites.len());
    for cs in &ctx.core.call_sites {
        let bare = bare_callee(&cs.callee_text);
        if bare.is_empty() {
            continue;
        }
        let to = match by_name.get(bare) {
            Some(fqdn) => ResolvedOrUnresolved::Resolved { fqdn: fqdn.clone() },
            None => ResolvedOrUnresolved::Unresolved {
                name: bare.to_string(),
            },
        };
        let confidence = to.default_confidence();
        edges.push(RawEdge {
            from_fqdn: cs.from_fqdn.clone(),
            kind: EdgeKind::Calls,
            to,
            sites: vec![cs.site.clone()],
            attributes: vec![],
            confidence,
            receiver_type: None,
        });
    }
    for edge in edges {
        ctx.core.push_edge(edge);
    }
}

/// Reduce a C callee expression to the bare identifier used for symbol
/// lookup: the trailing segment after the final `->` or `.` member
/// access. Bare identifiers pass through unchanged; punctuation-only
/// callees (`(*fp)`) have no clean name and are returned verbatim (they
/// simply won't match any symbol).
fn bare_callee(callee: &str) -> &str {
    let after_arrow = callee.rfind("->").map_or(callee, |i| &callee[i + 2..]);
    after_arrow.rsplit('.').next().unwrap_or(after_arrow).trim()
}

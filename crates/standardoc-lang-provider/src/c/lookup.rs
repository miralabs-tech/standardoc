//! Stage 3 R3 — AOT `ModuleLookup` builder for the C provider.
//!
//! Cross-workspace edge strengthening (`cross_workspace_post`) requires
//! every extracted file to carry a `ModuleLookup` so the peer resolver
//! can answer "does workspace W's module M export symbol S?". Rust and
//! TS build a full scope-aware lookup driven by their walker visitors;
//! C doesn't need that depth because C identifiers don't live in
//! nested lexical scopes the way module-relative imports do.
//!
//! What the resolver actually consults — `resolve_cross_workspace_import`
//! in `standardoc-core::storage::cross_workspace` — is just
//! `lookup.bindings.get(name)` with a `scope_idx == ROOT_SCOPE` filter.
//! The resolved FQDN is synthesised from `{origin_module}::{name}` at
//! lookup time, so `IdentResolution.resolved_fqdn` itself is informational.
//!
//! We therefore build a minimal lookup:
//!
//!   * One root-scope `IdentResolution` per top-level symbol that lives
//!     directly under `module_fqdn` (functions, typedefs, structs,
//!     unions, enums, globals, macros). `static` functions are excluded —
//!     by C ABI semantics a `static` fn is file-local and shouldn't be
//!     reachable from another workspace.
//!   * One `ImportRecord` per `#include` edge, with `local_name` taken
//!     from the include's basename and `origin_module` from the edge's
//!     target FQDN/name. Populates `workspace_imports` so the Stage 3b
//!     cross-workspace SQL join can answer "which TUs include this
//!     header?".
//!
//! Sub-symbols (struct fields, union members) carry a `module`
//! pointing at their parent type and so are excluded. Enum variants
//! emitted at file scope (per the C grammar's `enum Color { RED }`
//! exposing `RED` at file scope) ARE included — see `is_root_level`.

use standardoc_ir::{
    BindingSource, IdentResolution, ImportRecord, Kind, Language, LocalDeclKind, ModuleLookup,
    RawEdge, RawSymbol, ResolvedOrUnresolved, Visibility,
};

/// Build the file-level `ModuleLookup` for a parsed C translation unit.
///
/// `symbols` and `edges` are the walker's outputs from
/// `walk_translation_unit`. `module_fqdn` is the file-level module
/// FQDN (`<package>::<rel/with/separator>`). The returned lookup
/// carries the language tag, the module FQDN, every cross-workspace-
/// addressable binding, and one `ImportRecord` per resolved/unresolved
/// `#include` edge.
pub(crate) fn build_c_lookup(
    symbols: &[RawSymbol],
    edges: &[RawEdge],
    module_fqdn: &str,
) -> ModuleLookup {
    let mut lookup = ModuleLookup::new(module_fqdn.to_string(), Language::C);

    for symbol in symbols {
        if !is_root_level(symbol, module_fqdn) {
            continue;
        }
        if symbol.visibility == Visibility::Private {
            // `static` functions / file-local globals — by C ABI they
            // can't be the target of a cross-workspace import. Leave
            // them out of the bindings table.
            continue;
        }
        let decl_kind = local_decl_kind_for(symbol);
        lookup.push_binding(IdentResolution {
            name: symbol.name.clone(),
            source: BindingSource::LocalDecl { decl_kind },
            resolved_fqdn: Some(symbol.fqdn.clone()),
            aliases_to: None,
            mutability: None,
            scope_idx: ModuleLookup::ROOT_SCOPE,
            attributes: vec![],
            ir_kind: Some(symbol.kind),
        });
    }

    for edge in edges {
        let Some(record) = import_record_from_edge(edge, module_fqdn) else {
            continue;
        };
        lookup.push_import(record);
    }

    lookup
}

/// Returns `true` when `symbol` is a direct top-level child of the
/// file's module FQDN. Excludes sub-symbols (struct fields, union
/// members) which live under their parent type's FQDN, and the file's
/// own module symbol (which lives at the parent module's FQDN).
fn is_root_level(symbol: &RawSymbol, module_fqdn: &str) -> bool {
    if symbol.kind == Kind::Module {
        return false;
    }
    symbol.module.as_deref() == Some(module_fqdn)
}

/// Map an extracted C symbol to the closest matching `LocalDeclKind`.
/// `LanguageKind` carries the C-side discriminant (`fn` / `fn_decl` /
/// `struct` / `union` / `enum` / `typedef` / `global` / `macro_object`
/// / `macro_fn`) — we route on the IR `Kind` first then refine via
/// `language_kind` where the IR is too coarse.
fn local_decl_kind_for(symbol: &RawSymbol) -> LocalDeclKind {
    match symbol.kind {
        Kind::Callable => LocalDeclKind::Function,
        Kind::Module => LocalDeclKind::Module,
        Kind::Macro => LocalDeclKind::Macro,
        Kind::Value => LocalDeclKind::Var,
        Kind::Type => match symbol.language_kind.as_str() {
            "struct" => LocalDeclKind::Struct,
            "enum" => LocalDeclKind::Enum,
            "typedef" => LocalDeclKind::TypeAlias,
            // Union, opaque, and forward-declared types fall here.
            // `Custom` lets the UST escape hatch carry the original
            // language_kind string for consumers that need the
            // distinction.
            other => LocalDeclKind::Custom {
                lang: Language::C,
                tag: other.to_owned(),
            },
        },
    }
}

/// Translate an `EdgeKind::Imports` edge into an `ImportRecord`. C
/// `#include` directives produce these edges via `emit_include` in
/// the walker. The `local_name` is the include basename — C has no
/// real local binding the way TS `import` does, but the basename is
/// the closest equivalent and matches how header files name-collide.
/// Returns `None` for non-Imports edges or edge targets that don't
/// shape into a basename.
fn import_record_from_edge(edge: &RawEdge, module_fqdn: &str) -> Option<ImportRecord> {
    if edge.kind != standardoc_ir::EdgeKind::Imports {
        return None;
    }
    if edge.from_fqdn != module_fqdn {
        // C `#include` edges always source from the file's module
        // FQDN; anything else is some other kind of import (FFI binding
        // import, etc.) and isn't a `workspace_imports` row.
        return None;
    }
    let (origin_module, local_name) = match &edge.to {
        ResolvedOrUnresolved::Resolved { fqdn } => {
            // Builtin header fqdns carry `.h` (`<builtin>::c::stdio.h`);
            // strip it so `local_name` stays the bare stem, matching the
            // Unresolved branch and how headers name-collide in source.
            let local = fqdn.rsplit("::").next()?.trim_end_matches(".h").to_owned();
            (fqdn.clone(), local)
        }
        ResolvedOrUnresolved::Unresolved { name } => {
            let trimmed = name.trim_end_matches(".h");
            let local = trimmed.rsplit('/').next()?.to_owned();
            (name.clone(), local)
        }
        ResolvedOrUnresolved::UnresolvedBridge { .. } => return None,
    };
    Some(ImportRecord {
        local_name,
        origin_module,
        origin_symbol: None,
        is_type_only: false,
        is_re_export: false,
    })
}

#[cfg(test)]
mod tests;

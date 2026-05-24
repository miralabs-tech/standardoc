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
            let local = fqdn.rsplit("::").next()?.to_owned();
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
mod tests {
    use super::*;
    use standardoc_ir::{
        EdgeConfidence, EdgeKind, LanguageKind, RawEdge, RawSymbol, Site, SymbolLocation,
        Visibility,
    };

    fn sym(name: &str, fqdn: &str, kind: Kind, lang_kind: &str, vis: Visibility) -> RawSymbol {
        RawSymbol {
            decl_kind: None,
            implements_trait: None,
            receiver_type: None,
            entry_point: None,
            name: name.into(),
            fqdn: fqdn.into(),
            kind,
            language_kind: LanguageKind::from(lang_kind),
            module: fqdn.rsplit_once("::").map(|(m, _)| m.to_string()),
            visibility: vis,
            location: SymbolLocation {
                file: "src/a.c".into(),
                start_line: 1,
                end_line: 1,
                start_col: 0,
                end_col: 1,
            },
            signature: None,
            body_hash: None,
            attributes: vec![],
            flags: vec![],
        }
    }

    fn module_sym(module_fqdn: &str) -> RawSymbol {
        let parent = module_fqdn
            .rsplit_once("::")
            .map(|(m, _)| m.to_string());
        RawSymbol {
            decl_kind: None,
            implements_trait: None,
            receiver_type: None,
            entry_point: None,
            name: module_fqdn
                .rsplit("::")
                .next()
                .unwrap_or(module_fqdn)
                .to_string(),
            fqdn: module_fqdn.into(),
            kind: Kind::Module,
            language_kind: LanguageKind::from("module"),
            module: parent,
            visibility: Visibility::Public,
            location: SymbolLocation {
                file: "src/a.c".into(),
                start_line: 1,
                end_line: 1,
                start_col: 0,
                end_col: 1,
            },
            signature: None,
            body_hash: None,
            attributes: vec![],
            flags: vec![],
        }
    }

    fn imports_edge(from: &str, target: ResolvedOrUnresolved) -> RawEdge {
        RawEdge {
            from_fqdn: from.into(),
            kind: EdgeKind::Imports,
            to: target,
            sites: vec![Site {
                file: "src/a.c".into(),
                line: 1,
                col: 0,
            }],
            attributes: vec![],
            confidence: EdgeConfidence::Extracted,
        }
    }

    #[test]
    fn top_level_public_fn_emits_root_binding_with_resolved_fqdn() {
        let symbols = vec![
            module_sym("pkg::a"),
            sym(
                "do_work",
                "pkg::a::do_work",
                Kind::Callable,
                "fn",
                Visibility::Public,
            ),
        ];
        let lookup = build_c_lookup(&symbols, &[], "pkg::a");
        let entries = lookup.bindings.get("do_work").expect("root binding");
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.scope_idx, ModuleLookup::ROOT_SCOPE);
        assert_eq!(entry.resolved_fqdn.as_deref(), Some("pkg::a::do_work"));
        assert!(matches!(
            entry.source,
            BindingSource::LocalDecl {
                decl_kind: LocalDeclKind::Function,
            }
        ));
    }

    #[test]
    fn static_fn_is_excluded_from_bindings() {
        let symbols = vec![
            module_sym("pkg::a"),
            sym(
                "internal",
                "pkg::a::internal",
                Kind::Callable,
                "fn",
                Visibility::Private,
            ),
        ];
        let lookup = build_c_lookup(&symbols, &[], "pkg::a");
        assert!(!lookup.bindings.contains_key("internal"));
    }

    #[test]
    fn struct_typedef_and_enum_emit_typed_decl_kinds() {
        let symbols = vec![
            module_sym("pkg::a"),
            sym(
                "Point",
                "pkg::a::Point",
                Kind::Type,
                "struct",
                Visibility::Public,
            ),
            sym(
                "u32",
                "pkg::a::u32",
                Kind::Type,
                "typedef",
                Visibility::Public,
            ),
            sym(
                "Color",
                "pkg::a::Color",
                Kind::Type,
                "enum",
                Visibility::Public,
            ),
        ];
        let lookup = build_c_lookup(&symbols, &[], "pkg::a");
        for (name, expected) in [
            ("Point", LocalDeclKind::Struct),
            ("u32", LocalDeclKind::TypeAlias),
            ("Color", LocalDeclKind::Enum),
        ] {
            let entry = &lookup.bindings.get(name).expect("binding")[0];
            let BindingSource::LocalDecl { decl_kind } = &entry.source else {
                panic!("expected LocalDecl, got {:?}", entry.source);
            };
            assert_eq!(decl_kind, &expected, "{name}");
        }
    }

    #[test]
    fn sub_symbols_under_parent_type_are_excluded() {
        // Struct field with module pointing at the parent type — not
        // file-scoped, so it must NOT appear in bindings.
        let symbols = vec![
            module_sym("pkg::a"),
            sym(
                "Point",
                "pkg::a::Point",
                Kind::Type,
                "struct",
                Visibility::Public,
            ),
            sym(
                "x",
                "pkg::a::Point::x",
                Kind::Value,
                "field",
                Visibility::Public,
            ),
        ];
        let lookup = build_c_lookup(&symbols, &[], "pkg::a");
        assert!(lookup.bindings.contains_key("Point"));
        assert!(!lookup.bindings.contains_key("x"));
    }

    #[test]
    fn module_symbol_itself_is_not_pushed_as_binding() {
        let symbols = vec![module_sym("pkg::a")];
        let lookup = build_c_lookup(&symbols, &[], "pkg::a");
        assert!(lookup.bindings.is_empty());
    }

    #[test]
    fn system_include_emits_import_record_with_builtin_origin() {
        let symbols = vec![module_sym("pkg::a")];
        let edges = vec![imports_edge(
            "pkg::a",
            ResolvedOrUnresolved::Resolved {
                fqdn: "<builtin>::c::stdio".into(),
            },
        )];
        let lookup = build_c_lookup(&symbols, &edges, "pkg::a");
        assert_eq!(lookup.imports.len(), 1);
        let record = &lookup.imports[0];
        assert_eq!(record.local_name, "stdio");
        assert_eq!(record.origin_module, "<builtin>::c::stdio");
        assert!(!record.is_type_only);
    }

    #[test]
    fn local_include_emits_import_record_with_basename_local_name() {
        let symbols = vec![module_sym("pkg::a")];
        let edges = vec![imports_edge(
            "pkg::a",
            ResolvedOrUnresolved::Unresolved {
                name: "runtime/util.h".into(),
            },
        )];
        let lookup = build_c_lookup(&symbols, &edges, "pkg::a");
        assert_eq!(lookup.imports.len(), 1);
        assert_eq!(lookup.imports[0].local_name, "util");
        assert_eq!(lookup.imports[0].origin_module, "runtime/util.h");
    }

    #[test]
    fn module_fqdn_and_language_are_set_on_the_lookup() {
        let symbols = vec![module_sym("pkg::a")];
        let lookup = build_c_lookup(&symbols, &[], "pkg::a");
        assert_eq!(lookup.module_fqdn, "pkg::a");
        assert_eq!(lookup.language, Language::C);
    }

    #[test]
    fn non_imports_edge_is_skipped_when_building_import_records() {
        let symbols = vec![module_sym("pkg::a")];
        let edges = vec![RawEdge {
            from_fqdn: "pkg::a".into(),
            kind: EdgeKind::Calls,
            to: ResolvedOrUnresolved::Resolved {
                fqdn: "pkg::b::foo".into(),
            },
            sites: vec![],
            attributes: vec![],
            confidence: EdgeConfidence::Extracted,
        }];
        let lookup = build_c_lookup(&symbols, &edges, "pkg::a");
        assert!(lookup.imports.is_empty());
    }

    #[test]
    fn unknown_type_language_kind_falls_back_to_custom() {
        let symbols = vec![
            module_sym("pkg::a"),
            sym(
                "U",
                "pkg::a::U",
                Kind::Type,
                "union",
                Visibility::Public,
            ),
        ];
        let lookup = build_c_lookup(&symbols, &[], "pkg::a");
        let entry = &lookup.bindings.get("U").expect("binding")[0];
        let BindingSource::LocalDecl { decl_kind } = &entry.source else {
            panic!("expected LocalDecl");
        };
        assert!(matches!(
            decl_kind,
            LocalDeclKind::Custom { lang: Language::C, tag } if tag == "union"
        ));
    }
}

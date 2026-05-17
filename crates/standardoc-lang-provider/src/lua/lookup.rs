//! Stage 3 R3 — AOT `ModuleLookup` builder for the Lua provider.
//!
//! Mirrors `c::lookup` with the same minimal-surface contract: build
//! just enough lookup state for `cross_workspace_post` to resolve
//! peer-workspace imports. The cross-workspace resolver consults
//! `bindings[name]` at `ROOT_SCOPE` only — Lua has no module-system
//! lexical scoping the way Rust/TS do (no `mod`, no `namespace`), so
//! the file's top-level symbols are exactly the right set.
//!
//! What the builder walks:
//!
//!   * Every symbol whose `module` matches `module_fqdn` (i.e. lives
//!     directly under the file's module FQDN) becomes a root-scope
//!     `IdentResolution`. The Lua walker emits these for top-level
//!     `function foo()` / `local function foo()` / `M.foo = function`
//!     declarations.
//!   * Sub-symbols — struct-like fields on the module table (`module:
//!     Some("<module_fqdn>::M")`) — are excluded by the parent-FQDN
//!     comparison. The cross-workspace resolver doesn't need fine-
//!     grained sub-paths; consumers can reach them via the parent's
//!     resolved FQDN.
//!   * Every `EdgeKind::Imports` edge from this module becomes an
//!     `ImportRecord`. Lua imports come from `require("foo.bar")` and
//!     `local M = require("foo.bar")`; the walker emits the same
//!     edge shape regardless of the binding form.
//!
//! `Visibility::Private` (Lua's `local`) is INCLUDED in bindings —
//! unlike C's `static`, Lua's `local` is a syntactic scope marker
//! rather than an ABI gate. The cross-workspace resolver doesn't
//! actually expose local-only symbols (peer consumers can only
//! `require` the module, which only sees `M.foo` exports), but the
//! Stage 3a in-walker visitors do consume `local` bindings for
//! intra-file resolution, so the lookup mirrors what the walker
//! observes. Filtering happens downstream at consumption time.

use standardoc_ir::{
    BindingSource, EdgeKind, IdentResolution, ImportRecord, Kind, Language, LocalDeclKind,
    ModuleLookup, RawEdge, RawSymbol, ResolvedOrUnresolved,
};

/// Build the file-level `ModuleLookup` for a parsed Lua source file.
///
/// `symbols` and `edges` are the walker's outputs from `walk` /
/// `extract_file`. `module_fqdn` is the file-level module FQDN
/// (`<package>::<dotted/path/with/separator>`). The returned lookup
/// carries the language tag, the module FQDN, every cross-workspace-
/// addressable binding at root scope, and one `ImportRecord` per
/// `require(...)` edge sourced from the file's module FQDN.
pub(crate) fn build_lua_lookup(
    symbols: &[RawSymbol],
    edges: &[RawEdge],
    module_fqdn: &str,
) -> ModuleLookup {
    let mut lookup = ModuleLookup::new(module_fqdn.to_string(), Language::Lua);

    for symbol in symbols {
        if !is_root_level(symbol, module_fqdn) {
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
/// file's module FQDN. Excludes the file's own module symbol (which
/// lives at the parent module's FQDN) and sub-symbols on the module
/// table (`module: Some("<module_fqdn>::M")`).
fn is_root_level(symbol: &RawSymbol, module_fqdn: &str) -> bool {
    if symbol.kind == Kind::Module {
        return false;
    }
    symbol.module.as_deref() == Some(module_fqdn)
}

/// Map an extracted Lua symbol to the closest `LocalDeclKind`. Lua's
/// `language_kind` axis is narrower than C's: functions use `fn`,
/// values use `let` / `local` / `module-table` etc. `Custom` carries
/// the original tag through for downstream consumers that need the
/// exact discriminant.
fn local_decl_kind_for(symbol: &RawSymbol) -> LocalDeclKind {
    match symbol.kind {
        Kind::Function => LocalDeclKind::Function,
        Kind::Module => LocalDeclKind::Module,
        Kind::Macro => LocalDeclKind::Macro,
        Kind::Type => LocalDeclKind::TypeAlias,
        Kind::Value => match symbol.language_kind.as_str() {
            "let" | "local" => LocalDeclKind::Let,
            "const" => LocalDeclKind::Const,
            other => LocalDeclKind::Custom {
                lang: Language::Lua,
                tag: other.to_owned(),
            },
        },
    }
}

/// Translate an `EdgeKind::Imports` edge into an `ImportRecord`. Lua
/// emits these for `require("…")` calls (and `local foo =
/// require("…")` variants). The `local_name` is the import's tail
/// segment — closest equivalent to Lua's binding form when the
/// require result is assigned to a local; for bare `require("…")`
/// it still gives a reasonable handle.
fn import_record_from_edge(edge: &RawEdge, module_fqdn: &str) -> Option<ImportRecord> {
    if edge.kind != EdgeKind::Imports {
        return None;
    }
    if edge.from_fqdn != module_fqdn {
        return None;
    }
    let (origin_module, local_name) = match &edge.to {
        ResolvedOrUnresolved::Resolved { fqdn } => {
            let local = fqdn.rsplit("::").next()?.to_owned();
            (fqdn.clone(), local)
        }
        ResolvedOrUnresolved::Unresolved { name } => {
            // Lua require paths can be `foo.bar.baz` (dot-separated)
            // or pre-normalised `foo::bar::baz` — handle both.
            let local = name
                .rsplit(|c: char| c == '.' || c == ':' || c == '/')
                .next()
                .filter(|s| !s.is_empty())?
                .to_owned();
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

    fn sym(name: &str, fqdn: &str, kind: Kind, lang_kind: &str) -> RawSymbol {
        RawSymbol {
            name: name.into(),
            fqdn: fqdn.into(),
            kind,
            language_kind: LanguageKind::from(lang_kind),
            module: fqdn.rsplit_once("::").map(|(m, _)| m.to_string()),
            visibility: Visibility::Public,
            location: SymbolLocation {
                file: "src/a.lua".into(),
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
                file: "src/a.lua".into(),
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
                file: "src/a.lua".into(),
                line: 1,
                col: 0,
            }],
            attributes: vec![],
            confidence: EdgeConfidence::Extracted,
        }
    }

    #[test]
    fn top_level_function_emits_root_binding_with_resolved_fqdn() {
        let symbols = vec![
            module_sym("pkg::a"),
            sym("greet", "pkg::a::greet", Kind::Function, "fn"),
        ];
        let lookup = build_lua_lookup(&symbols, &[], "pkg::a");
        let entries = lookup.bindings.get("greet").expect("root binding");
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.scope_idx, ModuleLookup::ROOT_SCOPE);
        assert_eq!(entry.resolved_fqdn.as_deref(), Some("pkg::a::greet"));
        assert!(matches!(
            entry.source,
            BindingSource::LocalDecl {
                decl_kind: LocalDeclKind::Function,
            }
        ));
    }

    #[test]
    fn sub_symbol_under_module_table_is_excluded() {
        // `M.helper = function ... end` ends up at `pkg::a::M::helper`
        // with `module: Some("pkg::a::M")` — not file-scoped, so it
        // must NOT appear in bindings.
        let symbols = vec![
            module_sym("pkg::a"),
            sym("M", "pkg::a::M", Kind::Value, "module-table"),
            sym("helper", "pkg::a::M::helper", Kind::Function, "fn"),
        ];
        let lookup = build_lua_lookup(&symbols, &[], "pkg::a");
        assert!(lookup.bindings.contains_key("M"));
        assert!(!lookup.bindings.contains_key("helper"));
    }

    #[test]
    fn module_symbol_itself_is_not_pushed_as_binding() {
        let symbols = vec![module_sym("pkg::a")];
        let lookup = build_lua_lookup(&symbols, &[], "pkg::a");
        assert!(lookup.bindings.is_empty());
    }

    #[test]
    fn local_values_map_to_let_decl_kind() {
        let symbols = vec![
            module_sym("pkg::a"),
            sym("counter", "pkg::a::counter", Kind::Value, "local"),
        ];
        let lookup = build_lua_lookup(&symbols, &[], "pkg::a");
        let entry = &lookup.bindings.get("counter").expect("binding")[0];
        let BindingSource::LocalDecl { decl_kind } = &entry.source else {
            panic!("expected LocalDecl");
        };
        assert_eq!(decl_kind, &LocalDeclKind::Let);
    }

    #[test]
    fn unknown_value_language_kind_falls_back_to_custom() {
        let symbols = vec![
            module_sym("pkg::a"),
            sym(
                "TAG",
                "pkg::a::TAG",
                Kind::Value,
                "module-table-export",
            ),
        ];
        let lookup = build_lua_lookup(&symbols, &[], "pkg::a");
        let entry = &lookup.bindings.get("TAG").expect("binding")[0];
        let BindingSource::LocalDecl { decl_kind } = &entry.source else {
            panic!("expected LocalDecl");
        };
        assert!(matches!(
            decl_kind,
            LocalDeclKind::Custom { lang: Language::Lua, tag } if tag == "module-table-export"
        ));
    }

    #[test]
    fn require_with_dot_path_emits_import_with_tail_local_name() {
        let symbols = vec![module_sym("pkg::a")];
        let edges = vec![imports_edge(
            "pkg::a",
            ResolvedOrUnresolved::Unresolved {
                name: "foo.bar.baz".into(),
            },
        )];
        let lookup = build_lua_lookup(&symbols, &edges, "pkg::a");
        assert_eq!(lookup.imports.len(), 1);
        assert_eq!(lookup.imports[0].local_name, "baz");
        assert_eq!(lookup.imports[0].origin_module, "foo.bar.baz");
    }

    #[test]
    fn require_with_resolved_fqdn_emits_import_with_tail_segment() {
        let symbols = vec![module_sym("pkg::a")];
        let edges = vec![imports_edge(
            "pkg::a",
            ResolvedOrUnresolved::Resolved {
                fqdn: "other_pkg::lib::helpers".into(),
            },
        )];
        let lookup = build_lua_lookup(&symbols, &edges, "pkg::a");
        assert_eq!(lookup.imports.len(), 1);
        assert_eq!(lookup.imports[0].local_name, "helpers");
        assert_eq!(lookup.imports[0].origin_module, "other_pkg::lib::helpers");
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
        let lookup = build_lua_lookup(&symbols, &edges, "pkg::a");
        assert!(lookup.imports.is_empty());
    }

    #[test]
    fn import_edge_sourced_outside_this_module_is_ignored() {
        let symbols = vec![module_sym("pkg::a")];
        let edges = vec![imports_edge(
            "pkg::other",
            ResolvedOrUnresolved::Resolved {
                fqdn: "pkg::lib".into(),
            },
        )];
        let lookup = build_lua_lookup(&symbols, &edges, "pkg::a");
        assert!(lookup.imports.is_empty());
    }

    #[test]
    fn module_fqdn_and_language_are_set_on_the_lookup() {
        let symbols = vec![module_sym("pkg::a")];
        let lookup = build_lua_lookup(&symbols, &[], "pkg::a");
        assert_eq!(lookup.module_fqdn, "pkg::a");
        assert_eq!(lookup.language, Language::Lua);
    }
}

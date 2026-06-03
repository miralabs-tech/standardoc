use proc_macro2::Span;
use standardoc_ir::{
    AliasMutability, BindingSource, IdentResolution, ImportRecord, Language, LocalDeclKind,
    ModuleLookup, ScopeKind, ScopeRange,
};
use syn::spanned::Spanned;
use syn::visit::Visit;
use syn::{
    Expr, File, FnArg, GenericParam, ItemConst, ItemEnum, ItemFn, ItemImpl, ItemMacro, ItemMod,
    ItemStatic, ItemStruct, ItemTrait, ItemType, ItemUnion, ItemUse, Local, Pat, UseTree,
};

use super::walk::path_to_string;

/// Build the AOT identifier-resolution table for a Rust module (parity
/// with [`crate::ts::lookup::build_ts_lookup`]).
///
/// Two-pass design:
/// 1. `hoist_items` — top-level items (fn / struct / enum / trait /
///    type alias / const / static / union / macro / nested mod) plus
///    `use` declarations populate the ROOT scope. Rust hoisting is
///    file-wide so forward refs across items are legal.
/// 2. `syn::visit::visit_file` — full traversal for nested scopes
///    (fn body blocks, impl/trait methods, closures) and let bindings.
///
/// Imports flatten into `ModuleLookup.imports` for Stage 3b cross-
/// workspace SQL resolution. `use a::{B, C as D}` expands to two
/// records.
/// Derives the `(u32, u32)` span key used as the `ModuleLookup.span_to_scope`
/// HashMap key. Packs `(start.line, start.column)` because two distinct
/// scopes can't begin at the same source position. The walker uses the
/// same helper so query/recording stay symmetric.
pub(crate) fn scope_span_key(span: Span) -> (u32, u32) {
    let start = span.start();
    (
        u32::try_from(start.line).unwrap_or(u32::MAX),
        u32::try_from(start.column).unwrap_or(u32::MAX),
    )
}

pub(crate) fn build_rust_lookup(file: &File, module_fqdn: &str) -> ModuleLookup {
    let mut lookup = ModuleLookup::new(module_fqdn.to_string(), Language::Rust);
    let mut builder = LookupBuilder {
        lookup: &mut lookup,
        scope_stack: vec![ModuleLookup::ROOT_SCOPE],
    };
    builder.hoist_items(&file.items);
    syn::visit::visit_file(&mut builder, file);
    lookup
}

struct LookupBuilder<'a> {
    lookup: &'a mut ModuleLookup,
    scope_stack: Vec<u32>,
}

impl LookupBuilder<'_> {
    fn current_scope(&self) -> u32 {
        *self.scope_stack.last().unwrap_or(&ModuleLookup::ROOT_SCOPE)
    }

    fn push_scope(&mut self, kind: ScopeKind, span: Span) {
        let parent = Some(self.current_scope());
        let (lo, hi) = scope_span_key(span);
        let idx = self.lookup.push_scope_with_span(
            ScopeRange {
                start_line: u32::try_from(span.start().line).unwrap_or(u32::MAX),
                end_line: u32::try_from(span.end().line).unwrap_or(u32::MAX),
                parent,
                kind,
            },
            lo,
            hi,
        );
        self.scope_stack.push(idx);
    }

    fn pop_scope(&mut self) {
        self.scope_stack.pop();
    }

    fn add_binding(&mut self, name: String, source: BindingSource, attributes: Vec<String>) {
        let scope_idx = self.current_scope();
        self.lookup.push_binding(IdentResolution {
            name,
            source,
            resolved_fqdn: None,
            aliases_to: None,
            mutability: None,
            scope_idx,
            attributes,
            ir_kind: None,
        });
    }

    /// Stage 3e-2-bis — push an alias-carrying binding for `let x = bar;`
    /// patterns. Mirrors `ts::lookup::LookupBuilder::add_aliased_binding`.
    /// The `aliases_to` slot stores the RHS path text-as-written so the
    /// visitor-side `resolve_name` can re-resolve it through the module
    /// chain (alias_table → defined_fqdns → builtin → unresolved).
    fn add_aliased_binding(
        &mut self,
        name: String,
        decl_kind: LocalDeclKind,
        mutability: AliasMutability,
        aliases_to: String,
    ) {
        let scope_idx = self.current_scope();
        self.lookup.push_binding(IdentResolution {
            name,
            source: BindingSource::LocalDecl { decl_kind },
            resolved_fqdn: None,
            aliases_to: Some(aliases_to),
            mutability: Some(mutability),
            scope_idx,
            attributes: vec![mutability.as_slug().to_string()],
            ir_kind: None,
        });
    }

    fn hoist_items(&mut self, items: &[syn::Item]) {
        for item in items {
            match item {
                syn::Item::Fn(ItemFn { sig, .. }) => self.add_binding(
                    sig.ident.to_string(),
                    BindingSource::LocalDecl {
                        decl_kind: LocalDeclKind::Function,
                    },
                    vec![],
                ),
                syn::Item::Struct(ItemStruct { ident, .. }) => self.add_binding(
                    ident.to_string(),
                    BindingSource::LocalDecl {
                        decl_kind: LocalDeclKind::Struct,
                    },
                    vec![],
                ),
                syn::Item::Enum(ItemEnum { ident, .. }) => self.add_binding(
                    ident.to_string(),
                    BindingSource::LocalDecl {
                        decl_kind: LocalDeclKind::Enum,
                    },
                    vec![],
                ),
                syn::Item::Trait(ItemTrait { ident, .. }) => self.add_binding(
                    ident.to_string(),
                    BindingSource::LocalDecl {
                        decl_kind: LocalDeclKind::Trait,
                    },
                    vec![],
                ),
                syn::Item::Type(ItemType { ident, .. }) => self.add_binding(
                    ident.to_string(),
                    BindingSource::LocalDecl {
                        decl_kind: LocalDeclKind::TypeAlias,
                    },
                    vec![],
                ),
                syn::Item::Const(ItemConst { ident, .. }) => self.add_binding(
                    ident.to_string(),
                    BindingSource::LocalDecl {
                        decl_kind: LocalDeclKind::Const,
                    },
                    vec![],
                ),
                syn::Item::Static(ItemStatic { ident, .. }) => self.add_binding(
                    ident.to_string(),
                    BindingSource::LocalDecl {
                        decl_kind: LocalDeclKind::Const,
                    },
                    vec!["static".into()],
                ),
                syn::Item::Union(ItemUnion { ident, .. }) => self.add_binding(
                    ident.to_string(),
                    BindingSource::LocalDecl {
                        decl_kind: LocalDeclKind::Struct,
                    },
                    vec!["union".into()],
                ),
                syn::Item::Mod(ItemMod { ident, .. }) => self.add_binding(
                    ident.to_string(),
                    BindingSource::LocalDecl {
                        decl_kind: LocalDeclKind::Module,
                    },
                    vec![],
                ),
                syn::Item::Macro(ItemMacro {
                    ident: Some(name), ..
                }) => self.add_binding(
                    name.to_string(),
                    BindingSource::LocalDecl {
                        decl_kind: LocalDeclKind::Macro,
                    },
                    vec![],
                ),
                syn::Item::Use(ItemUse { tree, .. }) => {
                    self.walk_use_tree(tree, &mut Vec::new());
                }
                // Impl blocks have no own ident binding (the impl is
                // attached to a target type that's already hoisted).
                syn::Item::Impl(_)
                | syn::Item::ForeignMod(_)
                | syn::Item::ExternCrate(_)
                | syn::Item::Verbatim(_)
                | syn::Item::Macro(_)
                | _ => {}
            }
        }
    }

    fn walk_use_tree(&mut self, tree: &UseTree, prefix: &mut Vec<String>) {
        match tree {
            UseTree::Path(p) => {
                prefix.push(p.ident.to_string());
                self.walk_use_tree(&p.tree, prefix);
                prefix.pop();
            }
            UseTree::Name(n) => {
                let local_name = n.ident.to_string();
                let module_path = prefix.join("::");
                self.record_import(local_name.clone(), module_path, Some(local_name), false);
            }
            UseTree::Rename(r) => {
                let local_name = r.rename.to_string();
                let original = r.ident.to_string();
                let module_path = prefix.join("::");
                self.record_import(local_name, module_path, Some(original), false);
            }
            UseTree::Group(g) => {
                for item in &g.items {
                    self.walk_use_tree(item, prefix);
                }
            }
            UseTree::Glob(_) => {
                // Glob imports cannot enumerate locals — punt to Stage 3b
                // cross-workspace lookup (which can resolve via the
                // origin module's exported symbols).
                let module_path = prefix.join("::");
                self.lookup.push_import(ImportRecord {
                    local_name: "*".into(),
                    origin_module: module_path,
                    origin_symbol: None,
                    is_type_only: false,
                    is_re_export: false,
                });
            }
        }
    }

    fn record_import(
        &mut self,
        local_name: String,
        module_path: String,
        original: Option<String>,
        is_re_export: bool,
    ) {
        // Bug E-2 (write side) — compute the absolute canonical FQDN
        // the import points at, so cross-workspace queries for
        // `<this_module>::<local_name>` can follow re-exports to the
        // real definition. For `pub use span::Span` in `lur-common`
        // the binding `Span` resolves to `lur-common::span::Span` ;
        // appending `::new` (in `cross_workspace_post`'s suffix-chain
        // walk) then composes the real method FQDN.
        let canonical = original.as_deref().unwrap_or(&local_name);
        let absolute_module = compose_absolute_module_path(&self.lookup.module_fqdn, &module_path);
        let resolved_fqdn = Some(if absolute_module.is_empty() {
            canonical.to_string()
        } else {
            format!("{absolute_module}::{canonical}")
        });
        self.add_binding_with_resolved(
            local_name.clone(),
            BindingSource::Import {
                module_path: module_path.clone(),
                original_name: original.clone(),
                is_type_only: false,
                is_re_export,
            },
            vec![],
            resolved_fqdn,
        );
        self.lookup.push_import(ImportRecord {
            local_name,
            origin_module: module_path,
            origin_symbol: original,
            is_type_only: false,
            is_re_export,
        });
    }

    fn add_binding_with_resolved(
        &mut self,
        name: String,
        source: BindingSource,
        attributes: Vec<String>,
        resolved_fqdn: Option<String>,
    ) {
        let scope_idx = self.current_scope();
        self.lookup.push_binding(IdentResolution {
            name,
            source,
            resolved_fqdn,
            aliases_to: None,
            mutability: None,
            scope_idx,
            attributes,
            ir_kind: None,
        });
    }

    fn bind_generic_params(&mut self, generics: &syn::Generics) {
        for param in &generics.params {
            match param {
                GenericParam::Type(t) => {
                    self.add_binding(t.ident.to_string(), BindingSource::TypeParam, vec![]);
                }
                GenericParam::Const(c) => self.add_binding(
                    c.ident.to_string(),
                    BindingSource::TypeParam,
                    vec!["const-generic".into()],
                ),
                GenericParam::Lifetime(_) => {
                    // Lifetimes never appear in value/type identifier
                    // position — skip.
                }
            }
        }
    }

    fn bind_fn_params(&mut self, inputs: &syn::punctuated::Punctuated<FnArg, syn::Token![,]>) {
        for input in inputs {
            match input {
                FnArg::Typed(pt) => self.bind_pat(&pt.pat, BindingSource::Param, vec![]),
                FnArg::Receiver(_) => {
                    // `self` / `&self` / `&mut self` — bound implicitly
                    // via Self type, no binding needed.
                }
            }
        }
    }

    fn bind_pat(&mut self, pat: &Pat, source: BindingSource, extra_attrs: Vec<String>) {
        match pat {
            Pat::Ident(ident) => {
                self.add_binding(ident.ident.to_string(), source, extra_attrs);
            }
            Pat::Tuple(t) => {
                let mut attrs = extra_attrs;
                attrs.push("unhandled-destructuring".into());
                for elem in &t.elems {
                    self.bind_pat(elem, source.clone(), attrs.clone());
                }
            }
            Pat::TupleStruct(ts) => {
                let mut attrs = extra_attrs;
                attrs.push("unhandled-destructuring".into());
                for elem in &ts.elems {
                    self.bind_pat(elem, source.clone(), attrs.clone());
                }
            }
            Pat::Struct(s) => {
                let mut attrs = extra_attrs;
                attrs.push("unhandled-destructuring".into());
                for field in &s.fields {
                    self.bind_pat(&field.pat, source.clone(), attrs.clone());
                }
            }
            Pat::Reference(r) => self.bind_pat(&r.pat, source, extra_attrs),
            Pat::Type(t) => self.bind_pat(&t.pat, source, extra_attrs),
            Pat::Or(o) => {
                // Bind the first arm — all arms must bind the same set
                // of names so any one works.
                if let Some(first) = o.cases.first() {
                    self.bind_pat(first, source, extra_attrs);
                }
            }
            _ => {}
        }
    }
}

impl<'ast> Visit<'ast> for LookupBuilder<'_> {
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        self.push_scope(ScopeKind::Function, node.span());
        self.bind_generic_params(&node.sig.generics);
        self.bind_fn_params(&node.sig.inputs);
        syn::visit::visit_item_fn(self, node);
        self.pop_scope();
    }

    fn visit_item_struct(&mut self, node: &'ast ItemStruct) {
        self.push_scope(ScopeKind::TypeContainer, node.span());
        self.bind_generic_params(&node.generics);
        syn::visit::visit_item_struct(self, node);
        self.pop_scope();
    }

    fn visit_item_enum(&mut self, node: &'ast ItemEnum) {
        self.push_scope(ScopeKind::TypeContainer, node.span());
        self.bind_generic_params(&node.generics);
        for variant in &node.variants {
            self.add_binding(
                variant.ident.to_string(),
                BindingSource::LocalDecl {
                    decl_kind: LocalDeclKind::Const,
                },
                vec!["enum-variant".into()],
            );
        }
        syn::visit::visit_item_enum(self, node);
        self.pop_scope();
    }

    fn visit_item_trait(&mut self, node: &'ast ItemTrait) {
        self.push_scope(ScopeKind::TypeContainer, node.span());
        self.bind_generic_params(&node.generics);
        syn::visit::visit_item_trait(self, node);
        self.pop_scope();
    }

    fn visit_item_type(&mut self, node: &'ast ItemType) {
        self.push_scope(ScopeKind::TypeContainer, node.span());
        self.bind_generic_params(&node.generics);
        syn::visit::visit_item_type(self, node);
        self.pop_scope();
    }

    fn visit_item_impl(&mut self, node: &'ast ItemImpl) {
        self.push_scope(ScopeKind::TypeContainer, node.span());
        self.bind_generic_params(&node.generics);
        syn::visit::visit_item_impl(self, node);
        self.pop_scope();
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.push_scope(ScopeKind::Function, node.span());
        self.bind_generic_params(&node.sig.generics);
        self.bind_fn_params(&node.sig.inputs);
        syn::visit::visit_impl_item_fn(self, node);
        self.pop_scope();
    }

    fn visit_trait_item_fn(&mut self, node: &'ast syn::TraitItemFn) {
        self.push_scope(ScopeKind::Function, node.span());
        self.bind_generic_params(&node.sig.generics);
        self.bind_fn_params(&node.sig.inputs);
        syn::visit::visit_trait_item_fn(self, node);
        self.pop_scope();
    }

    fn visit_item_mod(&mut self, node: &'ast ItemMod) {
        self.push_scope(ScopeKind::Module, node.span());
        if let Some((_, items)) = &node.content {
            self.hoist_items(items);
        }
        syn::visit::visit_item_mod(self, node);
        self.pop_scope();
    }

    fn visit_block(&mut self, node: &'ast syn::Block) {
        self.push_scope(ScopeKind::Block, node.span());
        syn::visit::visit_block(self, node);
        self.pop_scope();
    }

    fn visit_local(&mut self, node: &'ast Local) {
        // Stage 3e-2-bis — alias detection: `let x = bar;` or
        // `let x = MyType::CONST;` captures the RHS path as `aliases_to`
        // so subsequent value-reads of `x` propagate to that target with
        // a `via-alias[-mutable]` slug. Restricted to single Pat::Ident
        // (optionally wrapped in Pat::Type for type-annotated bindings).
        // Non-Path RHS (call, literal, closure, …) falls through to the
        // plain `bind_pat` path — no alias, `x` becomes an opaque Local.
        if let Some(init) = node.init.as_ref()
            && let Some(alias_str) = resolve_alias_rhs(&init.expr)
            && let Some(pat_ident) = unwrap_pat_ident(&node.pat)
        {
            let mutability = if pat_ident.mutability.is_some() {
                AliasMutability::Mutable
            } else {
                AliasMutability::Const
            };
            self.add_aliased_binding(
                pat_ident.ident.to_string(),
                LocalDeclKind::Let,
                mutability,
                alias_str,
            );
            syn::visit::visit_local(self, node);
            return;
        }
        self.bind_pat(
            &node.pat,
            BindingSource::LocalDecl {
                decl_kind: LocalDeclKind::Let,
            },
            vec![],
        );
        syn::visit::visit_local(self, node);
    }

    fn visit_expr_closure(&mut self, node: &'ast syn::ExprClosure) {
        self.push_scope(ScopeKind::Function, node.span());
        for input in &node.inputs {
            self.bind_pat(input, BindingSource::Param, vec![]);
        }
        syn::visit::visit_expr_closure(self, node);
        self.pop_scope();
    }
}

/// Stage 3e-2-bis — leftmost-base of an alias-worthy RHS expression.
/// Restricted to `Expr::Path` (single-ident OR multi-segment) day-1.
/// Field accesses (`obj.field`), references (`&foo`), calls and
/// literals are NOT aliases — they're fresh values that don't share
/// identity with the original binding. Mirrors the conservative shape
/// of `ts::lookup::resolve_alias_rhs` but stores the full path text
/// rather than the leftmost-base ident (Rust paths are atomic at the
/// AST level, so the visitor's `resolve_name` re-resolves them through
/// the module chain in one shot).
fn resolve_alias_rhs(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Path(p) => {
            let s = path_to_string(&p.path);
            if s.is_empty() { None } else { Some(s) }
        }
        _ => None,
    }
}

/// Stage 3e-2-bis — peel `Pat::Type` wrappers (`let x: T = ...`) to get
/// at the underlying [`syn::PatIdent`]. Returns `None` for destructuring
/// patterns (tuple, struct, …) which the alias path doesn't handle.
fn unwrap_pat_ident(pat: &Pat) -> Option<&syn::PatIdent> {
    match pat {
        Pat::Ident(p) => Some(p),
        Pat::Type(t) => unwrap_pat_ident(&t.pat),
        _ => None,
    }
}

/// Compose an absolute module FQDN from the `use`-tree relative path
/// recorded on an [`ImportRecord`]. Applies Rust 2018 path-resolution
/// rules :
///
/// - `crate::xxx`  → `<crate_name>::xxx`
/// - `self::xxx`   → `<current_module>::xxx`
/// - `super::xxx`  → `<parent_module>::xxx`
/// - bare `xxx::…` → `<current_module>::xxx::…` (sibling submodule)
///
/// The "bare" branch is a heuristic — at the AST level we can't
/// disambiguate `mod xxx;` from an external crate `xxx`. For
/// workspace re-exports (`pub use mysub::Foo`), sibling-submodule is
/// the dominant reading ; for external imports (`use serde::Foo`) the
/// composed `<current_module>::serde::Foo` is harmless because it
/// doesn't resolve to any workspace symbol and the edge stays
/// unresolved (same outcome as pre-fix).
pub(crate) fn compose_absolute_module_path(current_module: &str, relative: &str) -> String {
    if relative.is_empty() {
        return current_module.to_string();
    }
    let crate_name = current_module.split("::").next().unwrap_or(current_module);
    if let Some(rest) = relative.strip_prefix("crate::") {
        return format!("{crate_name}::{rest}");
    }
    if relative == "crate" {
        return crate_name.to_string();
    }
    if let Some(rest) = relative.strip_prefix("self::") {
        return format!("{current_module}::{rest}");
    }
    if relative == "self" {
        return current_module.to_string();
    }
    if let Some(rest) = relative.strip_prefix("super::") {
        let parent = current_module
            .rsplit_once("::")
            .map_or(current_module, |(p, _)| p);
        return format!("{parent}::{rest}");
    }
    if relative == "super" {
        return current_module
            .rsplit_once("::")
            .map_or_else(|| current_module.to_string(), |(p, _)| p.to_string());
    }
    format!("{current_module}::{relative}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> File {
        syn::parse_file(src).expect("parse ok")
    }

    #[test]
    fn module_lookup_carries_module_fqdn_and_language() {
        let f = parse("fn f() {}\n");
        let lookup = build_rust_lookup(&f, "my_crate::module");
        assert_eq!(lookup.module_fqdn, "my_crate::module");
        assert_eq!(lookup.language, Language::Rust);
    }

    #[test]
    fn top_level_items_hoisted_to_root() {
        let f = parse(
            "fn f() {}\nstruct S;\nenum E { A }\ntrait T {}\ntype Ty = u32;\nconst C: u32 = 1;\nstatic ST: u32 = 0;\n",
        );
        let lookup = build_rust_lookup(&f, "m");
        for name in ["f", "S", "E", "T", "Ty", "C", "ST"] {
            let b = lookup
                .bindings
                .get(name)
                .and_then(|v| v.first())
                .unwrap_or_else(|| panic!("{name} binding"));
            assert_eq!(b.scope_idx, ModuleLookup::ROOT_SCOPE, "{name} at root");
        }
    }

    #[test]
    fn use_simple_binds_last_segment() {
        let f = parse("use std::collections::HashMap;\n");
        let lookup = build_rust_lookup(&f, "m");
        let b = lookup
            .bindings
            .get("HashMap")
            .and_then(|v| v.first())
            .expect("HashMap binding");
        match &b.source {
            BindingSource::Import {
                module_path,
                original_name,
                ..
            } => {
                assert_eq!(module_path, "std::collections");
                assert_eq!(original_name.as_deref(), Some("HashMap"));
            }
            other => panic!("expected Import, got {other:?}"),
        }
        assert_eq!(lookup.imports.len(), 1);
    }

    #[test]
    fn compose_absolute_module_path_handles_crate_self_super_and_bare() {
        // `crate::xxx` → `<crate_name>::xxx`
        assert_eq!(
            compose_absolute_module_path("my-crate::sub", "crate::pipeline::provider"),
            "my-crate::pipeline::provider"
        );
        assert_eq!(
            compose_absolute_module_path("my-crate::sub", "crate"),
            "my-crate"
        );
        // `self::xxx` → `<current_module>::xxx`
        assert_eq!(
            compose_absolute_module_path("my-crate::sub", "self::inner"),
            "my-crate::sub::inner"
        );
        assert_eq!(
            compose_absolute_module_path("my-crate::sub", "self"),
            "my-crate::sub"
        );
        // `super::xxx` → `<parent>::xxx`
        assert_eq!(
            compose_absolute_module_path("my-crate::sub::inner", "super::sibling"),
            "my-crate::sub::sibling"
        );
        assert_eq!(
            compose_absolute_module_path("my-crate::sub::inner", "super"),
            "my-crate::sub"
        );
        // bare `xxx` → `<current_module>::xxx` (sibling submodule)
        assert_eq!(
            compose_absolute_module_path("lur-common", "span"),
            "lur-common::span"
        );
        // empty relative collapses to current module
        assert_eq!(
            compose_absolute_module_path("my-crate::sub", ""),
            "my-crate::sub"
        );
    }

    #[test]
    fn pub_use_sibling_submodule_resolves_to_canonical_fqdn() {
        // Bug E-2 (write side) — `pub use span::{Position, Span}` in
        // `lur-common/src/lib.rs` must record `Span`'s binding with
        // `resolved_fqdn = Some("lur-common::span::Span")` so the
        // cross_workspace_post suffix-chain can compose the canonical
        // method FQDN when a peer crate calls `Span::new()`.
        let f = parse("pub use span::{Position, Span};\n");
        let lookup = build_rust_lookup(&f, "lur-common");
        let b = lookup
            .bindings
            .get("Span")
            .and_then(|v| v.first())
            .expect("Span binding");
        assert_eq!(
            b.resolved_fqdn.as_deref(),
            Some("lur-common::span::Span"),
            "Span re-export must point at the sibling-submodule canonical"
        );
        let p = lookup
            .bindings
            .get("Position")
            .and_then(|v| v.first())
            .expect("Position binding");
        assert_eq!(
            p.resolved_fqdn.as_deref(),
            Some("lur-common::span::Position")
        );
    }

    #[test]
    fn pub_use_crate_path_resolves_through_crate_root() {
        // `pub use crate::pipeline::provider::LanguageProvider` in
        // `standardoc-core::lib` resolves to
        // `standardoc-core::pipeline::provider::LanguageProvider`,
        // not `standardoc-core::crate::pipeline::...`.
        let f = parse("pub use crate::pipeline::provider::LanguageProvider;\n");
        let lookup = build_rust_lookup(&f, "standardoc-core");
        let b = lookup
            .bindings
            .get("LanguageProvider")
            .and_then(|v| v.first())
            .expect("LanguageProvider binding");
        assert_eq!(
            b.resolved_fqdn.as_deref(),
            Some("standardoc-core::pipeline::provider::LanguageProvider")
        );
    }

    #[test]
    fn use_rename_resolves_to_original_canonical() {
        // `pub use foo::OriginalName as Alias` records the alias as
        // local binding but resolved_fqdn points at the original.
        let f = parse("pub use sub::OriginalName as Alias;\n");
        let lookup = build_rust_lookup(&f, "my-crate");
        let b = lookup
            .bindings
            .get("Alias")
            .and_then(|v| v.first())
            .expect("Alias binding");
        assert_eq!(
            b.resolved_fqdn.as_deref(),
            Some("my-crate::sub::OriginalName"),
            "alias canonical must use original name, not alias name"
        );
    }

    #[test]
    fn use_rename_binds_alias_with_original() {
        let f = parse("use std::collections::HashMap as Map;\n");
        let lookup = build_rust_lookup(&f, "m");
        assert!(!lookup.bindings.contains_key("HashMap"));
        let b = lookup
            .bindings
            .get("Map")
            .and_then(|v| v.first())
            .expect("Map binding");
        match &b.source {
            BindingSource::Import {
                module_path,
                original_name,
                ..
            } => {
                assert_eq!(module_path, "std::collections");
                assert_eq!(original_name.as_deref(), Some("HashMap"));
            }
            other => panic!("expected Import, got {other:?}"),
        }
    }

    #[test]
    fn use_group_binds_each_member() {
        let f = parse("use std::collections::{HashMap, HashSet, BTreeMap};\n");
        let lookup = build_rust_lookup(&f, "m");
        for name in ["HashMap", "HashSet", "BTreeMap"] {
            let b = lookup
                .bindings
                .get(name)
                .and_then(|v| v.first())
                .unwrap_or_else(|| panic!("{name} binding"));
            match &b.source {
                BindingSource::Import { module_path, .. } => {
                    assert_eq!(module_path, "std::collections");
                }
                other => panic!("expected Import for {name}, got {other:?}"),
            }
        }
        assert_eq!(lookup.imports.len(), 3);
    }

    #[test]
    fn use_glob_records_star_import() {
        let f = parse("use std::collections::*;\n");
        let lookup = build_rust_lookup(&f, "m");
        assert_eq!(lookup.imports.len(), 1);
        assert_eq!(lookup.imports[0].local_name, "*");
        assert_eq!(lookup.imports[0].origin_module, "std::collections");
    }

    #[test]
    fn type_param_bound_in_function_scope() {
        let f = parse("fn f<T, U: Clone>(x: T) -> U { todo!() }\n");
        let lookup = build_rust_lookup(&f, "m");
        for name in ["T", "U"] {
            let b = lookup
                .bindings
                .get(name)
                .and_then(|v| v.first())
                .unwrap_or_else(|| panic!("{name} type-param binding"));
            assert!(matches!(b.source, BindingSource::TypeParam));
            assert_ne!(b.scope_idx, ModuleLookup::ROOT_SCOPE);
        }
    }

    #[test]
    fn fn_body_let_binding_scoped_below_root() {
        let f = parse("fn f() { let inner = 42; }\n");
        let lookup = build_rust_lookup(&f, "m");
        let inner = lookup
            .bindings
            .get("inner")
            .and_then(|v| v.first())
            .expect("inner binding");
        assert_ne!(inner.scope_idx, ModuleLookup::ROOT_SCOPE);
    }

    #[test]
    fn enum_variants_bound_inside_enum_scope() {
        let f = parse("enum Color { Red, Green, Blue }\n");
        let lookup = build_rust_lookup(&f, "m");
        assert!(lookup.bindings.contains_key("Color"));
        for v in ["Red", "Green", "Blue"] {
            let b = lookup
                .bindings
                .get(v)
                .and_then(|v| v.first())
                .unwrap_or_else(|| panic!("{v} variant binding"));
            assert_ne!(b.scope_idx, ModuleLookup::ROOT_SCOPE);
            assert!(b.attributes.iter().any(|a| a == "enum-variant"));
        }
    }

    #[test]
    fn impl_block_generics_bind_in_impl_scope() {
        let f = parse("impl<T: Clone> MyType<T> { fn method(&self) -> T { todo!() } }\n");
        let lookup = build_rust_lookup(&f, "m");
        let t = lookup
            .bindings
            .get("T")
            .and_then(|v| v.first())
            .expect("T binding");
        assert!(matches!(t.source, BindingSource::TypeParam));
    }

    #[test]
    fn resolve_local_walks_chain_to_root_use() {
        let f = parse("use std::vec::Vec;\nfn f() { let v: Vec<u32> = Vec::new(); }\n");
        let lookup = build_rust_lookup(&f, "m");
        let v_scope = lookup
            .bindings
            .get("v")
            .and_then(|v| v.first())
            .unwrap()
            .scope_idx;
        let vec_t = lookup
            .resolve_local("Vec", v_scope)
            .expect("Vec reachable via parent");
        assert!(matches!(vec_t.source, BindingSource::Import { .. }));
        assert_eq!(vec_t.scope_idx, ModuleLookup::ROOT_SCOPE);
    }
}

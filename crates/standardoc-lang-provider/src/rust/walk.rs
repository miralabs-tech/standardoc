use std::collections::HashMap;

use proc_macro2::Span;
use quote::ToTokens;
use standardoc_ir::{
    EdgeKind, Kind, LanguageKind, Modifiers, Param, RawAttribute, RawAttributeArg, RawDocument,
    RawEdge, RawSymbol, ResolvedOrUnresolved, Signature, SignatureMeta, Site, SymbolLocation,
    TypeRef, Visibility,
};
use syn::spanned::Spanned;

use crate::walk_core::WalkContextCore;

use super::body_hash;
use super::extract_call;
use super::extract_doc;
use super::extract_use;
use super::visibility;

pub(crate) struct WalkContext {
    pub(crate) core: WalkContextCore,
    pub(crate) crate_name: String,
    pub(crate) alias_table: HashMap<String, String>,
}

impl WalkContext {
    pub(crate) fn new(file_path: &str, crate_name: &str, file_module_fqdn: String) -> Self {
        Self {
            core: WalkContextCore::new(file_path.to_string(), file_module_fqdn),
            crate_name: crate_name.to_string(),
            alias_table: HashMap::new(),
        }
    }

    pub(crate) fn push_symbol(&mut self, sym: RawSymbol) {
        self.core.push_symbol(sym);
    }

    pub(crate) fn push_edge(&mut self, edge: RawEdge) {
        self.core.push_edge(edge);
    }

    pub(crate) fn push_document(&mut self, doc: RawDocument) {
        self.core.push_document(doc);
    }

    /// Push the symbol and, if `attrs` carries an outer doc-comment chain,
    /// also push a `RawDocument` keyed by the symbol's FQDN.
    pub(crate) fn push_symbol_with_doc(&mut self, sym: RawSymbol, attrs: &[syn::Attribute]) {
        let fqdn = sym.fqdn.clone();
        self.push_symbol(sym);
        if let Some(description) = extract_doc::extract_outer(attrs) {
            self.push_document(RawDocument {
                symbol_fqdn: fqdn,
                description,
            });
        }
    }

    pub(crate) fn add_alias(&mut self, alias: String, canonical: String) {
        self.alias_table.insert(alias, canonical);
    }

    /// Strict canonicalization: only resolves Rust keywords (`crate`/`self`/`super`)
    /// and aliases populated from `use`/`extern_crate`. Returns `None` when the
    /// leading segment is opaque (no alias, no keyword) — the caller decides
    /// whether to apply a module-local fallback.
    pub(crate) fn canonicalize(&self, path: &str, current_module: &str) -> Option<String> {
        let segments: Vec<&str> = path.split("::").filter(|s| !s.is_empty()).collect();
        if segments.is_empty() {
            return None;
        }
        let first = segments[0];
        let rest = if segments.len() > 1 {
            segments[1..].join("::")
        } else {
            String::new()
        };

        match first {
            "crate" => Some(join_segments(&self.crate_name, &rest)),
            "self" => Some(join_segments(current_module, &rest)),
            "super" => {
                let parent = current_module.rsplit_once("::").map_or("", |(p, _)| p);
                if parent.is_empty() {
                    None
                } else {
                    Some(join_segments(parent, &rest))
                }
            }
            _ => self
                .alias_table
                .get(first)
                .map(|aliased| join_segments(aliased, &rest)),
        }
    }

    /// Resolve a path written inside `current_module` against the file-local
    /// definitions and alias-table. The strategy mirrors Rust 2018 lookup:
    /// 1. strict canonicalize (keyword + alias)
    /// 2. for a single-ident path with no alias, fall back to module-local
    ///    (`<current_module>::<ident>`)
    /// 3. multi-segment paths with no alias are kept text-as-written (likely
    ///    absolute/extern crate path); the pipeline `promote_unresolved` may
    ///    still match by exact FQDN.
    pub(crate) fn resolve_path(&self, path: &str, current_module: &str) -> ResolvedOrUnresolved {
        if let Some(canonical) = self.canonicalize(path, current_module) {
            return if self.core.defined_fqdns.contains(&canonical) {
                ResolvedOrUnresolved::Resolved { fqdn: canonical }
            } else {
                ResolvedOrUnresolved::Unresolved { name: canonical }
            };
        }
        let segments: Vec<&str> = path.split("::").filter(|s| !s.is_empty()).collect();
        if segments.len() == 1 {
            let module_local = format!("{current_module}::{}", segments[0]);
            if self.core.defined_fqdns.contains(&module_local) {
                return ResolvedOrUnresolved::Resolved { fqdn: module_local };
            }
            return ResolvedOrUnresolved::Unresolved { name: module_local };
        }
        ResolvedOrUnresolved::Unresolved {
            name: path.to_string(),
        }
    }
}

fn join_segments(prefix: &str, rest: &str) -> String {
    if rest.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}::{rest}")
    }
}

/// Serialize a `syn::Path` as `a::b::c` without whitespace, dropping generics.
pub(crate) fn path_to_string(path: &syn::Path) -> String {
    let mut out = String::new();
    let mut first = true;
    for seg in &path.segments {
        if !first {
            out.push_str("::");
        }
        first = false;
        out.push_str(&seg.ident.to_string());
    }
    out
}

pub(crate) fn walk(
    parsed: &syn::File,
    module_fqdn: &str,
    file_path: &str,
    crate_name: &str,
) -> (Vec<RawSymbol>, Vec<RawEdge>, Vec<RawDocument>) {
    let mut ctx = WalkContext::new(file_path, crate_name, module_fqdn.to_string());
    walk_p1(&mut ctx, &parsed.items, module_fqdn);
    walk_p2(&mut ctx, &parsed.items, module_fqdn);
    (ctx.core.symbols, ctx.core.edges, ctx.core.documents)
}

// Pass 1: items → symbols + IMPORTS (use/extern_crate) + IMPLEMENTS + alias-table.
fn walk_p1(ctx: &mut WalkContext, items: &[syn::Item], current_module: &str) {
    for item in items {
        process_item_p1(ctx, item, current_module);
    }
}

fn process_item_p1(ctx: &mut WalkContext, item: &syn::Item, current_module: &str) {
    match item {
        syn::Item::Fn(it) => {
            let path = ctx.core.file_path.clone();
            ctx.push_symbol_with_doc(extract_fn(it, current_module, &path), &it.attrs);
        }
        syn::Item::Struct(it) => {
            let path = ctx.core.file_path.clone();
            ctx.push_symbol_with_doc(extract_struct(it, current_module, &path), &it.attrs);
        }
        syn::Item::Enum(it) => {
            let path = ctx.core.file_path.clone();
            ctx.push_symbol_with_doc(extract_enum(it, current_module, &path), &it.attrs);
        }
        syn::Item::Union(it) => {
            let path = ctx.core.file_path.clone();
            ctx.push_symbol_with_doc(extract_union(it, current_module, &path), &it.attrs);
        }
        syn::Item::Trait(it) => extract_trait(ctx, it, current_module),
        syn::Item::Impl(it) => extract_impl(ctx, it, current_module),
        syn::Item::Type(it) => {
            let path = ctx.core.file_path.clone();
            ctx.push_symbol_with_doc(extract_type_alias(it, current_module, &path), &it.attrs);
        }
        syn::Item::Const(it) => {
            let path = ctx.core.file_path.clone();
            ctx.push_symbol_with_doc(extract_const(it, current_module, &path), &it.attrs);
        }
        syn::Item::Static(it) => {
            let path = ctx.core.file_path.clone();
            ctx.push_symbol_with_doc(extract_static(it, current_module, &path), &it.attrs);
        }
        syn::Item::Macro(it) => {
            let path = ctx.core.file_path.clone();
            if let Some(sym) = extract_macro_def(it, current_module, &path) {
                ctx.push_symbol_with_doc(sym, &it.attrs);
            }
        }
        syn::Item::Use(it) => extract_use::process_use(ctx, it, current_module),
        syn::Item::ExternCrate(it) => extract_use::process_extern_crate(ctx, it),
        syn::Item::Mod(it) => {
            if let Some((_, items)) = &it.content {
                let inner_fqdn = format!("{current_module}::{}", it.ident);
                walk_p1(ctx, items, &inner_fqdn);
            }
        }
        // Item::ForeignMod, Item::TraitAlias, Item::Verbatim → skip day-1.
        _ => {}
    }
}

// Pass 2: walk fn bodies for CALLS edges (relies on alias-table + defined_fqdns from P1).
fn walk_p2(ctx: &mut WalkContext, items: &[syn::Item], current_module: &str) {
    for item in items {
        process_item_p2(ctx, item, current_module);
    }
}

fn process_item_p2(ctx: &mut WalkContext, item: &syn::Item, current_module: &str) {
    match item {
        syn::Item::Fn(it) => {
            let fn_fqdn = format!("{current_module}::{}", it.sig.ident);
            extract_call::visit_block(ctx, &it.block, current_module, &fn_fqdn);
        }
        syn::Item::Impl(it) => {
            let Some(target_name) = self_ty_target_name(&it.self_ty) else {
                // Mirror `extract_impl`: skip P2 traversal for non-nominal
                // self-types so we never emit CALLS edges anchored on a
                // garbage `from_fqdn` like `crate::& mut A::method`.
                return;
            };
            let target_fqdn = format!("{current_module}::{target_name}");
            for impl_item in &it.items {
                if let syn::ImplItem::Fn(item_fn) = impl_item {
                    let fn_fqdn = format!("{target_fqdn}::{}", item_fn.sig.ident);
                    extract_call::visit_block(ctx, &item_fn.block, current_module, &fn_fqdn);
                }
            }
        }
        syn::Item::Trait(it) => {
            let trait_fqdn = format!("{current_module}::{}", it.ident);
            for trait_item in &it.items {
                if let syn::TraitItem::Fn(item_fn) = trait_item
                    && let Some(block) = &item_fn.default
                {
                    let fn_fqdn = format!("{trait_fqdn}::{}", item_fn.sig.ident);
                    extract_call::visit_block(ctx, block, current_module, &fn_fqdn);
                }
            }
        }
        syn::Item::Mod(it) => {
            if let Some((_, items)) = &it.content {
                let inner_fqdn = format!("{current_module}::{}", it.ident);
                walk_p2(ctx, items, &inner_fqdn);
            }
        }
        _ => {}
    }
}

fn extract_fn(item: &syn::ItemFn, parent_fqdn: &str, path: &str) -> RawSymbol {
    let name = item.sig.ident.to_string();
    let fqdn = format!("{parent_fqdn}::{name}");
    let mut sig = extract_signature(&item.sig);
    sig.modifiers.deprecated = extract_deprecated(&item.attrs);
    RawSymbol {
        name,
        fqdn,
        kind: Kind::Function,
        language_kind: LanguageKind::from("fn"),
        module: Some(parent_fqdn.to_string()),
        visibility: visibility::map(&item.vis),
        location: span_to_location(item.span(), path),
        signature: Some(sig),
        body_hash: Some(body_hash::hash_tokens(&item.to_token_stream())),
        attributes: extract_attributes(&item.attrs, path),
    }
}

fn extract_struct(item: &syn::ItemStruct, parent_fqdn: &str, path: &str) -> RawSymbol {
    type_def_symbol(
        item.ident.to_string(),
        parent_fqdn,
        path,
        "struct",
        &item.vis,
        item.span(),
        &item.to_token_stream(),
        &item.attrs,
    )
}

fn extract_enum(item: &syn::ItemEnum, parent_fqdn: &str, path: &str) -> RawSymbol {
    type_def_symbol(
        item.ident.to_string(),
        parent_fqdn,
        path,
        "enum",
        &item.vis,
        item.span(),
        &item.to_token_stream(),
        &item.attrs,
    )
}

fn extract_union(item: &syn::ItemUnion, parent_fqdn: &str, path: &str) -> RawSymbol {
    type_def_symbol(
        item.ident.to_string(),
        parent_fqdn,
        path,
        "union",
        &item.vis,
        item.span(),
        &item.to_token_stream(),
        &item.attrs,
    )
}

fn extract_type_alias(item: &syn::ItemType, parent_fqdn: &str, path: &str) -> RawSymbol {
    type_def_symbol(
        item.ident.to_string(),
        parent_fqdn,
        path,
        "type_alias",
        &item.vis,
        item.span(),
        &item.to_token_stream(),
        &item.attrs,
    )
}

#[allow(clippy::too_many_arguments)]
fn type_def_symbol(
    name: String,
    parent_fqdn: &str,
    path: &str,
    language_kind: &str,
    vis: &syn::Visibility,
    span: Span,
    tokens: &proc_macro2::TokenStream,
    attrs: &[syn::Attribute],
) -> RawSymbol {
    let fqdn = format!("{parent_fqdn}::{name}");
    RawSymbol {
        name,
        fqdn,
        kind: Kind::Type,
        language_kind: LanguageKind::from(language_kind),
        module: Some(parent_fqdn.to_string()),
        visibility: visibility::map(vis),
        location: span_to_location(span, path),
        signature: None,
        body_hash: Some(body_hash::hash_tokens(tokens)),
        attributes: extract_attributes(attrs, path),
    }
}

fn extract_trait(ctx: &mut WalkContext, item: &syn::ItemTrait, parent_fqdn: &str) {
    let path = ctx.core.file_path.clone();
    let name = item.ident.to_string();
    let trait_fqdn = format!("{parent_fqdn}::{name}");
    let trait_visibility = visibility::map(&item.vis);

    ctx.push_symbol_with_doc(
        RawSymbol {
            name,
            fqdn: trait_fqdn.clone(),
            kind: Kind::Type,
            language_kind: LanguageKind::from("trait"),
            module: Some(parent_fqdn.to_string()),
            visibility: trait_visibility,
            location: span_to_location(item.span(), &path),
            signature: None,
            body_hash: Some(body_hash::hash_tokens(&item.to_token_stream())),
            attributes: extract_attributes(&item.attrs, &path),
        },
        &item.attrs,
    );

    for trait_item in &item.items {
        if let syn::TraitItem::Fn(item_fn) = trait_item {
            let fn_name = item_fn.sig.ident.to_string();
            let fn_fqdn = format!("{trait_fqdn}::{fn_name}");
            let mut sig = extract_signature(&item_fn.sig);
            sig.modifiers.deprecated = extract_deprecated(&item_fn.attrs);
            ctx.push_symbol_with_doc(
                RawSymbol {
                    name: fn_name,
                    fqdn: fn_fqdn,
                    kind: Kind::Function,
                    language_kind: LanguageKind::from("trait_fn"),
                    module: Some(trait_fqdn.clone()),
                    visibility: trait_visibility,
                    location: span_to_location(item_fn.span(), &path),
                    signature: Some(sig),
                    body_hash: Some(body_hash::hash_tokens(&item_fn.to_token_stream())),
                    attributes: extract_attributes(&item_fn.attrs, &path),
                },
                &item_fn.attrs,
            );
        }
    }
}

fn extract_impl(ctx: &mut WalkContext, item: &syn::ItemImpl, parent_fqdn: &str) {
    let path = ctx.core.file_path.clone();
    let Some(target_name) = self_ty_target_name(&item.self_ty) else {
        // Non-nominal self-type (`&T`, `&mut T`, `Box<T>`, tuples, ...) —
        // methods inside are accessed via trait dispatch, not by FQDN.
        // Emitting them with a synthetic parent path produces garbage
        // FQDNs like `crate::& mut A::method`. Skip the whole block.
        return;
    };
    let target_fqdn = format!("{parent_fqdn}::{target_name}");

    if let Some((_, trait_path, _)) = &item.trait_ {
        let trait_str = path_to_string(trait_path);
        let span = item.span();
        let to = ctx.resolve_path(&trait_str, parent_fqdn);
        let confidence = to.default_confidence();
        ctx.push_edge(RawEdge {
            from_fqdn: target_fqdn.clone(),
            kind: EdgeKind::Implements,
            to,
            sites: vec![Site {
                file: path.clone(),
                line: line_from_span(span),
                col: col_from_span(span),
            }],
            attributes: vec![],
            confidence,
        });
    }

    for impl_item in &item.items {
        if let syn::ImplItem::Fn(item_fn) = impl_item {
            let fn_name = item_fn.sig.ident.to_string();
            let fn_fqdn = format!("{target_fqdn}::{fn_name}");
            let mut sig = extract_signature(&item_fn.sig);
            sig.modifiers.deprecated = extract_deprecated(&item_fn.attrs);
            ctx.push_symbol_with_doc(
                RawSymbol {
                    name: fn_name,
                    fqdn: fn_fqdn,
                    kind: Kind::Function,
                    language_kind: LanguageKind::from("impl_fn"),
                    module: Some(target_fqdn.clone()),
                    visibility: visibility::map(&item_fn.vis),
                    location: span_to_location(item_fn.span(), &path),
                    signature: Some(sig),
                    body_hash: Some(body_hash::hash_tokens(&item_fn.to_token_stream())),
                    attributes: extract_attributes(&item_fn.attrs, &path),
                },
                &item_fn.attrs,
            );
        }
    }
}

fn extract_const(item: &syn::ItemConst, parent_fqdn: &str, path: &str) -> RawSymbol {
    value_def_symbol(
        item.ident.to_string(),
        parent_fqdn,
        path,
        "const",
        &item.vis,
        item.span(),
        &item.to_token_stream(),
        &item.attrs,
    )
}

fn extract_static(item: &syn::ItemStatic, parent_fqdn: &str, path: &str) -> RawSymbol {
    value_def_symbol(
        item.ident.to_string(),
        parent_fqdn,
        path,
        "static",
        &item.vis,
        item.span(),
        &item.to_token_stream(),
        &item.attrs,
    )
}

#[allow(clippy::too_many_arguments)]
fn value_def_symbol(
    name: String,
    parent_fqdn: &str,
    path: &str,
    language_kind: &str,
    vis: &syn::Visibility,
    span: Span,
    tokens: &proc_macro2::TokenStream,
    attrs: &[syn::Attribute],
) -> RawSymbol {
    let fqdn = format!("{parent_fqdn}::{name}");
    RawSymbol {
        name,
        fqdn,
        kind: Kind::Value,
        language_kind: LanguageKind::from(language_kind),
        module: Some(parent_fqdn.to_string()),
        visibility: visibility::map(vis),
        location: span_to_location(span, path),
        signature: None,
        body_hash: Some(body_hash::hash_tokens(tokens)),
        attributes: extract_attributes(attrs, path),
    }
}

fn extract_macro_def(item: &syn::ItemMacro, parent_fqdn: &str, path: &str) -> Option<RawSymbol> {
    let name = item.ident.as_ref()?.to_string();
    let fqdn = format!("{parent_fqdn}::{name}");
    let exported = item.attrs.iter().any(|a| a.path().is_ident("macro_export"));
    let visibility = if exported {
        Visibility::Public
    } else {
        Visibility::Private
    };
    Some(RawSymbol {
        name,
        fqdn,
        kind: Kind::Macro,
        language_kind: LanguageKind::from("macro_rules"),
        module: Some(parent_fqdn.to_string()),
        visibility,
        location: span_to_location(item.span(), path),
        signature: None,
        body_hash: Some(body_hash::hash_tokens(&item.to_token_stream())),
        attributes: extract_attributes(&item.attrs, path),
    })
}

fn extract_signature(sig: &syn::Signature) -> Signature {
    let params = sig.inputs.iter().map(extract_param).collect();
    let returns = match &sig.output {
        syn::ReturnType::Default => None,
        syn::ReturnType::Type(_, ty) => Some(TypeRef::new(ty.to_token_stream().to_string())),
    };
    let generic_params = sig
        .generics
        .params
        .iter()
        .map(|p| p.to_token_stream().to_string())
        .collect();
    let where_clause = sig.generics.where_clause.as_ref().map(|wc| {
        // `to_token_stream` includes the leading `where` keyword which we
        // strip so consumers see just the predicates.
        let raw = wc.to_token_stream().to_string();
        match raw.strip_prefix("where ") {
            Some(s) => s.to_string(),
            None => raw,
        }
    });
    Signature {
        params,
        returns,
        modifiers: Modifiers {
            is_async: sig.asyncness.is_some(),
            deprecated: None,
            generic_params,
            where_clause,
        },
        meta: SignatureMeta::default(),
    }
}

fn extract_param(arg: &syn::FnArg) -> Param {
    match arg {
        syn::FnArg::Receiver(recv) => {
            let ty_str = if recv.reference.is_some() {
                if recv.mutability.is_some() {
                    "&mut Self"
                } else {
                    "&Self"
                }
            } else if recv.mutability.is_some() {
                "mut Self"
            } else {
                "Self"
            };
            Param {
                name: "self".into(),
                ty: TypeRef::new(ty_str),
                default: None,
            }
        }
        syn::FnArg::Typed(pat_type) => Param {
            name: pat_type.pat.to_token_stream().to_string(),
            ty: TypeRef::new(pat_type.ty.to_token_stream().to_string()),
            default: None,
        },
    }
}

fn extract_attributes(attrs: &[syn::Attribute], path: &str) -> Vec<RawAttribute> {
    attrs
        .iter()
        .map(|attr| RawAttribute {
            name: attr.path().to_token_stream().to_string(),
            args: meta_to_args(&attr.meta),
            site: Site {
                file: path.into(),
                line: line_from_span(attr.span()),
                col: col_from_span(attr.span()),
            },
        })
        .collect()
}

fn meta_to_args(meta: &syn::Meta) -> Vec<RawAttributeArg> {
    match meta {
        syn::Meta::Path(_) => vec![],
        syn::Meta::List(list) => vec![RawAttributeArg {
            key: None,
            value: list.tokens.to_string(),
            is_string_literal: false,
        }],
        syn::Meta::NameValue(nv) => vec![RawAttributeArg {
            key: None,
            value: nv.value.to_token_stream().to_string(),
            is_string_literal: false,
        }],
    }
}

fn extract_deprecated(attrs: &[syn::Attribute]) -> Option<String> {
    for attr in attrs {
        if !attr.path().is_ident("deprecated") {
            continue;
        }
        return Some(match &attr.meta {
            syn::Meta::Path(_) => String::new(),
            syn::Meta::List(list) => list.tokens.to_string(),
            syn::Meta::NameValue(nv) => nv.value.to_token_stream().to_string(),
        });
    }
    None
}

/// Returns the trailing identifier of a `Type::Path` (e.g. `Foo` for
/// `module::Foo<T>`), `None` when the self-type is not a nominal path
/// (e.g. `&T`, `&mut T`, `Box<T>`, `(A, B)`, …). Callers MUST skip the
/// surrounding `impl` block when this returns `None` — methods on
/// non-nominal self-types are not addressable by FQDN graph-wise and
/// concatenating `& mut T::method` would pollute the index with garbage
/// FQDNs.
fn self_ty_target_name(ty: &syn::Type) -> Option<String> {
    match ty {
        syn::Type::Path(tp) => tp.path.segments.last().map(|s| s.ident.to_string()),
        _ => None,
    }
}

pub(crate) fn span_to_location(span: Span, path: &str) -> SymbolLocation {
    let start = span.start();
    let end = span.end();
    SymbolLocation {
        file: path.into(),
        start_line: clamp_line(start.line),
        start_col: clamp_col(start.column),
        end_line: clamp_line(end.line),
        end_col: clamp_col(end.column),
    }
}

pub(crate) fn line_from_span(span: Span) -> u32 {
    clamp_line(span.start().line)
}

pub(crate) fn col_from_span(span: Span) -> u32 {
    clamp_col(span.start().column)
}

fn clamp_line(n: usize) -> u32 {
    let v = u32::try_from(n).unwrap_or(u32::MAX);
    v.max(1)
}

fn clamp_col(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> syn::File {
        syn::parse_file(src).expect("test source not parsable")
    }

    #[test]
    fn walks_simple_fn_emits_function_symbol() {
        let parsed = parse("fn foo() {}");
        let (symbols, edges, _docs) = walk(&parsed, "mycrate", "src/lib.rs", "mycrate");
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].kind, Kind::Function);
        assert_eq!(symbols[0].fqdn, "mycrate::foo");
        assert_eq!(symbols[0].name, "foo");
        assert_eq!(symbols[0].visibility, Visibility::Private);
        assert!(edges.is_empty());
    }

    #[test]
    fn pub_fn_visibility_is_public() {
        let parsed = parse("pub fn foo() {}");
        let (symbols, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert_eq!(symbols[0].visibility, Visibility::Public);
    }

    #[test]
    fn fn_signature_captures_params_and_return() {
        let parsed = parse("pub fn add(a: u32, b: u32) -> u32 { a + b }");
        let (symbols, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let sig = symbols[0].signature.as_ref().unwrap();
        assert_eq!(sig.params.len(), 2);
        assert_eq!(sig.params[0].name, "a");
        assert_eq!(sig.params[0].ty.display, "u32");
        assert_eq!(sig.params[1].name, "b");
        assert_eq!(sig.returns.as_ref().unwrap().display, "u32");
    }

    #[test]
    fn async_fn_modifier_set() {
        let parsed = parse("async fn boot() {}");
        let (symbols, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert!(symbols[0].signature.as_ref().unwrap().modifiers.is_async);
    }

    #[test]
    fn deprecated_attribute_propagates_to_modifier() {
        let parsed = parse("#[deprecated = \"use bar\"] fn foo() {}");
        let (symbols, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let dep = symbols[0]
            .signature
            .as_ref()
            .unwrap()
            .modifiers
            .deprecated
            .as_deref();
        assert_eq!(dep, Some("\"use bar\""));
    }

    #[test]
    fn self_receiver_renders_as_self_typeref() {
        let parsed =
            parse("impl Foo {\n  fn a(self) {}\n  fn b(&self) {}\n  fn c(&mut self) {}\n}");
        let (symbols, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let a = &symbols.iter().find(|s| s.name == "a").unwrap().signature;
        let b = &symbols.iter().find(|s| s.name == "b").unwrap().signature;
        let c = &symbols.iter().find(|s| s.name == "c").unwrap().signature;
        assert_eq!(a.as_ref().unwrap().params[0].ty.display, "Self");
        assert_eq!(b.as_ref().unwrap().params[0].ty.display, "&Self");
        assert_eq!(c.as_ref().unwrap().params[0].ty.display, "&mut Self");
    }

    #[test]
    fn struct_emits_type_symbol() {
        let parsed = parse("pub struct Foo { x: u32 }");
        let (symbols, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].kind, Kind::Type);
        assert_eq!(symbols[0].language_kind.as_str(), "struct");
        assert_eq!(symbols[0].fqdn, "c::Foo");
    }

    #[test]
    fn enum_emits_type_symbol() {
        let parsed = parse("enum E { A, B }");
        let (symbols, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert_eq!(symbols[0].kind, Kind::Type);
        assert_eq!(symbols[0].language_kind.as_str(), "enum");
    }

    #[test]
    fn trait_emits_type_and_inner_fn_symbols() {
        let parsed = parse("pub trait T { fn foo(&self); fn bar(&self) -> u32 { 0 } }");
        let (symbols, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert_eq!(symbols.len(), 3);
        assert_eq!(symbols[0].kind, Kind::Type);
        assert_eq!(symbols[0].language_kind.as_str(), "trait");
        assert_eq!(symbols[0].fqdn, "c::T");
        assert_eq!(symbols[1].kind, Kind::Function);
        assert_eq!(symbols[1].fqdn, "c::T::foo");
        assert_eq!(symbols[1].language_kind.as_str(), "trait_fn");
        assert_eq!(symbols[1].visibility, Visibility::Public);
        assert_eq!(symbols[2].fqdn, "c::T::bar");
    }

    #[test]
    fn inherent_impl_emits_method_symbols() {
        let parsed = parse("struct Foo; impl Foo { pub fn a(&self) {} fn b(&self) {} }");
        let (symbols, edges, _docs) = walk(&parsed, "c", "src/lib.rs", "c");
        assert!(edges.is_empty(), "no IMPLEMENTS for inherent impl");
        let foo = symbols.iter().find(|s| s.fqdn == "c::Foo").unwrap();
        assert_eq!(foo.kind, Kind::Type);
        let a = symbols.iter().find(|s| s.fqdn == "c::Foo::a").unwrap();
        assert_eq!(a.visibility, Visibility::Public);
        let b = symbols.iter().find(|s| s.fqdn == "c::Foo::b").unwrap();
        assert_eq!(b.visibility, Visibility::Private);
    }

    #[test]
    fn trait_impl_emits_implements_edge() {
        let parsed = parse("struct Foo; impl SomeTrait for Foo { fn run(&self) {} }");
        let (symbols, edges, _docs) = walk(&parsed, "c", "src/lib.rs", "c");
        let imp: Vec<_> = edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Implements)
            .collect();
        assert_eq!(imp.len(), 1);
        assert_eq!(imp[0].from_fqdn, "c::Foo");
        // No alias/local match → fallback to module-local canonical "c::SomeTrait".
        match &imp[0].to {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "c::SomeTrait"),
            other => panic!("expected unresolved, got {other:?}"),
        }
        assert!(symbols.iter().any(|s| s.fqdn == "c::Foo::run"));
    }

    #[test]
    fn impl_block_on_non_nominal_self_type_emits_nothing() {
        // `impl<T> Iterator for &mut T` — self-type is a reference, not a
        // Path. Methods inside are accessed via trait dispatch; concating
        // `&mut T::method` produces garbage FQDNs.
        let parsed = parse(
            "impl<T> Iterator for &mut T { type Item = (); fn next(&mut self) -> Option<()> { None } }",
        );
        let (symbols, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");

        assert!(
            !symbols.iter().any(|s| s.fqdn.contains('&')),
            "no symbol should reference `&` in its fqdn, got {:?}",
            symbols.iter().map(|s| &s.fqdn).collect::<Vec<_>>()
        );
        let impls: Vec<_> = edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Implements)
            .collect();
        assert!(
            impls.is_empty(),
            "impl on non-nominal self-type must emit no IMPLEMENTS edge"
        );
    }

    #[test]
    fn impl_block_on_tuple_self_type_emits_nothing() {
        let parsed = parse("impl SomeTrait for (u32, u32) { fn run(&self) {} }");
        let (symbols, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert!(symbols.is_empty(), "tuple self-type must emit no symbol");
        assert!(edges.is_empty(), "tuple self-type must emit no edge");
    }

    #[test]
    fn trait_impl_with_use_alias_resolves_implements_target() {
        let parsed =
            parse("use crate::traits::Foo; struct Bar; impl Foo for Bar { fn run(&self) {} }");
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let imp = edges
            .iter()
            .find(|e| e.kind == EdgeKind::Implements)
            .expect("implements edge");
        match &imp.to {
            ResolvedOrUnresolved::Unresolved { name } => {
                assert_eq!(name, "c::traits::Foo");
            }
            other => panic!("expected unresolved canonical, got {other:?}"),
        }
    }

    #[test]
    fn const_emits_value_symbol() {
        let parsed = parse("const N: u32 = 0;");
        let (symbols, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert_eq!(symbols[0].kind, Kind::Value);
        assert_eq!(symbols[0].language_kind.as_str(), "const");
        assert_eq!(symbols[0].fqdn, "c::N");
    }

    #[test]
    fn static_emits_value_symbol() {
        let parsed = parse("static GLOBAL: u32 = 0;");
        let (symbols, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert_eq!(symbols[0].kind, Kind::Value);
        assert_eq!(symbols[0].language_kind.as_str(), "static");
    }

    #[test]
    fn type_alias_emits_type_symbol() {
        let parsed = parse("pub type Bytes = Vec<u8>;");
        let (symbols, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert_eq!(symbols[0].kind, Kind::Type);
        assert_eq!(symbols[0].language_kind.as_str(), "type_alias");
    }

    #[test]
    fn macro_rules_with_export_is_public() {
        let parsed = parse("#[macro_export] macro_rules! say { () => {}; }");
        let (symbols, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].kind, Kind::Macro);
        assert_eq!(symbols[0].visibility, Visibility::Public);
    }

    #[test]
    fn macro_rules_without_export_is_private() {
        let parsed = parse("macro_rules! say { () => {}; }");
        let (symbols, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert_eq!(symbols[0].visibility, Visibility::Private);
    }

    #[test]
    fn inline_mod_pushes_fqdn_without_emitting_module_symbol() {
        let parsed = parse("mod inner { pub fn deep() {} }");
        let (symbols, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert_eq!(
            symbols.len(),
            1,
            "only the deep fn — no Module symbol for inline mod"
        );
        assert_eq!(symbols[0].kind, Kind::Function);
        assert_eq!(symbols[0].fqdn, "c::inner::deep");
    }

    #[test]
    fn attributes_are_captured_with_path_name() {
        let parsed = parse("#[derive(Debug, Clone)] pub struct X;");
        let (symbols, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert_eq!(symbols[0].attributes.len(), 1);
        assert_eq!(symbols[0].attributes[0].name, "derive");
        assert_eq!(symbols[0].attributes[0].args.len(), 1);
        assert_eq!(symbols[0].attributes[0].args[0].value, "Debug , Clone");
    }

    #[test]
    fn generic_params_captured_as_strings() {
        let parsed = parse("fn id<T>(x: T) -> T { x }");
        let (symbols, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let g = &symbols[0]
            .signature
            .as_ref()
            .unwrap()
            .modifiers
            .generic_params;
        assert_eq!(g.len(), 1);
        assert_eq!(g[0], "T");
    }

    #[test]
    fn where_clause_captured_as_text_without_leading_keyword() {
        let parsed = parse("fn foo<T>(x: T) where T: Send + Sync {}");
        let (symbols, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let wc = symbols[0]
            .signature
            .as_ref()
            .unwrap()
            .modifiers
            .where_clause
            .as_deref();
        assert!(wc.is_some(), "where clause must be captured");
        let text = wc.unwrap();
        assert!(
            !text.starts_with("where"),
            "leading `where` must be stripped: `{text}`"
        );
        assert!(text.contains("Send"));
        assert!(text.contains("Sync"));
    }

    #[test]
    fn where_clause_is_none_when_absent() {
        let parsed = parse("fn bar() {}");
        let (symbols, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let wc = &symbols[0]
            .signature
            .as_ref()
            .unwrap()
            .modifiers
            .where_clause;
        assert!(wc.is_none());
    }

    #[test]
    fn inline_generic_bounds_remain_in_generic_params() {
        let parsed = parse("fn foo<T: Display + Clone>(x: T) {}");
        let (symbols, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let g = &symbols[0]
            .signature
            .as_ref()
            .unwrap()
            .modifiers
            .generic_params;
        assert_eq!(g.len(), 1);
        assert!(g[0].contains("Display"), "got {g:?}");
        assert!(g[0].contains("Clone"), "got {g:?}");
    }

    #[test]
    fn span_locations_are_captured() {
        // proc-macro2 with span-locations feature gives 1-based lines for parsed source.
        let parsed = parse("\n\nfn foo() {}\n");
        let (symbols, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert_eq!(symbols[0].location.start_line, 3);
    }

    #[test]
    fn path_to_string_drops_whitespace_and_generics() {
        let p: syn::Path = syn::parse_str("crate::foo::Bar").unwrap();
        assert_eq!(path_to_string(&p), "crate::foo::Bar");
        let p2: syn::Path = syn::parse_str("Vec::<u8>::new").unwrap();
        assert_eq!(path_to_string(&p2), "Vec::new");
    }

    #[test]
    fn canonicalize_crate_keyword_replaces_with_crate_name() {
        let mut ctx = WalkContext::new("src/lib.rs", "mycrate", "mycrate".to_string());
        ctx.alias_table.clear();
        assert_eq!(
            ctx.canonicalize("crate::foo::bar", "mycrate"),
            Some("mycrate::foo::bar".to_string())
        );
        assert_eq!(
            ctx.canonicalize("crate", "mycrate"),
            Some("mycrate".to_string())
        );
    }

    #[test]
    fn canonicalize_self_resolves_to_current_module() {
        let ctx = WalkContext::new("src/foo.rs", "c", "c::foo".to_string());
        assert_eq!(
            ctx.canonicalize("self::bar", "c::foo"),
            Some("c::foo::bar".to_string())
        );
    }

    #[test]
    fn canonicalize_super_pops_one_level() {
        let ctx = WalkContext::new("src/a/b.rs", "c", "c::a::b".to_string());
        assert_eq!(
            ctx.canonicalize("super::x", "c::a::b"),
            Some("c::a::x".to_string())
        );
        // No parent → None.
        assert_eq!(ctx.canonicalize("super::x", "c"), None);
    }

    #[test]
    fn canonicalize_alias_then_remaining_segments() {
        let mut ctx = WalkContext::new("src/lib.rs", "c", "c".to_string());
        ctx.add_alias("HM".into(), "std::collections::HashMap".into());
        assert_eq!(
            ctx.canonicalize("HM::new", "c"),
            Some("std::collections::HashMap::new".to_string())
        );
    }

    #[test]
    fn canonicalize_strict_returns_none_for_unaliased_single_ident() {
        let ctx = WalkContext::new("src/lib.rs", "c", "c::foo".to_string());
        // Strict mode: no module-local fallback. The fallback lives in resolve_path.
        assert_eq!(ctx.canonicalize("bar", "c::foo"), None);
    }

    #[test]
    fn canonicalize_opaque_multi_segment_without_alias_returns_none() {
        let ctx = WalkContext::new("src/lib.rs", "c", "c".to_string());
        assert_eq!(ctx.canonicalize("std::mem::take", "c"), None);
    }

    #[test]
    fn resolve_path_single_ident_falls_back_to_module_local() {
        let mut ctx = WalkContext::new("src/lib.rs", "c", "c::foo".to_string());
        ctx.core.defined_fqdns.insert("c::foo::bar".to_string());
        assert!(matches!(
            ctx.resolve_path("bar", "c::foo"),
            ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "c::foo::bar"
        ));
    }

    #[test]
    fn resolve_path_multi_segment_without_alias_keeps_text_as_written() {
        let ctx = WalkContext::new("src/lib.rs", "c", "c".to_string());
        match ctx.resolve_path("std::mem::take", "c") {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "std::mem::take"),
            other => panic!("expected unresolved as-written, got {other:?}"),
        }
    }

    #[test]
    fn resolve_path_returns_resolved_when_canonical_matches_defined_fqdn() {
        let mut ctx = WalkContext::new("src/lib.rs", "c", "c".to_string());
        ctx.core.defined_fqdns.insert("c::foo".to_string());
        assert!(matches!(
            ctx.resolve_path("self::foo", "c"),
            ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "c::foo"
        ));
    }
}

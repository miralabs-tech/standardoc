//! Per-item-kind symbol extraction for the Rust walker.
//!
//! Extracted from `walk.rs` (Phase 3.2+ structure split): one `extract_*`
//! per top-level `syn::Item` kind (fn / struct / enum / type-alias / union
//! / trait / impl / const / static / macro_def) plus the tightly-coupled
//! field/signature/attribute helpers (`extract_signature`, `extract_param`,
//! `extract_attributes`, `meta_to_args`, `extract_deprecated`, `push_field`,
//! `push_struct_fields`, `type_def_symbol`, `value_def_symbol`,
//! `classify_fn_entry_point`, `render_compact`).

use proc_macro2::Span;
use quote::ToTokens;
use standardoc_ir::{
    BuiltinTier, DeclKind, EdgeKind, EntryPointKind, Kind, Language, LanguageKind, Modifiers,
    Param, RawAttribute, RawAttributeArg, RawEdge, RawSymbol, ResolvedOrUnresolved, Signature,
    SignatureMeta, Site, TypeRef, Visibility, compact_rust_tokens,
};
use syn::spanned::Spanned;

use super::super::body_hash;
use super::super::extract_type;
use super::super::visibility;
use super::{
    WalkContext, col_from_span, line_from_span, lookup_scope_for, path_to_string,
    self_ty_target_name, span_to_location,
};
use crate::builtins::global as global_builtin_registry;

pub(super) fn extract_fn(item: &syn::ItemFn, parent_fqdn: &str, path: &str) -> RawSymbol {
    let name = item.sig.ident.to_string();
    let fqdn = format!("{parent_fqdn}::{name}");
    let mut sig = extract_signature(&item.sig);
    sig.modifiers.deprecated = extract_deprecated(&item.attrs);
    let entry_point = classify_fn_entry_point(item, parent_fqdn, &name);
    RawSymbol {
        decl_kind: Some(DeclKind::Function),
        implements_trait: None,
        receiver_type: None,
        entry_point,
        name,
        fqdn,
        kind: Kind::Callable,
        language_kind: LanguageKind::from("fn"),
        module: Some(parent_fqdn.to_string()),
        visibility: visibility::map(&item.vis),
        location: span_to_location(item.span(), path),
        signature: Some(sig),
        body_hash: Some(body_hash::hash_tokens(&item.to_token_stream())),
        attributes: extract_attributes(&item.attrs, path),
        flags: vec![],
    }
}

/// Phase 3 (Flow) — first-pass entry-point detector for Rust free
/// functions. Recognises two unambiguous shapes:
///
///   - `BinaryMain`: a fn literally named `main` sitting at the crate
///     root (parent fqdn has no `::`, i.e. it IS the crate name).
///     Works for any binary target — `src/main.rs` or `src/bin/*.rs`.
///   - `FfiExport`: any fn carrying `#[no_mangle]`. The `pub extern`
///     part is the C-callable shape but `#[no_mangle]` is the
///     definitive opt-in marker — checking it alone avoids false
///     positives from `extern "Rust" fn` (no-op ABI tag).
///
/// `PublicApi` (a `pub fn` re-exported up to the crate root) is
/// deferred — detecting it needs the resolver's transitive
/// `pub mod` chain, not just the immediate parent module.
fn classify_fn_entry_point(
    item: &syn::ItemFn,
    parent_fqdn: &str,
    name: &str,
) -> Option<EntryPointKind> {
    if name == "main" && !parent_fqdn.contains("::") {
        return Some(EntryPointKind::BinaryMain);
    }
    let has_no_mangle = item.attrs.iter().any(|a| a.path().is_ident("no_mangle"));
    if has_no_mangle {
        return Some(EntryPointKind::FfiExport);
    }
    None
}

/// Bug C-2 — push the struct symbol AND one `RawSymbol` per field.
/// Named fields use the field ident as name; tuple struct fields use
/// the positional index as both name and the fqdn segment. Each field's
/// type is rendered as a `TypeRef` string and stored on
/// `signature.returns` — the closest existing IR slot for a non-fn
/// "this symbol exposes a single value of type T" relationship. Stage
/// 2b-equivalent `UsesType` edges from a field to its type are NOT
/// emitted here (deferred to Bug C-3 — the Rust counterpart of TS
/// Stage 2b).
pub(super) fn extract_struct(ctx: &mut WalkContext, item: &syn::ItemStruct, parent_fqdn: &str) {
    let path = ctx.core.file_path.clone();
    let struct_name = item.ident.to_string();
    let struct_fqdn = format!("{parent_fqdn}::{struct_name}");
    let parent_sym = type_def_symbol(
        struct_name,
        parent_fqdn,
        &path,
        "struct",
        DeclKind::Struct,
        &item.vis,
        item.span(),
        &item.to_token_stream(),
        &item.attrs,
    );
    ctx.push_symbol_with_doc(parent_sym, &item.attrs);
    // Bug C-3 / Stage 3a-8c — struct-level generics live at the
    // struct's lookup scope. resolve_local filters `T` in `<T: …>`
    // body refs naturally.
    let scope_idx = lookup_scope_for(ctx, item.span());
    push_struct_fields(
        ctx,
        &item.fields,
        &struct_fqdn,
        &path,
        parent_fqdn,
        scope_idx,
    );
    extract_type::visit_generics(ctx, &item.generics, parent_fqdn, &struct_fqdn, scope_idx);
}

/// Bug C-2 — push the enum symbol AND one `RawSymbol` per variant.
/// Variants are typed as `Kind::Type` (they construct a value of a
/// distinct sum-type case). Inner fields of tuple/struct variants are
/// NOT decomposed in v1 — that's a follow-up if usage demands it.
pub(super) fn extract_enum(ctx: &mut WalkContext, item: &syn::ItemEnum, parent_fqdn: &str) {
    let path = ctx.core.file_path.clone();
    let enum_name = item.ident.to_string();
    let enum_fqdn = format!("{parent_fqdn}::{enum_name}");
    let parent_sym = type_def_symbol(
        enum_name,
        parent_fqdn,
        &path,
        "enum",
        DeclKind::Enum,
        &item.vis,
        item.span(),
        &item.to_token_stream(),
        &item.attrs,
    );
    ctx.push_symbol_with_doc(parent_sym, &item.attrs);
    // Bug C-3 / Stage 3a-8c — enum-level generics live at the enum's
    // lookup scope.
    let scope_idx = lookup_scope_for(ctx, item.span());
    for variant in &item.variants {
        let variant_name = variant.ident.to_string();
        let variant_fqdn = format!("{enum_fqdn}::{variant_name}");
        ctx.push_symbol_with_doc(
            RawSymbol {
                decl_kind: Some(DeclKind::EnumVariant),
                implements_trait: None,
                receiver_type: None,
                entry_point: None,
                name: variant_name,
                fqdn: variant_fqdn.clone(),
                kind: Kind::Type,
                language_kind: LanguageKind::from("enum_variant"),
                module: Some(enum_fqdn.clone()),
                // Variants inherit the enum's visibility — they're not
                // independently exportable in Rust.
                visibility: visibility::map(&item.vis),
                location: span_to_location(variant.span(), &path),
                signature: None,
                body_hash: Some(body_hash::hash_tokens(&variant.to_token_stream())),
                attributes: extract_attributes(&variant.attrs, &path),
                flags: vec![],
            },
            &variant.attrs,
        );
        // Bug C-3: walk the variant's inner field types
        // (`enum E { V(Foo, Bar) }` → V → UsesType{Foo, Bar}).
        // Inner fields are NOT pushed as sub-symbols (deferred follow-up)
        // but their type references are emitted from the variant fqdn.
        match &variant.fields {
            syn::Fields::Named(named) => {
                for field in &named.named {
                    extract_type::visit_type(
                        ctx,
                        &field.ty,
                        parent_fqdn,
                        &variant_fqdn,
                        extract_type::TYPE_CTX_ANNOTATION,
                        scope_idx,
                    );
                }
            }
            syn::Fields::Unnamed(unnamed) => {
                for field in &unnamed.unnamed {
                    extract_type::visit_type(
                        ctx,
                        &field.ty,
                        parent_fqdn,
                        &variant_fqdn,
                        extract_type::TYPE_CTX_ANNOTATION,
                        scope_idx,
                    );
                }
            }
            syn::Fields::Unit => {}
        }
    }
    extract_type::visit_generics(ctx, &item.generics, parent_fqdn, &enum_fqdn, scope_idx);
}

/// Shared between `extract_struct` and (later) struct-variant
/// decomposition: walk a `syn::Fields` enum and push a sub-symbol per
/// named/tuple field. Unit fields produce nothing.
///
/// `scope_idx` anchors `UsesType` emission against the lookup so
/// struct/enum-level generics are filtered via the parent chain.
fn push_struct_fields(
    ctx: &mut WalkContext,
    fields: &syn::Fields,
    parent_fqdn: &str,
    path: &str,
    current_module: &str,
    scope_idx: u32,
) {
    match fields {
        syn::Fields::Named(named) => {
            for field in &named.named {
                let Some(ident) = &field.ident else { continue };
                push_field(
                    ctx,
                    field,
                    &ident.to_string(),
                    parent_fqdn,
                    path,
                    "field",
                    current_module,
                    scope_idx,
                );
            }
        }
        syn::Fields::Unnamed(unnamed) => {
            for (idx, field) in unnamed.unnamed.iter().enumerate() {
                push_field(
                    ctx,
                    field,
                    &idx.to_string(),
                    parent_fqdn,
                    path,
                    "tuple_field",
                    current_module,
                    scope_idx,
                );
            }
        }
        syn::Fields::Unit => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn push_field(
    ctx: &mut WalkContext,
    field: &syn::Field,
    name: &str,
    parent_fqdn: &str,
    path: &str,
    language_kind: &str,
    current_module: &str,
    scope_idx: u32,
) {
    let field_fqdn = format!("{parent_fqdn}::{name}");
    let ty_str = compact_rust_tokens(&field.ty.to_token_stream().to_string());

    // Bug E-3 Phase 1: capture nominal type for named struct fields.
    // Tuple fields go through this same path but use numeric names
    // (`"0"`, `"1"`, ...) — Phase 1 doesn't resolve numeric receivers
    // (`self.0.method`) so this is harmless noise; lookup never matches.
    if language_kind == "field" {
        ctx.struct_fields.record(parent_fqdn, name, &field.ty);
    }

    let signature = Signature {
        params: vec![],
        returns: Some(TypeRef::new(ty_str)),
        modifiers: Modifiers::default(),
        meta: SignatureMeta::default(),
    };
    ctx.push_symbol_with_doc(
        RawSymbol {
            decl_kind: Some(DeclKind::Field),
            implements_trait: None,
            receiver_type: None,
            entry_point: None,
            name: name.to_string(),
            fqdn: field_fqdn.clone(),
            kind: Kind::Value,
            language_kind: LanguageKind::from(language_kind),
            module: Some(parent_fqdn.to_string()),
            visibility: visibility::map(&field.vis),
            location: span_to_location(field.span(), path),
            signature: Some(signature),
            body_hash: Some(body_hash::hash_tokens(&field.to_token_stream())),
            attributes: extract_attributes(&field.attrs, path),
            flags: vec![],
        },
        &field.attrs,
    );
    // Bug C-3: emit UsesType from the field fqdn for every named type
    // inside the field's annotation.
    extract_type::visit_type(
        ctx,
        &field.ty,
        current_module,
        &field_fqdn,
        extract_type::TYPE_CTX_ANNOTATION,
        scope_idx,
    );
}

pub(super) fn extract_union(item: &syn::ItemUnion, parent_fqdn: &str, path: &str) -> RawSymbol {
    type_def_symbol(
        item.ident.to_string(),
        parent_fqdn,
        path,
        "union",
        DeclKind::Union,
        &item.vis,
        item.span(),
        &item.to_token_stream(),
        &item.attrs,
    )
}

pub(super) fn extract_type_alias(item: &syn::ItemType, parent_fqdn: &str, path: &str) -> RawSymbol {
    type_def_symbol(
        item.ident.to_string(),
        parent_fqdn,
        path,
        "type_alias",
        DeclKind::TypeAlias,
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
    decl_kind: DeclKind,
    vis: &syn::Visibility,
    span: Span,
    tokens: &proc_macro2::TokenStream,
    attrs: &[syn::Attribute],
) -> RawSymbol {
    let fqdn = format!("{parent_fqdn}::{name}");
    RawSymbol {
        decl_kind: Some(decl_kind),
        implements_trait: None,
        receiver_type: None,
        entry_point: None,
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
        flags: vec![],
    }
}

pub(super) fn extract_trait(ctx: &mut WalkContext, item: &syn::ItemTrait, parent_fqdn: &str) {
    let path = ctx.core.file_path.clone();
    let name = item.ident.to_string();
    let trait_fqdn = format!("{parent_fqdn}::{name}");
    let trait_visibility = visibility::map(&item.vis);

    ctx.push_symbol_with_doc(
        RawSymbol {
            decl_kind: Some(DeclKind::Interface),
            implements_trait: None,
            receiver_type: None,
            entry_point: None,
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
            flags: vec![],
        },
        &item.attrs,
    );

    // Bug C-3 / Stage 3a-8c — trait-level generics live at the trait's
    // lookup scope; trait method scopes inherit via the parent chain.
    let trait_scope = lookup_scope_for(ctx, item.span());
    // Walk supertrait bounds (`trait T: Foo + Bar`) with type-extends.
    for bound in &item.supertraits {
        extract_type::visit_type_param_bound(
            ctx,
            bound,
            parent_fqdn,
            &trait_fqdn,
            extract_type::TYPE_CTX_EXTENDS,
            trait_scope,
        );
    }
    extract_type::visit_generics(ctx, &item.generics, parent_fqdn, &trait_fqdn, trait_scope);

    for trait_item in &item.items {
        if let syn::TraitItem::Fn(item_fn) = trait_item {
            let fn_name = item_fn.sig.ident.to_string();
            let fn_fqdn = format!("{trait_fqdn}::{fn_name}");
            let mut sig = extract_signature(&item_fn.sig);
            sig.modifiers.deprecated = extract_deprecated(&item_fn.attrs);
            ctx.push_symbol_with_doc(
                RawSymbol {
                    decl_kind: Some(DeclKind::Method),
                    implements_trait: None,
                    receiver_type: Some(TypeRef::new(&trait_fqdn)),
                    entry_point: None,
                    name: fn_name,
                    fqdn: fn_fqdn.clone(),
                    kind: Kind::Callable,
                    language_kind: LanguageKind::from("trait_fn"),
                    module: Some(trait_fqdn.clone()),
                    visibility: trait_visibility,
                    location: span_to_location(item_fn.span(), &path),
                    signature: Some(sig),
                    body_hash: Some(body_hash::hash_tokens(&item_fn.to_token_stream())),
                    attributes: extract_attributes(&item_fn.attrs, &path),
                    flags: vec![],
                },
                &item_fn.attrs,
            );
            // The method's own scope_idx has the trait scope as parent;
            // resolve_local from the method scope sees both fn-level
            // and trait-level generics naturally.
            let fn_scope = lookup_scope_for(ctx, item_fn.span());
            extract_type::visit_signature(ctx, &item_fn.sig, parent_fqdn, &fn_fqdn, fn_scope);
        }
    }
}

pub(super) fn extract_impl(ctx: &mut WalkContext, item: &syn::ItemImpl, parent_fqdn: &str) {
    let path = ctx.core.file_path.clone();
    let Some(target_name) = self_ty_target_name(&item.self_ty) else {
        // Non-nominal self-type (`&T`, `&mut T`, `Box<T>`, tuples, ...) —
        // methods inside are accessed via trait dispatch, not by FQDN.
        // Emitting them with a synthetic parent path produces garbage
        // FQDNs like `crate::& mut A::method`. Skip the whole block.
        return;
    };
    let target_fqdn = format!("{parent_fqdn}::{target_name}");

    // K-Step-C: capture the raw trait path so impl_fn emission below
    // can stamp `implements_trait` on each method. Resolution to a
    // canonical FQDN happens later in the pipeline (mirrors the
    // `Implements` edge's `to: ResolvedOrUnresolved` shape).
    let implements_trait_str = item
        .trait_
        .as_ref()
        .map(|(_, trait_path, _)| path_to_string(trait_path));

    if let Some((_, trait_path, _)) = &item.trait_ {
        let trait_str = path_to_string(trait_path);
        let span = item.span();
        // Bug B fix — consult the builtin registry on the trait's
        // leftmost segment BEFORE falling through to the local-module
        // resolver. Pre-fix, `impl Drop for X` produced a bogus
        // `standardoc-cli::Drop` IMPLEMENTS target because resolve_path's
        // single-ident fallback prefixes the current module. Now:
        //   - tier::Drop (Drop/Default/From/Clone/Display/...) → skip,
        //     mirrors the policy used for value-position references
        //   - tier::Attribute (Iterator/Future/Stream) → skip the edge;
        //     attribute promotion is the visitor's job, not an IMPLEMENTS
        //   - tier::Edge (Error) → emit with the synthetic FQDN +
        //     via-builtin attrs so the focus-graph keeps the semantic
        //     "this is an error type" signal
        let leftmost = trait_str.split("::").next().unwrap_or("");
        let builtin = global_builtin_registry().lookup(leftmost, Language::Rust);
        let emit = match builtin {
            Some(entry) => match entry.tier {
                BuiltinTier::Drop | BuiltinTier::Attribute => None,
                BuiltinTier::Edge => Some((
                    ResolvedOrUnresolved::Resolved {
                        fqdn: entry.synthetic_fqdn.clone(),
                    },
                    vec!["via-builtin".to_string()],
                )),
            },
            None => Some((ctx.resolve_path(&trait_str, parent_fqdn), vec![])),
        };
        if let Some((to, attrs)) = emit {
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
                attributes: attrs,
                confidence,
                receiver_type: None,
            });
        }
    }

    // Bug C-3 / Stage 3a-8c — impl-level generics live at the impl's
    // lookup scope; impl method scopes inherit via the parent chain.
    let impl_scope = lookup_scope_for(ctx, item.span());
    extract_type::visit_generics(ctx, &item.generics, parent_fqdn, &target_fqdn, impl_scope);
    if let Some((_, trait_path, _)) = &item.trait_ {
        // Walk the trait path's generic args (`impl Trait<Foo> for X`
        // → UsesType{Foo} with type-implements). The trait path
        // itself already produced an `Implements` edge above; this
        // adds the inner args as `UsesType`.
        for seg in &trait_path.segments {
            if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                for arg in &args.args {
                    if let syn::GenericArgument::Type(ty) = arg {
                        extract_type::visit_type(
                            ctx,
                            ty,
                            parent_fqdn,
                            &target_fqdn,
                            extract_type::TYPE_CTX_IMPLEMENTS,
                            impl_scope,
                        );
                    }
                }
            }
        }
    }

    for impl_item in &item.items {
        if let syn::ImplItem::Fn(item_fn) = impl_item {
            let fn_name = item_fn.sig.ident.to_string();
            let fn_fqdn = format!("{target_fqdn}::{fn_name}");
            // Bug E-3 extensions P-E3.1: record the method's nominal
            // return type so `type_of_expr` can propagate chains like
            // `repo.find_by_id(id).name` where `find_by_id` is workspace-
            // defined. Mirror of the free-fn recording in `process_item_p1`.
            if let syn::ReturnType::Type(_, ty) = &item_fn.sig.output {
                ctx.return_types.record(&fn_fqdn, ty);
            }
            let mut sig = extract_signature(&item_fn.sig);
            sig.modifiers.deprecated = extract_deprecated(&item_fn.attrs);
            ctx.push_symbol_with_doc(
                RawSymbol {
                    decl_kind: Some(DeclKind::Method),
                    implements_trait: implements_trait_str.clone(),
                    receiver_type: Some(TypeRef::new(&target_fqdn)),
                    entry_point: None,
                    name: fn_name,
                    fqdn: fn_fqdn.clone(),
                    kind: Kind::Callable,
                    language_kind: LanguageKind::from("impl_fn"),
                    module: Some(target_fqdn.clone()),
                    visibility: visibility::map(&item_fn.vis),
                    location: span_to_location(item_fn.span(), &path),
                    signature: Some(sig),
                    body_hash: Some(body_hash::hash_tokens(&item_fn.to_token_stream())),
                    attributes: extract_attributes(&item_fn.attrs, &path),
                    flags: vec![],
                },
                &item_fn.attrs,
            );
            // Same as trait method: fn's own scope_idx has impl scope
            // as parent, so resolve_local sees both layers' generics.
            let fn_scope = lookup_scope_for(ctx, item_fn.span());
            extract_type::visit_signature(ctx, &item_fn.sig, parent_fqdn, &fn_fqdn, fn_scope);
        }
    }
}

pub(super) fn extract_const(item: &syn::ItemConst, parent_fqdn: &str, path: &str) -> RawSymbol {
    value_def_symbol(
        item.ident.to_string(),
        parent_fqdn,
        path,
        "const",
        DeclKind::Const,
        &item.vis,
        item.span(),
        &item.to_token_stream(),
        &item.attrs,
    )
}

pub(super) fn extract_static(item: &syn::ItemStatic, parent_fqdn: &str, path: &str) -> RawSymbol {
    value_def_symbol(
        item.ident.to_string(),
        parent_fqdn,
        path,
        "static",
        DeclKind::Static,
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
    decl_kind: DeclKind,
    vis: &syn::Visibility,
    span: Span,
    tokens: &proc_macro2::TokenStream,
    attrs: &[syn::Attribute],
) -> RawSymbol {
    let fqdn = format!("{parent_fqdn}::{name}");
    RawSymbol {
        decl_kind: Some(decl_kind),
        implements_trait: None,
        receiver_type: None,
        entry_point: None,
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
        flags: vec![],
    }
}

pub(super) fn extract_macro_def(
    item: &syn::ItemMacro,
    parent_fqdn: &str,
    path: &str,
) -> Option<RawSymbol> {
    let name = item.ident.as_ref()?.to_string();
    let fqdn = format!("{parent_fqdn}::{name}");
    let exported = item.attrs.iter().any(|a| a.path().is_ident("macro_export"));
    let visibility = if exported {
        Visibility::Public
    } else {
        Visibility::Private
    };
    Some(RawSymbol {
        decl_kind: Some(DeclKind::DeclarativeMacro),
        implements_trait: None,
        receiver_type: None,
        entry_point: None,
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
        flags: vec![],
    })
}

fn extract_signature(sig: &syn::Signature) -> Signature {
    let params = sig.inputs.iter().map(extract_param).collect();
    let returns = match &sig.output {
        syn::ReturnType::Default => None,
        syn::ReturnType::Type(_, ty) => Some(TypeRef::new(render_compact(ty))),
    };
    let generic_params = sig.generics.params.iter().map(render_compact).collect();
    let where_clause = sig.generics.where_clause.as_ref().map(|wc| {
        // `to_token_stream` includes the leading `where` keyword which we
        // strip so consumers see just the predicates.
        let raw = render_compact(wc);
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
            name: render_compact(pat_type.pat.as_ref()),
            ty: TypeRef::new(render_compact(pat_type.ty.as_ref())),
            default: None,
        },
    }
}

fn extract_attributes(attrs: &[syn::Attribute], path: &str) -> Vec<RawAttribute> {
    attrs
        .iter()
        .map(|attr| RawAttribute {
            name: render_compact(attr.path()),
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
            value: compact_rust_tokens(&list.tokens.to_string()),
            is_string_literal: false,
        }],
        syn::Meta::NameValue(nv) => vec![RawAttributeArg {
            key: None,
            value: render_compact(&nv.value),
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
            syn::Meta::List(list) => compact_rust_tokens(&list.tokens.to_string()),
            syn::Meta::NameValue(nv) => render_compact(&nv.value),
        });
    }
    None
}

/// Local helper: renders a `ToTokens`-bearing AST node into the compact
/// canonical Rust display form. The Rust provider sources every `display`
/// / `name` string from `quote`'s pretty-printer, which inserts a space
/// between every token tree — `compact_rust_tokens` re-collapses those
/// spaces so the IR row payload is small.
fn render_compact<T: ToTokens + ?Sized>(t: &T) -> String {
    compact_rust_tokens(&t.to_token_stream().to_string())
}

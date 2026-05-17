use std::collections::{HashMap, HashSet};

use proc_macro2::Span;
use quote::ToTokens;
use standardoc_ir::{
    AliasMutability, BuiltinTag, BuiltinTier, EdgeKind, Kind, Language, LanguageKind, Modifiers,
    ModuleLookup, Param, RawAttribute, RawAttributeArg, RawCallSite, RawDocument, RawEdge,
    RawSymbol, ResolvedOrUnresolved, Signature, SignatureMeta, Site, SymbolLocation, TypeRef,
    Visibility, compact_rust_tokens,
};
use syn::spanned::Spanned;

use crate::builtins::global as global_builtin_registry;
use crate::walk_core::WalkContextCore;

use super::body_hash;
use super::extract_call;
use super::extract_doc;
use super::extract_type;
use super::extract_use;
use super::lookup as rust_lookup;
use super::visibility;

/// Recover the `ModuleLookup` scope_idx for the AST node spanning `span`.
/// Falls back to `ROOT_SCOPE` when the pre-pass didn't register the
/// span — the caller still gets a sensible enclosing scope for
/// `resolve_local` queries (module-level generics + imports stay
/// reachable).
pub(crate) fn lookup_scope_for(ctx: &WalkContext, span: Span) -> u32 {
    let (lo, hi) = rust_lookup::scope_span_key(span);
    ctx.core
        .lookup
        .scope_idx_for_span(lo, hi)
        .unwrap_or(ModuleLookup::ROOT_SCOPE)
}

pub(crate) struct WalkContext {
    pub(crate) core: WalkContextCore,
    pub(crate) crate_name: String,
    pub(crate) alias_table: HashMap<String, String>,
    /// Stage 3e-1b — flags accumulated from Attribute-tier builtin
    /// hits during the walk. Keyed by the source symbol's FQDN
    /// (the enclosing fn / struct / impl method owning the touched
    /// trait / type / call). Flushed onto `core.symbols[*].flags`
    /// before `walk()` returns. Mirrors the TS-side `TsWalkContext`
    /// machinery.
    pub(crate) attribute_flags: HashMap<String, HashSet<String>>,
}

impl WalkContext {
    pub(crate) fn new(file_path: &str, crate_name: &str, file_module_fqdn: String) -> Self {
        Self {
            core: WalkContextCore::new(file_path.to_string(), file_module_fqdn, Language::Rust),
            crate_name: crate_name.to_string(),
            alias_table: HashMap::new(),
            attribute_flags: HashMap::new(),
        }
    }

    /// Stage 3e-1b — record a builtin tag against `source_fqdn` so the
    /// post-walk flush stamps it onto the symbol's `flags` vec. Best-
    /// effort : duplicates collapse into a `HashSet` so the same flag
    /// never lands twice on the same symbol regardless of how many
    /// times the Attribute-tier builtin is touched in the symbol body.
    pub(crate) fn register_attribute_flag(&mut self, source_fqdn: &str, tag: &BuiltinTag) {
        self.attribute_flags
            .entry(source_fqdn.to_string())
            .or_default()
            .insert(tag.slug());
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

    pub(crate) fn push_call_site(&mut self, cs: RawCallSite) {
        self.core.push_call_site(cs);
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

    /// Stage 3e-2 — resolve a name read in value position. Pipeline mirrors
    /// the TS-side `ts::visit::CallVisitor::resolve_name`:
    ///
    /// 1. Single-ident paths consult [`ModuleLookup::resolve_local`] first.
    ///    - Hit at [`ModuleLookup::ROOT_SCOPE`] (hoisted item / import) →
    ///      fall through to module-level resolution; root-scope locals are
    ///      already covered by `defined_fqdns` + `alias_table`.
    ///    - Hit at nested scope with alias → propagate the alias's
    ///      canonical-text through module-level resolution, carrying the
    ///      [`AliasMutability`] so the visitor can stamp `via-alias[-mutable]`.
    ///    - Hit at nested scope without alias → [`NameResolution::Local`].
    /// 2. Multi-segment paths (`Foo::CONST`) skip the scope walk — locals
    ///    don't have `::`.
    /// 3. Module-level resolution checks the leftmost segment against the
    ///    builtin registry (Drop/Attribute/Edge tiers), then falls through
    ///    to [`WalkContext::resolve_path`].
    pub(crate) fn resolve_name(
        &self,
        path: &str,
        scope_idx: u32,
        current_module: &str,
    ) -> NameResolution {
        let segments: Vec<&str> = path.split("::").filter(|s| !s.is_empty()).collect();
        if segments.is_empty() {
            return NameResolution::Drop;
        }

        if segments.len() == 1 {
            if let Some(res) = self.core.lookup.resolve_local(segments[0], scope_idx) {
                if res.scope_idx != ModuleLookup::ROOT_SCOPE {
                    if let (Some(alias_str), Some(m)) = (res.aliases_to.as_deref(), res.mutability)
                    {
                        return self.resolve_module_level(alias_str, current_module, Some(m));
                    }
                    return NameResolution::Local;
                }
                // ROOT_SCOPE — fall through to module-level resolution.
            }
        }

        self.resolve_module_level(path, current_module, None)
    }

    /// Stage 3e-2 helper — module-level half of [`Self::resolve_name`].
    /// Builtin tier check on the leftmost segment, then [`Self::resolve_path`]
    /// fallback. Wraps the outcome in [`NameResolution::Target`] preserving
    /// the optional `alias_mut` propagated by the caller.
    fn resolve_module_level(
        &self,
        path: &str,
        current_module: &str,
        alias_mut: Option<AliasMutability>,
    ) -> NameResolution {
        let leftmost = path.split("::").next().unwrap_or("");
        if let Some(entry) = global_builtin_registry().lookup(leftmost, Language::Rust) {
            return match entry.tier {
                BuiltinTier::Drop => NameResolution::Drop,
                BuiltinTier::Attribute => NameResolution::Attribute(entry.tag.clone()),
                BuiltinTier::Edge => NameResolution::Target {
                    to: ResolvedOrUnresolved::Resolved {
                        fqdn: entry.synthetic_fqdn.clone(),
                    },
                    alias_mut,
                    via_builtin: Some(entry.tag.clone()),
                },
            };
        }
        NameResolution::Target {
            to: self.resolve_path(path, current_module),
            alias_mut,
            via_builtin: None,
        }
    }
}

/// Stage 3e-2 — outcome of resolving a name (single-ident or multi-segment
/// path) read in value position against the AOT [`ModuleLookup`] scope chain
/// plus [`WalkContext::resolve_path`] fall-through. Mirrors
/// `ts::visit::NameResolution`. Callers pattern-match the variants to decide
/// between emitting an edge (`Target`) or skipping (`Local` for nested-scope
/// bindings without alias, `Drop` for tier-classified noise, `Attribute` for
/// tier-classified source-flag promotion targets).
#[derive(Debug, Clone)]
pub(crate) enum NameResolution {
    /// Builtin classified as [`BuiltinTier::Drop`] (`Vec`, `Box`, `Some`,
    /// `Ok`, `String::from`, …). Caller silently skips emission.
    Drop,
    /// Nested-scope local binding (let, fn param, closure param, match arm)
    /// without an alias. Caller skips emission — locals aren't surfaced in
    /// the module graph by design.
    Local,
    /// Builtin classified as [`BuiltinTier::Attribute`] (`Iterator`,
    /// `IntoIterator`, `Future`, `Stream`, …). Caller skips the edge but
    /// registers the carried [`BuiltinTag`] against the enclosing FQDN via
    /// [`WalkContext::register_attribute_flag`] so the symbol picks up
    /// `"async"` / `"iter"` / custom UST flags in its `flags` vec.
    Attribute(BuiltinTag),
    /// Resolvable target — caller emits the edge with the carried `to`,
    /// optional `alias_mut` (Some when reached through a scope alias), and
    /// optional `via_builtin` (Some when the leftmost segment matched an
    /// [`BuiltinTier::Edge`]-tier builtin so the emitter can stamp
    /// `via-builtin` / `builtin-<slug>` attrs).
    Target {
        to: ResolvedOrUnresolved,
        alias_mut: Option<AliasMutability>,
        via_builtin: Option<BuiltinTag>,
    },
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
) -> (
    Vec<RawSymbol>,
    Vec<RawEdge>,
    Vec<RawDocument>,
    Vec<RawCallSite>,
) {
    let (s, e, d, c, _lookup) = walk_with_lookup(parsed, module_fqdn, file_path, crate_name);
    (s, e, d, c)
}

/// Stage 3 final-mile (R1) — same as [`walk`] but also returns the AOT
/// [`ModuleLookup`] so callers (production `extract_file`) can stash it
/// in [`ExtractedFile::module_lookup`] for pipeline persistence. Tests
/// continue to use [`walk`] when they don't care about the lookup.
pub(crate) fn walk_with_lookup(
    parsed: &syn::File,
    module_fqdn: &str,
    file_path: &str,
    crate_name: &str,
) -> (
    Vec<RawSymbol>,
    Vec<RawEdge>,
    Vec<RawDocument>,
    Vec<RawCallSite>,
    ModuleLookup,
) {
    let mut ctx = WalkContext::new(file_path, crate_name, module_fqdn.to_string());
    ctx.core.lookup = super::lookup::build_rust_lookup(parsed, module_fqdn);
    walk_p1(&mut ctx, &parsed.items, module_fqdn);
    walk_p2(&mut ctx, &parsed.items, module_fqdn);
    flush_attribute_flags(&mut ctx);
    (
        ctx.core.symbols,
        ctx.core.edges,
        ctx.core.documents,
        ctx.core.call_sites,
        ctx.core.lookup,
    )
}

/// Stage 3e-1b — apply Attribute-tier flags accumulated during the walk
/// onto each affected symbol's `flags` vec. Sorted + dedup'd so the
/// resulting order is deterministic across runs (important for the
/// body_hash-driven `apply_edges` plan that decides re-extraction
/// boundaries).
fn flush_attribute_flags(ctx: &mut WalkContext) {
    if ctx.attribute_flags.is_empty() {
        return;
    }
    let mut by_fqdn: HashMap<String, Vec<String>> = HashMap::new();
    for (fqdn, flags) in ctx.attribute_flags.drain() {
        let mut sorted: Vec<String> = flags.into_iter().collect();
        sorted.sort();
        by_fqdn.insert(fqdn, sorted);
    }
    for sym in ctx.core.symbols.iter_mut() {
        if let Some(extra) = by_fqdn.get(&sym.fqdn) {
            for f in extra {
                if !sym.flags.contains(f) {
                    sym.flags.push(f.clone());
                }
            }
        }
    }
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
            let fn_fqdn = format!("{current_module}::{}", it.sig.ident);
            ctx.push_symbol_with_doc(extract_fn(it, current_module, &path), &it.attrs);
            // Bug C-3: walk the signature for UsesType edges. Fn-level
            // Stage 3a-8c — fn-level + module-level generics are
            // reachable via the lookup's parent chain from the fn's
            // own scope_idx.
            let scope_idx = lookup_scope_for(ctx, it.span());
            extract_type::visit_signature(ctx, &it.sig, current_module, &fn_fqdn, scope_idx);
        }
        syn::Item::Struct(it) => {
            extract_struct(ctx, it, current_module);
        }
        syn::Item::Enum(it) => {
            extract_enum(ctx, it, current_module);
        }
        syn::Item::Union(it) => {
            let path = ctx.core.file_path.clone();
            let union_fqdn = format!("{current_module}::{}", it.ident);
            ctx.push_symbol_with_doc(extract_union(it, current_module, &path), &it.attrs);
            // Bug C-3: walk each union field's type for UsesType edges.
            let scope_idx = lookup_scope_for(ctx, it.span());
            for field in &it.fields.named {
                extract_type::visit_type(
                    ctx,
                    &field.ty,
                    current_module,
                    &union_fqdn,
                    extract_type::TYPE_CTX_ANNOTATION,
                    scope_idx,
                );
            }
            extract_type::visit_generics(ctx, &it.generics, current_module, &union_fqdn, scope_idx);
        }
        syn::Item::Trait(it) => extract_trait(ctx, it, current_module),
        syn::Item::Impl(it) => extract_impl(ctx, it, current_module),
        syn::Item::Type(it) => {
            let path = ctx.core.file_path.clone();
            let alias_fqdn = format!("{current_module}::{}", it.ident);
            ctx.push_symbol_with_doc(extract_type_alias(it, current_module, &path), &it.attrs);
            // Bug C-3: walk the alias RHS body for UsesType edges
            // (`type X<T> = Vec<Foo<T>>` → edge to Foo from X with
            // type-alias-body context; `T` is filtered via the lookup).
            let scope_idx = lookup_scope_for(ctx, it.span());
            extract_type::visit_type(
                ctx,
                &it.ty,
                current_module,
                &alias_fqdn,
                extract_type::TYPE_CTX_ALIAS_BODY,
                scope_idx,
            );
            extract_type::visit_generics(ctx, &it.generics, current_module, &alias_fqdn, scope_idx);
        }
        syn::Item::Const(it) => {
            let path = ctx.core.file_path.clone();
            let const_fqdn = format!("{current_module}::{}", it.ident);
            ctx.push_symbol_with_doc(extract_const(it, current_module, &path), &it.attrs);
            // Bug C-3: walk const's type annotation (`const X: Foo = …`).
            // Consts have no generics — module scope is the right anchor.
            extract_type::visit_type(
                ctx,
                &it.ty,
                current_module,
                &const_fqdn,
                extract_type::TYPE_CTX_ANNOTATION,
                ModuleLookup::ROOT_SCOPE,
            );
        }
        syn::Item::Static(it) => {
            let path = ctx.core.file_path.clone();
            let static_fqdn = format!("{current_module}::{}", it.ident);
            ctx.push_symbol_with_doc(extract_static(it, current_module, &path), &it.attrs);
            extract_type::visit_type(
                ctx,
                &it.ty,
                current_module,
                &static_fqdn,
                extract_type::TYPE_CTX_ANNOTATION,
                ModuleLookup::ROOT_SCOPE,
            );
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
        flags: vec![],
    }
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
fn extract_struct(ctx: &mut WalkContext, item: &syn::ItemStruct, parent_fqdn: &str) {
    let path = ctx.core.file_path.clone();
    let struct_name = item.ident.to_string();
    let struct_fqdn = format!("{parent_fqdn}::{struct_name}");
    let parent_sym = type_def_symbol(
        struct_name,
        parent_fqdn,
        &path,
        "struct",
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
fn extract_enum(ctx: &mut WalkContext, item: &syn::ItemEnum, parent_fqdn: &str) {
    let path = ctx.core.file_path.clone();
    let enum_name = item.ident.to_string();
    let enum_fqdn = format!("{parent_fqdn}::{enum_name}");
    let parent_sym = type_def_symbol(
        enum_name,
        parent_fqdn,
        &path,
        "enum",
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
    let signature = Signature {
        params: vec![],
        returns: Some(TypeRef::new(ty_str)),
        modifiers: Modifiers::default(),
        meta: SignatureMeta::default(),
    };
    ctx.push_symbol_with_doc(
        RawSymbol {
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
        flags: vec![],
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
                    name: fn_name,
                    fqdn: fn_fqdn.clone(),
                    kind: Kind::Function,
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
            let mut sig = extract_signature(&item_fn.sig);
            sig.modifiers.deprecated = extract_deprecated(&item_fn.attrs);
            ctx.push_symbol_with_doc(
                RawSymbol {
                    name: fn_name,
                    fqdn: fn_fqdn.clone(),
                    kind: Kind::Function,
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
        flags: vec![],
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
        let (symbols, edges, _docs, _) = walk(&parsed, "mycrate", "src/lib.rs", "mycrate");
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
        let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert_eq!(symbols[0].visibility, Visibility::Public);
    }

    #[test]
    fn fn_signature_captures_params_and_return() {
        let parsed = parse("pub fn add(a: u32, b: u32) -> u32 { a + b }");
        let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert!(symbols[0].signature.as_ref().unwrap().modifiers.is_async);
    }

    #[test]
    fn deprecated_attribute_propagates_to_modifier() {
        let parsed = parse("#[deprecated = \"use bar\"] fn foo() {}");
        let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let a = &symbols.iter().find(|s| s.name == "a").unwrap().signature;
        let b = &symbols.iter().find(|s| s.name == "b").unwrap().signature;
        let c = &symbols.iter().find(|s| s.name == "c").unwrap().signature;
        assert_eq!(a.as_ref().unwrap().params[0].ty.display, "Self");
        assert_eq!(b.as_ref().unwrap().params[0].ty.display, "&Self");
        assert_eq!(c.as_ref().unwrap().params[0].ty.display, "&mut Self");
    }

    #[test]
    fn struct_emits_type_symbol_and_field_sub_symbols() {
        // Bug C-2: a struct now pushes the parent type symbol AND one
        // Value-kind sub-symbol per named field.
        let parsed = parse("pub struct Foo { x: u32 }");
        let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let foo = symbols.iter().find(|s| s.fqdn == "c::Foo").unwrap();
        assert_eq!(foo.kind, Kind::Type);
        assert_eq!(foo.language_kind.as_str(), "struct");
        let field = symbols.iter().find(|s| s.fqdn == "c::Foo::x").unwrap();
        assert_eq!(field.kind, Kind::Value);
        assert_eq!(field.language_kind.as_str(), "field");
        assert_eq!(field.module.as_deref(), Some("c::Foo"));
        // Type captured on signature.returns as a TypeRef.
        assert_eq!(
            field
                .signature
                .as_ref()
                .unwrap()
                .returns
                .as_ref()
                .unwrap()
                .display,
            "u32",
        );
    }

    #[test]
    fn tuple_struct_emits_positional_field_sub_symbols() {
        let parsed = parse("pub struct Pair(pub u32, pub String);");
        let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let f0 = symbols.iter().find(|s| s.fqdn == "c::Pair::0").unwrap();
        let f1 = symbols.iter().find(|s| s.fqdn == "c::Pair::1").unwrap();
        assert_eq!(f0.language_kind.as_str(), "tuple_field");
        assert_eq!(f1.language_kind.as_str(), "tuple_field");
        assert_eq!(
            f0.signature
                .as_ref()
                .unwrap()
                .returns
                .as_ref()
                .unwrap()
                .display,
            "u32",
        );
        assert_eq!(
            f1.signature
                .as_ref()
                .unwrap()
                .returns
                .as_ref()
                .unwrap()
                .display,
            "String",
        );
    }

    #[test]
    fn enum_emits_type_symbol_and_variant_sub_symbols() {
        // Bug C-2: enum pushes the parent type symbol AND one Type-kind
        // sub-symbol per variant.
        let parsed = parse("enum E { A, B }");
        let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let e = symbols.iter().find(|s| s.fqdn == "c::E").unwrap();
        assert_eq!(e.kind, Kind::Type);
        assert_eq!(e.language_kind.as_str(), "enum");
        let a = symbols.iter().find(|s| s.fqdn == "c::E::A").unwrap();
        let b = symbols.iter().find(|s| s.fqdn == "c::E::B").unwrap();
        assert_eq!(a.language_kind.as_str(), "enum_variant");
        assert_eq!(a.module.as_deref(), Some("c::E"));
        assert_eq!(b.language_kind.as_str(), "enum_variant");
    }

    #[test]
    fn unit_struct_emits_only_parent_symbol() {
        let parsed = parse("pub struct Marker;");
        let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let marker = symbols.iter().find(|s| s.fqdn == "c::Marker").unwrap();
        assert_eq!(marker.language_kind.as_str(), "struct");
        // No sub-fields for a unit struct.
        let children: Vec<_> = symbols
            .iter()
            .filter(|s| s.module.as_deref() == Some("c::Marker"))
            .collect();
        assert!(
            children.is_empty(),
            "expected no sub-symbols for unit struct, got {children:?}",
        );
    }

    #[test]
    fn trait_emits_type_and_inner_fn_symbols() {
        let parsed = parse("pub trait T { fn foo(&self); fn bar(&self) -> u32 { 0 } }");
        let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (symbols, edges, _docs, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (symbols, edges, _docs, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (symbols, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");

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
        let (symbols, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert!(symbols.is_empty(), "tuple self-type must emit no symbol");
        assert!(edges.is_empty(), "tuple self-type must emit no edge");
    }

    #[test]
    fn trait_impl_with_use_alias_resolves_implements_target() {
        let parsed =
            parse("use crate::traits::Foo; struct Bar; impl Foo for Bar { fn run(&self) {} }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert_eq!(symbols[0].kind, Kind::Value);
        assert_eq!(symbols[0].language_kind.as_str(), "const");
        assert_eq!(symbols[0].fqdn, "c::N");
    }

    #[test]
    fn static_emits_value_symbol() {
        let parsed = parse("static GLOBAL: u32 = 0;");
        let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert_eq!(symbols[0].kind, Kind::Value);
        assert_eq!(symbols[0].language_kind.as_str(), "static");
    }

    #[test]
    fn type_alias_emits_type_symbol() {
        let parsed = parse("pub type Bytes = Vec<u8>;");
        let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert_eq!(symbols[0].kind, Kind::Type);
        assert_eq!(symbols[0].language_kind.as_str(), "type_alias");
    }

    #[test]
    fn macro_rules_with_export_is_public() {
        let parsed = parse("#[macro_export] macro_rules! say { () => {}; }");
        let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].kind, Kind::Macro);
        assert_eq!(symbols[0].visibility, Visibility::Public);
    }

    #[test]
    fn macro_rules_without_export_is_private() {
        let parsed = parse("macro_rules! say { () => {}; }");
        let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert_eq!(symbols[0].visibility, Visibility::Private);
    }

    #[test]
    fn inline_mod_pushes_fqdn_without_emitting_module_symbol() {
        let parsed = parse("mod inner { pub fn deep() {} }");
        let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert_eq!(symbols[0].attributes.len(), 1);
        assert_eq!(symbols[0].attributes[0].name, "derive");
        assert_eq!(symbols[0].attributes[0].args.len(), 1);
        assert_eq!(symbols[0].attributes[0].args[0].value, "Debug, Clone");
    }

    #[test]
    fn generic_params_captured_as_strings() {
        let parsed = parse("fn id<T>(x: T) -> T { x }");
        let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
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

    // --- Stage 3e-2 foundation: resolve_name tests ---

    #[test]
    fn stage3e2_resolve_name_empty_path_returns_drop() {
        let ctx = WalkContext::new("src/lib.rs", "c", "c".to_string());
        assert!(matches!(
            ctx.resolve_name("", ModuleLookup::ROOT_SCOPE, "c"),
            NameResolution::Drop
        ));
    }

    #[test]
    fn stage3e2_resolve_name_module_local_resolved_when_defined() {
        let mut ctx = WalkContext::new("src/lib.rs", "c", "c".to_string());
        ctx.core.defined_fqdns.insert("c::bar".to_string());
        match ctx.resolve_name("bar", ModuleLookup::ROOT_SCOPE, "c") {
            NameResolution::Target {
                to: ResolvedOrUnresolved::Resolved { fqdn },
                alias_mut: None,
                via_builtin: None,
            } => assert_eq!(fqdn, "c::bar"),
            other => panic!("expected Target Resolved, got {other:?}"),
        }
    }

    #[test]
    fn stage3e2_resolve_name_falls_back_to_unresolved_module_local() {
        let ctx = WalkContext::new("src/lib.rs", "c", "c".to_string());
        match ctx.resolve_name("missing", ModuleLookup::ROOT_SCOPE, "c") {
            NameResolution::Target {
                to: ResolvedOrUnresolved::Unresolved { name },
                ..
            } => assert_eq!(name, "c::missing"),
            other => panic!("expected Target Unresolved, got {other:?}"),
        }
    }

    #[test]
    fn stage3e2_resolve_name_builtin_drop_tier_returns_drop() {
        // `Vec` is Drop-tier on Rust per Stage 3e-1 (structural noise).
        let ctx = WalkContext::new("src/lib.rs", "c", "c".to_string());
        assert!(matches!(
            ctx.resolve_name("Vec", ModuleLookup::ROOT_SCOPE, "c"),
            NameResolution::Drop
        ));
        // Multi-segment with Drop-tier leftmost also drops.
        assert!(matches!(
            ctx.resolve_name("Vec::new", ModuleLookup::ROOT_SCOPE, "c"),
            NameResolution::Drop
        ));
    }

    #[test]
    fn stage3e2_resolve_name_builtin_attribute_tier_returns_attribute() {
        // `Iterator` is Attribute-tier on Rust per Stage 3e-1b (`iter` flag).
        let ctx = WalkContext::new("src/lib.rs", "c", "c".to_string());
        match ctx.resolve_name("Iterator", ModuleLookup::ROOT_SCOPE, "c") {
            NameResolution::Attribute(tag) => {
                assert_eq!(tag.slug(), "iter");
            }
            other => panic!("expected Attribute(iter), got {other:?}"),
        }
    }

    #[test]
    fn stage3e2_resolve_name_via_alias_table_resolves_to_canonical() {
        let mut ctx = WalkContext::new("src/lib.rs", "c", "c".to_string());
        ctx.add_alias("HM".into(), "std::collections::HashMap".into());
        match ctx.resolve_name("HM", ModuleLookup::ROOT_SCOPE, "c") {
            NameResolution::Target {
                to: ResolvedOrUnresolved::Unresolved { name },
                alias_mut: None,
                via_builtin: None,
            } => assert_eq!(name, "std::collections::HashMap"),
            other => panic!("expected Target Unresolved canonical, got {other:?}"),
        }
    }

    // --- Bug C-3 tests: Rust UsesType emission ---

    fn uses_type_edges(edges: &[RawEdge]) -> Vec<&RawEdge> {
        edges
            .iter()
            .filter(|e| e.kind == EdgeKind::UsesType)
            .collect()
    }

    fn uses_type_with<'a>(edges: &'a [RawEdge], attrs: &[&str]) -> Vec<&'a RawEdge> {
        edges
            .iter()
            .filter(|e| {
                e.kind == EdgeKind::UsesType
                    && attrs.iter().all(|a| e.attributes.iter().any(|x| x == a))
            })
            .collect()
    }

    fn resolved_targets(edges: &[&RawEdge]) -> Vec<String> {
        edges
            .iter()
            .filter_map(|e| match &e.to {
                ResolvedOrUnresolved::Resolved { fqdn } => Some(fqdn.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn bug_c3_fn_param_type_emits_uses_type() {
        let parsed = parse("pub struct Foo; pub fn process(x: Foo) {}");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let refs = uses_type_with(&edges, &["via-type", "type-annotation"]);
        let targets = resolved_targets(&refs);
        assert!(
            targets.contains(&"c::Foo".to_string()),
            "expected UsesType edge to c::Foo, got {targets:?}",
        );
        assert!(
            refs.iter().any(|e| e.from_fqdn == "c::process"),
            "expected edge from c::process, got {refs:?}",
        );
    }

    #[test]
    fn bug_c3_fn_return_type_emits_uses_type() {
        let parsed = parse("pub struct Bar; pub fn make() -> Bar { Bar }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let refs = uses_type_with(&edges, &["via-type", "type-annotation"]);
        let targets = resolved_targets(&refs);
        assert!(targets.contains(&"c::Bar".to_string()));
    }

    #[test]
    fn bug_c3_struct_field_type_emits_uses_type_from_field_fqdn() {
        let parsed = parse("pub struct Foo; pub struct Bar { pub f: Foo }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let refs = uses_type_with(&edges, &["via-type", "type-annotation"]);
        // Per-field provenance: edge originates from c::Bar::f, not c::Bar.
        let from_field: Vec<&RawEdge> = refs
            .iter()
            .copied()
            .filter(|e| e.from_fqdn == "c::Bar::f")
            .collect();
        assert!(
            !from_field.is_empty(),
            "expected UsesType edge from c::Bar::f, got {refs:?}",
        );
        let targets = resolved_targets(&from_field);
        assert!(targets.contains(&"c::Foo".to_string()));
    }

    #[test]
    fn bug_c3_generic_type_param_does_not_leak() {
        let parsed = parse("pub fn id<T>(x: T) -> T { x }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let refs = uses_type_edges(&edges);
        // `T` is fn-level generic → bound as local → no UsesType edge to c::T.
        let leaked: Vec<_> = refs
            .iter()
            .filter(|e| match &e.to {
                ResolvedOrUnresolved::Resolved { fqdn } => fqdn == "c::T",
                ResolvedOrUnresolved::Unresolved { name } => name == "c::T",
                _ => false,
            })
            .collect();
        assert!(
            leaked.is_empty(),
            "generic param T leaked as UsesType edge: {leaked:?}",
        );
    }

    #[test]
    fn bug_c3_struct_generic_param_does_not_leak_in_fields() {
        let parsed = parse("pub struct Box2<T> { pub inner: T }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let refs = uses_type_edges(&edges);
        let leaked: Vec<_> = refs
            .iter()
            .filter(|e| match &e.to {
                ResolvedOrUnresolved::Resolved { fqdn } => fqdn == "c::T",
                ResolvedOrUnresolved::Unresolved { name } => name == "c::T",
                _ => false,
            })
            .collect();
        assert!(leaked.is_empty(), "struct-level T leaked: {leaked:?}",);
    }

    #[test]
    fn bug_c3_generic_constraint_emits_type_constraint() {
        let parsed = parse("pub trait Foo {} pub fn process<T: Foo>(x: T) -> T { x }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let refs = uses_type_with(&edges, &["via-type", "type-constraint"]);
        let targets = resolved_targets(&refs);
        assert!(
            targets.contains(&"c::Foo".to_string()),
            "expected UsesType/type-constraint edge to c::Foo, got {targets:?}",
        );
    }

    #[test]
    fn bug_c3_where_clause_emits_type_constraint() {
        let parsed = parse("pub trait Foo {} pub fn process<T>(x: T) where T: Foo { let _ = x; }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let refs = uses_type_with(&edges, &["via-type", "type-constraint"]);
        let targets = resolved_targets(&refs);
        assert!(
            targets.contains(&"c::Foo".to_string()),
            "expected via-type/type-constraint via where-clause to c::Foo, got {targets:?}",
        );
    }

    #[test]
    fn bug_c3_type_alias_body_emits_uses_type() {
        let parsed = parse("pub struct Foo; pub type X = Foo;");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let refs = uses_type_with(&edges, &["via-type", "type-alias-body"]);
        let targets = resolved_targets(&refs);
        assert!(
            targets.contains(&"c::Foo".to_string()),
            "expected UsesType/type-alias-body to c::Foo, got {targets:?}",
        );
        assert!(refs.iter().all(|e| e.from_fqdn == "c::X"));
    }

    #[test]
    fn bug_c3_const_static_type_emits_uses_type() {
        let parsed = parse("pub struct Cfg; pub const K: Cfg = Cfg; pub static M: Cfg = Cfg;");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let refs = uses_type_with(&edges, &["via-type", "type-annotation"]);
        let const_edges: Vec<_> = refs.iter().filter(|e| e.from_fqdn == "c::K").collect();
        let static_edges: Vec<_> = refs.iter().filter(|e| e.from_fqdn == "c::M").collect();
        assert!(!const_edges.is_empty(), "expected const K → Cfg edge");
        assert!(!static_edges.is_empty(), "expected static M → Cfg edge");
    }

    #[test]
    fn bug_c3_trait_supertrait_emits_type_extends() {
        let parsed = parse("pub trait Foo {} pub trait Bar: Foo {}");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let refs = uses_type_with(&edges, &["via-type", "type-extends"]);
        let targets = resolved_targets(&refs);
        assert!(
            targets.contains(&"c::Foo".to_string()),
            "expected UsesType/type-extends from Bar to Foo, got {targets:?}",
        );
    }

    #[test]
    fn bug_c3_impl_trait_generic_arg_emits_type_implements() {
        let parsed = parse(
            "pub struct Foo; pub trait Iface<T> {} pub struct C; \
             impl Iface<Foo> for C {}",
        );
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let refs = uses_type_with(&edges, &["via-type", "type-implements"]);
        let targets = resolved_targets(&refs);
        assert!(
            targets.contains(&"c::Foo".to_string()),
            "expected UsesType/type-implements arg to c::Foo, got {targets:?}",
        );
    }

    #[test]
    fn stage3e1_drop_tier_wrapper_skipped_inner_arg_still_emits() {
        // `Vec<Foo>` — Stage 3e-1: `Vec` is now classified as
        // `BuiltinTier::Drop` (structural noise) and produces no
        // `UsesType` edge. The inner `Foo` still emits normally — the
        // recursion through `visit_type_path` happens regardless of the
        // wrapper's tier decision.
        let parsed = parse("pub struct Foo; pub fn collect() -> Vec<Foo> { vec![] }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let refs = uses_type_with(&edges, &["via-type", "type-annotation"]);
        let targets = resolved_targets(&refs);
        assert!(
            !targets.iter().any(|t| t == "<builtin>::rust::Vec"),
            "Drop-tier Vec must not surface, got {targets:?}",
        );
        assert!(
            targets.contains(&"c::Foo".to_string()),
            "inner Foo must still emit, got {targets:?}",
        );
    }

    #[test]
    fn stage3e1_uses_type_edge_tier_builtin_emits_with_attrs() {
        // `Error` is the lone `BuiltinTier::Edge` entry in the Rust
        // registry. A trait bound `T: Error` should produce a UsesType
        // edge to `<builtin>::rust::Error` carrying `via-builtin` plus
        // the `builtin-<tag>` slug — parity with TS Edge-tier emission.
        let parsed = parse("pub fn boom<T: Error>(e: T) {}");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let refs = uses_type_with(&edges, &["via-type", "via-builtin"]);
        let targets = resolved_targets(&refs);
        assert!(
            targets.iter().any(|t| t == "<builtin>::rust::Error"),
            "expected Edge-tier Error synthetic, got {targets:?}",
        );
        let has_tag_attr = refs.iter().any(|e| {
            e.attributes
                .iter()
                .any(|a| a.starts_with("builtin-") && a != "builtin-")
        });
        assert!(has_tag_attr, "expected builtin-<slug> attr on edge");
    }

    #[test]
    fn stage3e1b_uses_type_attribute_tier_promotes_flag_on_source_symbol() {
        // `Iterator` is `BuiltinTier::Attribute` (`Iter` tag). Stage
        // 3e-1b flushes that into `flags = ["iter"]` on the enclosing
        // fn ; no edge surfaces (the property is a fact about the fn,
        // not a graph neighbor worth a node).
        let parsed = parse("pub fn collect<T: Iterator>(it: T) {}");
        let (symbols, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let refs = uses_type_with(&edges, &["via-type"]);
        let targets = resolved_targets(&refs);
        assert!(
            !targets.iter().any(|t| t.ends_with("::Iterator")),
            "Attribute-tier Iterator must not surface as an edge, got {targets:?}",
        );
        let collect_sym = symbols
            .iter()
            .find(|s| s.fqdn == "c::collect")
            .expect("c::collect must be indexed");
        assert!(
            collect_sym.flags.contains(&"iter".to_string()),
            "expected `iter` flag on c::collect, got {:?}",
            collect_sym.flags
        );
    }

    #[test]
    fn stage3e1b_future_bound_promotes_async_flag() {
        // `Future` is `BuiltinTier::Attribute` (`Async` tag) — same
        // mechanism as Iterator but flagged as `"async"`.
        let parsed = parse("pub fn run<F: Future>(fut: F) {}");
        let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let run_sym = symbols
            .iter()
            .find(|s| s.fqdn == "c::run")
            .expect("c::run must be indexed");
        assert!(
            run_sym.flags.contains(&"async".to_string()),
            "expected `async` flag on c::run, got {:?}",
            run_sym.flags
        );
    }

    #[test]
    fn stage3e1b_attribute_flag_dedupes_across_multiple_hits() {
        // Same Attribute-tier trait touched twice in one fn signature
        // (param bound + return bound) must produce the flag exactly
        // once — `HashSet` dedup happens at the register-time site.
        let parsed = parse("pub fn pipe<I: Iterator>(i: I) -> impl Iterator { i }");
        let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let pipe_sym = symbols
            .iter()
            .find(|s| s.fqdn == "c::pipe")
            .expect("c::pipe must be indexed");
        let iter_count = pipe_sym.flags.iter().filter(|f| *f == "iter").count();
        assert_eq!(
            iter_count, 1,
            "iter flag must dedup, got flags = {:?}",
            pipe_sym.flags
        );
    }

    #[test]
    fn stage3e1_uses_type_primitive_skipped_via_registry() {
        // `u32` / `String` / `bool` are registered as `BuiltinTier::Drop`
        // primitives — no UsesType edge from a parameter / return slot.
        // Validates that the registry is the single source of truth now
        // (previously the deleted `RUST_BUILTIN_TYPES` const lived here).
        let parsed = parse("pub fn add(a: u32, b: u32, name: String) -> bool { true }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let refs = uses_type_with(&edges, &["via-type"]);
        let targets = resolved_targets(&refs);
        assert!(
            targets.is_empty(),
            "primitives + String must skip edges, got {targets:?}",
        );
    }

    #[test]
    fn bug_c3_unresolved_type_carries_unresolved_type_attr() {
        let parsed = parse("pub fn x(p: SomeUnknown) {}");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let refs = uses_type_with(&edges, &["via-type", "unresolved-type"]);
        assert!(
            !refs.is_empty(),
            "expected unresolved-type marker on unknown type ref",
        );
        let unresolved_names: Vec<&str> = refs
            .iter()
            .filter_map(|e| match &e.to {
                ResolvedOrUnresolved::Unresolved { name } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            unresolved_names
                .iter()
                .any(|n| n.ends_with("::SomeUnknown")),
            "expected unresolved canonical name, got {unresolved_names:?}",
        );
    }

    #[test]
    fn bug_c3_enum_variant_inner_field_types_emit_from_variant_fqdn() {
        let parsed = parse("pub struct Foo; pub struct Bar; pub enum E { V(Foo, Bar) }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let refs = uses_type_with(&edges, &["via-type", "type-annotation"]);
        let from_variant: Vec<&RawEdge> = refs
            .iter()
            .copied()
            .filter(|e| e.from_fqdn == "c::E::V")
            .collect();
        let targets = resolved_targets(&from_variant);
        assert!(
            targets.contains(&"c::Foo".to_string()) && targets.contains(&"c::Bar".to_string()),
            "expected variant V → Foo, Bar (got {targets:?})",
        );
    }

    // --- Stage 3c: class/struct/trait/impl-level generics propagate to
    // inner method bodies through the lookup's parent-chain walk. The
    // pre-3a-8c `outer_locals` HashSet plumbing handled the simple cases
    // but missed scenarios where an impl/trait-level generic was used
    // inside an inner method's signature without being explicitly
    // re-collected. These tests pin the now-automatic behaviour.

    #[test]
    fn stage3c_impl_method_filters_impl_level_generic() {
        let parsed =
            parse("pub struct S<T>(T); impl<T> S<T> { pub fn m(&self) -> T { unimplemented!() } }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let leaked: Vec<&RawEdge> = uses_type_edges(&edges)
            .into_iter()
            .filter(|e| {
                e.from_fqdn == "c::S::m"
                    && match &e.to {
                        ResolvedOrUnresolved::Resolved { fqdn } => fqdn == "c::T",
                        ResolvedOrUnresolved::Unresolved { name } => name == "c::T",
                        _ => false,
                    }
            })
            .collect();
        assert!(
            leaked.is_empty(),
            "impl-level generic T leaked into S::m's signature: {leaked:?}",
        );
    }

    #[test]
    fn stage3c_trait_method_filters_trait_level_generic() {
        let parsed = parse("pub trait Tr<T> { fn m(&self) -> T; }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let leaked: Vec<&RawEdge> = uses_type_edges(&edges)
            .into_iter()
            .filter(|e| {
                e.from_fqdn == "c::Tr::m"
                    && match &e.to {
                        ResolvedOrUnresolved::Resolved { fqdn } => fqdn == "c::T",
                        ResolvedOrUnresolved::Unresolved { name } => name == "c::T",
                        _ => false,
                    }
            })
            .collect();
        assert!(
            leaked.is_empty(),
            "trait-level generic T leaked into Tr::m: {leaked:?}",
        );
    }

    #[test]
    fn stage3c_impl_method_inner_generic_combined_with_outer_generic() {
        let parsed = parse("pub struct S<T>(T); impl<T> S<T> { pub fn m<U>(_x: T, _y: U) {} }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let m_refs: Vec<&RawEdge> = uses_type_edges(&edges)
            .into_iter()
            .filter(|e| e.from_fqdn == "c::S::m")
            .collect();
        let leaked_names: Vec<String> = m_refs
            .iter()
            .filter_map(|e| match &e.to {
                ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "c::T" || fqdn == "c::U" => {
                    Some(fqdn.clone())
                }
                ResolvedOrUnresolved::Unresolved { name } if name == "c::T" || name == "c::U" => {
                    Some(name.clone())
                }
                _ => None,
            })
            .collect();
        assert!(
            leaked_names.is_empty(),
            "neither outer T nor inner U should leak as a UsesType: got {leaked_names:?}",
        );
    }

    #[test]
    fn stage3c_trait_method_inner_generic_shadows_trait_generic() {
        // Inner `<T>` shadows the trait-level `<T>`. Either way the
        // resolution lands on `BindingSource::TypeParam` so no phantom
        // `c::T` UsesType edge fires.
        let parsed = parse("pub trait Tr<T> { fn m<T>(_x: T); }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let leaked: Vec<&RawEdge> = uses_type_edges(&edges)
            .into_iter()
            .filter(|e| {
                e.from_fqdn == "c::Tr::m"
                    && match &e.to {
                        ResolvedOrUnresolved::Resolved { fqdn } => fqdn == "c::T",
                        ResolvedOrUnresolved::Unresolved { name } => name == "c::T",
                        _ => false,
                    }
            })
            .collect();
        assert!(
            leaked.is_empty(),
            "shadowed T should still be a local TypeParam: {leaked:?}",
        );
    }
}

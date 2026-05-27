use std::collections::{HashMap, HashSet};

use proc_macro2::Span;
use quote::ToTokens;
use standardoc_ir::{
    AliasMutability, BuiltinTag, BuiltinTier, DeclKind, EdgeKind, EntryPointKind, Kind, Language,
    LanguageKind, Modifiers, ModuleLookup, Param, RawAttribute, RawAttributeArg, RawCallSite,
    RawDocument, RawEdge, RawSymbol, ResolvedOrUnresolved, Signature, SignatureMeta, Site,
    SymbolLocation, TypeRef, Visibility, compact_rust_tokens,
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
    /// 3. multi-segment paths with no alias : try each ancestor module from
    ///    `current_module` up to `file_module_fqdn` as the prefix for the
    ///    leftmost segment ; if any composes a defined FQDN, append the
    ///    rest. Without this, `IndexHandle::open()` called inside a test
    ///    submodule of the same file stays unresolved despite
    ///    `IndexHandle` being right there at the file's root scope.
    /// 4. fall back to text-as-written ; the pipeline `promote_unresolved`
    ///    may still match by exact FQDN.
    pub(crate) fn resolve_path(&self, path: &str, current_module: &str) -> ResolvedOrUnresolved {
        // Verbatim FQDN already defined locally (e.g. caller passed an
        // absolute path after `Self::xxx` → `<self_type>::xxx`
        // substitution). Skip canonicalize / ancestor-walk and return
        // the FQDN directly so the edge resolves to the matching symbol.
        if self.core.defined_fqdns.contains(path) {
            return ResolvedOrUnresolved::Resolved {
                fqdn: path.to_string(),
            };
        }
        if let Some(canonical) = self.canonicalize(path, current_module) {
            return if self.core.defined_fqdns.contains(&canonical) {
                ResolvedOrUnresolved::Resolved { fqdn: canonical }
            } else {
                ResolvedOrUnresolved::Unresolved { name: canonical }
            };
        }
        let segments: Vec<&str> = path.split("::").filter(|s| !s.is_empty()).collect();
        if segments.is_empty() {
            return ResolvedOrUnresolved::Unresolved {
                name: path.to_string(),
            };
        }
        // Walk `current_module` up to the file's root looking for a
        // defined `<probe>::<leftmost>`. Catches two related patterns :
        //
        // - Single-ident calls from nested `mod tests { use super::*; }`
        //   blocks to parent-module fns/items (e.g. `walk(...)` calling
        //   `parent::walk` from `parent::tests::test_fn`). The glob
        //   import doesn't bind enumerated parent items into the test
        //   scope, so the module-local fallback `<current_module>::walk`
        //   misses ; walking up finds `parent::walk`.
        //
        // - Multi-segment paths like `IndexHandle::open()` where the
        //   leftmost is a locally-defined type at the file root. Same
        //   ancestor walk, append the remaining segments.
        //
        // External paths (`std::mem::take`, `serde::Deserialize`) still
        // fall through to the text-as-written branch — no ancestor of
        // ours owns `std` or `serde`, so the walk is a no-op for them.
        let leftmost = segments[0];
        let rest = if segments.len() > 1 {
            Some(segments[1..].join("::"))
        } else {
            None
        };
        let mut probe = current_module.to_string();
        loop {
            let candidate = format!("{probe}::{leftmost}");
            if self.core.defined_fqdns.contains(&candidate) {
                let full = match &rest {
                    Some(r) => format!("{candidate}::{r}"),
                    None => candidate,
                };
                return if self.core.defined_fqdns.contains(&full) {
                    ResolvedOrUnresolved::Resolved { fqdn: full }
                } else {
                    ResolvedOrUnresolved::Unresolved { name: full }
                };
            }
            if probe == self.core.file_module_fqdn {
                break;
            }
            match probe.rsplit_once("::") {
                Some((parent, _)) => probe = parent.to_string(),
                None => break,
            }
        }
        // No ancestor owns the leftmost. Single-ident falls back to the
        // module-local unresolved (preserves pre-fix shape for callers
        // doing `let x = unknown;`) ; multi-segment falls back to
        // text-as-written (likely an external crate path).
        if rest.is_none() {
            return ResolvedOrUnresolved::Unresolved {
                name: format!("{current_module}::{leftmost}"),
            };
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

        if segments.len() == 1
            && let Some(res) = self.core.lookup.resolve_local(segments[0], scope_idx)
            && res.scope_idx != ModuleLookup::ROOT_SCOPE
        {
            if let (Some(alias_str), Some(m)) = (res.aliases_to.as_deref(), res.mutability) {
                return self.resolve_module_level(alias_str, current_module, Some(m));
            }
            return NameResolution::Local;
        }
        // ROOT_SCOPE — fall through to module-level resolution.

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

#[cfg(test)]
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
    for sym in &mut ctx.core.symbols {
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
            extract_call::visit_block(ctx, &it.block, current_module, &fn_fqdn, None);
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
                    extract_call::visit_block(
                        ctx,
                        &item_fn.block,
                        current_module,
                        &fn_fqdn,
                        Some(&target_fqdn),
                    );
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
                    // Trait default-method bodies see `Self` as the
                    // implementing type (unknown at extract time), so
                    // we leave self_type = None and let `Self::method`
                    // remain unresolved as before.
                    extract_call::visit_block(ctx, block, current_module, &fn_fqdn, None);
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
fn extract_struct(ctx: &mut WalkContext, item: &syn::ItemStruct, parent_fqdn: &str) {
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
fn extract_enum(ctx: &mut WalkContext, item: &syn::ItemEnum, parent_fqdn: &str) {
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

fn extract_union(item: &syn::ItemUnion, parent_fqdn: &str, path: &str) -> RawSymbol {
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

fn extract_type_alias(item: &syn::ItemType, parent_fqdn: &str, path: &str) -> RawSymbol {
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

fn extract_trait(ctx: &mut WalkContext, item: &syn::ItemTrait, parent_fqdn: &str) {
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

fn extract_const(item: &syn::ItemConst, parent_fqdn: &str, path: &str) -> RawSymbol {
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

fn extract_static(item: &syn::ItemStatic, parent_fqdn: &str, path: &str) -> RawSymbol {
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
mod tests;

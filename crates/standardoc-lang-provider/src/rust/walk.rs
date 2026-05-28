use std::collections::{HashMap, HashSet};

use proc_macro2::Span;
use standardoc_ir::{
    AliasMutability, BuiltinTag, BuiltinTier, Language, ModuleLookup, RawCallSite, RawDocument,
    RawEdge, RawSymbol, ResolvedOrUnresolved, SymbolLocation,
};
use syn::spanned::Spanned;

use crate::builtins::global as global_builtin_registry;
use crate::walk_core::WalkContextCore;

use super::extract_call;
use super::extract_doc;
use super::extract_type;
use super::extract_use;
use super::lookup as rust_lookup;

mod extract_items;

use extract_items::{
    extract_const, extract_enum, extract_fn, extract_impl, extract_macro_def, extract_static,
    extract_struct, extract_trait, extract_type_alias, extract_union,
};

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
    /// Bug E-3 Phase 1: per-file `struct_fqdn → {field → nominal type}`
    /// table populated by `push_field` during struct extraction. Read
    /// by `visit_expr_method_call` (P1.4) to resolve `self.field.method`
    /// when the enclosing impl's `self_type` is known.
    pub(crate) struct_fields: super::struct_field_table::StructFieldTable,
}

impl WalkContext {
    pub(crate) fn new(file_path: &str, crate_name: &str, file_module_fqdn: String) -> Self {
        Self {
            core: WalkContextCore::new(file_path.to_string(), file_module_fqdn, Language::Rust),
            crate_name: crate_name.to_string(),
            alias_table: HashMap::new(),
            attribute_flags: HashMap::new(),
            struct_fields: super::struct_field_table::StructFieldTable::default(),
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
            extract_call::visit_block(
                ctx,
                &it.block,
                current_module,
                &fn_fqdn,
                None,
                &it.sig.inputs,
            );
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
                        &item_fn.sig.inputs,
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
                    extract_call::visit_block(
                        ctx,
                        block,
                        current_module,
                        &fn_fqdn,
                        None,
                        &item_fn.sig.inputs,
                    );
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

// extract_fn / extract_struct / extract_enum / extract_union / extract_trait /
// extract_impl / extract_type_alias / extract_const / extract_static /
// extract_macro_def + helpers (push_field, push_struct_fields, type_def_symbol,
// value_def_symbol, extract_signature, extract_param, extract_attributes,
// meta_to_args, extract_deprecated, render_compact, classify_fn_entry_point)
// moved to `walk/extract_items.rs` and re-imported above.

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

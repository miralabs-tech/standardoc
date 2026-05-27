//! Bug C-3 — Rust counterpart of TS Stage 2b. Walks a `syn::Type` /
//! `syn::TypeParamBound` / `syn::Signature` from a known enclosing
//! symbol FQDN and emits `UsesType` edges for every resolvable named
//! type reference inside. The attribute taxonomy mirrors TS Stage 2b:
//!
//! - root tag `via-type`
//! - emission-context sub-tag (`type-annotation`, `type-constraint`,
//!   `type-alias-body`, `type-extends`, `type-implements`)
//! - `unresolved-type` marker when the resolution lands on
//!   `Unresolved{,Bridge}` (filterable consumer-side, no
//!   re-extraction needed when the viz toggles "show unknown types").
//!
//! The hook sites (in `walk.rs`) cover: fn signatures (params/return/
//! generic constraints/where clauses), struct/enum field types,
//! const/static types, type alias RHS, trait/impl block contents,
//! union fields.

use proc_macro2::Span;
use standardoc_ir::{
    BindingSource, BuiltinTier, EdgeKind, Language, RawEdge, ResolvedOrUnresolved, Site,
};
use syn::spanned::Spanned;
use syn::visit::Visit;

use super::walk::{WalkContext, col_from_span, line_from_span, path_to_string};
use crate::builtins::global as global_builtin_registry;

pub(crate) const TYPE_CTX_ANNOTATION: &str = "type-annotation";
pub(crate) const TYPE_CTX_CONSTRAINT: &str = "type-constraint";
pub(crate) const TYPE_CTX_ALIAS_BODY: &str = "type-alias-body";
pub(crate) const TYPE_CTX_EXTENDS: &str = "type-extends";
pub(crate) const TYPE_CTX_IMPLEMENTS: &str = "type-implements";
const TYPE_TAG_UNRESOLVED: &str = "unresolved-type";

/// Reserved markers that are syntactic placeholders, not real symbols —
/// `Self` refers back to the enclosing impl block, `self` is the receiver
/// keyword, `_` is the inferred-type placeholder. None should ever emit
/// a UsesType edge.
const SKIP_MARKERS: &[&str] = &["Self", "self", "_"];

/// Walk a single `syn::Type` and emit `UsesType` edges. `locals` is
/// the set of generic type-param names introduced in the enclosing
/// scope — names matching are skipped (avoids `<T>` leaking as a
/// phantom `<module>::T` UsesType edge). Hook this from any
/// declaration-site type position (struct field, const, var, alias).
pub(crate) fn visit_type(
    ctx: &mut WalkContext,
    ty: &syn::Type,
    current_module: &str,
    enclosing_fqdn: &str,
    emission_context: &'static str,
    scope_idx: u32,
) {
    let mut visitor = TypeRefVisitor {
        ctx,
        current_module: current_module.to_string(),
        enclosing_fqdn: enclosing_fqdn.to_string(),
        emission_context,
        scope_idx,
    };
    visitor.visit_type(ty);
}

/// Walk a single `syn::TypeParamBound` (the `Foo` in `<T: Foo>` or
/// `dyn Foo + Bar`) and emit a `UsesType` edge. The bound's trait path
/// is NOT a `Type::Path` in syn, so the generic `visit_type` doesn't
/// reach it — this entry point closes that gap.
pub(crate) fn visit_type_param_bound(
    ctx: &mut WalkContext,
    bound: &syn::TypeParamBound,
    current_module: &str,
    enclosing_fqdn: &str,
    emission_context: &'static str,
    scope_idx: u32,
) {
    let mut visitor = TypeRefVisitor {
        ctx,
        current_module: current_module.to_string(),
        enclosing_fqdn: enclosing_fqdn.to_string(),
        emission_context,
        scope_idx,
    };
    visitor.visit_type_param_bound(bound);
}

/// Convenience: walk a full `syn::Signature` and emit `UsesType` edges
/// for every type position inside (param types, return type, generic
/// param bounds, where-clause predicates). The fn's own generics and
/// any outer (impl/trait) generics are reachable through `scope_idx`'s
/// parent chain in the lookup — no separate `outer_locals` plumbing.
pub(crate) fn visit_signature(
    ctx: &mut WalkContext,
    sig: &syn::Signature,
    current_module: &str,
    enclosing_fqdn: &str,
    scope_idx: u32,
) {
    for input in &sig.inputs {
        if let syn::FnArg::Typed(pat) = input {
            visit_type(
                ctx,
                &pat.ty,
                current_module,
                enclosing_fqdn,
                TYPE_CTX_ANNOTATION,
                scope_idx,
            );
        }
    }
    if let syn::ReturnType::Type(_, ty) = &sig.output {
        visit_type(
            ctx,
            ty,
            current_module,
            enclosing_fqdn,
            TYPE_CTX_ANNOTATION,
            scope_idx,
        );
    }
    visit_generics(
        ctx,
        &sig.generics,
        current_module,
        enclosing_fqdn,
        scope_idx,
    );
}

/// Walk a `syn::Generics`' bounds + where-clause predicates and emit
/// `UsesType` edges under `type-constraint`. The generic param decls
/// themselves resolve to `BindingSource::TypeParam` in the lookup so
/// `T` in `<T: Foo>` is filtered out automatically; only `Foo` emits.
pub(crate) fn visit_generics(
    ctx: &mut WalkContext,
    generics: &syn::Generics,
    current_module: &str,
    enclosing_fqdn: &str,
    scope_idx: u32,
) {
    for param in &generics.params {
        if let syn::GenericParam::Type(tp) = param {
            for bound in &tp.bounds {
                visit_type_param_bound(
                    ctx,
                    bound,
                    current_module,
                    enclosing_fqdn,
                    TYPE_CTX_CONSTRAINT,
                    scope_idx,
                );
            }
        }
    }
    if let Some(wc) = &generics.where_clause {
        for pred in &wc.predicates {
            if let syn::WherePredicate::Type(pt) = pred {
                visit_type(
                    ctx,
                    &pt.bounded_ty,
                    current_module,
                    enclosing_fqdn,
                    TYPE_CTX_CONSTRAINT,
                    scope_idx,
                );
                for bound in &pt.bounds {
                    visit_type_param_bound(
                        ctx,
                        bound,
                        current_module,
                        enclosing_fqdn,
                        TYPE_CTX_CONSTRAINT,
                        scope_idx,
                    );
                }
            }
        }
    }
}

struct TypeRefVisitor<'a> {
    ctx: &'a mut WalkContext,
    current_module: String,
    enclosing_fqdn: String,
    emission_context: &'static str,
    /// Stage 3a-8c — scope_idx into `ctx.core.lookup.scopes` where this
    /// visitor was launched. `resolve_local` walks the parent chain
    /// from here, so generic params bound at the enclosing
    /// fn/impl/trait/struct/enum scope are reachable without separate
    /// `&HashSet<String>` plumbing.
    scope_idx: u32,
}

impl<'ast> Visit<'ast> for TypeRefVisitor<'_> {
    fn visit_type_path(&mut self, node: &'ast syn::TypePath) {
        let path_str = path_to_string(&node.path);
        emit_uses_type_path(
            self.ctx,
            &path_str,
            &self.current_module,
            &self.enclosing_fqdn,
            self.emission_context,
            self.scope_idx,
            node.span(),
        );
        // Recurse into generic args so `Vec<Foo>` emits on Foo even
        // when `Vec` is filtered as a builtin.
        syn::visit::visit_type_path(self, node);
    }

    fn visit_trait_bound(&mut self, node: &'ast syn::TraitBound) {
        let path_str = path_to_string(&node.path);
        emit_uses_type_path(
            self.ctx,
            &path_str,
            &self.current_module,
            &self.enclosing_fqdn,
            self.emission_context,
            self.scope_idx,
            node.span(),
        );
        syn::visit::visit_trait_bound(self, node);
    }
}

fn emit_uses_type_path(
    ctx: &mut WalkContext,
    path_str: &str,
    current_module: &str,
    enclosing_fqdn: &str,
    emission_context: &'static str,
    scope_idx: u32,
    span: Span,
) {
    let leftmost = path_str.split("::").next().unwrap_or("");
    if leftmost.is_empty() {
        return;
    }
    if SKIP_MARKERS.contains(&leftmost) {
        return;
    }
    // Stage 3a-8c — lookup-based local check. Generic params
    // (TypeParam) bound at `scope_idx` OR any ancestor scope filter
    // the emission; the parent-chain walk handles impl-level /
    // trait-level outer generics that the old `&HashSet<String>`
    // approach plumbed manually.
    if ctx
        .core
        .lookup
        .resolve_local(leftmost, scope_idx)
        .is_some_and(|r| matches!(r.source, BindingSource::TypeParam))
    {
        return;
    }
    if let Some(entry) = global_builtin_registry().lookup(leftmost, Language::Rust) {
        match entry.tier {
            // Stage 3e-1: `Drop` = structural noise (`Vec<T>`, `Box<T>`,
            // `Option<T>`, marker traits, …) — silently skipped, inner
            // type args still recurse via `visit_type_path` /
            // `visit_trait_bound`.
            BuiltinTier::Drop => return,
            // Stage 3e-1b: `Attribute` = source-flag promotion target.
            // `Iterator` / `IntoIterator` / `FromIterator` stamp `iter`
            // on the source symbol ; `Future` / `Stream` stamp `async`.
            // No edge — the property surfaces as `symbol.flags` instead.
            BuiltinTier::Attribute => {
                ctx.register_attribute_flag(enclosing_fqdn, &entry.tag);
                return;
            }
            BuiltinTier::Edge => {
                let to = ResolvedOrUnresolved::Resolved {
                    fqdn: entry.synthetic_fqdn.clone(),
                };
                let confidence = to.default_confidence();
                let file_path = ctx.core.file_path.clone();
                let attributes = vec![
                    "via-type".to_string(),
                    emission_context.to_string(),
                    "via-builtin".to_string(),
                    format!("builtin-{}", entry.tag.slug()),
                ];
                ctx.push_edge(RawEdge {
                    from_fqdn: enclosing_fqdn.to_string(),
                    kind: EdgeKind::UsesType,
                    to,
                    sites: vec![Site {
                        file: file_path,
                        line: line_from_span(span),
                        col: col_from_span(span),
                    }],
                    attributes,
                    confidence,
                    receiver_type: None,
                });
                return;
            }
        }
    }
    let to = ctx.resolve_path(path_str, current_module);
    if let ResolvedOrUnresolved::Resolved { fqdn } = &to
        && fqdn == enclosing_fqdn
    {
        return;
    }
    let is_unresolved = matches!(
        &to,
        ResolvedOrUnresolved::Unresolved { .. } | ResolvedOrUnresolved::UnresolvedBridge { .. }
    );
    let confidence = to.default_confidence();
    let mut attributes = vec!["via-type".to_string(), emission_context.to_string()];
    if is_unresolved {
        attributes.push(TYPE_TAG_UNRESOLVED.to_string());
    }
    let file_path = ctx.core.file_path.clone();
    ctx.push_edge(RawEdge {
        from_fqdn: enclosing_fqdn.to_string(),
        kind: EdgeKind::UsesType,
        to,
        sites: vec![Site {
            file: file_path,
            line: line_from_span(span),
            col: col_from_span(span),
        }],
        attributes,
        confidence,
        receiver_type: None,
    });
}

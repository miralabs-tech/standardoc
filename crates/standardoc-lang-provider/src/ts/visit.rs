use standardoc_ir::{
    AliasMutability, BuiltinTag, EdgeKind, ModuleLookup, RawCallArg, RawCallSite, RawEdge,
    ResolvedOrUnresolved,
};
use swc_core::common::{Span, Spanned};
use swc_core::ecma::ast::{
    ArrowExpr, BlockStmt, BlockStmtOrExpr, CallExpr, Callee, CatchClause, Expr, ExprOrSpread,
    ForInStmt, ForOfStmt, ForStmt, Function, Ident, JSXAttrOrSpread, JSXAttrValue, JSXElement,
    JSXElementChild, JSXElementName, JSXExpr, Lit, MemberProp, NewExpr, OptChainBase, OptChainExpr,
    Pat, TsAsExpr, TsEntityName, TsTypeAnn, TsTypeAssertion, TsTypeParamInstantiation, TsTypeRef,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::walk::{CallTarget, ResolutionOutcome, TsWalkContext};
use crate::template::JS_GLOBALS;

/// Bridge between the walker's [`ResolutionOutcome`] (tier-aware) and
/// the visitor's [`NameResolution`] (also-tier-aware-AND-scope-aware).
/// Drop / Attribute pass through unchanged ; Emit becomes a `Target`
/// stamped with the optional `AliasMutability` the caller derived from
/// the scope walk.
fn outcome_to_resolution(
    outcome: ResolutionOutcome,
    alias_mut: Option<AliasMutability>,
) -> NameResolution {
    match outcome {
        ResolutionOutcome::Drop => NameResolution::Drop,
        ResolutionOutcome::Attribute(tag) => NameResolution::Attribute(tag),
        ResolutionOutcome::Emit(target) => NameResolution::Target(target, alias_mut),
    }
}

/// Append `via-builtin` + `builtin-<slug>` attrs when the target is a
/// synthetic Edge-tier builtin. No-op when `via_builtin` is `None`. Pair
/// with every TS emit site that consumes a [`CallTarget`] so Calls /
/// References / UsesType edges expose the same builtin provenance as the
/// Rust UsesType emitter (`extract_type.rs`).
fn append_via_builtin(attrs: &mut Vec<String>, via_builtin: Option<&BuiltinTag>) {
    if let Some(tag) = via_builtin {
        attrs.push("via-builtin".to_string());
        attrs.push(format!("builtin-{}", tag.slug()));
    }
}

// --- Bug B Stage 2b: type-emission context sub-tags ---
//
// Attached as a SECOND attribute on every emitted `UsesType` edge (the
// FIRST is always `via-type`). Lets the viz / queries filter by the
// *kind* of type reference: a return-type annotation vs a generic
// instantiation vs a `keyof T` operator, etc. A third `unresolved-type`
// attribute is appended when the target is `Unresolved{,Bridge}`.

/// Param / return / var / class-prop / interface-member type annotation.
pub(crate) const TYPE_CTX_ANNOTATION: &str = "type-annotation";
/// `as Foo` / `<Foo>x` cast.
pub(crate) const TYPE_CTX_CAST: &str = "type-cast";
/// Generic instantiation in a `CallExpr` / `NewExpr` (`foo<Bar>()`).
pub(crate) const TYPE_CTX_INSTANTIATION: &str = "type-instantiation";
/// Generic constraint clause (`<T extends Foo>`).
pub(crate) const TYPE_CTX_CONSTRAINT: &str = "type-constraint";
/// Body of a `type X = …` alias.
pub(crate) const TYPE_CTX_ALIAS_BODY: &str = "type-alias-body";
/// Class `extends` super-class type arguments (`class X extends Foo<Bar>`).
pub(crate) const TYPE_CTX_EXTENDS: &str = "type-extends";
/// Class `implements` clause type arguments (or interface `extends`).
pub(crate) const TYPE_CTX_IMPLEMENTS: &str = "type-implements";
/// Class property type annotation (`class X { field: Foo }`).
pub(crate) const TYPE_CTX_CLASS_PROP: &str = "type-class-prop";
/// Interface member type annotation (`interface I { x: Foo; m(): Bar }`).
pub(crate) const TYPE_CTX_INTERFACE_MEMBER: &str = "type-interface-member";

/// Marker attribute added to `UsesType` edges whose `to` is `Unresolved`
/// (i.e. the type couldn't be tied to a known symbol — either a generic
/// `<T>`, an ambient builtin not in our filter list, or a cross-package
/// type pending Stage 3's AOT lookup). Lets consumers (viz, queries)
/// hide these by default and toggle them on for debugging.
const TYPE_TAG_UNRESOLVED: &str = "unresolved-type";

// --- Stage 2b walk-site entry points ---
//
// Used by `walk.rs` to emit `UsesType` edges from declaration-level
// type positions (type alias body, interface members, class props,
// class extends generic args, interface extends generic args). Build a
// minimal `CallVisitor` with the right `enclosing_fqdn` + type
// emission context, then ride swc's default traversal which fires our
// `visit_ts_type_ref` override on every leaf reference.

pub(crate) fn visit_type_ann_for_uses(
    ctx: &mut TsWalkContext<'_>,
    ann: &TsTypeAnn,
    current_module: &str,
    enclosing_fqdn: &str,
    emission_context: &'static str,
) {
    let mut visitor = CallVisitor::new(ctx, current_module, enclosing_fqdn);
    visitor.type_emission_context = Some(emission_context);
    ann.visit_with(&mut visitor);
}

pub(crate) fn visit_type_params_for_uses(
    ctx: &mut TsWalkContext<'_>,
    params: &TsTypeParamInstantiation,
    current_module: &str,
    enclosing_fqdn: &str,
    emission_context: &'static str,
) {
    let mut visitor = CallVisitor::new(ctx, current_module, enclosing_fqdn);
    visitor.type_emission_context = Some(emission_context);
    params.visit_with(&mut visitor);
}

pub(crate) fn visit_ts_type_for_uses(
    ctx: &mut TsWalkContext<'_>,
    ts_type: &swc_core::ecma::ast::TsType,
    current_module: &str,
    enclosing_fqdn: &str,
    emission_context: &'static str,
) {
    let mut visitor = CallVisitor::new(ctx, current_module, enclosing_fqdn);
    visitor.type_emission_context = Some(emission_context);
    ts_type.visit_with(&mut visitor);
}

pub(crate) fn visit_pat_for_uses(
    ctx: &mut TsWalkContext<'_>,
    pat: &Pat,
    current_module: &str,
    enclosing_fqdn: &str,
    emission_context: &'static str,
) {
    let mut visitor = CallVisitor::new(ctx, current_module, enclosing_fqdn);
    visitor.type_emission_context = Some(emission_context);
    pat.visit_with(&mut visitor);
}

pub(crate) fn visit_ts_fn_param_for_uses(
    ctx: &mut TsWalkContext<'_>,
    param: &swc_core::ecma::ast::TsFnParam,
    current_module: &str,
    enclosing_fqdn: &str,
    emission_context: &'static str,
) {
    let mut visitor = CallVisitor::new(ctx, current_module, enclosing_fqdn);
    visitor.type_emission_context = Some(emission_context);
    param.visit_with(&mut visitor);
}

pub(crate) fn visit_ts_type_param_decl_for_uses(
    ctx: &mut TsWalkContext<'_>,
    decl: &swc_core::ecma::ast::TsTypeParamDecl,
    current_module: &str,
    enclosing_fqdn: &str,
    emission_context: &'static str,
) {
    let mut visitor = CallVisitor::new(ctx, current_module, enclosing_fqdn);
    visitor.type_emission_context = Some(emission_context);
    decl.visit_with(&mut visitor);
}

/// Pass-2 entry: walk a function body for `CallExpr` / `NewExpr`. Mirror of
/// `rust::extract_call::visit_block`. Skips dynamic dispatch (`obj.method()`
/// always Unresolved with the method ident, day-1, no inference).
///
/// Lock 41 §2.4 PIVOT: the JSX template-extraction lives inside this same
/// visitor (rather than a separate `JsxRefVisitor` type the scaffold
/// posed) so the enclosing FQDN tracking is reused for free and we don't
/// run the AST twice. JSX nodes auto-fire `visit_jsx_element` on top of
/// the existing CallExpr / NewExpr / OptChain handling — the `jsx_context`
/// flag gates the REFERENCES emission so non-JSX TS files are unaffected.
pub(crate) fn visit_function_body(
    ctx: &mut TsWalkContext<'_>,
    function: &Function,
    current_module: &str,
    enclosing_fqdn: &str,
) {
    if function.body.is_none() {
        return;
    }
    let mut visitor = CallVisitor::new(ctx, current_module, enclosing_fqdn);
    // Stage 2: enter via `function.visit_with` so our `visit_function`
    // override pushes the function scope and seeds params before walking
    // the body. Stage 1 used `body.visit_with` which bypassed the
    // function envelope entirely.
    function.visit_with(&mut visitor);
}

/// Walk an arbitrary expression (typically the initializer of a `const fn = …`
/// arrow / function expression) for `CallExpr` / `NewExpr` nested inside.
pub(crate) fn visit_expression_for_calls(
    ctx: &mut TsWalkContext<'_>,
    expr: &Expr,
    current_module: &str,
    enclosing_fqdn: &str,
) {
    let mut visitor = CallVisitor::new(ctx, current_module, enclosing_fqdn);
    expr.visit_with(&mut visitor);
}

/// Pass-2 entry: walk a constructor body for `CallExpr` / `NewExpr` /
/// `Callee::Super`. Mirror of `visit_function_body` but adapted for SWC's
/// `Constructor` node, which is a distinct AST type from `Function` (no
/// `return_type`, no `type_params`, params are `ParamOrTsParamProp`).
pub(crate) fn visit_constructor_body(
    ctx: &mut TsWalkContext<'_>,
    ctor: &swc_core::ecma::ast::Constructor,
    current_module: &str,
    enclosing_fqdn: &str,
) {
    if ctor.body.is_none() {
        return;
    }
    let mut visitor = CallVisitor::new(ctx, current_module, enclosing_fqdn);
    // Enter via `ctor.visit_with` so our `visit_constructor` override
    // pushes the constructor scope and seeds params before walking the
    // body — same shape as `visit_function_body` delegates to `visit_function`.
    ctor.visit_with(&mut visitor);
}

/// `template-*` slug carried into `RawEdge.attributes` for JSX-extracted
/// REFERENCES edges. Mirror of [`crate::template::TemplateAttribute`] but
/// kept ASCII-only here so the visitor doesn't depend on the template
/// module's enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JsxAttribute {
    /// `prop={...}` JSX attribute value or spread `{...props}`.
    Bind,
    /// `{expr}` JSX child expression.
    Interpolation,
}

impl JsxAttribute {
    const fn as_slug(self) -> &'static str {
        match self {
            Self::Bind => "template-bind",
            Self::Interpolation => "template-interpolation",
        }
    }
}

/// Mutability of a `VarDecl` whose RHS we propagate as a scope alias.
/// `const` bindings are stable; `let`/`var` can be reassigned, so we
/// still track them but tag the emitted edges with `via-alias-mutable`
/// so consumers know the resolution could be stale if a reassignment
/// happened between the declaration and the read. Bug B Stage 2 does
/// NOT track reassignments (no SSA, no dataflow) — that's a Stage 3
/// concern once the per-module AOT lookup table is in place.
/// Visitor-side attribute slug for an `AliasMutability` propagated edge.
/// `via-alias` for const aliases, `via-alias-mutable` for let/var (where
/// the binding can be reassigned between alias-seeding and the read,
/// invalidating the propagation — the slug lets consumers discount these).
const fn alias_mut_slug(m: AliasMutability) -> &'static str {
    match m {
        AliasMutability::Const => "via-alias",
        AliasMutability::Mutable => "via-alias-mutable",
    }
}

/// Result of resolving a name against the AOT `ModuleLookup` plus the
/// `resolve_call` fall-through chain. Internal output type — callers
/// pattern-match on the variants to decide between emitting an edge
/// (Target) or skipping (Local for nested-scope bindings without alias,
/// Drop for tier-classified noise, Attribute for tier-classified
/// source-flag promotion targets).
enum NameResolution {
    /// Either an alias propagation (Some(mutability)) or a module-level
    /// resolution (None) — both carry the canonical target so the caller
    /// can emit `Calls` / `References` / `UsesType` accordingly. The
    /// inner [`CallTarget`] also carries the originating [`BuiltinTag`]
    /// when the resolution landed on an Edge-tier builtin.
    Target(CallTarget, Option<AliasMutability>),
    /// Nested-scope local binding (param, let/const/var/fn-expr) without
    /// an alias. Callers skip emission — locals aren't surfaced in the
    /// module graph by design.
    Local,
    /// Stage 3e-1 : the name matched a builtin classified as
    /// [`BuiltinTier::Drop`] (`Array` / `Map` / `undefined` / …).
    /// Every emit path silently returns — no edge, no flag.
    Drop,
    /// Stage 3e-1b : the name matched a builtin classified as
    /// [`BuiltinTier::Attribute`] (`Promise` / iterator families).
    /// Every emit path skips the edge but registers the carried tag
    /// against the enclosing FQDN via
    /// [`TsWalkContext::register_attribute_flag`] so the symbol picks
    /// up `"async"` / `"iter"` / custom UST flags in its `flags` vec.
    Attribute(BuiltinTag),
}

struct CallVisitor<'a, 'b> {
    ctx: &'a mut TsWalkContext<'b>,
    current_module: String,
    enclosing_fqdn: String,
    /// `Some(slug)` while walking the inside of a JSX expression slot.
    /// `None` everywhere else — keeps non-JSX TS extraction unchanged.
    jsx_context: Option<JsxAttribute>,
    /// Stage 3a-6c — scope_idx into `ctx.core.lookup.scopes` for the
    /// AST node currently being visited. Threaded by
    /// `enter_scope_at` / `exit_scope` at function / arrow / block /
    /// for-loop / catch boundaries. `ModuleLookup::ROOT_SCOPE` when
    /// the visitor is at module-top-level (matches the lookup's
    /// implicit root, where hoisted module-level decls + imports live).
    current_scope_idx: u32,
    /// Parent-scope stack mirroring nested `enter_scope_at` calls —
    /// `exit_scope` pops back to the parent. Replaces the old
    /// `Vec<Scope>` book-keeping; binding state lives in the lookup.
    scope_stack: Vec<u32>,
    /// Bug B Stage 2b — sub-tag attached to every `UsesType` edge emitted
    /// while this is `Some(_)`. Set/restored around the specific sub-tree
    /// walks (return type, type params, casts, instantiations, etc.) so a
    /// nested `TsTypeRef` deep inside knows its emission context. `None`
    /// outside any type position — `emit_uses_type` skips emission then
    /// because nothing's asking for a type-edge from here.
    type_emission_context: Option<&'static str>,
}

impl<'a, 'b> CallVisitor<'a, 'b> {
    fn new(ctx: &'a mut TsWalkContext<'b>, current_module: &str, enclosing_fqdn: &str) -> Self {
        Self {
            ctx,
            current_module: current_module.to_string(),
            enclosing_fqdn: enclosing_fqdn.to_string(),
            jsx_context: None,
            current_scope_idx: ModuleLookup::ROOT_SCOPE,
            scope_stack: Vec::with_capacity(8),
            type_emission_context: None,
        }
    }

    // --- Stage 3a-6c: scope_idx threading against the AOT lookup ---

    /// Switch `current_scope_idx` to the lookup-side scope covering
    /// `[byte_lo, byte_hi)`. Falls back to the current scope when the
    /// pre-pass didn't record this span (the visitor entered a node the
    /// builder didn't treat as scope-introducing — resolution still
    /// works, just against the enclosing scope).
    fn enter_scope_at(&mut self, byte_lo: u32, byte_hi: u32) {
        let parent = self.current_scope_idx;
        self.scope_stack.push(parent);
        self.current_scope_idx = self
            .ctx
            .core
            .lookup
            .scope_idx_for_span(byte_lo, byte_hi)
            .unwrap_or(parent);
    }

    fn exit_scope(&mut self) {
        self.current_scope_idx = self.scope_stack.pop().unwrap_or(ModuleLookup::ROOT_SCOPE);
    }

    /// Resolve `name` against the AOT lookup, then fall through to
    /// [`TsWalkContext::resolve_call`] for module-level resolution.
    ///
    /// Behaviour mirrors the pre-3a-6c scope-walk:
    /// - Lookup miss → fall through to `resolve_call` (alias_table →
    ///   defined_fqdns → builtin → unresolved canonical).
    /// - Lookup hit at `ROOT_SCOPE` (hoisted decl or import) → fall
    ///   through to `resolve_call` so the canonical FQDN is produced
    ///   exactly as before. The lookup's root-scope tracking is
    ///   informational here; `resolve_call` already owns the
    ///   alias_table for imports and `defined_fqdns` for hoisted decls.
    /// - Lookup hit at nested scope with `aliases_to` + `mutability` →
    ///   propagated alias — resolve the leftmost-base through
    ///   `resolve_call` and tag with the mutability slug.
    /// - Lookup hit at nested scope without alias → [`NameResolution::Local`].
    fn resolve_name(&self, name: &str) -> NameResolution {
        let lookup = &self.ctx.core.lookup;
        if let Some(res) = lookup.resolve_local(name, self.current_scope_idx) {
            if res.scope_idx == ModuleLookup::ROOT_SCOPE {
                return outcome_to_resolution(
                    self.ctx.resolve_call(name, &self.current_module),
                    None,
                );
            }
            if let (Some(alias_str), Some(m)) = (res.aliases_to.as_deref(), res.mutability) {
                return outcome_to_resolution(
                    self.ctx.resolve_call(alias_str, &self.current_module),
                    Some(m),
                );
            }
            return NameResolution::Local;
        }
        outcome_to_resolution(self.ctx.resolve_call(name, &self.current_module), None)
    }

    fn emit_call(&mut self, target: CallTarget, span: Span) {
        let mut attrs: Vec<String> = vec![];
        append_via_builtin(&mut attrs, target.via_builtin.as_ref());
        self.emit_call_raw(target.to, span, attrs);
    }

    /// IR-4-c: push an observational [`RawCallSite`] for the plugin
    /// layer. Independent of the Drop / Local / Attribute tier
    /// decisions made by `handle_callee_expr` — `foo()` still emits a
    /// call_site even when the matching Calls edge is suppressed.
    fn emit_call_site(
        &mut self,
        callee_text: String,
        args: Vec<RawCallArg>,
        receiver_chain: Vec<String>,
        span: Span,
    ) {
        if callee_text.is_empty() {
            // Pathological callee shape (`(0, x)()` etc.) — `callee_text_of`
            // returned the empty placeholder. Skip rather than emit a
            // useless call_site that the plugin layer can't act on.
            return;
        }
        let site = self.ctx.span_site(span);
        self.ctx.push_call_site(RawCallSite {
            from_fqdn: self.enclosing_fqdn.clone(),
            callee_text,
            args,
            receiver_chain,
            site,
        });
    }

    /// Bug B Stage 2 — `Calls` edge emitted through a propagated scope
    /// alias. Tags the edge with `via-alias` / `via-alias-mutable` so
    /// consumers can distinguish direct calls from chained-binding
    /// calls (and know to discount mutable-alias edges). Edge-tier
    /// builtins shadowed by an alias still carry `via-builtin` since the
    /// resolution propagates through the alias chain.
    fn emit_call_via_alias(&mut self, target: CallTarget, span: Span, mutability: AliasMutability) {
        let mut attrs: Vec<String> = vec![alias_mut_slug(mutability).to_string()];
        append_via_builtin(&mut attrs, target.via_builtin.as_ref());
        self.emit_call_raw(target.to, span, attrs);
    }

    /// Lowest-level Calls emitter — used directly for member-access
    /// expressions (`obj.method()`) where the resolution is always an
    /// explicit `Unresolved` constructed at the call site, with no
    /// `CallTarget` plumbing involved.
    fn emit_call_raw(&mut self, to: ResolvedOrUnresolved, span: Span, attributes: Vec<String>) {
        let site = self.ctx.span_site(span);
        let confidence = to.default_confidence();
        self.ctx.push_edge(RawEdge {
            from_fqdn: self.enclosing_fqdn.clone(),
            kind: EdgeKind::Calls,
            to,
            sites: vec![site],
            attributes,
            confidence,
            receiver_type: None,
        });
    }

    fn emit_template_ref(&mut self, name: &str, span: Span, attribute: &str) {
        // Stage 2: scope waterfall enriches the edge with `via-alias`
        // when applicable, but local bindings ARE still surfaced for
        // JSX template binds — the whole point of template extraction
        // is enumerating identifier reads a component depends on,
        // regardless of whether the binding is local (a `props` param
        // or a destructured slot is still the read we want surfaced).
        // Pure value-reads (`emit_value_ref`) keep the strict local-skip
        // rule; this asymmetry is intentional.
        let (target, alias_mut) = match self.resolve_name(name) {
            NameResolution::Drop => return,
            NameResolution::Attribute(tag) => {
                self.ctx.register_attribute_flag(&self.enclosing_fqdn, &tag);
                return;
            }
            // Local binding shadows the name — re-resolve through the
            // module-level chain to produce a canonical-unresolved (or
            // Drop/Attribute for builtins, which we then skip / flag).
            NameResolution::Local => match self.ctx.resolve_call(name, &self.current_module) {
                ResolutionOutcome::Drop => return,
                ResolutionOutcome::Attribute(tag) => {
                    self.ctx.register_attribute_flag(&self.enclosing_fqdn, &tag);
                    return;
                }
                ResolutionOutcome::Emit(t) => (t, None),
            },
            NameResolution::Target(t, m) => (t, m),
        };
        let confidence = target.to.default_confidence();
        let site = self.ctx.span_site(span);
        let mut attributes = vec![attribute.to_string()];
        if let Some(m) = alias_mut {
            attributes.push(alias_mut_slug(m).to_string());
        }
        append_via_builtin(&mut attributes, target.via_builtin.as_ref());
        self.ctx.push_edge(RawEdge {
            from_fqdn: self.enclosing_fqdn.clone(),
            kind: EdgeKind::References,
            to: target.to,
            sites: vec![site],
            attributes,
            confidence,
            receiver_type: None,
        });
    }

    /// Bug B Stage 2 — emit a `References` edge for a plain
    /// identifier read in value position. Waterfall:
    ///
    /// 1. Scope alias hit → emit `value-read` + `via-alias{,-mutable}`
    ///    pointing at the propagated target (which may itself be
    ///    `Unresolved` canonical for imported aliases — in which case
    ///    we still skip Stage 1's `Unresolved`-skip rule below).
    /// 2. Scope local-binding hit → skip (locals aren't surfaced).
    /// 3. Miss → `resolve_call` (Stage 1 path). Skip unresolved /
    ///    unresolved-bridge to preserve the Stage 1 safety net.
    ///
    /// Self-references on the enclosing FQDN are dropped explicitly
    /// (a function reading its own name in a recursive call is
    /// already covered by the `Calls` edge).
    fn emit_value_ref(&mut self, name: &str, span: Span) {
        if JS_GLOBALS.contains(&name) {
            return;
        }
        let (target, alias_mut) = match self.resolve_name(name) {
            NameResolution::Drop | NameResolution::Local => return,
            NameResolution::Attribute(tag) => {
                self.ctx.register_attribute_flag(&self.enclosing_fqdn, &tag);
                return;
            }
            NameResolution::Target(t, m) => (t, m),
        };
        let target_fqdn = match &target.to {
            ResolvedOrUnresolved::Resolved { fqdn } => fqdn,
            // Skip unresolved value-reads — Stage 1 safety net preserved.
            // An aliased canonical-unresolved target is also dropped here;
            // the matching `Calls` edge (when the alias is invoked) still
            // carries `via-alias{,-mutable}` via `emit_call_via_alias`.
            ResolvedOrUnresolved::Unresolved { .. }
            | ResolvedOrUnresolved::UnresolvedBridge { .. } => return,
        };
        if target_fqdn == &self.enclosing_fqdn {
            return;
        }
        let confidence = target.to.default_confidence();
        let site = self.ctx.span_site(span);
        let mut attributes = vec!["value-read".to_string()];
        if let Some(m) = alias_mut {
            attributes.push(alias_mut_slug(m).to_string());
        }
        append_via_builtin(&mut attributes, target.via_builtin.as_ref());
        self.ctx.push_edge(RawEdge {
            from_fqdn: self.enclosing_fqdn.clone(),
            kind: EdgeKind::References,
            to: target.to,
            sites: vec![site],
            attributes,
            confidence,
            receiver_type: None,
        });
    }

    /// After `handle_callee_expr` has emitted the `Calls` edge, walk
    /// the rest of the callee expression so nested base identifiers
    /// still produce their own `References` edges (e.g. `obj.method()`
    /// emits a `Calls` on `method` AND a `References` on `obj`).
    /// Skips the immediate identifier already counted as a call.
    /// Bug B Stage 2b — emit a `UsesType` edge for a TS type reference
    /// reached anywhere a type-position walk is currently active (the
    /// `type_emission_context` field on `self`). When the context is
    /// `None` no edge is emitted — this guards against type-position
    /// idents accidentally firing outside a real type position.
    ///
    /// Resolution: same waterfall as `emit_value_ref` (scope alias /
    /// scope local / fall-through `resolve_call`). Skipped names:
    /// - Drop-tier builtins (`Array<Foo>`, `Map<K,V>`, `Partial<T>`, …) —
    ///   `resolve_call` returns `None`, surfaced as `NameResolution::Skip`.
    ///   Inner type args still recurse from the caller's `visit_ts_type_ref`.
    /// - Attribute-tier builtins (`Promise<T>`, iterator families) — also
    ///   `Skip` for now; 3e-1b promotes them to source-symbol flags.
    /// - Scope-local bindings (incl. generic type params bound in the
    ///   enclosing function scope) — no edge for `function f<T>(x: T)`.
    /// - Self-references on `enclosing_fqdn`.
    ///
    /// Edge-tier builtins (`Date`, `JSON`, `Math`, `Error`, `Object`,
    /// `Symbol`, …) now DO surface — they emit a `UsesType` edge to the
    /// synthetic builtin FQDN with `via-builtin` / `builtin-<slug>`
    /// attributes (parity with Rust UsesType, fixing a Stage 3a-6c gap).
    ///
    /// Unresolved targets ARE emitted (with an extra `unresolved-type`
    /// attribute) so the viz can toggle them on/off without re-extracting.
    fn emit_uses_type(&mut self, name: &str, span: Span) {
        let Some(ctx) = self.type_emission_context else {
            return;
        };
        let target = match self.resolve_name(name) {
            NameResolution::Drop | NameResolution::Local => return,
            NameResolution::Attribute(tag) => {
                self.ctx.register_attribute_flag(&self.enclosing_fqdn, &tag);
                return;
            }
            NameResolution::Target(t, _) => t,
        };
        if let ResolvedOrUnresolved::Resolved { fqdn } = &target.to
            && fqdn == &self.enclosing_fqdn
        {
            return;
        }
        let is_unresolved = matches!(
            &target.to,
            ResolvedOrUnresolved::Unresolved { .. } | ResolvedOrUnresolved::UnresolvedBridge { .. }
        );
        let confidence = target.to.default_confidence();
        let site = self.ctx.span_site(span);
        let mut attributes = vec!["via-type".to_string(), ctx.to_string()];
        if is_unresolved {
            attributes.push(TYPE_TAG_UNRESOLVED.to_string());
        }
        append_via_builtin(&mut attributes, target.via_builtin.as_ref());
        self.ctx.push_edge(RawEdge {
            from_fqdn: self.enclosing_fqdn.clone(),
            kind: EdgeKind::UsesType,
            to: target.to,
            sites: vec![site],
            attributes,
            confidence,
            receiver_type: None,
        });
    }

    /// Push a type-emission context onto `self.type_emission_context`
    /// and return the previous value. Pair with [`restore_type_context`]
    /// to wrap a sub-tree walk. We use a `set/restore` pair rather than
    /// a closure because `Visit`-trait calls take `&mut self` and need
    /// to compose with `node.visit_with(self)` cleanly.
    const fn set_type_context(&mut self, ctx: &'static str) -> Option<&'static str> {
        self.type_emission_context.replace(ctx)
    }

    const fn restore_type_context(&mut self, prev: Option<&'static str>) {
        self.type_emission_context = prev;
    }

    fn walk_callee_remainder(&mut self, expr: &Expr) {
        match expr {
            Expr::Ident(ident) => {
                // Already emitted as Calls. In **JSX context**,
                // legacy behavior also surfaces the ident as a
                // `template-bind` Reference (event handlers like
                // `onClick={() => handle(payload)}` produce BOTH
                // the Calls edge and the template-bind edge — the
                // latter feeds component-graph tooling). Outside
                // JSX, Stage 1's value-read would be a redundant
                // duplicate of the Calls edge, so we skip.
                if self.jsx_context.is_some() {
                    ident.visit_with(self);
                }
            }
            // Property access: the prop name was emitted as an
            // unresolved Calls; recurse only into `obj` so the base
            // gets a References edge.
            Expr::Member(m) => m.obj.visit_with(self),
            Expr::OptChain(opt) => match opt.base.as_ref() {
                OptChainBase::Member(m) => m.obj.visit_with(self),
                // Nested call as callee — let normal traversal pick
                // it up (rare; e.g. `(getFn())()`).
                OptChainBase::Call(_) => expr.visit_with(self),
            },
            other => other.visit_with(self),
        }
    }

    fn handle_callee_expr(&mut self, callee: &Expr) {
        match callee {
            Expr::Ident(ident) => {
                // Stage 2: route through the scope waterfall before
                // falling through to `resolve_call`. Local bindings
                // (hoisted nested fns, closure-captured aliases pointing
                // at a local, etc.) are NOT surfaced — the matching
                // call lives entirely inside the enclosing FQDN.
                match self.resolve_name(ident.sym.as_ref()) {
                    NameResolution::Local | NameResolution::Drop => {}
                    NameResolution::Attribute(tag) => {
                        self.ctx.register_attribute_flag(&self.enclosing_fqdn, &tag);
                    }
                    NameResolution::Target(target, Some(m)) => {
                        self.emit_call_via_alias(target, ident.span, m);
                    }
                    NameResolution::Target(target, None) => {
                        self.emit_call(target, ident.span);
                    }
                }
            }
            Expr::Member(member) => {
                if let Some(name) = member_prop_name(&member.prop) {
                    self.emit_call_raw(
                        ResolvedOrUnresolved::Unresolved { name },
                        member.span,
                        vec![],
                    );
                }
            }
            Expr::OptChain(opt) => {
                if let OptChainBase::Member(member) = opt.base.as_ref()
                    && let Some(name) = member_prop_name(&member.prop)
                {
                    self.emit_call_raw(ResolvedOrUnresolved::Unresolved { name }, opt.span, vec![]);
                }
            }
            _ => {}
        }
    }
}

impl Visit for CallVisitor<'_, '_> {
    fn visit_call_expr(&mut self, node: &CallExpr) {
        // IR-4-c: observational call_site — emitted before the edge-tier
        // dispatch in `handle_callee_expr` so Drop / Local / Attribute
        // suppression of the graph edge doesn't suppress the plugin-
        // visible record.
        let (callee_text, recv_chain) = match &node.callee {
            Callee::Expr(e) => (callee_text_of(e), receiver_chain_for_callee(e)),
            Callee::Super(_) => ("super".to_string(), Vec::new()),
            Callee::Import(_) => ("import".to_string(), Vec::new()),
        };
        let args = args_from_swc(self.ctx, &node.args);
        self.emit_call_site(callee_text, args, recv_chain, node.span);

        if let Callee::Expr(expr) = &node.callee {
            self.handle_callee_expr(expr);
            // Walk the rest of the callee for base identifiers (e.g.
            // emit a References on `obj` in `obj.method()`). Avoids
            // double-firing on the ident we just emitted as a Call.
            self.walk_callee_remainder(expr);
        }
        // Args carry independent expression positions where value-reads
        // must surface. Walk them normally.
        for arg in &node.args {
            arg.visit_with(self);
        }
        // Stage 2b: type args (`foo<Bar>()`) fire `UsesType` edges
        // under the `type-instantiation` context.
        if let Some(type_args) = &node.type_args {
            let prev = self.set_type_context(TYPE_CTX_INSTANTIATION);
            type_args.visit_with(self);
            self.restore_type_context(prev);
        }
    }

    fn visit_new_expr(&mut self, node: &NewExpr) {
        let callee = node.callee.as_ref();
        // IR-4-c: `new Foo(x)` surfaces as a call_site with the
        // constructor expression as callee_text. No `new` prefix in the
        // text — plugins can derive it from edge-kind context.
        let args = node
            .args
            .as_deref()
            .map(|a| args_from_swc(self.ctx, a))
            .unwrap_or_default();
        self.emit_call_site(
            callee_text_of(callee),
            args,
            receiver_chain_for_callee(callee),
            node.span,
        );
        self.handle_callee_expr(callee);
        self.walk_callee_remainder(callee);
        if let Some(args) = &node.args {
            for arg in args {
                arg.visit_with(self);
            }
        }
        if let Some(type_args) = &node.type_args {
            let prev = self.set_type_context(TYPE_CTX_INSTANTIATION);
            type_args.visit_with(self);
            self.restore_type_context(prev);
        }
    }

    // OptCall is reached via visit_children on OptChainExpr — visiting it
    // separately would double-emit. We handle OptChainBase::Call here and
    // let the recursion visit the args inside.
    fn visit_opt_chain_expr(&mut self, node: &OptChainExpr) {
        if let OptChainBase::Call(call) = node.base.as_ref() {
            // IR-4-c: optional-call `foo?.()` — callee_text from the
            // wrapped callee, receiver_chain from its Member receiver
            // (if any). The `?` semantics are captured by the chain
            // helpers, not by the call_site itself.
            let callee_expr = call.callee.as_ref();
            self.emit_call_site(
                callee_text_of(callee_expr),
                args_from_swc(self.ctx, &call.args),
                receiver_chain_for_callee(callee_expr),
                node.span,
            );
            self.handle_callee_expr(callee_expr);
            self.walk_callee_remainder(callee_expr);
            for arg in &call.args {
                arg.visit_with(self);
            }
        } else {
            node.visit_children_with(self);
        }
    }

    /// JSX entry point. Walks the element manually rather than through
    /// `visit_children_with` so we can:
    ///   1. Treat the opening tag name specially (uppercase → component
    ///      ref; lowercase → HTML, ignored).
    ///   2. Tag attribute-value identifiers as `template-bind`.
    ///   3. Tag child `{expr}` identifiers as `template-interpolation`.
    ///   4. Recurse into nested JSX without re-firing on the parent name.
    fn visit_jsx_element(&mut self, node: &JSXElement) {
        // 1. Component ref on the opening tag name.
        if let JSXElementName::Ident(id) = &node.opening.name
            && id
                .sym
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_uppercase())
        {
            let name = id.sym.to_string();
            self.emit_template_ref(&name, id.span, "template-component-ref");
        }
        // 2. Attributes — each value's expr container fires under
        //    `JsxAttribute::Bind`.
        let saved = self.jsx_context;
        for attr in &node.opening.attrs {
            match attr {
                JSXAttrOrSpread::JSXAttr(a) => {
                    if let Some(value) = &a.value {
                        self.jsx_context = Some(JsxAttribute::Bind);
                        match value {
                            JSXAttrValue::JSXExprContainer(c) => {
                                if let JSXExpr::Expr(e) = &c.expr {
                                    e.visit_with(self);
                                }
                            }
                            JSXAttrValue::JSXElement(el) => {
                                el.visit_with(self);
                            }
                            JSXAttrValue::JSXFragment(frag) => {
                                frag.visit_with(self);
                            }
                            // Plain string literal value — not an expression.
                            JSXAttrValue::Str(_) => {}
                        }
                    }
                }
                JSXAttrOrSpread::SpreadElement(s) => {
                    self.jsx_context = Some(JsxAttribute::Bind);
                    s.expr.visit_with(self);
                }
            }
        }
        // 3. Children.
        for child in &node.children {
            match child {
                JSXElementChild::JSXExprContainer(c) => {
                    self.jsx_context = Some(JsxAttribute::Interpolation);
                    if let JSXExpr::Expr(e) = &c.expr {
                        e.visit_with(self);
                    }
                }
                JSXElementChild::JSXSpreadChild(s) => {
                    self.jsx_context = Some(JsxAttribute::Bind);
                    s.expr.visit_with(self);
                }
                JSXElementChild::JSXElement(el) => {
                    self.jsx_context = saved; // recursion takes the outer context (None).
                    el.visit_with(self);
                }
                JSXElementChild::JSXFragment(frag) => {
                    self.jsx_context = saved;
                    frag.visit_with(self);
                }
                JSXElementChild::JSXText(_) => {}
            }
        }
        self.jsx_context = saved;
    }

    /// Identifier reads. Two paths share the same entry point:
    ///
    /// 1. JSX expression slot — every plain identifier becomes a
    ///    `References` edge tagged with the active `template-*` slug
    ///    (`template-bind` / `template-interpolation`).
    /// 2. Plain value position (Bug B Stage 1) — emits a generic
    ///    `value-read` `References` edge when the name resolves to
    ///    an import alias or a module top-level symbol. Locals,
    ///    params, builtins, and types skip out cleanly because
    ///    `resolve_call` returns `Unresolved` for them.
    ///
    /// Member-prop names live in `MemberProp::Ident(IdentName)` (not
    /// `Ident`), so they never reach this method — the "left-most
    /// segment" rule from [`crate::template`] still applies.
    fn visit_ident(&mut self, ident: &Ident) {
        let name = ident.sym.to_string();
        if let Some(attribute) = self.jsx_context {
            if JS_GLOBALS.contains(&name.as_str()) {
                return;
            }
            self.emit_template_ref(&name, ident.span, attribute.as_slug());
            return;
        }
        // Stage 1 value-read emission. The filtering happens inside
        // `emit_value_ref` (JS_GLOBALS, unresolved, self-reference).
        self.emit_value_ref(&name, ident.span);
    }

    /// Suppress `visit_ident` for binding positions (declarator
    /// names, function params, destructuring slots). swc wraps every
    /// binding in `BindingIdent` and the default visitor recurses
    /// into the inner `Ident` — which would trigger `visit_ident`
    /// and emit a spurious `References` edge on the local name.
    ///
    /// Stage 2b: the `type_ann` field IS walked so param + var
    /// annotations produce `UsesType` edges. The `id` is still
    /// skipped. The emission context defaults to `type-annotation`
    /// but defers to any OUTER context already set by the caller
    /// (e.g. walking an `interface I { m(x: Foo): Bar }` body with
    /// `type-interface-member` context — the param annotation should
    /// inherit that outer tag rather than getting reset to
    /// `type-annotation`).
    fn visit_binding_ident(&mut self, node: &swc_core::ecma::ast::BindingIdent) {
        if let Some(ann) = &node.type_ann {
            let prev = self.type_emission_context;
            if prev.is_none() {
                self.type_emission_context = Some(TYPE_CTX_ANNOTATION);
            }
            ann.visit_with(self);
            self.type_emission_context = prev;
        }
    }

    /// Function declarations introduce a name binding (the function
    /// `ident`) plus a body. We want to walk the body for nested
    /// calls / reads, but the binding ident itself must not emit a
    /// `References` edge (it's a declaration, not a use). swc's
    /// default `visit_fn_decl` visits the ident, so we override.
    fn visit_fn_decl(&mut self, node: &swc_core::ecma::ast::FnDecl) {
        node.function.visit_with(self);
    }

    /// Same rationale as `visit_fn_decl` — skip the class name
    /// binding and only walk the class body.
    fn visit_class_decl(&mut self, node: &swc_core::ecma::ast::ClassDecl) {
        node.class.visit_with(self);
    }

    // --- Bug B Stage 2: scope-pushing overrides ---
    //
    // Push/pop a `Scope` frame at every lexical boundary. Each frame
    // collects (a) plain binding names seeded by a pre-pass over the
    // block's statements (function/class hoisting + lax let/const
    // forward refs) plus (b) alias mappings seeded at declarator
    // sites (`const x = FOO`). The frames terminate the
    // `resolve_name` waterfall before `resolve_call` is reached.

    /// Function envelope (decls + expressions + methods). Pushes a
    /// scope, seeds param bindings, walks param-default expressions
    /// and the body's statements directly. Stage 2b additions: bind
    /// generic type params (`<T>`) as scope-local so a body ident `T`
    /// in type position is filtered out, walk `type_params` with
    /// `type-constraint` context (so `<T extends Foo>` emits a
    /// `UsesType` on Foo), walk `return_type` with `type-annotation`
    /// context. We bypass `visit_block_stmt` on the body so the
    /// function scope and its body block don't double-push — in JS
    /// semantics the function body IS the function scope.
    fn visit_function(&mut self, node: &Function) {
        self.enter_scope_at(node.span.lo.0, node.span.hi.0);
        // Type params: bindings live in the lookup; walk only to emit
        // `UsesType` edges (Stage 2b) under the right type context.
        if let Some(type_params) = &node.type_params {
            let prev = self.set_type_context(TYPE_CTX_CONSTRAINT);
            type_params.visit_with(self);
            self.restore_type_context(prev);
        }
        for param in &node.params {
            param.visit_with(self);
        }
        if let Some(rt) = &node.return_type {
            let prev = self.set_type_context(TYPE_CTX_ANNOTATION);
            rt.visit_with(self);
            self.restore_type_context(prev);
        }
        // Body delegates to `visit_block_stmt` which enters the Block
        // scope (where the lookup pre-pass records body-local bindings
        // like `const fn = FOO`). Walking `body.stmts` directly would
        // leave `current_scope_idx` at the function scope and miss
        // those bindings when resolving inner idents.
        if let Some(body) = &node.body {
            body.visit_with(self);
        }
        self.exit_scope();
    }

    /// Constructor envelope — mirrors `visit_function` for SWC's
    /// `Constructor` node. No `return_type` (constructors are
    /// implicit-void in TS) and no `type_params` (TS forbids generic
    /// constructors). Params are `ParamOrTsParamProp` — the latter is
    /// the `constructor(private readonly db: Db)` shorthand whose
    /// idents are bound by `LookupBuilder::visit_constructor` with
    /// the `"param-property"` attribute.
    fn visit_constructor(&mut self, node: &swc_core::ecma::ast::Constructor) {
        self.enter_scope_at(node.span.lo.0, node.span.hi.0);
        for param in &node.params {
            param.visit_with(self);
        }
        if let Some(body) = &node.body {
            body.visit_with(self);
        }
        self.exit_scope();
    }

    /// Arrow expression — same pattern as `visit_function` but params
    /// are bare `Pat`s (no `Param` wrapper) and the body is either a
    /// block or a single expression.
    fn visit_arrow_expr(&mut self, node: &ArrowExpr) {
        self.enter_scope_at(node.span.lo.0, node.span.hi.0);
        if let Some(type_params) = &node.type_params {
            let prev = self.set_type_context(TYPE_CTX_CONSTRAINT);
            type_params.visit_with(self);
            self.restore_type_context(prev);
        }
        for param in &node.params {
            param.visit_with(self);
        }
        if let Some(rt) = &node.return_type {
            let prev = self.set_type_context(TYPE_CTX_ANNOTATION);
            rt.visit_with(self);
            self.restore_type_context(prev);
        }
        match node.body.as_ref() {
            BlockStmtOrExpr::BlockStmt(b) => {
                // Same rationale as `visit_function`: delegate to
                // `visit_block_stmt` so the Block scope is entered.
                b.visit_with(self);
            }
            BlockStmtOrExpr::Expr(e) => e.visit_with(self),
        }
        self.exit_scope();
    }

    /// Standalone block (`{ … }` inside an `if`, `else`, `try`, or
    /// just a bare block). Function bodies are walked stmt-by-stmt by
    /// `visit_function` / `visit_arrow_expr`, bypassing this method,
    /// so the function and body don't both push a frame.
    fn visit_block_stmt(&mut self, node: &BlockStmt) {
        self.enter_scope_at(node.span.lo.0, node.span.hi.0);
        node.visit_children_with(self);
        self.exit_scope();
    }

    fn visit_for_stmt(&mut self, node: &ForStmt) {
        self.enter_scope_at(node.span.lo.0, node.span.hi.0);
        node.visit_children_with(self);
        self.exit_scope();
    }

    fn visit_for_in_stmt(&mut self, node: &ForInStmt) {
        self.enter_scope_at(node.span.lo.0, node.span.hi.0);
        node.visit_children_with(self);
        self.exit_scope();
    }

    fn visit_for_of_stmt(&mut self, node: &ForOfStmt) {
        self.enter_scope_at(node.span.lo.0, node.span.hi.0);
        node.visit_children_with(self);
        self.exit_scope();
    }

    fn visit_catch_clause(&mut self, node: &CatchClause) {
        self.enter_scope_at(node.span.lo.0, node.span.hi.0);
        node.visit_children_with(self);
        self.exit_scope();
    }

    // --- Bug B Stage 2b: type-position overrides ---

    /// Every named TS type reference funnels through here.
    ///
    /// 1. Recurse into the type args FIRST so nested refs in
    ///    `Map<Foo, Bar>` still emit even when the outer wrapper
    ///    `Map` itself is filtered as a builtin.
    /// 2. Emit a `UsesType` edge on the leftmost ident of the entity
    ///    name (`Foo` from `Foo.Bar.Baz`). The filtering, scope
    ///    waterfall, and attribute composition live in `emit_uses_type`.
    fn visit_ts_type_ref(&mut self, node: &TsTypeRef) {
        if let Some(args) = &node.type_params {
            args.visit_with(self);
        }
        let name = leftmost_type_name(&node.type_name);
        self.emit_uses_type(&name, node.span);
    }

    /// `x as Foo` — walk the inner expression in value position, then
    /// walk the type annotation under `type-cast` context.
    fn visit_ts_as_expr(&mut self, node: &TsAsExpr) {
        node.expr.visit_with(self);
        let prev = self.set_type_context(TYPE_CTX_CAST);
        node.type_ann.visit_with(self);
        self.restore_type_context(prev);
    }

    /// `<Foo>x` — same as `visit_ts_as_expr` but the old-style cast.
    fn visit_ts_type_assertion(&mut self, node: &TsTypeAssertion) {
        node.expr.visit_with(self);
        let prev = self.set_type_context(TYPE_CTX_CAST);
        node.type_ann.visit_with(self);
        self.restore_type_context(prev);
    }
}

fn member_prop_name(prop: &MemberProp) -> Option<String> {
    match prop {
        MemberProp::Ident(i) => Some(i.sym.to_string()),
        MemberProp::PrivateName(p) => Some(format!("#{}", p.name)),
        MemberProp::Computed(_) => None,
    }
}

/// Extract the leftmost ident from a TS `TsEntityName` chain:
/// `Foo` → `"Foo"`, `Foo.Bar.Baz` → `"Foo"`. swc parses qualified type
/// names as left-recursive `TsQualifiedName { left, right }` so the
/// leftmost ident lives at the bottom of `left`.
fn leftmost_type_name(entity: &TsEntityName) -> String {
    match entity {
        TsEntityName::Ident(id) => id.sym.to_string(),
        TsEntityName::TsQualifiedName(q) => leftmost_type_name(&q.left),
    }
}

// --- IR-4-c: call_site emission helpers ---

/// Format a callee expression as the dotted text the IR records on
/// [`RawCallSite::callee_text`]. Mirrors swc's parse tree:
/// - `Ident("foo")` → `"foo"`
/// - `Member { obj, prop }` → `"<obj>.prop"` (computed props render
///   as `"<obj>.<computed>"` since the index isn't a stable name)
/// - `OptChain` → `"<obj>?.prop"` for the optional `.` variant
/// - `This` → `"this"`
/// - anything else → empty string (caller decides whether to skip)
fn callee_text_of(expr: &Expr) -> String {
    match expr {
        Expr::Ident(i) => i.sym.to_string(),
        Expr::Member(m) => {
            let base = callee_text_of(&m.obj);
            let prop = member_prop_name(&m.prop).unwrap_or_else(|| "<computed>".to_string());
            if base.is_empty() {
                prop
            } else {
                format!("{base}.{prop}")
            }
        }
        Expr::OptChain(opt) => match opt.base.as_ref() {
            OptChainBase::Member(m) => {
                let base = callee_text_of(&m.obj);
                let prop = member_prop_name(&m.prop).unwrap_or_else(|| "<computed>".to_string());
                if base.is_empty() {
                    prop
                } else {
                    format!("{base}?.{prop}")
                }
            }
            OptChainBase::Call(c) => callee_text_of(&c.callee),
        },
        Expr::This(_) => "this".to_string(),
        _ => String::new(),
    }
}

/// Walk a method-call receiver expression and produce the dotted
/// `receiver_chain` segment list in source order. `a.b.c()` (callee =
/// `Member { obj: a.b, prop: "c" }`) yields `["a", "b"]` (final
/// segment `c` is the method, not a receiver). Peels `Member` /
/// `OptChain::Member` layers; at the inner-most non-member base it
/// stops with a single segment (ident name, `this`, or full callee
/// text for non-trivial bases like `(x as any).y`).
fn receiver_chain_of(receiver: &Expr) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cursor: &Expr = receiver;
    loop {
        match cursor {
            Expr::Member(m) => {
                let seg = member_prop_name(&m.prop).unwrap_or_else(|| "<computed>".to_string());
                out.push(seg);
                cursor = &m.obj;
            }
            Expr::OptChain(opt) => match opt.base.as_ref() {
                OptChainBase::Member(m) => {
                    let seg = member_prop_name(&m.prop).unwrap_or_else(|| "<computed>".to_string());
                    out.push(seg);
                    cursor = &m.obj;
                }
                OptChainBase::Call(_) => {
                    out.push(callee_text_of(cursor));
                    break;
                }
            },
            Expr::Ident(i) => {
                out.push(i.sym.to_string());
                break;
            }
            Expr::This(_) => {
                out.push("this".to_string());
                break;
            }
            other => {
                out.push(callee_text_of(other));
                break;
            }
        }
    }
    out.reverse();
    out
}

/// If `callee` is a `Member` (or OptChain::Member) call, return the
/// `receiver_chain` walked from its `.obj`. Free-fn callees
/// (`Ident`, `This`, `Super`, dynamic `import`, …) return an empty
/// vec — there's no receiver concept.
fn receiver_chain_for_callee(callee: &Expr) -> Vec<String> {
    match callee {
        Expr::Member(m) => receiver_chain_of(&m.obj),
        Expr::OptChain(opt) => match opt.base.as_ref() {
            OptChainBase::Member(m) => receiver_chain_of(&m.obj),
            OptChainBase::Call(_) => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// Classify a positional arg into a [`RawCallArg`]. String literals get
/// `is_string_literal=true` (with quotes stripped); spread args carry
/// a `...` prefix in `value` and never count as a string literal.
/// Non-literal/non-ident expressions fall back to the source-span
/// snippet — best-effort textual representation for the plugin layer.
fn arg_from_swc(ctx: &TsWalkContext<'_>, arg: &ExprOrSpread) -> RawCallArg {
    let is_spread = arg.spread.is_some();
    let prefix = if is_spread { "..." } else { "" };
    if !is_spread && let Expr::Lit(Lit::Str(s)) = arg.expr.as_ref() {
        return RawCallArg {
            value: s.value.to_string_lossy().into_owned(),
            is_string_literal: true,
        };
    }
    if let Expr::Ident(i) = arg.expr.as_ref() {
        return RawCallArg {
            value: format!("{prefix}{}", i.sym),
            is_string_literal: false,
        };
    }
    let snippet = ctx
        .span_snippet(arg.expr.span())
        .unwrap_or_else(|| "<expr>".to_string());
    RawCallArg {
        value: format!("{prefix}{snippet}"),
        is_string_literal: false,
    }
}

fn args_from_swc(ctx: &TsWalkContext<'_>, args: &[ExprOrSpread]) -> Vec<RawCallArg> {
    args.iter().map(|a| arg_from_swc(ctx, a)).collect()
}

#[cfg(test)]
mod tests;

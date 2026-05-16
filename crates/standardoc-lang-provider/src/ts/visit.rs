use standardoc_ir::{AliasMutability, EdgeKind, ModuleLookup, RawEdge, ResolvedOrUnresolved};
use swc_core::common::Span;
use swc_core::ecma::ast::{
    ArrowExpr, BlockStmt, BlockStmtOrExpr, CallExpr, Callee, CatchClause, Expr,
    ForInStmt, ForOfStmt, ForStmt, Function, Ident, JSXAttrOrSpread, JSXAttrValue, JSXElement,
    JSXElementChild, JSXElementName, JSXExpr, MemberProp, NewExpr, OptChainBase,
    OptChainExpr, Pat, TsAsExpr, TsEntityName, TsTypeAnn, TsTypeAssertion,
    TsTypeParamInstantiation, TsTypeRef,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::walk::TsWalkContext;
use crate::template::{JS_GLOBALS, TS_BUILTIN_TYPES};

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
/// pattern-match on the two variants to decide between emitting an edge
/// (Target) or skipping (Local for nested-scope bindings without alias).
enum NameResolution {
    /// Either an alias propagation (Some(mutability)) or a module-level
    /// resolution (None) — both carry the canonical target so the caller
    /// can emit `Calls` / `References` / `UsesType` accordingly.
    Target(ResolvedOrUnresolved, Option<AliasMutability>),
    /// Nested-scope local binding (param, let/const/var/fn-expr) without
    /// an alias. Callers skip emission — locals aren't surfaced in the
    /// module graph by design.
    Local,
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
        self.current_scope_idx = self
            .scope_stack
            .pop()
            .unwrap_or(ModuleLookup::ROOT_SCOPE);
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
                return NameResolution::Target(
                    self.ctx.resolve_call(name, &self.current_module),
                    None,
                );
            }
            if let (Some(alias_str), Some(m)) = (res.aliases_to.as_deref(), res.mutability) {
                let target = self.ctx.resolve_call(alias_str, &self.current_module);
                return NameResolution::Target(target, Some(m));
            }
            return NameResolution::Local;
        }
        NameResolution::Target(self.ctx.resolve_call(name, &self.current_module), None)
    }

    fn emit_call(&mut self, to: ResolvedOrUnresolved, span: Span) {
        self.emit_call_inner(to, span, &[]);
    }

    /// Bug B Stage 2 — `Calls` edge emitted through a propagated scope
    /// alias. Tags the edge with `via-alias` / `via-alias-mutable` so
    /// consumers can distinguish direct calls from chained-binding
    /// calls (and know to discount mutable-alias edges).
    fn emit_call_via_alias(
        &mut self,
        to: ResolvedOrUnresolved,
        span: Span,
        mutability: AliasMutability,
    ) {
        self.emit_call_inner(to, span, &[alias_mut_slug(mutability)]);
    }

    fn emit_call_inner(&mut self, to: ResolvedOrUnresolved, span: Span, extra_attrs: &[&str]) {
        let site = self.ctx.span_site(span);
        let confidence = to.default_confidence();
        let attributes = extra_attrs.iter().map(|s| (*s).to_string()).collect();
        self.ctx.push_edge(RawEdge {
            from_fqdn: self.enclosing_fqdn.clone(),
            kind: EdgeKind::Calls,
            to,
            sites: vec![site],
            attributes,
            confidence,
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
        let (to, alias_mut) = match self.resolve_name(name) {
            NameResolution::Local => (self.ctx.resolve_call(name, &self.current_module), None),
            NameResolution::Target(t, m) => (t, m),
        };
        let confidence = to.default_confidence();
        let site = self.ctx.span_site(span);
        let mut attributes = vec![attribute.to_string()];
        if let Some(m) = alias_mut {
            attributes.push(alias_mut_slug(m).to_string());
        }
        self.ctx.push_edge(RawEdge {
            from_fqdn: self.enclosing_fqdn.clone(),
            kind: EdgeKind::References,
            to,
            sites: vec![site],
            attributes,
            confidence,
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
        let (to, alias_mut) = match self.resolve_name(name) {
            NameResolution::Local => return,
            NameResolution::Target(t, m) => (t, m),
        };
        let target_fqdn = match &to {
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
        let confidence = to.default_confidence();
        let site = self.ctx.span_site(span);
        let mut attributes = vec!["value-read".to_string()];
        if let Some(m) = alias_mut {
            attributes.push(alias_mut_slug(m).to_string());
        }
        self.ctx.push_edge(RawEdge {
            from_fqdn: self.enclosing_fqdn.clone(),
            kind: EdgeKind::References,
            to,
            sites: vec![site],
            attributes,
            confidence,
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
    /// - [`TS_BUILTIN_TYPES`] wrappers (`Array<Foo>`, `Map<K,V>`, …) —
    ///   inner args still recurse and emit their own edges from the
    ///   caller's `visit_ts_type_ref`.
    /// - Scope-local bindings (incl. generic type params bound in the
    ///   enclosing function scope) — no edge for `function f<T>(x: T)`.
    /// - Self-references on `enclosing_fqdn`.
    ///
    /// Unresolved targets ARE emitted (with an extra `unresolved-type`
    /// attribute) so the viz can toggle them on/off without re-extracting.
    fn emit_uses_type(&mut self, name: &str, span: Span) {
        let Some(ctx) = self.type_emission_context else {
            return;
        };
        if TS_BUILTIN_TYPES.contains(&name) {
            return;
        }
        let to = match self.resolve_name(name) {
            NameResolution::Local => return,
            NameResolution::Target(t, _) => t,
        };
        if let ResolvedOrUnresolved::Resolved { fqdn } = &to
            && fqdn == &self.enclosing_fqdn
        {
            return;
        }
        let is_unresolved = matches!(
            &to,
            ResolvedOrUnresolved::Unresolved { .. } | ResolvedOrUnresolved::UnresolvedBridge { .. }
        );
        let confidence = to.default_confidence();
        let site = self.ctx.span_site(span);
        let mut attributes = vec!["via-type".to_string(), ctx.to_string()];
        if is_unresolved {
            attributes.push(TYPE_TAG_UNRESOLVED.to_string());
        }
        self.ctx.push_edge(RawEdge {
            from_fqdn: self.enclosing_fqdn.clone(),
            kind: EdgeKind::UsesType,
            to,
            sites: vec![site],
            attributes,
            confidence,
        });
    }

    /// Push a type-emission context onto `self.type_emission_context`
    /// and return the previous value. Pair with [`restore_type_context`]
    /// to wrap a sub-tree walk. We use a `set/restore` pair rather than
    /// a closure because `Visit`-trait calls take `&mut self` and need
    /// to compose with `node.visit_with(self)` cleanly.
    fn set_type_context(&mut self, ctx: &'static str) -> Option<&'static str> {
        std::mem::replace(&mut self.type_emission_context, Some(ctx))
    }

    fn restore_type_context(&mut self, prev: Option<&'static str>) {
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
                    NameResolution::Local => {}
                    NameResolution::Target(to, Some(m)) => {
                        self.emit_call_via_alias(to, ident.span, m);
                    }
                    NameResolution::Target(to, None) => {
                        self.emit_call(to, ident.span);
                    }
                }
            }
            Expr::Member(member) => {
                if let Some(name) = member_prop_name(&member.prop) {
                    self.emit_call(ResolvedOrUnresolved::Unresolved { name }, member.span);
                }
            }
            Expr::OptChain(opt) => {
                if let OptChainBase::Member(member) = opt.base.as_ref()
                    && let Some(name) = member_prop_name(&member.prop)
                {
                    self.emit_call(ResolvedOrUnresolved::Unresolved { name }, opt.span);
                }
            }
            _ => {}
        }
    }
}

impl Visit for CallVisitor<'_, '_> {
    fn visit_call_expr(&mut self, node: &CallExpr) {
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
            self.handle_callee_expr(call.callee.as_ref());
            self.walk_callee_remainder(call.callee.as_ref());
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use swc_core::common::comments::SingleThreadedComments;
    use swc_core::common::{FileName, SourceMap, sync::Lrc};
    use swc_core::ecma::ast::{EsVersion, Module};
    use swc_core::ecma::parser::{Parser, StringInput, Syntax, TsSyntax, lexer::Lexer};

    fn parse_ts(source: &str) -> (Lrc<SourceMap>, Module, SingleThreadedComments) {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(
            Lrc::new(FileName::Custom("test.ts".into())),
            source.to_string(),
        );
        let comments = SingleThreadedComments::default();
        let lexer = Lexer::new(
            Syntax::Typescript(TsSyntax {
                tsx: false,
                decorators: false,
                dts: false,
                no_early_errors: true,
                disallow_ambiguous_jsx_like: false,
            }),
            EsVersion::EsNext,
            StringInput::from(&*fm),
            Some(&comments),
        );
        let mut parser = Parser::new_from(lexer);
        let module = parser.parse_module().expect("parse ok");
        (cm, module, comments)
    }

    fn parse_tsx(source: &str) -> (Lrc<SourceMap>, Module, SingleThreadedComments) {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(
            Lrc::new(FileName::Custom("test.tsx".into())),
            source.to_string(),
        );
        let comments = SingleThreadedComments::default();
        let lexer = Lexer::new(
            Syntax::Typescript(TsSyntax {
                tsx: true,
                decorators: false,
                dts: false,
                no_early_errors: true,
                disallow_ambiguous_jsx_like: false,
            }),
            EsVersion::EsNext,
            StringInput::from(&*fm),
            Some(&comments),
        );
        let mut parser = Parser::new_from(lexer);
        let module = parser.parse_module().expect("parse ok");
        (cm, module, comments)
    }

    fn run(source: &str) -> (Vec<standardoc_ir::RawSymbol>, Vec<RawEdge>) {
        let (cm, module, comments) = parse_ts(source);
        let (symbols, edges, _) = super::super::walk::walk(
            &module,
            "@app",
            "src/index.ts",
            "src",
            cm,
            &PathBuf::from("/tmp/pkg/src/index.ts"),
            &PathBuf::from("/tmp/pkg"),
            None,
            &comments,
        );
        (symbols, edges)
    }

    fn run_tsx(source: &str) -> (Vec<standardoc_ir::RawSymbol>, Vec<RawEdge>) {
        let (cm, module, comments) = parse_tsx(source);
        let (symbols, edges, _) = super::super::walk::walk(
            &module,
            "@app",
            "src/App.tsx",
            "src/App",
            cm,
            &PathBuf::from("/tmp/pkg/src/App.tsx"),
            &PathBuf::from("/tmp/pkg"),
            None,
            &comments,
        );
        (symbols, edges)
    }

    fn calls(edges: &[RawEdge]) -> Vec<&RawEdge> {
        edges.iter().filter(|e| e.kind == EdgeKind::Calls).collect()
    }

    fn references(edges: &[RawEdge]) -> Vec<&RawEdge> {
        edges
            .iter()
            .filter(|e| e.kind == EdgeKind::References)
            .collect()
    }

    #[test]
    fn simple_function_call_is_resolved_against_defined_fqdn() {
        let (_, edges) = run("function bar() {} function caller() { bar(); }");
        let cs = calls(&edges);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].from_fqdn, "src::caller");
        match &cs[0].to {
            ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "src::bar"),
            other => panic!("expected resolved, got {other:?}"),
        }
    }

    #[test]
    fn unknown_call_is_unresolved_module_local() {
        let (_, edges) = run("function caller() { unknown(); }");
        let cs = calls(&edges);
        assert_eq!(cs.len(), 1);
        match &cs[0].to {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "src::unknown"),
            other => panic!("expected unresolved, got {other:?}"),
        }
    }

    #[test]
    fn member_call_is_unresolved_with_method_ident() {
        let (_, edges) = run("function caller() { obj.run(); }");
        let cs = calls(&edges);
        assert_eq!(cs.len(), 1);
        match &cs[0].to {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "run"),
            other => panic!("expected unresolved method ident, got {other:?}"),
        }
    }

    #[test]
    fn nested_calls_in_arguments_are_captured() {
        let (_, edges) = run("function a() {} function b() {} function caller() { a(); b(); }");
        let cs = calls(&edges);
        assert_eq!(cs.len(), 2);
    }

    #[test]
    fn class_method_body_calls_attributed_to_method_fqdn() {
        let (_, edges) = run("function helper() {} class Foo { run(): void { helper(); } }");
        let cs = calls(&edges);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].from_fqdn, "src::Foo::run");
    }

    #[test]
    fn new_expr_emits_calls_edge() {
        let (_, edges) = run("class Foo {} function caller() { new Foo(); }");
        let cs = calls(&edges);
        assert_eq!(cs.len(), 1);
        match &cs[0].to {
            ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "src::Foo"),
            other => panic!("expected resolved Foo, got {other:?}"),
        }
    }

    #[test]
    fn arrow_const_body_calls_attributed_to_var_fqdn() {
        let (_, edges) = run("function helper() {} const run = () => { helper(); };");
        let cs = calls(&edges);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].from_fqdn, "src::run");
    }

    #[test]
    fn optional_chain_call_is_unresolved_method() {
        let (_, edges) = run("function caller() { obj?.run(); }");
        let cs = calls(&edges);
        assert_eq!(cs.len(), 1);
        match &cs[0].to {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "run"),
            other => panic!("expected unresolved method ident, got {other:?}"),
        }
    }

    #[test]
    fn alias_resolves_to_canonical_via_import_table() {
        let (_, edges) = run("import { foo } from './helper'; function caller() { foo(); }");
        let cs = calls(&edges);
        assert_eq!(cs.len(), 1);
        match &cs[0].to {
            ResolvedOrUnresolved::Unresolved { name } => {
                assert_eq!(name, "@app::src::helper::foo");
            }
            other => panic!("expected unresolved canonical via alias, got {other:?}"),
        }
    }

    // --- JSX template-extraction tests -----------------------------------

    fn refs_with_attribute<'a>(edges: &'a [RawEdge], attr: &str) -> Vec<&'a RawEdge> {
        edges
            .iter()
            .filter(|e| e.kind == EdgeKind::References && e.attributes.iter().any(|a| a == attr))
            .collect()
    }

    fn ref_target_name(edge: &RawEdge) -> &str {
        match &edge.to {
            ResolvedOrUnresolved::Resolved { fqdn } => fqdn.as_str(),
            ResolvedOrUnresolved::Unresolved { name }
            | ResolvedOrUnresolved::UnresolvedBridge { name, .. } => name.as_str(),
        }
    }

    #[test]
    fn jsx_uppercase_tag_emits_component_ref() {
        let (_, edges) = run_tsx("function App() { return <Header />; }");
        let comp = refs_with_attribute(&edges, "template-component-ref");
        assert_eq!(comp.len(), 1);
        assert!(ref_target_name(comp[0]).ends_with("Header"));
    }

    #[test]
    fn jsx_lowercase_tag_does_not_emit_component_ref() {
        let (_, edges) = run_tsx("function App() { return <div />; }");
        let comp = refs_with_attribute(&edges, "template-component-ref");
        assert!(comp.is_empty());
    }

    #[test]
    fn jsx_attribute_expression_emits_template_bind() {
        let (_, edges) = run_tsx("function App() { return <input value={text} />; }");
        let bind = refs_with_attribute(&edges, "template-bind");
        let names: Vec<&str> = bind.iter().map(|e| ref_target_name(e)).collect();
        assert!(names.iter().any(|n| n.ends_with("text")));
    }

    #[test]
    fn jsx_child_expression_emits_template_interpolation() {
        let (_, edges) = run_tsx("function App() { return <p>{message}</p>; }");
        let interp = refs_with_attribute(&edges, "template-interpolation");
        let names: Vec<&str> = interp.iter().map(|e| ref_target_name(e)).collect();
        assert!(names.iter().any(|n| n.ends_with("message")));
    }

    #[test]
    fn jsx_spread_attribute_emits_template_bind() {
        let (_, edges) = run_tsx("function App(props: any) { return <input {...props} />; }");
        let bind = refs_with_attribute(&edges, "template-bind");
        let names: Vec<&str> = bind.iter().map(|e| ref_target_name(e)).collect();
        assert!(names.iter().any(|n| n.ends_with("props")));
    }

    #[test]
    fn jsx_event_handler_call_inside_attribute_also_emits_calls_edge() {
        let (_, edges) =
            run_tsx("function App() { return <button onClick={() => handle(payload)} />; }");
        // Both the handle CALL and the handle/payload REFERENCES (Bind).
        let cs = calls(&edges);
        let bind = refs_with_attribute(&edges, "template-bind");
        let call_names: Vec<&str> = cs.iter().map(|e| ref_target_name(e)).collect();
        let bind_names: Vec<&str> = bind.iter().map(|e| ref_target_name(e)).collect();
        assert!(call_names.iter().any(|n| n.ends_with("handle")));
        assert!(bind_names.iter().any(|n| n.ends_with("handle")));
        assert!(bind_names.iter().any(|n| n.ends_with("payload")));
    }

    #[test]
    fn jsx_member_access_in_child_emits_root_only() {
        let (_, edges) = run_tsx("function App() { return <p>{user.name}</p>; }");
        let interp = refs_with_attribute(&edges, "template-interpolation");
        let names: Vec<&str> = interp.iter().map(|e| ref_target_name(e)).collect();
        assert!(names.iter().any(|n| n.ends_with("user")));
        assert!(!names.iter().any(|n| n.ends_with("name")));
    }

    #[test]
    fn jsx_static_string_attribute_does_not_emit_ref() {
        let (_, edges) = run_tsx(r#"function App() { return <div className="static" />; }"#);
        let bind = refs_with_attribute(&edges, "template-bind");
        assert!(bind.is_empty());
    }

    #[test]
    fn jsx_nested_components_both_emit_component_ref() {
        let (_, edges) = run_tsx("function App() { return <Layout><Header /></Layout>; }");
        let comp = refs_with_attribute(&edges, "template-component-ref");
        let names: Vec<&str> = comp.iter().map(|e| ref_target_name(e)).collect();
        assert!(names.iter().any(|n| n.ends_with("Layout")));
        assert!(names.iter().any(|n| n.ends_with("Header")));
    }

    #[test]
    fn jsx_globals_filtered_in_interpolation() {
        let (_, edges) = run_tsx("function App() { return <p>{Math.max(a, b)}</p>; }");
        let interp = refs_with_attribute(&edges, "template-interpolation");
        let names: Vec<&str> = interp.iter().map(|e| ref_target_name(e)).collect();
        assert!(!names.iter().any(|n| n.ends_with("Math")));
        assert!(names.iter().any(|n| n.ends_with('a')));
        assert!(names.iter().any(|n| n.ends_with('b')));
    }

    #[test]
    fn no_jsx_means_no_template_refs() {
        // Plain TS without JSX should produce zero REFERENCES edges from
        // the visitor (existing call-emission behavior preserved).
        let (_, edges) = run("function caller() { unknown(); }");
        assert!(references(&edges).is_empty());
    }

    // --- Bug B Stage 2 tests: scope tracking + alias propagation ---

    fn refs_with_all_attrs<'a>(edges: &'a [RawEdge], attrs: &[&str]) -> Vec<&'a RawEdge> {
        edges
            .iter()
            .filter(|e| {
                e.kind == EdgeKind::References
                    && attrs.iter().all(|a| e.attributes.iter().any(|x| x == a))
            })
            .collect()
    }

    fn calls_with_attr<'a>(edges: &'a [RawEdge], attr: &str) -> Vec<&'a RawEdge> {
        edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Calls && e.attributes.iter().any(|a| a == attr))
            .collect()
    }

    #[test]
    fn stage2_const_alias_propagates_to_aliased_value_read() {
        let (_, edges) = run(
            "function FOO() {} function takesArg(x) {} \
             function f() { const x = FOO; takesArg(x); }",
        );
        let refs = refs_with_all_attrs(&edges, &["value-read", "via-alias"]);
        let targets: Vec<&str> = refs
            .iter()
            .filter_map(|e| match &e.to {
                ResolvedOrUnresolved::Resolved { fqdn } => Some(fqdn.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            targets.contains(&"src::FOO"),
            "expected via-alias value-read to src::FOO, got {targets:?}",
        );
    }

    #[test]
    fn stage2_let_alias_tagged_via_alias_mutable() {
        let (_, edges) = run(
            "function FOO() {} function takesArg(x) {} \
             function f() { let x = FOO; takesArg(x); }",
        );
        let refs = refs_with_all_attrs(&edges, &["value-read", "via-alias-mutable"]);
        let targets: Vec<&str> = refs
            .iter()
            .filter_map(|e| match &e.to {
                ResolvedOrUnresolved::Resolved { fqdn } => Some(fqdn.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            targets.contains(&"src::FOO"),
            "expected via-alias-mutable value-read to src::FOO, got {targets:?}",
        );
    }

    #[test]
    fn stage2_aliased_call_carries_via_alias_attribute() {
        // Imported FOO → alias `fn` → `fn()` emits Calls{Unresolved{canonical}}
        // tagged `via-alias`.
        let (_, edges) = run(
            "import { FOO } from './m'; function f() { const fn = FOO; fn(); }",
        );
        let cs = calls_with_attr(&edges, "via-alias");
        let names: Vec<&str> = cs
            .iter()
            .filter_map(|e| match &e.to {
                ResolvedOrUnresolved::Unresolved { name } => Some(name.as_str()),
                ResolvedOrUnresolved::Resolved { fqdn } => Some(fqdn.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            names.iter().any(|n| n.ends_with("::m::FOO")),
            "expected via-alias call pointing at imported FOO, got {names:?}",
        );
    }

    #[test]
    fn stage2_local_function_decl_shadows_import_and_suppresses_call() {
        // Inner `function FOO() {}` shadows the import; inner FOO()
        // is a local call → no Calls edge to the imported symbol.
        let (_, edges) = run(
            "import { FOO } from './m'; function f() { function FOO() {} FOO(); }",
        );
        let names: Vec<&str> = calls(&edges)
            .iter()
            .filter_map(|e| match &e.to {
                ResolvedOrUnresolved::Unresolved { name } => Some(name.as_str()),
                ResolvedOrUnresolved::Resolved { fqdn } => Some(fqdn.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            !names.iter().any(|n| n.ends_with("::m::FOO")),
            "expected no call edge to imported FOO, got {names:?}",
        );
    }

    #[test]
    fn stage2_function_param_read_emits_no_reference() {
        let (_, edges) = run("function consume(a) {} function f(arg) { consume(arg); }");
        // The arg → consume call carries arg as an argument. Stage 1
        // would emit a value-read on `arg` (Unresolved → skip filtered).
        // Stage 2's scope binding skips emission too — but with the
        // explicit Local sentinel, no spurious value-read appears.
        let refs = refs_with_all_attrs(&edges, &["value-read"]);
        let on_arg: Vec<&&RawEdge> = refs
            .iter()
            .filter(|e| match &e.to {
                ResolvedOrUnresolved::Resolved { fqdn } => fqdn.ends_with("::arg"),
                ResolvedOrUnresolved::Unresolved { name }
                | ResolvedOrUnresolved::UnresolvedBridge { name, .. } => name.ends_with("::arg"),
            })
            .collect();
        assert!(
            on_arg.is_empty(),
            "expected no value-read on local param `arg`, got {on_arg:?}",
        );
    }

    #[test]
    fn stage2_hoisted_inner_function_call_is_local_no_edge() {
        // `function outer() { inner(); function inner() {} }` — the
        // forward call to `inner` resolves to the hoisted local fn
        // and emits NO module-level edge.
        let (_, edges) = run("function outer() { inner(); function inner() {} }");
        let names: Vec<&str> = calls(&edges)
            .iter()
            .filter_map(|e| match &e.to {
                ResolvedOrUnresolved::Resolved { fqdn } => Some(fqdn.as_str()),
                ResolvedOrUnresolved::Unresolved { name } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            !names.iter().any(|n| n.ends_with("::inner")),
            "expected no call edge for hoisted local `inner`, got {names:?}",
        );
    }

    #[test]
    fn stage2_chained_alias_resolves_through_scope() {
        // const x = FOO; const y = x; takesArg(y)
        //  → y's alias chains through x to src::FOO.
        let (_, edges) = run(
            "function FOO() {} function takesArg(z) {} \
             function f() { const x = FOO; const y = x; takesArg(y); }",
        );
        let refs = refs_with_all_attrs(&edges, &["value-read", "via-alias"]);
        let foo_refs: usize = refs
            .iter()
            .filter(|e| match &e.to {
                ResolvedOrUnresolved::Resolved { fqdn } => fqdn == "src::FOO",
                _ => false,
            })
            .count();
        assert!(
            foo_refs >= 1,
            "expected at least one chained via-alias value-read to src::FOO, got {refs:?}",
        );
    }

    #[test]
    fn stage2_member_chain_alias_resolves_leftmost_base() {
        // `const ns = FOO; consume(ns)` — alias seeded on FOO's
        // resolution. (`FOO.bar.baz` would chain through the same
        // leftmost-base extraction in `resolve_alias_rhs`.)
        let (_, edges) = run(
            "function FOO() {} function consume(z) {} \
             function f() { const ns = FOO; consume(ns); }",
        );
        let refs = refs_with_all_attrs(&edges, &["value-read", "via-alias"]);
        assert!(
            refs.iter().any(|e| matches!(
                &e.to,
                ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "src::FOO"
            )),
            "expected via-alias edge to src::FOO, got {refs:?}",
        );
    }

    #[test]
    fn stage2_block_scope_alias_does_not_leak_outside_block() {
        // `if` block introduces a fresh scope. Alias seeded inside is
        // invisible outside — `consume(x)` in the outer scope should
        // NOT carry a via-alias attribute on the param-named `x`.
        let (_, edges) = run(
            "function FOO() {} function consume(z) {} \
             function f(x) { if (true) { const x = FOO; } consume(x); }",
        );
        // The outer `x` is a function param → local → no edge.
        // The inner `const x = FOO` shadows it inside the if-block,
        // but that scope is popped before consume(x).
        let on_foo = refs_with_all_attrs(&edges, &["value-read", "via-alias"]);
        let leaked: Vec<_> = on_foo
            .iter()
            .filter(|e| matches!(
                &e.to,
                ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "src::FOO"
            ))
            .collect();
        assert!(
            leaked.is_empty(),
            "alias leaked out of if-block scope: {leaked:?}",
        );
    }

    #[test]
    fn stage2_call_via_resolved_alias_keeps_via_alias_attribute() {
        // const helper = ACTUAL; helper();
        //   → Calls{Resolved{src::ACTUAL}} tagged via-alias.
        let (_, edges) = run(
            "function ACTUAL() {} function f() { const helper = ACTUAL; helper(); }",
        );
        let cs = calls_with_attr(&edges, "via-alias");
        let resolved_targets: Vec<&str> = cs
            .iter()
            .filter_map(|e| match &e.to {
                ResolvedOrUnresolved::Resolved { fqdn } => Some(fqdn.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            resolved_targets.contains(&"src::ACTUAL"),
            "expected via-alias call to resolved src::ACTUAL, got {resolved_targets:?}",
        );
    }

    // --- Bug B Stage 2b tests: UsesType edges ---

    fn uses_type_edges(edges: &[RawEdge]) -> Vec<&RawEdge> {
        edges
            .iter()
            .filter(|e| e.kind == EdgeKind::UsesType)
            .collect()
    }

    fn uses_type_with_attrs<'a>(edges: &'a [RawEdge], attrs: &[&str]) -> Vec<&'a RawEdge> {
        edges
            .iter()
            .filter(|e| {
                e.kind == EdgeKind::UsesType
                    && attrs.iter().all(|a| e.attributes.iter().any(|x| x == a))
            })
            .collect()
    }

    fn resolved_fqdns(edges: &[&RawEdge]) -> Vec<String> {
        edges
            .iter()
            .filter_map(|e| match &e.to {
                ResolvedOrUnresolved::Resolved { fqdn } => Some(fqdn.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn stage2b_function_param_type_annotation_emits_uses_type() {
        let (_, edges) = run("interface Foo {} function f(x: Foo) { return x; }");
        let refs = uses_type_with_attrs(&edges, &["via-type", "type-annotation"]);
        let targets = resolved_fqdns(&refs);
        assert!(
            targets.contains(&"src::Foo".to_string()),
            "expected via-type/type-annotation edge to src::Foo, got {targets:?}",
        );
    }

    #[test]
    fn stage2b_function_return_type_annotation_emits_uses_type() {
        let (_, edges) = run("interface Bar {} function f(): Bar { return null as any; }");
        let refs = uses_type_with_attrs(&edges, &["via-type", "type-annotation"]);
        let targets = resolved_fqdns(&refs);
        assert!(
            targets.contains(&"src::Bar".to_string()),
            "expected via-type/type-annotation edge to src::Bar, got {targets:?}",
        );
    }

    #[test]
    fn stage2b_var_type_annotation_emits_uses_type() {
        let (_, edges) = run("interface Foo {} const x: Foo = null as any;");
        let refs = uses_type_with_attrs(&edges, &["via-type", "type-annotation"]);
        let foo_edges: Vec<_> = refs
            .iter()
            .filter(|e| matches!(&e.to, ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "src::Foo"))
            .collect();
        assert!(
            !foo_edges.is_empty(),
            "expected via-type edge on src::Foo from var annotation, got {refs:?}",
        );
        // The edge should originate from `src::x`, not the module.
        assert!(
            foo_edges.iter().any(|e| e.from_fqdn == "src::x"),
            "expected edge from src::x, got {foo_edges:?}",
        );
    }

    #[test]
    fn stage2b_type_alias_body_emits_uses_type() {
        let (_, edges) = run("interface A {} interface B {} type X = A | B;");
        let refs = uses_type_with_attrs(&edges, &["via-type", "type-alias-body"]);
        let targets = resolved_fqdns(&refs);
        assert!(
            targets.contains(&"src::A".to_string()) && targets.contains(&"src::B".to_string()),
            "expected via-type/type-alias-body edges to A and B, got {targets:?}",
        );
        // From-fqdn should be the alias.
        assert!(
            refs.iter().all(|e| e.from_fqdn == "src::X"),
            "expected all alias-body edges originating from src::X, got {refs:?}",
        );
    }

    #[test]
    fn stage2b_interface_member_type_emits_uses_type() {
        // Bug C-1: interface members are now per-member sub-symbols
        // (`src::Bar::x`), so the `via-type/type-interface-member`
        // edge originates from the member fqdn, not the interface.
        let (_, edges) = run("interface Foo {} interface Bar { x: Foo }");
        let refs = uses_type_with_attrs(&edges, &["via-type", "type-interface-member"]);
        let targets = resolved_fqdns(&refs);
        assert!(
            targets.contains(&"src::Foo".to_string()),
            "expected via-type/type-interface-member edge to src::Foo, got {targets:?}",
        );
        assert!(
            refs.iter().any(|e| e.from_fqdn == "src::Bar::x"),
            "expected edge from member src::Bar::x, got {refs:?}",
        );
    }

    #[test]
    fn stage2b_class_prop_type_emits_uses_type() {
        let (_, edges) = run("interface Foo {} class Bar { field: Foo; }");
        let refs = uses_type_with_attrs(&edges, &["via-type", "type-class-prop"]);
        let targets = resolved_fqdns(&refs);
        assert!(
            targets.contains(&"src::Foo".to_string()),
            "expected via-type/type-class-prop edge to src::Foo, got {targets:?}",
        );
        assert!(
            refs.iter().any(|e| e.from_fqdn == "src::Bar::field"),
            "expected edge from src::Bar::field, got {refs:?}",
        );
    }

    #[test]
    fn stage2b_generic_constraint_emits_uses_type() {
        let (_, edges) = run("interface Foo {} function f<T extends Foo>(x: T) { return x; }");
        let refs = uses_type_with_attrs(&edges, &["via-type", "type-constraint"]);
        let targets = resolved_fqdns(&refs);
        assert!(
            targets.contains(&"src::Foo".to_string()),
            "expected via-type/type-constraint edge to src::Foo, got {targets:?}",
        );
    }

    #[test]
    fn stage2b_generic_param_does_not_leak_as_uses_type() {
        // `<T>` is bound as a scope-local; `x: T` resolves Local → no edge.
        let (_, edges) = run("function f<T>(x: T) { return x; }");
        let refs = uses_type_edges(&edges);
        let on_t: Vec<_> = refs
            .iter()
            .filter(|e| match &e.to {
                ResolvedOrUnresolved::Resolved { fqdn } => fqdn.ends_with("::T"),
                ResolvedOrUnresolved::Unresolved { name }
                | ResolvedOrUnresolved::UnresolvedBridge { name, .. } => name.ends_with("::T"),
            })
            .collect();
        assert!(
            on_t.is_empty(),
            "expected no UsesType edge on generic param T, got {on_t:?}",
        );
    }

    #[test]
    fn stage2b_type_cast_emits_uses_type_with_type_cast_attr() {
        let (_, edges) = run("interface Foo {} function f(x: any) { return x as Foo; }");
        let refs = uses_type_with_attrs(&edges, &["via-type", "type-cast"]);
        let targets = resolved_fqdns(&refs);
        assert!(
            targets.contains(&"src::Foo".to_string()),
            "expected via-type/type-cast edge to src::Foo, got {targets:?}",
        );
    }

    #[test]
    fn stage2b_generic_instantiation_emits_uses_type_with_instantiation_attr() {
        let (_, edges) = run(
            "interface Foo {} function generic<T>(): T { return null as any; } \
             function f() { return generic<Foo>(); }",
        );
        let refs = uses_type_with_attrs(&edges, &["via-type", "type-instantiation"]);
        let targets = resolved_fqdns(&refs);
        assert!(
            targets.contains(&"src::Foo".to_string()),
            "expected via-type/type-instantiation edge to src::Foo, got {targets:?}",
        );
    }

    #[test]
    fn stage2b_ts_builtin_wrapper_filtered_inner_args_still_emit() {
        // `Map<Foo, Bar>` — outer Map filtered, inner Foo + Bar emit.
        let (_, edges) = run(
            "interface Foo {} interface Bar {} \
             function f(): Map<Foo, Bar> { return new Map(); }",
        );
        let refs = uses_type_with_attrs(&edges, &["via-type"]);
        let targets = resolved_fqdns(&refs);
        assert!(
            !targets.contains(&"src::Map".to_string()),
            "expected Map filtered as builtin, got {targets:?}",
        );
        assert!(
            targets.contains(&"src::Foo".to_string()),
            "expected via-type edge to src::Foo, got {targets:?}",
        );
        assert!(
            targets.contains(&"src::Bar".to_string()),
            "expected via-type edge to src::Bar, got {targets:?}",
        );
    }

    #[test]
    fn stage2b_class_extends_generic_args_emit_uses_type() {
        let (_, edges) = run(
            "interface Foo {} class Base<T> {} class Derived extends Base<Foo> {}",
        );
        let refs = uses_type_with_attrs(&edges, &["via-type", "type-extends"]);
        let targets = resolved_fqdns(&refs);
        assert!(
            targets.contains(&"src::Foo".to_string()),
            "expected via-type/type-extends edge to src::Foo, got {targets:?}",
        );
    }

    #[test]
    fn stage2b_class_implements_generic_args_emit_uses_type() {
        let (_, edges) = run(
            "interface Foo {} interface Iface<T> {} class C implements Iface<Foo> {}",
        );
        let refs = uses_type_with_attrs(&edges, &["via-type", "type-implements"]);
        let targets = resolved_fqdns(&refs);
        assert!(
            targets.contains(&"src::Foo".to_string()),
            "expected via-type/type-implements edge to src::Foo, got {targets:?}",
        );
    }

    #[test]
    fn stage2b_unresolved_type_carries_unresolved_type_attribute() {
        // `BareUnknown` is not defined locally or imported → Unresolved.
        // Stage 2b emits anyway with `unresolved-type` attribute (debug
        // toggleable in viz).
        let (_, edges) = run("function f(x: BareUnknown) { return x; }");
        let refs = uses_type_with_attrs(&edges, &["via-type", "unresolved-type"]);
        assert!(
            !refs.is_empty(),
            "expected at least one unresolved-type edge, got none",
        );
        // The target should be the canonical Unresolved form.
        let unresolved_names: Vec<&str> = refs
            .iter()
            .filter_map(|e| match &e.to {
                ResolvedOrUnresolved::Unresolved { name } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            unresolved_names.iter().any(|n| n.ends_with("::BareUnknown")),
            "expected unresolved target ending in ::BareUnknown, got {unresolved_names:?}",
        );
    }

    #[test]
    fn stage2b_local_var_typed_with_local_class_emits_uses_type() {
        // const x: LocalClass = new LocalClass() — `LocalClass` is a
        // top-level class, so the var's type-annotation resolves
        // cleanly to its fqdn (not a local binding).
        let (_, edges) = run("class LocalClass {} const x: LocalClass = new LocalClass();");
        let refs = uses_type_with_attrs(&edges, &["via-type", "type-annotation"]);
        let targets = resolved_fqdns(&refs);
        assert!(
            targets.contains(&"src::LocalClass".to_string()),
            "expected via-type edge to src::LocalClass, got {targets:?}",
        );
    }

    #[test]
    fn stage2b_qualified_type_name_uses_leftmost_base() {
        // `Foo.Bar` in type position → emit on Foo (the leftmost base).
        let (_, edges) = run("interface Foo {} function f(): Foo.Bar { return null as any; }");
        let refs = uses_type_with_attrs(&edges, &["via-type", "type-annotation"]);
        let targets = resolved_fqdns(&refs);
        assert!(
            targets.contains(&"src::Foo".to_string()),
            "expected via-type edge to leftmost src::Foo, got {targets:?}",
        );
    }

    // --- Bug C-1 tests: TS interface + enum member granularity ---

    fn symbol_fqdns(symbols: &[standardoc_ir::RawSymbol]) -> Vec<&str> {
        symbols.iter().map(|s| s.fqdn.as_str()).collect()
    }

    #[test]
    fn bug_c1_interface_property_is_indexed_as_sub_symbol() {
        let (symbols, _) = run("interface User { id: number; name: string; }");
        let fqdns = symbol_fqdns(&symbols);
        assert!(
            fqdns.contains(&"src::User"),
            "expected User interface in index, got {fqdns:?}",
        );
        assert!(
            fqdns.contains(&"src::User::id"),
            "expected User::id sub-symbol, got {fqdns:?}",
        );
        assert!(
            fqdns.contains(&"src::User::name"),
            "expected User::name sub-symbol, got {fqdns:?}",
        );
        // Sub-symbols must declare their parent via `module`.
        let id_sym = symbols
            .iter()
            .find(|s| s.fqdn == "src::User::id")
            .expect("User::id found");
        assert_eq!(id_sym.module.as_deref(), Some("src::User"));
        assert_eq!(id_sym.language_kind.as_str(), "interface_property");
    }

    #[test]
    fn bug_c1_interface_method_is_indexed_as_sub_symbol() {
        let (symbols, _) = run("interface API { fetch(): void; }");
        let fqdns = symbol_fqdns(&symbols);
        assert!(
            fqdns.contains(&"src::API::fetch"),
            "expected API::fetch sub-symbol, got {fqdns:?}",
        );
        let fetch_sym = symbols
            .iter()
            .find(|s| s.fqdn == "src::API::fetch")
            .unwrap();
        assert_eq!(fetch_sym.language_kind.as_str(), "interface_method");
        assert_eq!(fetch_sym.module.as_deref(), Some("src::API"));
    }

    #[test]
    fn bug_c1_interface_getter_setter_are_indexed() {
        let (symbols, _) = run("interface Box<T> { get value(): T; set value(v: T); }");
        let fqdns = symbol_fqdns(&symbols);
        let getter = symbols
            .iter()
            .find(|s| s.fqdn == "src::Box::value" && s.language_kind.as_str() == "interface_getter")
            .map(|s| s.fqdn.as_str());
        let setter = symbols
            .iter()
            .find(|s| s.fqdn == "src::Box::value" && s.language_kind.as_str() == "interface_setter")
            .map(|s| s.fqdn.as_str());
        assert!(
            getter.is_some() && setter.is_some(),
            "expected both getter+setter for Box::value, got {fqdns:?}",
        );
    }

    #[test]
    fn bug_c1_interface_member_type_edges_originate_from_member_fqdn() {
        let (symbols, edges) = run(
            "interface Foo {} interface Bar {} \
             interface Combo { x: Foo; m(p: Bar): void; }",
        );
        let fqdns = symbol_fqdns(&symbols);
        assert!(fqdns.contains(&"src::Combo::x"));
        assert!(fqdns.contains(&"src::Combo::m"));
        // Per-member edges: x → Foo from `src::Combo::x`.
        let x_edges: Vec<_> = edges
            .iter()
            .filter(|e| e.from_fqdn == "src::Combo::x")
            .collect();
        assert!(
            x_edges.iter().any(|e| matches!(
                &e.to,
                ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "src::Foo"
            )),
            "expected UsesType from Combo::x to Foo, got {x_edges:?}",
        );
        // m → Bar from `src::Combo::m`.
        let m_edges: Vec<_> = edges
            .iter()
            .filter(|e| e.from_fqdn == "src::Combo::m")
            .collect();
        assert!(
            m_edges.iter().any(|e| matches!(
                &e.to,
                ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "src::Bar"
            )),
            "expected UsesType from Combo::m to Bar, got {m_edges:?}",
        );
    }

    #[test]
    fn bug_c1_ts_enum_members_are_indexed_as_sub_symbols() {
        let (symbols, _) = run("enum Status { Active, Inactive, Pending = 5 }");
        let fqdns = symbol_fqdns(&symbols);
        assert!(fqdns.contains(&"src::Status"));
        assert!(fqdns.contains(&"src::Status::Active"));
        assert!(fqdns.contains(&"src::Status::Inactive"));
        assert!(fqdns.contains(&"src::Status::Pending"));
        let active = symbols
            .iter()
            .find(|s| s.fqdn == "src::Status::Active")
            .unwrap();
        assert_eq!(active.language_kind.as_str(), "enum_member");
        assert_eq!(active.module.as_deref(), Some("src::Status"));
    }

    #[test]
    fn bug_c1_interface_computed_key_skipped() {
        // Computed-key members (`[KEY]: T`) can't be assigned a stable
        // FQDN — skipped silently.
        let (symbols, _) = run("const KEY = 'k'; interface Dyn { [KEY]: number; }");
        let fqdns = symbol_fqdns(&symbols);
        assert!(fqdns.contains(&"src::Dyn"));
        // No anonymous sub-symbol pushed.
        let dyn_children: Vec<_> = symbols
            .iter()
            .filter(|s| s.module.as_deref() == Some("src::Dyn"))
            .collect();
        assert!(
            dyn_children.is_empty(),
            "expected no sub-symbols for computed-key interface, got {dyn_children:?}",
        );
    }

    // --- Stage 3c: class/interface-level generics propagate to inner
    // method/member signatures through the lookup's parent-chain walk.
    // Pre-3a-6c, the visitor's hand-maintained scope stack handled the
    // simple cases but the visitor only added type-params it knew about
    // — a class-level `<T>` reachable from a method's signature relied
    // on the lookup pre-pass having seeded both the class scope AND the
    // method scope's parent chain. Stage 3a-6c made resolve_local walk
    // the chain so the propagation is now structural rather than
    // duplicated.

    #[test]
    fn stage3c_class_method_filters_class_level_generic() {
        let (_, edges) = run(
            "class Box<T> { take(x: T): T { return x; } }",
        );
        let refs = uses_type_with_attrs(&edges, &["via-type", "type-annotation"]);
        let leaked: Vec<&RawEdge> = refs
            .iter()
            .copied()
            .filter(|e| {
                match &e.to {
                    ResolvedOrUnresolved::Resolved { fqdn } => fqdn.ends_with("::T"),
                    ResolvedOrUnresolved::Unresolved { name } => name.ends_with("::T"),
                    _ => false,
                }
            })
            .collect();
        assert!(
            leaked.is_empty(),
            "class-level T leaked into Box's method signature: {leaked:?}",
        );
    }

    #[test]
    fn stage3c_class_method_inner_generic_combined_with_outer() {
        let (_, edges) = run(
            "class Box<T> { take<U>(x: T, y: U): T { return x; } }",
        );
        let refs = uses_type_with_attrs(&edges, &["via-type", "type-annotation"]);
        let leaked: Vec<String> = refs
            .iter()
            .filter_map(|e| match &e.to {
                ResolvedOrUnresolved::Resolved { fqdn }
                    if fqdn.ends_with("::T") || fqdn.ends_with("::U") =>
                {
                    Some(fqdn.clone())
                }
                ResolvedOrUnresolved::Unresolved { name }
                    if name.ends_with("::T") || name.ends_with("::U") =>
                {
                    Some(name.clone())
                }
                _ => None,
            })
            .collect();
        assert!(
            leaked.is_empty(),
            "neither outer T nor inner U should leak: got {leaked:?}",
        );
    }

    #[test]
    fn stage3c_interface_method_inner_generic_combined_with_outer() {
        let (_, edges) = run(
            "interface Box<T> { take<U>(x: T, y: U): T; }",
        );
        let refs = uses_type_with_attrs(
            &edges,
            &["via-type", "type-interface-member"],
        );
        let leaked: Vec<String> = refs
            .iter()
            .filter_map(|e| match &e.to {
                ResolvedOrUnresolved::Resolved { fqdn }
                    if fqdn.ends_with("::T") || fqdn.ends_with("::U") =>
                {
                    Some(fqdn.clone())
                }
                ResolvedOrUnresolved::Unresolved { name }
                    if name.ends_with("::T") || name.ends_with("::U") =>
                {
                    Some(name.clone())
                }
                _ => None,
            })
            .collect();
        assert!(
            leaked.is_empty(),
            "interface-level T + method-level U must both be local: {leaked:?}",
        );
    }
}

use proc_macro2::Span;
use quote::ToTokens;
use standardoc_ir::{
    BuiltinTier, EdgeKind, Kind, Language, RawCallArg, RawCallSite, RawEdge, ResolvedOrUnresolved,
    Site,
};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::visit::Visit;

use crate::builtins::global as global_builtin_registry;

use super::walk::{
    NameResolution, WalkContext, col_from_span, line_from_span, lookup_scope_for, path_to_string,
};

mod local_type_env;
use local_type_env::LocalTypeEnv;

/// IR-4-b: classify a positional argument expression into a [`RawCallArg`].
/// String literals get `is_string_literal = true` with their unwrapped
/// `value()` (no surrounding quotes); identifiers carry their dotted path
/// text; anything else stringifies via `ToTokens` (token-text — a best-
/// effort textual representation of the AST node).
fn arg_from_expr(expr: &syn::Expr) -> RawCallArg {
    if let syn::Expr::Lit(lit) = expr
        && let syn::Lit::Str(s) = &lit.lit
    {
        return RawCallArg {
            value: s.value(),
            is_string_literal: true,
        };
    }
    if let syn::Expr::Path(p) = expr {
        return RawCallArg {
            value: path_to_string(&p.path),
            is_string_literal: false,
        };
    }
    RawCallArg {
        value: expr.to_token_stream().to_string(),
        is_string_literal: false,
    }
}

fn args_from_punctuated(args: &Punctuated<syn::Expr, syn::token::Comma>) -> Vec<RawCallArg> {
    args.iter().map(arg_from_expr).collect()
}

/// IR-4-b: walk a method-call receiver expression and produce the dotted
/// `receiver_chain` segment list in source order. `a.b.c()` (receiver =
/// `a.b`) yields `["a", "b"]`. The walk peels `ExprField` layers; at the
/// inner-most non-field base it pushes a single segment — the path text
/// for a `Path`, the full token text for anything else (computed access,
/// nested calls, etc.).
fn receiver_chain_from(receiver: &syn::Expr) -> Vec<String> {
    let mut chain: Vec<String> = Vec::new();
    let mut cursor: &syn::Expr = receiver;
    loop {
        match cursor {
            syn::Expr::Field(field) => {
                let member = match &field.member {
                    syn::Member::Named(n) => n.to_string(),
                    syn::Member::Unnamed(idx) => idx.index.to_string(),
                };
                chain.push(member);
                cursor = &field.base;
            }
            syn::Expr::Path(p) => {
                chain.push(path_to_string(&p.path));
                break;
            }
            other => {
                chain.push(other.to_token_stream().to_string());
                break;
            }
        }
    }
    chain.reverse();
    chain
}

pub(crate) fn visit_block(
    ctx: &mut WalkContext,
    block: &syn::Block,
    current_module: &str,
    enclosing_fqdn: &str,
    self_type: Option<&str>,
    fn_inputs: &Punctuated<syn::FnArg, syn::Token![,]>,
) {
    let file_path = ctx.core.file_path.clone();
    let initial_scope = lookup_scope_for(ctx, block.span());
    let local_env = LocalTypeEnv::from_fn_params(fn_inputs);
    let mut visitor = CallVisitor {
        ctx,
        current_module: current_module.to_string(),
        enclosing_fqdn: enclosing_fqdn.to_string(),
        self_type: self_type.map(str::to_string),
        file_path,
        current_scope_idx: initial_scope,
        local_env,
    };
    visitor.visit_block(block);
}

struct CallVisitor<'a> {
    ctx: &'a mut WalkContext,
    current_module: String,
    enclosing_fqdn: String,
    /// The concrete `Self` type FQDN when the surrounding block lives
    /// inside an `impl Foo { ... }` body (e.g. `c::Foo`). Used to
    /// substitute `Self::method` paths before resolution so calls like
    /// `Self::new()` resolve to `c::Foo::new` instead of staying as
    /// unresolved `Self::new`. `None` for free functions and trait
    /// default-method bodies (where `Self` is unknown at extract time).
    self_type: Option<String>,
    file_path: String,
    /// Stage 3e-2-bis — current scope_idx into `ctx.core.lookup.scopes`,
    /// maintained as a save/restore stack across `visit_block` and
    /// `visit_expr_closure` overrides. Mirrors `ts::visit::CallVisitor::
    /// current_scope_idx`. The flat `lookup_scope_for(span)` shortcut only
    /// works for scope-creation spans (exact HashMap match) — arbitrary
    /// expression spans inside a scope require this maintained value.
    current_scope_idx: u32,
    /// Bug E-3 Phase 1: binding → nominal type table populated from fn
    /// params (`from_fn_params`) and `let` bindings (`visit_local`). Read
    /// by `visit_expr_method_call` (P1.4) to annotate emitted CALLS edges
    /// with `receiver_type`. Flat per-fn-body — no nested-block scoping.
    local_env: LocalTypeEnv,
}

impl CallVisitor<'_> {
    /// Rewrite paths starting with `Self::` (or bare `Self`) to use the
    /// concrete impl-block type FQDN when one is set. Returns the path
    /// unchanged when no substitution applies — caller can keep its
    /// borrow on the original `&str`.
    fn substitute_self(&self, path: &str) -> Option<String> {
        let self_type = self.self_type.as_deref()?;
        if path == "Self" {
            return Some(self_type.to_string());
        }
        path.strip_prefix("Self::")
            .map(|rest| format!("{self_type}::{rest}"))
    }
}

impl CallVisitor<'_> {
    /// IR-4-b: push an observational [`RawCallSite`] alongside whatever
    /// `Calls` / `References` edge was (or wasn't) emitted. Call sites
    /// carry textual shape only — they're orthogonal to the edge tier
    /// decisions (Drop / Local / Attribute) made by `resolve_name`, so
    /// e.g. `Vec::new()` still surfaces as a `RawCallSite` even though
    /// the Drop tier suppresses the graph edge.
    fn emit_call_site(
        &mut self,
        callee_text: String,
        args: Vec<RawCallArg>,
        receiver_chain: Vec<String>,
        span: Span,
    ) {
        self.ctx.push_call_site(RawCallSite {
            from_fqdn: self.enclosing_fqdn.clone(),
            callee_text,
            args,
            receiver_chain,
            site: Site {
                file: self.file_path.clone(),
                line: line_from_span(span),
                col: col_from_span(span),
            },
        });
    }

    fn emit_call_with_attributes(
        &mut self,
        to: ResolvedOrUnresolved,
        span: Span,
        attributes: Vec<String>,
        receiver_type: Option<String>,
    ) {
        let confidence = to.default_confidence();
        self.ctx.push_edge(RawEdge {
            from_fqdn: self.enclosing_fqdn.clone(),
            kind: EdgeKind::Calls,
            to,
            sites: vec![Site {
                file: self.file_path.clone(),
                line: line_from_span(span),
                col: col_from_span(span),
            }],
            attributes,
            confidence,
            receiver_type,
        });
    }

    /// Bug E-3 Phase 1: derive a nominal receiver type for an
    /// `obj.method(...)` call from the dotted receiver chain. Rules:
    ///
    ///   * `chain = ["self"]` → `self_type` (full FQDN of impl block,
    ///     e.g. `crate::Foo`); the resolver post-pass forms
    ///     `<FQDN>::<method>` directly.
    ///   * `chain = ["self", <field>]` → `struct_fields.lookup(self_type,
    ///     field)` — nominal type of the field.
    ///   * `chain = [<binding>]` → `local_env.lookup(binding)` — nominal
    ///     type inferred from fn params + `let` initializers.
    ///   * Anything deeper (`x.field.method`, chains with parens) →
    ///     `None` (deferred to Phase 3).
    fn compute_receiver_type(&self, chain: &[String]) -> Option<String> {
        match chain {
            [head] if head == "self" => self.self_type.clone(),
            [head] => self.local_env.lookup(head).map(str::to_string),
            [head, field] if head == "self" => {
                let self_fqdn = self.self_type.as_deref()?;
                self.ctx
                    .struct_fields
                    .lookup(self_fqdn, field)
                    .map(str::to_string)
            }
            _ => None,
        }
    }

    /// Stage 3e-2-ter — emit a `Calls` edge for a `path` in call
    /// position. Pipeline mirrors [`Self::emit_value_ref`] but tags the
    /// edge as `EdgeKind::Calls` (no `value-read` attribute). Routed
    /// through [`WalkContext::resolve_name`] so the scope chain is
    /// honored: `let f = bar; f();` propagates the alias to `bar` with
    /// `via-alias`, fn-pointer parameters are skipped as Locals, etc.
    ///
    /// Self-reference is NOT skipped here — `fn foo() { foo(); }` is a
    /// legitimate recursion signal (different from value-position self-
    /// reads which are structural artifacts).
    fn emit_call_via_resolve_name(&mut self, path: &str, span: Span) {
        let substituted = self.substitute_self(path);
        let resolved_path = substituted.as_deref().unwrap_or(path);
        let (target, alias_mut, via_builtin) =
            match self
                .ctx
                .resolve_name(resolved_path, self.current_scope_idx, &self.current_module)
            {
                NameResolution::Drop | NameResolution::Local => return,
                NameResolution::Attribute(tag) => {
                    self.ctx.register_attribute_flag(&self.enclosing_fqdn, &tag);
                    return;
                }
                NameResolution::Target {
                    to,
                    alias_mut,
                    via_builtin,
                } => (to, alias_mut, via_builtin),
            };
        let mut attributes = Vec::new();
        if let Some(m) = alias_mut {
            attributes.push(m.as_slug().to_string());
        }
        if let Some(tag) = &via_builtin {
            attributes.push("via-builtin".to_string());
            attributes.push(format!("builtin-{}", tag.slug()));
        }
        self.emit_call_with_attributes(target, span, attributes, None);
    }

    /// Stage 3e-2 — emit a `References` edge for a `path` read in value
    /// position (`let x = foo;`, `MyType::CONST`, fn-pointer args, etc.).
    /// Pipeline mirrors `ts::visit::CallVisitor::emit_value_ref`:
    ///
    /// 1. [`WalkContext::resolve_name`] resolves the path against the
    ///    scope chain + builtin registry + alias table + defined_fqdns.
    /// 2. `Drop` / `Local` → silent skip (structural noise and locals
    ///    aren't surfaced in the module graph).
    /// 3. `Attribute(tag)` → register the flag on the enclosing FQDN, no
    ///    edge.
    /// 4. `Target { Unresolved | UnresolvedBridge, .. }` → skip (preserve
    ///    the Stage 1 safety net — unresolved value-reads create noise).
    /// 5. Self-references (target FQDN == enclosing FQDN) are dropped:
    ///    a fn reading its own name is already covered by the matching
    ///    `Calls` edge when the name appears in call position; in value
    ///    position it's almost always a structural artefact of the
    ///    enclosing fn-pointer pattern.
    /// 6. Emit `EdgeKind::References` with `attributes = ["value-read"]`
    ///    plus optional `via-alias[-mutable]` (scope-alias propagation)
    ///    and `via-builtin` / `builtin-<slug>` (Edge-tier builtin hit).
    fn emit_value_ref(&mut self, path: &str, span: Span) {
        let substituted = self.substitute_self(path);
        let resolved_path = substituted.as_deref().unwrap_or(path);
        let (target, alias_mut, via_builtin) =
            match self
                .ctx
                .resolve_name(resolved_path, self.current_scope_idx, &self.current_module)
            {
                NameResolution::Drop | NameResolution::Local => return,
                NameResolution::Attribute(tag) => {
                    self.ctx.register_attribute_flag(&self.enclosing_fqdn, &tag);
                    return;
                }
                NameResolution::Target {
                    to,
                    alias_mut,
                    via_builtin,
                } => (to, alias_mut, via_builtin),
            };
        let target_fqdn = match &target {
            ResolvedOrUnresolved::Resolved { fqdn } => fqdn,
            ResolvedOrUnresolved::Unresolved { .. }
            | ResolvedOrUnresolved::UnresolvedBridge { .. } => return,
        };
        if target_fqdn == &self.enclosing_fqdn {
            return;
        }
        let confidence = target.default_confidence();
        let mut attributes = vec!["value-read".to_string()];
        if let Some(m) = alias_mut {
            attributes.push(m.as_slug().to_string());
        }
        if let Some(tag) = &via_builtin {
            attributes.push("via-builtin".to_string());
            attributes.push(format!("builtin-{}", tag.slug()));
        }
        self.ctx.push_edge(RawEdge {
            from_fqdn: self.enclosing_fqdn.clone(),
            kind: EdgeKind::References,
            to: target,
            sites: vec![Site {
                file: self.file_path.clone(),
                line: line_from_span(span),
                col: col_from_span(span),
            }],
            attributes,
            confidence,
            receiver_type: None,
        });
    }
}

impl<'ast> Visit<'ast> for CallVisitor<'_> {
    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let syn::Expr::Path(expr_path) = &*node.func {
            let path_str = path_to_string(&expr_path.path);
            if !path_str.is_empty() {
                // IR-4-b: observational call-site — always emitted,
                // independent of the Drop/Local/Attribute tier dispatch
                // below. The plugin layer reads call_sites to interpret
                // textual call patterns (e.g. `tauri::invoke("foo")`)
                // regardless of whether the graph edge was suppressed.
                self.emit_call_site(
                    path_str.clone(),
                    args_from_punctuated(&node.args),
                    Vec::new(),
                    expr_path.span(),
                );
                // Stage 3e-2-ter: routed through `resolve_name` so the
                // scope chain is honored. Drop/Attribute tier dispatch
                // happens inside `resolve_name`; aliases propagate as
                // `via-alias[-mutable]`; Locals (fn-pointer params,
                // closure-typed lets) are silently skipped (mirror of
                // TS Bug B Stage 2 — calling a local emits no edge).
                self.emit_call_via_resolve_name(&path_str, expr_path.span());
            }
            // Stage 3e-2: we consumed `node.func` as the Calls target
            // above. Recurse only into args — letting the default
            // `visit_expr_call` walk re-visit `node.func` would trigger
            // our `visit_expr_path` override and double-emit a
            // `References` edge on the same path.
            for arg in &node.args {
                syn::visit::visit_expr(self, arg);
            }
            return;
        }
        // Non-path func (e.g. `(get_fn())()`) — surface the call shape
        // for the plugin layer via the full token text, then defer to
        // the default recursion for edge emission (visit_expr_path on
        // any inner identifier triggers value-reads).
        self.emit_call_site(
            node.func.to_token_stream().to_string(),
            args_from_punctuated(&node.args),
            Vec::new(),
            node.span(),
        );
        syn::visit::visit_expr_call(self, node);
    }

    /// Stage 3e-2 — surface `Expr::Path` reads in value position as
    /// `References` edges. Skips paths consumed by `visit_expr_call` as
    /// Calls targets (handled by the manual arg-only recursion there).
    /// Multi-segment paths like `MyType::CONST` are honored — the
    /// leftmost segment goes through the builtin tier check inside
    /// `WalkContext::resolve_name`, the full path through `resolve_path`.
    fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
        let path_str = path_to_string(&node.path);
        if !path_str.is_empty() {
            self.emit_value_ref(&path_str, node.span());
        }
        syn::visit::visit_expr_path(self, node);
    }

    /// Stage 3e-2-bis — keep `current_scope_idx` aligned with the AOT
    /// `ModuleLookup` scope chain. Nested blocks introduce new Block
    /// scopes during the pre-pass; we save/swap/restore so
    /// `emit_value_ref` can resolve locals against the right scope.
    fn visit_block(&mut self, node: &'ast syn::Block) {
        let saved = self.current_scope_idx;
        self.current_scope_idx = lookup_scope_for(self.ctx, node.span());
        syn::visit::visit_block(self, node);
        self.current_scope_idx = saved;
    }

    /// Stage 3e-2-bis — closures introduce a Function scope in the
    /// pre-pass (params bound there). Mirror the save/swap/restore so
    /// captured-via-closure path reads resolve correctly.
    fn visit_expr_closure(&mut self, node: &'ast syn::ExprClosure) {
        let saved = self.current_scope_idx;
        self.current_scope_idx = lookup_scope_for(self.ctx, node.span());
        syn::visit::visit_expr_closure(self, node);
        self.current_scope_idx = saved;
    }

    /// Bug E-3 Phase 1: capture `let x [: T] [= init]` bindings into the
    /// receiver-type env. Flat across nested blocks (no scoping) — Phase 1
    /// accepts the false-positive risk of late shadowing; Phase 3 may
    /// revisit if measured noise warrants it.
    fn visit_local(&mut self, node: &'ast syn::Local) {
        self.local_env.record_local(node);
        syn::visit::visit_local(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        let method = node.method.to_string();
        let span = node.method.span();
        // IR-4-b: observational call-site — `obj.field.foo(x)` yields
        // `callee_text = "<receiver-text>.foo"`, `receiver_chain` from
        // walking down through ExprField layers (final segment is the
        // receiver-text base, the method ident is NOT in the chain).
        let receiver_chain = receiver_chain_from(&node.receiver);
        // Bug E-3 Phase 1: derive the receiver's nominal type from the
        // chain + local_env + struct_fields. Consumed by the resolver
        // post-pass to form `<receiver_type>::<method>`.
        let receiver_type = self.compute_receiver_type(&receiver_chain);
        let callee_text = format!("{}.{}", receiver_chain.join("."), method);
        self.emit_call_site(
            callee_text,
            args_from_punctuated(&node.args),
            receiver_chain,
            span,
        );
        self.emit_call_with_attributes(
            ResolvedOrUnresolved::Unresolved { name: method },
            span,
            vec![],
            receiver_type,
        );
        syn::visit::visit_expr_method_call(self, node);
    }

    /// Walk into async block bodies (S3-N). The default visitor traverses
    /// `node.block`, so calls inside `async { foo(); bar() }` are surfaced via
    /// our `visit_expr_call` override. Edges keep their normal `Calls` kind;
    /// no attribute marker day-1 — the async-ness is a property of the
    /// enclosing context, not the edge.
    fn visit_expr_async(&mut self, node: &'ast syn::ExprAsync) {
        syn::visit::visit_expr_async(self, node);
    }

    /// Emit a `Calls` edge for the macro path (S3-M). Macro **args** stay
    /// opaque (token stream — not parsed as Rust); only the path is captured.
    /// The edge carries `attributes = ["macro"]` so consumers can filter or
    /// up-weight these in the blast-radius view.
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        let path_str = path_to_string(&node.path);
        if path_str.is_empty() {
            return;
        }
        let span = node.path.span();
        // IR-4-b: macro args are an opaque token stream (not parsed as
        // Rust expressions), so we surface the call-site with the
        // `<path>!` callee_text but `args = []`. Plugins that care
        // about macro payload can re-parse the token text from source
        // — the IR layer doesn't try to interpret it.
        self.emit_call_site(format!("{path_str}!"), Vec::new(), Vec::new(), span);
        // Bug E-1: skip the Calls edge for builtin Drop-tier macros
        // (`assert!`, `panic!`, `vec!`, `format!`, `env!`, ...).
        // The call_site row above still records the invocation for
        // consumers that want raw macro counts ; the graph edge would
        // be pure noise. Mirrors the Drop-tier handling in
        // `resolve_name` for type-level builtins.
        let leftmost = path_str.split("::").next().unwrap_or("");
        if let Some(entry) = global_builtin_registry().lookup(leftmost, Language::Rust)
            && entry.kind == Kind::Macro
            && entry.tier == BuiltinTier::Drop
        {
            return;
        }
        let to = self.ctx.resolve_path(&path_str, &self.current_module);
        self.emit_call_with_attributes(to, span, vec!["macro".to_string()], None);
    }
}

#[cfg(test)]
mod tests;

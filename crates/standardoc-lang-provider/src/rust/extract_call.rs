use proc_macro2::Span;
use quote::ToTokens;
use standardoc_ir::{
    BuiltinTier, EdgeKind, Kind, Language, RawCallArg, RawCallSite, RawEdge, ResolvedOrUnresolved,
    Site,
};
use syn::spanned::Spanned;
use syn::visit::Visit;

use crate::builtins::global as global_builtin_registry;

use super::walk::{
    NameResolution, WalkContext, col_from_span, line_from_span, lookup_scope_for, path_to_string,
};

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

fn args_from_punctuated(
    args: &syn::punctuated::Punctuated<syn::Expr, syn::token::Comma>,
) -> Vec<RawCallArg> {
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
) {
    let file_path = ctx.core.file_path.clone();
    let initial_scope = lookup_scope_for(ctx, block.span());
    let mut visitor = CallVisitor {
        ctx,
        current_module: current_module.to_string(),
        enclosing_fqdn: enclosing_fqdn.to_string(),
        self_type: self_type.map(str::to_string),
        file_path,
        current_scope_idx: initial_scope,
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

    fn emit_call(&mut self, to: ResolvedOrUnresolved, span: Span) {
        self.emit_call_with_attributes(to, span, vec![]);
    }

    fn emit_call_with_attributes(
        &mut self,
        to: ResolvedOrUnresolved,
        span: Span,
        attributes: Vec<String>,
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
        });
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
        self.emit_call_with_attributes(target, span, attributes);
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

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        let method = node.method.to_string();
        let span = node.method.span();
        // IR-4-b: observational call-site — `obj.field.foo(x)` yields
        // `callee_text = "<receiver-text>.foo"`, `receiver_chain` from
        // walking down through ExprField layers (final segment is the
        // receiver-text base, the method ident is NOT in the chain).
        let receiver_chain = receiver_chain_from(&node.receiver);
        let callee_text = format!("{}.{}", receiver_chain.join("."), method);
        self.emit_call_site(
            callee_text,
            args_from_punctuated(&node.args),
            receiver_chain,
            span,
        );
        self.emit_call(ResolvedOrUnresolved::Unresolved { name: method }, span);
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
        self.emit_call_with_attributes(to, span, vec!["macro".to_string()]);
    }
}

#[cfg(test)]
#[allow(clippy::case_sensitive_file_extension_comparisons)]
mod tests {
    use super::super::walk::walk;
    use standardoc_ir::{EdgeKind, ResolvedOrUnresolved};

    fn parse(src: &str) -> syn::File {
        syn::parse_file(src).expect("test source not parsable")
    }

    fn calls(edges: &[standardoc_ir::RawEdge]) -> Vec<&standardoc_ir::RawEdge> {
        edges.iter().filter(|e| e.kind == EdgeKind::Calls).collect()
    }

    #[test]
    fn expr_call_local_function_is_resolved_against_defined_fqdn() {
        let parsed = parse("fn bar() {} fn caller() { bar(); }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let cs = calls(&edges);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].from_fqdn, "c::caller");
        match &cs[0].to {
            ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "c::bar"),
            other => panic!("expected resolved, got {other:?}"),
        }
    }

    #[test]
    fn expr_call_unknown_external_is_unresolved_canonical() {
        let parsed = parse("fn caller() { std::mem::take(&mut 0); }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let cs = calls(&edges);
        assert_eq!(cs.len(), 1);
        match &cs[0].to {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "std::mem::take"),
            other => panic!("expected unresolved as-written, got {other:?}"),
        }
    }

    #[test]
    fn expr_call_via_alias_resolves_to_canonical() {
        let parsed = parse("use foo::bar; fn caller() { bar(); }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let cs = calls(&edges);
        assert_eq!(cs.len(), 1);
        match &cs[0].to {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "foo::bar"),
            other => panic!("expected unresolved canonical via alias, got {other:?}"),
        }
    }

    #[test]
    fn expr_method_call_is_always_unresolved_with_method_ident() {
        let parsed = parse("fn caller() { let v = vec![1]; v.iter().count(); }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        // Filter out the `vec!` macro edge — this test is about method calls.
        let cs: Vec<_> = calls(&edges)
            .into_iter()
            .filter(|e| !e.attributes.iter().any(|a| a == "macro"))
            .collect();
        // Two ExprMethodCall: .iter() and .count().
        assert_eq!(cs.len(), 2);
        let names: Vec<_> = cs
            .iter()
            .map(|e| match &e.to {
                ResolvedOrUnresolved::Unresolved { name } => name.clone(),
                _ => panic!("expected unresolved method call"),
            })
            .collect();
        assert!(names.contains(&"iter".to_string()));
        assert!(names.contains(&"count".to_string()));
    }

    #[test]
    fn nested_calls_in_arguments_are_captured() {
        let parsed = parse("fn a() {} fn b() {} fn caller() { a(); b(); }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let cs = calls(&edges);
        assert_eq!(cs.len(), 2);
    }

    #[test]
    fn impl_fn_body_calls_attributed_to_method_fqdn() {
        let parsed = parse("fn helper() {} struct F; impl F { fn run(&self) { helper(); } }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let cs = calls(&edges);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].from_fqdn, "c::F::run");
    }

    #[test]
    fn trait_default_body_calls_attributed_to_trait_fn_fqdn() {
        let parsed = parse("fn helper() {} trait T { fn run(&self) { helper(); } }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let cs = calls(&edges);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].from_fqdn, "c::T::run");
    }

    #[test]
    fn self_method_in_impl_block_resolves_to_target_type() {
        // `impl Foo { fn new() -> Self {...} fn other(&self) { Self::new() } }`
        // — `Self::new` inside `other()` should resolve to `c::Foo::new`,
        // not stay as the bogus `Self::new` text-as-written.
        let parsed = parse(
            "struct Foo; impl Foo { fn new() -> Self { Foo } fn other(&self) { Self::new(); } }",
        );
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let cs = calls(&edges);
        assert_eq!(cs.len(), 1, "exactly one Self::new call expected");
        match &cs[0].to {
            ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "c::Foo::new"),
            other => panic!("expected Resolved c::Foo::new, got {other:?}"),
        }
        assert_eq!(cs[0].from_fqdn, "c::Foo::other");
    }

    #[test]
    fn self_default_in_impl_block_resolves_via_substitution() {
        // `Self::default()` inside an impl Foo block must rewrite to
        // `c::Foo::default` before resolution. Since `default` isn't
        // defined in this file, the edge stays Unresolved BUT with the
        // composed canonical name (no longer the raw `Self::default`).
        let parsed = parse("struct Foo; impl Foo { fn other(&self) { Self::default(); } }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let cs = calls(&edges);
        assert_eq!(cs.len(), 1);
        match &cs[0].to {
            ResolvedOrUnresolved::Unresolved { name } => {
                assert!(
                    name.contains("Foo::default"),
                    "Self was not substituted, got {name:?}"
                );
                assert!(!name.contains("Self::"), "Self leaked, got {name:?}");
            }
            other => panic!("expected Unresolved with Self substituted, got {other:?}"),
        }
    }

    #[test]
    fn self_outside_impl_block_stays_unresolved() {
        // `Self::xxx` inside a free fn or trait default body has no
        // concrete self_type to substitute. The CallVisitor's self_type
        // is None, so substitution is a no-op and the path goes through
        // resolve_name verbatim — Unresolved with the original text.
        let parsed = parse("fn outside() { Self::new(); }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let cs = calls(&edges);
        // resolve_path's module-local fallback prefixes the current
        // module, so the unresolved name becomes `c::Self::new`. The
        // key invariant is that no spurious Foo-substitution happened.
        assert!(
            cs.iter().any(|e| match &e.to {
                ResolvedOrUnresolved::Unresolved { name } => name.contains("Self::new"),
                _ => false,
            }),
            "Self::new outside impl must stay as Self::new in the edge target, got {cs:?}"
        );
    }

    #[test]
    fn async_block_body_is_walked_for_calls() {
        let parsed = parse("fn outside() {} fn caller() { let _ = async { outside(); }; }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let cs = calls(&edges);
        let names: Vec<_> = cs
            .iter()
            .map(|e| match &e.to {
                ResolvedOrUnresolved::Resolved { fqdn } => fqdn.clone(),
                ResolvedOrUnresolved::Unresolved { name }
                | ResolvedOrUnresolved::UnresolvedBridge { name, .. } => name.clone(),
            })
            .collect();
        assert!(
            names.contains(&"c::outside".to_string()),
            "async block must surface inner calls, got {names:?}"
        );
    }

    #[test]
    fn user_macro_invocation_emits_call_edge_with_macro_attribute() {
        // User-defined macro (not in the Drop-tier builtin registry) still
        // emits a Calls edge so plugins can reach the macro definition.
        let parsed = parse("fn caller() { my_user_macro!(\"hi\"); }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let cs = calls(&edges);
        assert_eq!(cs.len(), 1, "exactly one user-macro call expected");
        assert!(
            cs[0].attributes.iter().any(|a| a == "macro"),
            "macro call edge must carry attribute=`macro`, got {:?}",
            cs[0].attributes
        );
        match &cs[0].to {
            ResolvedOrUnresolved::Unresolved { name } => {
                assert!(name.ends_with("my_user_macro"), "macro target = {name:?}");
            }
            other => panic!("expected unresolved macro target, got {other:?}"),
        }
    }

    #[test]
    fn builtin_macro_invocation_drops_calls_edge_keeps_call_site() {
        // Bug E-1: `println!` / `assert!` / `panic!` / `vec!` / etc. are
        // Drop-tier in the builtin registry. The Calls edge is suppressed
        // (graph noise) but the RawCallSite row is still emitted upstream
        // of the registry check so consumers wanting raw macro counts
        // continue to get them.
        let parsed =
            parse("fn caller() { println!(\"hi\"); assert!(true); panic!(\"x\"); vec![1]; }");
        let (_, edges, _, call_sites) = walk(&parsed, "c", "src/lib.rs", "c");
        let macro_edges: Vec<_> = calls(&edges)
            .into_iter()
            .filter(|e| e.attributes.iter().any(|a| a == "macro"))
            .collect();
        assert!(
            macro_edges.is_empty(),
            "builtin Drop-tier macros must not emit Calls edges, got {macro_edges:?}"
        );
        let macro_callees: Vec<_> = call_sites
            .iter()
            .filter(|c| c.callee_text.ends_with('!'))
            .map(|c| c.callee_text.as_str())
            .collect();
        for expected in ["println!", "assert!", "panic!", "vec!"] {
            assert!(
                macro_callees.contains(&expected),
                "call_site for {expected} must still be emitted, got {macro_callees:?}"
            );
        }
    }

    #[test]
    fn macro_invocation_args_remain_opaque() {
        // Tokens inside `println!(...)` are NOT parsed as Rust; only the path is captured.
        // After Bug E-1 the println! Calls edge itself is dropped (Drop-tier), but the
        // contract under test is that `outside()` doesn't leak out of the opaque token
        // stream — which still holds.
        let parsed = parse("fn outside() {} fn caller() { println!(\"{}\", outside()); }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let cs = calls(&edges);
        let outside_walked = cs.iter().any(|e| match &e.to {
            ResolvedOrUnresolved::Resolved { fqdn } => fqdn == "c::outside",
            _ => false,
        });
        assert!(!outside_walked, "macro tokens must remain opaque");
    }

    #[test]
    fn closure_body_is_walked_for_calls() {
        let parsed = parse("fn inner() {} fn caller() { let f = || inner(); f(); }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let cs = calls(&edges);
        // ExprCall captures: inner() inside closure + f() outside (f is not a Path,
        // it's an ExprPath to local var → emitted with name "f", not in defined_fqdns).
        let names: Vec<_> = cs
            .iter()
            .map(|e| match &e.to {
                ResolvedOrUnresolved::Resolved { fqdn } => fqdn.clone(),
                ResolvedOrUnresolved::Unresolved { name }
                | ResolvedOrUnresolved::UnresolvedBridge { name, .. } => name.clone(),
            })
            .collect();
        assert!(names.contains(&"c::inner".to_string()));
    }

    #[test]
    fn turbofish_call_drops_generics_in_emitted_path() {
        // Multi-segment unaliased path stays text-as-written. Generic
        // stripping itself is validated here with a non-builtin name so
        // Stage 3e-1 tier gating doesn't interfere (`Vec` is now Drop
        // and would skip the edge entirely — covered separately).
        let parsed = parse("fn caller() { MyType::<u8>::new(); }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let cs = calls(&edges);
        assert_eq!(cs.len(), 1);
        match &cs[0].to {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "MyType::new"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn stage3e1_call_drop_tier_builtin_skipped() {
        // `Vec` is registered as `BuiltinTier::Drop` — `Vec::new()` is
        // structural noise and produces no `Calls` edge. The body still
        // walks for any inner-call recursion (none here).
        let parsed = parse("fn caller() { Vec::<u8>::new(); String::new(); Box::new(0u8); }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let cs = calls(&edges);
        assert!(
            cs.is_empty(),
            "Drop-tier builtin calls must not surface, got {cs:?}"
        );
    }

    #[test]
    fn stage3e1_call_drop_skips_but_args_still_walked() {
        // The Drop-tier wrapper (`Some`) is skipped, but the call inside
        // the args (`inner()`) must still surface — the visitor recurses
        // through `syn::visit::visit_expr_call` after the tier decision.
        let parsed = parse("fn inner() {} fn caller() { Some(inner()); }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let names: Vec<_> = calls(&edges)
            .into_iter()
            .filter_map(|e| match &e.to {
                ResolvedOrUnresolved::Resolved { fqdn } => Some(fqdn.clone()),
                _ => None,
            })
            .collect();
        assert!(
            names.contains(&"c::inner".to_string()),
            "inner() call must surface even when wrapped in Some(_), got {names:?}"
        );
    }

    // --- Stage 3e-2: References emit via Expr::Path ---

    fn refs(edges: &[standardoc_ir::RawEdge]) -> Vec<&standardoc_ir::RawEdge> {
        edges
            .iter()
            .filter(|e| e.kind == EdgeKind::References)
            .collect()
    }

    fn value_reads(edges: &[standardoc_ir::RawEdge]) -> Vec<&standardoc_ir::RawEdge> {
        refs(edges)
            .into_iter()
            .filter(|e| e.attributes.iter().any(|a| a == "value-read"))
            .collect()
    }

    #[test]
    fn stage3e2_module_local_fn_pointer_emits_value_read_reference() {
        // `foo` in value position resolves to the module-local fn; emit
        // a References edge with `value-read`.
        let parsed = parse("fn foo() {} fn caller() { let _ = foo; }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let vr = value_reads(&edges);
        assert_eq!(vr.len(), 1, "exactly one value-read expected, got {vr:?}");
        assert_eq!(vr[0].from_fqdn, "c::caller");
        match &vr[0].to {
            ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "c::foo"),
            other => panic!("expected resolved c::foo, got {other:?}"),
        }
    }

    #[test]
    fn stage3e2_call_does_not_double_emit_as_reference() {
        // `bar()` should emit ONE Calls edge and ZERO References edges —
        // the manual arg-only recursion in visit_expr_call must suppress
        // the visit_expr_path fire on `node.func`.
        let parsed = parse("fn bar() {} fn caller() { bar(); }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert_eq!(calls(&edges).len(), 1);
        assert!(
            refs(&edges).is_empty(),
            "call-expr func must not surface as a value-read, got {:?}",
            refs(&edges)
        );
    }

    #[test]
    fn stage3e2_unresolved_path_does_not_emit_reference() {
        // Stage-1 safety net: an unindexed identifier in value position
        // produces no References edge (would create noise pointing at
        // unresolved targets).
        let parsed = parse("fn caller() { let _ = unknown_thing; }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert!(
            refs(&edges).is_empty(),
            "unresolved value-reads must be skipped, got {:?}",
            refs(&edges)
        );
    }

    #[test]
    fn stage3e2_local_binding_value_read_is_skipped() {
        // `let x = 0; let _ = x;` — `x` is a local binding (nested
        // scope), so the read must not emit a References edge.
        let parsed = parse("fn caller() { let x = 0; let _ = x; }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert!(
            refs(&edges).is_empty(),
            "local value-read must be skipped, got {:?}",
            refs(&edges)
        );
    }

    #[test]
    fn stage3e2_self_reference_is_skipped() {
        // `fn caller() { let _ = caller; }` — reading own name in value
        // position is dropped (recursion patterns are covered by Calls,
        // value-position self-refs are usually structural).
        let parsed = parse("fn caller() { let _ = caller; }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert!(
            refs(&edges).is_empty(),
            "self-reference must be skipped, got {:?}",
            refs(&edges)
        );
    }

    #[test]
    fn stage3e2_drop_tier_builtin_value_read_is_skipped() {
        // `Vec` in value position (as fn-pointer or generic carrier) is
        // Drop-tier → no References edge.
        let parsed = parse("fn caller() { let _ = Vec::<u8>::new; }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert!(
            refs(&edges).is_empty(),
            "Drop-tier builtin value-read must be skipped, got {:?}",
            refs(&edges)
        );
    }

    #[test]
    fn stage3e2_attribute_tier_builtin_value_read_promotes_flag() {
        // `Iterator` is Attribute-tier — reading it in value position
        // (rare but possible: trait-object construction) must NOT emit
        // an edge but MUST promote `iter` flag onto the enclosing fn.
        let parsed = parse(
            "fn caller() { let _: fn() -> &'static dyn Iterator<Item = u8> = || todo!(); let _ = Iterator::count; }",
        );
        let (symbols, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert!(
            refs(&edges).is_empty(),
            "Attribute-tier builtin value-read must skip the edge, got {:?}",
            refs(&edges)
        );
        let caller = symbols.iter().find(|s| s.fqdn == "c::caller").unwrap();
        assert!(
            caller.flags.iter().any(|f| f == "iter"),
            "Attribute-tier builtin value-read must register `iter` flag, got {:?}",
            caller.flags
        );
    }

    #[test]
    fn stage3e2_multi_segment_resolved_path_emits_value_read() {
        // `MyType::CONST` — multi-segment value-read. The leftmost
        // segment isn't a builtin, so the full path goes through
        // `resolve_path` which canonicalizes against alias_table.
        let parsed = parse("use other::MyType; fn caller() { let _ = MyType::CONST; }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        // `MyType::CONST` resolves to canonical `other::MyType::CONST`
        // which isn't in defined_fqdns → Unresolved → no edge emitted
        // (Stage 1 safety net for unresolved value-reads).
        // The test guards the absence of noise: no References edges
        // emitted in this case, since the canonical target isn't local.
        assert!(
            refs(&edges).is_empty(),
            "unresolved canonical multi-segment path must skip emit, got {:?}",
            refs(&edges)
        );
    }

    #[test]
    fn stage3e2_method_receiver_path_emits_value_read() {
        // `foo.bar()` — `foo` is a legitimate value-read on the receiver.
        // The default visit_expr_method_call recursion walks the receiver
        // via visit_expr → visit_expr_path, so we should see a References
        // edge on `foo`.
        let parsed = parse("fn foo() {} fn caller() { foo.bar(); }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let vr = value_reads(&edges);
        // `foo` reads as `c::foo` (Resolved).
        let foo_refs: Vec<_> = vr
            .iter()
            .filter(
                |e| matches!(&e.to, ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "c::foo"),
            )
            .collect();
        assert_eq!(
            foo_refs.len(),
            1,
            "exactly one value-read on receiver `foo` expected, got {vr:?}"
        );
    }

    #[test]
    fn stage3e2_value_read_dedupes_against_unresolved_module_local() {
        // Bare `bar` resolves to `c::bar` which isn't defined → Unresolved.
        // Per Stage 1 safety net, no References edge.
        let parsed = parse("fn caller() { let _ = bar; }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert!(
            refs(&edges).is_empty(),
            "unresolved bare ident value-read must be skipped, got {:?}",
            refs(&edges)
        );
    }

    // --- Stage 3e-2-bis: Rust let-binding alias propagation ---

    #[test]
    fn stage3e2bis_let_binding_propagates_alias_with_via_alias_slug() {
        // `let x = bar;` makes `x` an alias for `bar`. Reading `x`
        // surfaces as a References edge to `c::bar` with `via-alias`.
        let parsed = parse("fn bar() {} fn caller() { let x = bar; let _ = x; }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let vr = value_reads(&edges);
        // One value-read through alias (`x`) + one direct value-read on
        // the RHS (`bar` in `let x = bar;` is also an Expr::Path read).
        // Both must resolve to c::bar.
        let alias_refs: Vec<_> = vr
            .iter()
            .filter(|e| e.attributes.iter().any(|a| a == "via-alias"))
            .collect();
        assert_eq!(
            alias_refs.len(),
            1,
            "exactly one via-alias edge expected, got {vr:?}"
        );
        match &alias_refs[0].to {
            ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "c::bar"),
            other => panic!("expected resolved c::bar, got {other:?}"),
        }
    }

    #[test]
    fn stage3e2bis_let_mut_binding_propagates_alias_with_via_alias_mutable_slug() {
        // `let mut x = bar;` — the binding is mutable, so the alias is
        // tagged `via-alias-mutable` to signal staleness risk.
        let parsed = parse("fn bar() {} fn caller() { let mut x = bar; let _ = x; }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let mutable_refs: Vec<_> = value_reads(&edges)
            .into_iter()
            .filter(|e| e.attributes.iter().any(|a| a == "via-alias-mutable"))
            .collect();
        assert_eq!(
            mutable_refs.len(),
            1,
            "exactly one via-alias-mutable edge expected, got {:?}",
            value_reads(&edges)
        );
        match &mutable_refs[0].to {
            ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "c::bar"),
            other => panic!("expected resolved c::bar, got {other:?}"),
        }
    }

    #[test]
    fn stage3e2bis_non_path_rhs_does_not_create_alias() {
        // `let x = bar();` — RHS is a Call, not an alias-worthy Path.
        // `x` becomes an opaque Local → reading it surfaces nothing.
        let parsed = parse("fn bar() -> u32 { 0 } fn caller() { let x = bar(); let _ = x; }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        // The `bar()` call still emits a Calls edge.
        let cs = calls(&edges);
        assert!(cs.iter().any(|e| matches!(&e.to,
            ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "c::bar"
        )));
        // But `x` read should NOT produce a References edge — x is Local.
        assert!(
            refs(&edges).is_empty(),
            "non-Path RHS must not create alias propagation, got {:?}",
            refs(&edges)
        );
    }

    #[test]
    fn stage3e2bis_type_annotated_let_still_aliases() {
        // `let x: fn() = bar;` — the Pat::Type wrapper around Pat::Ident
        // must be peeled so alias detection still fires.
        let parsed = parse("fn bar() {} fn caller() { let x: fn() = bar; let _ = x; }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let alias_refs: Vec<_> = value_reads(&edges)
            .into_iter()
            .filter(|e| e.attributes.iter().any(|a| a == "via-alias"))
            .collect();
        assert_eq!(
            alias_refs.len(),
            1,
            "type-annotated let must still alias, got {:?}",
            value_reads(&edges)
        );
    }

    // --- Stage 3e-2-ter: scope-aware visit_expr_call ---

    #[test]
    fn stage3e2ter_let_alias_call_propagates_via_alias() {
        // `let f = bar; f();` — Stage 3e-2-bis registered `f` as alias
        // for `bar` (Const mutability). Stage 3e-2-ter routes the call
        // through resolve_name → emits Calls to c::bar with `via-alias`.
        let parsed = parse("fn bar() {} fn caller() { let f = bar; f(); }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let alias_calls: Vec<_> = calls(&edges)
            .into_iter()
            .filter(|e| e.attributes.iter().any(|a| a == "via-alias"))
            .collect();
        assert_eq!(
            alias_calls.len(),
            1,
            "exactly one via-alias Calls edge expected, got {:?}",
            calls(&edges)
        );
        match &alias_calls[0].to {
            ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "c::bar"),
            other => panic!("expected resolved c::bar, got {other:?}"),
        }
    }

    #[test]
    fn stage3e2ter_let_mut_alias_call_via_alias_mutable() {
        let parsed = parse("fn bar() {} fn caller() { let mut f = bar; f(); }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let mutable_calls: Vec<_> = calls(&edges)
            .into_iter()
            .filter(|e| e.attributes.iter().any(|a| a == "via-alias-mutable"))
            .collect();
        assert_eq!(
            mutable_calls.len(),
            1,
            "expected one via-alias-mutable call, got {:?}",
            calls(&edges)
        );
    }

    #[test]
    fn stage3e2ter_fn_pointer_param_call_is_skipped() {
        // `fn caller(cb: fn()) { cb(); }` — `cb` is a Local (Param) in
        // the caller's scope. Calling it emits NO edge (mirror of TS
        // Bug B Stage 2 — locals in call position are skipped).
        let parsed = parse("fn caller(cb: fn()) { cb(); }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert!(
            calls(&edges).is_empty(),
            "fn-pointer param call must not emit a Calls edge, got {:?}",
            calls(&edges)
        );
    }

    #[test]
    fn stage3e2ter_closure_typed_local_call_is_skipped() {
        // `let f = || {}; f();` — RHS is a closure (not Expr::Path), so
        // no alias is registered for `f`. Reading `f` in call position
        // resolves to a Local without alias → skip.
        let parsed = parse("fn caller() { let f = || {}; f(); }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let non_macro: Vec<_> = calls(&edges)
            .into_iter()
            .filter(|e| !e.attributes.iter().any(|a| a == "macro"))
            .collect();
        assert!(
            non_macro.is_empty(),
            "closure-typed local call must not emit a Calls edge, got {non_macro:?}"
        );
    }

    #[test]
    fn stage3e2ter_module_level_fn_call_still_resolves_post_refactor() {
        // Regression guard: routing through resolve_name MUST preserve
        // the pre-3e-2-ter behavior for plain module-local fn calls.
        let parsed = parse("fn bar() {} fn caller() { bar(); }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let cs = calls(&edges);
        assert_eq!(cs.len(), 1);
        // No alias/builtin attrs on a direct module-local call.
        assert!(
            cs[0].attributes.is_empty(),
            "direct call must carry no attrs, got {:?}",
            cs[0].attributes
        );
        match &cs[0].to {
            ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "c::bar"),
            other => panic!("expected resolved c::bar, got {other:?}"),
        }
    }

    #[test]
    fn stage3e2ter_self_recursion_still_emits_calls_edge() {
        // Self-reference exclusion applies to References (value-position),
        // NOT to Calls — `fn foo() { foo(); }` is a legitimate recursion
        // signal that must surface as a Calls edge.
        let parsed = parse("fn foo() { foo(); }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let self_calls: Vec<_> = calls(&edges)
            .into_iter()
            .filter(|e| {
                e.from_fqdn == "c::foo"
                    && matches!(&e.to, ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "c::foo")
            })
            .collect();
        assert_eq!(
            self_calls.len(),
            1,
            "recursion must emit a self-loop Calls edge, got {:?}",
            calls(&edges)
        );
    }

    #[test]
    fn stage3e2ter_drop_tier_call_still_skipped_via_resolve_name() {
        // Regression: Drop tier dispatch must still work post-refactor.
        let parsed = parse("fn caller() { Vec::<u8>::new(); }");
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        assert!(
            calls(&edges).is_empty(),
            "Drop-tier builtin call must be skipped, got {:?}",
            calls(&edges)
        );
    }

    #[test]
    fn stage3e2bis_destructuring_pattern_falls_through_to_bind_pat() {
        // `let (x, y) = (bar, baz);` — tuple destructuring isn't an
        // alias (the RHS is a Tuple expr, not a Path), so x and y stay
        // opaque Locals. Reading them produces no References edge.
        let parsed = parse(
            "fn bar() {} fn baz() {} fn caller() { let (x, y) = (bar, baz); let _ = x; let _ = y; }",
        );
        let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
        // The tuple expr's inner Expr::Path reads on bar and baz DO emit
        // value-reads at the RHS expression itself. But x/y reads on the
        // bottom lines must NOT emit (locals without alias).
        let resolved_targets: Vec<_> = value_reads(&edges)
            .iter()
            .filter_map(|e| match &e.to {
                ResolvedOrUnresolved::Resolved { fqdn } => Some(fqdn.clone()),
                _ => None,
            })
            .collect();
        // Should see c::bar and c::baz from the RHS tuple (2 reads),
        // NOT 4 (no x/y propagation).
        assert_eq!(
            resolved_targets.len(),
            2,
            "destructuring must not create alias propagation, got {resolved_targets:?}"
        );
        assert!(resolved_targets.contains(&"c::bar".to_string()));
        assert!(resolved_targets.contains(&"c::baz".to_string()));
    }

    // --- IR-4-b: call_sites emission (observational, separate from edges) ---

    fn call_sites_of(parsed: &syn::File) -> Vec<standardoc_ir::RawCallSite> {
        let (_, _, _, css) = walk(parsed, "c", "src/lib.rs", "c");
        css
    }

    #[test]
    fn ir4b_path_form_call_emits_call_site_with_classified_args() {
        // `foo("hi", 42, x)` — three positional args: a string literal,
        // a numeric literal, and an identifier. Each must surface with
        // the correct `is_string_literal` flag.
        let parsed = parse("fn caller() { let x = 0; foo(\"hi\", 42, x); }");
        let css = call_sites_of(&parsed);
        let cs = css
            .iter()
            .find(|c| c.callee_text == "foo")
            .unwrap_or_else(|| panic!("expected a call_site for foo(...), got {css:?}"));
        assert_eq!(cs.from_fqdn, "c::caller");
        assert!(cs.receiver_chain.is_empty());
        assert_eq!(cs.args.len(), 3);
        assert_eq!(cs.args[0].value, "hi");
        assert!(cs.args[0].is_string_literal);
        assert_eq!(cs.args[1].value, "42");
        assert!(!cs.args[1].is_string_literal);
        assert_eq!(cs.args[2].value, "x");
        assert!(!cs.args[2].is_string_literal);
    }

    #[test]
    fn ir4b_multi_segment_path_call_keeps_dotted_callee_text() {
        // `tauri::invoke("ping")` — multi-segment path is preserved
        // verbatim in `callee_text`; receiver_chain stays empty (path-
        // form has no receiver concept).
        let parsed = parse("fn caller() { tauri::invoke(\"ping\"); }");
        let css = call_sites_of(&parsed);
        let cs = css
            .iter()
            .find(|c| c.callee_text == "tauri::invoke")
            .unwrap_or_else(|| panic!("expected tauri::invoke call_site, got {css:?}"));
        assert!(cs.receiver_chain.is_empty());
        assert_eq!(cs.args.len(), 1);
        assert_eq!(cs.args[0].value, "ping");
        assert!(cs.args[0].is_string_literal);
    }

    #[test]
    fn ir4b_method_call_emits_receiver_chain_single_segment() {
        // `obj.bar(x)` — single-segment receiver_chain.
        let parsed = parse("fn caller() { let obj = 0; obj.bar(x); }");
        let css = call_sites_of(&parsed);
        let cs = css
            .iter()
            .find(|c| c.callee_text.ends_with(".bar"))
            .unwrap_or_else(|| panic!("expected obj.bar call_site, got {css:?}"));
        assert_eq!(cs.callee_text, "obj.bar");
        assert_eq!(cs.receiver_chain, vec!["obj".to_string()]);
        assert_eq!(cs.args.len(), 1);
        assert_eq!(cs.args[0].value, "x");
    }

    #[test]
    fn ir4b_chained_method_call_walks_field_layers_into_receiver_chain() {
        // `obj.field.bar(x)` — receiver is `ExprField { base: Path("obj"), member: "field" }`.
        // The chain must surface in source order: ["obj", "field"].
        let parsed = parse("fn caller() { let obj = 0; obj.field.bar(x); }");
        let css = call_sites_of(&parsed);
        let cs = css
            .iter()
            .find(|c| c.callee_text.ends_with(".bar"))
            .unwrap_or_else(|| panic!("expected obj.field.bar call_site, got {css:?}"));
        assert_eq!(cs.callee_text, "obj.field.bar");
        assert_eq!(
            cs.receiver_chain,
            vec!["obj".to_string(), "field".to_string()]
        );
    }

    #[test]
    fn ir4b_macro_call_emits_call_site_with_bang_suffix_and_no_args() {
        // `println!("hi")` — macro args are an opaque token stream, so
        // `args` stays empty by design. `callee_text` carries the `!`
        // suffix so consumers can distinguish macros from fn calls.
        let parsed = parse("fn caller() { println!(\"hi\"); }");
        let css = call_sites_of(&parsed);
        let cs = css
            .iter()
            .find(|c| c.callee_text == "println!")
            .unwrap_or_else(|| panic!("expected println! call_site, got {css:?}"));
        assert!(cs.args.is_empty());
        assert!(cs.receiver_chain.is_empty());
    }

    #[test]
    fn ir4b_drop_tier_call_still_emits_call_site() {
        // `Vec::<u8>::new()` — the Drop tier suppresses the graph edge,
        // but the call_site must still surface (observational, plugin-
        // layer reads it regardless of edge-tier decisions). Generics
        // are stripped in `callee_text` via `path_to_string`.
        let parsed = parse("fn caller() { Vec::<u8>::new(); }");
        let (_, edges, _, css) = walk(&parsed, "c", "src/lib.rs", "c");
        assert!(
            calls(&edges).is_empty(),
            "Drop-tier edge suppression must still hold"
        );
        let cs = css
            .iter()
            .find(|c| c.callee_text == "Vec::new")
            .unwrap_or_else(|| panic!("expected Vec::new call_site, got {css:?}"));
        assert!(cs.receiver_chain.is_empty());
    }

    #[test]
    fn ir4b_self_recursive_call_still_emits_call_site() {
        // `fn foo() { foo(); }` — self-recursion preserved as a Calls
        // edge (different from value-position self-refs) AND as a
        // call_site (observational signal of the recursion).
        let parsed = parse("fn foo() { foo(); }");
        let css = call_sites_of(&parsed);
        assert!(
            css.iter().any(|c| c.callee_text == "foo"),
            "self-recursive call must emit a call_site, got {css:?}"
        );
    }

    #[test]
    fn ir4b_call_site_from_fqdn_attributes_to_enclosing_method() {
        // Same as `impl_fn_body_calls_attributed_to_method_fqdn` but
        // checking the call_site path — `from_fqdn` must be the impl
        // method's fqdn, not the module's.
        let parsed = parse("fn helper() {} struct F; impl F { fn run(&self) { helper(); } }");
        let css = call_sites_of(&parsed);
        let cs = css
            .iter()
            .find(|c| c.callee_text == "helper")
            .unwrap_or_else(|| panic!("expected helper call_site, got {css:?}"));
        assert_eq!(cs.from_fqdn, "c::F::run");
    }

    #[test]
    fn ir4b_empty_callee_path_still_falls_through_to_default() {
        // Edge case — `path_to_string` returns an empty string for a
        // path with no segments. The call_site emit short-circuits in
        // that case (the `if !path_str.is_empty()` guard), so no
        // call_site is produced. Guard against regression by checking
        // that pathological inputs don't crash the walk.
        let parsed = parse("fn caller() { (|| {})(); }");
        // `(|| {})()` is a non-path func — we DO emit a call_site with
        // the stringified closure text. Just assert no panic + at
        // least one call_site emitted on the non-path branch.
        let css = call_sites_of(&parsed);
        assert!(
            css.iter().any(|c| !c.callee_text.is_empty()),
            "non-path call must still emit a non-empty call_site"
        );
    }
}

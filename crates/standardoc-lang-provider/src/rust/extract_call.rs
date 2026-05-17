use proc_macro2::Span;
use standardoc_ir::{BuiltinTier, EdgeKind, Language, RawEdge, ResolvedOrUnresolved, Site};
use syn::spanned::Spanned;
use syn::visit::Visit;

use super::walk::{
    NameResolution, WalkContext, col_from_span, line_from_span, lookup_scope_for, path_to_string,
};
use crate::builtins::global as global_builtin_registry;

pub(crate) fn visit_block(
    ctx: &mut WalkContext,
    block: &syn::Block,
    current_module: &str,
    enclosing_fqdn: &str,
) {
    let file_path = ctx.core.file_path.clone();
    let initial_scope = lookup_scope_for(ctx, block.span());
    let mut visitor = CallVisitor {
        ctx,
        current_module: current_module.to_string(),
        enclosing_fqdn: enclosing_fqdn.to_string(),
        file_path,
        current_scope_idx: initial_scope,
    };
    visitor.visit_block(block);
}

struct CallVisitor<'a> {
    ctx: &'a mut WalkContext,
    current_module: String,
    enclosing_fqdn: String,
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
        let (target, alias_mut, via_builtin) =
            match self
                .ctx
                .resolve_name(path, self.current_scope_idx, &self.current_module)
            {
                NameResolution::Drop | NameResolution::Local => return,
                NameResolution::Attribute(tag) => {
                    self.ctx
                        .register_attribute_flag(&self.enclosing_fqdn, &tag);
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
                let span = expr_path.span();
                let leftmost = path_str.split("::").next().unwrap_or("");
                match global_builtin_registry().lookup(leftmost, Language::Rust) {
                    // Stage 3e-1: Drop = structural noise (`Vec::new`,
                    // `Box::new`, `Some(x)`, `Ok(x)`, `String::from`).
                    // Silently skipped — inner args still walked below.
                    Some(entry) if matches!(entry.tier, BuiltinTier::Drop) => {}
                    // Stage 3e-1b: Attribute = source-flag promotion
                    // target. Rare in call position (most Iterator /
                    // Future hits happen via type bounds in
                    // `extract_type`), but covered for symmetry —
                    // e.g. `Future::poll(...)` would surface as a
                    // `"async"` flag on the enclosing fn.
                    Some(entry) if matches!(entry.tier, BuiltinTier::Attribute) => {
                        self.ctx
                            .register_attribute_flag(&self.enclosing_fqdn, &entry.tag);
                    }
                    // Edge-tier call (e.g. `Error::source(...)`) — emit
                    // straight to the synthetic builtin FQDN with the
                    // standard `via-builtin` / `builtin-<slug>` attrs.
                    Some(entry) => {
                        let to = ResolvedOrUnresolved::Resolved {
                            fqdn: entry.synthetic_fqdn.clone(),
                        };
                        let attrs = vec![
                            "via-builtin".to_string(),
                            format!("builtin-{}", entry.tag.slug()),
                        ];
                        self.emit_call_with_attributes(to, span, attrs);
                    }
                    None => {
                        let to = self.ctx.resolve_path(&path_str, &self.current_module);
                        self.emit_call(to, span);
                    }
                }
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
        // Non-path func (e.g. `(get_fn())()`) — default recursion walks
        // both the func sub-expression (where `visit_expr_path` can
        // legitimately surface value-reads) and the args.
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
        let to = self.ctx.resolve_path(&path_str, &self.current_module);
        self.emit_call_with_attributes(to, span, vec!["macro".to_string()]);
    }
}

#[cfg(test)]
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
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let cs = calls(&edges);
        assert_eq!(cs.len(), 2);
    }

    #[test]
    fn impl_fn_body_calls_attributed_to_method_fqdn() {
        let parsed = parse("fn helper() {} struct F; impl F { fn run(&self) { helper(); } }");
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let cs = calls(&edges);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].from_fqdn, "c::F::run");
    }

    #[test]
    fn trait_default_body_calls_attributed_to_trait_fn_fqdn() {
        let parsed = parse("fn helper() {} trait T { fn run(&self) { helper(); } }");
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let cs = calls(&edges);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].from_fqdn, "c::T::run");
    }

    #[test]
    fn async_block_body_is_walked_for_calls() {
        let parsed = parse("fn outside() {} fn caller() { let _ = async { outside(); }; }");
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
    fn macro_invocation_emits_call_edge_with_macro_attribute() {
        let parsed = parse("fn caller() { println!(\"hi\"); }");
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let cs = calls(&edges);
        assert_eq!(cs.len(), 1, "exactly one macro call expected");
        assert!(
            cs[0].attributes.iter().any(|a| a == "macro"),
            "macro call edge must carry attribute=`macro`, got {:?}",
            cs[0].attributes
        );
        match &cs[0].to {
            ResolvedOrUnresolved::Unresolved { name } => {
                assert!(name.ends_with("println"), "macro target = {name:?}");
            }
            other => panic!("expected unresolved macro target, got {other:?}"),
        }
    }

    #[test]
    fn macro_invocation_args_remain_opaque() {
        // Tokens inside `println!(...)` are NOT parsed as Rust; only the path is captured.
        let parsed = parse("fn outside() {} fn caller() { println!(\"{}\", outside()); }");
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let cs = calls(&edges);
        // ONE edge for the println macro itself; the `outside()` inside is unreachable.
        let macro_edges: Vec<_> = cs
            .iter()
            .filter(|e| e.attributes.iter().any(|a| a == "macro"))
            .collect();
        assert_eq!(macro_edges.len(), 1);
        let outside_walked = cs.iter().any(|e| match &e.to {
            ResolvedOrUnresolved::Resolved { fqdn } => fqdn == "c::outside",
            _ => false,
        });
        assert!(!outside_walked, "macro tokens must remain opaque");
    }

    #[test]
    fn closure_body_is_walked_for_calls() {
        let parsed = parse("fn inner() {} fn caller() { let f = || inner(); f(); }");
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (symbols, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let parsed = parse(
            "use other::MyType; fn caller() { let _ = MyType::CONST; }",
        );
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let vr = value_reads(&edges);
        // `foo` reads as `c::foo` (Resolved).
        let foo_refs: Vec<_> = vr
            .iter()
            .filter(|e| matches!(&e.to, ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "c::foo"))
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
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
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

    #[test]
    fn stage3e2bis_destructuring_pattern_falls_through_to_bind_pat() {
        // `let (x, y) = (bar, baz);` — tuple destructuring isn't an
        // alias (the RHS is a Tuple expr, not a Path), so x and y stay
        // opaque Locals. Reading them produces no References edge.
        let parsed = parse(
            "fn bar() {} fn baz() {} fn caller() { let (x, y) = (bar, baz); let _ = x; let _ = y; }",
        );
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
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
}

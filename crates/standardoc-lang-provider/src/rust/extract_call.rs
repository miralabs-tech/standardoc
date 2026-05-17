use proc_macro2::Span;
use standardoc_ir::{BuiltinTier, EdgeKind, Language, RawEdge, ResolvedOrUnresolved, Site};
use syn::spanned::Spanned;
use syn::visit::Visit;

use super::walk::{WalkContext, col_from_span, line_from_span, path_to_string};
use crate::builtins::global as global_builtin_registry;

pub(crate) fn visit_block(
    ctx: &mut WalkContext,
    block: &syn::Block,
    current_module: &str,
    enclosing_fqdn: &str,
) {
    let file_path = ctx.core.file_path.clone();
    let mut visitor = CallVisitor {
        ctx,
        current_module: current_module.to_string(),
        enclosing_fqdn: enclosing_fqdn.to_string(),
        file_path,
    };
    visitor.visit_block(block);
}

struct CallVisitor<'a> {
    ctx: &'a mut WalkContext,
    current_module: String,
    enclosing_fqdn: String,
    file_path: String,
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
        }
        // Recurse into args (nested calls) and the func expr (in case it's not a path).
        syn::visit::visit_expr_call(self, node);
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
}

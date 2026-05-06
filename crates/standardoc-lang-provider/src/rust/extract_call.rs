use proc_macro2::Span;
use standardoc_ir::{EdgeKind, RawEdge, ResolvedOrUnresolved, Site};
use syn::spanned::Spanned;
use syn::visit::Visit;

use super::walk::{WalkContext, col_from_span, line_from_span, path_to_string};

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
        self.ctx.push_edge(RawEdge {
            from_fqdn: self.enclosing_fqdn.clone(),
            kind: EdgeKind::Calls,
            to,
            sites: vec![Site {
                file: self.file_path.clone(),
                line: line_from_span(span),
                col: col_from_span(span),
            }],
            attributes: vec![],
        });
    }
}

impl<'ast> Visit<'ast> for CallVisitor<'_> {
    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let syn::Expr::Path(expr_path) = &*node.func {
            let path_str = path_to_string(&expr_path.path);
            if !path_str.is_empty() {
                let span = expr_path.span();
                let to = self.ctx.resolve_path(&path_str, &self.current_module);
                self.emit_call(to, span);
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

    // Lock 17 Q12: no async deep walk, no macro expansion day-1.
    fn visit_expr_async(&mut self, _node: &'ast syn::ExprAsync) {}

    fn visit_macro(&mut self, _node: &'ast syn::Macro) {}
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
        let cs = calls(&edges);
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
    fn async_block_body_is_not_walked_for_calls() {
        let parsed = parse("fn outside() {} fn caller() { let _ = async { outside(); }; }");
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let cs = calls(&edges);
        assert!(cs.is_empty(), "async block body must be skipped day-1");
    }

    #[test]
    fn macro_invocation_args_are_not_walked_for_calls() {
        let parsed = parse("fn outside() {} fn caller() { println!(\"{}\", outside()); }");
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let cs = calls(&edges);
        assert!(cs.is_empty(), "macro tokens must be opaque day-1");
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
        // Multi-segment unaliased path stays text-as-written (Rust 2018: prelude/extern,
        // not module-local). What we validate here is the generic stripping.
        let parsed = parse("fn caller() { Vec::<u8>::new(); }");
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let cs = calls(&edges);
        assert_eq!(cs.len(), 1);
        match &cs[0].to {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "Vec::new"),
            other => panic!("got {other:?}"),
        }
    }
}

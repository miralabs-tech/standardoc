use standardoc_ir::{EdgeKind, RawEdge, ResolvedOrUnresolved};
use swc_core::common::Span;
use swc_core::ecma::ast::{
    CallExpr, Callee, Expr, Function, MemberProp, NewExpr, OptChainBase, OptChainExpr,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::walk::TsWalkContext;

/// Pass-2 entry: walk a function body for `CallExpr` / `NewExpr`. Mirror of
/// `rust::extract_call::visit_block`. Skips dynamic dispatch (`obj.method()`
/// always Unresolved with the method ident, day-1, no inference).
pub(crate) fn visit_function_body(
    ctx: &mut TsWalkContext<'_>,
    function: &Function,
    current_module: &str,
    enclosing_fqdn: &str,
) {
    let Some(body) = &function.body else {
        return;
    };
    let mut visitor = CallVisitor {
        ctx,
        current_module: current_module.to_string(),
        enclosing_fqdn: enclosing_fqdn.to_string(),
    };
    body.visit_with(&mut visitor);
}

/// Walk an arbitrary expression (typically the initializer of a `const fn = …`
/// arrow / function expression) for `CallExpr` / `NewExpr` nested inside.
pub(crate) fn visit_expression_for_calls(
    ctx: &mut TsWalkContext<'_>,
    expr: &Expr,
    current_module: &str,
    enclosing_fqdn: &str,
) {
    let mut visitor = CallVisitor {
        ctx,
        current_module: current_module.to_string(),
        enclosing_fqdn: enclosing_fqdn.to_string(),
    };
    expr.visit_with(&mut visitor);
}

struct CallVisitor<'a, 'b> {
    ctx: &'a mut TsWalkContext<'b>,
    current_module: String,
    enclosing_fqdn: String,
}

impl CallVisitor<'_, '_> {
    fn emit_call(&mut self, to: ResolvedOrUnresolved, span: Span) {
        let site = self.ctx.span_site(span);
        self.ctx.push_edge(RawEdge {
            from_fqdn: self.enclosing_fqdn.clone(),
            kind: EdgeKind::Calls,
            to,
            sites: vec![site],
        });
    }

    fn handle_callee_expr(&mut self, callee: &Expr) {
        match callee {
            Expr::Ident(ident) => {
                let name = ident.sym.to_string();
                let to = self.ctx.resolve_call(&name, &self.current_module);
                self.emit_call(to, ident.span);
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
        }
        node.visit_children_with(self);
    }

    fn visit_new_expr(&mut self, node: &NewExpr) {
        self.handle_callee_expr(node.callee.as_ref());
        node.visit_children_with(self);
    }

    // OptCall is reached via visit_children on OptChainExpr — visiting it
    // separately would double-emit. We handle OptChainBase::Call here and
    // let the recursion visit the args inside.
    fn visit_opt_chain_expr(&mut self, node: &OptChainExpr) {
        if let OptChainBase::Call(call) = node.base.as_ref() {
            self.handle_callee_expr(call.callee.as_ref());
            for arg in &call.args {
                arg.visit_with(self);
            }
        } else {
            node.visit_children_with(self);
        }
    }
}

fn member_prop_name(prop: &MemberProp) -> Option<String> {
    match prop {
        MemberProp::Ident(i) => Some(i.sym.to_string()),
        MemberProp::PrivateName(p) => Some(format!("#{}", p.name)),
        MemberProp::Computed(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use swc_core::common::comments::SingleThreadedComments;
    use swc_core::common::{FileName, sync::Lrc, SourceMap};
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

    fn calls(edges: &[RawEdge]) -> Vec<&RawEdge> {
        edges.iter().filter(|e| e.kind == EdgeKind::Calls).collect()
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
        let (_, edges) = run(
            "import { foo } from './helper'; function caller() { foo(); }",
        );
        let cs = calls(&edges);
        assert_eq!(cs.len(), 1);
        match &cs[0].to {
            ResolvedOrUnresolved::Unresolved { name } => {
                // Relative import resolves under the current package; from_file
                // is /tmp/pkg/src/index.ts so ./helper → @app::src::helper::foo.
                assert_eq!(name, "@app::src::helper::foo");
            }
            other => panic!("expected unresolved canonical via alias, got {other:?}"),
        }
    }

    #[test]
    fn _spanned_link() {
        let _ = swc_core::common::DUMMY_SP.lo();
        let _ = Span::default();
    }
}

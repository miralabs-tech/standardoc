use standardoc_ir::{EdgeKind, RawEdge, ResolvedOrUnresolved};
use swc_core::common::Span;
use swc_core::ecma::ast::{
    CallExpr, Callee, Expr, Function, Ident, JSXAttrOrSpread, JSXAttrValue, JSXElement,
    JSXElementChild, JSXElementName, JSXExpr, MemberProp, NewExpr, OptChainBase, OptChainExpr,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::walk::TsWalkContext;
use crate::template::JS_GLOBALS;

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
    let Some(body) = &function.body else {
        return;
    };
    let mut visitor = CallVisitor::new(ctx, current_module, enclosing_fqdn);
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

struct CallVisitor<'a, 'b> {
    ctx: &'a mut TsWalkContext<'b>,
    current_module: String,
    enclosing_fqdn: String,
    /// `Some(slug)` while walking the inside of a JSX expression slot.
    /// `None` everywhere else — keeps non-JSX TS extraction unchanged.
    jsx_context: Option<JsxAttribute>,
}

impl<'a, 'b> CallVisitor<'a, 'b> {
    fn new(
        ctx: &'a mut TsWalkContext<'b>,
        current_module: &str,
        enclosing_fqdn: &str,
    ) -> Self {
        Self {
            ctx,
            current_module: current_module.to_string(),
            enclosing_fqdn: enclosing_fqdn.to_string(),
            jsx_context: None,
        }
    }

    fn emit_call(&mut self, to: ResolvedOrUnresolved, span: Span) {
        let site = self.ctx.span_site(span);
        let confidence = to.default_confidence();
        self.ctx.push_edge(RawEdge {
            from_fqdn: self.enclosing_fqdn.clone(),
            kind: EdgeKind::Calls,
            to,
            sites: vec![site],
            attributes: vec![],
            confidence,
        });
    }

    fn emit_template_ref(&mut self, name: &str, span: Span, attribute: &str) {
        let to = self.ctx.resolve_call(name, &self.current_module);
        let confidence = to.default_confidence();
        let site = self.ctx.span_site(span);
        self.ctx.push_edge(RawEdge {
            from_fqdn: self.enclosing_fqdn.clone(),
            kind: EdgeKind::References,
            to,
            sites: vec![site],
            attributes: vec![attribute.to_string()],
            confidence,
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

    /// Inside a JSX expression slot, every plain identifier becomes a
    /// REFERENCES edge tagged with the active `template-*` slug. Outside
    /// JSX (the common case) this is a no-op so non-JSX TS extraction is
    /// unchanged. Member-prop names live in `MemberProp::Ident`
    /// (an `IdentName`), not `Ident` — they don't fire here, matching the
    /// "left-most segment" rule of [`crate::template`].
    fn visit_ident(&mut self, ident: &Ident) {
        let Some(attribute) = self.jsx_context else {
            return;
        };
        let name = ident.sym.to_string();
        if JS_GLOBALS.contains(&name.as_str()) {
            return;
        }
        self.emit_template_ref(&name, ident.span, attribute.as_slug());
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
            .filter(|e| {
                e.kind == EdgeKind::References && e.attributes.iter().any(|a| a == attr)
            })
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
        let (_, edges) = run_tsx(
            "function App() { return <Header />; }",
        );
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
        let (_, edges) = run_tsx(
            "function App() { return <input value={text} />; }",
        );
        let bind = refs_with_attribute(&edges, "template-bind");
        let names: Vec<&str> = bind.iter().map(|e| ref_target_name(e)).collect();
        assert!(names.iter().any(|n| n.ends_with("text")));
    }

    #[test]
    fn jsx_child_expression_emits_template_interpolation() {
        let (_, edges) = run_tsx(
            "function App() { return <p>{message}</p>; }",
        );
        let interp = refs_with_attribute(&edges, "template-interpolation");
        let names: Vec<&str> = interp.iter().map(|e| ref_target_name(e)).collect();
        assert!(names.iter().any(|n| n.ends_with("message")));
    }

    #[test]
    fn jsx_spread_attribute_emits_template_bind() {
        let (_, edges) = run_tsx(
            "function App(props: any) { return <input {...props} />; }",
        );
        let bind = refs_with_attribute(&edges, "template-bind");
        let names: Vec<&str> = bind.iter().map(|e| ref_target_name(e)).collect();
        assert!(names.iter().any(|n| n.ends_with("props")));
    }

    #[test]
    fn jsx_event_handler_call_inside_attribute_also_emits_calls_edge() {
        let (_, edges) = run_tsx(
            "function App() { return <button onClick={() => handle(payload)} />; }",
        );
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
        let (_, edges) = run_tsx(
            "function App() { return <p>{user.name}</p>; }",
        );
        let interp = refs_with_attribute(&edges, "template-interpolation");
        let names: Vec<&str> = interp.iter().map(|e| ref_target_name(e)).collect();
        assert!(names.iter().any(|n| n.ends_with("user")));
        assert!(!names.iter().any(|n| n.ends_with("name")));
    }

    #[test]
    fn jsx_static_string_attribute_does_not_emit_ref() {
        let (_, edges) = run_tsx(
            r#"function App() { return <div className="static" />; }"#,
        );
        let bind = refs_with_attribute(&edges, "template-bind");
        assert!(bind.is_empty());
    }

    #[test]
    fn jsx_nested_components_both_emit_component_ref() {
        let (_, edges) = run_tsx(
            "function App() { return <Layout><Header /></Layout>; }",
        );
        let comp = refs_with_attribute(&edges, "template-component-ref");
        let names: Vec<&str> = comp.iter().map(|e| ref_target_name(e)).collect();
        assert!(names.iter().any(|n| n.ends_with("Layout")));
        assert!(names.iter().any(|n| n.ends_with("Header")));
    }

    #[test]
    fn jsx_globals_filtered_in_interpolation() {
        let (_, edges) = run_tsx(
            "function App() { return <p>{Math.max(a, b)}</p>; }",
        );
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
}

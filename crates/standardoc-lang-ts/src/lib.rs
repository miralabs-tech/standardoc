//! TypeScript / JavaScript language provider for Standardoc.
//!
//! Uses `swc_ecma_parser` + `swc_ecma_ast` to discover documentable symbols:
//! - function declarations (incl. async / generator)
//! - class declarations (plus their methods, fields and accessors)
//! - interface declarations
//! - type aliases
//! - enums
//! - top-level `const` / `let` / `var` bindings
//! - `export` forms (prefixed visibility becomes `public`)
//!
//! Leading `JSDoc` (`/** ... */`) comments are attached to each symbol, with
//! the `*` line prefixes stripped before the core extractor parses tags.

use standardoc_core::lang::{DiscoveredSymbol, LanguageProvider, ParseError};
use standardoc_core::model::{
    CommentDelimiters, CommentStyles, ParamInfo, RefKind, References, SourceRange, SymbolInfo,
    SymbolKind, TypeInfo, Visibility,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use swc_common::comments::{Comment, CommentKind, Comments, SingleThreadedComments};
use swc_common::sync::Lrc;
use swc_common::{FileName, SourceMap, Span};
use swc_ecma_ast::{
    ArrowExpr, ClassDecl, ClassMember, Decl, EsVersion, ExportDecl, Expr, FnDecl, MethodKind,
    ModuleDecl, ModuleItem, Stmt, TsEnumDecl, TsInterfaceDecl, TsTypeAliasDecl, VarDecl,
    VarDeclarator,
};
use swc_ecma_parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};

/// @doc lang.providers.ts TypeScript / JavaScript
/// @description Native TypeScript and JavaScript language provider — discovers types, classes, interfaces, functions via the `swc` full-AST parser.
/// @crate standardoc-lang-ts
/// @backend swc
#[derive(Debug, Default, Clone, Copy)]
pub struct TsProvider;

impl LanguageProvider for TsProvider {
    fn id(&self) -> &'static str {
        "typescript"
    }

    fn extensions(&self) -> &[&'static str] {
        &[".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"]
    }

    fn comment_styles(&self) -> &CommentStyles {
        ts_comment_styles()
    }

    fn discover_symbols(
        &self,
        content: &str,
        path: &Path,
    ) -> Result<Vec<DiscoveredSymbol>, ParseError> {
        let cm: Lrc<SourceMap> = Lrc::default();
        let comments = SingleThreadedComments::default();
        let fm = cm.new_source_file(
            Lrc::new(FileName::Custom(path.display().to_string())),
            content.to_owned(),
        );

        let is_tsx = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("tsx") || e.eq_ignore_ascii_case("jsx"));

        let lexer = Lexer::new(
            Syntax::Typescript(TsSyntax {
                tsx: is_tsx,
                decorators: true,
                ..Default::default()
            }),
            EsVersion::EsNext,
            StringInput::from(&*fm),
            Some(&comments),
        );
        let mut parser = Parser::new_from(lexer);
        let module = parser.parse_module().map_err(|err| ParseError::Syntax {
            path: path.display().to_string(),
            message: format!("{err:?}"),
        })?;

        let mut collector = Collector {
            path: Vec::new(),
            symbols: Vec::new(),
            comments: &comments,
            source_map: cm,
        };
        for item in &module.body {
            collector.visit_item(item);
        }
        Ok(collector.symbols)
    }
}

fn ts_comment_styles() -> &'static CommentStyles {
    static STYLES: OnceLock<CommentStyles> = OnceLock::new();
    STYLES.get_or_init(|| CommentStyles {
        single: vec!["//".to_owned()],
        multi: Some(CommentDelimiters {
            start: "/*".to_owned(),
            end: "*/".to_owned(),
        }),
        doc_single: vec![],
        doc_multi: Some(CommentDelimiters {
            start: "/**".to_owned(),
            end: "*/".to_owned(),
        }),
    })
}

struct Collector<'a> {
    path: Vec<String>,
    symbols: Vec<DiscoveredSymbol>,
    comments: &'a SingleThreadedComments,
    source_map: Lrc<SourceMap>,
}

impl Collector<'_> {
    fn visit_item(&mut self, item: &ModuleItem) {
        match item {
            ModuleItem::ModuleDecl(decl) => self.visit_module_decl(decl),
            ModuleItem::Stmt(stmt) => self.visit_stmt(stmt, Visibility::Private),
        }
    }

    fn visit_module_decl(&mut self, decl: &ModuleDecl) {
        if let ModuleDecl::ExportDecl(ExportDecl { decl, span, .. }) = decl {
            self.visit_decl(decl, Visibility::Public, *span);
        }
    }

    fn visit_stmt(&mut self, stmt: &Stmt, vis: Visibility) {
        if let Stmt::Decl(decl) = stmt {
            let span = decl_span(decl);
            self.visit_decl(decl, vis, span);
        }
    }

    fn visit_decl(&mut self, decl: &Decl, vis: Visibility, outer_span: Span) {
        match decl {
            Decl::Fn(f) => self.emit_fn(f, vis, outer_span),
            Decl::Class(c) => self.emit_class(c, vis, outer_span),
            Decl::Var(v) => self.emit_var(v, vis),
            Decl::TsInterface(i) => self.emit_interface(i, vis, outer_span),
            Decl::TsTypeAlias(t) => self.emit_type_alias(t, vis, outer_span),
            Decl::TsEnum(e) => self.emit_enum(e, vis, outer_span),
            _ => {}
        }
    }

    fn fqn_for(&self, name: &str) -> Vec<String> {
        let mut fqn = self.path.clone();
        fqn.push(name.to_owned());
        fqn
    }

    fn emit_fn(&mut self, f: &FnDecl, vis: Visibility, outer_span: Span) {
        let name = f.ident.sym.to_string();
        let fqn = self.fqn_for(&name);
        let params: Vec<ParamInfo> = f
            .function
            .params
            .iter()
            .map(|p| param_from_pat(&p.pat))
            .collect();
        let returns = f.function.return_type.as_ref().map(|t| TypeInfo {
            repr: type_repr(&t.type_ann),
        });
        let signature = format!(
            "function {}{}({}){}",
            name,
            type_params_repr(f.function.type_params.as_deref()),
            params
                .iter()
                .map(param_display)
                .collect::<Vec<_>>()
                .join(", "),
            returns
                .as_ref()
                .map(|r| format!(": {}", r.repr))
                .unwrap_or_default()
        );
        let references =
            collect_ts_fn_references(&f.function.params, f.function.return_type.as_deref());
        let symbol = SymbolInfo {
            kind: SymbolKind::Function,
            visibility: vis,
            signature,
            params,
            returns,
            generics: vec![],
            decorators: vec![],
            is_async: f.function.is_async,
            is_deprecated: false,
            references,
        };
        self.push(fqn, symbol, outer_span, &f.ident.sym);
    }

    fn emit_class(&mut self, c: &ClassDecl, vis: Visibility, outer_span: Span) {
        let name = c.ident.sym.to_string();
        let fqn = self.fqn_for(&name);

        // Refs: Extends + Implements from class header.
        let heritage = collect_class_heritage(&c.class);
        // List of implemented traits/interfaces — tagged on each method so
        // `find_implementations(I)` can find them.
        let implements_targets: Vec<String> = heritage
            .outgoing
            .iter()
            .filter(|s| s.kind == RefKind::Implements)
            .map(|s| s.target.clone())
            .collect();

        let symbol = SymbolInfo {
            kind: SymbolKind::Class,
            visibility: vis,
            signature: format!(
                "class {name}{}",
                type_params_repr(c.class.type_params.as_deref())
            ),
            params: vec![],
            returns: None,
            generics: vec![],
            decorators: vec![],
            is_async: false,
            is_deprecated: false,
            references: heritage,
        };
        self.push(fqn, symbol, outer_span, &c.ident.sym);

        self.path.push(name);
        for member in &c.class.body {
            self.visit_class_member(member, &implements_targets);
        }
        self.path.pop();
    }

    fn visit_class_member(&mut self, member: &ClassMember, implements_targets: &[String]) {
        match member {
            ClassMember::Method(m) => {
                let prop_name = match &m.key {
                    swc_ecma_ast::PropName::Ident(i) => i.sym.to_string(),
                    _ => return,
                };
                let fqn = self.fqn_for(&prop_name);
                let params: Vec<ParamInfo> = m
                    .function
                    .params
                    .iter()
                    .map(|p| param_from_pat(&p.pat))
                    .collect();
                let returns = m.function.return_type.as_ref().map(|t| TypeInfo {
                    repr: type_repr(&t.type_ann),
                });
                let kind = SymbolKind::Method;
                let signature = format!(
                    "{}{}({}){}",
                    match m.kind {
                        MethodKind::Getter => "get ",
                        MethodKind::Setter => "set ",
                        MethodKind::Method => "",
                    },
                    prop_name,
                    params
                        .iter()
                        .map(param_display)
                        .collect::<Vec<_>>()
                        .join(", "),
                    returns
                        .as_ref()
                        .map(|r| format!(": {}", r.repr))
                        .unwrap_or_default(),
                );
                let mut references =
                    collect_ts_fn_references(&m.function.params, m.function.return_type.as_deref());
                // Tag methods with `Implements -> I` for each interface
                // declared by parent class — consistent with Rust behavior
                // (`impl Trait for X` tagged on methods).
                for target in implements_targets {
                    references.push(RefKind::Implements, target.clone(), 0);
                }
                let symbol = SymbolInfo {
                    kind,
                    visibility: class_member_vis(m.accessibility),
                    signature,
                    params,
                    returns,
                    generics: vec![],
                    decorators: vec![],
                    is_async: m.function.is_async,
                    is_deprecated: false,
                    references,
                };
                self.push(fqn, symbol, m.span, &prop_name);
            }
            ClassMember::ClassProp(p) => {
                let name = match &p.key {
                    swc_ecma_ast::PropName::Ident(i) => i.sym.to_string(),
                    _ => return,
                };
                let fqn = self.fqn_for(&name);
                let ty_repr = p.type_ann.as_ref().map(|t| type_repr(&t.type_ann));
                let signature = ty_repr
                    .as_ref()
                    .map_or_else(|| name.clone(), |t| format!("{name}: {t}"));
                let mut references = References::default();
                if let Some(t) = &p.type_ann {
                    let mut names = Vec::new();
                    collect_ts_type_idents(&t.type_ann, &mut names);
                    for n in names {
                        references.push(RefKind::FieldType, n, 0);
                    }
                }
                let symbol = SymbolInfo {
                    kind: SymbolKind::Field,
                    visibility: class_member_vis(p.accessibility),
                    signature,
                    params: vec![],
                    returns: ty_repr.map(|r| TypeInfo { repr: r }),
                    generics: vec![],
                    decorators: vec![],
                    is_async: false,
                    is_deprecated: false,
                    references,
                };
                self.push(fqn, symbol, p.span, &name);
            }
            _ => {}
        }
    }

    fn emit_var(&mut self, v: &VarDecl, vis: Visibility) {
        for decl in &v.decls {
            self.emit_var_declarator(decl, vis);
        }
    }

    fn emit_var_declarator(&mut self, d: &VarDeclarator, vis: Visibility) {
        let name = match &d.name {
            swc_ecma_ast::Pat::Ident(i) => i.id.sym.to_string(),
            _ => return,
        };
        let fqn = self.fqn_for(&name);
        // Arrow function assigned to `const`? Treat as function.
        if let Some(init) = &d.init {
            if let Expr::Arrow(arrow) = init.as_ref() {
                let symbol = symbol_from_arrow(&name, arrow, vis);
                self.push(fqn, symbol, d.span, &name);
                return;
            }
        }
        let type_ann_str = match &d.name {
            swc_ecma_ast::Pat::Ident(i) => i.type_ann.as_ref().map(|t| type_repr(&t.type_ann)),
            _ => None,
        };
        let signature = type_ann_str
            .as_ref()
            .map_or_else(|| format!("const {name}"), |t| format!("const {name}: {t}"));
        // Refs: const annotation type -> FieldType (close to a static).
        let mut references = References::default();
        if let swc_ecma_ast::Pat::Ident(i) = &d.name {
            if let Some(t) = &i.type_ann {
                let mut names = Vec::new();
                collect_ts_type_idents(&t.type_ann, &mut names);
                for n in names {
                    references.push(RefKind::FieldType, n, 0);
                }
            }
        }
        let symbol = SymbolInfo {
            kind: SymbolKind::Const,
            visibility: vis,
            signature,
            params: vec![],
            returns: type_ann_str.map(|t| TypeInfo { repr: t }),
            generics: vec![],
            decorators: vec![],
            is_async: false,
            is_deprecated: false,
            references,
        };
        self.push(fqn, symbol, d.span, &name);
    }

    fn emit_interface(&mut self, i: &TsInterfaceDecl, vis: Visibility, outer_span: Span) {
        let name = i.id.sym.to_string();
        let fqn = self.fqn_for(&name);

        // Refs: `extends Foo, Bar` + member types (properties / signatures).
        let mut references = References::default();
        for ext in &i.extends {
            if let Some(target) = expr_short_name(&ext.expr) {
                if !is_ts_primitive_type_name(&target) {
                    references.push(RefKind::Extends, target, 0);
                }
            }
        }
        for member in &i.body.body {
            collect_interface_member_refs(member, &mut references);
        }

        let symbol = SymbolInfo {
            kind: SymbolKind::Interface,
            visibility: vis,
            signature: format!(
                "interface {name}{}",
                type_params_repr(i.type_params.as_deref())
            ),
            params: vec![],
            returns: None,
            generics: vec![],
            decorators: vec![],
            is_async: false,
            is_deprecated: false,
            references,
        };
        self.push(fqn, symbol, outer_span, &i.id.sym);

        self.path.push(name);
        for member in &i.body.body {
            self.emit_interface_member(member);
        }
        self.path.pop();
    }

    fn emit_interface_member(&mut self, member: &swc_ecma_ast::TsTypeElement) {
        use swc_ecma_ast::TsTypeElement as E;
        match member {
            E::TsMethodSignature(m) => {
                let name = match m.key.as_ref() {
                    Expr::Ident(i) => i.sym.to_string(),
                    _ => return,
                };
                let fqn = self.fqn_for(&name);
                let params: Vec<ParamInfo> = m.params.iter().map(param_from_ts_fn_param).collect();
                let returns = m.type_ann.as_ref().map(|t| TypeInfo {
                    repr: type_repr(&t.type_ann),
                });
                let optional = if m.optional { "?" } else { "" };
                let signature = format!(
                    "{name}{optional}{}({}){}",
                    type_params_repr(m.type_params.as_deref()),
                    params
                        .iter()
                        .map(param_display)
                        .collect::<Vec<_>>()
                        .join(", "),
                    returns
                        .as_ref()
                        .map(|r| format!(": {}", r.repr))
                        .unwrap_or_default(),
                );
                let mut references = References::default();
                for p in &m.params {
                    let mut names = Vec::new();
                    collect_ts_fn_param_idents(p, &mut names);
                    for n in names {
                        references.push(RefKind::ParamType, n, 0);
                    }
                }
                if let Some(t) = &m.type_ann {
                    let mut names = Vec::new();
                    collect_ts_type_idents(&t.type_ann, &mut names);
                    for n in names {
                        references.push(RefKind::ReturnType, n, 0);
                    }
                }
                let symbol = SymbolInfo {
                    kind: SymbolKind::Method,
                    visibility: Visibility::Public,
                    signature,
                    params,
                    returns,
                    generics: vec![],
                    decorators: vec![],
                    is_async: false,
                    is_deprecated: false,
                    references,
                };
                self.push(fqn, symbol, m.span, &name);
            }
            E::TsPropertySignature(p) => {
                let name = match p.key.as_ref() {
                    Expr::Ident(i) => i.sym.to_string(),
                    _ => return,
                };
                let fqn = self.fqn_for(&name);
                let ty_repr = p.type_ann.as_ref().map(|t| type_repr(&t.type_ann));
                let optional = if p.optional { "?" } else { "" };
                let signature = ty_repr.as_ref().map_or_else(
                    || format!("{name}{optional}"),
                    |t| format!("{name}{optional}: {t}"),
                );
                let mut references = References::default();
                if let Some(t) = &p.type_ann {
                    let mut names = Vec::new();
                    collect_ts_type_idents(&t.type_ann, &mut names);
                    for n in names {
                        references.push(RefKind::FieldType, n, 0);
                    }
                }
                let symbol = SymbolInfo {
                    kind: SymbolKind::Field,
                    visibility: Visibility::Public,
                    signature,
                    params: vec![],
                    returns: ty_repr.map(|r| TypeInfo { repr: r }),
                    generics: vec![],
                    decorators: vec![],
                    is_async: false,
                    is_deprecated: false,
                    references,
                };
                self.push(fqn, symbol, p.span, &name);
            }
            _ => {}
        }
    }

    fn emit_type_alias(&mut self, t: &TsTypeAliasDecl, vis: Visibility, outer_span: Span) {
        let name = t.id.sym.to_string();
        let fqn = self.fqn_for(&name);
        // Type alias RHS is itself a type — collect its idents as `Other`
        // (generic relationship).
        let mut references = References::default();
        let mut names = Vec::new();
        collect_ts_type_idents(&t.type_ann, &mut names);
        for n in names {
            references.push(RefKind::Other, n, 0);
        }
        let symbol = SymbolInfo {
            kind: SymbolKind::TypeAlias,
            visibility: vis,
            signature: format!("type {} = {}", name, type_repr(&t.type_ann)),
            params: vec![],
            returns: None,
            generics: vec![],
            decorators: vec![],
            is_async: false,
            is_deprecated: false,
            references,
        };
        self.push(fqn, symbol, outer_span, &t.id.sym);
    }

    fn emit_enum(&mut self, e: &TsEnumDecl, vis: Visibility, outer_span: Span) {
        let name = e.id.sym.to_string();
        let fqn = self.fqn_for(&name);
        let symbol = SymbolInfo {
            kind: SymbolKind::Enum,
            visibility: vis,
            signature: format!("enum {name}"),
            params: vec![],
            returns: None,
            generics: vec![],
            decorators: vec![],
            is_async: false,
            is_deprecated: false,
            references: References::default(),
        };
        self.push(fqn, symbol, outer_span, &e.id.sym);
    }

    fn push(&mut self, fqn: Vec<String>, symbol: SymbolInfo, span: Span, name: &str) {
        let leading_comment = self.leading_jsdoc(span);
        self.symbols.push(DiscoveredSymbol {
            fqn,
            symbol,
            source_range: self.span_to_range(span, name),
            leading_comment,
            leading_comment_line_start: None,
        });
    }

    fn leading_jsdoc(&self, span: Span) -> Option<String> {
        let raw = self.comments.get_leading(span.lo)?;
        let doc_blocks: Vec<&Comment> = raw
            .iter()
            .filter(|c| c.kind == CommentKind::Block && c.text.starts_with('*'))
            .collect();
        if doc_blocks.is_empty() {
            return None;
        }
        let mut out = String::new();
        for (i, c) in doc_blocks.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            // JSDoc content is `* body * body * body`, leading `*` already stripped from `text`.
            let lines = c.text.trim_start_matches('*').lines();
            for line in lines {
                let trimmed = line.trim_start();
                let stripped = trimmed
                    .strip_prefix("* ")
                    .or_else(|| trimmed.strip_prefix('*'))
                    .unwrap_or(trimmed);
                out.push_str(stripped);
                out.push('\n');
            }
        }
        let trimmed = out.trim_end().to_owned();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }

    fn span_to_range(&self, span: Span, _name: &str) -> SourceRange {
        span_to_source_range(&self.source_map, span)
    }
}

fn span_to_source_range(sm: &SourceMap, span: Span) -> SourceRange {
    let lo = sm.lookup_char_pos(span.lo);
    let hi = sm.lookup_char_pos(span.hi);
    SourceRange {
        line_start: u32::try_from(lo.line).unwrap_or(0),
        line_end: u32::try_from(hi.line).unwrap_or(0),
        column_start: u32::try_from(lo.col.0).unwrap_or(0).saturating_add(1),
        column_end: u32::try_from(hi.col.0).unwrap_or(0).saturating_add(1),
    }
}

const fn class_member_vis(accessibility: Option<swc_ecma_ast::Accessibility>) -> Visibility {
    match accessibility {
        Some(swc_ecma_ast::Accessibility::Private) => Visibility::Private,
        Some(swc_ecma_ast::Accessibility::Protected) => Visibility::Internal,
        Some(swc_ecma_ast::Accessibility::Public) | None => Visibility::Public,
    }
}

fn symbol_from_arrow(name: &str, arrow: &ArrowExpr, vis: Visibility) -> SymbolInfo {
    let params: Vec<ParamInfo> = arrow.params.iter().map(param_from_pat).collect();
    let returns = arrow.return_type.as_ref().map(|t| TypeInfo {
        repr: type_repr(&t.type_ann),
    });
    let signature = format!(
        "const {} = ({}){} => ...",
        name,
        params
            .iter()
            .map(param_display)
            .collect::<Vec<_>>()
            .join(", "),
        returns
            .as_ref()
            .map(|r| format!(": {}", r.repr))
            .unwrap_or_default()
    );
    let references = collect_ts_arrow_references(&arrow.params, arrow.return_type.as_deref());
    SymbolInfo {
        kind: SymbolKind::Function,
        visibility: vis,
        signature,
        params,
        returns,
        generics: vec![],
        decorators: vec![],
        is_async: arrow.is_async,
        is_deprecated: false,
        references,
    }
}

/// Walk members of `interface { ... }` to collect `FieldType` for properties
/// and `ParamType`/`ReturnType` for methods/signatures.
fn collect_interface_member_refs(member: &swc_ecma_ast::TsTypeElement, refs: &mut References) {
    use swc_ecma_ast::TsTypeElement as E;
    match member {
        E::TsPropertySignature(p) => {
            if let Some(t) = &p.type_ann {
                let mut names = Vec::new();
                collect_ts_type_idents(&t.type_ann, &mut names);
                for n in names {
                    refs.push(RefKind::FieldType, n, 0);
                }
            }
        }
        E::TsMethodSignature(m) => {
            for p in &m.params {
                let mut names = Vec::new();
                collect_ts_fn_param_idents(p, &mut names);
                for n in names {
                    refs.push(RefKind::ParamType, n, 0);
                }
            }
            if let Some(t) = &m.type_ann {
                let mut names = Vec::new();
                collect_ts_type_idents(&t.type_ann, &mut names);
                for n in names {
                    refs.push(RefKind::ReturnType, n, 0);
                }
            }
        }
        E::TsCallSignatureDecl(c) => {
            for p in &c.params {
                let mut names = Vec::new();
                collect_ts_fn_param_idents(p, &mut names);
                for n in names {
                    refs.push(RefKind::ParamType, n, 0);
                }
            }
            if let Some(t) = &c.type_ann {
                let mut names = Vec::new();
                collect_ts_type_idents(&t.type_ann, &mut names);
                for n in names {
                    refs.push(RefKind::ReturnType, n, 0);
                }
            }
        }
        E::TsIndexSignature(i) => {
            if let Some(t) = &i.type_ann {
                let mut names = Vec::new();
                collect_ts_type_idents(&t.type_ann, &mut names);
                for n in names {
                    refs.push(RefKind::FieldType, n, 0);
                }
            }
        }
        _ => {}
    }
}

/// Adapter for `TsFnParam` (different from core module `Param`).
fn collect_ts_fn_param_idents(p: &swc_ecma_ast::TsFnParam, out: &mut Vec<String>) {
    use swc_ecma_ast::TsFnParam as P;
    match p {
        P::Ident(i) => {
            if let Some(t) = &i.type_ann {
                collect_ts_type_idents(&t.type_ann, out);
            }
        }
        P::Rest(r) => {
            if let Some(t) = &r.type_ann {
                collect_ts_type_idents(&t.type_ann, out);
            }
        }
        P::Array(a) => {
            if let Some(t) = &a.type_ann {
                collect_ts_type_idents(&t.type_ann, out);
            }
        }
        P::Object(o) => {
            if let Some(t) = &o.type_ann {
                collect_ts_type_idents(&t.type_ann, out);
            }
        }
    }
}

fn param_from_pat(pat: &swc_ecma_ast::Pat) -> ParamInfo {
    match pat {
        swc_ecma_ast::Pat::Ident(i) => ParamInfo {
            name: i.id.sym.to_string(),
            type_repr: i.type_ann.as_ref().map(|t| type_repr(&t.type_ann)),
            default: None,
            is_optional: i.optional,
            is_variadic: false,
        },
        swc_ecma_ast::Pat::Rest(r) => {
            let mut inner = param_from_pat(&r.arg);
            inner.is_variadic = true;
            inner
        }
        swc_ecma_ast::Pat::Assign(a) => {
            let mut inner = param_from_pat(&a.left);
            inner.default = Some("<default>".to_owned());
            inner.is_optional = true;
            inner
        }
        _ => ParamInfo {
            name: "_".to_owned(),
            type_repr: None,
            default: None,
            is_optional: false,
            is_variadic: false,
        },
    }
}

fn type_params_repr(params: Option<&swc_ecma_ast::TsTypeParamDecl>) -> String {
    let Some(params) = params else {
        return String::new();
    };
    if params.params.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = params.params.iter().map(type_param_repr).collect();
    format!("<{}>", parts.join(", "))
}

fn type_param_repr(p: &swc_ecma_ast::TsTypeParam) -> String {
    let mut s = p.name.sym.to_string();
    if let Some(constraint) = &p.constraint {
        s.push_str(" extends ");
        s.push_str(&type_repr(constraint));
    }
    if let Some(default) = &p.default {
        s.push_str(" = ");
        s.push_str(&type_repr(default));
    }
    s
}

fn param_from_ts_fn_param(p: &swc_ecma_ast::TsFnParam) -> ParamInfo {
    use swc_ecma_ast::TsFnParam as P;
    match p {
        P::Ident(i) => ParamInfo {
            name: i.id.sym.to_string(),
            type_repr: i.type_ann.as_ref().map(|t| type_repr(&t.type_ann)),
            default: None,
            is_optional: i.optional,
            is_variadic: false,
        },
        P::Rest(r) => {
            let inner_name = match r.arg.as_ref() {
                swc_ecma_ast::Pat::Ident(i) => i.id.sym.to_string(),
                _ => "_".to_owned(),
            };
            ParamInfo {
                name: inner_name,
                type_repr: r.type_ann.as_ref().map(|t| type_repr(&t.type_ann)),
                default: None,
                is_optional: false,
                is_variadic: true,
            }
        }
        P::Array(a) => ParamInfo {
            name: "_".to_owned(),
            type_repr: a.type_ann.as_ref().map(|t| type_repr(&t.type_ann)),
            default: None,
            is_optional: false,
            is_variadic: false,
        },
        P::Object(o) => ParamInfo {
            name: "_".to_owned(),
            type_repr: o.type_ann.as_ref().map(|t| type_repr(&t.type_ann)),
            default: None,
            is_optional: false,
            is_variadic: false,
        },
    }
}

fn param_display(p: &ParamInfo) -> String {
    let mut s = if p.is_variadic {
        format!("...{}", p.name)
    } else {
        p.name.clone()
    };
    if p.is_optional {
        s.push('?');
    }
    if let Some(t) = &p.type_repr {
        s.push_str(": ");
        s.push_str(t);
    }
    s
}

// -------- Cross-references extraction --------

/// Primitive type names and common generic built-ins to filter out — otherwise
/// `find_usages("string")` would match the entire codebase.
fn is_ts_primitive_type_name(name: &str) -> bool {
    matches!(
        name,
        "any"
            | "unknown"
            | "number"
            | "object"
            | "boolean"
            | "bigint"
            | "string"
            | "symbol"
            | "void"
            | "undefined"
            | "null"
            | "never"
            | "intrinsic"
            | "Promise"
            | "Array"
            | "ReadonlyArray"
            | "Map"
            | "Set"
            | "ReadonlyMap"
            | "ReadonlySet"
            | "Record"
            | "Partial"
            | "Required"
            | "Readonly"
            | "Pick"
            | "Omit"
            | "Exclude"
            | "Extract"
            | "this"
            | "self"
    )
}

fn collect_ts_type_idents(ty: &swc_ecma_ast::TsType, out: &mut Vec<String>) {
    use swc_ecma_ast::TsType as T;
    match ty {
        T::TsTypeRef(r) => {
            let name = entity_name(&r.type_name);
            // Keep only final segment (e.g. `foo.Bar` -> `Bar`) to match
            // keys by short name.
            let last_segment = name
                .rsplit_once('.')
                .map_or(name.as_str(), |(_, last)| last)
                .to_owned();
            if !is_ts_primitive_type_name(&last_segment) {
                out.push(last_segment);
            }
            if let Some(params) = &r.type_params {
                for p in &params.params {
                    collect_ts_type_idents(p, out);
                }
            }
        }
        T::TsArrayType(a) => collect_ts_type_idents(&a.elem_type, out),
        T::TsTupleType(t) => {
            for el in &t.elem_types {
                collect_ts_type_idents(&el.ty, out);
            }
        }
        T::TsUnionOrIntersectionType(u) => match u {
            swc_ecma_ast::TsUnionOrIntersectionType::TsUnionType(ut) => {
                for t in &ut.types {
                    collect_ts_type_idents(t, out);
                }
            }
            swc_ecma_ast::TsUnionOrIntersectionType::TsIntersectionType(it) => {
                for t in &it.types {
                    collect_ts_type_idents(t, out);
                }
            }
        },
        T::TsParenthesizedType(p) => collect_ts_type_idents(&p.type_ann, out),
        T::TsOptionalType(o) => collect_ts_type_idents(&o.type_ann, out),
        T::TsRestType(r) => collect_ts_type_idents(&r.type_ann, out),
        T::TsConditionalType(c) => {
            collect_ts_type_idents(&c.check_type, out);
            collect_ts_type_idents(&c.extends_type, out);
            collect_ts_type_idents(&c.true_type, out);
            collect_ts_type_idents(&c.false_type, out);
        }
        T::TsIndexedAccessType(i) => {
            collect_ts_type_idents(&i.obj_type, out);
            collect_ts_type_idents(&i.index_type, out);
        }
        T::TsTypeOperator(op) => collect_ts_type_idents(&op.type_ann, out),
        T::TsMappedType(m) => {
            if let Some(t) = &m.type_ann {
                collect_ts_type_idents(t, out);
            }
        }
        // FnOrConstructor types and others: no useful idents at this level,
        // so ignore. Wildcard covers TsTypeLit / TsLitType / TsKeywordType /
        // TsThisType / TsFnOrConstructorType / etc.
        _ => {}
    }
}

fn collect_pat_type_idents(pat: &swc_ecma_ast::Pat, out: &mut Vec<String>) {
    match pat {
        swc_ecma_ast::Pat::Ident(i) => {
            if let Some(t) = &i.type_ann {
                collect_ts_type_idents(&t.type_ann, out);
            }
        }
        swc_ecma_ast::Pat::Rest(r) => collect_pat_type_idents(&r.arg, out),
        swc_ecma_ast::Pat::Assign(a) => collect_pat_type_idents(&a.left, out),
        swc_ecma_ast::Pat::Array(a) => {
            if let Some(t) = &a.type_ann {
                collect_ts_type_idents(&t.type_ann, out);
            }
        }
        swc_ecma_ast::Pat::Object(o) => {
            if let Some(t) = &o.type_ann {
                collect_ts_type_idents(&t.type_ann, out);
            }
        }
        _ => {}
    }
}

/// Outgoing refs for a TS function from params + return type.
fn collect_ts_fn_references(
    params: &[swc_ecma_ast::Param],
    return_type: Option<&swc_ecma_ast::TsTypeAnn>,
) -> References {
    let mut refs = References::default();
    for p in params {
        let mut names = Vec::new();
        collect_pat_type_idents(&p.pat, &mut names);
        for name in names {
            refs.push(RefKind::ParamType, name, 0);
        }
    }
    if let Some(rt) = return_type {
        let mut names = Vec::new();
        collect_ts_type_idents(&rt.type_ann, &mut names);
        for name in names {
            refs.push(RefKind::ReturnType, name, 0);
        }
    }
    refs
}

/// Variant for arrow functions where params are already `Pat`.
fn collect_ts_arrow_references(
    params: &[swc_ecma_ast::Pat],
    return_type: Option<&swc_ecma_ast::TsTypeAnn>,
) -> References {
    let mut refs = References::default();
    for pat in params {
        let mut names = Vec::new();
        collect_pat_type_idents(pat, &mut names);
        for name in names {
            refs.push(RefKind::ParamType, name, 0);
        }
    }
    if let Some(rt) = return_type {
        let mut names = Vec::new();
        collect_ts_type_idents(&rt.type_ann, &mut names);
        for name in names {
            refs.push(RefKind::ReturnType, name, 0);
        }
    }
    refs
}

/// Extract `Implements` / `Extends` from TS class header.
fn collect_class_heritage(class: &swc_ecma_ast::Class) -> References {
    let mut refs = References::default();
    if let Some(super_class) = &class.super_class {
        if let Some(name) = expr_short_name(super_class) {
            refs.push(RefKind::Extends, name, 0);
        }
    }
    for impl_clause in &class.implements {
        if let Some(name) = expr_short_name(&impl_clause.expr) {
            if !is_ts_primitive_type_name(&name) {
                refs.push(RefKind::Implements, name, 0);
            }
        }
    }
    refs
}

fn expr_short_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Ident(i) => Some(i.sym.to_string()),
        Expr::Member(m) => match &m.prop {
            // For `foo.Bar`, keep only `Bar`.
            swc_ecma_ast::MemberProp::Ident(i) => Some(i.sym.to_string()),
            _ => None,
        },
        _ => None,
    }
}

fn type_repr(ty: &swc_ecma_ast::TsType) -> String {
    // Minimal, not pretty — enough to show the canonical shape. Upgrading to
    // a proper printer (e.g. swc_ecma_codegen) can land later.
    use swc_ecma_ast::TsKeywordTypeKind as K;
    use swc_ecma_ast::TsType as T;
    match ty {
        T::TsKeywordType(k) => match k.kind {
            K::TsAnyKeyword => "any",
            K::TsUnknownKeyword => "unknown",
            K::TsNumberKeyword => "number",
            K::TsObjectKeyword => "object",
            K::TsBooleanKeyword => "boolean",
            K::TsBigIntKeyword => "bigint",
            K::TsStringKeyword => "string",
            K::TsSymbolKeyword => "symbol",
            K::TsVoidKeyword => "void",
            K::TsUndefinedKeyword => "undefined",
            K::TsNullKeyword => "null",
            K::TsNeverKeyword => "never",
            K::TsIntrinsicKeyword => "intrinsic",
        }
        .to_owned(),
        T::TsTypeRef(r) => {
            let name = entity_name(&r.type_name);
            r.type_params.as_ref().map_or_else(
                || name.clone(),
                |params| {
                    let args: Vec<String> = params.params.iter().map(|p| type_repr(p)).collect();
                    format!("{name}<{}>", args.join(", "))
                },
            )
        }
        T::TsArrayType(a) => format!("{}[]", type_repr(&a.elem_type)),
        T::TsUnionOrIntersectionType(u) => match u {
            swc_ecma_ast::TsUnionOrIntersectionType::TsUnionType(ut) => ut
                .types
                .iter()
                .map(|t| type_repr(t))
                .collect::<Vec<_>>()
                .join(" | "),
            swc_ecma_ast::TsUnionOrIntersectionType::TsIntersectionType(it) => it
                .types
                .iter()
                .map(|t| type_repr(t))
                .collect::<Vec<_>>()
                .join(" & "),
        },
        T::TsLitType(l) => lit_repr(&l.lit),
        T::TsFnOrConstructorType(_) => "<fn>".to_owned(),
        _ => "<unknown>".to_owned(),
    }
}

fn entity_name(name: &swc_ecma_ast::TsEntityName) -> String {
    match name {
        swc_ecma_ast::TsEntityName::Ident(i) => i.sym.to_string(),
        swc_ecma_ast::TsEntityName::TsQualifiedName(q) => {
            format!("{}.{}", entity_name(&q.left), q.right.sym)
        }
    }
}

fn lit_repr(lit: &swc_ecma_ast::TsLit) -> String {
    use swc_ecma_ast::TsLit;
    match lit {
        TsLit::Str(_) => "\"<string>\"".to_owned(),
        TsLit::Number(n) => n.value.to_string(),
        TsLit::Bool(b) => b.value.to_string(),
        TsLit::BigInt(b) => b.value.to_string(),
        TsLit::Tpl(_) => "<template-lit>".to_owned(),
    }
}

const fn decl_span(decl: &Decl) -> Span {
    match decl {
        Decl::Fn(f) => f.function.span,
        Decl::Class(c) => c.class.span,
        Decl::Var(v) => v.span,
        Decl::TsInterface(i) => i.span,
        Decl::TsTypeAlias(t) => t.span,
        Decl::TsEnum(e) => e.span,
        Decl::TsModule(m) => m.span,
        Decl::Using(u) => u.span,
    }
}

// ─── Entry-point public surface ──────────────────────────────────────────────

/// A single re-export declared in a barrel/index file.
#[derive(Debug)]
pub struct ReExport {
    /// Module specifier (e.g. `"./compile"`, `"../match.ts"`).
    pub specifier: String,
    /// `None` = `export * from` (star). `Some` = list of local names in target.
    pub local_names: Option<Vec<String>>,
}

/// Parse all `export ... from '...'` declarations in a TS/JS file.
/// Does not include `export { local }` forms without a source module.
pub fn parse_module_reexports(content: &str) -> Vec<ReExport> {
    let cm: Lrc<SourceMap> = Lrc::default();
    let fm = cm.new_source_file(
        Lrc::new(FileName::Custom("<reexport-parse>".to_owned())),
        content.to_owned(),
    );
    let lexer = Lexer::new(
        Syntax::Typescript(TsSyntax {
            ..Default::default()
        }),
        EsVersion::EsNext,
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    let module = match parser.parse_module() {
        Ok(m) => m,
        Err(_) => return vec![],
    };
    let mut exports = Vec::new();
    for item in &module.body {
        if let ModuleItem::ModuleDecl(decl) = item {
            match decl {
                ModuleDecl::ExportNamed(named) if named.src.is_some() => {
                    let specifier = named
                        .src
                        .as_ref()
                        .unwrap()
                        .value
                        .to_atom_lossy()
                        .to_string();
                    let local_names: Vec<String> = named
                        .specifiers
                        .iter()
                        .filter_map(|s| {
                            if let swc_ecma_ast::ExportSpecifier::Named(n) = s {
                                Some(match &n.orig {
                                    swc_ecma_ast::ModuleExportName::Ident(i) => i.sym.to_string(),
                                    swc_ecma_ast::ModuleExportName::Str(s) => {
                                        s.value.to_atom_lossy().to_string()
                                    }
                                })
                            } else {
                                None
                            }
                        })
                        .collect();
                    exports.push(ReExport {
                        specifier,
                        local_names: Some(local_names),
                    });
                }
                ModuleDecl::ExportAll(all) => {
                    exports.push(ReExport {
                        specifier: all.src.value.to_atom_lossy().to_string(),
                        local_names: None,
                    });
                }
                _ => {}
            }
        }
    }
    exports
}

/// Resolve a module specifier relative to a directory, trying common TS extensions.
pub fn resolve_specifier(from_dir: &Path, specifier: &str) -> Option<PathBuf> {
    for suffix in &["", ".ts", ".tsx", ".js", ".jsx", "/index.ts", "/index.js"] {
        let candidate = from_dir.join(format!("{specifier}{suffix}"));
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// Build the public surface map starting from a TypeScript entry point.
///
/// Returns a map: canonical absolute path → `None` (all public) or `Some(names)`.
/// Traverses the re-export graph (BFS, no cycle) so barrel chains like
/// `index.ts → types.ts → types/index.ts → Pattern.ts` are followed correctly.
pub fn build_public_surface(entry_path: &Path) -> HashMap<PathBuf, Option<HashSet<String>>> {
    let mut surface: HashMap<PathBuf, Option<HashSet<String>>> = HashMap::new();
    let mut queue: std::collections::VecDeque<(PathBuf, Option<HashSet<String>>)> =
        std::collections::VecDeque::new();
    let mut visited: HashSet<PathBuf> = HashSet::new();

    let Ok(canonical_entry) = entry_path.canonicalize() else {
        return surface;
    };
    queue.push_back((canonical_entry, None));

    while let Some((path, wanted)) = queue.pop_front() {
        if visited.contains(&path) {
            continue;
        }
        visited.insert(path.clone());

        // Merge into surface: None (star) dominates over Some(set).
        let slot = surface
            .entry(path.clone())
            .or_insert_with(|| wanted.clone());
        match (&mut *slot, &wanted) {
            (slot @ Some(_), None) => *slot = None,
            (Some(existing), Some(new_names)) => existing.extend(new_names.iter().cloned()),
            _ => {}
        }

        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let re_exports = parse_module_reexports(&content);
        let dir = path.parent().unwrap_or(&path);

        for re_export in re_exports {
            let Some(resolved) = resolve_specifier(dir, &re_export.specifier) else {
                continue;
            };
            let Ok(canonical) = resolved.canonicalize() else {
                continue;
            };
            if visited.contains(&canonical) {
                continue;
            }

            let to_propagate: Option<HashSet<String>> = match (&wanted, &re_export.local_names) {
                // parent is star → propagate whatever the re-export says
                (None, None) => None,
                (None, Some(local)) => Some(local.iter().cloned().collect()),
                // parent is named → only propagate names the parent wants
                (Some(parent_set), None) => Some(parent_set.clone()),
                (Some(parent_set), Some(local)) => {
                    let intersection: HashSet<String> = local
                        .iter()
                        .filter(|n| parent_set.contains(*n))
                        .cloned()
                        .collect();
                    if intersection.is_empty() {
                        continue;
                    }
                    Some(intersection)
                }
            };

            queue.push_back((canonical, to_propagate));
        }
    }

    surface
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_exported_function() {
        let src = r"
            /** Adds two numbers. */
            export function add(a: number, b: number): number {
                return a + b;
            }
        ";
        let p = TsProvider;
        let symbols = p.discover_symbols(src, Path::new("lib.ts")).unwrap();
        assert_eq!(symbols.len(), 1);
        let s = &symbols[0];
        assert_eq!(s.fqn, vec!["add".to_owned()]);
        assert_eq!(s.symbol.kind, SymbolKind::Function);
        assert_eq!(s.symbol.visibility, Visibility::Public);
        assert_eq!(s.symbol.params.len(), 2);
        assert_eq!(s.symbol.params[0].name, "a");
        assert_eq!(s.symbol.params[0].type_repr.as_deref(), Some("number"));
        assert!(s
            .leading_comment
            .as_deref()
            .unwrap()
            .contains("Adds two numbers"));
    }

    #[test]
    fn discovers_class_and_methods() {
        let src = r"
            export class Calculator {
                /** Adds. */
                add(a: number, b: number): number { return a + b; }
                private internal(): void {}
            }
        ";
        let p = TsProvider;
        let symbols = p.discover_symbols(src, Path::new("lib.ts")).unwrap();
        let class = symbols
            .iter()
            .find(|s| s.symbol.kind == SymbolKind::Class)
            .unwrap();
        assert_eq!(class.fqn, vec!["Calculator"]);
        assert_eq!(class.symbol.visibility, Visibility::Public);
        let methods: Vec<_> = symbols
            .iter()
            .filter(|s| s.symbol.kind == SymbolKind::Method)
            .collect();
        assert_eq!(methods.len(), 2);
        let add = methods
            .iter()
            .find(|s| s.fqn.last().unwrap() == "add")
            .unwrap();
        assert_eq!(add.fqn, vec!["Calculator", "add"]);
        assert_eq!(add.symbol.visibility, Visibility::Public);
        let internal = methods
            .iter()
            .find(|s| s.fqn.last().unwrap() == "internal")
            .unwrap();
        assert_eq!(internal.symbol.visibility, Visibility::Private);
    }

    #[test]
    fn jsdoc_adjacent_to_class_attaches_correctly() {
        // Positive case: when the JSDoc sits immediately above the class with
        // no intervening line, swc attaches it via `comments.get_leading`. The
        // pipeline's `symbol_starts.contains(line_end + 1)` filter then
        // suppresses the free-floating-anchor pass. No STD001-style duplicate
        // can happen here — equivalent of the Rust adjacent case.
        let src = "/** @doc some.thing Label */\n\
                   export class Foo {}\n";
        let p = TsProvider;
        let symbols = p.discover_symbols(src, Path::new("test.ts")).unwrap();
        let class = symbols
            .iter()
            .find(|s| s.fqn == vec!["Foo".to_owned()])
            .expect("class Foo");
        let comment = class
            .leading_comment
            .as_deref()
            .expect("jsdoc must attach to the class");
        assert!(comment.contains("@doc some.thing"), "got: {comment:?}");
        assert_eq!(class.leading_comment_line_start, None);
    }

    #[test]
    fn jsdoc_with_decorator_between_is_currently_orphaned() {
        // Documented limitation: when a class decorator (`@Injectable()`,
        // `@Component({...})`, etc.) sits between the JSDoc and the class
        // declaration, swc's `comments.get_leading(class.span.lo)` does NOT
        // pick up the JSDoc — `ExportDecl.span` starts at `export`, not at the
        // decorator. The JSDoc therefore becomes a free-floating-anchor block
        // instead of a hybrid on the class symbol. This is a separate bug from
        // the Rust STD001 duplicate (Rust syn attaches `///` to the symbol via
        // `#[doc=...]` regardless of attributes — TS swc does not).
        //
        // Tracked in `notes/DSL-RENDER-FEATURES.md`. Fix path: extend
        // `leading_jsdoc` to walk past `Class.decorators` spans when looking
        // up leading comments, and emit `leading_comment_line_start` so the
        // pipeline filter skips the consumed range.
        let src = "/** @doc some.thing Label */\n\
                   @Injectable()\n\
                   export class Foo {}\n";
        let p = TsProvider;
        let symbols = p.discover_symbols(src, Path::new("test.ts")).unwrap();
        let class = symbols
            .iter()
            .find(|s| s.fqn == vec!["Foo".to_owned()])
            .expect("class Foo (decorators must parse)");
        assert!(
            class.leading_comment.is_none(),
            "current TS behavior: jsdoc separated by decorator stays orphaned; \
             got attached comment: {:?}",
            class.leading_comment
        );
        assert_eq!(class.leading_comment_line_start, None);
    }

    #[test]
    fn discovers_interface_and_type_alias() {
        let src = r"
            export interface User { id: number; name: string; }
            export type Handler = (n: number) => void;
        ";
        let p = TsProvider;
        let symbols = p.discover_symbols(src, Path::new("lib.ts")).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.symbol.kind == SymbolKind::Interface && s.fqn == vec!["User"]));
        assert!(symbols
            .iter()
            .any(|s| s.symbol.kind == SymbolKind::TypeAlias && s.fqn == vec!["Handler"]));
    }

    #[test]
    fn arrow_const_is_treated_as_function() {
        let src = r"
            export const greet = (name: string): string => `hi ${name}`;
        ";
        let p = TsProvider;
        let symbols = p.discover_symbols(src, Path::new("lib.ts")).unwrap();
        assert_eq!(symbols.len(), 1);
        let s = &symbols[0];
        assert_eq!(s.fqn, vec!["greet"]);
        assert_eq!(s.symbol.kind, SymbolKind::Function);
        assert_eq!(s.symbol.params.len(), 1);
    }
}

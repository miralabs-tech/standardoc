use standardoc_ir::{
    AliasMutability, BindingSource, IdentResolution, ImportRecord, Language, LocalDeclKind,
    ModuleLookup, ScopeKind, ScopeRange,
};
use swc_core::common::BytePos;
use swc_core::ecma::ast::{
    ArrowExpr, BlockStmt, CatchClause, Class, ClassDecl, Constructor, Decl, DefaultDecl,
    ExportDecl, ExportDefaultDecl, ExportSpecifier, Expr, FnDecl, ForInStmt, ForOfStmt, ForStmt,
    Function, ImportDecl, ImportSpecifier, MemberExpr, Module, ModuleDecl, ModuleExportName,
    ModuleItem, NamedExport, ObjectPatProp, ParamOrTsParamProp, Pat, Stmt, TsEnumDecl,
    TsInterfaceDecl, TsParamPropParam, TsTypeAliasDecl, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

/// Build the AOT identifier-resolution table for a TS/JS module.
///
/// Two-pass design:
/// 1. `hoist_module_items` — top-level imports + hoisted declarations
///    (functions, classes, interfaces, enums, type aliases, var/let/const)
///    are pushed into the ROOT scope before any traversal. This matches
///    JS hoisting semantics where forward refs are legal at module level.
/// 2. `module.visit_with(builder)` — full AST walk for nested scopes
///    (function bodies, blocks, for-loops, catch clauses, classes) and
///    bindings introduced at the point of declaration inside those scopes
///    (var/let/const, function params, catch params, type params).
///
/// Imports are also flattened into `ModuleLookup.imports` for the Stage
/// 3b cross-workspace SQL join.
pub(crate) fn build_ts_lookup(module: &Module, module_fqdn: &str) -> ModuleLookup {
    let mut lookup = ModuleLookup::new(module_fqdn.to_string(), Language::TypeScript);
    let mut builder = LookupBuilder {
        lookup: &mut lookup,
        scope_stack: vec![ModuleLookup::ROOT_SCOPE],
    };
    builder.hoist_module_items(&module.body);
    module.visit_with(&mut builder);
    lookup
}

struct LookupBuilder<'a> {
    lookup: &'a mut ModuleLookup,
    scope_stack: Vec<u32>,
}

impl LookupBuilder<'_> {
    fn current_scope(&self) -> u32 {
        *self.scope_stack.last().unwrap_or(&ModuleLookup::ROOT_SCOPE)
    }

    fn push_scope(&mut self, kind: ScopeKind, lo: BytePos, hi: BytePos) {
        let parent = Some(self.current_scope());
        let idx = self.lookup.push_scope_with_span(
            ScopeRange {
                start_line: lo.0,
                end_line: hi.0,
                parent,
                kind,
            },
            lo.0,
            hi.0,
        );
        self.scope_stack.push(idx);
    }

    fn pop_scope(&mut self) {
        self.scope_stack.pop();
    }

    fn add_binding(&mut self, name: String, source: BindingSource, attributes: Vec<String>) {
        let scope_idx = self.current_scope();
        self.lookup.push_binding(IdentResolution {
            name,
            source,
            resolved_fqdn: None,
            aliases_to: None,
            mutability: None,
            scope_idx,
            attributes,
            ir_kind: None,
        });
    }

    fn add_aliased_binding(
        &mut self,
        name: String,
        decl_kind: LocalDeclKind,
        mutability: AliasMutability,
        aliases_to: String,
    ) {
        let scope_idx = self.current_scope();
        self.lookup.push_binding(IdentResolution {
            name,
            source: BindingSource::LocalDecl { decl_kind },
            resolved_fqdn: None,
            aliases_to: Some(aliases_to),
            mutability: Some(mutability),
            scope_idx,
            attributes: vec![mutability.as_slug().to_string()],
            ir_kind: None,
        });
    }

    fn hoist_module_items(&mut self, items: &[ModuleItem]) {
        for item in items {
            match item {
                ModuleItem::Stmt(stmt) => self.hoist_stmt(stmt),
                ModuleItem::ModuleDecl(decl) => match decl {
                    ModuleDecl::Import(import) => self.add_import(import),
                    ModuleDecl::ExportDecl(ExportDecl { decl, .. }) => self.hoist_decl(decl),
                    ModuleDecl::ExportDefaultDecl(ExportDefaultDecl { decl, .. }) => {
                        self.hoist_default_decl(decl);
                    }
                    ModuleDecl::ExportNamed(named) => self.record_export_named(named),
                    _ => {}
                },
            }
        }
    }

    fn hoist_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::Decl(decl) = stmt {
            self.hoist_decl(decl);
        }
    }

    fn hoist_decl(&mut self, decl: &Decl) {
        match decl {
            Decl::Fn(fn_decl) => {
                self.add_binding(
                    fn_decl.ident.sym.to_string(),
                    BindingSource::LocalDecl {
                        decl_kind: LocalDeclKind::Function,
                    },
                    vec![],
                );
            }
            Decl::Class(class_decl) => {
                self.add_binding(
                    class_decl.ident.sym.to_string(),
                    BindingSource::LocalDecl {
                        decl_kind: LocalDeclKind::Class,
                    },
                    vec![],
                );
            }
            Decl::TsInterface(decl) => {
                self.add_binding(
                    decl.id.sym.to_string(),
                    BindingSource::LocalDecl {
                        decl_kind: LocalDeclKind::Interface,
                    },
                    vec![],
                );
            }
            Decl::TsEnum(decl) => {
                self.add_binding(
                    decl.id.sym.to_string(),
                    BindingSource::LocalDecl {
                        decl_kind: LocalDeclKind::Enum,
                    },
                    vec![],
                );
            }
            Decl::TsTypeAlias(decl) => {
                self.add_binding(
                    decl.id.sym.to_string(),
                    BindingSource::LocalDecl {
                        decl_kind: LocalDeclKind::TypeAlias,
                    },
                    vec![],
                );
            }
            Decl::Var(var_decl) => self.hoist_var_decl(var_decl),
            Decl::TsModule(_) | Decl::Using(_) => {
                // MVP: namespaces + using decls not hoisted in depth here —
                // Stage 4 refinement once we see them in concrete codebases.
            }
        }
    }

    fn hoist_default_decl(&mut self, decl: &DefaultDecl) {
        match decl {
            DefaultDecl::Class(class_expr) => {
                if let Some(ident) = &class_expr.ident {
                    self.add_binding(
                        ident.sym.to_string(),
                        BindingSource::LocalDecl {
                            decl_kind: LocalDeclKind::Class,
                        },
                        vec![],
                    );
                }
            }
            DefaultDecl::Fn(fn_expr) => {
                if let Some(ident) = &fn_expr.ident {
                    self.add_binding(
                        ident.sym.to_string(),
                        BindingSource::LocalDecl {
                            decl_kind: LocalDeclKind::Function,
                        },
                        vec![],
                    );
                }
            }
            DefaultDecl::TsInterfaceDecl(decl) => {
                self.add_binding(
                    decl.id.sym.to_string(),
                    BindingSource::LocalDecl {
                        decl_kind: LocalDeclKind::Interface,
                    },
                    vec![],
                );
            }
        }
    }

    fn hoist_var_decl(&mut self, var_decl: &VarDecl) {
        let decl_kind = var_decl_kind_to_local(var_decl.kind);
        for declarator in &var_decl.decls {
            self.bind_var_declarator(declarator, &decl_kind);
        }
    }

    fn bind_var_declarator(&mut self, declarator: &VarDeclarator, decl_kind: &LocalDeclKind) {
        // Alias detection: `const x = FOO` or `const x = obj.prop` captures
        // the leftmost base identifier as the aliased target.
        if let Some(init) = declarator.init.as_deref()
            && let Some(base) = resolve_alias_rhs(init)
            && let Pat::Ident(ident) = &declarator.name
        {
            let mutability = match decl_kind {
                LocalDeclKind::Const => AliasMutability::Const,
                _ => AliasMutability::Mutable,
            };
            self.add_aliased_binding(
                ident.id.sym.to_string(),
                decl_kind.clone(),
                mutability,
                base,
            );
            return;
        }
        self.bind_pat(
            &declarator.name,
            BindingSource::LocalDecl {
                decl_kind: decl_kind.clone(),
            },
            vec![],
        );
    }

    fn bind_pat(&mut self, pat: &Pat, source: BindingSource, extra_attrs: Vec<String>) {
        match pat {
            Pat::Ident(ident) => {
                self.add_binding(ident.id.sym.to_string(), source, extra_attrs);
            }
            Pat::Array(array) => {
                let mut attrs = extra_attrs;
                attrs.push("unhandled-destructuring".into());
                for elem in array.elems.iter().flatten() {
                    self.bind_pat(elem, source.clone(), attrs.clone());
                }
            }
            Pat::Object(obj) => {
                let mut attrs = extra_attrs;
                attrs.push("unhandled-destructuring".into());
                for prop in &obj.props {
                    match prop {
                        ObjectPatProp::KeyValue(kv) => {
                            self.bind_pat(&kv.value, source.clone(), attrs.clone());
                        }
                        ObjectPatProp::Assign(assign) => {
                            self.add_binding(
                                assign.key.sym.to_string(),
                                source.clone(),
                                attrs.clone(),
                            );
                        }
                        ObjectPatProp::Rest(rest) => {
                            self.bind_pat(&rest.arg, source.clone(), attrs.clone());
                        }
                    }
                }
            }
            Pat::Rest(rest) => {
                self.bind_pat(&rest.arg, source, extra_attrs);
            }
            Pat::Assign(assign) => {
                self.bind_pat(&assign.left, source, extra_attrs);
            }
            Pat::Invalid(_) | Pat::Expr(_) => {}
        }
    }

    fn add_import(&mut self, decl: &ImportDecl) {
        let module_path = decl.src.value.to_string_lossy().into_owned();
        let decl_type_only = decl.type_only;
        for spec in &decl.specifiers {
            let (local_name, original_name, spec_type_only) = match spec {
                ImportSpecifier::Named(named) => {
                    let original = named
                        .imported
                        .as_ref()
                        .map(|module_export| match module_export {
                            ModuleExportName::Ident(ident) => ident.sym.to_string(),
                            ModuleExportName::Str(s) => s.value.to_string_lossy().into_owned(),
                        })
                        .or_else(|| Some(named.local.sym.to_string()));
                    (named.local.sym.to_string(), original, named.is_type_only)
                }
                ImportSpecifier::Default(default) => (default.local.sym.to_string(), None, false),
                ImportSpecifier::Namespace(ns) => (ns.local.sym.to_string(), None, false),
            };
            let is_type_only = decl_type_only || spec_type_only;
            self.add_binding(
                local_name.clone(),
                BindingSource::Import {
                    module_path: module_path.clone(),
                    original_name: original_name.clone(),
                    is_type_only,
                    is_re_export: false,
                },
                if is_type_only {
                    vec!["type-only".into()]
                } else {
                    vec![]
                },
            );
            self.lookup.push_import(ImportRecord {
                local_name,
                origin_module: module_path.clone(),
                origin_symbol: original_name,
                is_type_only,
                is_re_export: false,
            });
        }
    }

    fn record_export_named(&mut self, named: &NamedExport) {
        // Re-exports (`export { x } from "..."`) introduce bindings that
        // originate from another module — record both the import-like
        // binding (so the resolver can chase the origin) and the flat
        // ImportRecord for Stage 3b.
        let Some(src) = named.src.as_deref() else {
            return;
        };
        let module_path = src.value.to_string_lossy().into_owned();
        let decl_type_only = named.type_only;
        for spec in &named.specifiers {
            if let ExportSpecifier::Named(named_spec) = spec {
                let local_export_name = match &named_spec.orig {
                    ModuleExportName::Ident(i) => i.sym.to_string(),
                    ModuleExportName::Str(s) => s.value.to_string_lossy().into_owned(),
                };
                let local_alias = named_spec.exported.as_ref().map_or_else(
                    || local_export_name.clone(),
                    |m| match m {
                        ModuleExportName::Ident(i) => i.sym.to_string(),
                        ModuleExportName::Str(s) => s.value.to_string_lossy().into_owned(),
                    },
                );
                let is_type_only = decl_type_only || named_spec.is_type_only;
                self.add_binding(
                    local_alias.clone(),
                    BindingSource::Import {
                        module_path: module_path.clone(),
                        original_name: Some(local_export_name.clone()),
                        is_type_only,
                        is_re_export: true,
                    },
                    if is_type_only {
                        vec!["type-only".into(), "re-export".into()]
                    } else {
                        vec!["re-export".into()]
                    },
                );
                self.lookup.push_import(ImportRecord {
                    local_name: local_alias,
                    origin_module: module_path.clone(),
                    origin_symbol: Some(local_export_name),
                    is_type_only,
                    is_re_export: true,
                });
            }
        }
    }
}

impl Visit for LookupBuilder<'_> {
    fn visit_function(&mut self, node: &Function) {
        self.push_scope(ScopeKind::Function, node.span.lo, node.span.hi);
        bind_type_params_from_function(self, node);
        for param in &node.params {
            self.bind_pat(&param.pat, BindingSource::Param, vec![]);
        }
        node.visit_children_with(self);
        self.pop_scope();
    }

    fn visit_arrow_expr(&mut self, node: &ArrowExpr) {
        self.push_scope(ScopeKind::Function, node.span.lo, node.span.hi);
        for tp in node.type_params.iter().flat_map(|t| &t.params) {
            self.add_binding(tp.name.sym.to_string(), BindingSource::TypeParam, vec![]);
        }
        for param in &node.params {
            self.bind_pat(param, BindingSource::Param, vec![]);
        }
        node.visit_children_with(self);
        self.pop_scope();
    }

    fn visit_block_stmt(&mut self, node: &BlockStmt) {
        self.push_scope(ScopeKind::Block, node.span.lo, node.span.hi);
        node.visit_children_with(self);
        self.pop_scope();
    }

    fn visit_class(&mut self, node: &Class) {
        self.push_scope(ScopeKind::TypeContainer, node.span.lo, node.span.hi);
        for tp in node.type_params.iter().flat_map(|t| &t.params) {
            self.add_binding(tp.name.sym.to_string(), BindingSource::TypeParam, vec![]);
        }
        node.visit_children_with(self);
        self.pop_scope();
    }

    /// Constructor envelope — mirror of `visit_function` for SWC's
    /// `Constructor` node (distinct AST type from `Function`, no
    /// `return_type`, no `type_params`). Params are `ParamOrTsParamProp`:
    /// plain `Param` follows the usual `bind_pat` path, while the
    /// `TsParamProp` shorthand (`constructor(private readonly db: Db)`)
    /// also seeds a binding for the ident — tagged with the
    /// `"param-property"` attribute so consumers can recognise the
    /// double-duty (param + implicit `this.db = db` assignment).
    fn visit_constructor(&mut self, node: &Constructor) {
        self.push_scope(ScopeKind::Function, node.span.lo, node.span.hi);
        for param in &node.params {
            match param {
                ParamOrTsParamProp::Param(p) => {
                    self.bind_pat(&p.pat, BindingSource::Param, vec![]);
                }
                ParamOrTsParamProp::TsParamProp(prop) => match &prop.param {
                    TsParamPropParam::Ident(id) => self.add_binding(
                        id.id.sym.to_string(),
                        BindingSource::Param,
                        vec!["param-property".into()],
                    ),
                    TsParamPropParam::Assign(assign) => self.bind_pat(
                        &assign.left,
                        BindingSource::Param,
                        vec!["param-property".into()],
                    ),
                },
            }
        }
        node.visit_children_with(self);
        self.pop_scope();
    }

    fn visit_class_decl(&mut self, node: &ClassDecl) {
        // Class name is hoisted at module level by `hoist_module_items`.
        // Just descend so `visit_class` handles type params + body scope.
        node.visit_children_with(self);
    }

    fn visit_for_stmt(&mut self, node: &ForStmt) {
        self.push_scope(ScopeKind::Loop, node.span.lo, node.span.hi);
        node.visit_children_with(self);
        self.pop_scope();
    }

    fn visit_for_in_stmt(&mut self, node: &ForInStmt) {
        self.push_scope(ScopeKind::Loop, node.span.lo, node.span.hi);
        node.visit_children_with(self);
        self.pop_scope();
    }

    fn visit_for_of_stmt(&mut self, node: &ForOfStmt) {
        self.push_scope(ScopeKind::Loop, node.span.lo, node.span.hi);
        node.visit_children_with(self);
        self.pop_scope();
    }

    fn visit_catch_clause(&mut self, node: &CatchClause) {
        self.push_scope(ScopeKind::Catch, node.span.lo, node.span.hi);
        if let Some(pat) = &node.param {
            self.bind_pat(pat, BindingSource::Param, vec![]);
        }
        node.visit_children_with(self);
        self.pop_scope();
    }

    fn visit_var_decl(&mut self, node: &VarDecl) {
        // Top-level (ROOT_SCOPE) var/let/const were already hoisted in the
        // first pass — skip re-binding to avoid duplicates. Nested ones
        // bind at the current scope at the point of declaration.
        if self.current_scope() != ModuleLookup::ROOT_SCOPE {
            let decl_kind = var_decl_kind_to_local(node.kind);
            for declarator in &node.decls {
                self.bind_var_declarator(declarator, &decl_kind);
            }
        }
        node.visit_children_with(self);
    }

    fn visit_fn_decl(&mut self, node: &FnDecl) {
        // Nested fn decls bind at the current scope (their name); top-level
        // is hoisted already.
        if self.current_scope() != ModuleLookup::ROOT_SCOPE {
            self.add_binding(
                node.ident.sym.to_string(),
                BindingSource::LocalDecl {
                    decl_kind: LocalDeclKind::Function,
                },
                vec![],
            );
        }
        node.visit_children_with(self);
    }

    fn visit_ts_type_alias_decl(&mut self, node: &TsTypeAliasDecl) {
        // Top-level already hoisted; nested would land here. Type params
        // bind in the alias's own scope.
        self.push_scope(ScopeKind::TypeContainer, node.span.lo, node.span.hi);
        for tp in node.type_params.iter().flat_map(|t| &t.params) {
            self.add_binding(tp.name.sym.to_string(), BindingSource::TypeParam, vec![]);
        }
        node.visit_children_with(self);
        self.pop_scope();
    }

    fn visit_ts_interface_decl(&mut self, node: &TsInterfaceDecl) {
        self.push_scope(ScopeKind::TypeContainer, node.span.lo, node.span.hi);
        for tp in node.type_params.iter().flat_map(|t| &t.params) {
            self.add_binding(tp.name.sym.to_string(), BindingSource::TypeParam, vec![]);
        }
        node.visit_children_with(self);
        self.pop_scope();
    }

    fn visit_ts_enum_decl(&mut self, node: &TsEnumDecl) {
        // Enum members are sub-symbols (Bug C-1) — they are bindings inside
        // the enum's own scope so qualified refs `Color.Red` can resolve.
        self.push_scope(ScopeKind::TypeContainer, node.span.lo, node.span.hi);
        for member in &node.members {
            let name = ts_enum_member_id_name(&member.id);
            self.add_binding(
                name,
                BindingSource::LocalDecl {
                    decl_kind: LocalDeclKind::Const,
                },
                vec!["enum-member".into()],
            );
        }
        node.visit_children_with(self);
        self.pop_scope();
    }
}

fn bind_type_params_from_function(builder: &mut LookupBuilder<'_>, node: &Function) {
    for tp in node.type_params.iter().flat_map(|t| &t.params) {
        builder.add_binding(tp.name.sym.to_string(), BindingSource::TypeParam, vec![]);
    }
}

const fn var_decl_kind_to_local(kind: VarDeclKind) -> LocalDeclKind {
    match kind {
        VarDeclKind::Var => LocalDeclKind::Var,
        VarDeclKind::Let => LocalDeclKind::Let,
        VarDeclKind::Const => LocalDeclKind::Const,
    }
}

fn ts_enum_member_id_name(id: &swc_core::ecma::ast::TsEnumMemberId) -> String {
    match id {
        swc_core::ecma::ast::TsEnumMemberId::Ident(i) => i.sym.to_string(),
        swc_core::ecma::ast::TsEnumMemberId::Str(s) => s.value.to_string_lossy().into_owned(),
    }
}

/// Leftmost-base ident of an alias RHS expression. Mirrors the Stage 2
/// `resolve_alias_rhs` heuristic: `FOO` / `obj.prop` / `obj.a.b` /
/// `obj[x]` all alias to `FOO` / `obj` / `obj` / `obj`. Returns `None`
/// for non-aliasable RHS (calls, literals, function expressions, etc.).
fn resolve_alias_rhs(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Ident(ident) => Some(ident.sym.to_string()),
        Expr::Member(MemberExpr { obj, .. }) => resolve_alias_rhs(obj),
        _ => None,
    }
}

#[cfg(test)]
mod tests;

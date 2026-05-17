use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use standardoc_ir::{
    Blake3Hash, BuiltinTag, BuiltinTier, EdgeKind, Kind, Language, LanguageKind, Modifiers,
    ModuleLookup, Param, RawCallSite, RawDocument, RawEdge, RawSymbol, ResolvedOrUnresolved,
    Signature, SignatureMeta, Site, SymbolLocation, TypeRef, Visibility,
};
use swc_core::common::BytePos;
use swc_core::common::comments::SingleThreadedComments;
use swc_core::common::errors::SourceMapper;
use swc_core::common::{SourceMap, Span, Spanned, sync::Lrc};
use swc_core::ecma::ast::{
    Class, ClassDecl, ClassMember, ClassMethod, Decl, DefaultDecl, ExportDecl, ExportDefaultDecl,
    FnDecl, ImportDecl, ImportSpecifier, MemberProp, Module, ModuleDecl, ModuleExportName,
    ModuleItem, Param as AstParam, Pat, Stmt, TsEnumDecl, TsInterfaceDecl, TsTypeAliasDecl,
    VarDecl, VarDeclarator,
};

use super::extract_doc;
use super::helpers::map_access_modifier;
use super::lookup::build_ts_lookup;
use super::resolver::{TsConfigPaths, resolve_import};
use super::visit;
use crate::builtins::global as global_builtin_registry;
use crate::walk_core::WalkContextCore;

/// Per-file walker state for the TS/JS provider.
///
/// Composes `WalkContextCore` (file path / module FQDN / symbols / edges /
/// defined_fqdns) with TS-specific resolution state: the owning package
/// name (FQDN namespace), an import alias-table, and the swc inputs we need
/// to compute spans / body hashes / import resolution from inside the walk.
pub(crate) struct TsWalkContext<'a> {
    pub(crate) core: WalkContextCore,
    pub(crate) package_name: String,
    pub(crate) import_aliases: HashMap<String, ResolvedImport>,
    pub(crate) cm: Lrc<SourceMap>,
    pub(crate) from_file_abs_path: PathBuf,
    pub(crate) package_root: PathBuf,
    pub(crate) tsconfig: Option<TsConfigPaths>,
    pub(crate) comments: &'a SingleThreadedComments,
    /// Stage 3e-1b — flags accumulated from Attribute-tier builtin hits
    /// while walking. Keyed by the source symbol's FQDN (the enclosing
    /// function / class FQDN that owns the touched type / call / ref).
    /// Flushed into each symbol's `flags` vec at `into_outputs`.
    pub(crate) attribute_flags: HashMap<String, HashSet<String>>,
}

impl<'a> TsWalkContext<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        file_path: String,
        package_name: String,
        file_module_fqdn: String,
        cm: Lrc<SourceMap>,
        from_file_abs_path: PathBuf,
        package_root: PathBuf,
        tsconfig: Option<TsConfigPaths>,
        comments: &'a SingleThreadedComments,
    ) -> Self {
        Self {
            core: WalkContextCore::new(file_path, file_module_fqdn, Language::TypeScript),
            package_name,
            import_aliases: HashMap::new(),
            cm,
            from_file_abs_path,
            package_root,
            tsconfig,
            comments,
            attribute_flags: HashMap::new(),
        }
    }

    pub(crate) fn push_symbol(&mut self, sym: RawSymbol) {
        self.core.push_symbol(sym);
    }

    pub(crate) fn push_edge(&mut self, edge: RawEdge) {
        self.core.push_edge(edge);
    }

    pub(crate) fn push_document(&mut self, doc: RawDocument) {
        self.core.push_document(doc);
    }

    pub(crate) fn push_call_site(&mut self, cs: RawCallSite) {
        self.core.push_call_site(cs);
    }

    /// Push the symbol and, if a JSDoc block precedes `attached_pos`, also
    /// push a `RawDocument` keyed by the symbol's FQDN. swc attaches leading
    /// comments to the `BytePos` of the first significant token after the
    /// comment, so callers must pass the OUTERMOST span of the wrapping item
    /// (e.g. `ExportDecl.span.lo`, not `FnDecl.function.span.lo`).
    pub(crate) fn push_symbol_with_doc(&mut self, sym: RawSymbol, attached_pos: BytePos) {
        let fqdn = sym.fqdn.clone();
        self.push_symbol(sym);
        if let Some(description) = extract_doc::extract_at(self.comments, attached_pos) {
            self.push_document(RawDocument {
                symbol_fqdn: fqdn,
                description,
            });
        }
    }

    pub(crate) fn add_import_alias(&mut self, local_name: String, resolved: ResolvedImport) {
        self.import_aliases.insert(local_name, resolved);
    }

    /// Resolve a single-ident call target through the alias-table, then through
    /// `<current_module_fqdn>::<name>` against `defined_fqdns`, then through
    /// the language's [`BuiltinRegistry`] (Stage 3a-6b chain). Multi-segment
    /// member-expression calls (`obj.method`) are handled separately by the
    /// visitor (always Unresolved by method ident, day-1).
    ///
    /// Stage 3e-1 / 3e-1b: the return type is a 3-variant
    /// [`ResolutionOutcome`] so callers can distinguish the three skip
    /// semantics introduced by tier classification :
    ///
    /// - [`ResolutionOutcome::Drop`] — structural noise (`Array`,
    ///   `Map`, `undefined`, …). Caller silently skips ; nothing to
    ///   record.
    /// - [`ResolutionOutcome::Attribute(tag)`] — semantic effect on
    ///   the *source* symbol (`Promise` → `"async"` flag, `Iterator`
    ///   → `"iter"` flag). Caller skips the edge and registers the
    ///   tag against the enclosing FQDN via
    ///   [`TsWalkContext::register_attribute_flag`].
    /// - [`ResolutionOutcome::Emit(target)`] — emit the edge.
    ///   `target.via_builtin` is `Some(tag)` for Edge-tier builtins
    ///   so callers can stamp `via-builtin` / `builtin-<slug>` attrs.
    pub(crate) fn resolve_call(&self, name: &str, current_module_fqdn: &str) -> ResolutionOutcome {
        if let Some(import) = self.import_aliases.get(name) {
            return ResolutionOutcome::Emit(CallTarget::plain(import.target.clone()));
        }
        let local = format!("{current_module_fqdn}::{name}");
        if self.core.defined_fqdns.contains(&local) {
            return ResolutionOutcome::Emit(CallTarget::plain(ResolvedOrUnresolved::Resolved {
                fqdn: local,
            }));
        }
        if let Some(entry) = global_builtin_registry().lookup(name, self.core.lookup.language) {
            return match entry.tier {
                BuiltinTier::Drop => ResolutionOutcome::Drop,
                BuiltinTier::Attribute => ResolutionOutcome::Attribute(entry.tag.clone()),
                BuiltinTier::Edge => ResolutionOutcome::Emit(CallTarget {
                    to: ResolvedOrUnresolved::Resolved {
                        fqdn: entry.synthetic_fqdn.clone(),
                    },
                    via_builtin: Some(entry.tag.clone()),
                }),
            };
        }
        ResolutionOutcome::Emit(CallTarget::plain(ResolvedOrUnresolved::Unresolved {
            name: local,
        }))
    }

    /// Stage 3e-1b — record a builtin tag against the symbol identified
    /// by `source_fqdn` so the post-walk flush can stamp it onto the
    /// symbol's `flags` vec. Best-effort : callers fire-and-forget,
    /// duplicates collapse into a `HashSet` so the same flag never lands
    /// twice on the same symbol regardless of how many times the same
    /// Attribute-tier builtin is touched in the symbol's body.
    pub(crate) fn register_attribute_flag(&mut self, source_fqdn: &str, tag: &BuiltinTag) {
        self.attribute_flags
            .entry(source_fqdn.to_string())
            .or_default()
            .insert(tag.slug());
    }

    pub(crate) fn span_location(&self, span: Span) -> SymbolLocation {
        let start = self.cm.lookup_char_pos(span.lo);
        let end = self.cm.lookup_char_pos(span.hi);
        SymbolLocation {
            file: self.core.file_path.clone(),
            start_line: clamp_line(start.line),
            start_col: clamp_col(start.col_display),
            end_line: clamp_line(end.line),
            end_col: clamp_col(end.col_display),
        }
    }

    pub(crate) fn span_site(&self, span: Span) -> Site {
        let start = self.cm.lookup_char_pos(span.lo);
        Site {
            file: self.core.file_path.clone(),
            line: clamp_line(start.line),
            col: clamp_col(start.col_display),
        }
    }

    pub(crate) fn body_hash_of(&self, span: Span) -> Option<Blake3Hash> {
        let snippet = self.span_snippet(span)?;
        Some(Blake3Hash::new(
            *blake3::hash(snippet.as_bytes()).as_bytes(),
        ))
    }

    pub(crate) fn span_snippet(&self, span: Span) -> Option<String> {
        self.cm.span_to_snippet(span).ok()
    }

    pub(crate) fn into_outputs(
        self,
    ) -> (
        Vec<RawSymbol>,
        Vec<RawEdge>,
        Vec<RawDocument>,
        Vec<RawCallSite>,
    ) {
        let (s, e, d, c, _) = self.into_outputs_with_lookup();
        (s, e, d, c)
    }

    /// Stage 3 final-mile (R1) — same as [`Self::into_outputs`] but also
    /// returns the AOT [`ModuleLookup`] so the pipeline can persist it via
    /// `put_module_lookup`. The lookup is what cross-workspace queries
    /// consult to resolve imports against this workspace's modules.
    pub(crate) fn into_outputs_with_lookup(
        mut self,
    ) -> (
        Vec<RawSymbol>,
        Vec<RawEdge>,
        Vec<RawDocument>,
        Vec<RawCallSite>,
        ModuleLookup,
    ) {
        // Stage 3e-1b — flush accumulated Attribute-tier flags onto the
        // symbols they belong to. Iteration order is stable (sorted by
        // flag string) so the resulting `symbol.flags` vec is
        // deterministic across runs — important for diff tools and the
        // body_hash-driven `apply_edges` plan.
        if !self.attribute_flags.is_empty() {
            let mut by_fqdn: HashMap<String, Vec<String>> = HashMap::new();
            for (fqdn, flags) in self.attribute_flags.drain() {
                let mut sorted: Vec<String> = flags.into_iter().collect();
                sorted.sort();
                by_fqdn.insert(fqdn, sorted);
            }
            for sym in self.core.symbols.iter_mut() {
                if let Some(extra) = by_fqdn.get(&sym.fqdn) {
                    for f in extra {
                        if !sym.flags.contains(f) {
                            sym.flags.push(f.clone());
                        }
                    }
                }
            }
        }
        (
            self.core.symbols,
            self.core.edges,
            self.core.documents,
            self.core.call_sites,
            self.core.lookup,
        )
    }
}

/// Outcome of [`TsWalkContext::resolve_call`]. Three variants reflect
/// the three skip semantics introduced by Stage 3e-1 tier
/// classification + 3e-1b's flag promotion :
#[derive(Debug, Clone)]
pub(crate) enum ResolutionOutcome {
    /// Drop tier builtin or unmatched name with no entry to record.
    /// Caller silently skips emission.
    Drop,
    /// Attribute tier builtin — caller skips the edge but registers
    /// the carried [`BuiltinTag`] against the enclosing symbol via
    /// [`TsWalkContext::register_attribute_flag`].
    Attribute(BuiltinTag),
    /// Resolvable target — caller emits the edge using the carried
    /// [`CallTarget`]. `target.via_builtin` is `Some(tag)` for
    /// Edge-tier builtins (so the emitter can stamp `via-builtin` /
    /// `builtin-<slug>` attrs).
    Emit(CallTarget),
}

/// A resolved (or unresolved-canonical) import, keyed in `import_aliases` by
/// the local binding name as written in source.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedImport {
    pub(crate) target: ResolvedOrUnresolved,
}

/// Output of [`TsWalkContext::resolve_call`]. Carries the canonical edge
/// target plus, when the resolution landed on a `BuiltinTier::Edge` builtin,
/// the originating [`BuiltinTag`] so emitters can attach `via-builtin` /
/// `builtin-<slug>` attributes (Rust UsesType already does this — Stage
/// 3e-1 brings TS to parity).
#[derive(Debug, Clone)]
pub(crate) struct CallTarget {
    pub(crate) to: ResolvedOrUnresolved,
    pub(crate) via_builtin: Option<BuiltinTag>,
}

impl CallTarget {
    /// Wrap a non-builtin resolution (import alias, defined local, or
    /// canonical-unresolved). `via_builtin` is `None`.
    pub(crate) fn plain(to: ResolvedOrUnresolved) -> Self {
        Self {
            to,
            via_builtin: None,
        }
    }
}

/// Stamp the standard heritage edge (`Extends` / `Implements`) for a
/// class / interface heritage clause, folding `via-builtin` / `builtin-<slug>`
/// attrs when the target is a synthetic Edge-tier builtin (matches the
/// Rust UsesType emission pattern).
fn push_heritage_edge(
    ctx: &mut TsWalkContext<'_>,
    from_fqdn: &str,
    kind: EdgeKind,
    target: CallTarget,
    span: Span,
) {
    let mut attributes: Vec<String> = vec![];
    if let Some(tag) = &target.via_builtin {
        attributes.push("via-builtin".to_string());
        attributes.push(format!("builtin-{}", tag.slug()));
    }
    let confidence = target.to.default_confidence();
    let site = ctx.span_site(span);
    ctx.push_edge(RawEdge {
        from_fqdn: from_fqdn.to_string(),
        kind,
        to: target.to,
        sites: vec![site],
        attributes,
        confidence,
    });
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn walk(
    module: &Module,
    package_name: &str,
    file_path: &str,
    file_module_fqdn: &str,
    cm: Lrc<SourceMap>,
    from_file_abs_path: &Path,
    package_root: &Path,
    tsconfig: Option<TsConfigPaths>,
    comments: &SingleThreadedComments,
) -> (
    Vec<RawSymbol>,
    Vec<RawEdge>,
    Vec<RawDocument>,
    Vec<RawCallSite>,
) {
    let (s, e, d, c, _lookup) = walk_with_lookup(
        module,
        package_name,
        file_path,
        file_module_fqdn,
        cm,
        from_file_abs_path,
        package_root,
        tsconfig,
        comments,
    );
    (s, e, d, c)
}

/// Stage 3 final-mile (R1) — same as [`walk`] but also returns the AOT
/// [`ModuleLookup`] so callers (production `extract_file`) can stash it
/// in [`ExtractedFile::module_lookup`] for pipeline persistence. Tests
/// continue to use [`walk`] when they don't care about the lookup.
#[allow(clippy::too_many_arguments)]
pub(crate) fn walk_with_lookup(
    module: &Module,
    package_name: &str,
    file_path: &str,
    file_module_fqdn: &str,
    cm: Lrc<SourceMap>,
    from_file_abs_path: &Path,
    package_root: &Path,
    tsconfig: Option<TsConfigPaths>,
    comments: &SingleThreadedComments,
) -> (
    Vec<RawSymbol>,
    Vec<RawEdge>,
    Vec<RawDocument>,
    Vec<RawCallSite>,
    ModuleLookup,
) {
    let mut ctx = TsWalkContext::new(
        file_path.to_string(),
        package_name.to_string(),
        file_module_fqdn.to_string(),
        cm,
        from_file_abs_path.to_path_buf(),
        package_root.to_path_buf(),
        tsconfig,
        comments,
    );
    ctx.core.lookup = build_ts_lookup(module, file_module_fqdn);
    walk_p1(&mut ctx, &module.body, file_module_fqdn);
    walk_p2(&mut ctx, &module.body, file_module_fqdn);
    ctx.into_outputs_with_lookup()
}

fn walk_p1(ctx: &mut TsWalkContext<'_>, items: &[ModuleItem], current_module: &str) {
    for item in items {
        process_item_p1(ctx, item, current_module);
    }
}

fn process_item_p1(ctx: &mut TsWalkContext<'_>, item: &ModuleItem, current_module: &str) {
    match item {
        ModuleItem::ModuleDecl(ModuleDecl::Import(it)) => {
            process_import(ctx, it, current_module);
        }
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(it)) => {
            process_export_decl(ctx, it, current_module);
        }
        ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultDecl(it)) => {
            process_export_default_decl(ctx, it, current_module);
        }
        ModuleItem::ModuleDecl(ModuleDecl::ExportAll(it)) => {
            // `export * from "./foo"` — re-exports every symbol of "./foo".
            process_re_export_all(ctx, it, current_module);
        }
        ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(it)) => {
            // `export { x } from "./foo"` — re-exports named symbols. Without
            // `src` this is a local re-export (no cross-module traceability) —
            // we skip it day-1, mirroring `process_import`'s alias-only path.
            if let Some(src) = &it.src {
                process_re_export_named(ctx, src, &it.specifiers, it.span, current_module);
            }
        }
        ModuleItem::Stmt(Stmt::Decl(decl)) => {
            process_decl(ctx, decl, current_module, false, decl.span().lo);
        }
        // `export = …` / TS-only export forms are PUNTed day-1.
        _ => {}
    }
}

fn process_re_export_all(
    ctx: &mut TsWalkContext<'_>,
    item: &swc_core::ecma::ast::ExportAll,
    current_module: &str,
) {
    let spec = item.src.value.to_string_lossy().into_owned();
    let canonical = resolve_import(
        &spec,
        &ctx.from_file_abs_path,
        &ctx.package_root,
        &ctx.package_name,
        ctx.tsconfig.as_ref(),
    )
    .unwrap_or_else(|| spec.clone());
    let target = if ctx.core.defined_fqdns.contains(&canonical) {
        ResolvedOrUnresolved::Resolved { fqdn: canonical }
    } else {
        ResolvedOrUnresolved::Unresolved { name: canonical }
    };
    let confidence = target.default_confidence();
    ctx.push_edge(RawEdge {
        from_fqdn: current_module.to_string(),
        kind: EdgeKind::Imports,
        to: target,
        sites: vec![ctx.span_site(item.span)],
        attributes: vec!["re-export".to_string(), "wildcard".to_string()],
        confidence,
    });
}

fn process_re_export_named(
    ctx: &mut TsWalkContext<'_>,
    src: &swc_core::ecma::ast::Str,
    specifiers: &[swc_core::ecma::ast::ExportSpecifier],
    span: Span,
    current_module: &str,
) {
    use swc_core::ecma::ast::ExportSpecifier;
    let spec = src.value.to_string_lossy().into_owned();
    let canonical = resolve_import(
        &spec,
        &ctx.from_file_abs_path,
        &ctx.package_root,
        &ctx.package_name,
        ctx.tsconfig.as_ref(),
    )
    .unwrap_or_else(|| spec.clone());
    if specifiers.is_empty() {
        let target = if ctx.core.defined_fqdns.contains(&canonical) {
            ResolvedOrUnresolved::Resolved { fqdn: canonical }
        } else {
            ResolvedOrUnresolved::Unresolved { name: canonical }
        };
        let confidence = target.default_confidence();
        ctx.push_edge(RawEdge {
            from_fqdn: current_module.to_string(),
            kind: EdgeKind::Imports,
            to: target,
            sites: vec![ctx.span_site(span)],
            attributes: vec!["re-export".to_string()],
            confidence,
        });
        return;
    }
    for spec_item in specifiers {
        let imported_name = match spec_item {
            ExportSpecifier::Named(named) => match &named.orig {
                ModuleExportName::Ident(i) => i.sym.to_string(),
                ModuleExportName::Str(s) => s.value.to_string_lossy().into_owned(),
            },
            ExportSpecifier::Default(_) => "default".to_string(),
            ExportSpecifier::Namespace(_) => {
                // `export * as ns from …` — emit a single edge to the module.
                let target = if ctx.core.defined_fqdns.contains(&canonical) {
                    ResolvedOrUnresolved::Resolved {
                        fqdn: canonical.clone(),
                    }
                } else {
                    ResolvedOrUnresolved::Unresolved {
                        name: canonical.clone(),
                    }
                };
                let confidence = target.default_confidence();
                ctx.push_edge(RawEdge {
                    from_fqdn: current_module.to_string(),
                    kind: EdgeKind::Imports,
                    to: target,
                    sites: vec![ctx.span_site(span)],
                    attributes: vec!["re-export".to_string(), "namespace".to_string()],
                    confidence,
                });
                continue;
            }
        };
        let target_fqdn = format!("{canonical}::{imported_name}");
        let target = if ctx.core.defined_fqdns.contains(&target_fqdn) {
            ResolvedOrUnresolved::Resolved {
                fqdn: target_fqdn.clone(),
            }
        } else {
            ResolvedOrUnresolved::Unresolved {
                name: target_fqdn.clone(),
            }
        };
        let confidence = target.default_confidence();
        ctx.push_edge(RawEdge {
            from_fqdn: current_module.to_string(),
            kind: EdgeKind::Imports,
            to: target,
            sites: vec![ctx.span_site(span)],
            attributes: vec!["re-export".to_string()],
            confidence,
        });
    }
}

fn walk_p2(ctx: &mut TsWalkContext<'_>, items: &[ModuleItem], current_module: &str) {
    for item in items {
        process_item_p2(ctx, item, current_module);
    }
}

fn process_item_p2(ctx: &mut TsWalkContext<'_>, item: &ModuleItem, current_module: &str) {
    match item {
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(it)) => {
            visit_decl_bodies(ctx, &it.decl, current_module);
        }
        ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultDecl(it)) => {
            visit_default_decl_bodies(ctx, &it.decl, current_module);
        }
        ModuleItem::Stmt(Stmt::Decl(decl)) => {
            visit_decl_bodies(ctx, decl, current_module);
        }
        ModuleItem::Stmt(Stmt::Expr(expr_stmt)) => {
            // Top-level expression statement (Vue 3 script-setup idiom:
            // `onMount(() => { ... });` at module scope). No enclosing
            // symbol — calls attribute to the module fqdn itself.
            visit::visit_expression_for_calls(ctx, &expr_stmt.expr, current_module, current_module);
        }
        _ => {}
    }
}

fn visit_decl_bodies(ctx: &mut TsWalkContext<'_>, decl: &Decl, current_module: &str) {
    match decl {
        Decl::Fn(it) => {
            let fn_fqdn = format!("{current_module}::{}", it.ident.sym);
            visit::visit_function_body(ctx, &it.function, current_module, &fn_fqdn);
        }
        Decl::Class(it) => {
            visit_class_methods(ctx, &it.ident.sym, &it.class, current_module);
        }
        Decl::Var(var) => {
            visit_var_initializers(ctx, var, current_module);
        }
        _ => {}
    }
}

fn visit_default_decl_bodies(
    ctx: &mut TsWalkContext<'_>,
    decl: &DefaultDecl,
    current_module: &str,
) {
    match decl {
        DefaultDecl::Fn(fn_expr) => {
            let name = fn_expr
                .ident
                .as_ref()
                .map_or_else(|| "default".to_string(), |i| i.sym.to_string());
            let fn_fqdn = format!("{current_module}::{name}");
            visit::visit_function_body(ctx, &fn_expr.function, current_module, &fn_fqdn);
        }
        DefaultDecl::Class(class_expr) => {
            let name = class_expr
                .ident
                .as_ref()
                .map_or_else(|| "default".to_string(), |i| i.sym.to_string());
            visit_class_methods(ctx, &name, &class_expr.class, current_module);
        }
        DefaultDecl::TsInterfaceDecl(_) => {}
    }
}

fn visit_class_methods(
    ctx: &mut TsWalkContext<'_>,
    class_name: &str,
    class: &Class,
    current_module: &str,
) {
    let class_fqdn = format!("{current_module}::{class_name}");
    for member in &class.body {
        match member {
            ClassMember::Method(method) => {
                let Some(method_name) = method_name_string(&method.key) else {
                    continue;
                };
                let method_fqdn = format!("{class_fqdn}::{method_name}");
                visit::visit_function_body(ctx, &method.function, current_module, &method_fqdn);
            }
            ClassMember::Constructor(ctor) => {
                // FQDN must match `extract_constructor`'s Pass-1 shape.
                let ctor_fqdn = format!("{class_fqdn}::constructor");
                visit::visit_constructor_body(ctx, ctor, current_module, &ctor_fqdn);
            }
            _ => {}
        }
    }
}

fn visit_var_initializers(ctx: &mut TsWalkContext<'_>, var: &VarDecl, current_module: &str) {
    for declarator in &var.decls {
        let Some(name) = declarator_name(declarator) else {
            continue;
        };
        let Some(init) = &declarator.init else {
            continue;
        };
        let var_fqdn = format!("{current_module}::{name}");
        visit::visit_expression_for_calls(ctx, init, current_module, &var_fqdn);
    }
}

fn process_decl(
    ctx: &mut TsWalkContext<'_>,
    decl: &Decl,
    current_module: &str,
    exported: bool,
    outer_pos: BytePos,
) {
    match decl {
        Decl::Fn(it) => {
            let sym = extract_fn_decl(ctx, it, current_module, exported);
            ctx.push_symbol_with_doc(sym, outer_pos);
        }
        Decl::Class(it) => {
            extract_class_decl(ctx, it, current_module, exported, outer_pos);
        }
        Decl::Var(it) => {
            extract_var_decl(ctx, it, current_module, exported, outer_pos);
        }
        Decl::TsInterface(it) => {
            extract_interface_decl(ctx, it, current_module, exported, outer_pos);
        }
        Decl::TsTypeAlias(it) => {
            let sym = extract_type_alias_decl(ctx, it, current_module, exported);
            let alias_fqdn = sym.fqdn.clone();
            ctx.push_symbol_with_doc(sym, outer_pos);
            // Bug B Stage 2b: walk the alias body for `UsesType` edges
            // (`type X = Foo | Bar` → edges from X to Foo and Bar).
            visit::visit_ts_type_for_uses(
                ctx,
                &it.type_ann,
                current_module,
                &alias_fqdn,
                visit::TYPE_CTX_ALIAS_BODY,
            );
        }
        Decl::TsEnum(it) => {
            extract_enum_decl(ctx, it, current_module, exported, outer_pos);
        }
        Decl::TsModule(_) | Decl::Using(_) => {}
    }
}

fn process_export_decl(ctx: &mut TsWalkContext<'_>, item: &ExportDecl, current_module: &str) {
    process_decl(ctx, &item.decl, current_module, true, item.span.lo);
}

fn process_export_default_decl(
    ctx: &mut TsWalkContext<'_>,
    item: &ExportDefaultDecl,
    current_module: &str,
) {
    let outer_pos = item.span.lo;
    match &item.decl {
        DefaultDecl::Fn(fn_expr) => {
            let name = fn_expr
                .ident
                .as_ref()
                .map_or_else(|| "default".to_string(), |i| i.sym.to_string());
            let span = fn_expr.function.span;
            let signature = build_function_signature(ctx, &fn_expr.function);
            let body_hash = ctx.body_hash_of(span);
            ctx.push_symbol_with_doc(
                RawSymbol {
                    fqdn: format!("{current_module}::{name}"),
                    name,
                    kind: Kind::Function,
                    language_kind: LanguageKind::from("function"),
                    module: Some(current_module.to_string()),
                    visibility: Visibility::Public,
                    location: ctx.span_location(span),
                    signature: Some(signature),
                    body_hash,
                    attributes: vec![],
                    flags: vec![],
                },
                outer_pos,
            );
        }
        DefaultDecl::Class(class_expr) => {
            let name = class_expr
                .ident
                .as_ref()
                .map_or_else(|| "default".to_string(), |i| i.sym.to_string());
            extract_class_inner(
                ctx,
                &name,
                &class_expr.class,
                current_module,
                true,
                outer_pos,
            );
        }
        DefaultDecl::TsInterfaceDecl(interface) => {
            let exported = true;
            let span = interface.span;
            let name = interface.id.sym.to_string();
            ctx.push_symbol_with_doc(
                RawSymbol {
                    fqdn: format!("{current_module}::{name}"),
                    name,
                    kind: Kind::Type,
                    language_kind: LanguageKind::from("interface"),
                    module: Some(current_module.to_string()),
                    visibility: map_access_modifier(None, exported),
                    location: ctx.span_location(span),
                    signature: None,
                    body_hash: ctx.body_hash_of(span),
                    attributes: vec![],
                    flags: vec![],
                },
                outer_pos,
            );
        }
    }
}

fn process_import(ctx: &mut TsWalkContext<'_>, item: &ImportDecl, current_module: &str) {
    let span = item.span;
    let spec = item.src.value.to_string_lossy().into_owned();
    let canonical = resolve_import(
        &spec,
        &ctx.from_file_abs_path,
        &ctx.package_root,
        &ctx.package_name,
        ctx.tsconfig.as_ref(),
    )
    .unwrap_or_else(|| spec.clone());
    for spec_item in &item.specifiers {
        let (local, imported_name) = match spec_item {
            ImportSpecifier::Named(named) => {
                let local = named.local.sym.to_string();
                let imported = named.imported.as_ref().map_or_else(
                    || local.clone(),
                    |n| match n {
                        ModuleExportName::Ident(i) => i.sym.to_string(),
                        ModuleExportName::Str(s) => s.value.to_string_lossy().into_owned(),
                    },
                );
                (local, Some(imported))
            }
            ImportSpecifier::Default(d) => (d.local.sym.to_string(), Some("default".to_string())),
            ImportSpecifier::Namespace(ns) => (ns.local.sym.to_string(), None),
        };
        let target_fqdn = match &imported_name {
            Some(name) => format!("{canonical}::{name}"),
            None => canonical.clone(),
        };
        let target = if ctx.core.defined_fqdns.contains(&target_fqdn) {
            ResolvedOrUnresolved::Resolved {
                fqdn: target_fqdn.clone(),
            }
        } else {
            ResolvedOrUnresolved::Unresolved {
                name: target_fqdn.clone(),
            }
        };
        ctx.add_import_alias(
            local,
            ResolvedImport {
                target: target.clone(),
            },
        );
        let confidence = target.default_confidence();
        ctx.push_edge(RawEdge {
            from_fqdn: current_module.to_string(),
            kind: EdgeKind::Imports,
            to: target,
            sites: vec![ctx.span_site(span)],
            attributes: vec![],
            confidence,
        });
    }
    if item.specifiers.is_empty() {
        // Side-effect import (`import "polyfill";`) → emit a single IMPORTS edge
        // with the resolved canonical, no alias.
        let target = if ctx.core.defined_fqdns.contains(&canonical) {
            ResolvedOrUnresolved::Resolved { fqdn: canonical }
        } else {
            ResolvedOrUnresolved::Unresolved { name: canonical }
        };
        let confidence = target.default_confidence();
        ctx.push_edge(RawEdge {
            from_fqdn: current_module.to_string(),
            kind: EdgeKind::Imports,
            to: target,
            sites: vec![ctx.span_site(span)],
            attributes: vec![],
            confidence,
        });
    }
}

fn extract_fn_decl(
    ctx: &TsWalkContext<'_>,
    item: &FnDecl,
    parent_fqdn: &str,
    exported: bool,
) -> RawSymbol {
    let name = item.ident.sym.to_string();
    let fqdn = format!("{parent_fqdn}::{name}");
    let span = item.function.span;
    let signature = build_function_signature(ctx, &item.function);
    RawSymbol {
        name,
        fqdn,
        kind: Kind::Function,
        language_kind: LanguageKind::from("function"),
        module: Some(parent_fqdn.to_string()),
        visibility: map_access_modifier(None, exported),
        location: ctx.span_location(span),
        signature: Some(signature),
        body_hash: ctx.body_hash_of(span),
        attributes: vec![],
        flags: vec![],
    }
}

fn extract_class_decl(
    ctx: &mut TsWalkContext<'_>,
    item: &ClassDecl,
    parent_fqdn: &str,
    exported: bool,
    outer_pos: BytePos,
) {
    let name = item.ident.sym.to_string();
    extract_class_inner(ctx, &name, &item.class, parent_fqdn, exported, outer_pos);
}

fn extract_class_inner(
    ctx: &mut TsWalkContext<'_>,
    name: &str,
    class: &Class,
    parent_fqdn: &str,
    exported: bool,
    outer_pos: BytePos,
) {
    let class_fqdn = format!("{parent_fqdn}::{name}");
    let class_span = class.span;
    ctx.push_symbol_with_doc(
        RawSymbol {
            name: name.to_string(),
            fqdn: class_fqdn.clone(),
            kind: Kind::Type,
            language_kind: LanguageKind::from("class"),
            module: Some(parent_fqdn.to_string()),
            visibility: map_access_modifier(None, exported),
            location: ctx.span_location(class_span),
            signature: None,
            body_hash: ctx.body_hash_of(class_span),
            attributes: vec![],
            flags: vec![],
        },
        outer_pos,
    );

    if let Some(super_class) = &class.super_class {
        let span = super_class.span();
        // Stage 3e-1 / 3e-1b: tier-dependent dispatch. Drop ⇒ skip;
        // Attribute ⇒ flag the class (e.g. `class C extends Promise`
        // becomes flagged `async`); Emit ⇒ stamp the heritage edge.
        // Generic args still recurse regardless so inner type refs
        // surface their own UsesType edges.
        match ctx.resolve_call(&render_expr_name(super_class), parent_fqdn) {
            ResolutionOutcome::Drop => {}
            ResolutionOutcome::Attribute(tag) => {
                ctx.register_attribute_flag(&class_fqdn, &tag);
            }
            ResolutionOutcome::Emit(target) => {
                push_heritage_edge(ctx, &class_fqdn, EdgeKind::Extends, target, span);
            }
        }
        // Bug B Stage 2b: walk extends' generic args
        // (`class X extends Foo<Bar>` → UsesType edge to Bar).
        if let Some(type_params) = &class.super_type_params {
            visit::visit_type_params_for_uses(
                ctx,
                type_params,
                parent_fqdn,
                &class_fqdn,
                visit::TYPE_CTX_EXTENDS,
            );
        }
    }
    for impl_target in &class.implements {
        let span = impl_target.span;
        match ctx.resolve_call(&render_ts_entity_name(&impl_target.expr), parent_fqdn) {
            ResolutionOutcome::Drop => {}
            ResolutionOutcome::Attribute(tag) => {
                ctx.register_attribute_flag(&class_fqdn, &tag);
            }
            ResolutionOutcome::Emit(target) => {
                push_heritage_edge(ctx, &class_fqdn, EdgeKind::Implements, target, span);
            }
        }
        // Bug B Stage 2b: walk implements' generic args
        // (`class X implements Foo<Bar>` → UsesType edge to Bar).
        if let Some(type_args) = &impl_target.type_args {
            visit::visit_type_params_for_uses(
                ctx,
                type_args,
                parent_fqdn,
                &class_fqdn,
                visit::TYPE_CTX_IMPLEMENTS,
            );
        }
    }

    for member in &class.body {
        match member {
            ClassMember::Method(method) => {
                if let Some(method_name) = method_name_string(&method.key) {
                    let method_sym = extract_method(ctx, method, &class_fqdn, &method_name);
                    ctx.push_symbol_with_doc(method_sym, method.span.lo);
                }
            }
            ClassMember::PrivateMethod(pmethod) => {
                let private_name = format!("#{}", pmethod.key.name);
                let sym = extract_private_method(ctx, pmethod, &class_fqdn, &private_name);
                ctx.push_symbol_with_doc(sym, pmethod.span.lo);
            }
            ClassMember::Constructor(ctor) => {
                let sym = extract_constructor(ctx, ctor, &class_fqdn);
                ctx.push_symbol_with_doc(sym, ctor.span.lo);
            }
            ClassMember::ClassProp(prop) => {
                if let Some(prop_name) = method_name_string(&prop.key) {
                    let sym = extract_class_prop(ctx, prop, &class_fqdn, &prop_name);
                    let prop_fqdn = sym.fqdn.clone();
                    ctx.push_symbol_with_doc(sym, prop.span.lo);
                    // Bug B Stage 2b: walk the prop's type annotation
                    // (`class X { field: Foo }` → UsesType from
                    // `X::field` to Foo).
                    if let Some(ann) = &prop.type_ann {
                        visit::visit_type_ann_for_uses(
                            ctx,
                            ann,
                            parent_fqdn,
                            &prop_fqdn,
                            visit::TYPE_CTX_CLASS_PROP,
                        );
                    }
                }
            }
            ClassMember::PrivateProp(pprop) => {
                let private_name = format!("#{}", pprop.key.name);
                let sym = extract_private_prop(ctx, pprop, &class_fqdn, &private_name);
                let prop_fqdn = sym.fqdn.clone();
                ctx.push_symbol_with_doc(sym, pprop.span.lo);
                if let Some(ann) = &pprop.type_ann {
                    visit::visit_type_ann_for_uses(
                        ctx,
                        ann,
                        parent_fqdn,
                        &prop_fqdn,
                        visit::TYPE_CTX_CLASS_PROP,
                    );
                }
            }
            // TsIndexSignature is a typing artifact (`[key: string]: T`) with no
            // symbol identity. Empty / StaticBlock / AutoAccessor are skipped
            // day-1 — additive enrichment if usage surfaces a real need.
            _ => {}
        }
    }
}

fn extract_constructor(
    ctx: &TsWalkContext<'_>,
    ctor: &swc_core::ecma::ast::Constructor,
    class_fqdn: &str,
) -> RawSymbol {
    let fqdn = format!("{class_fqdn}::constructor");
    let span = ctor.span;
    let raw_access = ctor.accessibility.map(|a| match a {
        swc_core::ecma::ast::Accessibility::Public => "public",
        swc_core::ecma::ast::Accessibility::Private => "private",
        swc_core::ecma::ast::Accessibility::Protected => "protected",
    });
    let visibility = map_access_modifier(raw_access, raw_access.is_none());
    RawSymbol {
        name: "constructor".to_string(),
        fqdn,
        kind: Kind::Function,
        language_kind: LanguageKind::from("constructor"),
        module: Some(class_fqdn.to_string()),
        visibility,
        location: ctx.span_location(span),
        signature: None,
        body_hash: ctx.body_hash_of(span),
        attributes: vec![],
        flags: vec![],
    }
}

fn extract_class_prop(
    ctx: &TsWalkContext<'_>,
    prop: &swc_core::ecma::ast::ClassProp,
    class_fqdn: &str,
    prop_name: &str,
) -> RawSymbol {
    let fqdn = format!("{class_fqdn}::{prop_name}");
    let span = prop.span;
    let raw_access = prop.accessibility.map(|a| match a {
        swc_core::ecma::ast::Accessibility::Public => "public",
        swc_core::ecma::ast::Accessibility::Private => "private",
        swc_core::ecma::ast::Accessibility::Protected => "protected",
    });
    let visibility = map_access_modifier(raw_access, raw_access.is_none());
    let language_kind = if prop.is_static {
        LanguageKind::from("static_property")
    } else {
        LanguageKind::from("property")
    };
    RawSymbol {
        name: prop_name.to_string(),
        fqdn,
        kind: Kind::Value,
        language_kind,
        module: Some(class_fqdn.to_string()),
        visibility,
        location: ctx.span_location(span),
        signature: None,
        body_hash: ctx.body_hash_of(span),
        attributes: vec![],
        flags: vec![],
    }
}

fn extract_private_method(
    ctx: &TsWalkContext<'_>,
    method: &swc_core::ecma::ast::PrivateMethod,
    class_fqdn: &str,
    method_name: &str,
) -> RawSymbol {
    let fqdn = format!("{class_fqdn}::{method_name}");
    let span = method.span;
    RawSymbol {
        name: method_name.to_string(),
        fqdn,
        kind: Kind::Function,
        language_kind: LanguageKind::from("method"),
        module: Some(class_fqdn.to_string()),
        // ECMAScript `#name` private is always private regardless of TS accessibility.
        visibility: Visibility::Private,
        location: ctx.span_location(span),
        signature: Some(build_function_signature(ctx, &method.function)),
        body_hash: ctx.body_hash_of(span),
        attributes: vec![],
        flags: vec![],
    }
}

fn extract_private_prop(
    ctx: &TsWalkContext<'_>,
    prop: &swc_core::ecma::ast::PrivateProp,
    class_fqdn: &str,
    prop_name: &str,
) -> RawSymbol {
    let fqdn = format!("{class_fqdn}::{prop_name}");
    let span = prop.span;
    let language_kind = if prop.is_static {
        LanguageKind::from("static_property")
    } else {
        LanguageKind::from("property")
    };
    RawSymbol {
        name: prop_name.to_string(),
        fqdn,
        kind: Kind::Value,
        language_kind,
        module: Some(class_fqdn.to_string()),
        visibility: Visibility::Private,
        location: ctx.span_location(span),
        signature: None,
        body_hash: ctx.body_hash_of(span),
        attributes: vec![],
        flags: vec![],
    }
}

fn extract_method(
    ctx: &TsWalkContext<'_>,
    method: &ClassMethod,
    class_fqdn: &str,
    method_name: &str,
) -> RawSymbol {
    let fqdn = format!("{class_fqdn}::{method_name}");
    let span = method.span;
    let raw_access = method.accessibility.map(|a| match a {
        swc_core::ecma::ast::Accessibility::Public => "public",
        swc_core::ecma::ast::Accessibility::Private => "private",
        swc_core::ecma::ast::Accessibility::Protected => "protected",
    });
    let visibility = map_access_modifier(raw_access, raw_access.is_none());
    RawSymbol {
        name: method_name.to_string(),
        fqdn,
        kind: Kind::Function,
        language_kind: LanguageKind::from("method"),
        module: Some(class_fqdn.to_string()),
        visibility,
        location: ctx.span_location(span),
        signature: Some(build_function_signature(ctx, &method.function)),
        body_hash: ctx.body_hash_of(span),
        attributes: vec![],
        flags: vec![],
    }
}

fn extract_var_decl(
    ctx: &mut TsWalkContext<'_>,
    item: &VarDecl,
    parent_fqdn: &str,
    exported: bool,
    outer_pos: BytePos,
) {
    for declarator in &item.decls {
        let Some(name) = declarator_name(declarator) else {
            continue;
        };
        let span = declarator.span;
        let fqdn = format!("{parent_fqdn}::{name}");
        let signature = signature_from_declarator(ctx, declarator);
        let language_kind = match item.kind {
            swc_core::ecma::ast::VarDeclKind::Const => "const",
            swc_core::ecma::ast::VarDeclKind::Let => "let",
            swc_core::ecma::ast::VarDeclKind::Var => "var",
        };
        let kind = signature.as_ref().map_or(Kind::Value, |_| Kind::Function);
        let language_kind = if signature.is_some() {
            LanguageKind::from("function")
        } else {
            LanguageKind::from(language_kind)
        };
        ctx.push_symbol_with_doc(
            RawSymbol {
                name,
                fqdn: fqdn.clone(),
                kind,
                language_kind,
                module: Some(parent_fqdn.to_string()),
                visibility: map_access_modifier(None, exported),
                location: ctx.span_location(span),
                signature,
                body_hash: ctx.body_hash_of(span),
                attributes: vec![],
                flags: vec![],
            },
            outer_pos,
        );
        // Bug B Stage 2b: walk the declarator's pattern for the type
        // annotation (`const x: Foo = …`, `let [a, b]: [Foo, Bar] = …`).
        // Init walking happens later in `visit_var_initializers` via the
        // CallVisitor — that path also picks up nested `BindingIdent`
        // annotations, but only when an init is present. Walking the
        // pattern here covers init-less let-decls and top-level ambient
        // shapes consistently.
        visit::visit_pat_for_uses(
            ctx,
            &declarator.name,
            parent_fqdn,
            &fqdn,
            visit::TYPE_CTX_ANNOTATION,
        );
    }
}

fn extract_interface_decl(
    ctx: &mut TsWalkContext<'_>,
    item: &TsInterfaceDecl,
    parent_fqdn: &str,
    exported: bool,
    outer_pos: BytePos,
) {
    let name = item.id.sym.to_string();
    let fqdn = format!("{parent_fqdn}::{name}");
    let span = item.span;
    ctx.push_symbol_with_doc(
        RawSymbol {
            name,
            fqdn: fqdn.clone(),
            kind: Kind::Type,
            language_kind: LanguageKind::from("interface"),
            module: Some(parent_fqdn.to_string()),
            visibility: map_access_modifier(None, exported),
            location: ctx.span_location(span),
            signature: None,
            body_hash: ctx.body_hash_of(span),
            attributes: vec![],
            flags: vec![],
        },
        outer_pos,
    );
    for ext in &item.extends {
        let span = ext.span;
        match ctx.resolve_call(&render_expr_name(&ext.expr), parent_fqdn) {
            ResolutionOutcome::Drop => {}
            ResolutionOutcome::Attribute(tag) => {
                ctx.register_attribute_flag(&fqdn, &tag);
            }
            ResolutionOutcome::Emit(target) => {
                push_heritage_edge(ctx, &fqdn, EdgeKind::Extends, target, span);
            }
        }
        // Bug B Stage 2b: walk the extends' generic args
        // (`interface I extends J<K>` → UsesType edge to K).
        if let Some(type_args) = &ext.type_args {
            visit::visit_type_params_for_uses(
                ctx,
                type_args,
                parent_fqdn,
                &fqdn,
                visit::TYPE_CTX_EXTENDS,
            );
        }
    }
    // Bug C-1 — push a `RawSymbol` per interface body member so the
    // viz can display interface internals (properties, methods, …).
    // Each member's type annotations emit `UsesType` edges from the
    // member's fqdn (rather than the interface fqdn), giving the graph
    // per-field provenance instead of an aggregate-on-the-parent edge.
    // `TsCallSignatureDecl` / `TsConstructSignatureDecl` /
    // `TsIndexSignature` are typing artifacts without symbol identity
    // (no name we can attach to) and are intentionally skipped.
    for member in &item.body.body {
        use swc_core::ecma::ast::TsTypeElement;
        match member {
            TsTypeElement::TsPropertySignature(prop) => {
                if prop.computed {
                    continue;
                }
                let Some(member_name) = interface_member_key_name(&prop.key) else {
                    continue;
                };
                let member_fqdn = format!("{fqdn}::{member_name}");
                ctx.push_symbol_with_doc(
                    RawSymbol {
                        name: member_name,
                        fqdn: member_fqdn.clone(),
                        kind: Kind::Value,
                        language_kind: LanguageKind::from("interface_property"),
                        module: Some(fqdn.clone()),
                        visibility: map_access_modifier(None, exported),
                        location: ctx.span_location(prop.span),
                        signature: None,
                        body_hash: ctx.body_hash_of(prop.span),
                        attributes: vec![],
                        flags: vec![],
                    },
                    prop.span.lo,
                );
                if let Some(ann) = &prop.type_ann {
                    visit::visit_type_ann_for_uses(
                        ctx,
                        ann,
                        parent_fqdn,
                        &member_fqdn,
                        visit::TYPE_CTX_INTERFACE_MEMBER,
                    );
                }
            }
            TsTypeElement::TsMethodSignature(method) => {
                if method.computed {
                    continue;
                }
                let Some(member_name) = interface_member_key_name(&method.key) else {
                    continue;
                };
                let member_fqdn = format!("{fqdn}::{member_name}");
                ctx.push_symbol_with_doc(
                    RawSymbol {
                        name: member_name,
                        fqdn: member_fqdn.clone(),
                        kind: Kind::Function,
                        language_kind: LanguageKind::from("interface_method"),
                        module: Some(fqdn.clone()),
                        visibility: map_access_modifier(None, exported),
                        location: ctx.span_location(method.span),
                        signature: None,
                        body_hash: ctx.body_hash_of(method.span),
                        attributes: vec![],
                        flags: vec![],
                    },
                    method.span.lo,
                );
                for p in &method.params {
                    visit::visit_ts_fn_param_for_uses(
                        ctx,
                        p,
                        parent_fqdn,
                        &member_fqdn,
                        visit::TYPE_CTX_ANNOTATION,
                    );
                }
                if let Some(ann) = &method.type_ann {
                    visit::visit_type_ann_for_uses(
                        ctx,
                        ann,
                        parent_fqdn,
                        &member_fqdn,
                        visit::TYPE_CTX_ANNOTATION,
                    );
                }
                if let Some(type_params) = &method.type_params {
                    visit::visit_ts_type_param_decl_for_uses(
                        ctx,
                        type_params,
                        parent_fqdn,
                        &member_fqdn,
                        visit::TYPE_CTX_CONSTRAINT,
                    );
                }
            }
            TsTypeElement::TsGetterSignature(getter) => {
                if getter.computed {
                    continue;
                }
                let Some(member_name) = interface_member_key_name(&getter.key) else {
                    continue;
                };
                let member_fqdn = format!("{fqdn}::{member_name}");
                ctx.push_symbol_with_doc(
                    RawSymbol {
                        name: member_name,
                        fqdn: member_fqdn.clone(),
                        kind: Kind::Function,
                        language_kind: LanguageKind::from("interface_getter"),
                        module: Some(fqdn.clone()),
                        visibility: map_access_modifier(None, exported),
                        location: ctx.span_location(getter.span),
                        signature: None,
                        body_hash: ctx.body_hash_of(getter.span),
                        attributes: vec![],
                        flags: vec![],
                    },
                    getter.span.lo,
                );
                if let Some(ann) = &getter.type_ann {
                    visit::visit_type_ann_for_uses(
                        ctx,
                        ann,
                        parent_fqdn,
                        &member_fqdn,
                        visit::TYPE_CTX_ANNOTATION,
                    );
                }
            }
            TsTypeElement::TsSetterSignature(setter) => {
                if setter.computed {
                    continue;
                }
                let Some(member_name) = interface_member_key_name(&setter.key) else {
                    continue;
                };
                let member_fqdn = format!("{fqdn}::{member_name}");
                ctx.push_symbol_with_doc(
                    RawSymbol {
                        name: member_name,
                        fqdn: member_fqdn.clone(),
                        kind: Kind::Function,
                        language_kind: LanguageKind::from("interface_setter"),
                        module: Some(fqdn.clone()),
                        visibility: map_access_modifier(None, exported),
                        location: ctx.span_location(setter.span),
                        signature: None,
                        body_hash: ctx.body_hash_of(setter.span),
                        attributes: vec![],
                        flags: vec![],
                    },
                    setter.span.lo,
                );
                visit::visit_ts_fn_param_for_uses(
                    ctx,
                    &setter.param,
                    parent_fqdn,
                    &member_fqdn,
                    visit::TYPE_CTX_ANNOTATION,
                );
            }
            // Call / construct / index signatures have no symbol identity
            // (anonymous, structural). Their type annotations could still
            // emit edges from the interface fqdn, but day-1 we skip them
            // — they're rare in modern TS and the structural typing they
            // express is better handled by Stage 3's lookup table.
            TsTypeElement::TsCallSignatureDecl(_)
            | TsTypeElement::TsConstructSignatureDecl(_)
            | TsTypeElement::TsIndexSignature(_) => {}
        }
    }
}

fn extract_type_alias_decl(
    ctx: &TsWalkContext<'_>,
    item: &TsTypeAliasDecl,
    parent_fqdn: &str,
    exported: bool,
) -> RawSymbol {
    let name = item.id.sym.to_string();
    let fqdn = format!("{parent_fqdn}::{name}");
    let span = item.span;
    RawSymbol {
        name,
        fqdn,
        kind: Kind::Type,
        language_kind: LanguageKind::from("type_alias"),
        module: Some(parent_fqdn.to_string()),
        visibility: map_access_modifier(None, exported),
        location: ctx.span_location(span),
        signature: None,
        body_hash: ctx.body_hash_of(span),
        attributes: vec![],
        flags: vec![],
    }
}

/// Bug C-1 — extracts the enum symbol AND one `RawSymbol` per
/// `TsEnumMember`. Pushes everything internally (the caller no longer
/// receives a `RawSymbol`); this mirrors `extract_interface_decl`'s
/// convention for declarations that own multiple sub-symbols.
fn extract_enum_decl(
    ctx: &mut TsWalkContext<'_>,
    item: &TsEnumDecl,
    parent_fqdn: &str,
    exported: bool,
    outer_pos: BytePos,
) {
    let name = item.id.sym.to_string();
    let enum_fqdn = format!("{parent_fqdn}::{name}");
    let span = item.span;
    ctx.push_symbol_with_doc(
        RawSymbol {
            name,
            fqdn: enum_fqdn.clone(),
            kind: Kind::Type,
            language_kind: LanguageKind::from("enum"),
            module: Some(parent_fqdn.to_string()),
            visibility: map_access_modifier(None, exported),
            location: ctx.span_location(span),
            signature: None,
            body_hash: ctx.body_hash_of(span),
            attributes: vec![],
            flags: vec![],
        },
        outer_pos,
    );
    for member in &item.members {
        let member_name = ts_enum_member_id_name(&member.id);
        let member_fqdn = format!("{enum_fqdn}::{member_name}");
        ctx.push_symbol_with_doc(
            RawSymbol {
                name: member_name,
                fqdn: member_fqdn,
                kind: Kind::Value,
                language_kind: LanguageKind::from("enum_member"),
                module: Some(enum_fqdn.clone()),
                visibility: map_access_modifier(None, exported),
                location: ctx.span_location(member.span),
                signature: None,
                body_hash: ctx.body_hash_of(member.span),
                attributes: vec![],
                flags: vec![],
            },
            member.span.lo,
        );
    }
}

fn build_function_signature(
    ctx: &TsWalkContext<'_>,
    function: &swc_core::ecma::ast::Function,
) -> Signature {
    let params = function
        .params
        .iter()
        .map(|p| build_param(ctx, p))
        .collect();
    let returns = function
        .return_type
        .as_ref()
        .and_then(|ann| ctx.span_snippet(ann.type_ann.span()))
        .map(TypeRef::new);
    let generic_params = function
        .type_params
        .as_ref()
        .map(|tp| {
            tp.params
                .iter()
                .map(|p| {
                    ctx.span_snippet(p.span)
                        .unwrap_or_else(|| p.name.sym.to_string())
                })
                .collect()
        })
        .unwrap_or_default();
    Signature {
        params,
        returns,
        modifiers: Modifiers {
            is_async: function.is_async,
            deprecated: None,
            generic_params,
            where_clause: None,
        },
        meta: SignatureMeta::default(),
    }
}

fn build_param(ctx: &TsWalkContext<'_>, param: &AstParam) -> Param {
    let (name, ty, default) = render_pat(ctx, &param.pat);
    Param { name, ty, default }
}

fn render_pat(ctx: &TsWalkContext<'_>, pat: &Pat) -> (String, TypeRef, Option<String>) {
    match pat {
        Pat::Ident(b) => {
            let name = b.id.sym.to_string();
            let ty = b
                .type_ann
                .as_ref()
                .and_then(|ann| ctx.span_snippet(ann.type_ann.span()))
                .map_or_else(|| TypeRef::new("any"), TypeRef::new);
            (name, ty, None)
        }
        Pat::Assign(assign) => {
            let (name, ty, _) = render_pat(ctx, &assign.left);
            let default = ctx.span_snippet(assign.right.span());
            (name, ty, default)
        }
        Pat::Rest(rest) => {
            let (name, inner_ty, default) = render_pat(ctx, &rest.arg);
            // RestPat carries its own type_ann (the array type). Prefer it
            // over the inner Pat's annotation, which is typically absent.
            let ty = rest
                .type_ann
                .as_ref()
                .and_then(|ann| ctx.span_snippet(ann.type_ann.span()))
                .map_or(inner_ty, TypeRef::new);
            (format!("...{name}"), ty, default)
        }
        Pat::Array(_) | Pat::Object(_) => {
            let snippet = ctx
                .span_snippet(pat.span())
                .unwrap_or_else(|| "_".to_string());
            (snippet, TypeRef::new("any"), None)
        }
        Pat::Invalid(_) | Pat::Expr(_) => ("_".to_string(), TypeRef::new("any"), None),
    }
}

fn signature_from_declarator(
    ctx: &TsWalkContext<'_>,
    declarator: &VarDeclarator,
) -> Option<Signature> {
    let init = declarator.init.as_ref()?;
    match init.as_ref() {
        swc_core::ecma::ast::Expr::Arrow(arrow) => {
            let params: Vec<Param> = arrow
                .params
                .iter()
                .map(|p| {
                    let (name, ty, default) = render_pat(ctx, p);
                    Param { name, ty, default }
                })
                .collect();
            let returns = arrow
                .return_type
                .as_ref()
                .and_then(|ann| ctx.span_snippet(ann.type_ann.span()))
                .map(TypeRef::new);
            let generic_params = arrow
                .type_params
                .as_ref()
                .map(|tp| {
                    tp.params
                        .iter()
                        .map(|p| {
                            ctx.span_snippet(p.span)
                                .unwrap_or_else(|| p.name.sym.to_string())
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(Signature {
                params,
                returns,
                modifiers: Modifiers {
                    is_async: arrow.is_async,
                    deprecated: None,
                    generic_params,
                    where_clause: None,
                },
                meta: SignatureMeta::default(),
            })
        }
        swc_core::ecma::ast::Expr::Fn(fn_expr) => {
            Some(build_function_signature(ctx, &fn_expr.function))
        }
        _ => None,
    }
}

fn declarator_name(declarator: &VarDeclarator) -> Option<String> {
    match &declarator.name {
        Pat::Ident(b) => Some(b.id.sym.to_string()),
        _ => None,
    }
}

fn method_name_string(key: &swc_core::ecma::ast::PropName) -> Option<String> {
    match key {
        swc_core::ecma::ast::PropName::Ident(i) => Some(i.sym.to_string()),
        swc_core::ecma::ast::PropName::Str(s) => Some(s.value.to_string_lossy().into_owned()),
        swc_core::ecma::ast::PropName::Num(n) => Some(n.value.to_string()),
        swc_core::ecma::ast::PropName::Computed(_) | swc_core::ecma::ast::PropName::BigInt(_) => {
            None
        }
    }
}

/// Bug C-1 — extract a stable name from a TS interface-member key
/// (`TsPropertySignature.key: Box<Expr>` and friends). Common shapes:
/// `Expr::Ident` → identifier name, `Expr::Lit(Str)` → quoted property
/// name. Computed / spread / unknown shapes return `None` so the
/// caller skips emission of an anonymous sub-symbol.
fn interface_member_key_name(expr: &swc_core::ecma::ast::Expr) -> Option<String> {
    match expr {
        swc_core::ecma::ast::Expr::Ident(id) => Some(id.sym.to_string()),
        swc_core::ecma::ast::Expr::Lit(swc_core::ecma::ast::Lit::Str(s)) => {
            Some(s.value.to_string_lossy().into_owned())
        }
        swc_core::ecma::ast::Expr::Lit(swc_core::ecma::ast::Lit::Num(n)) => {
            Some(n.value.to_string())
        }
        _ => None,
    }
}

/// Bug C-1 — extract the textual name of a `TsEnumMember.id`.
/// swc parses both `Ident(Foo)` and `Str("Foo")` flavors (TS lets you
/// write `enum E { "foo bar" = 1 }`).
fn ts_enum_member_id_name(id: &swc_core::ecma::ast::TsEnumMemberId) -> String {
    match id {
        swc_core::ecma::ast::TsEnumMemberId::Ident(i) => i.sym.to_string(),
        swc_core::ecma::ast::TsEnumMemberId::Str(s) => s.value.to_string_lossy().into_owned(),
    }
}

pub(crate) fn render_member_expr_name(expr: &swc_core::ecma::ast::Expr) -> String {
    match expr {
        swc_core::ecma::ast::Expr::Ident(i) => i.sym.to_string(),
        swc_core::ecma::ast::Expr::Member(m) => {
            let prefix = render_member_expr_name(&m.obj);
            let prop = match &m.prop {
                MemberProp::Ident(i) => i.sym.to_string(),
                MemberProp::PrivateName(p) => format!("#{}", p.name),
                MemberProp::Computed(_) => "?".to_string(),
            };
            if prefix.is_empty() {
                prop
            } else {
                format!("{prefix}.{prop}")
            }
        }
        _ => String::new(),
    }
}

fn render_expr_name(expr: &swc_core::ecma::ast::Expr) -> String {
    match expr {
        swc_core::ecma::ast::Expr::Ident(i) => i.sym.to_string(),
        swc_core::ecma::ast::Expr::Member(_) => render_member_expr_name(expr),
        _ => String::new(),
    }
}

fn render_ts_entity_name(expr: &swc_core::ecma::ast::Expr) -> String {
    render_expr_name(expr)
}

fn clamp_line(n: usize) -> u32 {
    let v = u32::try_from(n).unwrap_or(u32::MAX);
    v.max(1)
}

fn clamp_col(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use swc_core::common::{BytePos, FileName, sync::Lrc};
    use swc_core::ecma::ast::EsVersion;
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

    fn run(
        source: &str,
    ) -> (
        Vec<RawSymbol>,
        Vec<RawEdge>,
        Vec<RawDocument>,
        Vec<RawCallSite>,
    ) {
        let (cm, module, comments) = parse_ts(source);
        walk(
            &module,
            "@app",
            "src/index.ts",
            "src",
            cm,
            &PathBuf::from("/tmp/pkg/src/index.ts"),
            &PathBuf::from("/tmp/pkg"),
            None,
            &comments,
        )
    }

    #[test]
    fn function_decl_emits_function_symbol() {
        let (symbols, edges, _docs, _) = run("function foo() {}");
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].kind, Kind::Function);
        assert_eq!(symbols[0].fqdn, "src::foo");
        assert_eq!(symbols[0].visibility, Visibility::Private);
        assert!(edges.is_empty());
    }

    #[test]
    fn export_function_decl_is_public() {
        let (symbols, _, _, _) = run("export function foo() {}");
        assert_eq!(symbols[0].visibility, Visibility::Public);
    }

    #[test]
    fn function_signature_captures_param_types_and_return() {
        let (symbols, _, _, _) =
            run("export function add(a: number, b: number): number { return a + b; }");
        let sig = symbols[0].signature.as_ref().unwrap();
        assert_eq!(sig.params.len(), 2);
        assert_eq!(sig.params[0].name, "a");
        assert_eq!(sig.params[0].ty.display, "number");
        assert_eq!(sig.params[1].name, "b");
        assert_eq!(sig.returns.as_ref().unwrap().display, "number");
    }

    #[test]
    fn async_function_modifier_set() {
        let (symbols, _, _, _) = run("export async function boot() {}");
        assert!(symbols[0].signature.as_ref().unwrap().modifiers.is_async);
    }

    #[test]
    fn function_default_param_captured() {
        let (symbols, _, _, _) = run("export function f(x: number = 7) {}");
        let p = &symbols[0].signature.as_ref().unwrap().params[0];
        assert_eq!(p.default.as_deref(), Some("7"));
    }

    #[test]
    fn rest_param_prefixed_with_ellipsis() {
        let (symbols, _, _, _) = run("export function f(...args: number[]) {}");
        let p = &symbols[0].signature.as_ref().unwrap().params[0];
        assert_eq!(p.name, "...args");
        assert_eq!(p.ty.display, "number[]");
    }

    #[test]
    fn generic_params_captured_as_strings() {
        let (symbols, _, _, _) = run("export function id<T>(x: T): T { return x; }");
        let g = &symbols[0]
            .signature
            .as_ref()
            .unwrap()
            .modifiers
            .generic_params;
        assert_eq!(g.len(), 1);
        assert_eq!(g[0], "T");
    }

    #[test]
    fn class_emits_type_symbol_and_methods() {
        let (symbols, _, _, _) = run("export class Foo { run(): void {} }");
        let foo = symbols.iter().find(|s| s.fqdn == "src::Foo").unwrap();
        assert_eq!(foo.kind, Kind::Type);
        assert_eq!(foo.language_kind.as_str(), "class");
        let run = symbols.iter().find(|s| s.fqdn == "src::Foo::run").unwrap();
        assert_eq!(run.kind, Kind::Function);
        assert_eq!(run.language_kind.as_str(), "method");
    }

    #[test]
    fn class_extends_emits_extends_edge() {
        let (_, edges, _, _) = run("class Foo extends Bar {}");
        let ext: Vec<_> = edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Extends)
            .collect();
        assert_eq!(ext.len(), 1);
        assert_eq!(ext[0].from_fqdn, "src::Foo");
        match &ext[0].to {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "src::Bar"),
            other => panic!("expected unresolved, got {other:?}"),
        }
    }

    #[test]
    fn class_implements_emits_implements_edge() {
        let (_, edges, _, _) = run("class Foo implements IBar {}");
        let imp: Vec<_> = edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Implements)
            .collect();
        assert_eq!(imp.len(), 1);
        assert_eq!(imp[0].from_fqdn, "src::Foo");
        match &imp[0].to {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "src::IBar"),
            other => panic!("expected unresolved, got {other:?}"),
        }
    }

    #[test]
    fn class_method_accessibility_maps_to_visibility() {
        let (symbols, _, _, _) =
            run("class Foo { public a() {} private b() {} protected c() {} d() {} }");
        let a = symbols.iter().find(|s| s.fqdn == "src::Foo::a").unwrap();
        assert_eq!(a.visibility, Visibility::Public);
        let b = symbols.iter().find(|s| s.fqdn == "src::Foo::b").unwrap();
        assert_eq!(b.visibility, Visibility::Private);
        let c = symbols.iter().find(|s| s.fqdn == "src::Foo::c").unwrap();
        assert_eq!(c.visibility, Visibility::Protected);
        let d = symbols.iter().find(|s| s.fqdn == "src::Foo::d").unwrap();
        assert_eq!(d.visibility, Visibility::Public);
    }

    #[test]
    fn interface_emits_type_symbol() {
        let (symbols, _, _, _) = run("export interface IFoo { x: number }");
        assert_eq!(symbols[0].kind, Kind::Type);
        assert_eq!(symbols[0].language_kind.as_str(), "interface");
        assert_eq!(symbols[0].fqdn, "src::IFoo");
    }

    #[test]
    fn interface_extends_emits_extends_edge() {
        let (_, edges, _, _) = run("interface A extends B {}");
        let ext: Vec<_> = edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Extends)
            .collect();
        assert_eq!(ext.len(), 1);
        assert_eq!(ext[0].from_fqdn, "src::A");
    }

    #[test]
    fn type_alias_emits_type_symbol() {
        let (symbols, _, _, _) = run("export type Bytes = Uint8Array;");
        assert_eq!(symbols[0].kind, Kind::Type);
        assert_eq!(symbols[0].language_kind.as_str(), "type_alias");
        assert_eq!(symbols[0].fqdn, "src::Bytes");
    }

    #[test]
    fn enum_emits_type_symbol() {
        let (symbols, _, _, _) = run("export enum Color { Red, Green }");
        assert_eq!(symbols[0].kind, Kind::Type);
        assert_eq!(symbols[0].language_kind.as_str(), "enum");
    }

    #[test]
    fn const_var_emits_value_symbol() {
        let (symbols, _, _, _) = run("export const N = 42;");
        assert_eq!(symbols[0].kind, Kind::Value);
        assert_eq!(symbols[0].language_kind.as_str(), "const");
        assert_eq!(symbols[0].fqdn, "src::N");
    }

    #[test]
    fn arrow_const_emits_function_symbol() {
        let (symbols, _, _, _) = run("export const add = (a: number, b: number): number => a + b;");
        assert_eq!(symbols[0].kind, Kind::Function);
        assert_eq!(symbols[0].language_kind.as_str(), "function");
        let sig = symbols[0].signature.as_ref().unwrap();
        assert_eq!(sig.params.len(), 2);
        assert_eq!(sig.params[0].name, "a");
        assert_eq!(sig.returns.as_ref().unwrap().display, "number");
    }

    #[test]
    fn import_named_emits_import_edge_and_alias() {
        let (_, edges, _, _) = run("import { foo } from './helper';");
        let imp: Vec<_> = edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Imports)
            .collect();
        assert_eq!(imp.len(), 1);
    }

    #[test]
    fn import_default_emits_import_edge() {
        let (_, edges, _, _) = run("import React from 'react';");
        assert!(edges.iter().any(|e| e.kind == EdgeKind::Imports));
    }

    #[test]
    fn import_namespace_emits_import_edge() {
        let (_, edges, _, _) = run("import * as utils from './utils';");
        assert!(edges.iter().any(|e| e.kind == EdgeKind::Imports));
    }

    #[test]
    fn import_side_effect_emits_one_edge_no_alias() {
        let (_, edges, _, _) = run("import 'polyfill';");
        let imp: Vec<_> = edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Imports)
            .collect();
        assert_eq!(imp.len(), 1);
    }

    #[test]
    fn export_default_function_named() {
        let (symbols, _, _, _) = run("export default function foo() {}");
        let foo = symbols.iter().find(|s| s.fqdn == "src::foo").unwrap();
        assert_eq!(foo.kind, Kind::Function);
        assert_eq!(foo.visibility, Visibility::Public);
    }

    #[test]
    fn export_default_function_anonymous_uses_default_name() {
        let (symbols, _, _, _) = run("export default function () {}");
        assert_eq!(symbols[0].fqdn, "src::default");
    }

    #[test]
    fn export_default_class_named() {
        let (symbols, _, _, _) = run("export default class Foo {}");
        let foo = symbols.iter().find(|s| s.fqdn == "src::Foo").unwrap();
        assert_eq!(foo.kind, Kind::Type);
    }

    #[test]
    fn span_locations_are_captured() {
        let (symbols, _, _, _) = run("\n\nexport function foo() {}\n");
        assert_eq!(symbols[0].location.start_line, 3);
    }

    #[test]
    fn body_hash_changes_with_body_content() {
        let (sym_a, _, _, _) = run("export function foo() { return 1; }");
        let (sym_b, _, _, _) = run("export function foo() { return 2; }");
        assert_ne!(sym_a[0].body_hash, sym_b[0].body_hash);
    }

    fn expect_emit(outcome: ResolutionOutcome) -> CallTarget {
        match outcome {
            ResolutionOutcome::Emit(target) => target,
            other => panic!("expected ResolutionOutcome::Emit, got {other:?}"),
        }
    }

    #[test]
    fn resolve_call_via_alias_table() {
        let (cm, _module, comments) = parse_ts("");
        let mut ctx = TsWalkContext::new(
            "src/index.ts".into(),
            "@app".into(),
            "src".into(),
            cm,
            PathBuf::new(),
            PathBuf::new(),
            None,
            &comments,
        );
        ctx.add_import_alias(
            "Foo".into(),
            ResolvedImport {
                target: ResolvedOrUnresolved::Unresolved {
                    name: "@app::src::foo::Foo".into(),
                },
            },
        );
        let target = expect_emit(ctx.resolve_call("Foo", "src"));
        assert!(
            target.via_builtin.is_none(),
            "alias resolution carries no via_builtin"
        );
        match target.to {
            ResolvedOrUnresolved::Unresolved { name } => {
                assert_eq!(name, "@app::src::foo::Foo");
            }
            other => panic!("expected unresolved canonical via alias, got {other:?}"),
        }
    }

    #[test]
    fn resolve_call_module_local_resolved_when_defined() {
        let (cm, _module, comments) = parse_ts("");
        let mut ctx = TsWalkContext::new(
            "src/index.ts".into(),
            "@app".into(),
            "src".into(),
            cm,
            PathBuf::new(),
            PathBuf::new(),
            None,
            &comments,
        );
        ctx.core.defined_fqdns.insert("src::foo".into());
        let target = expect_emit(ctx.resolve_call("foo", "src"));
        assert!(
            target.via_builtin.is_none(),
            "module-local resolution carries no via_builtin"
        );
        match target.to {
            ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "src::foo"),
            other => panic!("expected resolved, got {other:?}"),
        }
    }

    #[test]
    fn resolve_call_falls_back_to_unresolved_module_local() {
        let (cm, _module, comments) = parse_ts("");
        let ctx = TsWalkContext::new(
            "src/index.ts".into(),
            "@app".into(),
            "src".into(),
            cm,
            PathBuf::new(),
            PathBuf::new(),
            None,
            &comments,
        );
        let target = expect_emit(ctx.resolve_call("nope", "src"));
        assert!(
            target.via_builtin.is_none(),
            "fall-through unresolved carries no via_builtin"
        );
        match target.to {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "src::nope"),
            other => panic!("expected unresolved module-local, got {other:?}"),
        }
    }

    #[test]
    fn stage3e1_resolve_call_builtin_edge_emits_synthetic_with_tag() {
        // `Math` is registered in the TS builtins as `BuiltinTier::Edge`
        // with tag `BuiltinTag::Math` — resolution should produce a
        // synthetic FQDN AND propagate the tag for downstream attribute
        // stamping (`via-builtin` + `builtin-math`).
        let (cm, _module, comments) = parse_ts("");
        let ctx = TsWalkContext::new(
            "src/index.ts".into(),
            "@app".into(),
            "src".into(),
            cm,
            PathBuf::new(),
            PathBuf::new(),
            None,
            &comments,
        );
        let target = expect_emit(ctx.resolve_call("Math", "src"));
        match &target.to {
            ResolvedOrUnresolved::Resolved { fqdn } => {
                assert!(
                    fqdn.starts_with("<builtin>::"),
                    "expected synthetic FQDN, got {fqdn}"
                );
                assert!(
                    fqdn.ends_with("::Math"),
                    "expected synthetic to end with ::Math, got {fqdn}"
                );
            }
            other => panic!("expected Resolved synthetic for Edge tier, got {other:?}"),
        }
        match target.via_builtin {
            Some(BuiltinTag::Math) => {}
            other => panic!("expected via_builtin = Some(Math), got {other:?}"),
        }
    }

    #[test]
    fn stage3e1_resolve_call_builtin_drop_returns_none() {
        // `Array` is registered as `BuiltinTier::Drop` in the TS builtins —
        // structural container noise, no edge should be emitted. Resolver
        // returns `None` so callers can skip without further inspection.
        let (cm, _module, comments) = parse_ts("");
        let ctx = TsWalkContext::new(
            "src/index.ts".into(),
            "@app".into(),
            "src".into(),
            cm,
            PathBuf::new(),
            PathBuf::new(),
            None,
            &comments,
        );
        assert!(
            matches!(ctx.resolve_call("Array", "src"), ResolutionOutcome::Drop),
            "Drop-tier builtin must resolve to ResolutionOutcome::Drop"
        );
    }

    #[test]
    fn stage3e1b_resolve_call_attribute_tier_returns_tag() {
        // `Promise` is `BuiltinTier::Attribute` (Async tag). Stage 3e-1b
        // surfaces the tag through `ResolutionOutcome::Attribute(tag)`
        // so callers can flag the enclosing source symbol.
        let (cm, _module, comments) = parse_ts("");
        let ctx = TsWalkContext::new(
            "src/index.ts".into(),
            "@app".into(),
            "src".into(),
            cm,
            PathBuf::new(),
            PathBuf::new(),
            None,
            &comments,
        );
        match ctx.resolve_call("Promise", "src") {
            ResolutionOutcome::Attribute(BuiltinTag::Async) => {}
            other => panic!("expected Attribute(Async) for Promise, got {other:?}"),
        }
    }

    #[test]
    fn stage3e1b_register_attribute_flag_flushes_to_symbol() {
        // End-to-end : register a flag against an FQDN, push a matching
        // symbol, drain via `into_outputs`, assert the symbol picks the
        // flag up.
        let (cm, _module, comments) = parse_ts("");
        let mut ctx = TsWalkContext::new(
            "src/index.ts".into(),
            "@app".into(),
            "src".into(),
            cm,
            PathBuf::new(),
            PathBuf::new(),
            None,
            &comments,
        );
        ctx.push_symbol(RawSymbol {
            name: "doStuff".into(),
            fqdn: "src::doStuff".into(),
            kind: Kind::Function,
            language_kind: LanguageKind::from("function"),
            module: Some("src".into()),
            visibility: Visibility::Public,
            location: SymbolLocation {
                file: "src/index.ts".into(),
                start_line: 1,
                end_line: 5,
                start_col: 0,
                end_col: 1,
            },
            signature: None,
            body_hash: None,
            attributes: vec![],
            flags: vec![],
        });
        ctx.register_attribute_flag("src::doStuff", &BuiltinTag::Async);
        ctx.register_attribute_flag("src::doStuff", &BuiltinTag::Iter);
        // Duplicate register is a no-op (HashSet dedup).
        ctx.register_attribute_flag("src::doStuff", &BuiltinTag::Async);

        let (symbols, _, _, _) = ctx.into_outputs();
        let s = symbols
            .into_iter()
            .find(|s| s.fqdn == "src::doStuff")
            .expect("symbol must be retained");
        assert!(s.flags.contains(&"async".to_string()), "got {:?}", s.flags);
        assert!(s.flags.contains(&"iter".to_string()), "got {:?}", s.flags);
        assert_eq!(s.flags.len(), 2, "dedup must yield exactly 2 flags");
    }

    #[test]
    fn stage3e1_resolve_call_alias_overrides_builtin() {
        // An explicit import alias shadows a global builtin name. The
        // alias path wins (carries no via_builtin) — guards against the
        // tier dispatch accidentally taking precedence over user code
        // that locally rebinds `Promise`, `Math`, etc.
        let (cm, _module, comments) = parse_ts("");
        let mut ctx = TsWalkContext::new(
            "src/index.ts".into(),
            "@app".into(),
            "src".into(),
            cm,
            PathBuf::new(),
            PathBuf::new(),
            None,
            &comments,
        );
        ctx.add_import_alias(
            "Promise".into(),
            ResolvedImport {
                target: ResolvedOrUnresolved::Resolved {
                    fqdn: "@app::src::polyfill::Promise".into(),
                },
            },
        );
        let target = expect_emit(ctx.resolve_call("Promise", "src"));
        assert!(
            target.via_builtin.is_none(),
            "alias-shadowed Promise is NOT a builtin"
        );
        match target.to {
            ResolvedOrUnresolved::Resolved { fqdn } => {
                assert_eq!(fqdn, "@app::src::polyfill::Promise");
            }
            other => panic!("expected aliased Resolved, got {other:?}"),
        }
    }

    #[test]
    fn _bytepos_unused() {
        // Touch BytePos to keep the import warning-free if test setup grows.
        let _ = BytePos(0);
        let _ = Span::default().lo();
    }
}

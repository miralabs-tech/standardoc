//! Per-decl-kind symbol extraction for the TS/JS walker.
//!
//! Extracted from `walk.rs` (Phase 3.2+ structure split): one `extract_*`
//! per top-level decl kind (fn / class / interface / type alias / enum /
//! var) plus the class-body / interface-body iterators. The heritage
//! edge emitter (`push_heritage_edge`) and the signature/render helpers
//! consumed exclusively by extraction (`build_function_signature`,
//! `build_param`, `render_pat`, `signature_from_declarator`,
//! `declarator_name`, `method_name_string`, `interface_member_key_name`,
//! `ts_enum_member_id_name`) live here too.

use standardoc_ir::{
    DeclKind, EdgeKind, EntryPointKind, Kind, LanguageKind, Modifiers, Param, RawEdge, RawSymbol,
    Signature, SignatureMeta, TypeRef, Visibility,
};
use swc_core::common::{BytePos, Span, Spanned};
use swc_core::ecma::ast::{
    Class, ClassDecl, ClassMember, ClassMethod, FnDecl, Param as AstParam, Pat, TsEnumDecl,
    TsInterfaceDecl, TsTypeAliasDecl, VarDecl, VarDeclarator,
};

use super::super::helpers::map_access_modifier;
use super::super::visit;
use super::{
    CallTarget, ResolutionOutcome, TsWalkContext, render_expr_name, render_ts_entity_name,
};

/// Phase 3 (Flow) — first-pass entry-point detector for TS/JS
/// function declarations. The Rust/C convention of a `main` symbol
/// sitting at the crate root doesn't map cleanly to TS: there's no
/// single declarable "binary entry" — what runs is whatever the
/// runtime loads (Node / Bun / browser bundle), with the package's
/// `"main"` / `"bin"` field naming a FILE, not a symbol. Without
/// wiring that lookup through, we use a positional heuristic that
/// catches the common script shape (`main.ts` / `index.ts` with a
/// `function main()` at its top level) without false-positiving on
/// nested helpers:
///
///   - `BinaryMain`: a fn named `main` whose `parent_fqdn` has at
///     most one `::` segment — i.e. it sits at the project root
///     (`<project>::main`) OR at the root of a top-level file
///     (`<project>::<file>::main`). Deeper paths
///     (`<project>::<dir>::<file>::main`) are *not* tagged: a `main`
///     buried inside a subfolder almost always means "the function
///     in this module that does the main thing", not the runtime
///     entry-point.
///
/// `PublicApi` (an export that ends up re-exported from the package
/// barrel) is deferred — it would need export-graph walking across
/// the project. `FfiExport` has no clean TS equivalent (FFI lives in
/// the wasm-bindgen Rust glue, already covered by the Rust pass).
pub(super) fn classify_ts_fn_entry_point(name: &str, parent_fqdn: &str) -> Option<EntryPointKind> {
    if name == "main" && parent_fqdn.matches("::").count() <= 1 {
        return Some(EntryPointKind::BinaryMain);
    }
    None
}

/// Stamp the standard heritage edge (`Extends` / `Implements`) for a
/// class / interface heritage clause, folding `via-builtin` / `builtin-<slug>`
/// attrs when the target is a synthetic Edge-tier builtin (matches the
/// Rust UsesType emission pattern).
pub(super) fn push_heritage_edge(
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
        receiver_type: None,
    });
}

pub(super) fn extract_fn_decl(
    ctx: &TsWalkContext<'_>,
    item: &FnDecl,
    parent_fqdn: &str,
    exported: bool,
) -> RawSymbol {
    let name = item.ident.sym.to_string();
    let fqdn = format!("{parent_fqdn}::{name}");
    let span = item.function.span;
    let signature = build_function_signature(ctx, &item.function);
    let entry_point = classify_ts_fn_entry_point(&name, parent_fqdn);
    RawSymbol {
        decl_kind: Some(DeclKind::Function),
        implements_trait: None,
        receiver_type: None,
        entry_point,
        name,
        fqdn,
        kind: Kind::Callable,
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

pub(super) fn extract_class_decl(
    ctx: &mut TsWalkContext<'_>,
    item: &ClassDecl,
    parent_fqdn: &str,
    exported: bool,
    outer_pos: BytePos,
) {
    let name = item.ident.sym.to_string();
    extract_class_inner(ctx, &name, &item.class, parent_fqdn, exported, outer_pos);
}

pub(super) fn extract_class_inner(
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
            decl_kind: Some(DeclKind::Class),
            implements_trait: None,
            receiver_type: None,
            entry_point: None,
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

pub(super) fn extract_constructor(
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
        decl_kind: Some(DeclKind::Constructor),
        implements_trait: None,
        receiver_type: None,
        entry_point: None,
        name: "constructor".to_string(),
        fqdn,
        kind: Kind::Callable,
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

pub(super) fn extract_class_prop(
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
        decl_kind: Some(DeclKind::Field),
        implements_trait: None,
        receiver_type: None,
        entry_point: None,
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

pub(super) fn extract_private_method(
    ctx: &TsWalkContext<'_>,
    method: &swc_core::ecma::ast::PrivateMethod,
    class_fqdn: &str,
    method_name: &str,
) -> RawSymbol {
    let fqdn = format!("{class_fqdn}::{method_name}");
    let span = method.span;
    let decl_kind = match method.kind {
        swc_core::ecma::ast::MethodKind::Getter => DeclKind::Getter,
        swc_core::ecma::ast::MethodKind::Setter => DeclKind::Setter,
        swc_core::ecma::ast::MethodKind::Method => DeclKind::Method,
    };
    RawSymbol {
        decl_kind: Some(decl_kind),
        implements_trait: None,
        receiver_type: None,
        entry_point: None,
        name: method_name.to_string(),
        fqdn,
        kind: Kind::Callable,
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

pub(super) fn extract_private_prop(
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
        decl_kind: Some(DeclKind::Field),
        implements_trait: None,
        receiver_type: None,
        entry_point: None,
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

pub(super) fn extract_method(
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
    let decl_kind = match method.kind {
        swc_core::ecma::ast::MethodKind::Getter => DeclKind::Getter,
        swc_core::ecma::ast::MethodKind::Setter => DeclKind::Setter,
        swc_core::ecma::ast::MethodKind::Method => DeclKind::Method,
    };
    RawSymbol {
        decl_kind: Some(decl_kind),
        implements_trait: None,
        receiver_type: None,
        entry_point: None,
        name: method_name.to_string(),
        fqdn,
        kind: Kind::Callable,
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

pub(super) fn extract_var_decl(
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
        let kind = signature.as_ref().map_or(Kind::Value, |_| Kind::Callable);
        let decl_kind = if signature.is_some() {
            DeclKind::Function
        } else {
            match item.kind {
                swc_core::ecma::ast::VarDeclKind::Const => DeclKind::Const,
                swc_core::ecma::ast::VarDeclKind::Let | swc_core::ecma::ast::VarDeclKind::Var => {
                    DeclKind::Var
                }
            }
        };
        let language_kind = if signature.is_some() {
            LanguageKind::from("function")
        } else {
            LanguageKind::from(language_kind)
        };
        ctx.push_symbol_with_doc(
            RawSymbol {
                decl_kind: Some(decl_kind),
                implements_trait: None,
                receiver_type: None,
                entry_point: None,
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

pub(super) fn extract_interface_decl(
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
            decl_kind: Some(DeclKind::Interface),
            implements_trait: None,
            receiver_type: None,
            entry_point: None,
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
                        decl_kind: Some(DeclKind::Field),
                        implements_trait: None,
                        receiver_type: None,
                        entry_point: None,
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
                        decl_kind: Some(DeclKind::Method),
                        implements_trait: None,
                        receiver_type: None,
                        entry_point: None,
                        name: member_name,
                        fqdn: member_fqdn.clone(),
                        kind: Kind::Callable,
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
                        decl_kind: Some(DeclKind::Getter),
                        implements_trait: None,
                        receiver_type: None,
                        entry_point: None,
                        name: member_name,
                        fqdn: member_fqdn.clone(),
                        kind: Kind::Callable,
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
                        decl_kind: Some(DeclKind::Setter),
                        implements_trait: None,
                        receiver_type: None,
                        entry_point: None,
                        name: member_name,
                        fqdn: member_fqdn.clone(),
                        kind: Kind::Callable,
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

pub(super) fn extract_type_alias_decl(
    ctx: &TsWalkContext<'_>,
    item: &TsTypeAliasDecl,
    parent_fqdn: &str,
    exported: bool,
) -> RawSymbol {
    let name = item.id.sym.to_string();
    let fqdn = format!("{parent_fqdn}::{name}");
    let span = item.span;
    RawSymbol {
        decl_kind: Some(DeclKind::TypeAlias),
        implements_trait: None,
        receiver_type: None,
        entry_point: None,
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
pub(super) fn extract_enum_decl(
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
            decl_kind: Some(DeclKind::Enum),
            implements_trait: None,
            receiver_type: None,
            entry_point: None,
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
                decl_kind: Some(DeclKind::EnumVariant),
                implements_trait: None,
                receiver_type: None,
                entry_point: None,
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

pub(super) fn build_function_signature(
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

pub(super) fn declarator_name(declarator: &VarDeclarator) -> Option<String> {
    match &declarator.name {
        Pat::Ident(b) => Some(b.id.sym.to_string()),
        _ => None,
    }
}

pub(super) fn method_name_string(key: &swc_core::ecma::ast::PropName) -> Option<String> {
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

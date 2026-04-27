//! Rust language provider for Standardoc.
//!
//! Uses `syn` (full + visit + span-locations features) to discover documentable
//! items. Emits a [`DiscoveredSymbol`] per:
//! - free function / async fn / unsafe fn / const fn
//! - struct / enum / union / type alias
//! - trait
//! - impl block items (methods, associated consts, associated types)
//! - top-level const / static
//! - module (nested `mod foo { ... }`)
//! - `macro_rules!` macro
//!
//! The provider itself does **not** interpret `@doc` annotations. It extracts
//! the Rustdoc comment preceding each symbol into `leading_comment` and lets
//! the core extractor parse tags out of it.

use proc_macro2::Span;
use quote::ToTokens;
use standardoc_core::lang::{DiscoveredSymbol, LanguageProvider, ParseError};
use standardoc_core::model::{
    CommentDelimiters, CommentStyles, ParamInfo, RefKind, References, SourceRange, SymbolInfo,
    SymbolKind, TypeInfo, Visibility,
};
use std::path::Path;
use std::sync::OnceLock;
use syn::spanned::Spanned;
use syn::{
    parse_quote, Attribute, Block, Expr, ExprLit, File as SynFile, FnArg, Generics, Ident,
    ImplItem, Item, ItemConst, ItemEnum, ItemFn, ItemImpl, ItemMod, ItemStatic, ItemStruct,
    ItemTrait, ItemType, ItemUnion, Lit, Meta, MetaNameValue, ReturnType, Signature, TraitItem,
    Type, Visibility as SynVis,
};

/// @doc lang.providers.rust Rust
/// @description Native Rust language provider — discovers items, methods, trait impls via the `syn` full-AST parser.
/// @crate standardoc-lang-rust
/// @backend syn
#[derive(Debug, Default, Clone, Copy)]
pub struct RustProvider;

impl LanguageProvider for RustProvider {
    fn id(&self) -> &'static str {
        "rust"
    }

    fn extensions(&self) -> &[&'static str] {
        &[".rs"]
    }

    fn comment_styles(&self) -> &CommentStyles {
        rust_comment_styles()
    }

    fn discover_symbols(
        &self,
        content: &str,
        path: &Path,
    ) -> Result<Vec<DiscoveredSymbol>, ParseError> {
        let file = syn::parse_file(content).map_err(|err| ParseError::Syntax {
            path: path.display().to_string(),
            message: err.to_string(),
        })?;

        let mut collector = Collector::default();
        collector.walk_items(&file.items);
        Ok(collector.symbols)
    }
}

fn rust_comment_styles() -> &'static CommentStyles {
    static STYLES: OnceLock<CommentStyles> = OnceLock::new();
    STYLES.get_or_init(|| CommentStyles {
        single: vec!["//".to_owned()],
        multi: Some(CommentDelimiters {
            start: "/*".to_owned(),
            end: "*/".to_owned(),
        }),
        doc_single: vec!["///".to_owned(), "//!".to_owned()],
        doc_multi: Some(CommentDelimiters {
            start: "/**".to_owned(),
            end: "*/".to_owned(),
        }),
    })
}

#[derive(Default)]
struct Collector {
    path: Vec<String>,
    symbols: Vec<DiscoveredSymbol>,
}

impl Collector {
    fn walk_items(&mut self, items: &[Item]) {
        for item in items {
            self.visit(item);
        }
    }

    fn visit(&mut self, item: &Item) {
        match item {
            Item::Fn(f) => self.emit_fn(f),
            Item::Struct(s) => self.emit_struct(s),
            Item::Enum(e) => self.emit_enum(e),
            Item::Union(u) => self.emit_union(u),
            Item::Trait(t) => self.emit_trait(t),
            Item::Impl(i) => self.emit_impl(i),
            Item::Const(c) => self.emit_const(c),
            Item::Static(s) => self.emit_static(s),
            Item::Type(t) => self.emit_type_alias(t),
            Item::Mod(m) => self.emit_mod(m),
            Item::Macro(m) => {
                if let Some(ident) = &m.ident {
                    self.emit_macro_rules(ident, &m.attrs);
                }
            }
            _ => {}
        }
    }

    fn fqn_for(&self, name: &str) -> Vec<String> {
        let mut fqn = self.path.clone();
        fqn.push(name.to_owned());
        fqn
    }

    fn push(&mut self, fqn: Vec<String>, symbol: SymbolInfo, span: Span, attrs: &[Attribute]) {
        let symbol_line_start = span_to_range(span).line_start;
        let leading_comment_line_start = first_doc_attr_line(attrs)
            .filter(|line| *line + 1 < symbol_line_start);
        self.symbols.push(DiscoveredSymbol {
            fqn,
            symbol,
            source_range: span_to_range(span),
            leading_comment: extract_doc_comment(attrs),
            leading_comment_line_start,
        });
    }

    fn emit_fn(&mut self, f: &ItemFn) {
        let fqn = self.fqn_for(&f.sig.ident.to_string());
        let symbol = symbol_from_fn(&f.sig, &f.attrs, &f.vis, SymbolKind::Function);
        self.push(fqn, symbol, f.sig.ident.span(), &f.attrs);
    }

    fn emit_struct(&mut self, s: &ItemStruct) {
        let fqn = self.fqn_for(&s.ident.to_string());
        let symbol = SymbolInfo {
            kind: SymbolKind::Struct,
            visibility: to_visibility(&s.vis),
            signature: header(&s.vis, "struct", &s.ident, &s.generics),
            params: vec![],
            returns: None,
            generics: generic_params(&s.generics),
            decorators: decorators_from_attrs(&s.attrs),
            is_async: false,
            is_deprecated: has_deprecated(&s.attrs),
            references: collect_field_references(&s.fields), // Fields impls IntoIterator
        };
        self.push(fqn, symbol, s.ident.span(), &s.attrs);
    }

    fn emit_enum(&mut self, e: &ItemEnum) {
        let fqn = self.fqn_for(&e.ident.to_string());
        let symbol = SymbolInfo {
            kind: SymbolKind::Enum,
            visibility: to_visibility(&e.vis),
            signature: header(&e.vis, "enum", &e.ident, &e.generics),
            params: vec![],
            returns: None,
            generics: generic_params(&e.generics),
            decorators: decorators_from_attrs(&e.attrs),
            is_async: false,
            is_deprecated: has_deprecated(&e.attrs),
            references: collect_enum_references(e),
        };
        self.push(fqn, symbol, e.ident.span(), &e.attrs);
    }

    fn emit_union(&mut self, u: &ItemUnion) {
        let fqn = self.fqn_for(&u.ident.to_string());
        let symbol = SymbolInfo {
            kind: SymbolKind::Struct,
            visibility: to_visibility(&u.vis),
            signature: header(&u.vis, "union", &u.ident, &u.generics),
            params: vec![],
            returns: None,
            generics: generic_params(&u.generics),
            decorators: decorators_from_attrs(&u.attrs),
            is_async: false,
            is_deprecated: has_deprecated(&u.attrs),
            references: collect_field_references(&u.fields.named),
        };
        self.push(fqn, symbol, u.ident.span(), &u.attrs);
    }

    fn emit_trait(&mut self, t: &ItemTrait) {
        let fqn = self.fqn_for(&t.ident.to_string());
        let symbol = SymbolInfo {
            kind: SymbolKind::Trait,
            visibility: to_visibility(&t.vis),
            signature: header(&t.vis, "trait", &t.ident, &t.generics),
            params: vec![],
            returns: None,
            generics: generic_params(&t.generics),
            decorators: decorators_from_attrs(&t.attrs),
            is_async: false,
            is_deprecated: has_deprecated(&t.attrs),
            references: References::default(),
        };
        self.push(fqn, symbol, t.ident.span(), &t.attrs);

        self.path.push(t.ident.to_string());
        for item in &t.items {
            match item {
                TraitItem::Fn(f) => {
                    let fqn = self.fqn_for(&f.sig.ident.to_string());
                    let method =
                        symbol_from_fn(&f.sig, &f.attrs, &SynVis::Inherited, SymbolKind::Method);
                    self.push(fqn, method, f.sig.ident.span(), &f.attrs);
                }
                TraitItem::Const(c) => {
                    let fqn = self.fqn_for(&c.ident.to_string());
                    let sym = SymbolInfo {
                        kind: SymbolKind::Const,
                        visibility: Visibility::Inherited,
                        signature: format!("const {}: {}", c.ident, pretty_type(&c.ty)),
                        params: vec![],
                        returns: None,
                        generics: vec![],
                        decorators: decorators_from_attrs(&c.attrs),
                        is_async: false,
                        is_deprecated: has_deprecated(&c.attrs),
                        references: References::default(),
                    };
                    self.push(fqn, sym, c.ident.span(), &c.attrs);
                }
                TraitItem::Type(t) => {
                    let fqn = self.fqn_for(&t.ident.to_string());
                    let sym = SymbolInfo {
                        kind: SymbolKind::TypeAlias,
                        visibility: Visibility::Inherited,
                        signature: format!("type {}{}", t.ident, generics_tokens(&t.generics)),
                        params: vec![],
                        returns: None,
                        generics: generic_params(&t.generics),
                        decorators: decorators_from_attrs(&t.attrs),
                        is_async: false,
                        is_deprecated: has_deprecated(&t.attrs),
                        references: References::default(),
                    };
                    self.push(fqn, sym, t.ident.span(), &t.attrs);
                }
                _ => {}
            }
        }
        self.path.pop();
    }

    fn emit_impl(&mut self, i: &ItemImpl) {
        // For `impl Trait for Type` blocks, items use the trait's visibility
        // (there's no way to write `pub fn` on a trait method implementation),
        // so `to_visibility` returns `Inherited` and the extractor's default
        // `Public` inclusion drops them. They're semantically as visible as
        // the trait itself — we force `Public` here so audits see them.
        let is_trait_impl = i.trait_.is_some();
        let trait_name = impl_trait_name(i);

        let prefix = impl_prefix(i);
        self.path.push(prefix);
        for item in &i.items {
            match item {
                ImplItem::Fn(f) => {
                    let fqn = self.fqn_for(&f.sig.ident.to_string());
                    let mut sym = symbol_from_fn(&f.sig, &f.attrs, &f.vis, SymbolKind::Method);
                    if is_trait_impl && sym.visibility == Visibility::Inherited {
                        sym.visibility = Visibility::Public;
                    }
                    // Tag the method with `Implements -> Trait` so an agent
                    // calling `find_implementations(Trait)` can recover the
                    // implementor type from the parent FQN.
                    if let Some(name) = trait_name.as_ref() {
                        sym.references.push(RefKind::Implements, name.clone(), 0);
                    }
                    self.push(fqn, sym, f.sig.ident.span(), &f.attrs);
                }
                ImplItem::Const(c) => {
                    let fqn = self.fqn_for(&c.ident.to_string());
                    let visibility = if is_trait_impl {
                        Visibility::Public
                    } else {
                        to_visibility(&c.vis)
                    };
                    let sym = SymbolInfo {
                        kind: SymbolKind::Const,
                        visibility,
                        signature: format!("const {}: {}", c.ident, pretty_type(&c.ty)),
                        params: vec![],
                        returns: None,
                        generics: vec![],
                        decorators: decorators_from_attrs(&c.attrs),
                        is_async: false,
                        is_deprecated: has_deprecated(&c.attrs),
                        references: References::default(),
                    };
                    self.push(fqn, sym, c.ident.span(), &c.attrs);
                }
                ImplItem::Type(t) => {
                    let fqn = self.fqn_for(&t.ident.to_string());
                    let visibility = if is_trait_impl {
                        Visibility::Public
                    } else {
                        to_visibility(&t.vis)
                    };
                    let sym = SymbolInfo {
                        kind: SymbolKind::TypeAlias,
                        visibility,
                        signature: format!("type {}{}", t.ident, generics_tokens(&t.generics)),
                        params: vec![],
                        returns: None,
                        generics: generic_params(&t.generics),
                        decorators: decorators_from_attrs(&t.attrs),
                        is_async: false,
                        is_deprecated: has_deprecated(&t.attrs),
                        references: References::default(),
                    };
                    self.push(fqn, sym, t.ident.span(), &t.attrs);
                }
                _ => {}
            }
        }
        self.path.pop();
    }

    fn emit_const(&mut self, c: &ItemConst) {
        let fqn = self.fqn_for(&c.ident.to_string());
        let symbol = SymbolInfo {
            kind: SymbolKind::Const,
            visibility: to_visibility(&c.vis),
            signature: format!(
                "{}const {}: {}",
                vis_prefix(&c.vis),
                c.ident,
                pretty_type(&c.ty)
            ),
            params: vec![],
            returns: None,
            generics: vec![],
            decorators: decorators_from_attrs(&c.attrs),
            is_async: false,
            is_deprecated: has_deprecated(&c.attrs),
            references: References::default(),
        };
        self.push(fqn, symbol, c.ident.span(), &c.attrs);
    }

    fn emit_static(&mut self, s: &ItemStatic) {
        let fqn = self.fqn_for(&s.ident.to_string());
        let symbol = SymbolInfo {
            kind: SymbolKind::Static,
            visibility: to_visibility(&s.vis),
            signature: format!(
                "{}static {}: {}",
                vis_prefix(&s.vis),
                s.ident,
                pretty_type(&s.ty)
            ),
            params: vec![],
            returns: None,
            generics: vec![],
            decorators: decorators_from_attrs(&s.attrs),
            is_async: false,
            is_deprecated: has_deprecated(&s.attrs),
            references: References::default(),
        };
        self.push(fqn, symbol, s.ident.span(), &s.attrs);
    }

    fn emit_type_alias(&mut self, t: &ItemType) {
        let fqn = self.fqn_for(&t.ident.to_string());
        let symbol = SymbolInfo {
            kind: SymbolKind::TypeAlias,
            visibility: to_visibility(&t.vis),
            signature: format!(
                "{}type {}{} = {}",
                vis_prefix(&t.vis),
                t.ident,
                generics_tokens(&t.generics),
                pretty_type(&t.ty)
            ),
            params: vec![],
            returns: None,
            generics: generic_params(&t.generics),
            decorators: decorators_from_attrs(&t.attrs),
            is_async: false,
            is_deprecated: has_deprecated(&t.attrs),
            references: References::default(),
        };
        self.push(fqn, symbol, t.ident.span(), &t.attrs);
    }

    fn emit_mod(&mut self, m: &ItemMod) {
        // External declarations (`mod foo;`) point at a sibling file that is
        // scanned on its own — emitting a symbol here would produce a noisy
        // duplicate. Skip them entirely.
        let Some((_, items)) = &m.content else {
            return;
        };

        let fqn = self.fqn_for(&m.ident.to_string());
        let symbol = SymbolInfo {
            kind: SymbolKind::Module,
            visibility: to_visibility(&m.vis),
            signature: format!("{}mod {}", vis_prefix(&m.vis), m.ident),
            params: vec![],
            returns: None,
            generics: vec![],
            decorators: decorators_from_attrs(&m.attrs),
            is_async: false,
            is_deprecated: has_deprecated(&m.attrs),
            references: References::default(),
        };
        self.push(fqn, symbol, m.ident.span(), &m.attrs);

        self.path.push(m.ident.to_string());
        self.walk_items(items);
        self.path.pop();
    }

    fn emit_macro_rules(&mut self, ident: &Ident, attrs: &[Attribute]) {
        let fqn = self.fqn_for(&ident.to_string());
        let symbol = SymbolInfo {
            kind: SymbolKind::Macro,
            visibility: Visibility::Public,
            signature: format!("macro_rules! {ident}"),
            params: vec![],
            returns: None,
            generics: vec![],
            decorators: decorators_from_attrs(attrs),
            is_async: false,
            is_deprecated: has_deprecated(attrs),
            references: References::default(),
        };
        self.push(fqn, symbol, ident.span(), attrs);
    }
}

fn symbol_from_fn(
    sig: &Signature,
    attrs: &[Attribute],
    vis: &SynVis,
    kind: SymbolKind,
) -> SymbolInfo {
    let params = sig.inputs.iter().map(param_from_fn_arg).collect();
    let returns = match &sig.output {
        ReturnType::Default => None,
        ReturnType::Type(_, ty) => Some(TypeInfo {
            repr: pretty_type(ty),
        }),
    };
    let references = collect_fn_references(sig);
    SymbolInfo {
        kind,
        visibility: to_visibility(vis),
        signature: pretty_fn_signature(sig, vis),
        params,
        returns,
        generics: generic_params(&sig.generics),
        decorators: decorators_from_attrs(attrs),
        is_async: sig.asyncness.is_some(),
        is_deprecated: has_deprecated(attrs),
        references,
    }
}

fn param_from_fn_arg(arg: &FnArg) -> ParamInfo {
    match arg {
        FnArg::Receiver(r) => ParamInfo {
            name: "self".to_owned(),
            type_repr: Some(pretty_type(&r.ty)),
            default: None,
            is_optional: false,
            is_variadic: false,
        },
        FnArg::Typed(pt) => {
            let name = match &*pt.pat {
                syn::Pat::Ident(pi) => pi.ident.to_string(),
                other => other.to_token_stream().to_string(),
            };
            ParamInfo {
                name,
                type_repr: Some(pretty_type(&pt.ty)),
                default: None,
                is_optional: false,
                is_variadic: false,
            }
        }
    }
}

fn to_visibility(v: &SynVis) -> Visibility {
    match v {
        SynVis::Public(_) => Visibility::Public,
        SynVis::Restricted(r) => {
            if r.path.is_ident("crate") {
                Visibility::Crate
            } else {
                Visibility::Internal
            }
        }
        SynVis::Inherited => Visibility::Inherited,
    }
}

fn vis_prefix(v: &SynVis) -> String {
    match v {
        SynVis::Public(_) => "pub ".to_owned(),
        SynVis::Restricted(r) => format!("{} ", r.to_token_stream()),
        SynVis::Inherited => String::new(),
    }
}

fn header(vis: &SynVis, keyword: &str, ident: &Ident, generics: &Generics) -> String {
    format!(
        "{}{} {}{}",
        vis_prefix(vis),
        keyword,
        ident,
        generics_tokens(generics)
    )
}

fn generics_tokens(g: &Generics) -> String {
    let s = g.to_token_stream().to_string();
    if s.trim().is_empty() {
        String::new()
    } else {
        s
    }
}

fn generic_params(g: &Generics) -> Vec<String> {
    g.params
        .iter()
        .map(|p| p.to_token_stream().to_string())
        .collect()
}

fn decorators_from_attrs(attrs: &[Attribute]) -> Vec<String> {
    attrs
        .iter()
        .filter(|a| !a.path().is_ident("doc"))
        .map(|a| a.to_token_stream().to_string())
        .collect()
}

fn has_deprecated(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| a.path().is_ident("deprecated"))
}

fn first_doc_attr_line(attrs: &[Attribute]) -> Option<u32> {
    attrs
        .iter()
        .find(|a| a.path().is_ident("doc"))
        .map(|a| u32::try_from(a.path().span().start().line).unwrap_or(u32::MAX))
}

fn extract_doc_comment(attrs: &[Attribute]) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        if let Meta::NameValue(MetaNameValue {
            value: Expr::Lit(ExprLit {
                lit: Lit::Str(s), ..
            }),
            ..
        }) = &attr.meta
        {
            let v = s.value();
            lines.push(v.strip_prefix(' ').unwrap_or(&v).to_owned());
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

/// Pretty-prints a function signature by wrapping it in a dummy `ItemFn`,
/// running `prettyplease`, and stripping the empty body. Produces the same
/// formatting `rustfmt` would.
fn pretty_fn_signature(sig: &Signature, vis: &SynVis) -> String {
    let empty_body: Block = parse_quote!({});
    let item = ItemFn {
        attrs: vec![],
        vis: vis.clone(),
        sig: sig.clone(),
        block: Box::new(empty_body),
    };
    let file = SynFile {
        shebang: None,
        attrs: vec![],
        items: vec![Item::Fn(item)],
    };
    let pretty = prettyplease::unparse(&file);
    pretty
        .trim_end()
        .trim_end_matches('}')
        .trim_end()
        .trim_end_matches('{')
        .trim_end()
        .to_owned()
}

/// Pretty-prints a type by wrapping it in a dummy `type __ = ...;` alias,
/// running `prettyplease`, and extracting the right-hand side.
fn pretty_type(ty: &Type) -> String {
    let item: ItemType = parse_quote!(type __StdocDummy = #ty;);
    let file = SynFile {
        shebang: None,
        attrs: vec![],
        items: vec![Item::Type(item)],
    };
    let pretty = prettyplease::unparse(&file);
    match (pretty.find('='), pretty.rfind(';')) {
        (Some(eq), Some(semi)) if eq < semi => pretty[eq + 1..semi].trim().to_owned(),
        _ => ty.to_token_stream().to_string(),
    }
}

// -------- Cross-references extraction --------

/// List primitive/pseudo type names we do not record as `outgoing reference`.
/// Otherwise `find_usages("i32")` would return almost the whole project,
/// which has no practical value.
fn is_primitive_type_name(name: &str) -> bool {
    matches!(
        name,
        "bool"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "f32"
            | "f64"
            | "char"
            | "str"
            | "String"
            | "Self"
            | "self"
            | "_"
            | "Vec"
            | "Option"
            | "Result"
            | "Box"
            | "Arc"
            | "Rc"
    )
}

/// Walk a `syn::Type` and collect encountered non-primitive type identifiers.
/// For `Vec<MyStruct>`, keep `MyStruct` (`Vec` is filtered).
/// For `&[MyTrait]`, keep `MyTrait`. For function types, trait objects,
/// macros, etc., on ne descend pas — non critique en Phase 1.
fn collect_type_idents(ty: &Type, out: &mut Vec<String>) {
    match ty {
        Type::Path(tp) => {
            for seg in &tp.path.segments {
                let name = seg.ident.to_string();
                if !is_primitive_type_name(&name) {
                    out.push(name);
                }
                if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                    for arg in &args.args {
                        if let syn::GenericArgument::Type(inner) = arg {
                            collect_type_idents(inner, out);
                        }
                    }
                }
            }
        }
        Type::Reference(r) => collect_type_idents(&r.elem, out),
        Type::Array(a) => collect_type_idents(&a.elem, out),
        Type::Slice(s) => collect_type_idents(&s.elem, out),
        Type::Tuple(t) => {
            for inner in &t.elems {
                collect_type_idents(inner, out);
            }
        }
        Type::Group(g) => collect_type_idents(&g.elem, out),
        Type::Paren(p) => collect_type_idents(&p.elem, out),
        Type::Ptr(p) => collect_type_idents(&p.elem, out),
        _ => {}
    }
}

/// Build function `outgoing references` from its signature.
fn collect_fn_references(sig: &Signature) -> References {
    let mut refs = References::default();

    for arg in &sig.inputs {
        let mut names = Vec::new();
        match arg {
            FnArg::Receiver(r) => collect_type_idents(&r.ty, &mut names),
            FnArg::Typed(pt) => collect_type_idents(&pt.ty, &mut names),
        }
        for name in names {
            refs.push(RefKind::ParamType, name, 0);
        }
    }

    if let ReturnType::Type(_, ty) = &sig.output {
        let mut names = Vec::new();
        collect_type_idents(ty, &mut names);
        for name in names {
            refs.push(RefKind::ReturnType, name, 0);
        }
    }

    refs
}

/// Build `outgoing references` from an iterator of fields.
fn collect_field_references<'a>(fields: impl IntoIterator<Item = &'a syn::Field>) -> References {
    let mut refs = References::default();
    for field in fields {
        let mut names = Vec::new();
        collect_type_idents(&field.ty, &mut names);
        for name in names {
            refs.push(RefKind::FieldType, name, 0);
        }
    }
    refs
}

fn collect_enum_references(e: &ItemEnum) -> References {
    let mut refs = References::default();
    for variant in &e.variants {
        for field in &variant.fields {
            let mut names = Vec::new();
            collect_type_idents(&field.ty, &mut names);
            for name in names {
                refs.push(RefKind::FieldType, name, 0);
            }
        }
    }
    refs
}

/// Extract short trait name from `impl Trait for X` (ex: `LanguageProvider`).
fn impl_trait_name(i: &ItemImpl) -> Option<String> {
    let (_, trait_path, _) = i.trait_.as_ref()?;
    trait_path.segments.last().map(|s| s.ident.to_string())
}

fn span_to_range(span: Span) -> SourceRange {
    let start = span.start();
    let end = span.end();
    SourceRange {
        line_start: u32::try_from(start.line).unwrap_or(0),
        line_end: u32::try_from(end.line).unwrap_or(0),
        column_start: u32::try_from(start.column).unwrap_or(0).saturating_add(1),
        column_end: u32::try_from(end.column).unwrap_or(0).saturating_add(1),
    }
}

fn impl_prefix(i: &ItemImpl) -> String {
    // For `impl Foo`, return `"Foo"` (with generics if any).
    // For `impl Trait for Foo`, return `"<Foo as Trait>"` so trait method
    // FQNs don't collide with inherent method FQNs.
    //
    // **Important**: include type arguments to disambiguate two
    // `impl Trait for SameType<DifferentArgs>` — without this, keys collide
    // silently (bug found during dogfooding via STD001).
    let ty = self_ty_to_string(&i.self_ty);
    if let Some((_, trait_path, _)) = &i.trait_ {
        let trait_str = trait_path.to_token_stream().to_string();
        format!("<{ty} as {trait_str}>")
    } else {
        ty
    }
}

/// Serialize target type of an `impl` as string **including type args**.
/// Compact no-space format (`BTreeMap<DocKey,DocBlock>`) to remain usable
/// inside a `DocKey`.
fn self_ty_to_string(ty: &Type) -> String {
    match ty {
        Type::Path(tp) => {
            let mut out = String::new();
            for (i, seg) in tp.path.segments.iter().enumerate() {
                if i > 0 {
                    out.push_str("::");
                }
                out.push_str(&seg.ident.to_string());
                if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                    let inner: Vec<String> = args
                        .args
                        .iter()
                        .map(|a| a.to_token_stream().to_string().replace([' ', '\n'], ""))
                        .collect();
                    if !inner.is_empty() {
                        out.push('<');
                        out.push_str(&inner.join(","));
                        out.push('>');
                    }
                }
            }
            // Keep only final segment + args (short keys).
            // Ex: `std::collections::BTreeMap<String,DocBlock>` → `BTreeMap<String,DocBlock>`.
            if let Some(last_double_colon) = out.rfind("::") {
                let after = &out[last_double_colon + 2..];
                // Find `<` immediately after name (not inside args).
                return after.to_owned();
            }
            out
        }
        // Generic fallback for exotic types (TraitObject, ImplTrait,
        // etc.) — peu probables comme `self_ty` d'un impl mais on couvre.
        _ => ty.to_token_stream().to_string().replace([' ', '\n'], ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_top_level_function() {
        let src = r"
            /// Adds two numbers.
            pub fn add(a: i32, b: i32) -> i32 { a + b }
        ";
        let p = RustProvider;
        let symbols = p.discover_symbols(src, Path::new("lib.rs")).unwrap();
        assert_eq!(symbols.len(), 1);
        let s = &symbols[0];
        assert_eq!(s.fqn, vec!["add".to_owned()]);
        assert_eq!(s.symbol.kind, SymbolKind::Function);
        assert_eq!(s.symbol.visibility, Visibility::Public);
        assert_eq!(s.symbol.params.len(), 2);
        assert_eq!(s.leading_comment.as_deref(), Some("Adds two numbers."));
    }

    #[test]
    fn leading_comment_line_start_set_when_attributes_separate_doc_from_symbol() {
        let src = "/// @doc lang.providers.rust Rust\n\
                   #[derive(Debug, Default)]\n\
                   pub struct Provider;\n";
        let p = RustProvider;
        let symbols = p.discover_symbols(src, Path::new("lib.rs")).unwrap();
        assert_eq!(symbols.len(), 1);
        let s = &symbols[0];
        assert_eq!(s.source_range.line_start, 3);
        assert_eq!(s.leading_comment_line_start, Some(1));
    }

    #[test]
    fn leading_comment_line_start_none_when_doc_is_adjacent() {
        let src = "/// hello\npub fn bar() {}\n";
        let p = RustProvider;
        let symbols = p.discover_symbols(src, Path::new("lib.rs")).unwrap();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].leading_comment_line_start, None);
    }

    #[test]
    fn walks_into_modules() {
        let src = r"
            mod inner {
                pub fn nested() {}
            }
        ";
        let p = RustProvider;
        let symbols = p.discover_symbols(src, Path::new("lib.rs")).unwrap();
        let nested = symbols
            .iter()
            .find(|s| s.symbol.kind == SymbolKind::Function)
            .expect("nested fn discovered");
        assert_eq!(nested.fqn, vec!["inner".to_owned(), "nested".to_owned()]);
    }

    #[test]
    fn struct_fields_are_not_symbols() {
        let src = r"
            pub struct Foo { pub x: i32 }
        ";
        let p = RustProvider;
        let symbols = p.discover_symbols(src, Path::new("lib.rs")).unwrap();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].symbol.kind, SymbolKind::Struct);
    }

    #[test]
    fn trait_impl_methods_get_prefixed_fqn() {
        let src = r"
            struct Foo;
            impl MyTrait for Foo {
                fn do_it(&self) {}
            }
        ";
        let p = RustProvider;
        let symbols = p.discover_symbols(src, Path::new("lib.rs")).unwrap();
        let method = symbols
            .iter()
            .find(|s| s.symbol.kind == SymbolKind::Method)
            .expect("method discovered");
        assert_eq!(method.fqn.first().unwrap(), "<Foo as MyTrait>");
        assert_eq!(method.fqn.last().unwrap(), "do_it");
    }
}

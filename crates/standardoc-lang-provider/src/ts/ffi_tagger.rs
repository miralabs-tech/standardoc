//! Stage 2 — FFI binding extractor for the TS/JS provider.
//!
//! Detects two import-side shapes at parse time:
//!
//!   1. **`bun:ffi dlopen`** —
//!      ```ts
//!      import { dlopen, FFIType } from "bun:ffi";
//!      const lib = dlopen(`./libfoo.${suffix}`, {
//!          foo: { args: [FFIType.i32], returns: FFIType.i32 },
//!          bar: { args: [], returns: FFIType.void },
//!      });
//!      ```
//!      Each key (`foo`, `bar`) becomes one `Import` binding with
//!      `FfiAbi::C` + `convention = "bun-dlopen"`.
//!
//!   2. **`Deno.dlopen`** — same shape, member-expression callee. The
//!      second argument's key set is the binding set. Convention is
//!      `"deno-dlopen"`.
//!
//! Each detected key yields **two** outputs to keep the storage layer
//! happy:
//!
//!   * A `RawSymbol` (kind `Value`, `language_kind = "ffi_import"`)
//!     anchored at the source location of the property key, so the
//!     `apply_ffi_bindings` round-trip finds a matching `symbols.id`
//!     to attach the binding to.
//!   * A `RawFfiBinding` whose `symbol_fqdn` matches that symbol's
//!     FQDN, so the cross-language `ffi_resolve` pass can pair it
//!     with the corresponding C-side export.
//!
//! Aliased imports (`import { dlopen as bunOpen } from "bun:ffi"`) are
//! handled by tracking the local-name → original-name binding from the
//! `ImportDeclaration` pass. Both fully-qualified `Deno.dlopen` and the
//! aliased global `globalThis.Deno.dlopen` shapes are accepted.
//!
//! NAPI-style imports (`require("bindings")("addon")`, `import addon
//! from "./addon.node"`) are intentionally out of scope here — they
//! don't surface the foreign symbol names statically. A future revision
//! can emit a single `convention = "napi"` placeholder binding per
//! addon import without per-function granularity.
//!
//! Calls whose second argument is not an inline object literal
//! (`dlopen(path, symbols_var)`) are skipped — we can't enumerate the
//! key set statically. The caller falls back to whatever the consumer
//! infers from the dynamic value's declaration site.

use standardoc_ir::{
    FfiAbi, FfiDirection, Kind, LanguageKind, RawFfiBinding, RawSymbol, Signature, SymbolLocation,
    Visibility,
};
use swc_core::common::{SourceMap, Spanned};
use swc_core::ecma::ast::{
    CallExpr, Callee, Expr, ImportSpecifier, Lit, MemberExpr, MemberProp, Module, ModuleDecl,
    ModuleExportName, ModuleItem, ObjectLit, Pat, Prop, PropOrSpread, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::helpers::prop_name_static;

/// Source module identifier the bun:ffi `dlopen` re-export lives in.
const BUN_FFI_MODULE: &str = "bun:ffi";

/// Foreign export name we're looking for inside a `bun:ffi` import.
const BUN_FFI_DLOPEN: &str = "dlopen";

/// Convention slugs stamped on the produced `RawFfiBinding` records.
/// Day-one resolve ignores `convention` (matches on `abi + abi_name`),
/// but consumers can filter by this hint to show "this binding came
/// from a `bun:ffi dlopen`" in the viz / debug output.
const CONVENTION_BUN_DLOPEN: &str = "bun-dlopen";
const CONVENTION_DENO_DLOPEN: &str = "deno-dlopen";
const CONVENTION_NAPI: &str = "napi";

/// `language_kind` value stamped on every virtual TS symbol the
/// tagger emits to anchor a binding. Distinct from `fn` (an authored
/// function) so consumers and the viz can render these as native-
/// boundary nodes rather than first-class TS functions.
const FFI_IMPORT_LANGUAGE_KIND: &str = "ffi_import";

/// `abi` slug for NAPI bindings. NAPI is the Node-API surface used
/// by every modern native addon (`.node` artefact + `bindings`
/// package). It doesn't fit the built-in `FfiAbi::Lua` / `Jni` /
/// `PythonCApi` slots, so we route through `FfiAbi::Other("napi")`
/// for now — bumping it into the enum proper is a one-line follow-up
/// once consumers settle on a uniform tag.
const NAPI_ABI_SLUG: &str = "napi";

/// The `require("bindings")` factory package used by Node addons to
/// locate the `.node` artefact across build configurations.
const BINDINGS_PACKAGE: &str = "bindings";

/// File extension suffix marking a compiled native addon.
const NODE_ADDON_EXT: &str = ".node";

/// Public entry point. Walks `module` once for imports of `bun:ffi`,
/// then a second time via the `FfiVisitor` for `dlopen` / `Deno.dlopen`
/// call sites. Returns the matching `(symbols, bindings)` pair — the
/// caller appends `symbols` to its own list and stuffs `bindings` into
/// `ExtractedFile.ffi_bindings`.
pub(crate) fn extract_ffi_bindings(
    module: &Module,
    module_fqdn: &str,
    file_path: &str,
    cm: &SourceMap,
) -> (Vec<RawSymbol>, Vec<RawFfiBinding>) {
    let dlopen_locals = collect_bun_dlopen_locals(module);
    let mut visitor = FfiVisitor {
        cm,
        file_path,
        module_fqdn,
        dlopen_locals,
        symbols: Vec::new(),
        bindings: Vec::new(),
    };
    module.visit_with(&mut visitor);
    (visitor.symbols, visitor.bindings)
}

/// Collect the local names a TS file uses to refer to `bun:ffi`'s
/// `dlopen` export. Multiple imports of the same module are merged.
/// Returns an empty vec when no `bun:ffi` import is present — the
/// visitor then ignores every bare-identifier `dlopen(...)` call.
fn collect_bun_dlopen_locals(module: &Module) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(decl)) = item else {
            continue;
        };
        if decl.src.value.to_string_lossy() != BUN_FFI_MODULE {
            continue;
        }
        for spec in &decl.specifiers {
            let ImportSpecifier::Named(named) = spec else {
                continue;
            };
            let imported_name: std::borrow::Cow<'_, str> = match &named.imported {
                Some(ModuleExportName::Ident(id)) => std::borrow::Cow::Borrowed(id.sym.as_ref()),
                Some(ModuleExportName::Str(s)) => s.value.to_string_lossy(),
                None => std::borrow::Cow::Borrowed(named.local.sym.as_ref()),
            };
            if imported_name == BUN_FFI_DLOPEN {
                out.push(named.local.sym.as_ref().to_owned());
            }
        }
    }
    out
}

struct FfiVisitor<'a> {
    cm: &'a SourceMap,
    file_path: &'a str,
    module_fqdn: &'a str,
    dlopen_locals: Vec<String>,
    symbols: Vec<RawSymbol>,
    bindings: Vec<RawFfiBinding>,
}

impl FfiVisitor<'_> {
    fn callee_kind(&self, call: &CallExpr) -> Option<DlopenFlavor> {
        let Callee::Expr(expr) = &call.callee else {
            return None;
        };
        match expr.as_ref() {
            Expr::Ident(id) => {
                if self.dlopen_locals.iter().any(|l| l == id.sym.as_ref()) {
                    Some(DlopenFlavor::Bun)
                } else {
                    None
                }
            }
            Expr::Member(member) => is_deno_dlopen(member).then_some(DlopenFlavor::Deno),
            _ => None,
        }
    }

    fn emit_for_call(&mut self, call: &CallExpr, flavor: DlopenFlavor) {
        // bun:ffi  signature: dlopen(path, symbols)
        // Deno     signature: Deno.dlopen(path, symbols)
        // Both put the symbols-object as the second argument; the first
        // is the library path (string / template-string), which we don't
        // need to read for binding emission.
        let Some(symbols_arg) = call.args.get(1) else {
            return;
        };
        let Expr::Object(obj) = symbols_arg.expr.as_ref() else {
            return;
        };
        let convention = match flavor {
            DlopenFlavor::Bun => CONVENTION_BUN_DLOPEN,
            DlopenFlavor::Deno => CONVENTION_DENO_DLOPEN,
        };
        self.emit_for_object(obj, convention);
    }

    fn emit_for_object(&mut self, obj: &ObjectLit, convention: &'static str) {
        for prop_spread in &obj.props {
            let PropOrSpread::Prop(prop) = prop_spread else {
                continue;
            };
            // We accept any property kind that has a static name we can
            // use as the binding's `abi_name` — `KeyValue`, `Shorthand`,
            // `Method` all qualify. Spread (`...defaults`) is skipped
            // above; computed keys (`[name]: ...`) skip below.
            let (name, name_span) = match prop.as_ref() {
                Prop::KeyValue(kv) => match prop_name_static(&kv.key) {
                    Some(n) => (n, kv.key.span()),
                    None => continue,
                },
                Prop::Shorthand(id) => (id.sym.as_ref().to_owned(), id.span),
                Prop::Method(m) => match prop_name_static(&m.key) {
                    Some(n) => (n, m.key.span()),
                    None => continue,
                },
                Prop::Getter(g) => match prop_name_static(&g.key) {
                    Some(n) => (n, g.key.span()),
                    None => continue,
                },
                Prop::Setter(s) => match prop_name_static(&s.key) {
                    Some(n) => (n, s.key.span()),
                    None => continue,
                },
                Prop::Assign(_) => continue,
            };
            let location = self.span_location(name_span);
            let fqdn = format!("{}::{}", self.module_fqdn, name);
            self.symbols.push(RawSymbol {
                decl_kind: None,
                implements_trait: None,
                receiver_type: None,
                entry_point: None,
                name: name.clone(),
                fqdn: fqdn.clone(),
                kind: Kind::Value,
                language_kind: LanguageKind::from(FFI_IMPORT_LANGUAGE_KIND),
                module: Some(self.module_fqdn.to_owned()),
                visibility: Visibility::Private,
                location,
                signature: None as Option<Signature>,
                body_hash: None,
                attributes: vec![],
                flags: vec!["ffi-import".to_owned()],
            });
            self.bindings.push(RawFfiBinding {
                symbol_fqdn: fqdn,
                abi: FfiAbi::C,
                direction: FfiDirection::Import,
                abi_name: name,
                convention: Some(convention.to_owned()),
            });
        }
    }

    fn span_location(&self, span: swc_core::common::Span) -> SymbolLocation {
        let start = self.cm.lookup_char_pos(span.lo);
        let end = self.cm.lookup_char_pos(span.hi);
        SymbolLocation {
            file: self.file_path.to_owned(),
            start_line: clamp_u32(start.line),
            start_col: clamp_u32(start.col_display),
            end_line: clamp_u32(end.line),
            end_col: clamp_u32(end.col_display),
        }
    }
}

impl Visit for FfiVisitor<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Some(flavor) = self.callee_kind(call) {
            self.emit_for_call(call, flavor);
        }
        // Continue descending — nested dlopen calls (e.g. inside an
        // IIFE or factory) still surface.
        call.visit_children_with(self);
    }

    fn visit_import_decl(&mut self, decl: &swc_core::ecma::ast::ImportDecl) {
        // `import addon from "./addon.node"` — the foreign artefact
        // is a compiled Node addon. Emit one virtual symbol +
        // placeholder binding per specifier so the viz can render
        // the JS ↔ native boundary even without per-function
        // resolution.
        let src_value = decl.src.value.to_string_lossy().into_owned();
        if !src_value.ends_with(NODE_ADDON_EXT) {
            return;
        }
        let abi_name = node_path_basename(&src_value);
        for spec in &decl.specifiers {
            let (local_name, span) = match spec {
                ImportSpecifier::Default(d) => (d.local.sym.as_ref().to_owned(), d.local.span),
                ImportSpecifier::Namespace(n) => (n.local.sym.as_ref().to_owned(), n.local.span),
                ImportSpecifier::Named(n) => (n.local.sym.as_ref().to_owned(), n.local.span),
            };
            self.emit_napi_binding(&local_name, &abi_name, span);
        }
    }

    fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
        if let Some(name) = let_name_from_pat(&decl.name)
            && let Some(init) = decl.init.as_deref()
            && let Some(abi_name) = napi_abi_name_from_init(init)
        {
            self.emit_napi_binding(&name, &abi_name, decl.span);
        }
        decl.visit_children_with(self);
    }
}

impl FfiVisitor<'_> {
    fn emit_napi_binding(
        &mut self,
        local_name: &str,
        abi_name: &str,
        span: swc_core::common::Span,
    ) {
        let fqdn = format!("{}::{}", self.module_fqdn, local_name);
        let location = self.span_location(span);
        self.symbols.push(RawSymbol {
            decl_kind: None,
            implements_trait: None,
            receiver_type: None,
            entry_point: None,
            name: local_name.to_owned(),
            fqdn: fqdn.clone(),
            kind: Kind::Value,
            language_kind: LanguageKind::from(FFI_IMPORT_LANGUAGE_KIND),
            module: Some(self.module_fqdn.to_owned()),
            visibility: Visibility::Private,
            location,
            signature: None as Option<Signature>,
            body_hash: None,
            attributes: vec![],
            flags: vec!["ffi-import".to_owned(), "napi".to_owned()],
        });
        self.bindings.push(RawFfiBinding {
            symbol_fqdn: fqdn,
            abi: FfiAbi::Other(NAPI_ABI_SLUG.to_owned()),
            direction: FfiDirection::Import,
            abi_name: abi_name.to_owned(),
            convention: Some(CONVENTION_NAPI.to_owned()),
        });
    }
}

/// Pull the basename out of a `./path/to/addon.node` literal so the
/// binding's `abi_name` carries the addon identity even when the path
/// uses prefixes / build-output suffixes.
fn node_path_basename(path: &str) -> String {
    let stripped = path.strip_suffix(NODE_ADDON_EXT).unwrap_or(path);
    stripped
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(stripped)
        .to_owned()
}

/// Pattern-match an expression that yields a Node addon:
///
///   * `require("./addon.node")` → basename of the path
///   * `require("bindings")("name")` → the `"name"` argument
///
/// Returns `None` for anything else.
fn napi_abi_name_from_init(expr: &Expr) -> Option<String> {
    let Expr::Call(call) = expr else { return None };
    let Callee::Expr(callee_expr) = &call.callee else {
        return None;
    };
    match callee_expr.as_ref() {
        // `require("...node")` shape.
        Expr::Ident(id) if id.sym.as_ref() == "require" => {
            let first = call.args.first()?;
            let Expr::Lit(Lit::Str(s)) = first.expr.as_ref() else {
                return None;
            };
            let raw = s.value.to_string_lossy();
            if !raw.ends_with(NODE_ADDON_EXT) {
                return None;
            }
            Some(node_path_basename(&raw))
        }
        // `require("bindings")("name")` shape.
        Expr::Call(inner) => {
            let Callee::Expr(inner_callee) = &inner.callee else {
                return None;
            };
            let Expr::Ident(id) = inner_callee.as_ref() else {
                return None;
            };
            if id.sym.as_ref() != "require" {
                return None;
            }
            let inner_first = inner.args.first()?;
            let Expr::Lit(Lit::Str(inner_s)) = inner_first.expr.as_ref() else {
                return None;
            };
            if inner_s.value.to_string_lossy() != BINDINGS_PACKAGE {
                return None;
            }
            let outer_first = call.args.first()?;
            let Expr::Lit(Lit::Str(s)) = outer_first.expr.as_ref() else {
                return None;
            };
            Some(s.value.to_string_lossy().into_owned())
        }
        _ => None,
    }
}

/// Extract the binding name from a simple `let x = ...` declarator.
/// Returns `None` for destructuring patterns (`const { a, b } =
/// require(...)`) — the consumer can fall back to per-property
/// imports via `bun:ffi`'s `{ a, b }` shape if they really need
/// that granularity.
fn let_name_from_pat(pat: &Pat) -> Option<String> {
    if let Pat::Ident(bi) = pat {
        Some(bi.id.sym.as_ref().to_owned())
    } else {
        None
    }
}

/// Internal flag distinguishing the two detected dialects. Used only
/// to stamp `convention` on the produced bindings.
#[derive(Clone, Copy)]
enum DlopenFlavor {
    Bun,
    Deno,
}

/// Recognise `Deno.dlopen` and `globalThis.Deno.dlopen`. The latter is
/// the form ESLint / no-implicit-globals projects tend to use.
fn is_deno_dlopen(member: &MemberExpr) -> bool {
    if !member_prop_is(&member.prop, "dlopen") {
        return false;
    }
    match member.obj.as_ref() {
        Expr::Ident(id) => id.sym.as_ref() == "Deno",
        Expr::Member(parent) => {
            // globalThis.Deno
            if !member_prop_is(&parent.prop, "Deno") {
                return false;
            }
            matches!(parent.obj.as_ref(), Expr::Ident(id) if id.sym.as_ref() == "globalThis")
        }
        _ => false,
    }
}

fn member_prop_is(prop: &MemberProp, name: &str) -> bool {
    match prop {
        MemberProp::Ident(id) => id.sym.as_ref() == name,
        MemberProp::Computed(c) => match c.expr.as_ref() {
            Expr::Lit(Lit::Str(s)) => s.value.to_string_lossy() == name,
            _ => false,
        },
        MemberProp::PrivateName(_) => false,
    }
}

fn clamp_u32(v: usize) -> u32 {
    u32::try_from(v).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests;

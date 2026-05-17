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
    ModuleExportName, ModuleItem, ObjectLit, Prop, PropName, PropOrSpread,
};
use swc_core::ecma::visit::{Visit, VisitWith};

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

/// `language_kind` value stamped on every virtual TS symbol the
/// tagger emits to anchor a binding. Distinct from `fn` (an authored
/// function) so consumers and the viz can render these as native-
/// boundary nodes rather than first-class TS functions.
const FFI_IMPORT_LANGUAGE_KIND: &str = "ffi_import";

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

/// Static property-name extraction. Returns `Some(name)` for
/// identifiers, string keys, and numeric keys (stringified). Returns
/// `None` for computed keys (`[expr]: ...`) and bigint keys.
fn prop_name_static(key: &PropName) -> Option<String> {
    match key {
        PropName::Ident(id) => Some(id.sym.as_ref().to_owned()),
        PropName::Str(s) => Some(s.value.to_string_lossy().into_owned()),
        PropName::Num(n) => Some(n.value.to_string()),
        PropName::Computed(_) | PropName::BigInt(_) => None,
    }
}

fn clamp_u32(v: usize) -> u32 {
    u32::try_from(v).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use swc_core::common::{FileName, SourceMap, sync::Lrc};
    use swc_core::ecma::ast::EsVersion;
    use swc_core::ecma::parser::{Parser, StringInput, Syntax, TsSyntax, lexer::Lexer};

    fn parse(src: &str) -> (Module, Lrc<SourceMap>) {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(Lrc::new(FileName::Custom("test.ts".into())), src.to_string());
        let lexer = Lexer::new(
            Syntax::Typescript(TsSyntax::default()),
            EsVersion::EsNext,
            StringInput::from(&*fm),
            None,
        );
        let mut parser = Parser::new_from(lexer);
        let module = parser.parse_module().expect("parse module");
        (module, cm)
    }

    fn run(src: &str) -> (Vec<RawSymbol>, Vec<RawFfiBinding>) {
        let (module, cm) = parse(src);
        extract_ffi_bindings(&module, "pkg::test", "test.ts", &cm)
    }

    #[test]
    fn bun_ffi_dlopen_emits_symbol_and_binding_per_key() {
        let src = "import { dlopen, FFIType } from \"bun:ffi\";\n\
                   const lib = dlopen(\"./libfoo.so\", {\n\
                   \tfoo: { args: [FFIType.i32], returns: FFIType.i32 },\n\
                   \tbar: { args: [], returns: FFIType.void },\n\
                   });\n";
        let (symbols, bindings) = run(src);

        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"foo"));
        assert!(names.contains(&"bar"));
        for s in &symbols {
            assert_eq!(s.kind, Kind::Value);
            assert_eq!(s.language_kind.as_str(), "ffi_import");
            assert!(s.flags.iter().any(|f| f == "ffi-import"));
        }
        let binding_names: Vec<&str> = bindings.iter().map(|b| b.abi_name.as_str()).collect();
        assert!(binding_names.contains(&"foo"));
        assert!(binding_names.contains(&"bar"));
        for b in &bindings {
            assert_eq!(b.abi, FfiAbi::C);
            assert_eq!(b.direction, FfiDirection::Import);
            assert_eq!(b.convention.as_deref(), Some("bun-dlopen"));
        }
    }

    #[test]
    fn deno_dlopen_emits_with_deno_convention() {
        let src = "const lib = Deno.dlopen(\"./libfoo.so\", {\n\
                   \tfoo: { parameters: [\"i32\"], result: \"i32\" },\n\
                   });\n";
        let (symbols, bindings) = run(src);
        assert_eq!(symbols.len(), 1);
        assert_eq!(bindings.len(), 1);
        assert_eq!(symbols[0].fqdn, "pkg::test::foo");
        assert_eq!(bindings[0].convention.as_deref(), Some("deno-dlopen"));
    }

    #[test]
    fn aliased_bun_dlopen_import_is_tracked() {
        let src = "import { dlopen as bunOpen } from \"bun:ffi\";\n\
                   const lib = bunOpen(\"./libfoo.so\", { foo: {} });\n";
        let (symbols, bindings) = run(src);
        assert_eq!(symbols.len(), 1);
        assert_eq!(bindings.len(), 1);
        assert_eq!(symbols[0].name, "foo");
    }

    #[test]
    fn bare_dlopen_without_bun_ffi_import_is_ignored() {
        // No `bun:ffi` import means `dlopen` could be anything (user-
        // defined fn, FFI from another lib). The tagger plays safe and
        // emits nothing.
        let src = "const lib = dlopen(\"./libfoo.so\", { foo: {} });\n";
        let (symbols, bindings) = run(src);
        assert!(symbols.is_empty());
        assert!(bindings.is_empty());
    }

    #[test]
    fn deno_dlopen_via_globalthis_is_recognised() {
        let src = "const lib = globalThis.Deno.dlopen(\"./libfoo.so\", { foo: {} });\n";
        let (symbols, bindings) = run(src);
        assert_eq!(symbols.len(), 1);
        assert_eq!(bindings[0].convention.as_deref(), Some("deno-dlopen"));
    }

    #[test]
    fn non_object_second_arg_is_skipped_silently() {
        // `dlopen(path, symbols_var)` — we can't enumerate the keys
        // statically. Skip cleanly rather than mis-attribute.
        let src = "import { dlopen } from \"bun:ffi\";\n\
                   const syms = { foo: {} };\n\
                   const lib = dlopen(\"./libfoo.so\", syms);\n";
        let (symbols, bindings) = run(src);
        assert!(symbols.is_empty());
        assert!(bindings.is_empty());
    }

    #[test]
    fn computed_property_keys_are_skipped() {
        let src = "import { dlopen } from \"bun:ffi\";\n\
                   const lib = dlopen(\"./libfoo.so\", { [dynamic]: {}, real: {} });\n";
        let (symbols, _bindings) = run(src);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["real"]);
    }

    #[test]
    fn string_literal_keys_are_accepted() {
        let src = "import { dlopen } from \"bun:ffi\";\n\
                   const lib = dlopen(\"./libfoo.so\", { \"foo\": {}, \"bar-baz\": {} });\n";
        let (symbols, _bindings) = run(src);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"foo"));
        assert!(names.contains(&"bar-baz"));
    }

    #[test]
    fn dlopen_inside_function_body_still_emits() {
        // The visitor descends into nested expressions, so a factory
        // pattern `() => dlopen(...)` still produces bindings.
        let src = "import { dlopen } from \"bun:ffi\";\n\
                   function loadLib() {\n\
                   \treturn dlopen(\"./libfoo.so\", { foo: {} });\n\
                   }\n";
        let (symbols, bindings) = run(src);
        assert_eq!(symbols.len(), 1);
        assert_eq!(bindings.len(), 1);
    }

    #[test]
    fn multiple_dlopen_calls_emit_all_keys() {
        // Two distinct dlopen sites both contribute. Today the FQDN
        // scheme collides if both sites import the same name; the
        // visitor still emits both rows, and storage de-dupes on
        // primary key at insert time. Document the behaviour.
        let src = "import { dlopen } from \"bun:ffi\";\n\
                   const a = dlopen(\"./a.so\", { alpha: {} });\n\
                   const b = dlopen(\"./b.so\", { beta: {} });\n";
        let (symbols, bindings) = run(src);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));
        assert_eq!(bindings.len(), 2);
    }

    #[test]
    fn span_locations_point_at_the_property_key() {
        let src = "import { dlopen } from \"bun:ffi\";\n\
                   const lib = dlopen(\"./libfoo.so\", { foo: {} });\n";
        let (symbols, _bindings) = run(src);
        let foo = symbols.iter().find(|s| s.name == "foo").unwrap();
        // The property key `foo` is on line 2.
        assert_eq!(foo.location.start_line, 2);
        assert_eq!(foo.location.file, "test.ts");
    }
}

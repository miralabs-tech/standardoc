//! Stage 2 — FFI binding extractor for the Rust provider.
//!
//! Detects two shapes at top-level of a parsed `syn::File`:
//!
//!   1. `extern "<abi>" { fn name(...); … }` — every `ForeignItem::Fn`
//!      inside the block becomes an `Import` binding. The block's ABI
//!      string maps directly to `FfiAbi` (`"C"` → `FfiAbi::C`,
//!      `"system"` → `FfiAbi::System`, others → `FfiAbi::Other(...)`).
//!      `#[link_name = "alt"]` on a foreign item overrides `abi_name`
//!      so the linker-level identifier is what resolves cross-language.
//!
//!   2. `#[no_mangle] pub extern "<abi>" fn name() { ... }` — the
//!      function is an `Export` binding. The same rules apply for ABI
//!      and `#[link_name]` (and Rust 2024's `#[unsafe(no_mangle)]` /
//!      `#[unsafe(link_name = "...")]` shape).
//!
//! Scope is intentionally narrow: only top-level items. Nested `mod`
//! blocks are not descended, because the FQDN reconstruction inside a
//! nested mod requires knowing the full path the walker is tracking
//! elsewhere — and in practice nearly every FFI binding sits at the
//! crate root or a top-level module. A future revision can lift this
//! by sharing the walker's module-path stack.

use standardoc_ir::{FfiAbi, FfiDirection, RawFfiBinding};
use syn::{Abi, ForeignItem, Item, Lit, Meta};

/// Walk every top-level item in `parsed` and emit the FFI bindings it
/// participates in. `module_fqdn` is the file-level module FQDN — used
/// to construct `symbol_fqdn` for each binding (no nested-mod support).
pub(crate) fn extract_ffi_bindings(
    parsed: &syn::File,
    module_fqdn: &str,
) -> Vec<RawFfiBinding> {
    let mut out = Vec::new();
    for item in &parsed.items {
        match item {
            Item::ForeignMod(fmod) => {
                let abi = abi_from_abi_node(Some(&fmod.abi));
                for foreign_item in &fmod.items {
                    let ForeignItem::Fn(fitem) = foreign_item else {
                        continue;
                    };
                    let name = fitem.sig.ident.to_string();
                    let abi_name =
                        link_name_override(&fitem.attrs).unwrap_or_else(|| name.clone());
                    out.push(RawFfiBinding {
                        symbol_fqdn: format!("{module_fqdn}::{name}"),
                        abi: abi.clone(),
                        direction: FfiDirection::Import,
                        abi_name,
                        convention: None,
                    });
                }
            }
            Item::Fn(item_fn) => {
                let Some(abi) = item_fn.sig.abi.as_ref() else {
                    continue;
                };
                if !has_no_mangle(&item_fn.attrs) {
                    continue;
                }
                let abi = abi_from_abi_node(Some(abi));
                let name = item_fn.sig.ident.to_string();
                let abi_name =
                    link_name_override(&item_fn.attrs).unwrap_or_else(|| name.clone());
                out.push(RawFfiBinding {
                    symbol_fqdn: format!("{module_fqdn}::{name}"),
                    abi,
                    direction: FfiDirection::Export,
                    abi_name,
                    convention: None,
                });
            }
            _ => {}
        }
    }
    out
}

/// Map a `syn::Abi` (or its absence) to an `FfiAbi`. The default when
/// the ABI string is missing or unparseable is `C` — matches rustc's
/// own default for bare `extern { ... }` and `extern fn`.
fn abi_from_abi_node(abi: Option<&Abi>) -> FfiAbi {
    let Some(abi) = abi else {
        return FfiAbi::C;
    };
    let Some(lit) = abi.name.as_ref() else {
        return FfiAbi::C;
    };
    FfiAbi::from_slug(&lit.value().to_ascii_lowercase())
}

/// Returns the `#[link_name = "..."]` value if the attribute list
/// carries one. Recognises both pre-2024 (`#[link_name = "X"]`) and
/// Rust 2024 (`#[unsafe(link_name = "X")]`) spellings.
fn link_name_override(attrs: &[syn::Attribute]) -> Option<String> {
    for attr in attrs {
        if let Some(name) = read_kv_str(&attr.meta, "link_name") {
            return Some(name);
        }
        // Rust 2024 `#[unsafe(link_name = "x")]` — descend into the
        // inner Meta list.
        if attr.path().is_ident("unsafe")
            && let Meta::List(list) = &attr.meta
            && let Ok(inner_metas) = list.parse_args_with(
                syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated,
            )
        {
            for meta in inner_metas {
                if let Some(name) = read_kv_str(&meta, "link_name") {
                    return Some(name);
                }
            }
        }
    }
    None
}

fn read_kv_str(meta: &Meta, key: &str) -> Option<String> {
    let Meta::NameValue(nv) = meta else {
        return None;
    };
    if !nv.path.is_ident(key) {
        return None;
    }
    let syn::Expr::Lit(expr_lit) = &nv.value else {
        return None;
    };
    let Lit::Str(s) = &expr_lit.lit else {
        return None;
    };
    Some(s.value())
}

/// Returns `true` when the attribute list carries either `#[no_mangle]`
/// or the Rust 2024 `#[unsafe(no_mangle)]` form.
fn has_no_mangle(attrs: &[syn::Attribute]) -> bool {
    for attr in attrs {
        if attr.path().is_ident("no_mangle") {
            return true;
        }
        if attr.path().is_ident("unsafe")
            && let Meta::List(list) = &attr.meta
            && let Ok(inner_metas) = list.parse_args_with(
                syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated,
            )
            && inner_metas.iter().any(|m| m.path().is_ident("no_mangle"))
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> syn::File {
        syn::parse_file(src).expect("syn parse ok")
    }

    #[test]
    fn extern_c_block_emits_import_per_foreign_fn() {
        let src = r#"
            extern "C" {
                fn lur_vm_init() -> i32;
                fn lur_vm_run(src: *const i8) -> i32;
            }
        "#;
        let bindings = extract_ffi_bindings(&parse(src), "lurlang::lib");
        assert_eq!(bindings.len(), 2);
        for b in &bindings {
            assert_eq!(b.direction, FfiDirection::Import);
            assert_eq!(b.abi, FfiAbi::C);
        }
        let names: Vec<&str> = bindings.iter().map(|b| b.abi_name.as_str()).collect();
        assert!(names.contains(&"lur_vm_init"));
        assert!(names.contains(&"lur_vm_run"));
    }

    #[test]
    fn no_mangle_pub_extern_c_fn_emits_export() {
        let src = r#"
            #[no_mangle]
            pub extern "C" fn rust_callback(data: *const u8) -> i32 {
                0
            }
        "#;
        let bindings = extract_ffi_bindings(&parse(src), "lurlang::lib");
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].direction, FfiDirection::Export);
        assert_eq!(bindings[0].abi, FfiAbi::C);
        assert_eq!(bindings[0].abi_name, "rust_callback");
        assert_eq!(bindings[0].symbol_fqdn, "lurlang::lib::rust_callback");
    }

    #[test]
    fn extern_c_fn_without_no_mangle_is_not_export() {
        let src = r#"
            pub extern "C" fn maybe_callback() {}
        "#;
        let bindings = extract_ffi_bindings(&parse(src), "lurlang::lib");
        assert!(
            bindings.is_empty(),
            "`extern \"C\"` alone (no #[no_mangle]) keeps Rust's name mangling — \
             linker symbol is unstable, not an FFI export"
        );
    }

    #[test]
    fn link_name_override_takes_precedence_for_imports() {
        let src = r#"
            extern "C" {
                #[link_name = "alt_native_name"]
                fn rust_side_alias();
            }
        "#;
        let bindings = extract_ffi_bindings(&parse(src), "x::y");
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].abi_name, "alt_native_name");
        assert_eq!(bindings[0].symbol_fqdn, "x::y::rust_side_alias");
    }

    #[test]
    fn link_name_override_takes_precedence_for_exports() {
        let src = r#"
            #[no_mangle]
            #[link_name = "exposed_to_c"]
            pub extern "C" fn rust_name() {}
        "#;
        let bindings = extract_ffi_bindings(&parse(src), "x::y");
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].direction, FfiDirection::Export);
        assert_eq!(bindings[0].abi_name, "exposed_to_c");
    }

    #[test]
    fn extern_system_block_lands_as_system_abi() {
        let src = r#"
            extern "system" {
                fn GetCurrentProcessId() -> u32;
            }
        "#;
        let bindings = extract_ffi_bindings(&parse(src), "winapi::process");
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].abi, FfiAbi::System);
    }

    #[test]
    fn unknown_abi_lands_in_other() {
        let src = r#"
            extern "Rust" {
                fn whatever();
            }
        "#;
        let bindings = extract_ffi_bindings(&parse(src), "x");
        assert_eq!(bindings.len(), 1);
        match &bindings[0].abi {
            FfiAbi::Other(s) => assert_eq!(s, "rust"),
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn empty_file_yields_no_bindings() {
        let bindings = extract_ffi_bindings(&parse(""), "x");
        assert!(bindings.is_empty());
    }

    #[test]
    fn rust_fn_without_extern_is_ignored() {
        let src = r#"
            pub fn normal() {}
            #[no_mangle]
            pub fn no_mangle_but_rust_abi() {}
        "#;
        let bindings = extract_ffi_bindings(&parse(src), "x");
        assert!(
            bindings.is_empty(),
            "no_mangle without an `extern` ABI is not a cross-language binding"
        );
    }
}

//! Pass 0 collector for the workspace-global return-type registry.
//!
//! Walks a workspace's Rust files at item-level only (no expression
//! traversal, no edge emission, no symbol push) and records each
//! free fn / impl-method's return type into a shared
//! [`GlobalReturnTypeRegistry`]. Designed to run BEFORE per-file
//! extract so the registry is fully populated when `type_of_expr`
//! looks up cross-file fn calls during Pass 1.
//!
//! Item-level only: this collector matches the contract of
//! `walk::process_item_p1`'s `ReturnType::Type` branch but skips
//! everything else (sigs, struct fields, impl bodies, ...). A typical
//! Rust workspace populates the registry in 50-200 ms for a few
//! thousand files (parse is the dominant cost; the item walk itself is
//! linear in items).
//!
//! Inputs use the same `(crate_name, crate_rel, content)` shape as
//! `extract_file` so the orchestrator can feed the same file iterator
//! to both passes without re-discovery.

use syn::ImplItem;

use super::global_return_type_registry::GlobalReturnTypeRegistry;
use super::module_path;
use super::type_name::parametric_type;

/// Workspace file slot for Pass 0 — one entry per Rust file the
/// collector should scan. Mirrors the params of
/// `extract::extract_file` (minus the `path` which is unused at item
/// scan).
pub(crate) struct WorkspaceFile<'a> {
    pub(crate) crate_name: &'a str,
    pub(crate) crate_rel: &'a str,
    pub(crate) content: &'a str,
}

/// Build a [`GlobalReturnTypeRegistry`] from the workspace's Rust
/// files. Best-effort: files that fail to parse are silently skipped
/// (the per-file Pass 1 will surface the parse error through its own
/// path).
pub(crate) fn collect_global_returns(files: &[WorkspaceFile<'_>]) -> GlobalReturnTypeRegistry {
    let mut registry = GlobalReturnTypeRegistry::default();
    for f in files {
        let Ok(parsed) = syn::parse_file(f.content) else {
            continue;
        };
        let module_fqdn = module_path::compute(f.crate_name, f.crate_rel);
        record_items(&mut registry, &parsed.items, &module_fqdn);
    }
    registry
}

fn record_items(
    registry: &mut GlobalReturnTypeRegistry,
    items: &[syn::Item],
    current_module: &str,
) {
    for item in items {
        record_item(registry, item, current_module);
    }
}

fn record_item(registry: &mut GlobalReturnTypeRegistry, item: &syn::Item, current_module: &str) {
    match item {
        syn::Item::Fn(it) => {
            if let syn::ReturnType::Type(_, ty) = &it.sig.output {
                let fn_fqdn = format!("{current_module}::{}", it.sig.ident);
                registry.record(&fn_fqdn, ty);
            }
        }
        syn::Item::Impl(it) => {
            // Pass 0 only handles the inherent + trait impl methods
            // for nominal self types. The receiver FQDN matches what
            // `extract_fn_in_impl` produces in `walk_p1` — base ident
            // of the path tail. Anonymous / parametric `Self`
            // contexts are recorded under that ident only; downstream
            // lookup may miss them when the caller carries the full
            // FQDN, but the per-file `ReturnTypeTable` covers the
            // same-file path so the gap is bounded to genuinely
            // cross-file impl chains on unparseable Self types.
            let Some(self_ident) = impl_self_ident(&it.self_ty) else {
                return;
            };
            let impl_fqdn = format!("{current_module}::{self_ident}");
            for impl_item in &it.items {
                if let ImplItem::Fn(m) = impl_item
                    && let syn::ReturnType::Type(_, ty) = &m.sig.output
                {
                    let method_fqdn = format!("{impl_fqdn}::{}", m.sig.ident);
                    registry.record(&method_fqdn, ty);
                }
            }
        }
        syn::Item::Mod(it) => {
            // Inline modules contribute fns under their nested path.
            // `mod foo { fn bar() -> T {} }` → `<current>::foo::bar`.
            if let Some((_, sub)) = &it.content {
                let nested = format!("{current_module}::{}", it.ident);
                record_items(registry, sub, &nested);
            }
        }
        _ => {}
    }
}

/// Pull the nominal head ident from an impl block's self type. Returns
/// `None` for non-path self types (impl trait for `&T`, tuples, etc.).
fn impl_self_ident(ty: &syn::Type) -> Option<String> {
    parametric_type(ty)
        .map(|p| {
            // `nominal_of` slices off generic args. For `Foo<T>` → "Foo".
            super::type_name::nominal_of(&p).to_string()
        })
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests;

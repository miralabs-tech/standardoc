//! Shared nominal-type extraction for Rust `syn::Type` nodes.
//!
//! Returns the last path segment (no generics) when the type is a
//! `Path`, stripping through `&`, `&mut`, parens and groups. Returns
//! `None` for tuples, closures, slices, impl-trait, never, and other
//! non-nominal shapes.
//!
//! Reused by:
//!   - `extract_call::local_type_env` (Bug E-3 Phase 1: binding -> type)
//!   - `struct_field_table` (Bug E-3 Phase 1: struct.field -> type)
//!   - Future: Phase 3 chained-method receivers.

use syn::Type;

pub(crate) fn nominal_type(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
        Type::Reference(r) => nominal_type(&r.elem),
        Type::Paren(p) => nominal_type(&p.elem),
        Type::Group(g) => nominal_type(&g.elem),
        _ => None,
    }
}

#[cfg(test)]
mod tests;

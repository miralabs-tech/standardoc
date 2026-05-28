//! Bug E-3 Phase 1: per-workspace `struct_fqdn → {field → type}` map.
//!
//! Populated by `push_field` during Pass 1 (struct extraction) when the
//! field's `syn::Type` yields a nominal name. Read by
//! `visit_expr_method_call` (P1.4) to resolve `self.field.method()`
//! when the enclosing impl's `self_type` is known.
//!
//! Scope:
//!   * Named-field structs only (tuple-field accesses via numeric index
//!     are out of scope Phase 1).
//!   * Bug E-3 ext P-E3.2.1: types stored *parametrically*
//!     (`"Vec<RawSymbol>"`) so closure-arg substitution can resolve
//!     `T = RawSymbol` for chains like
//!     `extracted.symbols.iter().map(|s| s.name.as_str())`. Type
//!     aliases stay at their declared name (no resolution to underlying
//!     type Phase 1).
//!   * `struct_fqdn` is whatever `push_field` is called with as
//!     `parent_fqdn` (the fully-qualified struct path).

use std::collections::HashMap;

use syn::Type;

use super::type_name::parametric_type;

#[derive(Default, Debug)]
pub(crate) struct StructFieldTable {
    by_struct: HashMap<String, HashMap<String, String>>,
}

impl StructFieldTable {
    /// Record `<struct_fqdn>.<field_name> : <ty>` when `<ty>` is nominal.
    /// Silently skips non-nominal types (closures, tuples, slices, ...).
    /// Bug E-3 ext P-E3.2.1: stores the parametric form preserving
    /// generics (`Vec<Foo>`, `Iterator<T>`); consumers slice via
    /// `nominal_of` when they only need the bare nominal head.
    pub(crate) fn record(&mut self, struct_fqdn: &str, field_name: &str, ty: &Type) {
        let Some(t) = parametric_type(ty) else { return };
        self.by_struct
            .entry(struct_fqdn.to_string())
            .or_default()
            .insert(field_name.to_string(), t);
    }

    /// Look up `<struct_fqdn>.<field_name>` and return its parametric
    /// type string (`"Vec<Foo>"`). Returns `None` when the struct is
    /// unknown, the field isn't recorded, or the field had a non-nominal
    /// type at extract time.
    pub(crate) fn lookup(&self, struct_fqdn: &str, field_name: &str) -> Option<&str> {
        self.by_struct
            .get(struct_fqdn)?
            .get(field_name)
            .map(String::as_str)
    }
}

#[cfg(test)]
mod tests;

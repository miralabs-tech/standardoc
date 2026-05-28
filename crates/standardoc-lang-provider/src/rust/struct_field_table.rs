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
//!   * Bug E-3 ext P-E3.2.2: a parallel nominal→FQDN side-index lets
//!     `lookup` resolve bare nominal short names (`"RawSymbol"`) to
//!     the recorded FQDN. Names that collide across two definitions
//!     stay ambiguous (lookup falls through to `None`).

use std::collections::HashMap;

use syn::Type;

use super::type_name::parametric_type;

#[derive(Default, Debug)]
pub(crate) struct StructFieldTable {
    by_struct: HashMap<String, HashMap<String, String>>,
    /// Bug E-3 ext P-E3.2.2: nominal short name → recorded struct FQDN.
    /// `Some(fqdn)` for unique nominals, `None` for collisions across
    /// definitions (lookup then falls through). Populated alongside
    /// `record`.
    by_nominal: HashMap<String, Option<String>>,
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
        // Bug E-3 ext P-E3.2.2: keep nominal→FQDN side-index in sync.
        // First record for a given nominal wins; a subsequent record
        // from a *different* FQDN flags the nominal ambiguous (`None`).
        if let Some(nominal) = struct_fqdn.rsplit("::").next() {
            self.by_nominal
                .entry(nominal.to_string())
                .and_modify(|cur| {
                    if cur.as_deref() != Some(struct_fqdn) {
                        *cur = None;
                    }
                })
                .or_insert_with(|| Some(struct_fqdn.to_string()));
        }
    }

    /// Look up `<struct_key>.<field_name>` and return its parametric type
    /// string. `<struct_key>` may be either the full FQDN (matches
    /// `by_struct` directly) or a nominal short name (resolves via the
    /// `by_nominal` side-index, falling through if the nominal is
    /// ambiguous across definitions). Returns `None` when nothing
    /// matches.
    pub(crate) fn lookup(&self, struct_key: &str, field_name: &str) -> Option<&str> {
        if let Some(fields) = self.by_struct.get(struct_key) {
            return fields.get(field_name).map(String::as_str);
        }
        let nominal_fqdn = self.by_nominal.get(struct_key)?.as_deref()?;
        self.by_struct
            .get(nominal_fqdn)?
            .get(field_name)
            .map(String::as_str)
    }
}

#[cfg(test)]
mod tests;

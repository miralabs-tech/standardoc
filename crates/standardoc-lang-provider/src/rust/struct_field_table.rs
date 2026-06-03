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

use std::collections::{HashMap, HashSet};

use syn::Type;

use super::type_name::parametric_type;

#[derive(Default, Debug)]
#[allow(clippy::struct_field_names)]
pub(crate) struct StructFieldTable {
    by_struct: HashMap<String, HashMap<String, String>>,
    /// Bug E-3 ext P-E3.2.2: nominal short name → recorded struct FQDN.
    /// `Some(fqdn)` for unique nominals, `None` for collisions across
    /// definitions (lookup then falls through). Populated alongside
    /// `record`.
    by_nominal: HashMap<String, Option<String>>,
    /// Bug field-as-CALL V2 (`fn()` extension): presence-only field-name
    /// table populated unconditionally by `record_presence` regardless
    /// of whether the field's type is nominal. The typed `by_struct`
    /// table skips non-nominal types (closures, `fn()`, slices, tuples)
    /// because their parametric form yields `None`, but the field-as-
    /// CALL guard in `visit_expr_method_call` only needs to know
    /// "does this struct have a field named X" — not the field's type.
    /// Mirrors `by_struct`'s nominal-short side-index via
    /// `by_presence_nominal` for the same disambiguated lookup.
    by_struct_field_names: HashMap<String, HashSet<String>>,
    by_presence_nominal: HashMap<String, Option<String>>,
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

    /// Bug field-as-CALL V2 (`fn()` extension): unconditionally record
    /// that `<struct_fqdn>` has a field named `<field_name>`, regardless
    /// of whether its type was nominal. Used by the
    /// `visit_expr_method_call` guard so `s.bare_ptr()` where
    /// `bare_ptr: fn()` (or `Box<dyn Fn>` / closure / tuple) is still
    /// recognised as a field-call. Populated for every field — named or
    /// tuple — alongside `record` in `push_field`.
    pub(crate) fn record_presence(&mut self, struct_fqdn: &str, field_name: &str) {
        self.by_struct_field_names
            .entry(struct_fqdn.to_string())
            .or_default()
            .insert(field_name.to_string());
        if let Some(nominal) = struct_fqdn.rsplit("::").next() {
            self.by_presence_nominal
                .entry(nominal.to_string())
                .and_modify(|cur| {
                    if cur.as_deref() != Some(struct_fqdn) {
                        *cur = None;
                    }
                })
                .or_insert_with(|| Some(struct_fqdn.to_string()));
        }
    }

    /// Bug field-as-CALL V2: presence-only lookup. Returns `true` when
    /// `<struct_key>` has a field named `<field_name>`, whatever its
    /// type. Mirrors `lookup`'s nominal-short resolution via
    /// `by_presence_nominal`.
    pub(crate) fn has_field(&self, struct_key: &str, field_name: &str) -> bool {
        if let Some(names) = self.by_struct_field_names.get(struct_key) {
            return names.contains(field_name);
        }
        let Some(nominal_fqdn) = self
            .by_presence_nominal
            .get(struct_key)
            .and_then(|o| o.as_deref())
        else {
            return false;
        };
        self.by_struct_field_names
            .get(nominal_fqdn)
            .is_some_and(|names| names.contains(field_name))
    }
}

#[cfg(test)]
mod tests;

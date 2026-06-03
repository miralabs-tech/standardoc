//! Bug E-3 extensions Phase E-3.1: per-file `fqdn → return type`
//! table for workspace fns and impl methods.
//!
//! Populated during Pass 1 (item extraction) when an `ItemFn` /
//! `ImplItem::Fn` carries a non-default return type that yields a
//! nominal name. Read by `type_of_expr` to propagate types across
//! workspace-method-call chains (`get_thing().bar()`) without an MCP
//! round-trip — the lookup mirrors the builtin registry's `lookup_method`
//! contract but for symbols defined in the workspace being walked.
//!
//! Scope:
//!   * Bug E-3 ext P-E3.2.1: return types stored *parametrically*
//!     (`"Option<User>"`) so closure-arg substitution and Iterator
//!     chain propagation reach across workspace fn boundaries. Type
//!     aliases stay at their declared name (no resolution to underlying
//!     type Phase E-3.1).
//!   * Per-file. Cross-file chains (`other_file::get_thing().bar()`)
//!     stay unresolved until a global workspace return table is
//!     introduced (deferred to a later phase).
//!   * Free fns AND impl methods share the same FQDN-keyed map — a free
//!     fn `crate::foo` lives next to a method `crate::Bar::baz`.

use std::collections::HashMap;

use syn::Type;

use super::type_name::parametric_type;

#[derive(Default, Debug)]
pub(crate) struct ReturnTypeTable {
    by_fqdn: HashMap<String, String>,
}

impl ReturnTypeTable {
    /// Record `<fqdn> -> <ret_type>` when `<ret_type>` is nominal.
    /// Silently skips non-nominal return types (closures, tuples,
    /// slices, `()`, ...). Stores parametrically — `Option<User>` rather
    /// than `Option` — so downstream chain propagation can substitute
    /// `T` for the inner type.
    pub(crate) fn record(&mut self, fqdn: &str, ret: &Type) {
        let Some(t) = parametric_type(ret) else {
            return;
        };
        self.by_fqdn.insert(fqdn.to_string(), t);
    }

    /// Look up the parametric return type string for `<fqdn>` (e.g.
    /// `"Option<User>"`). Callers slice via `nominal_of` when only the
    /// bare nominal head is needed.
    pub(crate) fn lookup(&self, fqdn: &str) -> Option<&str> {
        self.by_fqdn.get(fqdn).map(String::as_str)
    }
}

#[cfg(test)]
mod tests;

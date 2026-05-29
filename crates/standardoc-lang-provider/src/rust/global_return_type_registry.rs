//! Workspace-global `fqdn → return type` registry for cross-file
//! type-flow propagation.
//!
//! `ReturnTypeTable` is per-file: a fresh `WalkContext` carries one,
//! populated only with fns/methods defined in the file currently being
//! walked. Chains like `let x = other_crate::get_user(id).unwrap();
//! x.name()` therefore lose the `User` type at the file boundary —
//! `type_of_expr` cannot resolve `other_crate::get_user` because its
//! signature lives in a different `WalkContext`.
//!
//! This registry mirrors `ReturnTypeTable`'s shape and `record` /
//! `lookup` contract but is **populated by a workspace-wide pre-pass**
//! (`collect_global_returns`) and **shared by Arc across all per-file
//! extracts**. The lookup chain in `type_of_expr` becomes:
//!
//!   1. per-file `WalkContext.return_types` (existing)
//!   2. workspace `GlobalReturnTypeRegistry` (new)
//!   3. builtin method registry (existing)
//!
//! Scope mirrors `ReturnTypeTable`:
//!   * Parametric type storage (`"Option<User>"`) so closure-arg
//!     substitution and Iterator chain propagation reach across file
//!     boundaries.
//!   * Free fns AND impl methods share the same FQDN-keyed map.
//!   * Type aliases stay at their declared name.

use std::collections::HashMap;

use syn::Type;

use super::type_name::parametric_type;

#[derive(Default, Debug)]
pub(crate) struct GlobalReturnTypeRegistry {
    by_fqdn: HashMap<String, String>,
}

impl GlobalReturnTypeRegistry {
    /// Record `<fqdn> -> <ret_type>` when `<ret_type>` is nominal.
    /// Silently skips non-nominal return types (closures, tuples,
    /// slices, `()`, ...). Stores parametrically — `Option<User>`
    /// rather than `Option` — so downstream chain propagation can
    /// substitute `T` for the inner type.
    pub(crate) fn record(&mut self, fqdn: &str, ret: &Type) {
        let Some(t) = parametric_type(ret) else {
            return;
        };
        self.by_fqdn.insert(fqdn.to_string(), t);
    }

    /// Look up the parametric return type string for `<fqdn>`.
    /// Returns `None` when the FQDN isn't recorded or its return type
    /// was non-nominal at record time.
    pub(crate) fn lookup(&self, fqdn: &str) -> Option<&str> {
        self.by_fqdn.get(fqdn).map(String::as_str)
    }

    /// Number of recorded entries — used by collectors / tests.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.by_fqdn.len()
    }
}

#[cfg(test)]
mod tests;

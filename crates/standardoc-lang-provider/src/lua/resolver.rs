use standardoc_ir::ResolvedOrUnresolved;

/// Resolve a `require("a.b.c")` argument into an `IMPORTS` edge target.
///
/// Per `notes/locks/lua-provider-38.md` §1 Q3 + memory feedback
/// `feedback_scope_graph_not_lsp.md`: Standardoc does NOT reproduce the
/// Lua runtime `package.path` resolver. We emit `Unresolved { name }` and
/// let the storage layer perform closest-fqdn matching at insert time
/// (the same `resolve_target` path as TS cross-file imports, fixed in
/// session 29).
///
/// The dotted name is preserved as-is — the storage layer will try to
/// match it against any defined symbol whose FQDN ends with the same
/// dotted suffix.
pub(crate) fn resolve_require(require_arg: &str) -> ResolvedOrUnresolved {
    ResolvedOrUnresolved::Unresolved {
        name: require_arg.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn require_returns_unresolved_with_raw_name() {
        let target = resolve_require("utils.strings");
        match target {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "utils.strings"),
            other => panic!("expected Unresolved, got {other:?}"),
        }
    }

    #[test]
    fn require_preserves_single_segment() {
        let target = resolve_require("json");
        match target {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "json"),
            other => panic!("expected Unresolved, got {other:?}"),
        }
    }

    #[test]
    fn require_preserves_deep_dotted_name() {
        let target = resolve_require("lib.json.encode");
        match target {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "lib.json.encode"),
            other => panic!("expected Unresolved, got {other:?}"),
        }
    }
}

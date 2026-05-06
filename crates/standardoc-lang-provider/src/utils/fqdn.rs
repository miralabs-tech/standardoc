//! `::`-separated FQDN string utilities. Shared by every provider that
//! emits canonical fqdns (rust / ts / lua / vue+svelte SFC orchestrator).

/// Returns the last `::`-separated segment of `fqdn`. Falls back to the
/// whole string when no `::` separator is present (e.g. a single-segment
/// crate-name fqdn).
pub(crate) fn last_segment(fqdn: &str) -> &str {
    fqdn.rsplit("::").next().unwrap_or(fqdn)
}

/// Returns everything before the last `::` segment, or `None` when the
/// fqdn is a single segment.
pub(crate) fn parent_module(fqdn: &str) -> Option<String> {
    fqdn.rsplit_once("::").map(|(parent, _)| parent.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_segment_simple() {
        assert_eq!(last_segment("crate::foo::bar"), "bar");
    }

    #[test]
    fn last_segment_no_separator_returns_whole() {
        assert_eq!(last_segment("foo"), "foo");
    }

    #[test]
    fn last_segment_empty_string_returns_empty() {
        assert_eq!(last_segment(""), "");
    }

    #[test]
    fn parent_module_strips_last_segment() {
        assert_eq!(
            parent_module("crate::foo::bar"),
            Some("crate::foo".to_string())
        );
    }

    #[test]
    fn parent_module_single_segment_is_none() {
        assert_eq!(parent_module("foo"), None);
    }

    #[test]
    fn parent_module_double_segment_returns_first() {
        assert_eq!(parent_module("crate::foo"), Some("crate".to_string()));
    }
}

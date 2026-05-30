/// Canonical synthetic FQDN of the builtin trait symbol implied by a
/// `#[derive(...)]` entry.
///
/// Marker traits without callable methods (`Copy`, `Eq`) are intentionally
/// excluded — emitting `IMPLEMENTS` edges for them inflates the index
/// without giving the resolver anything to dispatch against. External
/// derive macros (`strum`, `thiserror`, `derive_builder`, …) are
/// out-of-scope in V1 — only stdlib + serde stdlib derives are mapped.
pub(crate) fn derive_trait_fqdn(name: &str) -> Option<&'static str> {
    Some(match name {
        "Clone" => "<builtin>::rust::Clone",
        "Debug" => "<builtin>::rust::Debug",
        "Default" => "<builtin>::rust::Default",
        "PartialEq" => "<builtin>::rust::PartialEq",
        "PartialOrd" => "<builtin>::rust::PartialOrd",
        "Ord" => "<builtin>::rust::Ord",
        "Hash" => "<builtin>::rust::Hash",
        "Serialize" => "<builtin>::rust::Serialize",
        "Deserialize" => "<builtin>::rust::Deserialize",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_clone_to_builtin_rust_clone() {
        assert_eq!(derive_trait_fqdn("Clone"), Some("<builtin>::rust::Clone"));
    }

    #[test]
    fn maps_debug_to_builtin_rust_debug() {
        assert_eq!(derive_trait_fqdn("Debug"), Some("<builtin>::rust::Debug"));
    }

    #[test]
    fn maps_default_to_builtin_rust_default() {
        assert_eq!(
            derive_trait_fqdn("Default"),
            Some("<builtin>::rust::Default")
        );
    }

    #[test]
    fn maps_partial_eq_to_builtin_rust_partial_eq() {
        assert_eq!(
            derive_trait_fqdn("PartialEq"),
            Some("<builtin>::rust::PartialEq")
        );
    }

    #[test]
    fn maps_partial_ord_to_builtin_rust_partial_ord() {
        assert_eq!(
            derive_trait_fqdn("PartialOrd"),
            Some("<builtin>::rust::PartialOrd")
        );
    }

    #[test]
    fn maps_ord_to_builtin_rust_ord() {
        assert_eq!(derive_trait_fqdn("Ord"), Some("<builtin>::rust::Ord"));
    }

    #[test]
    fn maps_hash_to_builtin_rust_hash() {
        assert_eq!(derive_trait_fqdn("Hash"), Some("<builtin>::rust::Hash"));
    }

    #[test]
    fn maps_serialize_to_builtin_rust_serialize() {
        assert_eq!(
            derive_trait_fqdn("Serialize"),
            Some("<builtin>::rust::Serialize")
        );
    }

    #[test]
    fn maps_deserialize_to_builtin_rust_deserialize() {
        assert_eq!(
            derive_trait_fqdn("Deserialize"),
            Some("<builtin>::rust::Deserialize")
        );
    }

    #[test]
    fn skips_copy_marker_trait_without_methods() {
        assert_eq!(derive_trait_fqdn("Copy"), None);
    }

    #[test]
    fn skips_eq_marker_trait_without_methods() {
        assert_eq!(derive_trait_fqdn("Eq"), None);
    }

    #[test]
    fn returns_none_for_external_derives() {
        assert_eq!(derive_trait_fqdn("Error"), None);
        assert_eq!(derive_trait_fqdn("Display"), None);
        assert_eq!(derive_trait_fqdn("Builder"), None);
        assert_eq!(derive_trait_fqdn("Unknown"), None);
    }
}

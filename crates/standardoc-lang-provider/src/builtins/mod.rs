use standardoc_ir::BuiltinRegistry;
use std::sync::OnceLock;

mod c;
mod js;
mod lua;
mod rust;
mod ts;

/// Build the default workspace-agnostic `BuiltinRegistry` — populated with
/// every natively-supported language's prelude. UST-added languages
/// register their own entries via `BuiltinRegistry::register_user` at
/// runtime; cross-substrat bridges via `register_bridge`.
pub fn standard() -> BuiltinRegistry {
    let mut reg = BuiltinRegistry::new();
    js::register_all(&mut reg);
    ts::register_all(&mut reg);
    rust::register_all(&mut reg);
    lua::register_all(&mut reg);
    c::register_all(&mut reg);
    reg
}

static GLOBAL_REGISTRY: OnceLock<BuiltinRegistry> = OnceLock::new();

/// Process-wide static accessor for the standard builtin registry. Built
/// once on first call (lazy), shared by every extraction in the
/// daemon. Stage 3a consumers borrow this reference instead of
/// rebuilding the registry per file.
pub fn global() -> &'static BuiltinRegistry {
    GLOBAL_REGISTRY.get_or_init(standard)
}

#[cfg(test)]
mod tests {
    use super::*;
    use standardoc_ir::Language;

    #[test]
    fn standard_registry_covers_every_native_language() {
        let reg = standard();
        for lang in [
            Language::JavaScript,
            Language::TypeScript,
            Language::Rust,
            Language::Lua,
            Language::C,
        ] {
            assert!(
                reg.by_language.get(&lang).map_or(0, Vec::len) > 0,
                "{lang:?} must have at least one registered builtin"
            );
        }
    }

    #[test]
    fn known_signature_builtins_are_registered() {
        let reg = standard();
        assert!(reg.lookup("JSON", Language::JavaScript).is_some());
        assert!(reg.lookup("Promise", Language::TypeScript).is_some());
        assert!(reg.lookup("Partial", Language::TypeScript).is_some());
        assert!(reg.lookup("Vec", Language::Rust).is_some());
        assert!(reg.lookup("Result", Language::Rust).is_some());
        assert!(reg.lookup("pairs", Language::Lua).is_some());
        assert!(reg.lookup("table.insert", Language::Lua).is_some());
    }

    #[test]
    fn rust_derive_trait_targets_are_seeded() {
        // `rust::derive_impls` emits `IMPLEMENTS → <builtin>::rust::<Trait>`
        // edges for every recognised derive. Each target must resolve to
        // a real seeded symbol row, otherwise the edge stays unresolved.
        let reg = standard();
        for trait_name in [
            "Clone",
            "Debug",
            "Default",
            "PartialEq",
            "PartialOrd",
            "Ord",
            "Hash",
            "Serialize",
            "Deserialize",
        ] {
            let entry = reg.lookup(trait_name, Language::Rust).unwrap_or_else(|| {
                panic!("{trait_name} must be seeded for derive-IMPLEMENTS targets")
            });
            assert_eq!(
                entry.synthetic_fqdn,
                format!("<builtin>::rust::{trait_name}"),
                "synthetic fqdn for {trait_name} must match derive_impls map"
            );
        }
    }

    #[test]
    fn rust_derive_trait_methods_are_seeded() {
        // Phase 5 resolver walks `<trait_fqdn>::<method>` after a derive
        // IMPLEMENTS hit. Each (trait, method) pair must produce a
        // BuiltinMethodEntry with the expected synthetic_fqdn so the
        // resolver lands on a real symbol.
        let reg = standard();
        for (trait_name, method) in [
            ("Clone", "clone"),
            ("Debug", "fmt"),
            ("Default", "default"),
            ("PartialEq", "eq"),
            ("PartialOrd", "partial_cmp"),
            ("Ord", "cmp"),
            ("Hash", "hash"),
            ("Serialize", "serialize"),
            ("Deserialize", "deserialize"),
        ] {
            let entry = reg
                .lookup_method(trait_name, method, Language::Rust)
                .unwrap_or_else(|| {
                    panic!("{trait_name}::{method} must be seeded for trait dispatch")
                });
            assert_eq!(
                entry.synthetic_fqdn,
                format!("<builtin>::rust::{trait_name}::{method}")
            );
        }
    }

    #[test]
    fn synthetic_fqdns_use_short_language_slug() {
        let reg = standard();
        let json = reg
            .lookup("JSON", Language::JavaScript)
            .expect("JSON registered");
        assert_eq!(json.synthetic_fqdn, "<builtin>::js::JSON");

        let vec_t = reg.lookup("Vec", Language::Rust).expect("Vec registered");
        assert_eq!(vec_t.synthetic_fqdn, "<builtin>::rust::Vec");

        let pairs = reg
            .lookup("pairs", Language::Lua)
            .expect("pairs registered");
        assert_eq!(pairs.synthetic_fqdn, "<builtin>::lua::pairs");
    }
}

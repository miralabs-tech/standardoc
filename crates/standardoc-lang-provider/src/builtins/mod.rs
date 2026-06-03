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
    fn ts_registry_subsumes_js_ambient_globals() {
        // No `JsProvider` exists — `.js`/`.jsx`/`.vue`/`.svelte` all flow
        // through `TsProvider`, which looks builtins up under
        // `Language::TypeScript`. The ambient runtime globals must resolve
        // there or every bare `console` / `parseInt` / `Proxy` reference
        // lands `Unresolved`. They stay registered under JavaScript too
        // (seed parity) so a future `JsProvider` keeps working.
        let reg = standard();
        for name in [
            "console",
            "window",
            "document",
            "globalThis",
            "self",
            "parseInt",
            "parseFloat",
            "Proxy",
            "Reflect",
            "encodeURIComponent",
            "decodeURIComponent",
            "undefined",
            "NaN",
            "Infinity",
            "isNaN",
            "isFinite",
        ] {
            assert!(
                reg.lookup(name, Language::TypeScript).is_some(),
                "{name} must resolve under TypeScript (TsProvider live path)"
            );
            assert!(
                reg.lookup(name, Language::JavaScript).is_some(),
                "{name} must stay seeded under JavaScript"
            );
        }
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

    #[test]
    fn c_header_namespace_distinct_from_functions() {
        // C headers carry `.h` in their builtin fqdn so `#include <time.h>`
        // (Module) no longer collides with the `time()` function (Callable)
        // at the seeding layer — they are separate synthetic symbol rows.
        let reg = standard();
        let header = reg
            .lookup("time.h", Language::C)
            .expect("time.h header registered");
        assert_eq!(header.kind, standardoc_ir::Kind::Module);
        assert_eq!(header.synthetic_fqdn, "<builtin>::c::time.h");

        let func = reg
            .lookup("time", Language::C)
            .expect("time() function registered");
        assert_eq!(func.kind, standardoc_ir::Kind::Callable);
        assert_eq!(func.synthetic_fqdn, "<builtin>::c::time");
    }

    #[test]
    fn js_ts_error_globals_reach_parity() {
        // The 4 ECMAScript error globals TS already carried must also be
        // seeded under JavaScript so a `.js` doesn't under-classify them.
        let reg = standard();
        for name in [
            "Error",
            "TypeError",
            "RangeError",
            "SyntaxError",
            "ReferenceError",
            "EvalError",
            "URIError",
        ] {
            assert!(
                reg.lookup(name, Language::JavaScript).is_some(),
                "JS {name}"
            );
            assert!(
                reg.lookup(name, Language::TypeScript).is_some(),
                "TS {name}"
            );
        }
    }

    #[test]
    fn namespace_globals_are_modules_in_both_registries() {
        // `Math`/`JSON` are namespace objects (member access, never
        // constructed) — `Kind::Module` across JS and TS, matching
        // `console`. Guards against the prior JS-Module / TS-Type split.
        let reg = standard();
        for lang in [Language::JavaScript, Language::TypeScript] {
            for name in ["Math", "JSON"] {
                let entry = reg.lookup(name, lang).expect("namespace global seeded");
                assert_eq!(
                    entry.kind,
                    standardoc_ir::Kind::Module,
                    "{name} in {lang:?}"
                );
            }
        }
    }

    #[test]
    fn int_methods_are_split_by_signedness() {
        // `abs` is signed-only, `is_power_of_two` unsigned-only — no
        // phantom `u8::abs` / `i8::is_power_of_two` rows get seeded.
        let reg = standard();
        assert!(reg.lookup_method("i8", "abs", Language::Rust).is_some());
        assert!(reg.lookup_method("u8", "abs", Language::Rust).is_none());
        let u8_p2 = reg.lookup_method("u8", "is_power_of_two", Language::Rust);
        let i8_p2 = reg.lookup_method("i8", "is_power_of_two", Language::Rust);
        assert!(u8_p2.is_some());
        assert!(i8_p2.is_none());
        // Common surface still seeded on both halves.
        let u8_sat = reg.lookup_method("u8", "saturating_add", Language::Rust);
        let i8_sat = reg.lookup_method("i8", "saturating_add", Language::Rust);
        assert!(u8_sat.is_some());
        assert!(i8_sat.is_some());
    }
}

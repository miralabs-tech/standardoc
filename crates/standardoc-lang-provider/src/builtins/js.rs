use standardoc_ir::{BuiltinEntry, BuiltinRegistry, BuiltinTag, BuiltinTier, Kind, Language};

/// Ambient JS runtime globals — identifiers reachable without an import in
/// both JS and TS (`console`, `window`, `parseInt`, `Proxy`, `undefined`, …).
/// Shared with [`crate::builtins::ts`] so the TS registry stays a strict
/// superset of JS: there is no `JsProvider`, so every `.js`/`.jsx`/`.ts`/
/// `.tsx`/`.vue`/`.svelte` file resolves builtins against the TS map under
/// [`Language::TypeScript`] — a global missing there resolves `Unresolved`
/// instead of being classified.
pub(crate) fn register_ambient_globals(reg: &mut BuiltinRegistry, language: Language) {
    let add = |reg: &mut BuiltinRegistry,
               names: &[&str],
               kind: Kind,
               tag: BuiltinTag,
               tier: BuiltinTier| {
        for name in names {
            reg.register(BuiltinEntry::new(*name, language, kind, tag.clone(), tier));
        }
    };

    // --- Tier::Edge --- I/O surface, observable effects, audit-relevant
    add(
        reg,
        &["console"],
        Kind::Module,
        BuiltinTag::Console,
        BuiltinTier::Edge,
    );
    add(
        reg,
        &["window", "document", "globalThis", "self"],
        Kind::Value,
        BuiltinTag::Custom {
            tag: "global-object".into(),
        },
        BuiltinTier::Edge,
    );
    add(
        reg,
        &["Proxy", "Reflect"],
        Kind::Type,
        BuiltinTag::Reflection,
        BuiltinTier::Edge,
    );
    add(
        reg,
        &["parseInt", "parseFloat"],
        Kind::Callable,
        BuiltinTag::Decode,
        BuiltinTier::Edge,
    );
    add(
        reg,
        &["encodeURI", "encodeURIComponent"],
        Kind::Callable,
        BuiltinTag::Encode,
        BuiltinTier::Edge,
    );
    add(
        reg,
        &["decodeURI", "decodeURIComponent"],
        Kind::Callable,
        BuiltinTag::Decode,
        BuiltinTier::Edge,
    );

    // --- Tier::Drop --- structural noise, no edge, no attribute
    add(
        reg,
        &["undefined", "NaN", "Infinity"],
        Kind::Value,
        BuiltinTag::Custom {
            tag: "global-constant".into(),
        },
        BuiltinTier::Drop,
    );
    // Predicate helpers — structural, never semantically interesting.
    add(
        reg,
        &["isNaN", "isFinite"],
        Kind::Callable,
        BuiltinTag::Reflection,
        BuiltinTier::Drop,
    );
}

pub(crate) fn register_all(reg: &mut BuiltinRegistry) {
    register_ambient_globals(reg, Language::JavaScript);

    let add = |reg: &mut BuiltinRegistry,
               names: &[&str],
               kind: Kind,
               tag: BuiltinTag,
               tier: BuiltinTier| {
        for name in names {
            reg.register(BuiltinEntry::new(
                *name,
                Language::JavaScript,
                kind,
                tag.clone(),
                tier,
            ));
        }
    };

    // --- Tier::Edge --- std-lib namespaces, reflection, observable APIs
    add(
        reg,
        &["Math"],
        Kind::Module,
        BuiltinTag::Math,
        BuiltinTier::Edge,
    );
    add(
        reg,
        &["Date"],
        Kind::Type,
        BuiltinTag::Time,
        BuiltinTier::Edge,
    );
    add(
        reg,
        &["JSON"],
        Kind::Module,
        BuiltinTag::Custom { tag: "json".into() },
        BuiltinTier::Edge,
    );
    add(
        reg,
        &["RegExp"],
        Kind::Type,
        BuiltinTag::Format,
        BuiltinTier::Edge,
    );
    add(
        reg,
        &[
            "Error",
            "TypeError",
            "RangeError",
            "SyntaxError",
            "ReferenceError",
            "EvalError",
            "URIError",
        ],
        Kind::Type,
        BuiltinTag::Custom {
            tag: "error".into(),
        },
        BuiltinTier::Edge,
    );
    // `Object` and `Symbol` are reflection/metaprogramming surfaces
    // (Object.keys/freeze/assign, Symbol.iterator) — track as edges.
    add(
        reg,
        &["Object", "Symbol"],
        Kind::Type,
        BuiltinTag::Reflection,
        BuiltinTier::Edge,
    );

    // --- Tier::Attribute --- semantic effect folded into the source symbol
    // `Promise<T>` → source fn flagged async; the wrapper itself is not
    // an edge target (the inner type arg is still recursed normally).
    add(
        reg,
        &["Promise"],
        Kind::Type,
        BuiltinTag::Async,
        BuiltinTier::Attribute,
    );

    // --- Tier::Drop --- structural noise, no edge, no attribute
    // Primitive-cast wrappers: `Array()`, `String()`, `Number()`, `Boolean()`
    // — pure conversion noise, no semantic edge worth drawing.
    add(
        reg,
        &["Array", "Number", "String", "Boolean"],
        Kind::Type,
        BuiltinTag::Reflection,
        BuiltinTier::Drop,
    );
    // Data structure wrappers: the value is in the contained type, not
    // the container — equivalent to Rust's `Vec` / `HashMap` Drop tier.
    add(
        reg,
        &["Map", "Set", "WeakMap", "WeakSet"],
        Kind::Type,
        BuiltinTag::Iter,
        BuiltinTier::Drop,
    );
}

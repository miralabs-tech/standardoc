use standardoc_ir::{BuiltinEntry, BuiltinRegistry, BuiltinTag, BuiltinTier, Kind, Language};

use super::js::register_ambient_globals;

pub(crate) fn register_all(reg: &mut BuiltinRegistry) {
    // TS is a strict superset of JS — seed the ambient runtime globals
    // (`console`, `parseInt`, `Proxy`, …) so they resolve here too. Every
    // TsProvider file looks builtins up under `Language::TypeScript`.
    register_ambient_globals(reg, Language::TypeScript);

    let add = |reg: &mut BuiltinRegistry,
               names: &[&str],
               kind: Kind,
               tag: BuiltinTag,
               tier: BuiltinTier| {
        for name in names {
            reg.register(BuiltinEntry::new(
                *name,
                Language::TypeScript,
                kind,
                tag.clone(),
                tier,
            ));
        }
    };

    // --- Tier::Edge --- reflection, errors, observable APIs
    add(
        reg,
        &["Object", "Symbol"],
        Kind::Type,
        BuiltinTag::Reflection,
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
    // `JSON`/`Math` are namespace objects — you call `JSON.parse` /
    // `Math.max`, never construct them — so `Kind::Module`, consistent
    // with `console` (shared ambient seed) and the JS registry.
    add(
        reg,
        &["JSON"],
        Kind::Module,
        BuiltinTag::Custom { tag: "json".into() },
        BuiltinTier::Edge,
    );
    add(
        reg,
        &["Math"],
        Kind::Module,
        BuiltinTag::Math,
        BuiltinTier::Edge,
    );

    // --- Tier::Attribute --- semantic effect folded into the source symbol
    // Promise & friends → source fn flagged async; the wrapper itself
    // is not an edge target (the inner type arg is still recursed).
    add(
        reg,
        &["Promise", "PromiseLike"],
        Kind::Type,
        BuiltinTag::Async,
        BuiltinTier::Attribute,
    );
    // Iter trait family — implementing or returning these implies an
    // iter-shape on the source symbol; flag it, don't draw an edge.
    add(
        reg,
        &[
            "Iterator",
            "Iterable",
            "IterableIterator",
            "Generator",
            "GeneratorFunction",
        ],
        Kind::Type,
        BuiltinTag::Iter,
        BuiltinTier::Attribute,
    );
    // Async iter family — same logic, async-flavored.
    add(
        reg,
        &[
            "AsyncIterator",
            "AsyncIterable",
            "AsyncIterableIterator",
            "AsyncGenerator",
            "AsyncGeneratorFunction",
        ],
        Kind::Type,
        BuiltinTag::Async,
        BuiltinTier::Attribute,
    );

    // --- Tier::Drop --- structural noise, no edge, no attribute
    // Container generics — the value is in the type arg, not the
    // container itself (analogous to Rust Vec / HashMap Drop tier).
    add(
        reg,
        &[
            "Array",
            "ReadonlyArray",
            "Map",
            "ReadonlyMap",
            "Set",
            "ReadonlySet",
            "WeakMap",
            "WeakSet",
        ],
        Kind::Type,
        BuiltinTag::Iter,
        BuiltinTier::Drop,
    );
    // TS utility / mapped types — pure type-level reflection, never
    // observable at runtime, drawing edges to them is noise.
    add(
        reg,
        &[
            "Partial",
            "Required",
            "Readonly",
            "Pick",
            "Omit",
            "Exclude",
            "Extract",
            "NonNullable",
            "ReturnType",
            "Parameters",
            "InstanceType",
            "Record",
            "ConstructorParameters",
            "ThisParameterType",
            "OmitThisParameter",
            "ThisType",
            "Capitalize",
            "Uncapitalize",
            "Lowercase",
            "Uppercase",
            "NoInfer",
            "Awaited",
        ],
        Kind::Type,
        BuiltinTag::Reflection,
        BuiltinTier::Drop,
    );
    add(
        reg,
        &["Function"],
        Kind::Type,
        BuiltinTag::Custom {
            tag: "callable".into(),
        },
        BuiltinTier::Drop,
    );
    // Boxed primitive constructors — pure cast wrappers.
    add(
        reg,
        &["Number", "String", "Boolean"],
        Kind::Type,
        BuiltinTag::Reflection,
        BuiltinTier::Drop,
    );
    // Typed arrays / raw memory buffers — equivalent to Rust slices,
    // drop the wrapper edge; element-type info is captured elsewhere.
    add(
        reg,
        &[
            "ArrayBuffer",
            "ArrayBufferLike",
            "SharedArrayBuffer",
            "DataView",
            "Int8Array",
            "Uint8Array",
            "Uint8ClampedArray",
            "Int16Array",
            "Uint16Array",
            "Int32Array",
            "Uint32Array",
            "Float32Array",
            "Float64Array",
            "BigInt64Array",
            "BigUint64Array",
        ],
        Kind::Type,
        BuiltinTag::Memory,
        BuiltinTier::Drop,
    );
}

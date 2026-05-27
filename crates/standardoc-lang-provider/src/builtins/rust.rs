use standardoc_ir::{
    BuiltinEntry, BuiltinMethodEntry, BuiltinRegistry, BuiltinTag, BuiltinTier, Kind, Language,
};

pub(crate) fn register_all(reg: &mut BuiltinRegistry) {
    register_types_and_macros(reg);
    register_methods(reg);
}

fn register_types_and_macros(reg: &mut BuiltinRegistry) {
    let add = |reg: &mut BuiltinRegistry,
               names: &[&str],
               kind: Kind,
               tag: BuiltinTag,
               tier: BuiltinTier| {
        for name in names {
            reg.register(BuiltinEntry::new(
                *name,
                Language::Rust,
                kind,
                tag.clone(),
                tier,
            ));
        }
    };

    // Reserved markers `Self` / `self` / `_` are intentionally NOT in the
    // registry — they're syntactic placeholders, not real symbols.
    // Consumers handle them via a local SKIP_MARKERS const in the
    // extraction layer.

    // --- Tier::Edge --- error trait — implementing it is a semantic
    // "this is an error type" signal worth showing in the graph.
    add(
        reg,
        &["Error"],
        Kind::Type,
        BuiltinTag::Custom {
            tag: "error".into(),
        },
        BuiltinTier::Edge,
    );

    // --- Tier::Attribute --- iter-ness / async-ness folded into source
    // Implementing `Iterator` or returning `impl Iterator` flags the
    // source symbol as iter-shaped; same for `Future`/`Stream` → async.
    add(
        reg,
        &["Iterator", "IntoIterator", "FromIterator"],
        Kind::Type,
        BuiltinTag::Iter,
        BuiltinTier::Attribute,
    );
    add(
        reg,
        &["Future", "Stream"],
        Kind::Type,
        BuiltinTag::Async,
        BuiltinTier::Attribute,
    );

    // --- Tier::Drop --- structural noise, no edge, no attribute
    // Primitive scalars — ubiquitous, the inner type info is the value.
    add(
        reg,
        &[
            "bool", "char", "str", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32",
            "i64", "i128", "isize", "f32", "f64",
        ],
        Kind::Type,
        BuiltinTag::Memory,
        BuiltinTier::Drop,
    );
    // Heap / smart pointers — wrap a payload of interest; the payload
    // (inner type arg) is what gets recursed, the wrapper is noise.
    add(
        reg,
        &[
            "Box",
            "Rc",
            "Arc",
            "Pin",
            "Cell",
            "RefCell",
            "UnsafeCell",
            "Mutex",
            "RwLock",
        ],
        Kind::Type,
        BuiltinTag::Memory,
        BuiltinTier::Drop,
    );
    // Standard collections — same logic as TS Map/Set, JS Array.
    add(
        reg,
        &[
            "Vec",
            "VecDeque",
            "LinkedList",
            "BinaryHeap",
            "HashMap",
            "HashSet",
            "BTreeMap",
            "BTreeSet",
        ],
        Kind::Type,
        BuiltinTag::Iter,
        BuiltinTier::Drop,
    );
    // Strings & paths — too ubiquitous to draw edges; the type info
    // is captured on the symbol's signature.returns slot directly.
    add(
        reg,
        &[
            "String", "OsString", "OsStr", "PathBuf", "Path", "CString", "CStr",
        ],
        Kind::Type,
        BuiltinTag::Format,
        BuiltinTier::Drop,
    );
    // Sum / option containers — `Result`/`Option` permeate every Rust
    // function signature; tracing them is pure noise. The error-or-ok
    // shape is implicit in `signature.returns`.
    add(
        reg,
        &["Option", "Result", "Cow"],
        Kind::Type,
        BuiltinTag::Reflection,
        BuiltinTier::Drop,
    );
    add(
        reg,
        &["Some", "None", "Ok", "Err"],
        Kind::Type,
        BuiltinTag::Custom {
            tag: "variant".into(),
        },
        BuiltinTier::Drop,
    );
    // Marker traits — auto-derived, structural.
    add(
        reg,
        &["Send", "Sync", "Sized", "Unpin", "Unsize"],
        Kind::Type,
        BuiltinTag::Reflection,
        BuiltinTier::Drop,
    );
    // Common derive / blanket traits — ubiquitous, no audit value.
    add(
        reg,
        &[
            "Drop",
            "Clone",
            "Copy",
            "Default",
            "PartialEq",
            "Eq",
            "PartialOrd",
            "Ord",
            "Hash",
            "Debug",
            "Display",
            "From",
            "Into",
            "TryFrom",
            "TryInto",
            "AsRef",
            "AsMut",
            "Borrow",
            "BorrowMut",
            "ToString",
            "ToOwned",
        ],
        Kind::Type,
        BuiltinTag::Reflection,
        BuiltinTier::Drop,
    );
    // Callable trait family — closure-shape, structural.
    add(
        reg,
        &["Fn", "FnMut", "FnOnce"],
        Kind::Type,
        BuiltinTag::Custom {
            tag: "callable".into(),
        },
        BuiltinTier::Drop,
    );

    // --- Tier::Drop --- declarative + compile-time macros. The macro
    // itself is a well-known API surface, and its body can't be analyzed
    // without macro expansion ; emitting Calls edges to them adds noise
    // proportional to test density (~7300 test-macro CALLS pre-fix) with
    // zero audit value. The matching `RawCallSite` row is still emitted
    // upstream of the registry check so consumers wanting raw macro
    // counts continue to get them.
    add(
        reg,
        &[
            // Assertions
            "assert",
            "assert_eq",
            "assert_ne",
            "debug_assert",
            "debug_assert_eq",
            "debug_assert_ne",
            // Diverging
            "panic",
            "unimplemented",
            "unreachable",
            "todo",
            // Print / debug
            "print",
            "println",
            "eprint",
            "eprintln",
            "dbg",
            // Format
            "format",
            "write",
            "writeln",
            // Collection
            "vec",
            // Pattern
            "matches",
            // Compile-time
            "include_str",
            "include_bytes",
            "include",
            "env",
            "option_env",
            "cfg",
            "file",
            "line",
            "column",
            "module_path",
            "stringify",
            "concat",
        ],
        Kind::Macro,
        BuiltinTag::Custom {
            tag: "macro".into(),
        },
        BuiltinTier::Drop,
    );
}

/// Bug E-3 Phase 2: register a focused set of stdlib method entries
/// for the receiver types most often seen in bare-ident unresolved
/// CALLS edges (top 15 from the P1.0 baseline). Activated by the
/// resolver only when Phase 1 attached a `receiver_type` matching one
/// of these parents — so disambiguation is name + receiver, no blind
/// suffix matching.
fn register_methods(reg: &mut BuiltinRegistry) {
    let add = |reg: &mut BuiltinRegistry, parent_type: &str, methods: &[&str]| {
        for m in methods {
            reg.register_method(BuiltinMethodEntry::new(parent_type, *m, Language::Rust));
        }
    };
    // Bug E-3 Phase 3.1: bulk-register methods that share an unambiguous
    // return type. Read by `compute_receiver_type` to walk chained calls
    // like `x.iter().map(...).filter(...)` — each step's return becomes
    // the next step's receiver.
    let add_returning =
        |reg: &mut BuiltinRegistry, parent_type: &str, returns: &str, methods: &[&str]| {
            for m in methods {
                reg.register_method(
                    BuiltinMethodEntry::new(parent_type, *m, Language::Rust).with_returns(returns),
                );
            }
        };

    add(
        reg,
        "Vec",
        &[
            "push",
            "pop",
            "len",
            "is_empty",
            "clear",
            "clone",
            "contains",
            "extend",
            "sort",
            "sort_by",
            "sort_by_key",
            "get",
            "get_mut",
            "insert",
            "remove",
            "swap_remove",
            "truncate",
            "retain",
            "first",
            "last",
            "split_off",
            "with_capacity",
            "capacity",
            "reserve",
            "dedup",
            "concat",
            "join",
        ],
    );
    add_returning(
        reg,
        "Vec",
        "Iterator",
        &["iter", "iter_mut", "into_iter", "drain"],
    );
    add_returning(reg, "Vec", "slice", &["as_slice", "as_mut_slice"]);

    add(
        reg,
        "Option",
        &[
            "unwrap",
            "unwrap_or",
            "unwrap_or_else",
            "unwrap_or_default",
            "expect",
            "is_some",
            "is_none",
            "is_some_and",
            "map_or",
            "map_or_else",
            "take",
            "replace",
            "cloned",
            "copied",
        ],
    );
    add_returning(
        reg,
        "Option",
        "Option",
        &[
            "as_ref",
            "as_mut",
            "as_deref",
            "as_deref_mut",
            "map",
            "and",
            "and_then",
            "or",
            "or_else",
            "filter",
        ],
    );
    add_returning(reg, "Option", "Result", &["ok_or", "ok_or_else"]);
    add_returning(reg, "Option", "Iterator", &["iter"]);

    add(
        reg,
        "Result",
        &[
            "unwrap",
            "unwrap_or",
            "unwrap_or_else",
            "unwrap_or_default",
            "unwrap_err",
            "expect",
            "expect_err",
            "is_ok",
            "is_err",
            "is_ok_and",
            "is_err_and",
            "map_or",
            "map_or_else",
        ],
    );
    add_returning(
        reg,
        "Result",
        "Result",
        &[
            "as_ref", "as_mut", "map", "map_err", "and", "and_then", "or", "or_else",
        ],
    );
    add_returning(reg, "Result", "Option", &["ok", "err"]);

    add(
        reg,
        "Iterator",
        &[
            "next",
            "collect",
            "count",
            "sum",
            "product",
            "fold",
            "reduce",
            "for_each",
            "any",
            "all",
            "size_hint",
        ],
    );
    // Iterator adapters return `impl Iterator` — treated nominally as
    // "Iterator" so the chain `x.iter().map(...).filter(...).collect()`
    // keeps walking.
    add_returning(
        reg,
        "Iterator",
        "Iterator",
        &[
            "map",
            "filter",
            "filter_map",
            "flat_map",
            "flatten",
            "zip",
            "chain",
            "take",
            "take_while",
            "skip",
            "skip_while",
            "enumerate",
            "peekable",
            "rev",
            "cloned",
            "copied",
            "cycle",
            "step_by",
            "inspect",
            "by_ref",
            "scan",
        ],
    );
    add_returning(
        reg,
        "Iterator",
        "Option",
        &[
            "find",
            "find_map",
            "position",
            "min",
            "max",
            "min_by",
            "max_by",
            "min_by_key",
            "max_by_key",
            "last",
            "nth",
        ],
    );

    add(
        reg,
        "String",
        &[
            "push",
            "push_str",
            "len",
            "is_empty",
            "as_bytes",
            "as_mut_str",
            "clone",
            "into_bytes",
            "clear",
            "truncate",
            "contains",
            "starts_with",
            "ends_with",
            "find",
            "rfind",
            "capacity",
            "with_capacity",
            "from_utf8",
            "from_utf8_lossy",
            "into",
        ],
    );
    add_returning(
        reg,
        "String",
        "String",
        &[
            "to_string",
            "replace",
            "replacen",
            "to_lowercase",
            "to_uppercase",
            "repeat",
            "to_owned",
        ],
    );
    add_returning(
        reg,
        "String",
        "str",
        &["as_str", "trim", "trim_start", "trim_end"],
    );
    add_returning(
        reg,
        "String",
        "Iterator",
        &[
            "chars",
            "bytes",
            "lines",
            "split",
            "splitn",
            "split_whitespace",
        ],
    );
    add_returning(reg, "String", "Result", &["parse"]);

    add(
        reg,
        "str",
        &[
            "len",
            "is_empty",
            "as_bytes",
            "contains",
            "starts_with",
            "ends_with",
            "find",
            "rfind",
            "as_ptr",
            "is_ascii",
            "split_at",
        ],
    );
    add_returning(
        reg,
        "str",
        "String",
        &[
            "to_string",
            "to_owned",
            "to_lowercase",
            "to_uppercase",
            "replace",
            "replacen",
            "repeat",
        ],
    );
    add_returning(reg, "str", "str", &["trim", "trim_start", "trim_end"]);
    add_returning(
        reg,
        "str",
        "Iterator",
        &[
            "split",
            "splitn",
            "split_whitespace",
            "chars",
            "bytes",
            "lines",
            "matches",
            "char_indices",
        ],
    );
    add_returning(reg, "str", "Option", &["strip_prefix", "strip_suffix"]);
    add_returning(reg, "str", "Result", &["parse"]);

    add(
        reg,
        "HashMap",
        &[
            "insert",
            "get",
            "get_mut",
            "remove",
            "contains_key",
            "len",
            "is_empty",
            "entry",
            "clear",
            "with_capacity",
            "capacity",
            "extend",
            "retain",
        ],
    );
    add_returning(
        reg,
        "HashMap",
        "Iterator",
        &[
            "iter",
            "iter_mut",
            "into_iter",
            "keys",
            "values",
            "values_mut",
            "drain",
        ],
    );

    add(
        reg,
        "BTreeMap",
        &[
            "insert",
            "get",
            "get_mut",
            "remove",
            "contains_key",
            "len",
            "is_empty",
            "entry",
            "clear",
            "first_key_value",
            "last_key_value",
        ],
    );
    add_returning(
        reg,
        "BTreeMap",
        "Iterator",
        &[
            "iter",
            "iter_mut",
            "into_iter",
            "keys",
            "values",
            "values_mut",
            "range",
            "range_mut",
        ],
    );

    add(
        reg,
        "HashSet",
        &[
            "insert",
            "remove",
            "contains",
            "len",
            "is_empty",
            "clear",
            "with_capacity",
            "extend",
            "retain",
        ],
    );
    add_returning(
        reg,
        "HashSet",
        "Iterator",
        &[
            "iter",
            "into_iter",
            "drain",
            "intersection",
            "union",
            "difference",
            "symmetric_difference",
        ],
    );

    add(
        reg,
        "BTreeSet",
        &[
            "insert", "remove", "contains", "len", "is_empty", "clear", "first", "last",
        ],
    );
    add_returning(reg, "BTreeSet", "Iterator", &["iter", "into_iter", "range"]);

    add(
        reg,
        "PathBuf",
        &[
            "new",
            "push",
            "pop",
            "set_extension",
            "set_file_name",
            "into_os_string",
            "with_capacity",
            "capacity",
            "clear",
            "from",
            "to_string_lossy",
            "to_str",
            "exists",
            "is_file",
            "is_dir",
            "starts_with",
            "ends_with",
            "is_absolute",
            "is_relative",
            "display",
        ],
    );
    add_returning(reg, "PathBuf", "Path", &["as_path"]);
    add_returning(
        reg,
        "PathBuf",
        "Option",
        &["file_name", "file_stem", "extension", "parent"],
    );
    add_returning(reg, "PathBuf", "Iterator", &["components", "ancestors"]);
    add_returning(reg, "PathBuf", "PathBuf", &["join"]);
    add_returning(
        reg,
        "PathBuf",
        "Result",
        &["canonicalize", "read_dir", "metadata", "strip_prefix"],
    );

    add(
        reg,
        "Path",
        &[
            "new",
            "to_string_lossy",
            "to_str",
            "exists",
            "is_file",
            "is_dir",
            "starts_with",
            "ends_with",
            "is_absolute",
            "is_relative",
            "as_os_str",
            "display",
        ],
    );
    add_returning(reg, "Path", "PathBuf", &["to_path_buf", "join"]);
    add_returning(
        reg,
        "Path",
        "Option",
        &["file_name", "file_stem", "extension", "parent"],
    );
    add_returning(reg, "Path", "Iterator", &["components", "ancestors"]);
    add_returning(
        reg,
        "Path",
        "Result",
        &["canonicalize", "read_dir", "metadata", "strip_prefix"],
    );
}

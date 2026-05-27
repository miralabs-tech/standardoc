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

    add(
        reg,
        "Vec",
        &[
            "push",
            "pop",
            "len",
            "is_empty",
            "iter",
            "iter_mut",
            "into_iter",
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
            "drain",
            "retain",
            "first",
            "last",
            "as_slice",
            "as_mut_slice",
            "split_off",
            "with_capacity",
            "capacity",
            "reserve",
            "dedup",
            "concat",
            "join",
        ],
    );

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
            "as_ref",
            "as_mut",
            "as_deref",
            "as_deref_mut",
            "map",
            "map_or",
            "map_or_else",
            "and",
            "and_then",
            "or",
            "or_else",
            "take",
            "replace",
            "filter",
            "ok_or",
            "ok_or_else",
            "cloned",
            "copied",
            "iter",
        ],
    );

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
            "ok",
            "err",
            "as_ref",
            "as_mut",
            "map",
            "map_err",
            "map_or",
            "map_or_else",
            "and",
            "and_then",
            "or",
            "or_else",
        ],
    );

    add(
        reg,
        "Iterator",
        &[
            "next",
            "map",
            "filter",
            "filter_map",
            "flat_map",
            "flatten",
            "collect",
            "count",
            "sum",
            "product",
            "fold",
            "reduce",
            "for_each",
            "find",
            "find_map",
            "position",
            "any",
            "all",
            "min",
            "max",
            "min_by",
            "max_by",
            "min_by_key",
            "max_by_key",
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
            "last",
            "nth",
            "size_hint",
            "scan",
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
            "as_str",
            "as_bytes",
            "as_mut_str",
            "clone",
            "into_bytes",
            "clear",
            "truncate",
            "trim",
            "trim_start",
            "trim_end",
            "to_string",
            "to_owned",
            "contains",
            "starts_with",
            "ends_with",
            "split",
            "splitn",
            "split_whitespace",
            "replace",
            "replacen",
            "to_lowercase",
            "to_uppercase",
            "chars",
            "bytes",
            "lines",
            "find",
            "rfind",
            "parse",
            "repeat",
            "capacity",
            "with_capacity",
            "from_utf8",
            "from_utf8_lossy",
            "into",
        ],
    );

    add(
        reg,
        "str",
        &[
            "len",
            "is_empty",
            "as_bytes",
            "to_string",
            "to_owned",
            "contains",
            "starts_with",
            "ends_with",
            "split",
            "splitn",
            "split_whitespace",
            "split_at",
            "replace",
            "replacen",
            "to_lowercase",
            "to_uppercase",
            "chars",
            "bytes",
            "lines",
            "find",
            "rfind",
            "parse",
            "repeat",
            "trim",
            "trim_start",
            "trim_end",
            "strip_prefix",
            "strip_suffix",
            "matches",
            "char_indices",
            "as_ptr",
            "is_ascii",
        ],
    );

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
            "iter",
            "iter_mut",
            "into_iter",
            "keys",
            "values",
            "values_mut",
            "clear",
            "with_capacity",
            "capacity",
            "extend",
            "drain",
            "retain",
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
            "iter",
            "iter_mut",
            "into_iter",
            "keys",
            "values",
            "values_mut",
            "clear",
            "range",
            "range_mut",
            "first_key_value",
            "last_key_value",
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
            "iter",
            "into_iter",
            "clear",
            "with_capacity",
            "intersection",
            "union",
            "difference",
            "symmetric_difference",
            "extend",
            "drain",
            "retain",
        ],
    );

    add(
        reg,
        "BTreeSet",
        &[
            "insert",
            "remove",
            "contains",
            "len",
            "is_empty",
            "iter",
            "into_iter",
            "clear",
            "range",
            "first",
            "last",
        ],
    );

    add(
        reg,
        "PathBuf",
        &[
            "new",
            "push",
            "pop",
            "set_extension",
            "set_file_name",
            "as_path",
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
            "file_name",
            "file_stem",
            "extension",
            "parent",
            "components",
            "ancestors",
            "join",
            "display",
            "canonicalize",
            "read_dir",
            "metadata",
            "starts_with",
            "ends_with",
            "strip_prefix",
            "is_absolute",
            "is_relative",
        ],
    );

    add(
        reg,
        "Path",
        &[
            "new",
            "to_path_buf",
            "to_string_lossy",
            "to_str",
            "exists",
            "is_file",
            "is_dir",
            "file_name",
            "file_stem",
            "extension",
            "parent",
            "components",
            "ancestors",
            "join",
            "display",
            "canonicalize",
            "read_dir",
            "metadata",
            "starts_with",
            "ends_with",
            "strip_prefix",
            "is_absolute",
            "is_relative",
            "as_os_str",
        ],
    );
}

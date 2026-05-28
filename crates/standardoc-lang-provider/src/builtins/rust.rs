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
    //
    // Bug E-3 ext P-E3.2: the `returns` string may be a parametric
    // template (`"Iterator<T>"`) — substituted against the receiver's
    // generic args at lookup time using rules per parent nominal
    // (`T` = args[0]; `E` = args[1] for Result; `K` / `V` = args[0] /
    // args[1] for maps).
    let add_returning =
        |reg: &mut BuiltinRegistry, parent_type: &str, returns: &str, methods: &[&str]| {
            for m in methods {
                reg.register_method(
                    BuiltinMethodEntry::new(parent_type, *m, Language::Rust).with_returns(returns),
                );
            }
        };
    // Bug E-3 ext P-E3.2: register methods that take a closure arg of a
    // single template type — used by `visit_expr_method_call` to bind
    // each closure-input ident pat to the substituted arg type. Use
    // [`add_full`] when both `returns` and `closure_arg` apply.
    let add_with_closure =
        |reg: &mut BuiltinRegistry, parent_type: &str, closure_arg: &str, methods: &[&str]| {
            for m in methods {
                reg.register_method(
                    BuiltinMethodEntry::new(parent_type, *m, Language::Rust)
                        .with_closure_arg(closure_arg),
                );
            }
        };
    let add_full = |reg: &mut BuiltinRegistry,
                    parent_type: &str,
                    returns: &str,
                    closure_arg: &str,
                    methods: &[&str]| {
        for m in methods {
            reg.register_method(
                BuiltinMethodEntry::new(parent_type, *m, Language::Rust)
                    .with_returns(returns)
                    .with_closure_arg(closure_arg),
            );
        }
    };

    add(
        reg,
        "Vec",
        &[
            "push",
            "len",
            "is_empty",
            "clear",
            "clone",
            "contains",
            "extend",
            "sort",
            "sort_by",
            "sort_by_key",
            "insert",
            "truncate",
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
        "Iterator<T>",
        &["iter", "iter_mut", "into_iter", "drain"],
    );
    add_returning(reg, "Vec", "slice", &["as_slice", "as_mut_slice"]);
    add_with_closure(reg, "Vec", "T", &["retain"]);
    // Bug E-3.3: index-style methods on `Vec<T>` return the inner type
    // (either bare `T` for infallible variants or `Option<T>` for the
    // bounds-checked ones).
    add_returning(reg, "Vec", "T", &["swap_remove", "remove"]);
    add_returning(
        reg,
        "Vec",
        "Option<T>",
        &["pop", "first", "last", "get", "get_mut"],
    );

    add(
        reg,
        "Option",
        &["is_some", "is_none", "is_some_and", "map_or", "map_or_else"],
    );
    // Bug E-3.3: unwrap / expect / fallback variants yield the inner
    // type. Substitute via `T` against the receiver's generic args so
    // `Option<Foo>::unwrap()` propagates as `Foo` for chained
    // `.method()` resolution.
    add_returning(
        reg,
        "Option",
        "T",
        &[
            "unwrap",
            "unwrap_or",
            "unwrap_or_else",
            "unwrap_or_default",
            "expect",
            "take",
            "replace",
            "cloned",
            "copied",
        ],
    );
    add_returning(
        reg,
        "Option",
        "Option<T>",
        &[
            "as_ref",
            "as_mut",
            "as_deref",
            "as_deref_mut",
            "or",
            "or_else",
        ],
    );
    // Option::map / and_then / and transform the inner type (Option<U>);
    // we can't infer U here, so drop the generic in `returns` while still
    // binding the closure-arg from the receiver's T.
    add_returning(reg, "Option", "Option", &["and"]);
    add_full(reg, "Option", "Option", "T", &["map", "and_then"]);
    // Option::filter preserves T; closure receives `&T` (stripped by
    // [`strip_refs`] before binding).
    add_full(reg, "Option", "Option<T>", "T", &["filter"]);
    add_returning(reg, "Option", "Result", &["ok_or", "ok_or_else"]);
    add_returning(reg, "Option", "Iterator<T>", &["iter"]);

    add(
        reg,
        "Result",
        &[
            "is_ok",
            "is_err",
            "is_ok_and",
            "is_err_and",
            "map_or",
            "map_or_else",
        ],
    );
    // Bug E-3.3: Ok-branch unwrappers return T.
    add_returning(
        reg,
        "Result",
        "T",
        &[
            "unwrap",
            "unwrap_or",
            "unwrap_or_else",
            "unwrap_or_default",
            "expect",
        ],
    );
    // Bug E-3.3: Err-branch unwrappers return E.
    add_returning(reg, "Result", "E", &["unwrap_err", "expect_err"]);
    add_returning(reg, "Result", "Result<T, E>", &["as_ref", "as_mut", "or"]);
    // Result::map transforms the Ok branch (Result<U, E>); drop T from
    // the parametric chain but preserve E so a follow-up `.map_err(|e|
    // ...)` can still type its closure arg.
    add_full(reg, "Result", "Result<_, E>", "T", &["map", "and_then"]);
    // Result::map_err transforms the Err branch (Result<T, F>); preserve
    // T, drop E, bind closure-arg from E.
    add_full(reg, "Result", "Result<T, _>", "E", &["map_err", "or_else"]);
    add_returning(reg, "Result", "Result", &["and"]);
    add_returning(reg, "Result", "Option", &["ok", "err"]);

    add(
        reg,
        "Iterator",
        &[
            "collect",
            "count",
            "sum",
            "product",
            "fold",
            "reduce",
            "size_hint",
        ],
    );
    // Bug E-3.3: `next` yields `Option<Self::Item>`. With the receiver
    // tracked as `Iterator<T>` (Bug E-3.2 chain), `T` substitutes to the
    // Item type so `vec.iter().next().unwrap()` propagates `Foo`.
    add_returning(reg, "Iterator", "Option<T>", &["next"]);
    // Bug E-3 ext P-E3.2: split Iterator adapters by whether they
    // preserve the Item type or substitute it via a closure.
    //   * preserve T → `Iterator<T>` so subsequent `.find(|x| ...)` etc.
    //     can still type the closure arg.
    //   * transform T (map / filter_map / flat_map / scan) → drop to
    //     bare `Iterator` because the new Item type is the closure's
    //     return value (unknown without body inference).
    //   * `for_each` / `any` / `all` carry no return but take a closure
    //     over T.
    add_returning(
        reg,
        "Iterator",
        "Iterator<T>",
        &[
            "take",
            "skip",
            "enumerate",
            "peekable",
            "rev",
            "cloned",
            "copied",
            "cycle",
            "step_by",
            "by_ref",
            "flatten",
            "zip",
            "chain",
        ],
    );
    add_full(
        reg,
        "Iterator",
        "Iterator<T>",
        "T",
        &["filter", "take_while", "skip_while", "inspect"],
    );
    add_full(
        reg,
        "Iterator",
        "Iterator",
        "T",
        &["map", "filter_map", "flat_map", "scan"],
    );
    add_with_closure(reg, "Iterator", "T", &["for_each", "any", "all"]);
    add_returning(
        reg,
        "Iterator",
        "Option<T>",
        &[
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
    add_full(reg, "Iterator", "Option<T>", "T", &["find"]);
    add_full(reg, "Iterator", "Option", "T", &["find_map", "position"]);

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
    // iter / iter_mut / into_iter / drain yield `(&K, &V)` tuples —
    // V0 cannot bind tuple destructure, so keep them bare `Iterator`.
    add_returning(
        reg,
        "HashMap",
        "Iterator",
        &["iter", "iter_mut", "into_iter", "drain"],
    );
    add_returning(reg, "HashMap", "Iterator<K>", &["keys"]);
    add_returning(reg, "HashMap", "Iterator<V>", &["values", "values_mut"]);
    // Bug E-3.3: lookup-style methods on `HashMap<K, V>` return `Option<V>`.
    add_returning(reg, "HashMap", "Option<V>", &["get", "get_mut", "remove"]);

    add(
        reg,
        "BTreeMap",
        &[
            "insert",
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
        &["iter", "iter_mut", "into_iter", "range", "range_mut"],
    );
    add_returning(reg, "BTreeMap", "Iterator<K>", &["keys"]);
    add_returning(reg, "BTreeMap", "Iterator<V>", &["values", "values_mut"]);
    add_returning(reg, "BTreeMap", "Option<V>", &["get", "get_mut", "remove"]);

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
        ],
    );
    add_returning(
        reg,
        "HashSet",
        "Iterator<T>",
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
    add_with_closure(reg, "HashSet", "T", &["retain"]);

    add(
        reg,
        "BTreeSet",
        &[
            "insert", "remove", "contains", "len", "is_empty", "clear", "first", "last",
        ],
    );
    add_returning(
        reg,
        "BTreeSet",
        "Iterator<T>",
        &["iter", "into_iter", "range"],
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

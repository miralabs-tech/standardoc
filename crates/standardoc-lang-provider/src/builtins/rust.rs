use standardoc_ir::{BuiltinEntry, BuiltinRegistry, BuiltinTag, BuiltinTier, Kind, Language};

pub(crate) fn register_all(reg: &mut BuiltinRegistry) {
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
		BuiltinTag::Custom { tag: "error".into() },
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
	add(reg, &["Future", "Stream"], Kind::Type, BuiltinTag::Async, BuiltinTier::Attribute);

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
		BuiltinTag::Custom { tag: "variant".into() },
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
		BuiltinTag::Custom { tag: "callable".into() },
		BuiltinTier::Drop,
	);
}

use standardoc_ir::{BuiltinEntry, BuiltinRegistry, BuiltinTag, Kind, Language};

pub(crate) fn register_all(reg: &mut BuiltinRegistry) {
	let add = |reg: &mut BuiltinRegistry, names: &[&str], kind: Kind, tag: BuiltinTag| {
		for name in names {
			reg.register(BuiltinEntry::new(*name, Language::Rust, kind, tag.clone()));
		}
	};

	// Reserved markers — `Self`, `self`, `_` aren't really symbols.
	add(
		reg,
		&["Self", "self", "_"],
		Kind::Type,
		BuiltinTag::Reflection,
	);

	// Primitive scalars.
	add(
		reg,
		&[
			"bool", "char", "str", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32",
			"i64", "i128", "isize", "f32", "f64",
		],
		Kind::Type,
		BuiltinTag::Memory,
	);

	// Heap / smart pointers.
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
	);

	// Standard collections.
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
	);

	// Strings & paths.
	add(
		reg,
		&[
			"String", "OsString", "OsStr", "PathBuf", "Path", "CString", "CStr",
		],
		Kind::Type,
		BuiltinTag::Format,
	);

	// Sum / option containers (and their bare variants).
	add(
		reg,
		&["Option", "Result", "Cow"],
		Kind::Type,
		BuiltinTag::Reflection,
	);
	add(
		reg,
		&["Some", "None", "Ok", "Err"],
		Kind::Type,
		BuiltinTag::Custom { tag: "variant".into() },
	);

	// Iterator traits.
	add(
		reg,
		&["Iterator", "IntoIterator", "FromIterator"],
		Kind::Type,
		BuiltinTag::Iter,
	);

	// Futures / streams.
	add(reg, &["Future", "Stream"], Kind::Type, BuiltinTag::Async);

	// Marker traits.
	add(
		reg,
		&["Send", "Sync", "Sized", "Unpin", "Unsize"],
		Kind::Type,
		BuiltinTag::Reflection,
	);

	// Common derive / blanket traits.
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
	);
	add(
		reg,
		&["Error"],
		Kind::Type,
		BuiltinTag::Custom { tag: "error".into() },
	);

	// Callable traits.
	add(
		reg,
		&["Fn", "FnMut", "FnOnce"],
		Kind::Type,
		BuiltinTag::Custom { tag: "callable".into() },
	);
}

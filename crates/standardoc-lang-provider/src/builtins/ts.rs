use standardoc_ir::{BuiltinEntry, BuiltinRegistry, BuiltinTag, Kind, Language};

pub(crate) fn register_all(reg: &mut BuiltinRegistry) {
	let add = |reg: &mut BuiltinRegistry, names: &[&str], kind: Kind, tag: BuiltinTag| {
		for name in names {
			reg.register(BuiltinEntry::new(*name, Language::TypeScript, kind, tag.clone()));
		}
	};

	// Containers / collections.
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
	);

	// Async wrappers.
	add(
		reg,
		&["Promise", "PromiseLike", "Awaited"],
		Kind::Type,
		BuiltinTag::Async,
	);

	// Sync iteration protocols.
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
	);

	// Async iteration protocols.
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
	);

	// Utility / mapped types.
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
		],
		Kind::Type,
		BuiltinTag::Reflection,
	);

	// Callable / function shape.
	add(
		reg,
		&["Function"],
		Kind::Type,
		BuiltinTag::Custom { tag: "callable".into() },
	);

	// Misc lib types — boxed primitives & introspection containers.
	add(
		reg,
		&["Object", "Number", "String", "Boolean", "Symbol"],
		Kind::Type,
		BuiltinTag::Reflection,
	);
	add(reg, &["Date"], Kind::Type, BuiltinTag::Time);
	add(reg, &["RegExp"], Kind::Type, BuiltinTag::Format);
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
		BuiltinTag::Custom { tag: "error".into() },
	);
	add(
		reg,
		&["JSON"],
		Kind::Type,
		BuiltinTag::Custom { tag: "json".into() },
	);
	add(reg, &["Math"], Kind::Type, BuiltinTag::Math);

	// Typed arrays / raw memory buffers.
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
	);
}

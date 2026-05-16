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
				Language::JavaScript,
				kind,
				tag.clone(),
				tier,
			));
		}
	};

	// --- Tier::Edge --- I/O surface, observable effects, audit-relevant
	add(reg, &["console"], Kind::Module, BuiltinTag::Console, BuiltinTier::Edge);
	add(
		reg,
		&["window", "document", "globalThis", "self"],
		Kind::Value,
		BuiltinTag::Custom { tag: "global-object".into() },
		BuiltinTier::Edge,
	);
	add(reg, &["Math"], Kind::Module, BuiltinTag::Math, BuiltinTier::Edge);
	add(reg, &["Date"], Kind::Type, BuiltinTag::Time, BuiltinTier::Edge);
	add(
		reg,
		&["JSON"],
		Kind::Module,
		BuiltinTag::Custom { tag: "json".into() },
		BuiltinTier::Edge,
	);
	add(reg, &["RegExp"], Kind::Type, BuiltinTag::Format, BuiltinTier::Edge);
	add(
		reg,
		&["Error", "TypeError", "RangeError"],
		Kind::Type,
		BuiltinTag::Custom { tag: "error".into() },
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
	add(reg, &["Proxy", "Reflect"], Kind::Type, BuiltinTag::Reflection, BuiltinTier::Edge);
	add(reg, &["parseInt", "parseFloat"], Kind::Function, BuiltinTag::Decode, BuiltinTier::Edge);
	add(
		reg,
		&["encodeURI", "encodeURIComponent"],
		Kind::Function,
		BuiltinTag::Encode,
		BuiltinTier::Edge,
	);
	add(
		reg,
		&["decodeURI", "decodeURIComponent"],
		Kind::Function,
		BuiltinTag::Decode,
		BuiltinTier::Edge,
	);

	// --- Tier::Attribute --- semantic effect folded into the source symbol
	// `Promise<T>` → source fn flagged async; the wrapper itself is not
	// an edge target (the inner type arg is still recursed normally).
	add(reg, &["Promise"], Kind::Type, BuiltinTag::Async, BuiltinTier::Attribute);

	// --- Tier::Drop --- structural noise, no edge, no attribute
	add(
		reg,
		&["undefined", "NaN", "Infinity"],
		Kind::Value,
		BuiltinTag::Custom { tag: "global-constant".into() },
		BuiltinTier::Drop,
	);
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
	// Predicate helpers — structural, never semantically interesting.
	add(reg, &["isNaN", "isFinite"], Kind::Function, BuiltinTag::Reflection, BuiltinTier::Drop);
}

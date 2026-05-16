use standardoc_ir::{BuiltinEntry, BuiltinRegistry, BuiltinTag, Kind, Language};

pub(crate) fn register_all(reg: &mut BuiltinRegistry) {
	let add = |reg: &mut BuiltinRegistry, names: &[&str], kind: Kind, tag: BuiltinTag| {
		for name in names {
			reg.register(BuiltinEntry::new(*name, Language::JavaScript, kind, tag.clone()));
		}
	};

	add(reg, &["console"], Kind::Module, BuiltinTag::Console);
	add(
		reg,
		&["window", "document", "globalThis", "self"],
		Kind::Value,
		BuiltinTag::Custom { tag: "global-object".into() },
	);
	add(
		reg,
		&["undefined", "NaN", "Infinity"],
		Kind::Value,
		BuiltinTag::Custom { tag: "global-constant".into() },
	);
	add(reg, &["Math"], Kind::Module, BuiltinTag::Math);
	add(reg, &["Date"], Kind::Type, BuiltinTag::Time);
	add(
		reg,
		&["Object", "Array", "Number", "String", "Boolean", "Symbol"],
		Kind::Type,
		BuiltinTag::Reflection,
	);
	add(
		reg,
		&["JSON"],
		Kind::Module,
		BuiltinTag::Custom { tag: "json".into() },
	);
	add(reg, &["RegExp"], Kind::Type, BuiltinTag::Format);
	add(
		reg,
		&["Error", "TypeError", "RangeError"],
		Kind::Type,
		BuiltinTag::Custom { tag: "error".into() },
	);
	add(
		reg,
		&["Map", "Set", "WeakMap", "WeakSet"],
		Kind::Type,
		BuiltinTag::Iter,
	);
	add(reg, &["Promise"], Kind::Type, BuiltinTag::Async);
	add(reg, &["Proxy", "Reflect"], Kind::Type, BuiltinTag::Reflection);
	add(
		reg,
		&["parseInt", "parseFloat"],
		Kind::Function,
		BuiltinTag::Decode,
	);
	add(
		reg,
		&["isNaN", "isFinite"],
		Kind::Function,
		BuiltinTag::Reflection,
	);
	add(
		reg,
		&["encodeURI", "encodeURIComponent"],
		Kind::Function,
		BuiltinTag::Encode,
	);
	add(
		reg,
		&["decodeURI", "decodeURIComponent"],
		Kind::Function,
		BuiltinTag::Decode,
	);
}

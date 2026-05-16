use standardoc_ir::{BuiltinEntry, BuiltinRegistry, BuiltinTag, Kind, Language};

pub(crate) fn register_all(reg: &mut BuiltinRegistry) {
	let add = |reg: &mut BuiltinRegistry, names: &[&str], kind: Kind, tag: BuiltinTag| {
		for name in names {
			reg.register(BuiltinEntry::new(*name, Language::Lua, kind, tag.clone()));
		}
	};

	// Top-level builtin globals (Lua 5.1 / 5.4 prelude).
	add(
		reg,
		&[
			"assert",
			"error",
			"pcall",
			"xpcall",
			"select",
			"type",
			"rawequal",
			"rawget",
			"rawset",
			"rawlen",
			"getmetatable",
			"setmetatable",
		],
		Kind::Function,
		BuiltinTag::Reflection,
	);
	add(
		reg,
		&["print"],
		Kind::Function,
		BuiltinTag::Console,
	);
	add(
		reg,
		&["pairs", "ipairs", "next"],
		Kind::Function,
		BuiltinTag::Iter,
	);
	add(
		reg,
		&["tonumber"],
		Kind::Function,
		BuiltinTag::Decode,
	);
	add(
		reg,
		&["tostring"],
		Kind::Function,
		BuiltinTag::Encode,
	);
	add(
		reg,
		&["require", "load", "loadfile", "loadstring", "dofile"],
		Kind::Function,
		BuiltinTag::Custom { tag: "module-loader".into() },
	);
	add(
		reg,
		&["collectgarbage"],
		Kind::Function,
		BuiltinTag::Memory,
	);

	// Standard library modules-as-tables.
	add(
		reg,
		&["string", "table", "math", "io", "os", "debug", "coroutine", "package"],
		Kind::Module,
		BuiltinTag::Reflection,
	);

	// Hot members of those modules — covers what people actually call.
	add(
		reg,
		&[
			"table.insert",
			"table.remove",
			"table.concat",
			"table.sort",
			"table.unpack",
			"table.pack",
		],
		Kind::Function,
		BuiltinTag::Iter,
	);
	add(
		reg,
		&[
			"string.format",
			"string.sub",
			"string.gsub",
			"string.match",
			"string.find",
			"string.len",
			"string.rep",
			"string.upper",
			"string.lower",
			"string.byte",
			"string.char",
		],
		Kind::Function,
		BuiltinTag::Format,
	);
	add(
		reg,
		&[
			"math.floor",
			"math.ceil",
			"math.abs",
			"math.min",
			"math.max",
			"math.random",
			"math.sqrt",
			"math.pi",
		],
		Kind::Function,
		BuiltinTag::Math,
	);
	add(
		reg,
		&["io.open", "io.read", "io.write", "io.close", "io.lines"],
		Kind::Function,
		BuiltinTag::FileSystem,
	);
	add(
		reg,
		&["os.time", "os.date", "os.clock", "os.difftime"],
		Kind::Function,
		BuiltinTag::Time,
	);
	add(
		reg,
		&["os.exit", "os.getenv", "os.execute"],
		Kind::Function,
		BuiltinTag::Process,
	);
	add(
		reg,
		&[
			"coroutine.create",
			"coroutine.resume",
			"coroutine.yield",
			"coroutine.wrap",
			"coroutine.status",
		],
		Kind::Function,
		BuiltinTag::Async,
	);
}

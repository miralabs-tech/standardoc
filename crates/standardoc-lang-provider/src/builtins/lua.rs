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
				Language::Lua,
				kind,
				tag.clone(),
				tier,
			));
		}
	};

	// Lua has no Drop / Attribute tier: unlike JS/Rust where wrapper
	// generics (`Array`, `Vec`) and async wrappers (`Promise`, `Future`)
	// are method-call or type-annotation noise, Lua exposes every
	// builtin operation as an explicit global function call resolved
	// through the builtin lookup. Every entry below is Edge — dropping
	// any would blind the graph to a category of observable effects.

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
		BuiltinTier::Edge,
	);
	add(reg, &["print"], Kind::Function, BuiltinTag::Console, BuiltinTier::Edge);
	add(reg, &["pairs", "ipairs", "next"], Kind::Function, BuiltinTag::Iter, BuiltinTier::Edge);
	add(reg, &["tonumber"], Kind::Function, BuiltinTag::Decode, BuiltinTier::Edge);
	add(reg, &["tostring"], Kind::Function, BuiltinTag::Encode, BuiltinTier::Edge);
	add(
		reg,
		&["require", "load", "loadfile", "loadstring", "dofile"],
		Kind::Function,
		BuiltinTag::Custom { tag: "module-loader".into() },
		BuiltinTier::Edge,
	);
	add(reg, &["collectgarbage"], Kind::Function, BuiltinTag::Memory, BuiltinTier::Edge);

	// Standard library modules-as-tables.
	add(
		reg,
		&["string", "table", "math", "io", "os", "debug", "coroutine", "package"],
		Kind::Module,
		BuiltinTag::Reflection,
		BuiltinTier::Edge,
	);

	// Hot members of those modules — covers what people actually call.
	// In Lua all manipulation of tables/strings goes through these
	// explicit globals, so each one is a real observable operation.
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
		BuiltinTier::Edge,
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
		BuiltinTier::Edge,
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
		BuiltinTier::Edge,
	);
	add(
		reg,
		&["io.open", "io.read", "io.write", "io.close", "io.lines"],
		Kind::Function,
		BuiltinTag::FileSystem,
		BuiltinTier::Edge,
	);
	add(
		reg,
		&["os.time", "os.date", "os.clock", "os.difftime"],
		Kind::Function,
		BuiltinTag::Time,
		BuiltinTier::Edge,
	);
	add(
		reg,
		&["os.exit", "os.getenv", "os.execute"],
		Kind::Function,
		BuiltinTag::Process,
		BuiltinTier::Edge,
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
		BuiltinTier::Edge,
	);
}

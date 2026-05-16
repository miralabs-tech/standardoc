use crate::bridge_kind::BridgeKind;
use crate::kinds::{Kind, Language};
use crate::lookup::{BuiltinTag, Substrate};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// How the resolver should treat an identifier hit on this builtin.
///
/// - `Drop`: the builtin is structural noise (wrapper generics like
///   `Vec`, `Array`, `Map`, primitive constants like `undefined`).
///   Resolver returns "no edge", inner type args are still recursed.
/// - `Attribute`: the builtin carries a semantic effect on the
///   *source* symbol (e.g. `Promise`/`Future` → flag the enclosing fn
///   as `async`). No edge in the graph — the tag is folded into the
///   source symbol's attribute set.
/// - `Edge`: the builtin is an observable action / API surface worth
///   tracking as a graph edge (`JSON.parse`, `console.log`, `fetch`,
///   `Math.random`, …). Emits a tagged edge to the synthetic builtin
///   symbol, eagerly seeded in the DB at cold-start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinTier {
	Drop,
	Attribute,
	Edge,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BuiltinEntry {
	pub name: String,
	pub language: Language,
	pub kind: Kind,
	pub tag: BuiltinTag,
	pub tier: BuiltinTier,
	pub synthetic_fqdn: String,
}

impl BuiltinEntry {
	pub fn new(
		name: impl Into<String>,
		language: Language,
		kind: Kind,
		tag: BuiltinTag,
		tier: BuiltinTier,
	) -> Self {
		let name = name.into();
		let synthetic_fqdn = make_synthetic_fqdn(language, &name);
		Self {
			name,
			language,
			kind,
			tag,
			tier,
			synthetic_fqdn,
		}
	}
}

/// One source-language name → target-substrate fqdn binding inside a
/// `SubstrateBridge`. Used for FFI / UST bridges, e.g. `fs.readFileSync`
/// in a JS substrate resolving to a Rust extern crate fn.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BridgeMapping {
	pub source_name: String,
	pub target_fqdn: String,
}

/// A cross-substrat binding catalog. Multiple bridges may exist between
/// the same `(from, to)` pair if they originate from different mechanisms
/// (`uniffi`, `wasm-bindgen`, custom UST FFI, …) — that's what
/// `bridge_kind` disambiguates.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SubstrateBridge {
	pub from: Substrate,
	pub to: Substrate,
	pub bridge_kind: BridgeKind,
	pub mappings: Vec<BridgeMapping>,
}

/// Aggregate of all known builtins, user extensions, and substrate
/// bridges. Single source of truth for resolving identifiers that don't
/// match any local binding or import in a `ModuleLookup`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuiltinRegistry {
	pub by_language: HashMap<Language, Vec<BuiltinEntry>>,
	#[serde(default)]
	pub user_extensions: Vec<BuiltinEntry>,
	#[serde(default)]
	pub bridges: Vec<SubstrateBridge>,
}

impl BuiltinRegistry {
	pub fn new() -> Self {
		Self::default()
	}

	pub fn register(&mut self, entry: BuiltinEntry) {
		self.by_language
			.entry(entry.language)
			.or_default()
			.push(entry);
	}

	pub fn register_user(&mut self, entry: BuiltinEntry) {
		self.user_extensions.push(entry);
	}

	pub fn register_bridge(&mut self, bridge: SubstrateBridge) {
		self.bridges.push(bridge);
	}

	/// Find a builtin matching `name` in `language`. Checks the native
	/// per-language map first, then the UST `user_extensions` filtered by
	/// language.
	pub fn lookup(&self, name: &str, language: Language) -> Option<&BuiltinEntry> {
		self.by_language
			.get(&language)
			.and_then(|entries| entries.iter().find(|e| e.name == name))
			.or_else(|| {
				self.user_extensions
					.iter()
					.find(|e| e.language == language && e.name == name)
			})
	}

	/// Find a bridge mapping for `source_name` from substrate `from` to
	/// substrate `to`. Returns the first match across all bridges
	/// registered between the two substrates.
	pub fn lookup_bridge(
		&self,
		from: &Substrate,
		to: &Substrate,
		source_name: &str,
	) -> Option<&BridgeMapping> {
		self.bridges
			.iter()
			.filter(|b| &b.from == from && &b.to == to)
			.flat_map(|b| b.mappings.iter())
			.find(|m| m.source_name == source_name)
	}
}

/// Build the canonical synthetic fqdn for a builtin: `<builtin>::<lang>::<name>`.
/// The `<builtin>` prefix uses `<` / `>` so it cannot collide with any
/// valid identifier in Rust, TS, JS, Lua, Python, Ruby, Java, C, or C++.
/// `lang` uses the short slug (`rust`, `ts`, `js`, `lua`, …).
pub fn make_synthetic_fqdn(language: Language, canonical_name: &str) -> String {
	format!("<builtin>::{}::{}", language_slug(language), canonical_name)
}

fn language_slug(lang: Language) -> &'static str {
	match lang {
		Language::Rust => "rust",
		Language::TypeScript => "ts",
		Language::JavaScript => "js",
		Language::Lua => "lua",
		Language::Vue => "vue",
		Language::Svelte => "svelte",
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn synthetic_fqdn_scheme() {
		assert_eq!(
			make_synthetic_fqdn(Language::JavaScript, "JSON.parse"),
			"<builtin>::js::JSON.parse"
		);
		assert_eq!(
			make_synthetic_fqdn(Language::Rust, "Vec::new"),
			"<builtin>::rust::Vec::new"
		);
		assert_eq!(
			make_synthetic_fqdn(Language::Lua, "table.insert"),
			"<builtin>::lua::table.insert"
		);
	}

	#[test]
	fn synthetic_fqdn_prefix_cannot_be_valid_identifier() {
		// `<` and `>` are invalid in identifiers across every language we
		// extract — so the synthetic fqdn can never collide with a real
		// user-defined symbol.
		let fqdn = make_synthetic_fqdn(Language::TypeScript, "Promise");
		assert!(fqdn.starts_with("<builtin>::"));
		assert!(fqdn.contains("<"));
		assert!(fqdn.contains(">"));
	}

	#[test]
	fn lookup_native_then_user_extension() {
		let mut reg = BuiltinRegistry::new();
		reg.register(BuiltinEntry::new(
			"JSON.parse",
			Language::JavaScript,
			Kind::Function,
			BuiltinTag::Decode,
			BuiltinTier::Edge,
		));
		reg.register_user(BuiltinEntry::new(
			"myCustomGlobal",
			Language::JavaScript,
			Kind::Value,
			BuiltinTag::Custom {
				tag: "ust:user-defined".into(),
			},
			BuiltinTier::Edge,
		));

		let native = reg
			.lookup("JSON.parse", Language::JavaScript)
			.expect("native builtin present");
		assert_eq!(native.synthetic_fqdn, "<builtin>::js::JSON.parse");

		let user = reg
			.lookup("myCustomGlobal", Language::JavaScript)
			.expect("user extension reachable via lookup");
		assert!(matches!(user.tag, BuiltinTag::Custom { .. }));

		assert!(reg.lookup("nope", Language::JavaScript).is_none());
		assert!(reg.lookup("JSON.parse", Language::Rust).is_none());
	}

	#[test]
	fn lookup_bridge_matches_by_substrate_pair_and_source_name() {
		let mut reg = BuiltinRegistry::new();
		reg.register_bridge(SubstrateBridge {
			from: Substrate::native(Language::JavaScript),
			to: Substrate::native(Language::Rust),
			bridge_kind: BridgeKind::new("napi"),
			mappings: vec![BridgeMapping {
				source_name: "fs.readFileSync".into(),
				target_fqdn: "my_crate::fs::read_file_sync".into(),
			}],
		});

		let hit = reg
			.lookup_bridge(
				&Substrate::native(Language::JavaScript),
				&Substrate::native(Language::Rust),
				"fs.readFileSync",
			)
			.expect("bridge mapping reachable");
		assert_eq!(hit.target_fqdn, "my_crate::fs::read_file_sync");

		assert!(reg
			.lookup_bridge(
				&Substrate::native(Language::Rust),
				&Substrate::native(Language::JavaScript),
				"fs.readFileSync",
			)
			.is_none());
	}
}

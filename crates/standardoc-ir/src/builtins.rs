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

/// Bug E-3 Phase 2: a built-in *method* of a previously-registered
/// builtin type (e.g. `Vec::push`, `Option::unwrap`, `Iterator::map`).
/// Distinct from `BuiltinEntry` because methods always have a receiver
/// type and never participate in the Drop/Attribute/Edge tier system —
/// they're always seeded as synthetic symbols so the receiver-type
/// resolver can land its `<Type>::<method>` lookups on a real
/// `symbols.id`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BuiltinMethodEntry {
    /// Nominal name of the receiver type, e.g. `"Vec"`, `"Option"`.
    /// Matched against `edges.receiver_type` populated by the extractor.
    pub parent_type: String,
    /// Method ident as written at the call site, e.g. `"push"`.
    pub method: String,
    pub language: Language,
    /// Synthetic symbol fqdn, e.g. `"<builtin>::rust::Vec::push"`.
    pub synthetic_fqdn: String,
    /// Bug E-3 Phase 3: nominal return type of the method when known
    /// unambiguously (e.g. `Vec::iter` → `"Iterator"`, `Path::parent`
    /// → `"Option"`). Used by `compute_receiver_type` to propagate
    /// types through chained method calls like
    /// `x.iter().map(...).filter(...)`. `None` for methods whose return
    /// depends on type parameters / inner types (e.g. `Iterator::collect`,
    /// `Option::unwrap`).
    ///
    /// Bug E-3 ext P-E3.2: may be a parametric template (`"Iterator<T>"`)
    /// substituted from the receiver's generic args at lookup time. Token
    /// rules per parent nominal: `T` = args[0]; `E` = args[1] for Result;
    /// `K` / `V` = args[0] / args[1] for `HashMap` / `BTreeMap`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returns: Option<String>,
    /// Bug E-3 ext P-E3.2: template for the closure-argument type when
    /// the method takes a closure. Substituted against the receiver's
    /// generic args using the same token rules as `returns`. E.g.
    /// `Option::map` → `"T"`, `Result::map_err` → `"E"`, `HashMap::retain`
    /// → `"(&K, &mut V)"`. Read by `visit_expr_method_call` to bind the
    /// closure's input ident pat into a per-closure `ClosureScope` frame
    /// so calls inside the closure body emit `receiver_type` edges.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closure_arg_type: Option<String>,
}

impl BuiltinMethodEntry {
    #[must_use]
    pub fn new(
        parent_type: impl Into<String>,
        method: impl Into<String>,
        language: Language,
    ) -> Self {
        let parent_type = parent_type.into();
        let method = method.into();
        let qualified = format!("{parent_type}::{method}");
        let synthetic_fqdn = make_synthetic_fqdn(language, &qualified);
        Self {
            parent_type,
            method,
            language,
            synthetic_fqdn,
            returns: None,
            closure_arg_type: None,
        }
    }

    /// Chainable setter: `BuiltinMethodEntry::new(...).with_returns("Iterator")`.
    /// Drop the call to leave `returns = None` for polymorphic methods.
    #[must_use]
    pub fn with_returns(mut self, returns: impl Into<String>) -> Self {
        self.returns = Some(returns.into());
        self
    }

    /// Bug E-3 ext P-E3.2: chainable setter for the closure-arg template.
    /// `BuiltinMethodEntry::new("Option", "map", ...).with_closure_arg("T")`.
    #[must_use]
    pub fn with_closure_arg(mut self, closure_arg_type: impl Into<String>) -> Self {
        self.closure_arg_type = Some(closure_arg_type.into());
        self
    }
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
    /// Bug E-3 Phase 2: per-language method tables keyed by
    /// `(parent_type, method)`. Always seeded as synthetic symbols at
    /// cold-start so the resolver's `<Type>::<method>` lookup hits.
    #[serde(default)]
    pub methods_by_language: HashMap<Language, Vec<BuiltinMethodEntry>>,
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

    /// Bug E-3 Phase 2: register a builtin method (e.g. `Vec::push`).
    /// Goes into the per-language method table — kept separate from
    /// `by_language` because methods are looked up by
    /// `(parent_type, method)` rather than the bare `name` shared with
    /// type/macro builtins.
    pub fn register_method(&mut self, entry: BuiltinMethodEntry) {
        self.methods_by_language
            .entry(entry.language)
            .or_default()
            .push(entry);
    }

    /// Bug E-3 Phase 2: resolve `<parent_type>.<method>(...)` against the
    /// per-language method table. Returns the matching entry (whose
    /// `synthetic_fqdn` is the seeded symbol FQDN) or `None`.
    #[must_use]
    pub fn lookup_method(
        &self,
        parent_type: &str,
        method: &str,
        language: Language,
    ) -> Option<&BuiltinMethodEntry> {
        self.methods_by_language.get(&language).and_then(|methods| {
            methods
                .iter()
                .find(|m| m.parent_type == parent_type && m.method == method)
        })
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

const fn language_slug(lang: Language) -> &'static str {
    match lang {
        Language::Rust => "rust",
        Language::TypeScript => "ts",
        Language::JavaScript => "js",
        Language::Lua => "lua",
        Language::Vue => "vue",
        Language::Svelte => "svelte",
        Language::C => "c",
    }
}

#[cfg(test)]
mod tests;

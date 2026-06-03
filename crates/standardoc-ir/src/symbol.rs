use serde::{Deserialize, Serialize};

use crate::attribute::RawAttribute;
use crate::hash::Blake3Hash;
use crate::kinds::{DeclKind, EntryPointKind, Kind, Visibility};
use crate::language_kind::LanguageKind;
use crate::location::SymbolLocation;
use crate::signature::{Signature, TypeRef};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RawSymbol {
    pub name: String,
    pub fqdn: String,
    pub kind: Kind,
    pub language_kind: LanguageKind,
    /// Phase 2 K refined declaration kind — populated per language in
    /// K-Step-B+ (Rust/TS/C/Lua). `None` on rows extracted before the
    /// language gained DeclKind coverage, or for languages that have
    /// not been migrated yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decl_kind: Option<DeclKind>,
    /// Phase 2 K-Step-C — when this symbol is a `Method` declared
    /// inside `impl Trait for Type { ... }`, this carries the trait
    /// FQDN. Inherent impl methods and free functions leave this
    /// `None`. The relation is also visible as an `EdgeKind::Implements`
    /// edge from the receiver type to the trait; this field is the
    /// per-method projection of that relationship.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implements_trait: Option<String>,
    /// Phase 2 K-Step-C — for `DeclKind::Method` symbols, the type
    /// that the method dispatches on. Rust: the impl-er (`Bar` for
    /// `impl Foo for Bar { fn baz }`); for trait method definitions
    /// (`trait Foo { fn baz }`) the trait itself, as the receiver is
    /// `Self : Foo`. Free functions leave this `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver_type: Option<TypeRef>,
    /// Phase 3 (Flow) — when set, this symbol is an entry-point
    /// (binary `main`, public crate API, FFI export). Populated per
    /// language. `None` for internal symbols.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_point: Option<EntryPointKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    pub visibility: Visibility,
    pub location: SymbolLocation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<Signature>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_hash: Option<Blake3Hash>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attributes: Vec<RawAttribute>,
    /// Computed semantic flags surfaced post-extraction. Distinct from
    /// `attributes` (which mirror source-level `#[derive(…)]` /
    /// decorators) and from `signature.modifiers` (which mirror
    /// syntactic keywords like `async`, `const`, `unsafe`). Flags here
    /// are *derived* by the resolver / walker from observed semantics —
    /// e.g. Stage 3e-1b stamps `"async"` when a fn returns
    /// `Promise<T>` / `Future<T>` (regardless of an explicit `async`
    /// keyword) and `"iter"` when an `Iterator` / `Generator` trait or
    /// type is touched. UST languages can register their own builtin
    /// tags (`lua:coroutine-yielding`, …) which surface here as
    /// arbitrary strings without an IR schema change.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<String>,
}

#[cfg(test)]
mod tests;

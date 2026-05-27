use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Coarse "this is callable" bucket — `DeclKind::{Function, Method,
    /// Constructor, Getter, Setter}` all live under this `Kind`. Phase 2
    /// K-Step-D renamed from `Function` to clarify the umbrella role.
    Callable,
    Type,
    Value,
    Module,
    Macro,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EdgeKind {
    Calls,
    Imports,
    Extends,
    Implements,
    References,
    UsesType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EdgeConfidence {
    #[default]
    Extracted,
    Inferred,
    Ambiguous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Public,
    Private,
    Crate,
    Protected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceOrigin {
    Workspace,
    CargoRegistry,
    NodeModulesDts,
    ManualExternal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Rust,
    TypeScript,
    JavaScript,
    Lua,
    Vue,
    Svelte,
    C,
}

/// Refined declaration-kind layer paired with the coarse [`Kind`]
/// bucket. Phase 2 K (K-Step-A) foundation — every variant is
/// cross-language enough to map onto multiple frontends; per-language
/// nuance not covered here lives in `language_kind` (string) or the
/// `Custom { lang, tag }` escape hatch below.
///
/// Hierarchy of coverage by [`Kind`]:
///   - `Kind::Module`   → `Module` / `Namespace` / `Crate`
///   - `Kind::Type`     → `Struct` / `Enum` / `Union` / `Class` /
///                        `Interface` (Rust trait collapses here) /
///                        `TypeAlias`
///   - `Kind::Callable` → `Function` / `Method` / `Constructor` /
///                        `Getter` / `Setter`
///   - `Kind::Value`    → `Const` / `Static` / `Var` / `Field` /
///                        `EnumVariant`
///   - `Kind::Macro`    → `DeclarativeMacro` / `ProcMacro` /
///                        `Decorator`
///
/// `Impl` is intentionally absent — Rust `impl` blocks are wrappers,
/// not symbols; their methods carry `DeclKind::Method` directly.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeclKind {
    Module,
    Namespace,
    Crate,
    Struct,
    Enum,
    Union,
    Class,
    Interface,
    TypeAlias,
    Function,
    Method,
    Constructor,
    Getter,
    Setter,
    Const,
    Static,
    Var,
    Field,
    EnumVariant,
    DeclarativeMacro,
    ProcMacro,
    Decorator,
    Custom { lang: Language, tag: String },
}

/// Phase 3 (Flow) — classifies a symbol as an entry-point into the
/// codebase, answering the viz question "where does flow start?".
/// `None` on `RawSymbol.entry_point` means "this is an internal
/// symbol, not an entry-point"; otherwise the variant tells the
/// consumer (viz, agent, MCP) what kind of entry-point this is so it
/// can be highlighted / grouped / used as a flow root.
///
/// Population is per-language (extractor-side) — first-pass coverage
/// is Rust + C; TS / Lua follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryPointKind {
    /// Binary entry-point — Rust `fn main()` in a `bin` crate, C
    /// `int main(int, char**)`. The kernel hands the program here.
    BinaryMain,
    /// Public API surface of a library — a `pub` symbol at the crate
    /// root (or transitively re-exported there) for Rust, a
    /// non-static top-level declaration for C. External consumers
    /// can name this from outside the crate / translation unit.
    PublicApi,
    /// FFI-visible export — Rust `#[no_mangle] pub extern fn …`,
    /// C `luaopen_*` (Lua C-API entrypoint), JNI / NAPI exports. A
    /// foreign runtime (the dynamic linker, a Lua VM, a Node addon
    /// loader) calls in here across the substrate boundary.
    FfiExport,
}

#[cfg(test)]
mod tests;

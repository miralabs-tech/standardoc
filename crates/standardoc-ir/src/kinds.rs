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
mod tests {
    use super::*;

    #[test]
    fn kind_lowercase() {
        assert_eq!(
            serde_json::to_string(&Kind::Callable).unwrap(),
            "\"callable\""
        );
        assert_eq!(serde_json::to_string(&Kind::Macro).unwrap(), "\"macro\"");
    }

    #[test]
    fn edge_kind_screaming() {
        assert_eq!(
            serde_json::to_string(&EdgeKind::Calls).unwrap(),
            "\"CALLS\""
        );
        assert_eq!(
            serde_json::to_string(&EdgeKind::UsesType).unwrap(),
            "\"USES_TYPE\""
        );
    }

    #[test]
    fn visibility_lowercase() {
        assert_eq!(
            serde_json::to_string(&Visibility::Crate).unwrap(),
            "\"crate\""
        );
        assert_eq!(
            serde_json::to_string(&Visibility::Protected).unwrap(),
            "\"protected\""
        );
    }

    #[test]
    fn source_origin_snake() {
        assert_eq!(
            serde_json::to_string(&SourceOrigin::CargoRegistry).unwrap(),
            "\"cargo_registry\""
        );
        assert_eq!(
            serde_json::to_string(&SourceOrigin::NodeModulesDts).unwrap(),
            "\"node_modules_dts\""
        );
        assert_eq!(
            serde_json::to_string(&SourceOrigin::ManualExternal).unwrap(),
            "\"manual_external\""
        );
    }

    #[test]
    fn language_lowercase() {
        assert_eq!(serde_json::to_string(&Language::Rust).unwrap(), "\"rust\"");
        assert_eq!(
            serde_json::to_string(&Language::TypeScript).unwrap(),
            "\"typescript\""
        );
        assert_eq!(
            serde_json::to_string(&Language::JavaScript).unwrap(),
            "\"javascript\""
        );
        assert_eq!(serde_json::to_string(&Language::Lua).unwrap(), "\"lua\"");
        assert_eq!(serde_json::to_string(&Language::Vue).unwrap(), "\"vue\"");
        assert_eq!(
            serde_json::to_string(&Language::Svelte).unwrap(),
            "\"svelte\""
        );
        assert_eq!(serde_json::to_string(&Language::C).unwrap(), "\"c\"");
    }

    #[test]
    fn language_round_trip_all_variants() {
        for lang in [
            Language::Rust,
            Language::TypeScript,
            Language::JavaScript,
            Language::Lua,
            Language::Vue,
            Language::Svelte,
            Language::C,
        ] {
            let s = serde_json::to_string(&lang).unwrap();
            let back: Language = serde_json::from_str(&s).unwrap();
            assert_eq!(lang, back);
        }
    }

    #[test]
    fn round_trip_all_kinds() {
        for kind in [
            Kind::Callable,
            Kind::Type,
            Kind::Value,
            Kind::Module,
            Kind::Macro,
        ] {
            let s = serde_json::to_string(&kind).unwrap();
            let back: Kind = serde_json::from_str(&s).unwrap();
            assert_eq!(kind, back);
        }
    }

    #[test]
    fn round_trip_all_edge_kinds() {
        for kind in [
            EdgeKind::Calls,
            EdgeKind::Imports,
            EdgeKind::Extends,
            EdgeKind::Implements,
            EdgeKind::References,
            EdgeKind::UsesType,
        ] {
            let s = serde_json::to_string(&kind).unwrap();
            let back: EdgeKind = serde_json::from_str(&s).unwrap();
            assert_eq!(kind, back);
        }
    }

    #[test]
    fn decl_kind_snake_case_built_ins() {
        assert_eq!(
            serde_json::to_string(&DeclKind::Function).unwrap(),
            "\"function\""
        );
        assert_eq!(
            serde_json::to_string(&DeclKind::Method).unwrap(),
            "\"method\""
        );
        assert_eq!(
            serde_json::to_string(&DeclKind::DeclarativeMacro).unwrap(),
            "\"declarative_macro\""
        );
        assert_eq!(
            serde_json::to_string(&DeclKind::EnumVariant).unwrap(),
            "\"enum_variant\""
        );
        assert_eq!(
            serde_json::to_string(&DeclKind::TypeAlias).unwrap(),
            "\"type_alias\""
        );
    }

    #[test]
    fn decl_kind_round_trip_built_ins() {
        for kind in [
            DeclKind::Module,
            DeclKind::Namespace,
            DeclKind::Crate,
            DeclKind::Struct,
            DeclKind::Enum,
            DeclKind::Union,
            DeclKind::Class,
            DeclKind::Interface,
            DeclKind::TypeAlias,
            DeclKind::Function,
            DeclKind::Method,
            DeclKind::Constructor,
            DeclKind::Getter,
            DeclKind::Setter,
            DeclKind::Const,
            DeclKind::Static,
            DeclKind::Var,
            DeclKind::Field,
            DeclKind::EnumVariant,
            DeclKind::DeclarativeMacro,
            DeclKind::ProcMacro,
            DeclKind::Decorator,
        ] {
            let s = serde_json::to_string(&kind).unwrap();
            let back: DeclKind = serde_json::from_str(&s).unwrap();
            assert_eq!(kind, back);
        }
    }

    #[test]
    fn decl_kind_custom_round_trip() {
        let dk = DeclKind::Custom {
            lang: Language::Rust,
            tag: "macro_rules_call".into(),
        };
        let s = serde_json::to_string(&dk).unwrap();
        let back: DeclKind = serde_json::from_str(&s).unwrap();
        assert_eq!(dk, back);
    }

    #[test]
    fn entry_point_kind_snake_case() {
        assert_eq!(
            serde_json::to_string(&EntryPointKind::BinaryMain).unwrap(),
            "\"binary_main\""
        );
        assert_eq!(
            serde_json::to_string(&EntryPointKind::PublicApi).unwrap(),
            "\"public_api\""
        );
        assert_eq!(
            serde_json::to_string(&EntryPointKind::FfiExport).unwrap(),
            "\"ffi_export\""
        );
    }

    #[test]
    fn entry_point_kind_round_trip_all() {
        for k in [
            EntryPointKind::BinaryMain,
            EntryPointKind::PublicApi,
            EntryPointKind::FfiExport,
        ] {
            let s = serde_json::to_string(&k).unwrap();
            let back: EntryPointKind = serde_json::from_str(&s).unwrap();
            assert_eq!(k, back);
        }
    }
}

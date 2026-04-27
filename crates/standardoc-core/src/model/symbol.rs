use crate::model::reference::References;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Trait,
    Interface,
    TypeAlias,
    Const,
    Static,
    Module,
    Macro,
    Field,
    Variant,
    /// Default category when language yields an unclassified symbol.
    #[default]
    Other,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Visibility {
    Public,
    Private,
    /// Rust-style `pub(crate)`.
    Crate,
    /// Package-private (Java / Go-ish).
    Internal,
    /// Inherits the surrounding context's default visibility.
    #[default]
    Inherited,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParamInfo {
    pub name: String,
    pub type_repr: Option<String>,
    pub default: Option<String>,
    #[serde(default)]
    pub is_optional: bool,
    #[serde(default)]
    pub is_variadic: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TypeInfo {
    pub repr: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolInfo {
    pub kind: SymbolKind,
    pub visibility: Visibility,
    /// Canonical one-line signature reformatted by the language provider.
    pub signature: String,
    #[serde(default)]
    pub params: Vec<ParamInfo>,
    pub returns: Option<TypeInfo>,
    #[serde(default)]
    pub generics: Vec<String>,
    /// Attributes / decorators / annotations attached to the symbol
    /// (e.g. `#[deprecated]`, `@staticmethod`).
    #[serde(default)]
    pub decorators: Vec<String>,
    #[serde(default)]
    pub is_async: bool,
    /// True when the language marks the symbol deprecated via an attribute,
    /// not via an `@deprecated` tag.
    #[serde(default)]
    pub is_deprecated: bool,
    /// Outgoing references — what other symbols this one points at (param
    /// types, return types, field types, trait implementations, calls).
    /// Default-empty for declarative symbols. The reverse index lives in
    /// `IndexState` and is rebuilt from these.
    #[serde(default)]
    pub references: References,
}

use serde::{Deserialize, Serialize};

/// Edge type between one symbol and another.
///
/// Permet aux tools cross-ref (`find_usages`, `find_implementations`,
/// `search_by_return_type`, ...) to filter by semantics instead of returning
/// everything indiscriminately. Variants are intentionally broad: we prefer
/// over-classification over missing references in Phase 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RefKind {
    /// This function calls `target` in its body.
    Call,
    /// `target` appears as a parameter type.
    ParamType,
    /// `target` is this function's return type.
    ReturnType,
    /// `target` is the type of a field (struct/class) or enum variant.
    FieldType,
    /// Ce symbole `impl target for Self` (Rust) ou `class X implements target` (TS).
    Implements,
    /// This symbol extends / inherits from `target`.
    Extends,
    /// `target` appears as generic parameter: `Vec<target>`, `Foo<T = target>`.
    GenericArg,
    /// Unclassified reference (fallback when uncertain).
    Other,
}

/// Outgoing edge: this symbol references another at this location with this kind.
///
/// `target` is a **textual name** as it appears in source — not a resolved
/// `DocKey`. Resolution happens in reverse index by short-name (label) match.
/// Ambiguous names (multiple symbols with same short name) return all usages;
/// the agent can filter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolRef {
    pub kind: RefKind,
    pub target: String,
    pub line: u32,
}

/// All outgoing references from a symbol. Empty for purely declarative symbols
/// (constants, simple `pub mod`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct References {
    pub outgoing: Vec<SymbolRef>,
}

impl References {
    pub const fn empty() -> Self {
        Self {
            outgoing: Vec::new(),
        }
    }

    pub fn push(&mut self, kind: RefKind, target: impl Into<String>, line: u32) {
        self.outgoing.push(SymbolRef {
            kind,
            target: target.into(),
            line,
        });
    }
}

/// Incoming edge rebuilt by reverse index. Worker populates it from each
/// symbol's `outgoing` references.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingRef {
    /// Key of the referencing symbol.
    pub from_key: String,
    pub kind: RefKind,
    pub line: u32,
}

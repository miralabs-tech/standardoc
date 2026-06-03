//! Sourcemap protocol types for the preproc<->extractor contract.
//!
//! These structs match the on-wire JSON shape described in `SOURCEMAP_v1.md`.
//! Producers emit `@stdoc:<tag> <json>` Lua comments; consumers parse the JSON
//! payload via `serde_json::from_str::<...>(json)`. Each annotation carries a
//! `v: u32` version field — consumers should reject payloads with
//! `v > supported`, log + skip on parse errors, and tolerate unknown fields.
//!
//! The crate is intentionally minimal (serde derives only) so that both the
//! `standarlua` preproc (producer) and the `standardoc-core` extractor
//! (consumer) can depend on it without dragging in IR or storage code.

use serde::{Deserialize, Serialize};

// --- Match annotations -----------------------------------------------------

/// `@stdoc:match-begin` — emitted before the lowered scrutinee assignment of
/// a `match ... with ... end` cluster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchBeginAnnotation {
    pub v: u32,
    pub mid: String,
    pub scrut: String,
    pub arm_count: u32,
}

/// `@stdoc:match-arm` — emitted before each lowered arm body of a match
/// cluster. `idx` is the 0-based arm index within the cluster.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchArmAnnotation {
    pub v: u32,
    pub mid: String,
    pub idx: u32,
    pub pattern: PatternNode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard: Option<String>,
}

/// `@stdoc:match-end` — emitted after the closing `end` of a match cluster.
/// `result` is the synthesized local name aggregating the arm results, or
/// `None` for the statement form (no result var).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchEndAnnotation {
    pub v: u32,
    pub mid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

// --- Safe-nav annotations --------------------------------------------------

/// `@stdoc:safe-nav` — emitted alongside the desugared wrapper of a `?.` /
/// `?:` operator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafeNavAnnotation {
    pub v: u32,
    pub source: String,
    pub target: String,
    pub op: SafeNavOp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SafeNavOp {
    /// `?.` member access.
    Member,
    /// `?:` method call.
    Call,
}

// --- Compound-op annotations -----------------------------------------------

/// `@stdoc:compound-op` — emitted alongside the desugared plain assignment of
/// a compound operator (`+=`, `-=`, `..=`, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompoundOpAnnotation {
    pub v: u32,
    pub op: String,
    pub lhs: String,
    pub rhs: String,
}

// --- Type-strip annotations ------------------------------------------------

/// `@stdoc:type-strip` — emitted whenever a type annotation is removed from
/// runtime Lua. Carries the structured type info for the extractor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeStripAnnotation {
    pub v: u32,
    pub ident: String,
    pub ty: TypeNode,
    pub site: TypeStripSite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TypeStripSite {
    /// `local x: T = ...`
    Local,
    /// `function f(x: T)`
    Param,
    /// `function f(): T` — the synthetic ident `_return` carries this.
    Return,
    /// `type X = { f: T }`
    Field,
}

// --- Type declaration annotations ------------------------------------------

/// `@stdoc:type-decl` — emitted for every `type X = ...` declaration so the
/// extractor can populate a per-module type table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeDeclAnnotation {
    pub v: u32,
    pub name: String,
    pub ty: TypeNode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub generics: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<SourceSpan>,
}

/// Span carried by `type-decl` annotations when the producer can attach
/// source location info. Line numbers are 1-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSpan {
    pub start_line: u32,
    pub end_line: u32,
}

// --- Structural type tree --------------------------------------------------

/// Structural type tree used by both `TypeStripAnnotation` and
/// `TypeDeclAnnotation`. Recursive; designed for compact wire representation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TypeNode {
    /// Primitives: `"number"`, `"string"`, `"boolean"`, `"nil"`, `"any"`.
    Primitive { name: String },
    /// `T?` shorthand for `T | nil`.
    Optional { inner: Box<Self> },
    /// Heterogeneous union: `T | U`.
    Union { items: Vec<Self> },
    /// Literal-only union: `200 | 201 | 202`. Distinct from `Union` because
    /// the v2 narrowing checker treats this specially (fast-path
    /// exhaustiveness).
    #[serde(rename = "literalunion")]
    LiteralUnion { values: Vec<serde_json::Value> },
    /// Record / object: `{ x: number, y: number }`.
    Record { fields: Vec<TypeField> },
    /// Map: `{ [K]: V }`.
    Map { key: Box<Self>, value: Box<Self> },
    /// Array: `{T}`.
    Array { inner: Box<Self> },
    /// Function: `fun(P1, P2): R`. `generics` is reserved for inline generic
    /// declarations (`fun<T>(T): T`); v1 preproc may emit empty.
    Function {
        params: Vec<Self>,
        returns: Box<Self>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        generics: Vec<String>,
    },
    /// Named reference, optionally with generic args.
    Named {
        name: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<Self>,
    },
    /// `any` escape hatch.
    Any,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeField {
    pub key: String,
    pub ty: TypeNode,
    /// `f?: T` syntax — VALUE is optional but key is required. Distinct
    /// from `T?` (the value itself is `T | nil`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
}

// --- Pattern tree ----------------------------------------------------------

/// Structured match pattern carried by `MatchArmAnnotation.pattern`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum PatternNode {
    /// Literal: numbers, strings, booleans, `nil` (serialized as JSON `null`).
    Literal { value: serde_json::Value },
    /// Inclusive numeric range `lo..=hi`. `inclusive` is forward-compat for
    /// a future exclusive variant; v1 always emits `true`.
    Range { lo: f64, hi: f64, inclusive: bool },
    /// Type-only check (no binding). Used inside other constructs or for
    /// arms that only need the type-check without capturing the value.
    Type { name: String },
    /// Capture binding. Optional `ty` is the inline type annotation
    /// (`n: number`); `None` is a bare capture (any value).
    Bind {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ty: Option<String>,
    },
    /// Alternation: `100 | 101 | 102`. Or-pattern alternatives MUST bind the
    /// same set of names (producer enforces).
    Or { items: Vec<Self> },
    /// Table destructure: `{ code = c, msg = m }`.
    Table { fields: Vec<TableField> },
    /// Tuple destructure for multi-arg match: `(a, b)` in
    /// `match (x, y) with`. Lowered as multi-local destructure.
    Tuple { items: Vec<Self> },
    /// As-binding: `pat @ name`. Inner pattern matches AND the whole
    /// matched value gets bound to `name`.
    As { name: String, inner: Box<Self> },
    /// `_` wildcard. Matches anything, binds nothing.
    Wildcard,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableField {
    pub key: String,
    pub pattern: PatternNode,
}

#[cfg(test)]
mod tests;

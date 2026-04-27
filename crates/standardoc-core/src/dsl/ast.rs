//! AST types for the Standardoc DSL.
//!
//! A parsed template is a sequence of `Node`s: literal text, references to
//! indexed blocks, control-flow blocks (`each`, `if`), and alias references
//! (used inside `each` bodies).

use crate::model::DocKey;

/// A parsed template — the output of the parser, input of the evaluator.
#[derive(Debug, Clone)]
pub struct Template {
    pub nodes: Vec<Node>,
}

#[derive(Debug, Clone)]
pub enum Node {
    /// Literal markdown text between expressions.
    Text(String),
    /// `{{ @doc.KEY:... }}`
    Reference(Reference),
    /// `{{ alias.field }}` inside an `each` body.
    Alias(AliasRef),
    /// `{{ each <alias> in <source> }} ... {{ /each }}`.
    ///
    /// `source` can be:
    /// - a reference to a tag of a block (`@doc.K:param`) -> alias binds
    ///   chaque occurrence de tag (`TagOccurrence`)
    /// - une query inter-blocks (`@docs.module(K)`, `@docs.all`) → l'alias
    ///   each matched `DocBlock` (`Block`)
    Each {
        alias: String,
        collection: EachSource,
        body: Vec<Node>,
    },
    /// `{{ if <condition> }} ... [{{ else }} ...] {{ /if }}`
    If {
        condition: Condition,
        then_body: Vec<Node>,
        else_body: Option<Vec<Node>>,
    },
}

#[derive(Debug, Clone)]
pub struct Reference {
    pub key: DocKey,
    pub access: Access,
}

/// Where do elements iterated by `each` come from?
#[derive(Debug, Clone)]
pub enum EachSource {
    /// `each p in @doc.K:param` — iteration over tag occurrences from one
    /// block. Alias `p` is bound to `TagFields`.
    Tag(Reference),
    /// `each f in @docs.module(K)` or `@docs.all` — iteration over full blocks.
    /// Alias `f` is bound to a `DocBlock`.
    Blocks(BlockQuery),
}

/// How to select a subset of blocks in the index.
#[derive(Debug, Clone)]
pub enum BlockQuery {
    /// `@docs.module(KEY)` — anchor at `KEY` plus every block whose key
    /// starts with `KEY.` (dot-children) or `KEY::` (satellites). Strict
    /// segment boundary: `module(api.user)` does NOT match `api.users`.
    Module(DocKey),
    /// `@docs.all` — all blocks in the index. Exhaustive iteration,
    /// useful for global index pages.
    All,
    /// `@docs.satellites(KEY)` — only the satellites under `KEY`
    /// (`KEY::*`), excluding the anchor at `KEY` and any dot-children.
    Satellites(DocKey),
}

#[derive(Debug, Clone)]
pub enum Access {
    /// `@doc.KEY` — resolves to the block's `label` (sane default for bare refs).
    Bare,
    /// `@doc.KEY:label` / `@doc.KEY:origin` / etc. — a top-level block field.
    /// Sub-paths for `meta` / `symbol` land here too (e.g. `["meta", "path"]`).
    Field(Vec<String>),
    /// `@doc.KEY:TAG` — joined fields of the (single) occurrence.
    /// Error if tag is `cardinality: Multi` (use `each`, `[n]`,
    /// `first(t)` ou `last(t)`).
    Tag(String),
    /// `@doc.KEY:TAG[i]` — the indexed occurrence, joined.
    TagIndex { tag: String, index: usize },
    /// `@doc.KEY:TAG[i].FIELD` — one named field of one occurrence.
    TagField {
        tag: String,
        index: usize,
        field: String,
    },
    /// `@doc.KEY:TAG.FIELD` — implicit shortcut to first occurrence.
    /// Valid on `cardinality: Single` (single `@returns`, `@description`,
    /// etc.). Erreur sur Multi parce que c'est ambigu.
    TagShortcut { tag: String, field: String },
    /// `@doc.KEY:has(TAG)` / `:count(TAG)` / `:first(TAG)` / `:last(TAG)`,
    /// with optional `.field` for `first` / `last`:
    /// `:first(param).name`, `:last(see).target`. On `has` / `count`, the
    /// `.field` is an error (they return scalars).
    Func {
        name: FuncName,
        tag: String,
        field: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FuncName {
    Has,
    Count,
    First,
    Last,
}

/// Alias reference used inside an `each` body — `{{ p }}`, `{{ p.name }}`,
/// `{{ f.has(example) }}`, `{{ f.first(example).content }}`, etc.
///
/// For **tag aliases** (each over `:tag`), only `Bare` and `Path([field])`
/// are valid — the rest is rejected at evaluation.
/// For **block aliases** (each over `@docs.…`), all variants are supported
/// and mirror the same semantics as `Access` on
/// `@doc.K`.
#[derive(Debug, Clone)]
pub struct AliasRef {
    pub alias: String,
    pub access: AliasAccess,
}

#[derive(Debug, Clone)]
pub enum AliasAccess {
    /// `{{ f }}` — default projection (block) or joined fields (tag).
    Bare,
    /// `{{ f.X.Y }}` — dotted path (block field, meta, symbol, tag,
    /// tag.field, or tag-alias schema field depending on binding).
    Path(Vec<String>),
    /// `{{ f.has(t) }}` / `{{ f.count(t) }}` / `{{ f.first(t) }}` /
    /// `{{ f.last(t).field }}` — only valid on block aliases.
    Func {
        name: FuncName,
        tag: String,
        field: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub enum Condition {
    /// Truthy if the target resolves to a non-empty / non-zero value.
    Truthy(CondTarget),
    /// `<target> <op> <literal>` — typically `:count(TAG) > 0` ou
    /// `f.symbol.kind == "function"`.
    Compare {
        left: CondTarget,
        op: CompareOp,
        right: Literal,
    },
}

/// Left-hand side of `if … [op LITERAL]`: either direct reference or alias
/// (used inside `each`).
#[derive(Debug, Clone)]
pub enum CondTarget {
    Ref(Reference),
    Alias(AliasRef),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    Eq,
    Ne,
    Gt,
    Lt,
    Gte,
    Lte,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Literal {
    Int(i64),
    String(String),
    Bool(bool),
}

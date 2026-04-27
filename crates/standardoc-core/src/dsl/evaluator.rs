//! Evaluates a parsed `Template` against a `DocBlock` source.
//!
//! The evaluator walks the AST, resolves references by looking up blocks in a
//! caller-provided `BlockSource`, and produces the rendered string. Missing
//! references raise `EvalError` — callers guard with `{{ if ...:has(x) }}`.

use crate::config::{TagCardinality, TagSchema};
use crate::dsl::ast::{
    Access, AliasAccess, AliasRef, BlockQuery, CompareOp, CondTarget, Condition, EachSource,
    FuncName, Literal, Node, Reference, Template,
};
use crate::dsl::schema;
use crate::model::{DocBlock, DocKey, TagFields};
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EvalError {
    #[error("unknown block key '{0}'")]
    UnknownKey(String),

    #[error("unknown block field '{field}' on key '{key}'")]
    UnknownField { key: String, field: String },

    #[error("tag '{tag}' not found on key '{key}'")]
    UnknownTag { key: String, tag: String },

    #[error("tag index out of bounds: {tag}[{index}] (size={size}) on key '{key}'")]
    IndexOutOfBounds {
        key: String,
        tag: String,
        index: usize,
        size: usize,
    },

    #[error("unknown field '{field}' on tag '{tag}' (no schema or field absent)")]
    UnknownTagField { tag: String, field: String },

    #[error("undefined alias '{0}'")]
    UndefinedAlias(String),

    #[error("type error: {0}")]
    Type(String),

    /// DSL `:tag` or `:tag.field` was used on a tag declared as
    /// `cardinality: Multi`. For this kind of tag (`param`, `example`, ...),
    /// the user must disambiguate explicitly.
    #[error(
        "ambiguous access '{access}' on '{key}': '{tag}' is multi-occurrence — \
         use '{tag}[N]', 'first({tag})', 'last({tag})', or 'each x in @doc.{key}:{tag}'"
    )]
    AmbiguousAccess {
        key: String,
        tag: String,
        access: String,
    },
}

/// Trait implemented by anything that can hand out a `DocBlock` by key —
/// typically the `Index`, but any map works in tests.
///
/// `keys()` returns the full set of available keys, **sorted**, so
/// `each f in @docs.module(X)` iteration is stable run-to-run.
/// Default `BTreeMap` impls provide this ordering naturally.
pub trait BlockSource {
    fn get(&self, key: &DocKey) -> Option<DocBlock>;
    fn keys(&self) -> Vec<DocKey>;
}

impl BlockSource for BTreeMap<DocKey, DocBlock> {
    fn get(&self, key: &DocKey) -> Option<DocBlock> {
        self.get(key).cloned()
    }
    fn keys(&self) -> Vec<DocKey> {
        self.keys().cloned().collect()
    }
}

impl BlockSource for BTreeMap<String, DocBlock> {
    fn get(&self, key: &DocKey) -> Option<DocBlock> {
        self.get(key.as_str()).cloned()
    }
    fn keys(&self) -> Vec<DocKey> {
        self.keys().map(|k| DocKey::new(k.clone())).collect()
    }
}

pub struct Evaluator<'a, S: BlockSource> {
    source: &'a S,
    schemas: &'a BTreeMap<String, TagSchema>,
}

impl<'a, S: BlockSource> Evaluator<'a, S> {
    pub const fn new(source: &'a S, schemas: &'a BTreeMap<String, TagSchema>) -> Self {
        Self { source, schemas }
    }

    pub fn render(&self, template: &Template) -> Result<String, EvalError> {
        let mut out = String::new();
        let mut scope = Scope::default();
        self.render_nodes(&template.nodes, &mut scope, &mut out)?;
        Ok(out)
    }

    fn render_nodes(
        &self,
        nodes: &[Node],
        scope: &mut Scope,
        out: &mut String,
    ) -> Result<(), EvalError> {
        for node in nodes {
            self.render_node(node, scope, out)?;
        }
        Ok(())
    }

    fn render_node(
        &self,
        node: &Node,
        scope: &mut Scope,
        out: &mut String,
    ) -> Result<(), EvalError> {
        match node {
            Node::Text(t) => out.push_str(t),
            Node::Reference(r) => {
                let rendered = self.render_reference(r)?;
                out.push_str(&rendered);
            }
            Node::Alias(a) => {
                let rendered = self.render_alias(a, scope)?;
                out.push_str(&rendered);
            }
            Node::Each {
                alias,
                collection,
                body,
            } => self.render_each(alias, collection, body, scope, out)?,
            Node::If {
                condition,
                then_body,
                else_body,
            } => {
                let truthy = self.evaluate_condition(condition, scope)?;
                if truthy {
                    self.render_nodes(then_body, scope, out)?;
                } else if let Some(eb) = else_body {
                    self.render_nodes(eb, scope, out)?;
                }
            }
        }
        Ok(())
    }

    fn render_each(
        &self,
        alias: &str,
        collection: &EachSource,
        body: &[Node],
        scope: &mut Scope,
        out: &mut String,
    ) -> Result<(), EvalError> {
        match collection {
            EachSource::Tag(reference) => {
                let block = self.fetch_block(&reference.key)?;
                let tag = tag_name_from_access(&reference.access).ok_or_else(|| {
                    EvalError::Type(
                        "'each' collection must point to a tag, e.g. '@doc.foo:param'".into(),
                    )
                })?;
                if let Some(occurrences) = block.tags.get(&tag) {
                    for fields in occurrences {
                        scope.push_tag(alias.to_owned(), tag.clone(), fields.clone());
                        self.render_nodes(body, scope, out)?;
                        scope.pop();
                    }
                }
            }
            EachSource::Blocks(query) => {
                let blocks = self.collect_blocks(query);
                for block in blocks {
                    scope.push_block(alias.to_owned(), block);
                    self.render_nodes(body, scope, out)?;
                    scope.pop();
                }
            }
        }
        Ok(())
    }

    /// Enumerate blocks matched by a `BlockQuery`, with stable order
    /// (`BlockSource` returns keys sorted).
    fn collect_blocks(&self, query: &BlockQuery) -> Vec<DocBlock> {
        let keys = self.source.keys();
        keys.into_iter()
            .filter(|k| match query {
                BlockQuery::All => true,
                BlockQuery::Module(prefix) => is_module_member(k.as_str(), prefix.as_str()),
                BlockQuery::Satellites(prefix) => is_satellite_of(k.as_str(), prefix.as_str()),
            })
            .filter_map(|k| self.source.get(&k))
            .collect()
    }

    fn fetch_block(&self, key: &DocKey) -> Result<DocBlock, EvalError> {
        self.source
            .get(key)
            .ok_or_else(|| EvalError::UnknownKey(key.as_str().to_owned()))
    }

    fn render_reference(&self, r: &Reference) -> Result<String, EvalError> {
        let block = self.fetch_block(&r.key)?;
        match &r.access {
            Access::Bare => Ok(render_default_projection(&block)),
            Access::Field(path) => resolve_field(&block, path),
            Access::Tag(tag) => self.render_tag_bare(&block, tag),
            Access::TagIndex { tag, index } => Self::render_tag_index(&block, tag, *index),
            Access::TagField { tag, index, field } => {
                self.render_tag_field(&block, tag, *index, field)
            }
            Access::TagShortcut { tag, field } => {
                self.render_tag_shortcut_field(&block, tag, field)
            }
            Access::Func { name, tag, field } => {
                self.evaluate_func(*name, &block, tag, field.as_deref())
            }
        }
    }

    fn evaluate_func(
        &self,
        name: FuncName,
        block: &DocBlock,
        tag: &str,
        field: Option<&str>,
    ) -> Result<String, EvalError> {
        match (name, field) {
            // has/count + chaining = error (scalars are not projectable)
            (FuncName::Has | FuncName::Count, Some(f)) => Err(EvalError::Type(format!(
                "'{name:?}({tag}).{f}' is invalid — has/count return a scalar, not an occurrence"
            ))),
            // Without field: existing behavior (joined fields or scalar)
            (_, None) => Ok(evaluate_func_scalar(name, block, tag)),
            // first/last with field -> resolved through schema
            (FuncName::First | FuncName::Last, Some(field)) => {
                let occurrences = block.tags.get(tag);
                let Some(occ) = occurrences.and_then(|v| match name {
                    FuncName::First => v.first(),
                    FuncName::Last => v.last(),
                    _ => None,
                }) else {
                    // Missing tag or empty occurrence -> error consistent with :tag[N].field
                    return Err(EvalError::UnknownTag {
                        key: block.key.as_str().to_owned(),
                        tag: tag.to_owned(),
                    });
                };
                schema::field_value(self.schemas, tag, field, occ)
                    .cloned()
                    .ok_or_else(|| EvalError::UnknownTagField {
                        tag: tag.to_owned(),
                        field: field.to_owned(),
                    })
            }
        }
    }

    /// Standalone `:tag` — reject Multi (ambiguous), accept Single.
    fn render_tag_bare(&self, block: &DocBlock, tag: &str) -> Result<String, EvalError> {
        if self.tag_cardinality(tag) == TagCardinality::Multi {
            return Err(EvalError::AmbiguousAccess {
                key: block.key.as_str().to_owned(),
                tag: tag.to_owned(),
                access: tag.to_owned(),
            });
        }
        let occurrences = tag_occurrences(block, tag)?;
        let fields = &occurrences[0];
        if fields.is_empty() {
            return Ok(String::new());
        }
        if fields.len() == 1 {
            return Ok(fields[0].clone());
        }
        Ok(fields.join(" "))
    }

    /// `:tag.field` shortcut — reject Multi, accept Single.
    fn render_tag_shortcut_field(
        &self,
        block: &DocBlock,
        tag: &str,
        field: &str,
    ) -> Result<String, EvalError> {
        if self.tag_cardinality(tag) == TagCardinality::Multi {
            return Err(EvalError::AmbiguousAccess {
                key: block.key.as_str().to_owned(),
                tag: tag.to_owned(),
                access: format!("{tag}.{field}"),
            });
        }
        self.render_tag_field(block, tag, 0, field)
    }

    fn tag_cardinality(&self, tag: &str) -> TagCardinality {
        self.schemas
            .get(tag)
            .map_or(TagCardinality::Single, |s| s.cardinality)
    }

    fn render_tag_index(block: &DocBlock, tag: &str, index: usize) -> Result<String, EvalError> {
        let occurrences = tag_occurrences(block, tag)?;
        if index >= occurrences.len() {
            return Err(EvalError::IndexOutOfBounds {
                key: block.key.as_str().to_owned(),
                tag: tag.to_owned(),
                index,
                size: occurrences.len(),
            });
        }
        Ok(occurrences[index].join(" "))
    }

    fn render_tag_field(
        &self,
        block: &DocBlock,
        tag: &str,
        index: usize,
        field: &str,
    ) -> Result<String, EvalError> {
        let occurrences = tag_occurrences(block, tag)?;
        if index >= occurrences.len() {
            return Err(EvalError::IndexOutOfBounds {
                key: block.key.as_str().to_owned(),
                tag: tag.to_owned(),
                index,
                size: occurrences.len(),
            });
        }
        let fields = &occurrences[index];
        schema::field_value(self.schemas, tag, field, fields)
            .cloned()
            .ok_or_else(|| EvalError::UnknownTagField {
                tag: tag.to_owned(),
                field: field.to_owned(),
            })
    }

    fn render_alias(&self, a: &AliasRef, scope: &Scope) -> Result<String, EvalError> {
        let entry = scope
            .lookup(&a.alias)
            .ok_or_else(|| EvalError::UndefinedAlias(a.alias.clone()))?;
        match &entry.binding {
            ScopeBinding::TagOccurrence { tag, fields } => {
                self.render_tag_alias(&a.alias, &a.access, tag, fields)
            }
            ScopeBinding::Block(block) => self.render_block_alias(&a.access, block),
        }
    }

    fn render_tag_alias(
        &self,
        alias_name: &str,
        access: &AliasAccess,
        tag: &str,
        fields: &TagFields,
    ) -> Result<String, EvalError> {
        match access {
            AliasAccess::Bare => Ok(fields.join(" ")),
            AliasAccess::Path(path) if path.len() == 1 => {
                let field = &path[0];
                schema::field_value(self.schemas, tag, field, fields)
                    .cloned()
                    .ok_or_else(|| EvalError::UnknownTagField {
                        tag: tag.to_owned(),
                        field: field.clone(),
                    })
            }
            AliasAccess::Path(_) => Err(EvalError::Type(format!(
                "alias '{alias_name}' (tag occurrence) does not support nested paths"
            ))),
            AliasAccess::Func { .. } => Err(EvalError::Type(format!(
                "alias '{alias_name}' is bound to a tag occurrence — has()/count()/first()/last() apply to blocks, not tag occurrences"
            ))),
        }
    }

    /// Resolve `{{ f.X… }}` when `f` is bound to a block. Delegates to the
    /// same machinery as direct `@doc.K:X` via an `AliasAccess -> Access`
    /// translation. This keeps block aliases symmetric with block refs.
    fn render_block_alias(
        &self,
        access: &AliasAccess,
        block: &DocBlock,
    ) -> Result<String, EvalError> {
        match access {
            AliasAccess::Bare => Ok(render_default_projection(block)),
            AliasAccess::Path(path) => match block_alias_access(path)? {
                Access::Bare => Ok(render_default_projection(block)),
                Access::Field(p) => resolve_field(block, &p),
                Access::Tag(tag) => self.render_tag_bare(block, &tag),
                Access::TagShortcut { tag, field } => {
                    self.render_tag_shortcut_field(block, &tag, &field)
                }
                _ => Err(EvalError::Type(
                    "block alias path produced an unexpected access kind".into(),
                )),
            },
            AliasAccess::Func { name, tag, field } => {
                self.evaluate_func(*name, block, tag, field.as_deref())
            }
        }
    }

    fn evaluate_condition(&self, cond: &Condition, scope: &Scope) -> Result<bool, EvalError> {
        match cond {
            Condition::Truthy(target) => self.evaluate_truthy_target(target, scope),
            Condition::Compare { left, op, right } => {
                let left_val = self.compare_lhs(left, scope)?;
                let cmp = match (left_val, right) {
                    (CompareLhs::Int(n), Literal::Int(m)) => compare_int(*op, n, *m),
                    (CompareLhs::Int(n), Literal::Bool(true)) => n != 0,
                    (CompareLhs::Int(n), Literal::Bool(false)) => n == 0,
                    (CompareLhs::String(s), Literal::String(t)) => compare_string(*op, &s, t),
                    (CompareLhs::Bool(b), Literal::Bool(t)) => compare_bool(*op, b, *t),
                    (lhs, lit) => {
                        return Err(EvalError::Type(format!(
                            "cannot compare {lhs:?} with literal {lit:?}"
                        )));
                    }
                };
                Ok(cmp)
            }
        }
    }

    fn evaluate_truthy_target(
        &self,
        target: &CondTarget,
        scope: &Scope,
    ) -> Result<bool, EvalError> {
        match target {
            CondTarget::Ref(r) => self.evaluate_truthy_ref(r),
            CondTarget::Alias(a) => self.evaluate_truthy_alias(a, scope),
        }
    }

    fn evaluate_truthy_ref(&self, r: &Reference) -> Result<bool, EvalError> {
        let block = self.fetch_block(&r.key)?;
        if let Access::Func { name, tag, field } = &r.access {
            // For first/last with field, treat like a normal ref
            // (truthy = non-empty field).
            if field.is_some() {
                return match self.evaluate_func(*name, &block, tag, field.as_deref()) {
                    Ok(s) => Ok(!s.trim().is_empty()),
                    Err(EvalError::UnknownTag { .. } | EvalError::UnknownTagField { .. }) => {
                        Ok(false)
                    }
                    Err(e) => Err(e),
                };
            }
            let val = evaluate_func_scalar(*name, &block, tag);
            return Ok(!val.is_empty() && val != "0" && val != "false");
        }
        // For `if @doc.x:tag` or `if @doc.x:tag.field`, "missing value"
        // should yield `false`, not an error. Otherwise users are forced to
        // write `if :has(tag)` everywhere, which is verbose and redundant.
        // Note: we intentionally do NOT swallow `AmbiguousAccess` because it is
        // a real DSL error that must be fixed by the user.
        match self.render_reference(r) {
            Ok(rendered) => Ok(!rendered.trim().is_empty()),
            Err(
                EvalError::UnknownTag { .. }
                | EvalError::IndexOutOfBounds { .. }
                | EvalError::UnknownTagField { .. }
                | EvalError::UnknownField { .. },
            ) => Ok(false),
            Err(other) => Err(other),
        }
    }

    fn evaluate_truthy_alias(&self, a: &AliasRef, scope: &Scope) -> Result<bool, EvalError> {
        // Special case `if alias.has(tag)` or `if alias.count(tag)` on a block:
        // dispatch to scalar to keep the "0/false/empty" rule.
        if let AliasAccess::Func {
            name: FuncName::Has | FuncName::Count,
            tag,
            field: None,
        } = &a.access
        {
            let entry = scope
                .lookup(&a.alias)
                .ok_or_else(|| EvalError::UndefinedAlias(a.alias.clone()))?;
            if let ScopeBinding::Block(block) = &entry.binding {
                let val = evaluate_func_scalar(
                    if matches!(
                        a.access,
                        AliasAccess::Func {
                            name: FuncName::Has,
                            ..
                        }
                    ) {
                        FuncName::Has
                    } else {
                        FuncName::Count
                    },
                    block,
                    tag,
                );
                return Ok(!val.is_empty() && val != "0" && val != "false");
            }
        }
        match self.render_alias(a, scope) {
            Ok(rendered) => Ok(!rendered.trim().is_empty()),
            Err(
                EvalError::UnknownTag { .. }
                | EvalError::IndexOutOfBounds { .. }
                | EvalError::UnknownTagField { .. }
                | EvalError::UnknownField { .. },
            ) => Ok(false),
            Err(other) => Err(other),
        }
    }

    fn compare_lhs(&self, target: &CondTarget, scope: &Scope) -> Result<CompareLhs, EvalError> {
        match target {
            CondTarget::Ref(r) => self.compare_lhs_ref(r),
            CondTarget::Alias(a) => self.compare_lhs_alias(a, scope),
        }
    }

    fn compare_lhs_ref(&self, r: &Reference) -> Result<CompareLhs, EvalError> {
        let block = self.fetch_block(&r.key)?;
        if let Access::Func { name, tag, field } = &r.access {
            if field.is_none() {
                if *name == FuncName::Count {
                    let n = block.tags.get(tag).map_or(0, Vec::len);
                    return Ok(CompareLhs::Int(i64::try_from(n).unwrap_or(i64::MAX)));
                }
                if *name == FuncName::Has {
                    return Ok(CompareLhs::Bool(block.tags.contains_key(tag)));
                }
            }
        }
        let s = self.render_reference(r)?;
        Ok(CompareLhs::String(s))
    }

    fn compare_lhs_alias(&self, a: &AliasRef, scope: &Scope) -> Result<CompareLhs, EvalError> {
        if let AliasAccess::Func {
            name,
            tag,
            field: None,
        } = &a.access
        {
            let entry = scope
                .lookup(&a.alias)
                .ok_or_else(|| EvalError::UndefinedAlias(a.alias.clone()))?;
            if let ScopeBinding::Block(block) = &entry.binding {
                if *name == FuncName::Count {
                    let n = block.tags.get(tag).map_or(0, Vec::len);
                    return Ok(CompareLhs::Int(i64::try_from(n).unwrap_or(i64::MAX)));
                }
                if *name == FuncName::Has {
                    return Ok(CompareLhs::Bool(block.tags.contains_key(tag)));
                }
            }
        }
        let s = self.render_alias(a, scope)?;
        Ok(CompareLhs::String(s))
    }
}

#[derive(Debug)]
enum CompareLhs {
    Int(i64),
    String(String),
    Bool(bool),
}

const fn compare_int(op: CompareOp, a: i64, b: i64) -> bool {
    match op {
        CompareOp::Eq => a == b,
        CompareOp::Ne => a != b,
        CompareOp::Gt => a > b,
        CompareOp::Lt => a < b,
        CompareOp::Gte => a >= b,
        CompareOp::Lte => a <= b,
    }
}

/// True for the anchor at `prefix` plus every descendant — dot-children
/// (`prefix.x`) and satellites (`prefix::x`). Strict segment boundary so
/// `module(api.user)` does NOT match `api.users`.
fn is_module_member(key: &str, prefix: &str) -> bool {
    if !key.starts_with(prefix) {
        return false;
    }
    if key.len() == prefix.len() {
        return true;
    }
    let bytes = key.as_bytes();
    let next = bytes[prefix.len()];
    if next == b'.' {
        return true;
    }
    next == b':' && bytes.get(prefix.len() + 1) == Some(&b':')
}

/// True only for satellites of `prefix` (`prefix::*`) — excludes the
/// anchor at `prefix` and any dot-children.
fn is_satellite_of(key: &str, prefix: &str) -> bool {
    if !key.starts_with(prefix) {
        return false;
    }
    let bytes = key.as_bytes();
    bytes.len() > prefix.len() + 2 && bytes[prefix.len()] == b':' && bytes[prefix.len() + 1] == b':'
}

fn compare_string(op: CompareOp, a: &str, b: &str) -> bool {
    match op {
        CompareOp::Eq => a == b,
        CompareOp::Ne => a != b,
        CompareOp::Gt => a > b,
        CompareOp::Lt => a < b,
        CompareOp::Gte => a >= b,
        CompareOp::Lte => a <= b,
    }
}

const fn compare_bool(op: CompareOp, a: bool, b: bool) -> bool {
    match op {
        CompareOp::Eq => a == b,
        CompareOp::Ne => a != b,
        CompareOp::Gt | CompareOp::Lt | CompareOp::Gte | CompareOp::Lte => false,
    }
}

fn tag_occurrences<'a>(block: &'a DocBlock, tag: &str) -> Result<&'a Vec<TagFields>, EvalError> {
    block.tags.get(tag).ok_or_else(|| EvalError::UnknownTag {
        key: block.key.as_str().to_owned(),
        tag: tag.to_owned(),
    })
}

/// Default projection when user writes `{{ @doc.X }}` (without `:`).
///
/// Strategy: combine `signature` (when symbol is available) and
/// `description` (when present) to produce a useful markdown summary in one
/// expression. Degraded cases:
///
/// - symbol + description -> `{signature}\n\n{description}`
/// - symbol only          -> `{signature}`
/// - description only     -> `{description}`
/// - nothing              -> `label` (= v1 behavior)
///
/// Goal: a "lazy" template like `### {{ @doc.foo }}` should produce
/// something useful by default, without forcing users to write
/// write `{{ @doc.foo:symbol.signature }}\n\n{{ @doc.foo:description }}`.
fn render_default_projection(block: &DocBlock) -> String {
    let signature = block
        .symbol
        .as_ref()
        .map(|s| s.signature.trim())
        .filter(|s| !s.is_empty());
    // Description = first occurrence of `description` tag, joined fields.
    let description = block
        .tags
        .get("description")
        .and_then(|occ| occ.first())
        .map(|fields| fields.join(" "))
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());

    match (signature, description) {
        (Some(sig), Some(desc)) => format!("{sig}\n\n{desc}"),
        (Some(sig), None) => sig.to_owned(),
        (None, Some(desc)) => desc,
        (None, None) => block.label.clone(),
    }
}

fn evaluate_func_scalar(name: FuncName, block: &DocBlock, tag: &str) -> String {
    match name {
        FuncName::Has => block.tags.contains_key(tag).to_string(),
        FuncName::Count => block.tags.get(tag).map_or(0, Vec::len).to_string(),
        FuncName::First => block
            .tags
            .get(tag)
            .and_then(|v| v.first())
            .map(|fields| fields.join(" "))
            .unwrap_or_default(),
        FuncName::Last => block
            .tags
            .get(tag)
            .and_then(|v| v.last())
            .map(|fields| fields.join(" "))
            .unwrap_or_default(),
    }
}

fn resolve_field(block: &DocBlock, path: &[String]) -> Result<String, EvalError> {
    let first = path.first().ok_or_else(|| EvalError::UnknownField {
        key: block.key.as_str().to_owned(),
        field: String::new(),
    })?;
    match first.as_str() {
        "label" if path.len() == 1 => Ok(block.label.clone()),
        "key" if path.len() == 1 => Ok(block.key.as_str().to_owned()),
        "origin" if path.len() == 1 => Ok(origin_str(block).to_owned()),
        "meta" => resolve_meta(block, &path[1..]),
        "symbol" => resolve_symbol(block, &path[1..]),
        other => Err(EvalError::UnknownField {
            key: block.key.as_str().to_owned(),
            field: other.to_owned(),
        }),
    }
}

/// Explicit whitelist of `meta.X` fields exposed to DSL.
///
/// Why not direct `serde_json::to_value(&meta)`? Because it exposes
/// **everything** serializable, including internal fields (mtime,
/// `last_indexed`) that may change without notice. A whitelist ensures
/// user templates only rely on stable, documented fields.
fn resolve_meta(block: &DocBlock, path: &[String]) -> Result<String, EvalError> {
    if path.is_empty() {
        // Standalone `:meta` has no useful representation — explicit error.
        return Err(EvalError::UnknownField {
            key: block.key.as_str().to_owned(),
            field: "meta".to_owned(),
        });
    }
    if path.len() > 1 {
        return Err(EvalError::UnknownField {
            key: block.key.as_str().to_owned(),
            field: format!("meta.{}", path.join(".")),
        });
    }
    let m = &block.meta;
    let value = match path[0].as_str() {
        "path" => m.path.to_string_lossy().replace('\\', "/"),
        "lineStart" => m.line_start.to_string(),
        "lineEnd" => m.line_end.to_string(),
        "column" => m.column.to_string(),
        "fileExt" => m.file_ext.clone(),
        "commentStyle" => comment_style_str(m.comment_style).to_owned(),
        other => {
            return Err(EvalError::UnknownField {
                key: block.key.as_str().to_owned(),
                field: format!("meta.{other}"),
            });
        }
    };
    Ok(value)
}

/// Explicit whitelist of `symbol.X` fields exposed to DSL.
///
/// Structured sub-objects (`params`, `returns`) are intentionally not exposed.
/// Users should go through tags (`:param`, `:returns`), which are more
/// expressive and already schema-aware. `generics` and `decorators` are
/// joined with `, ` to produce useful scalar projections.
fn resolve_symbol(block: &DocBlock, path: &[String]) -> Result<String, EvalError> {
    let Some(symbol) = block.symbol.as_ref() else {
        return Err(EvalError::UnknownField {
            key: block.key.as_str().to_owned(),
            field: "symbol".to_owned(),
        });
    };
    if path.is_empty() {
        return Err(EvalError::UnknownField {
            key: block.key.as_str().to_owned(),
            field: "symbol".to_owned(),
        });
    }
    if path.len() > 1 {
        return Err(EvalError::UnknownField {
            key: block.key.as_str().to_owned(),
            field: format!("symbol.{}", path.join(".")),
        });
    }
    let value = match path[0].as_str() {
        "signature" => symbol.signature.clone(),
        "kind" => symbol_kind_str(symbol.kind).to_owned(),
        "visibility" => visibility_str(symbol.visibility).to_owned(),
        "isAsync" => symbol.is_async.to_string(),
        "isDeprecated" => symbol.is_deprecated.to_string(),
        "generics" => symbol.generics.join(", "),
        "decorators" => symbol.decorators.join(", "),
        other => {
            return Err(EvalError::UnknownField {
                key: block.key.as_str().to_owned(),
                field: format!("symbol.{other}"),
            });
        }
    };
    Ok(value)
}

const fn comment_style_str(style: crate::model::CommentStyle) -> &'static str {
    match style {
        crate::model::CommentStyle::SingleLine => "single-line",
        crate::model::CommentStyle::MultiLine => "multi-line",
        crate::model::CommentStyle::DocSingle => "doc-single",
        crate::model::CommentStyle::DocMulti => "doc-multi",
    }
}

const fn symbol_kind_str(kind: crate::model::SymbolKind) -> &'static str {
    use crate::model::SymbolKind as K;
    match kind {
        K::Function => "function",
        K::Method => "method",
        K::Class => "class",
        K::Struct => "struct",
        K::Enum => "enum",
        K::Trait => "trait",
        K::Interface => "interface",
        K::TypeAlias => "type-alias",
        K::Const => "const",
        K::Static => "static",
        K::Module => "module",
        K::Macro => "macro",
        K::Field => "field",
        K::Variant => "variant",
        K::Other => "other",
    }
}

const fn visibility_str(v: crate::model::Visibility) -> &'static str {
    use crate::model::Visibility as V;
    match v {
        V::Public => "public",
        V::Private => "private",
        V::Crate => "crate",
        V::Internal => "internal",
        V::Inherited => "inherited",
    }
}

const fn origin_str(block: &DocBlock) -> &'static str {
    match block.origin {
        crate::model::BlockOrigin::Inferred => "inferred",
        crate::model::BlockOrigin::Annotated => "annotated",
        crate::model::BlockOrigin::Hybrid => "hybrid",
    }
}

/// Translate dotted path of a block alias (`f.PATH`) into `Access`.
///
/// Rules:
/// - vide → `Bare` (default projection)
/// - `[label]` | `[key]` | `[origin]` → `Field`
/// - `[meta, ...]` | `[symbol, ...]` → `Field` (sub-path)
/// - `[X]` -> `Tag(X)` (resolved according to cardinality)
/// - `[X, Y]` → `TagShortcut { tag: X, field: Y }` (Single only)
/// - plus long → erreur
fn block_alias_access(path: &[String]) -> Result<Access, EvalError> {
    if path.is_empty() {
        return Ok(Access::Bare);
    }
    let head = &path[0];
    match head.as_str() {
        "label" | "key" | "origin" if path.len() == 1 => Ok(Access::Field(path.to_vec())),
        "meta" | "symbol" => Ok(Access::Field(path.to_vec())),
        _ => match path.len() {
            1 => Ok(Access::Tag(head.clone())),
            2 => Ok(Access::TagShortcut {
                tag: head.clone(),
                field: path[1].clone(),
            }),
            _ => Err(EvalError::Type(format!(
                "block alias path '{}' is too long — max 2 segments (tag.field)",
                path.join(".")
            ))),
        },
    }
}

/// Helper: extract tag name from an `Access` that references one.
/// Returns `None` for non-tag accesses (`Bare`, `Field`, etc.) — caller emits
/// the appropriate error.
fn tag_name_from_access(access: &Access) -> Option<String> {
    match access {
        Access::Tag(t)
        | Access::TagIndex { tag: t, .. }
        | Access::TagField { tag: t, .. }
        | Access::TagShortcut { tag: t, .. }
        | Access::Func { tag: t, .. } => Some(t.clone()),
        Access::Bare | Access::Field(_) => None,
    }
}

// -------- Scope (alias bindings for nested `each`) --------
//
// An alias can be bound either to a tag occurrence (each over tag) or to a
// full block (each over blocks). Dispatch happens when resolving `AliasRef`
// by checking binding type.

#[derive(Default)]
struct Scope {
    stack: Vec<ScopeEntry>,
}

enum ScopeBinding {
    /// `each p in @doc.K:tag` — alias bound to a tag occurrence.
    TagOccurrence { tag: String, fields: TagFields },
    /// `each f in @docs.module(K)` — alias bound to a full block.
    /// Boxed because `DocBlock` is heavy (~350 bytes) and most scopes use
    /// `TagOccurrence` (~50 bytes) — no reason to pay memory overhead in the
    /// common case.
    Block(Box<DocBlock>),
}

struct ScopeEntry {
    alias: String,
    binding: ScopeBinding,
}

impl Scope {
    fn push_tag(&mut self, alias: String, tag: String, fields: TagFields) {
        self.stack.push(ScopeEntry {
            alias,
            binding: ScopeBinding::TagOccurrence { tag, fields },
        });
    }

    fn push_block(&mut self, alias: String, block: DocBlock) {
        self.stack.push(ScopeEntry {
            alias,
            binding: ScopeBinding::Block(Box::new(block)),
        });
    }

    fn pop(&mut self) {
        self.stack.pop();
    }

    fn lookup(&self, alias: &str) -> Option<&ScopeEntry> {
        self.stack.iter().rev().find(|e| e.alias == alias)
    }
}

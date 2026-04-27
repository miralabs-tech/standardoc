use crate::model::{
    BlockOrigin, Diagnostic, DocKey, DocMeta, SymbolInfo, TagFields, TagName, VirtualConfidence,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Canonical structure of a single documentable block.
///
/// `tags` uses `BTreeMap` (not `HashMap`) so JSON output is byte-stable across
/// runs — this matters for diffing the canonical output and for
/// content-addressed caching.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocBlock {
    pub key: DocKey,
    pub label: String,
    pub origin: BlockOrigin,
    #[serde(default)]
    pub tags: BTreeMap<TagName, Vec<TagFields>>,
    /// Populated when an AST provider contributed to this block.
    /// `None` for `BlockOrigin::Annotated` blocks or unsupported languages.
    pub symbol: Option<SymbolInfo>,
    pub meta: DocMeta,
    /// Hash of the semantic body — excludes whitespace-only changes.
    /// Lets downstream clients skip re-render when nothing meaningful changed.
    pub body_hash: u64,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
    /// Virtual annotations synthesized by `crate::virtual_annotation` from
    /// AST plus naming heuristics. Same shape as `tags` so consumers can
    /// merge or discriminate freely. Empty when the pass is disabled or
    /// skipped (e.g. private symbol below the configured visibility tier).
    #[serde(default)]
    pub virtual_tags: BTreeMap<TagName, Vec<TagFields>>,
    /// Highest confidence tier across all signals that produced
    /// `virtual_tags`. `None` when `virtual_tags` is empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub virtual_confidence: Option<VirtualConfidence>,
    /// Heuristics that contributed (`"constructor"`, `"verb-get"`,
    /// `"trait-impl-display"`, `"param-name"`, …). Useful for diagnostics
    /// and tooling that wants to explain *why* a virtual annotation was
    /// emitted.
    #[serde(default)]
    pub virtual_sources: Vec<String>,
}

use serde::{Deserialize, Serialize};

/// Provenance of a [`DocBlock`](super::DocBlock): AST-only, annotation-only, or a mix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BlockOrigin {
    /// Fully derived from AST parsing — no `@doc` annotation in source.
    Inferred,
    /// Fully declared via `@doc` annotations — no AST provider contributed.
    Annotated,
    /// AST-derived base enriched by `@doc` annotations (typical case).
    Hybrid,
}

//! DSL parser and evaluator for `{{ @doc.<key>:<access> }}` expressions.
//!
//! # Grammar at a glance
//!
//! ```text
//! {{ @doc.KEY:label }}                    — block field
//! {{ @doc.KEY:meta.path }}                — metadata sub-path
//! {{ @doc.KEY:symbol.signature }}         — AST sub-path
//! {{ @doc.KEY:description }}              — tag shortcut (first occurrence, joined)
//! {{ @doc.KEY:param[0].name }}            — indexed tag, named field
//! {{ @doc.KEY:has(example) }}             — function (also count / first / last)
//!
//! {{ each p in @doc.KEY:param }}
//!   - **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
//! {{ /each }}
//!
//! {{ if @doc.KEY:has(example) }}
//!   ...
//! {{ else }}
//!   ...
//! {{ /if }}
//! ```
//!
//! Inside a tag access, the named fields come from the `TagSchema`. Built-in
//! schemas (`param`, `returns`, `example`, `description`, `since`, `deprecated`,
//! `see`) are merged with user-provided `tags` config — user entries win.

pub mod ast;
pub mod evaluator;
pub mod parser;
pub mod schema;

pub use ast::*;
pub use evaluator::{BlockSource, EvalError, Evaluator};
pub use parser::{parse, ParseError};
pub use schema::{builtin_schemas, merged_schemas};

use crate::config::TagSchema;
use crate::model::{DocBlock, DocKey};
use std::collections::BTreeMap;

/// Convenience: parse + evaluate in one shot with a `BTreeMap` block source.
///
/// Returns the rendered string, or the first parse/eval error encountered.
pub fn render_string(
    template_src: &str,
    blocks: &BTreeMap<String, DocBlock>,
    user_schemas: &BTreeMap<String, TagSchema>,
) -> Result<String, RenderError> {
    let template = parse(template_src)?;
    let schemas = merged_schemas(user_schemas);
    let by_key: BTreeMap<DocKey, DocBlock> = blocks
        .iter()
        .map(|(k, v)| (DocKey::new(k.clone()), v.clone()))
        .collect();
    let evaluator = Evaluator::new(&by_key, &schemas);
    Ok(evaluator.render(&template)?)
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error(transparent)]
    Parse(#[from] ParseError),
    #[error(transparent)]
    Eval(#[from] EvalError),
}

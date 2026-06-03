//! Cross-provider utility helpers.
//!
//! Hosts logic that was previously copy-pasted across the three language
//! providers (`rust/`, `ts/`, `lua/`) plus the SFC orchestrator
//! (`workspace.rs`) and the template parsers (`template/`). Per the
//! intra-crate-only dedup decision (no `standardoc-common` crate day-1),
//! these are all `pub(crate)` — promote to a separate crate ONLY when
//! a real cross-crate consumer materialises.

pub(crate) mod fqdn;
pub(crate) mod hash;
pub(crate) mod location;
pub(crate) mod path_ext;
pub(crate) mod template_text;

pub(crate) use fqdn::{last_segment, parent_module};
pub(crate) use hash::hash_bytes;
pub(crate) use location::file_span;
pub(crate) use path_ext::strip_extension;
pub(crate) use template_text::find_top_level_keyword;

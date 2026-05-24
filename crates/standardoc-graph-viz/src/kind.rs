//! Symbol kind taxonomy. Mirrors the five `kind` values the indexer
//! emits (`callable` / `type` / `value` / `module` / `macro`) plus an
//! `Unknown` fallback so a payload carrying an unexpected variant
//! deserialises cleanly instead of panicking.
//!
//! Only the enum survives the cards-only refactor — chip glyphs and
//! per-`Kind` section grouping (`glyph()`, `section_label()`,
//! `SECTIONS_ORDER`) belonged to the old Blueprint sections rendering
//! and were dropped with it.

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    Callable,
    Type,
    Value,
    Module,
    Macro,
    #[serde(other)]
    #[default]
    Unknown,
}

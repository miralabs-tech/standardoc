//! Symbol kind taxonomy. Mirrors the five `kind` values the indexer
//! emits (`function` / `type` / `value` / `module` / `macro`) plus an
//! `Unknown` fallback so a payload carrying an unexpected variant
//! deserialises cleanly instead of panicking — this preserves the
//! `_ => "·"` glyph fallback the renderer used to have.
//!
//! Compile-time exhaustiveness everywhere on the Rust side; TS side
//! mirrors the same shape and feeds it through `matchigo.compile()`.

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    Function,
    Type,
    Value,
    Module,
    Macro,
    #[serde(other)]
    #[default]
    Unknown,
}

impl Kind {
    /// Short glyph painted in the right corner of each chip when the
    /// payload omits a finer `language_kind` tag.
    pub(crate) fn glyph(self) -> &'static str {
        match self {
            Self::Function => "fn",
            Self::Type => "T",
            Self::Value => "val",
            Self::Module => "mod",
            Self::Macro => "mac",
            Self::Unknown => "·",
        }
    }

    /// Long human label used in section headers.
    pub(crate) fn section_label(self) -> &'static str {
        match self {
            Self::Function => "Functions",
            Self::Type => "Types",
            Self::Value => "Values",
            Self::Module => "Modules",
            Self::Macro => "Macros",
            Self::Unknown => "Other",
        }
    }
}

/// Section-display order within an owner. Modules first (sub-namespace
/// declarations at the top), then Types, Functions, Values, Macros,
/// then catch-all. Mirrors what most IDEs surface by default and
/// matches the order code-review tools use to surface diffs.
pub(crate) const SECTIONS_ORDER: [Kind; 6] = [
    Kind::Module,
    Kind::Type,
    Kind::Function,
    Kind::Value,
    Kind::Macro,
    Kind::Unknown,
];

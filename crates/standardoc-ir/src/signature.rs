use serde::{Deserialize, Serialize};

use crate::bridge_kind::BridgeKind;

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Signature {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<Param>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returns: Option<TypeRef>,
    #[serde(default, skip_serializing_if = "Modifiers::is_default")]
    pub modifiers: Modifiers,
    #[serde(default, skip_serializing_if = "SignatureMeta::is_default")]
    pub meta: SignatureMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    pub ty: TypeRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TypeRef {
    pub display: String,
}

impl TypeRef {
    /// Constructs a `TypeRef` from any `display`-bearing string and applies
    /// the generic `normalize_display` pass (trim + collapse internal
    /// multi-whitespace into single space). Call sites that source their
    /// string from `quote::ToTokens::to_token_stream().to_string()` should
    /// pre-process the raw text with `compact_rust_tokens` before passing
    /// it here — `new` alone preserves single spaces (so TypeScript unions
    /// like `string | number` survive).
    pub fn new(display: impl Into<String>) -> Self {
        Self {
            display: normalize_display(&display.into()),
        }
    }
}

/// Generic neutralizer applied inside [`TypeRef::new`]. Trims leading and
/// trailing whitespace, then collapses every run of internal whitespace
/// (spaces, tabs, newlines) into a single space. Single spaces between
/// tokens are preserved, so language-specific spacing conventions
/// (TypeScript unions `string | number`, Rust `dyn Trait`) round-trip
/// unchanged. Idempotent.
fn normalize_display(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Re-renders the Rust-token-stream-derived `display` form in compact
/// canonical Rust style: no spaces around `<`, `>`, `::`, `&`, `(`, `)`,
/// `[`, `]`, `:` (single-colon type/bound annotations), and `'`
/// (lifetime apostrophe). Single space is preserved after `,`, `;`, `:`,
/// and around keywords (`dyn`, `impl`, `mut`, …) and binary operators
/// (`+` in trait bounds, `->` in return types).
///
/// Use this in lang providers that source their type strings from
/// `quote::ToTokens::to_token_stream().to_string()` (Rust provider): the
/// syn pretty-printer inserts a space between every token tree, which
/// bloats the `display` field on every find_symbol / get_context call.
/// TS and Lua providers source their strings from raw source snippets
/// or annotation text and do not need this pre-pass — [`TypeRef::new`]'s
/// normalize alone handles their cases.
///
/// Idempotent on already-compact input.
pub fn compact_rust_tokens(s: &str) -> String {
    let tokens: Vec<&str> = s.split_whitespace().collect();
    if tokens.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(s.len());
    for (i, &tok) in tokens.iter().enumerate() {
        if i > 0 {
            let prev = tokens[i - 1];
            if needs_space_between_rust_tokens(prev, tok) {
                out.push(' ');
            }
        }
        out.push_str(tok);
    }
    out
}

fn needs_space_between_rust_tokens(prev: &str, curr: &str) -> bool {
    if matches!(curr, ">" | ")" | "]" | "," | ";" | ":") {
        return false;
    }
    if matches!(curr, "<" | "(" | "[") {
        return false;
    }
    if matches!(prev, "<" | "(" | "[" | "&" | "'") {
        return false;
    }
    if prev == "::" || curr == "::" {
        return false;
    }
    true
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Modifiers {
    #[serde(default, rename = "async", skip_serializing_if = "std::ops::Not::not")]
    pub is_async: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub generic_params: Vec<String>,
    /// Raw text of the `where T: Foo, U: Bar` clause when present. Inline
    /// generic bounds (`<T: Display>`) are already part of `generic_params`,
    /// so this only captures the trailing `where` extension.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub where_clause: Option<String>,
}

impl Modifiers {
    const fn is_default(&self) -> bool {
        !self.is_async
            && self.deprecated.is_none()
            && self.generic_params.is_empty()
            && self.where_clause.is_none()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SignatureMeta {
    /// Bridges through which this symbol is exposed across substrate
    /// boundaries. Multiple entries support dual-target apps
    /// (e.g. a Tauri command also compiled to wasm — `["tauri", "wasm-bindgen"]`).
    ///
    /// IR-3 1.0 shape: `Vec<BridgeKind>` (was `Option<BridgeKind>` pre-3e-3).
    /// Default is the empty vec — backward-compatible with pre-IR-3 rows
    /// where the field was absent via `skip_serializing_if`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exposed_via: Vec<BridgeKind>,
}

impl SignatureMeta {
    const fn is_default(&self) -> bool {
        self.exposed_via.is_empty()
    }
}

#[cfg(test)]
mod tests;

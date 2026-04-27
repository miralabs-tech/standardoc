//! Language provider trait + discovered-symbol type.
//!
//! Concrete implementations live in separate crates (`standardoc-lang-ts`,
//! `standardoc-lang-rust`, `standardoc-lang-python`, `standardoc-lang-tree-sitter`).
//! Adding a new language = implementing this trait in a new crate.
//! No core modification required.

use crate::model::{CommentStyles, Diagnostic, DocBlock, SourceRange, SymbolInfo};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("syntax error in {path}: {message}")]
    Syntax { path: String, message: String },

    #[error("unsupported language feature in {path}: {message}")]
    Unsupported { path: String, message: String },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

/// A symbol discovered by a language provider, ready to be folded into a `DocBlock`.
#[derive(Debug, Clone)]
pub struct DiscoveredSymbol {
    /// Fully qualified name segments, e.g. `["api", "users", "create"]`.
    /// Joined with `.` to form the default `DocKey` under `KeyStrategy::Fqn`.
    pub fqn: Vec<String>,
    pub symbol: SymbolInfo,
    pub source_range: SourceRange,
    /// Raw text of the comment immediately preceding the symbol, if any.
    /// The extractor is responsible for parsing `@doc`/`@param`/... tags
    /// out of this string — language providers don't interpret annotations.
    pub leading_comment: Option<String>,
    /// 1-indexed line where the leading doc-comment block starts, when the
    /// provider has consumed a comment that is not contiguous with the
    /// symbol (e.g. Rust `///` separated from `pub struct Foo` by one or
    /// more `#[derive(...)]` attributes). The pipeline uses this to skip
    /// free-floating-anchor extraction on spans the provider already
    /// folded into `leading_comment`. `None` when the comment is directly
    /// adjacent to the symbol — the existing `symbol_starts` filter handles
    /// that case.
    pub leading_comment_line_start: Option<u32>,
}

pub trait LanguageProvider: Send + Sync {
    /// Stable identifier, e.g. `"typescript"`.
    fn id(&self) -> &'static str;

    /// File extensions handled by this provider, e.g. `&[".ts", ".tsx"]`.
    fn extensions(&self) -> &[&'static str];

    fn comment_styles(&self) -> &CommentStyles;

    fn discover_symbols(
        &self,
        content: &str,
        path: &Path,
    ) -> Result<Vec<DiscoveredSymbol>, ParseError>;

    /// Cross-check an annotated `@doc` block against the parsed symbol.
    /// Default: no extra checks. Override to catch drift such as `@param a`
    /// referring to a parameter that no longer exists in the signature.
    fn validate_doc_against_symbol(
        &self,
        _doc: &DocBlock,
        _symbol: &SymbolInfo,
    ) -> Vec<Diagnostic> {
        Vec::new()
    }

    /// Suggest a starter `@doc` template for a freshly inferred symbol.
    /// Used by the MCP `get_doc_template` tool so agents produce valid syntax.
    fn suggest_doc_template(&self, symbol: &SymbolInfo) -> String {
        default_template(symbol)
    }
}

fn default_template(_symbol: &SymbolInfo) -> String {
    String::from("@doc <key>\n@description \n")
}

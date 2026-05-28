//! Reusable DSL parser for the `standar*` ecosystem.
//!
//! The DSL grammar supports nested blocks, arrays, scalar literals, and
//! interpolated strings — designed to host config files like
//! `standarbuild`'s `.sxb` (task definitions) and `standardoc`'s `.sxd`
//! (workspace config). The parser is consumer-agnostic: it produces a
//! generic [`ast::File`] tree ; downstream crates lower it into their
//! own typed schemas.
//!
//! # Example
//!
//! ```no_run
//! let src = r#"
//!     version "0.1.0"
//!     project "ext" {
//!         path "./ext"
//!         tasks ["build" "test"]
//!     }
//! "#;
//! let file = standarx_dsl::parse(src).expect("parse ok");
//! assert_eq!(file.stmts.len(), 2);
//! ```

pub mod ast;
pub mod diag;
pub mod lexer;
pub mod parser;

pub use ast::{File, Stmt};
pub use diag::{Diag, DiagKind, Severity, Span, Spanned};

/// Parse a `.sxb` / `.sxd` source string into an [`ast::File`] tree.
///
/// Errors carry a [`Span`] referring to the offending byte range in `src`
/// — downstream consumers can render them however they prefer (see
/// `standarbuild`'s `diag::render` module for a sample renderer).
pub fn parse(src: &str) -> Result<File, Diag> {
    let tokens = lexer::tokenize(src)?;
    parser::parse_tokens(tokens, src.len()..src.len())
}

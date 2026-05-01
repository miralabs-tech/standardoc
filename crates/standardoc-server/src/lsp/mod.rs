//! LSP daemon module.
//!
//! Module split mirrors VISION.md L96 (small files, dispatch by concern):
//!
//! - [`serve`]    — public `serve_lsp` entry, orchestrates the LSP boot
//! - [`handler`]  — `StandardocLsp` struct + `impl LanguageServer`
//! - [`progress`] — cold start runner with `workDoneProgress` reports
//! - [`paths`]    — LSP `Uri` ↔ workspace-relative path helpers
//! - [`error`]    — `ServerError` → `jsonrpc::Error` conversion

mod error;
mod handler;
mod paths;
mod progress;
mod serve;

pub use handler::StandardocLsp;
pub use serve::{build_lsp_service, serve_lsp};

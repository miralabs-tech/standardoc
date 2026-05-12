//! MCP daemon module.
//!
//! Module split mirrors VISION.md L96 (small files, dispatch by concern):
//!
//! - [`serve`] — public `serve_mcp` entry, orchestrates the MCP boot
//! - [`handler`] — `StandardocMcp` struct + day-1 tool param shapes;
//!   `#[tool_router]` + `#[tool]` macros wired in session 26
//! - [`progress`] — cold-start runner with best-effort
//!   `notifications/progress` (when the client opted in via `progressToken`)
//! - [`error`] — `ServerError` → `rmcp::ErrorData` conversion helper

mod error;
mod handler;
mod progress;
mod serve;

pub use handler::{ResolveExternalJson, ResolveExternalParams, StandardocMcp};
pub use serve::{
    build_mcp_handler, build_mcp_handler_with_rag, kick_off_indexing, serve_mcp, serve_mcp_http,
};

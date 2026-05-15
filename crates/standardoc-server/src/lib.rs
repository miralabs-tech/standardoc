#![allow(clippy::result_large_err)]

mod error;
mod lifecycle;
mod lsp;
mod mcp;

pub use error::ServerError;
pub use lifecycle::{RunningServer, index_once, open_workspace, rescan};
pub use lsp::{StandardocLsp, build_lsp_service, serve_lsp};
pub use mcp::{
    ResolveExternalJson, ResolveExternalParams, StandardocMcp, build_mcp_handler,
    build_mcp_handler_with_rag, kick_off_indexing, serve_mcp, serve_mcp_http,
};

pub use standardoc_core::query;

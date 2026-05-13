//! RAG layer wiring on top of `standardoc-rag`.
//!
//! Three concerns lifted into core (where the workspace state lives) :
//!
//! 1. [`discovery::discover_prose_sources`] — convention paths + frontmatter
//!    opt-in scan, honouring `.stdignore` via `ScanFilters`.
//! 2. [`lookup::CoreSymbolLookup`] — bridges `standardoc-rag::SymbolLookup`
//!    to the core's `query` API so the linker can produce confidence
//!    + def-site boost.
//! 3. [`pipeline::RagPipeline`] — orchestrates chunk → embed → store →
//!    link for a single file (`run_for_source`) or for the whole
//!    workspace (`run_cold_start`).
//!
//! Daemons (LSP / MCP server / CLI) instantiate one `RagPipeline` at
//! boot and feed it through their existing lifecycle hooks.

mod discovery;
mod lookup;
mod pipeline;
mod relink_watcher;
mod watcher;

pub use discovery::{
    FrontmatterDirective, discover_prose_sources, is_convention_path, read_frontmatter_directive,
};
pub use lookup::{CoreSymbolLookup, WORKSPACE_FQDN_LIMIT};
pub use pipeline::{RagPipeline, RagPipelineError};
pub use relink_watcher::{RevisionRelinkHandle, spawn_revision_relink_watcher};
pub use watcher::{RagWatcherHandle, spawn_rag_watcher};

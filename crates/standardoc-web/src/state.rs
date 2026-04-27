//! Trait implemented by caller to expose index to web crate.
//!
//! We keep the web crate decoupled from `standardoc-server`: no cross-imports,
//! no cycle. `standardoc-server` implements `WebState` on its own `ServerState`
//! and passes an `Arc<dyn WebState>` to the router.
//!
//! The trait exposes only what REST/SSE handlers need — intentionally narrow
//! so tests can mock it without booting a full workspace.

use crate::types::{
    BlockSummary, DeletePageError, DocResponse, PageResponse, PageSummary, ReorderPageError,
    ResolvedSourceConfig, SavePageError, SearchMatch,
};
use std::path::Path;
use tokio::sync::broadcast;

/// Event broadcast on SSE channel when index changes.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IndexEvent {
    /// Emitted after each rescan that changes at least one block.
    IndexChanged { revision: u64 },
    /// Emitted periodically as heartbeat — keeps SSE connection alive behind
    /// aggressive proxies (typically 30s).
    Heartbeat { revision: u64 },
}

pub trait WebState: Send + Sync {
    /// Current revision number — incremented at each full or incremental
    /// rescan that mutates index. Client uses it as cache key invalidation
    /// after `IndexChanged`.
    fn revision(&self) -> u64;

    /// Compact list of all blocks for sidebar.
    fn list_blocks(&self) -> Vec<BlockSummary>;

    /// Full block detail, including markdown rendered to HTML.
    /// `None` if key does not exist in current index.
    fn get_doc(&self, key: &str) -> Option<DocResponse>;

    /// Simple full-text search — frontend uses it for search bar.
    /// Implementation can rank as it wants as long as results are sorted by
    /// descending relevance.
    fn search(&self, query: &str, limit: usize) -> Vec<SearchMatch>;

    /// DSL reference source markdown — served as-is on `/api/dsl-reference`.
    /// Client compiles it as MDX.
    fn dsl_reference_markdown(&self) -> &str;

    /// Compact list of all narrative pages (for "Guide" sidebar).
    /// Summary without body — each page is then loaded individually via
    /// `get_page`.
    fn list_pages(&self) -> Vec<PageSummary>;

    /// Full narrative page detail.
    ///
    /// Slug -> markdown rendered to HTML + frontmatter. If slug does not
    /// exist in index but matches an auto-generatable slug (see
    /// `reference/<key>`), adapter can generate a template on the fly.
    fn get_page(&self, slug: &str) -> Option<PageResponse>;

    /// Persist page content on disk at `.standardoc/pages/<slug>.md`.
    /// Creates parent directories when needed.
    ///
    /// If a page already exists on disk for this slug (potentially with
    /// `NN-` ordering prefix in filename), write into **existing file** to
    /// preserve order — otherwise write `<slug>.md`.
    ///
    /// Adapter is responsible for validating slug against path traversal
    /// (`..`, absolute paths, empty segments) before writing.
    fn save_page(&self, slug: &str, source: &str) -> Result<PageResponse, SavePageError>;

    /// Delete on-disk file of a page. Page can "reappear" on next rescan if
    /// it is auto-generatable (`reference/<key>` slug); otherwise it
    /// disappears from sidebar. Returns `NotOnDisk` if page exists in memory
    /// but without file (auto-page) — nothing to delete.
    fn delete_page(&self, slug: &str) -> Result<(), DeletePageError>;

    /// Update only `order:` field in page frontmatter.
    /// Used by reorder UI (up/down buttons). Does not touch body.
    fn reorder_page(&self, slug: &str, order: i32) -> Result<(), ReorderPageError>;

    /// Subscribe to SSE event stream. Each call returns a new `Receiver`.
    /// Implementation owns `Sender` and pings when index changes.
    fn subscribe(&self) -> broadcast::Receiver<IndexEvent>;

    /// Workspace root — useful for resolving relative paths client-side when
    /// showing links to source.
    fn workspace_root(&self) -> &Path;

    /// Resolved configuration for source-link (vscode / github / source-view).
    /// Adapter resolves `mode: "auto"` based on context
    /// (daemon vs static export). Client builds final URL.
    fn source_config(&self, is_static_export: bool) -> ResolvedSourceConfig;
}

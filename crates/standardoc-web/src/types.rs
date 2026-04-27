//! Wire types for the REST API.
//!
//! These structs are the **single** contract surface between Rust backend and
//! TS frontend. Any change here must be mirrored in `web/src/api/types.ts`.
//! Convention: `serde(rename_all = "camelCase")` to stay idiomatic on JS side,
//! without TS casts.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Compact summary for sidebar and search bar. No markdown, no extended tags —
/// we want `GET /api/index` to stay sub-second on a 10k-block workspace.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockSummary {
    pub key: String,
    pub label: String,
    /// Symbol category (function, struct, ...). `None` for purely annotated
    /// blocks without AST symbol.
    pub kind: Option<String>,
    pub path: String,
    pub line_start: u32,
    pub signature: Option<String>,
    pub has_description: bool,
    /// FQN segments used to build sidebar tree client-side.
    /// Ex: `["std", "io", "BufReader"]`.
    pub module_path: Vec<String>,
    pub deprecated: bool,
}

/// Full response for `/api/doc/:key` — everything needed to render
/// documentation page with no extra REST call.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocResponse {
    pub key: String,
    pub label: String,
    pub origin: String,
    pub kind: Option<String>,
    pub visibility: Option<String>,
    pub signature: Option<String>,
    pub description_md: Option<String>,
    /// Description rendered to HTML (pulldown-cmark + syntect for code blocks).
    /// Frontend injects as-is — no JS-side re-render.
    pub description_html: Option<String>,
    pub params: Vec<DocParam>,
    pub returns: Option<DocReturns>,
    pub examples: Vec<DocExample>,
    pub see: Vec<String>,
    pub deprecated: Option<String>,
    pub since: Option<String>,
    pub meta: DocResponseMeta,
    /// Custom tags declared in `.standardoc.json`, grouped by name.
    /// Format unchanged vs core-side `DocBlock` — this is a thin passthrough.
    pub custom_tags: BTreeMap<String, Vec<Vec<String>>>,
    pub references: DocReferences,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocParam {
    pub name: String,
    pub type_repr: Option<String>,
    pub description: Option<String>,
    pub is_optional: bool,
    pub is_variadic: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocReturns {
    pub type_repr: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocExample {
    pub title: Option<String>,
    pub language: Option<String>,
    pub code: String,
    /// HTML-highlighted code from syntect. Frontend injects it directly.
    pub code_html: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocResponseMeta {
    pub path: String,
    pub line_start: u32,
    pub line_end: u32,
    pub file_ext: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocReferences {
    /// Symbols this doc points at (calls, type uses).
    pub outgoing: Vec<DocRef>,
    /// Symbols pointing at this doc.
    pub incoming: Vec<DocRef>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocRef {
    pub key: String,
    pub label: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexResponse {
    pub revision: u64,
    pub workspace_root: String,
    pub blocks: Vec<BlockSummary>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub revision: u64,
    pub query: String,
    pub matches: Vec<SearchMatch>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchMatch {
    pub key: String,
    pub label: String,
    pub kind: Option<String>,
    pub path: String,
    pub snippet: Option<String>,
    /// Relevance score — informational, client already sorts by received order.
    pub score: f32,
}

/// Body of `PATCH /api/page/:slug` — only updates frontmatter `order:`
/// without touching content. Used by reorder UI.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReorderPageRequest {
    pub order: i32,
}

/// Body of `PUT /api/page/:slug`.
///
/// Full markdown file content (frontmatter + body). We let users edit raw
/// content — more transparent for disk persistence and avoids complexity of
/// a strongly typed wire frontmatter schema.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavePageRequest {
    pub source: String,
}

/// Structured errors for page-mutator endpoints.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SavePageError {
    /// Malformed slug (path traversal, empty segments, forbidden chars).
    InvalidSlug,
    /// I/O failure (permissions, disk full, etc.).
    IoError,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReorderPageError {
    InvalidSlug,
    NotOnDisk,
    IoError,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeletePageError {
    InvalidSlug,
    NotFound,
    /// Page exists in index but not on disk — auto-page, nothing to delete
    /// (user probably wants to edit then save).
    NotOnDisk,
    IoError,
}

/// `/api/pages` response — flat list of all pages.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PagesResponse {
    pub revision: u64,
    pub pages: Vec<PageSummary>,
}

/// Compact narrative-page entry for "Guide" sidebar.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageSummary {
    pub slug: String,
    pub title: String,
    /// Hierarchy: `["reference", "auth"]` for `/reference/auth/login`.
    /// Empty for root pages.
    pub section: Vec<String>,
    /// Order inside section. `None` = alphabetically sorted after ordered entries.
    pub order: Option<i32>,
    /// `"md"` or `"mdx"` — informs client about render pipeline.
    pub kind: String,
    /// `true` if page exists on disk (user-curated). `false` if it is
    /// auto-generated on the fly from index.
    pub on_disk: bool,
}

/// Full response for `/api/page/:slug`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageResponse {
    pub slug: String,
    pub title: String,
    pub kind: String,
    pub frontmatter: BTreeMap<String, serde_json::Value>,
    /// Raw markdown **after** DSL eval but **before** HTML/MDX rendering.
    /// Used by client-side runtime MDX compilation or future editor flows.
    pub source: String,
    /// Pre-rendered HTML (Md only). `None` for MDX pages compiled client-side.
    /// Frontend decides based on `kind`.
    pub html: Option<String>,
    pub on_disk: bool,
    /// Workspace-relative path when `onDisk=true`, so future editors can
    /// resolve where to write.
    pub path: Option<String>,
    pub section: Vec<String>,
    pub order: Option<i32>,
}

/// Source-link wire format. `.standardoc.json` `auto` mode is resolved
/// server-side by context (daemon -> vscode, static export -> github if
/// configured else source-view). Client builds final URL from
/// `path` + `lineStart` + this config.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "mode",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ResolvedSourceConfig {
    /// `vscode://file/<workspaceRoot>/<path>:<line>`.
    Vscode { workspace_root: String },
    /// `https://github.com/<repo>/blob/<branch>/<path>#L<line>`.
    Github { repo: String, branch: String },
    /// No external URL — client shows embedded highlighted source panel.
    /// To be implemented later (placeholder).
    SourceView,
}

//! Static site export.
//!
//! `export_to(state, out_dir)` writes the documentation snapshot to a
//! directory. Two modes, transparent to the caller:
//!
//! - **Data + frontend** (when a frontend is bundled at build time): writes
//!   all embedded assets (HTML, JS, CSS, fonts, brand SVGs), pre-renders
//!   pages into `static-data.json`, and patches `index.html` to inject
//!   `window.__STDOC_STATIC__=true` so the frontend loads `static-data.json`
//!   instead of calling `/api/*`. Result: a CDN-deployable site.
//!
//! - **Data only** (default in the open-source `standardoc-server` binary,
//!   which ships without a bundled frontend): writes `static-data.json`
//!   alone. Consumers can plug it into any external static site generator
//!   (Astro, Vitepress, Hugo, …) or build their own viewer against the same
//!   schema as the REST API.

#[cfg(feature = "embedded-frontend")]
use crate::embed::WebAssets;
use crate::state::WebState;
use serde::Serialize;
use std::path::Path;

/// Errors that can occur during export.
#[derive(Debug)]
pub enum ExportError {
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Json(e) => write!(f, "JSON serialisation error: {e}"),
        }
    }
}

impl From<std::io::Error> for ExportError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for ExportError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StaticData {
    revision: u64,
    workspace_root: String,
    blocks: Vec<crate::types::BlockSummary>,
    pages: Vec<crate::types::PageSummary>,
    /// Slug → full page content (empty string = home page).
    page_contents: std::collections::BTreeMap<String, crate::types::PageResponse>,
    /// DocKey → full DocResponse for every public block. Lets the frontend
    /// render `/reference/<key>` (DocPage) without hitting `/api/doc/:key`.
    docs: std::collections::BTreeMap<String, crate::types::DocResponse>,
    /// Resolved source-link config (vscode/github/source-view). The export
    /// path passes `is_static_export = true` so `mode: auto` resolves to the
    /// CDN-friendly target.
    source_config: crate::types::ResolvedSourceConfig,
}

/// Export the documentation snapshot to `out_dir` (created if absent).
///
/// Returns the number of frontend assets written. `Ok(0)` means data-only
/// mode (no frontend was bundled at build time) — `static-data.json` is
/// still written and is consumable by any external tool.
pub fn export_to(state: &dyn WebState, out_dir: &Path) -> Result<usize, ExportError> {
    std::fs::create_dir_all(out_dir)?;

    #[cfg(feature = "embedded-frontend")]
    let asset_count = {
        let mut count = 0usize;
        for path_cow in WebAssets::iter() {
            let path_str = path_cow.as_ref();
            if path_str == ".gitkeep" {
                continue;
            }
            if let Some(asset) = WebAssets::get(path_str) {
                let dest = out_dir.join(path_str);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                if path_str == "index.html" {
                    let raw = std::str::from_utf8(&asset.data).unwrap_or("");
                    let patched = raw.replace(
                        "</head>",
                        "<script>window.__STDOC_STATIC__=true;</script></head>",
                    );
                    std::fs::write(&dest, patched.as_bytes())?;
                } else {
                    std::fs::write(&dest, &*asset.data)?;
                }
                count += 1;
            }
        }
        count
    };
    #[cfg(not(feature = "embedded-frontend"))]
    let asset_count = 0usize;

    // ── 2. Pre-render all pages into static-data.json ────────────────────────
    let pages = state.list_pages();
    let blocks = state.list_blocks();
    let revision = state.revision();
    let workspace_root = state.workspace_root().display().to_string();

    let mut page_contents = std::collections::BTreeMap::new();
    // Home page (slug "").
    if let Some(home) = state.get_page("") {
        page_contents.insert(String::new(), home);
    }
    // All pages listed in the sidebar.
    for ps in &pages {
        page_contents.entry(ps.slug.clone()).or_insert_with(|| {
            state
                .get_page(&ps.slug)
                .unwrap_or_else(|| crate::types::PageResponse {
                    slug: ps.slug.clone(),
                    title: ps.title.clone(),
                    kind: ps.kind.clone(),
                    frontmatter: Default::default(),
                    source: String::new(),
                    html: None,
                    on_disk: ps.on_disk,
                    path: None,
                    section: ps.section.clone(),
                    order: ps.order,
                })
        });
    }

    // Pre-render every block's DocResponse so DocPage works in static mode.
    let mut docs = std::collections::BTreeMap::new();
    for b in &blocks {
        if let Some(doc) = state.get_doc(&b.key) {
            docs.insert(b.key.clone(), doc);
        }
    }

    let source_config = state.source_config(true);
    let data = StaticData {
        revision,
        workspace_root,
        blocks,
        pages,
        page_contents,
        docs,
        source_config,
    };
    let json = serde_json::to_vec_pretty(&data)?;
    std::fs::write(out_dir.join("static-data.json"), json)?;

    Ok(asset_count)
}

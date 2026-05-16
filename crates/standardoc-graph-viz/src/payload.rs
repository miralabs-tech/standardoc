//! Data shapes the JS host POSTs at the engine. Mirrors the
//! `BrowseSymbolJson` / `BrowseEdgeJson` types defined in the VSCode
//! extension's `graph/protocol.ts`. Keep the two in sync (or
//! eventually emit them from this crate via `ts-rs` once the API
//! stabilises).
//!
//! Several fields (`focal`, `file`, `start_line`, `outbound`) are
//! deserialised today but not yet consumed by the renderer. They are
//! kept on the struct so the JS host doesn't have to drop them on
//! send; the next iteration of the engine (focal layout, click-to-open
//! source, directed edges) will read them.

#![allow(dead_code)]

use serde::Deserialize;

use crate::kind::Kind;

#[derive(Debug, Deserialize)]
pub(crate) struct GraphPayload {
    #[serde(default)]
    pub symbols: Vec<SymbolEntry>,
    #[serde(default)]
    pub edges: Vec<EdgeEntry>,
    #[serde(default)]
    pub focal: Option<String>,
}

/// Edges-only payload accepted by `GraphEngine::set_edges`. Used when
/// the host already pushed a `load_graph` (which laid out the nodes
/// and built `node_by_fqdn`) and now wants to refresh just the edge
/// set — e.g. after lazily fetching a hovered symbol's `get_context`.
/// Re-using `load_graph` for this re-runs the cluster pack AND resets
/// the viewport, which kills user-applied pan/zoom.
#[derive(Debug, Deserialize)]
pub(crate) struct EdgesPayload {
    #[serde(default)]
    pub edges: Vec<EdgeEntry>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SymbolEntry {
    pub fqdn: String,
    pub name: String,
    #[serde(default)]
    pub kind: Kind,
    pub visibility: String,
    #[serde(default)]
    pub module: Option<String>,
    #[serde(default)]
    pub language_kind: String,
    #[serde(default)]
    pub is_external: bool,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub start_line: u32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct EdgeEntry {
    pub from: String,
    pub to: String,
    /// CALLS / IMPORTS / EXTENDS / IMPLEMENTS / REFERENCES / DEFINES / USES_TYPE / EXPOSES_API.
    pub kind: String,
    #[serde(default)]
    pub outbound: bool,
}

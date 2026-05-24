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
    /// Project lookup table for the `SymbolEntry.project_id` foreign
    /// key. The layout frames symbols by project and nests frames by
    /// `rel_path`; entries no symbol references are ignored.
    #[serde(default)]
    pub projects: Vec<ProjectEntry>,
    #[serde(default)]
    pub focal: Option<String>,
}

/// One detected project — the framing tier above the module tree.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ProjectEntry {
    pub project_id: u32,
    pub label: String,
    /// Ecosystem tag (`rust` / `node` / `bun` / `deno` / `python` /
    /// `lua` / `c` / `cpp` / `custom:<tag>` / `unknown`) — drives the
    /// frame colour via `Palette::project_color`.
    #[serde(default)]
    pub kind: String,
    /// POSIX-style path relative to the workspace root. Project
    /// frames nest by `rel_path` prefix.
    #[serde(default)]
    pub rel_path: String,
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
    pub language: String,
    #[serde(default)]
    pub is_external: bool,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub start_line: u32,
    #[serde(default)]
    pub project_id: Option<u32>,
    /// Refined declaration kind (snake_case string from `DeclKind`,
    /// or `custom:<lang>:<tag>` for the escape hatch). K-Step-F will
    /// shape nodes by this; today it is just passed through.
    #[serde(default)]
    pub decl_kind: Option<String>,
    /// For a method, the FQDN of the trait it implements (Rust
    /// `impl Trait for Type`) when known. Lights up trait-grouping
    /// in K-Step-F.
    #[serde(default)]
    pub implements_trait: Option<String>,
    /// For a method, the printed receiver type (`&Foo`, `Box<Self>`,
    /// `self`, …). Pairs with `implements_trait` to disambiguate
    /// overloaded method names across receivers.
    #[serde(default)]
    pub receiver_type: Option<String>,
    /// Phase 3 (Flow) — when set, this symbol is an entry-point (one
    /// of `binary_main` / `public_api` / `ffi_export`). The flow viz
    /// uses this to identify roots; internal symbols are `None`.
    /// String type mirrors the `decl_kind` choice — viz consumes the
    /// snake_case wire value without pulling `standardoc_ir::EntryPointKind`.
    #[serde(default)]
    pub entry_point: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct EdgeEntry {
    pub from: String,
    pub to: String,
    /// CALLS / IMPORTS / EXTENDS / IMPLEMENTS / REFERENCES / USES_TYPE.
    pub kind: String,
    #[serde(default)]
    pub outbound: bool,
}

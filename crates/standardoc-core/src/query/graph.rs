//! Pre-composed graph payload for the visualization layer.
//!
//! `fetch_graph` joins symbol rows and edges in the shape the
//! `standardoc-graph-viz` WASM crate consumes directly — flat
//! `SymbolEntry` records (file / start_line at the top level, not
//! nested under `location`) and `EdgeEntry` records with a precomputed
//! `outbound` hint. This deletes the playground's `rawToBrowse` shim
//! and pushes graph composition into the source-of-truth layer, per
//! the "viz is a dumb renderer" contract.
//!
//! Two modes:
//!
//! - `focal = Some(fqdn)` → bounded BFS expansion of depth
//!   `depth` (clamped to `1..=FETCH_GRAPH_MAX_DEPTH`). Each hop
//!   loads outbound (`edges_from`) AND inbound (`edges_to`) edges,
//!   filters by `kinds` BEFORE enqueueing, and stops as soon as
//!   `max_nodes` is reached.
//! - `focal = None` → bounded snapshot of the workspace ordered by
//!   `fqdn` ASC. Loads up to `max_nodes` symbols, then collects every
//!   `edges_from` whose target is in the bounded set (no dangling
//!   edges, no `edges_to` round-trip since the symmetric direction
//!   would re-discover the same edges).
//!
//! Unresolved / `UnresolvedBridge` edge targets are dropped — the
//! viz layer needs a real FQDN to connect both sides of an edge.
//! Edge dedup is keyed on `(from, to, kind)` so the same logical
//! edge re-discovered from both directions of the BFS doesn't
//! double-render.

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use standardoc_ir::{
    DeclKind, EdgeKind, EntryPointKind, Kind, LanguageKind, ResolvedOrUnresolved, Visibility,
};

use crate::query::{edges_from, edges_to};
use crate::storage::conv::{
    decl_kind_from_sql_text, entry_point_from_sql_text, kind_from_sql_text,
    visibility_from_sql_text,
};
use crate::storage::error::StorageError;
use crate::storage::handle::IndexHandle;
use crate::storage::module_lookup::PRIMARY_WORKSPACE_ID;

/// Default BFS depth when `focal` is set and the caller omits `depth`.
/// Two hops is enough to expose direct neighbors + their direct
/// neighbors (the "context ring" most UIs render around a focal node)
/// without exploding payload size on hub-shaped symbols.
pub const FETCH_GRAPH_DEFAULT_DEPTH: u8 = 2;

/// Hard ceiling on BFS depth. Five hops is already past the point
/// where a 2D layout stays legible; protects against accidental
/// `depth=255` calls turning into workspace-wide scans.
pub const FETCH_GRAPH_MAX_DEPTH: u8 = 5;

/// Default cap on the number of symbols returned. Tuned for
/// interactive layout responsiveness (~16 ms re-pack at this size on
/// the Canvas2D path of the playground).
pub const FETCH_GRAPH_DEFAULT_MAX_NODES: u32 = 500;

/// Hard ceiling on `max_nodes` requests. Above this, the viz layout
/// + the JSON envelope cross the threshold where the daemon ↔ wasm
/// round-trip itself becomes the bottleneck — past that, the right
/// answer is paginated focal expansion, not a single mega-graph.
pub const FETCH_GRAPH_MAX_NODES_CAP: u32 = 5000;

/// Page size used by the bounded-mode `list_symbols` walk. Matches
/// the MCP-side default — the daemon caps `list_symbols` at 100
/// items per call internally.
const BOUNDED_LIST_PAGE: usize = 100;

/// Composed request describing what slice of the graph to materialize.
///
/// Built by the MCP / LSP transport layer after clamping user input;
/// the core composition does NOT clamp again — it trusts the values
/// it receives. That separation keeps the clamp policy reviewable in
/// one place (the transport surface) without scattering `min`/`max`
/// expressions through the composition path.
#[derive(Debug, Clone)]
pub struct GraphRequest {
    /// FQDN to center the expansion on. `None` → bounded snapshot mode.
    pub focal: Option<String>,
    /// BFS depth for focal mode. Ignored when `focal` is `None`.
    pub depth: u8,
    /// Allow-list of edge kinds. `None` admits every kind; otherwise
    /// edges whose `kind` is not in the set are dropped AND not used
    /// to enqueue further neighbors during BFS.
    pub kinds: Option<HashSet<EdgeKind>>,
    /// Hard cap on the number of symbol nodes returned.
    pub max_nodes: u32,
    /// `false` scopes the result to workspace-authored symbols
    /// (excluding the `is_external = 1` rows from npm `.d.ts`,
    /// Cargo crate metadata, luarocks etc.).
    pub include_external: bool,
}

/// Flat symbol entry shaped for the WASM viz consumer. Mirrors the
/// `SymbolEntry` definition in `standardoc-graph-viz::payload` byte
/// for byte (lowercase `kind`, lowercase `visibility`, top-level
/// `file` + `start_line`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphSymbol {
    pub fqdn: String,
    pub name: String,
    pub kind: Kind,
    pub visibility: Visibility,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    pub language_kind: LanguageKind,
    pub language: String,
    pub is_external: bool,
    pub file: String,
    pub start_line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decl_kind: Option<DeclKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implements_trait: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver_type: Option<String>,
    /// Phase 3 (Flow) — when set, this symbol is an entry-point the
    /// viz can highlight as a flow root. `None` for internal symbols.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_point: Option<EntryPointKind>,
}

/// Edge record paired with an `outbound` hint relative to the focal
/// node (focal mode) or to the iterated source (bounded mode). The
/// viz uses `outbound` to colour direction; both sides are still
/// fully addressable via `from` / `to`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
    pub outbound: bool,
}

/// Project metadata for the symbols in a response. The viz layer
/// frames symbols by `project_id` and colours each frame by `kind`;
/// shipping the lookup table here keeps the viz a dumb renderer
/// (it never re-derives the project tree itself).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphProject {
    pub project_id: u32,
    pub label: String,
    /// Ecosystem tag — `rust` / `node` / `bun` / `deno` / `python` /
    /// `lua` / `c` / `cpp` / `custom:<tag>` / `unknown`.
    pub kind: String,
    /// POSIX-style path relative to the workspace root. The viz nests
    /// project frames by `rel_path` prefix.
    pub rel_path: String,
}

/// Composed response. `focal` echoes the request so the consumer
/// can stamp the centered node without round-tripping its own state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphResponse {
    pub symbols: Vec<GraphSymbol>,
    pub edges: Vec<GraphEdge>,
    /// Every project detected in the workspace. The consumer indexes
    /// it by `project_id` (the `GraphSymbol.project_id` foreign key)
    /// and ignores entries no symbol references.
    #[serde(default)]
    pub projects: Vec<GraphProject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focal: Option<String>,
}

/// Top-level entry point. Dispatches on `focal` and returns the
/// fully composed payload. Returns an empty `symbols` + `edges`
/// vec when `focal` is `Some` but no symbol matches that FQDN —
/// the request still echoes `focal` so the caller can distinguish
/// "unknown FQDN" from "no neighbors at any depth".
pub fn fetch_graph(handle: &IndexHandle, req: GraphRequest) -> Result<GraphResponse, StorageError> {
    match req.focal {
        Some(focal) => compose_focal(
            handle,
            focal,
            req.depth,
            req.kinds.as_ref(),
            req.max_nodes,
            req.include_external,
        ),
        None => compose_bounded(
            handle,
            req.kinds.as_ref(),
            req.max_nodes,
            req.include_external,
        ),
    }
}

fn compose_focal(
    handle: &IndexHandle,
    focal: String,
    depth: u8,
    kinds: Option<&HashSet<EdgeKind>>,
    max_nodes: u32,
    include_external: bool,
) -> Result<GraphResponse, StorageError> {
    let max_nodes = max_nodes as usize;
    let mut visited: HashSet<String> = HashSet::new();
    let mut frontier: Vec<String> = Vec::new();
    let mut edges: Vec<GraphEdge> = Vec::new();
    let mut seen_edges: HashSet<(String, String, EdgeKind)> = HashSet::new();

    // Seed: the focal node must exist in the index, else return an
    // empty payload (still echoing `focal` per the contract above).
    if !symbol_exists(handle, &focal)? {
        return Ok(GraphResponse {
            symbols: Vec::new(),
            edges: Vec::new(),
            projects: Vec::new(),
            focal: Some(focal),
        });
    }
    visited.insert(focal.clone());
    frontier.push(focal.clone());

    for _ in 0..depth {
        if visited.len() >= max_nodes || frontier.is_empty() {
            break;
        }
        let mut next_frontier: Vec<String> = Vec::new();
        for node in &frontier {
            // Outbound expansion (edges_from).
            for e in edges_from(handle, node)? {
                if !kind_allowed(kinds, e.kind) {
                    continue;
                }
                let Some(target) = resolved_fqdn(&e.to) else {
                    continue;
                };
                push_edge(
                    &mut edges,
                    &mut seen_edges,
                    node.clone(),
                    target.to_owned(),
                    e.kind,
                    true,
                );
                if visited.len() < max_nodes && !visited.contains(target) {
                    visited.insert(target.to_owned());
                    next_frontier.push(target.to_owned());
                }
            }
            // Inbound expansion (edges_to). `from_fqdn` is always
            // resolved because it is JOINed from `symbols.fqdn`.
            for e in edges_to(handle, node)? {
                if !kind_allowed(kinds, e.kind) {
                    continue;
                }
                push_edge(
                    &mut edges,
                    &mut seen_edges,
                    e.from_fqdn.clone(),
                    node.clone(),
                    e.kind,
                    false,
                );
                if visited.len() < max_nodes && !visited.contains(&e.from_fqdn) {
                    visited.insert(e.from_fqdn.clone());
                    next_frontier.push(e.from_fqdn);
                }
            }
        }
        frontier = next_frontier;
    }

    let visited_vec: Vec<String> = visited.iter().cloned().collect();
    let symbols = load_graph_symbols(handle, &visited_vec, include_external)?;
    let projects = load_graph_projects(handle)?;
    Ok(GraphResponse {
        symbols,
        edges,
        projects,
        focal: Some(focal),
    })
}

fn compose_bounded(
    handle: &IndexHandle,
    kinds: Option<&HashSet<EdgeKind>>,
    max_nodes: u32,
    include_external: bool,
) -> Result<GraphResponse, StorageError> {
    let max_nodes = max_nodes as usize;
    let node_fqdns = list_bounded_fqdns(handle, max_nodes, include_external)?;
    let node_set: HashSet<&str> = node_fqdns.iter().map(String::as_str).collect();

    let mut edges: Vec<GraphEdge> = Vec::new();
    let mut seen_edges: HashSet<(String, String, EdgeKind)> = HashSet::new();
    for from in &node_fqdns {
        for e in edges_from(handle, from)? {
            if !kind_allowed(kinds, e.kind) {
                continue;
            }
            let Some(target) = resolved_fqdn(&e.to) else {
                continue;
            };
            // Drop dangling edges whose target is outside the bounded
            // set — the viz can't render half an edge.
            if !node_set.contains(target) {
                continue;
            }
            push_edge(
                &mut edges,
                &mut seen_edges,
                from.clone(),
                target.to_owned(),
                e.kind,
                true,
            );
        }
    }

    let symbols = load_graph_symbols(handle, &node_fqdns, include_external)?;
    let projects = load_graph_projects(handle)?;
    Ok(GraphResponse {
        symbols,
        edges,
        projects,
        focal: None,
    })
}

/// Load every detected project as a flat `GraphProject` list. The
/// whole table is ~10-50 rows even on large monorepos, so we ship it
/// wholesale rather than filtering to referenced ids — the consumer
/// indexes by `project_id` and skips entries with no symbols.
fn load_graph_projects(handle: &IndexHandle) -> Result<Vec<GraphProject>, StorageError> {
    let projects = crate::query::projects::list_projects(handle)?;
    Ok(projects
        .into_iter()
        .map(|p| GraphProject {
            project_id: p.project_id,
            label: p.label,
            kind: p.kind.as_str().into_owned(),
            rel_path: p.rel_path,
        })
        .collect())
}

fn kind_allowed(allow: Option<&HashSet<EdgeKind>>, kind: EdgeKind) -> bool {
    allow.is_none_or(|set| set.contains(&kind))
}

const fn resolved_fqdn(target: &ResolvedOrUnresolved) -> Option<&str> {
    match target {
        ResolvedOrUnresolved::Resolved { fqdn } => Some(fqdn.as_str()),
        ResolvedOrUnresolved::Unresolved { .. } | ResolvedOrUnresolved::UnresolvedBridge { .. } => {
            None
        }
    }
}

fn push_edge(
    edges: &mut Vec<GraphEdge>,
    seen: &mut HashSet<(String, String, EdgeKind)>,
    from: String,
    to: String,
    kind: EdgeKind,
    outbound: bool,
) {
    let key = (from.clone(), to.clone(), kind);
    if seen.insert(key) {
        edges.push(GraphEdge {
            from,
            to,
            kind,
            outbound,
        });
    }
}

fn symbol_exists(handle: &IndexHandle, fqdn: &str) -> Result<bool, StorageError> {
    let pool = handle.pool()?;
    let conn = pool.get()?;
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM symbols WHERE workspace_id = ?1 AND fqdn = ?2 LIMIT 1",
            rusqlite::params![PRIMARY_WORKSPACE_ID, fqdn],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

fn list_bounded_fqdns(
    handle: &IndexHandle,
    max_nodes: usize,
    include_external: bool,
) -> Result<Vec<String>, StorageError> {
    if max_nodes == 0 {
        return Ok(Vec::new());
    }
    let pool = handle.pool()?;
    let conn = pool.get()?;
    let mut out: Vec<String> = Vec::with_capacity(max_nodes.min(BOUNDED_LIST_PAGE));
    let mut cursor: Option<String> = None;
    while out.len() < max_nodes {
        let remaining = max_nodes - out.len();
        let page_size = remaining.min(BOUNDED_LIST_PAGE);
        let page = list_fqdns_page(&conn, page_size, include_external, cursor.as_deref())?;
        if page.is_empty() {
            break;
        }
        cursor = page.last().cloned();
        let was_full_page = page.len() == page_size;
        out.extend(page);
        if !was_full_page {
            break;
        }
    }
    Ok(out)
}

fn list_fqdns_page(
    conn: &Connection,
    limit: usize,
    include_external: bool,
    cursor: Option<&str>,
) -> Result<Vec<String>, StorageError> {
    let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
    let mut stmt = conn.prepare(
        "SELECT fqdn FROM symbols \
         WHERE workspace_id = ?1 \
           AND (?2 = 1 OR is_external = 0) \
           AND (?3 IS NULL OR fqdn > ?3) \
         ORDER BY fqdn ASC \
         LIMIT ?4",
    )?;
    let rows = stmt
        .query_map(
            rusqlite::params![PRIMARY_WORKSPACE_ID, include_external, cursor, limit_i64],
            |row| row.get::<_, String>(0),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn load_graph_symbols(
    handle: &IndexHandle,
    fqdns: &[String],
    include_external: bool,
) -> Result<Vec<GraphSymbol>, StorageError> {
    if fqdns.is_empty() {
        return Ok(Vec::new());
    }
    let pool = handle.pool()?;
    let conn = pool.get()?;
    // `json_each` over a JSON array sidesteps the SQLITE_MAX_VARIABLE_NUMBER
    // ceiling (999 by default), so the same SQL works for both a small
    // BFS expansion and the 5000-node bounded ceiling without chunking.
    let fqdns_json = serde_json::to_string(fqdns)?;
    let mut stmt = conn.prepare(
        "SELECT s.fqdn, s.name, s.kind, s.language_kind, s.language, s.module, \
                s.visibility, s.file_path, s.start_line, s.is_external, f.project_id, \
                s.decl_kind, s.implements_trait, s.receiver_type, s.entry_point \
         FROM symbols s \
         LEFT JOIN files f ON f.path = s.file_path \
         WHERE s.workspace_id = ?1 \
           AND s.fqdn IN (SELECT value FROM json_each(?2)) \
           AND (?3 = 1 OR s.is_external = 0) \
         ORDER BY s.fqdn ASC",
    )?;
    let rows: Vec<GraphSymbol> = stmt
        .query_map(
            rusqlite::params![PRIMARY_WORKSPACE_ID, fqdns_json, include_external],
            read_graph_symbol,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(|raw| -> Result<GraphSymbol, StorageError> {
            let decl_kind = raw
                .decl_kind
                .as_deref()
                .map(decl_kind_from_sql_text)
                .transpose()?;
            let entry_point = raw
                .entry_point
                .as_deref()
                .map(entry_point_from_sql_text)
                .transpose()?;
            Ok(GraphSymbol {
                fqdn: raw.fqdn,
                name: raw.name,
                kind: kind_from_sql_text(&raw.kind)?,
                visibility: visibility_from_sql_text(&raw.visibility)?,
                module: raw.module,
                language_kind: LanguageKind::from(raw.language_kind),
                language: raw.language,
                is_external: raw.is_external,
                file: raw.file_path,
                start_line: raw.start_line,
                project_id: raw.project_id,
                decl_kind,
                implements_trait: raw.implements_trait,
                receiver_type: raw.receiver_type,
                entry_point,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

struct GraphSymbolRowRaw {
    fqdn: String,
    name: String,
    kind: String,
    language_kind: String,
    language: String,
    module: Option<String>,
    visibility: String,
    file_path: String,
    start_line: u32,
    is_external: bool,
    project_id: Option<u32>,
    decl_kind: Option<String>,
    implements_trait: Option<String>,
    receiver_type: Option<String>,
    entry_point: Option<String>,
}

fn read_graph_symbol(row: &rusqlite::Row<'_>) -> rusqlite::Result<GraphSymbolRowRaw> {
    Ok(GraphSymbolRowRaw {
        fqdn: row.get(0)?,
        name: row.get(1)?,
        kind: row.get(2)?,
        language_kind: row.get(3)?,
        language: row.get(4)?,
        module: row.get(5)?,
        visibility: row.get(6)?,
        file_path: row.get(7)?,
        start_line: row.get(8)?,
        is_external: row.get(9)?,
        project_id: row.get(10)?,
        decl_kind: row.get(11)?,
        implements_trait: row.get(12)?,
        receiver_type: row.get(13)?,
        entry_point: row.get(14)?,
    })
}

// Convenience used by tests / direct callers that want to surface the
// composed payload as JSON without an MCP roundtrip.
#[doc(hidden)]
pub fn graph_response_lookup(symbols: &[GraphSymbol]) -> HashMap<&str, &GraphSymbol> {
    symbols.iter().map(|s| (s.fqdn.as_str(), s)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::conv::{edge_confidence_to_sql_text, edge_kind_to_sql_text};
    use standardoc_ir::{EdgeConfidence, Kind, LanguageKind, Visibility};
    use tempfile::{TempDir, tempdir};

    fn fresh_handle() -> (TempDir, IndexHandle) {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        (dir, handle)
    }

    fn seed_file(handle: &IndexHandle, path: &str) {
        let conn = handle.pool().unwrap().get().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO files \
               (path, content_hash, language, last_scanned, byte_size, is_external) \
             VALUES (?1, 'aa', 'rust', 0, 0, 0)",
            [path],
        )
        .unwrap();
    }

    fn seed_symbol(handle: &IndexHandle, fqdn: &str, name: &str, kind: Kind, is_external: bool) {
        let conn = handle.pool().unwrap().get().unwrap();
        conn.execute(
            "INSERT INTO symbols \
               (workspace_id, fqdn, name, kind, language_kind, language, module, visibility, \
                file_path, start_line, end_line, start_col, end_col, \
                signature_json, body_hash, is_external, source_origin, \
                last_modified_revision, flags) \
             VALUES (?1, ?2, ?3, ?4, 'fn', 'rust', NULL, 'public', \
                     'src/a.rs', 1, 1, 0, 0, \
                     NULL, NULL, ?5, 'workspace', 0, '[]')",
            rusqlite::params![
                PRIMARY_WORKSPACE_ID,
                fqdn,
                name,
                crate::storage::conv::kind_to_sql_text(kind),
                i64::from(is_external),
            ],
        )
        .unwrap();
    }

    fn seed_resolved_edge(handle: &IndexHandle, from: &str, to: &str, kind: EdgeKind) {
        let conn = handle.pool().unwrap().get().unwrap();
        let from_id: i64 = conn
            .query_row(
                "SELECT id FROM symbols WHERE workspace_id = ?1 AND fqdn = ?2",
                rusqlite::params![PRIMARY_WORKSPACE_ID, from],
                |r| r.get(0),
            )
            .unwrap();
        let to_id: i64 = conn
            .query_row(
                "SELECT id FROM symbols WHERE workspace_id = ?1 AND fqdn = ?2",
                rusqlite::params![PRIMARY_WORKSPACE_ID, to],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO edges \
               (from_symbol_id, kind, to_symbol_id, to_unresolved, \
                attributes, confidence) \
             VALUES (?1, ?2, ?3, NULL, '[]', ?4)",
            rusqlite::params![
                from_id,
                edge_kind_to_sql_text(kind),
                to_id,
                edge_confidence_to_sql_text(EdgeConfidence::Extracted),
            ],
        )
        .unwrap();
    }

    fn req(focal: Option<&str>) -> GraphRequest {
        GraphRequest {
            focal: focal.map(str::to_owned),
            depth: FETCH_GRAPH_DEFAULT_DEPTH,
            kinds: None,
            max_nodes: FETCH_GRAPH_DEFAULT_MAX_NODES,
            include_external: false,
        }
    }

    #[test]
    fn bounded_mode_returns_workspace_symbols_ordered_by_fqdn() {
        let (_d, h) = fresh_handle();
        seed_file(&h, "src/a.rs");
        seed_symbol(&h, "crate::beta", "beta", Kind::Callable, false);
        seed_symbol(&h, "crate::alpha", "alpha", Kind::Callable, false);

        let resp = fetch_graph(&h, req(None)).unwrap();
        let fqdns: Vec<&str> = resp.symbols.iter().map(|s| s.fqdn.as_str()).collect();
        assert_eq!(fqdns, vec!["crate::alpha", "crate::beta"]);
        assert!(resp.edges.is_empty());
        assert!(resp.focal.is_none());
    }

    #[test]
    fn bounded_mode_keeps_only_edges_with_both_ends_in_set() {
        let (_d, h) = fresh_handle();
        seed_file(&h, "src/a.rs");
        seed_symbol(&h, "crate::a", "a", Kind::Callable, false);
        seed_symbol(&h, "crate::b", "b", Kind::Callable, false);
        seed_symbol(&h, "crate::c", "c", Kind::Callable, false);
        seed_resolved_edge(&h, "crate::a", "crate::b", EdgeKind::Calls);

        let mut r = req(None);
        r.max_nodes = 2; // includes a + b only, c is sliced off
        let resp = fetch_graph(&h, r).unwrap();
        assert_eq!(resp.symbols.len(), 2);
        assert_eq!(resp.edges.len(), 1);
        assert_eq!(resp.edges[0].from, "crate::a");
        assert_eq!(resp.edges[0].to, "crate::b");
        assert_eq!(resp.edges[0].kind, EdgeKind::Calls);
        assert!(resp.edges[0].outbound);
    }

    #[test]
    fn bounded_mode_excludes_externals_when_flag_false() {
        let (_d, h) = fresh_handle();
        seed_file(&h, "src/a.rs");
        seed_symbol(&h, "crate::local", "local", Kind::Callable, false);
        seed_symbol(&h, "extern::ext", "ext", Kind::Callable, true);

        let resp = fetch_graph(&h, req(None)).unwrap();
        let fqdns: Vec<&str> = resp.symbols.iter().map(|s| s.fqdn.as_str()).collect();
        assert_eq!(fqdns, vec!["crate::local"]);
    }

    #[test]
    fn focal_mode_unknown_fqdn_returns_empty_payload_with_focal_echo() {
        let (_d, h) = fresh_handle();
        seed_file(&h, "src/a.rs");
        seed_symbol(&h, "crate::a", "a", Kind::Callable, false);

        let resp = fetch_graph(&h, req(Some("crate::missing"))).unwrap();
        assert!(resp.symbols.is_empty());
        assert!(resp.edges.is_empty());
        assert_eq!(resp.focal.as_deref(), Some("crate::missing"));
    }

    #[test]
    fn focal_mode_bfs_collects_outbound_and_inbound_neighbors() {
        let (_d, h) = fresh_handle();
        seed_file(&h, "src/a.rs");
        seed_symbol(&h, "crate::root", "root", Kind::Callable, false);
        seed_symbol(&h, "crate::callee", "callee", Kind::Callable, false);
        seed_symbol(&h, "crate::caller", "caller", Kind::Callable, false);
        seed_resolved_edge(&h, "crate::root", "crate::callee", EdgeKind::Calls);
        seed_resolved_edge(&h, "crate::caller", "crate::root", EdgeKind::Calls);

        let mut r = req(Some("crate::root"));
        r.depth = 1;
        let resp = fetch_graph(&h, r).unwrap();

        let fqdns: HashSet<&str> = resp.symbols.iter().map(|s| s.fqdn.as_str()).collect();
        assert!(fqdns.contains("crate::root"));
        assert!(fqdns.contains("crate::callee"));
        assert!(fqdns.contains("crate::caller"));
        assert_eq!(resp.edges.len(), 2);
        let outbound: Vec<_> = resp.edges.iter().filter(|e| e.outbound).collect();
        let inbound: Vec<_> = resp.edges.iter().filter(|e| !e.outbound).collect();
        assert_eq!(outbound.len(), 1);
        assert_eq!(outbound[0].to, "crate::callee");
        assert_eq!(inbound.len(), 1);
        assert_eq!(inbound[0].from, "crate::caller");
    }

    #[test]
    fn focal_mode_depth_two_expands_beyond_direct_neighbors() {
        let (_d, h) = fresh_handle();
        seed_file(&h, "src/a.rs");
        seed_symbol(&h, "crate::a", "a", Kind::Callable, false);
        seed_symbol(&h, "crate::b", "b", Kind::Callable, false);
        seed_symbol(&h, "crate::c", "c", Kind::Callable, false);
        seed_resolved_edge(&h, "crate::a", "crate::b", EdgeKind::Calls);
        seed_resolved_edge(&h, "crate::b", "crate::c", EdgeKind::Calls);

        let mut r = req(Some("crate::a"));
        r.depth = 1;
        let resp = fetch_graph(&h, r).unwrap();
        let fqdns: HashSet<&str> = resp.symbols.iter().map(|s| s.fqdn.as_str()).collect();
        assert!(fqdns.contains("crate::a"));
        assert!(fqdns.contains("crate::b"));
        assert!(!fqdns.contains("crate::c"));

        let mut r = req(Some("crate::a"));
        r.depth = 2;
        let resp = fetch_graph(&h, r).unwrap();
        let fqdns: HashSet<&str> = resp.symbols.iter().map(|s| s.fqdn.as_str()).collect();
        assert!(fqdns.contains("crate::c"));
    }

    #[test]
    fn focal_mode_kinds_filter_blocks_unwanted_edge_kinds() {
        let (_d, h) = fresh_handle();
        seed_file(&h, "src/a.rs");
        seed_symbol(&h, "crate::a", "a", Kind::Callable, false);
        seed_symbol(&h, "crate::b", "b", Kind::Callable, false);
        seed_symbol(&h, "crate::c", "c", Kind::Callable, false);
        seed_resolved_edge(&h, "crate::a", "crate::b", EdgeKind::Calls);
        seed_resolved_edge(&h, "crate::a", "crate::c", EdgeKind::Imports);

        let mut r = req(Some("crate::a"));
        r.depth = 1;
        r.kinds = Some(HashSet::from([EdgeKind::Calls]));
        let resp = fetch_graph(&h, r).unwrap();

        let fqdns: HashSet<&str> = resp.symbols.iter().map(|s| s.fqdn.as_str()).collect();
        assert!(fqdns.contains("crate::b"));
        assert!(!fqdns.contains("crate::c"));
        assert_eq!(resp.edges.len(), 1);
        assert_eq!(resp.edges[0].kind, EdgeKind::Calls);
    }

    #[test]
    fn focal_mode_skips_unresolved_targets() {
        let (_d, h) = fresh_handle();
        seed_file(&h, "src/a.rs");
        seed_symbol(&h, "crate::a", "a", Kind::Callable, false);
        // Insert an unresolved edge directly (no `to_symbol_id`).
        let conn = h.pool().unwrap().get().unwrap();
        let a_id: i64 = conn
            .query_row("SELECT id FROM symbols WHERE fqdn = 'crate::a'", [], |r| {
                r.get(0)
            })
            .unwrap();
        conn.execute(
            "INSERT INTO edges \
               (from_symbol_id, kind, to_symbol_id, to_unresolved, \
                attributes, confidence) \
             VALUES (?1, 'CALLS', NULL, 'phantom', '[]', 'extracted')",
            [a_id],
        )
        .unwrap();

        let mut r = req(Some("crate::a"));
        r.depth = 1;
        let resp = fetch_graph(&h, r).unwrap();
        assert_eq!(resp.symbols.len(), 1);
        assert!(resp.edges.is_empty());
    }

    #[test]
    fn focal_mode_max_nodes_caps_expansion() {
        let (_d, h) = fresh_handle();
        seed_file(&h, "src/a.rs");
        for i in 0..6 {
            seed_symbol(
                &h,
                &format!("crate::n{i}"),
                &format!("n{i}"),
                Kind::Callable,
                false,
            );
        }
        for i in 0..5 {
            seed_resolved_edge(
                &h,
                "crate::n0",
                &format!("crate::n{}", i + 1),
                EdgeKind::Calls,
            );
        }

        let mut r = req(Some("crate::n0"));
        r.depth = 1;
        r.max_nodes = 3;
        let resp = fetch_graph(&h, r).unwrap();
        assert!(resp.symbols.len() <= 3);
    }

    #[test]
    fn wire_shape_is_flat_with_lowercase_kind_and_visibility() {
        let (_d, h) = fresh_handle();
        seed_file(&h, "src/a.rs");
        seed_symbol(&h, "crate::x", "x", Kind::Callable, false);

        let resp = fetch_graph(&h, req(None)).unwrap();
        let json = serde_json::to_value(&resp).unwrap();
        let entry = &json["symbols"][0];
        assert_eq!(entry["fqdn"], "crate::x");
        assert_eq!(entry["kind"], "callable");
        assert_eq!(entry["visibility"], "public");
        assert_eq!(entry["language_kind"], "fn");
        assert_eq!(entry["file"], "src/a.rs");
        assert_eq!(entry["start_line"], 1);
        assert_eq!(entry["is_external"], false);
        // `location` MUST NOT be nested under symbols.
        assert!(entry.get("location").is_none());
        // K-Step-E: nullable refinement fields are skipped when NULL.
        assert!(entry.get("decl_kind").is_none());
        assert!(entry.get("implements_trait").is_none());
        assert!(entry.get("receiver_type").is_none());
        // Phase 3 (Flow): entry_point is skipped when NULL too.
        assert!(entry.get("entry_point").is_none());
    }

    #[test]
    fn wire_shape_carries_entry_point_when_set() {
        let (_d, h) = fresh_handle();
        seed_file(&h, "src/a.rs");
        seed_symbol(&h, "runme::main", "main", Kind::Callable, false);
        // Backfill the Phase 3 entry_point column directly — the seed
        // helper has no knob for it, mirroring how the K-Step-E test
        // backfills decl_kind / implements_trait / receiver_type.
        let conn = h.pool().unwrap().get().unwrap();
        conn.execute(
            "UPDATE symbols SET entry_point = 'binary_main' \
             WHERE workspace_id = ?1 AND fqdn = 'runme::main'",
            rusqlite::params![PRIMARY_WORKSPACE_ID],
        )
        .unwrap();

        let resp = fetch_graph(&h, req(None)).unwrap();
        let json = serde_json::to_value(&resp).unwrap();
        let entry = &json["symbols"][0];
        assert_eq!(entry["fqdn"], "runme::main");
        assert_eq!(entry["entry_point"], "binary_main");
    }

    #[test]
    fn wire_shape_carries_decl_kind_and_method_refinement_when_set() {
        let (_d, h) = fresh_handle();
        seed_file(&h, "src/a.rs");
        seed_symbol(&h, "crate::Foo::bar", "bar", Kind::Callable, false);
        // Backfill the K-Step-A/C nullable refinement columns directly.
        let conn = h.pool().unwrap().get().unwrap();
        conn.execute(
            "UPDATE symbols SET decl_kind = 'method', implements_trait = 'core::fmt::Debug', \
                                receiver_type = '&Foo' \
             WHERE workspace_id = ?1 AND fqdn = 'crate::Foo::bar'",
            rusqlite::params![PRIMARY_WORKSPACE_ID],
        )
        .unwrap();

        let resp = fetch_graph(&h, req(None)).unwrap();
        let json = serde_json::to_value(&resp).unwrap();
        let entry = &json["symbols"][0];
        assert_eq!(entry["fqdn"], "crate::Foo::bar");
        assert_eq!(entry["decl_kind"], "method");
        assert_eq!(entry["implements_trait"], "core::fmt::Debug");
        assert_eq!(entry["receiver_type"], "&Foo");
    }

    #[test]
    fn edge_kind_serializes_screaming_snake_case() {
        let (_d, h) = fresh_handle();
        seed_file(&h, "src/a.rs");
        seed_symbol(&h, "crate::a", "a", Kind::Callable, false);
        seed_symbol(&h, "crate::b", "b", Kind::Callable, false);
        seed_resolved_edge(&h, "crate::a", "crate::b", EdgeKind::UsesType);

        let resp = fetch_graph(&h, req(None)).unwrap();
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["edges"][0]["kind"], "USES_TYPE");
        // Sanity: untouched IR is also `Calls` not lowercase.
        assert_eq!(
            serde_json::to_string(&EdgeKind::Calls).unwrap(),
            "\"CALLS\""
        );
        // Suppress Visibility unused-import lint when feature flags
        // shake out — we use it transitively through GraphSymbol.
        let _ = Visibility::Public;
        let _ = LanguageKind::from("fn");
    }
}

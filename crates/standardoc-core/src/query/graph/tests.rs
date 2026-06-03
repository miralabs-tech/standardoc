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

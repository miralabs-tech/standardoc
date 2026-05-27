use super::*;
use crate::storage::migrate::ensure_schema;
use crate::storage::module_lookup::put_module_lookup;
use crate::storage::workspace_catalog::register_linked_workspace;
use rusqlite::Connection;
use standardoc_ir::{ImportRecord, IndexingMode, Language, LinkDirection, ModuleLookup};
use tempfile::TempDir;

fn fresh_db_at(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    ensure_schema(&conn).unwrap();
    conn
}

fn fresh_mem_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    ensure_schema(&conn).unwrap();
    conn
}

fn sample_lookup(fqdn: &str, language: Language, origin: &str) -> ModuleLookup {
    let mut m = ModuleLookup::new(fqdn.into(), language);
    m.push_import(ImportRecord {
        local_name: "HashMap".into(),
        origin_module: origin.into(),
        origin_symbol: Some("HashMap".into()),
        is_type_only: false,
        is_re_export: false,
    });
    m
}

/// Set up a temporary peer workspace dir with its own `.standardoc/index.db`
/// populated with `n` sample module_lookups under `workspace_id='primary'`.
fn setup_peer_workspace(modules: &[(&str, Language, &str)]) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let stdoc_dir = dir.path().join(".standardoc");
    std::fs::create_dir_all(&stdoc_dir).unwrap();
    let db_path = stdoc_dir.join("index.db");
    let conn = fresh_db_at(&db_path);
    for (fqdn, lang, origin) in modules {
        let lookup = sample_lookup(fqdn, *lang, origin);
        put_module_lookup(&conn, "primary", &lookup).unwrap();
    }
    dir
}

#[test]
fn stage3b7a_skips_peer_with_missing_standardoc_db() {
    let peer_dir = tempfile::tempdir().unwrap();
    let mut primary = fresh_mem_db();
    let peer_id = register_linked_workspace(
        &primary,
        peer_dir.path().to_string_lossy().as_ref(),
        LinkDirection::In,
        IndexingMode::BlobImport,
    )
    .unwrap();

    let stats = import_active_peer_workspaces(&mut primary).unwrap();
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].workspace_id, peer_id);
    assert_eq!(stats[0].status, PeerImportStatus::SkippedMissing);
    assert_eq!(stats[0].module_lookups_imported, 0);
}

#[test]
fn stage3b7a_imports_peer_module_lookups_under_peer_workspace_id() {
    let peer = setup_peer_workspace(&[
        ("peer::mod_a", Language::Rust, "std::collections"),
        ("peer::mod_b", Language::TypeScript, "react"),
    ]);
    let mut primary = fresh_mem_db();
    let peer_id = register_linked_workspace(
        &primary,
        peer.path().to_string_lossy().as_ref(),
        LinkDirection::In,
        IndexingMode::BlobImport,
    )
    .unwrap();

    let stats = import_active_peer_workspaces(&mut primary).unwrap();
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].status, PeerImportStatus::Imported);
    assert_eq!(stats[0].module_lookups_imported, 2);
    assert_eq!(stats[0].workspace_imports_imported, 2);

    // Verify the rows are tagged with the peer's UUID, not 'primary'.
    let count: i64 = primary
        .query_row(
            "SELECT COUNT(*) FROM module_lookups WHERE workspace_id = ?1",
            params![&peer_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);

    // Primary's own lookups are untouched (zero — fresh DB).
    let primary_count: i64 = primary
        .query_row(
            "SELECT COUNT(*) FROM module_lookups WHERE workspace_id = 'primary'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(primary_count, 0);
}

#[test]
fn stage3b7a_re_import_overwrites_prior_copy_idempotently() {
    let peer = setup_peer_workspace(&[("peer::initial", Language::Rust, "std::collections")]);
    let mut primary = fresh_mem_db();
    let peer_id = register_linked_workspace(
        &primary,
        peer.path().to_string_lossy().as_ref(),
        LinkDirection::In,
        IndexingMode::BlobImport,
    )
    .unwrap();

    // First import.
    let stats1 = import_active_peer_workspaces(&mut primary).unwrap();
    assert_eq!(stats1[0].module_lookups_imported, 1);

    // Repopulate the peer DB with different rows.
    let peer_db = peer.path().join(".standardoc").join("index.db");
    let peer_conn = Connection::open(&peer_db).unwrap();
    // Delete the prior, add 3 new rows.
    peer_conn
        .execute(
            "DELETE FROM module_lookups WHERE workspace_id = 'primary'",
            [],
        )
        .unwrap();
    peer_conn
        .execute(
            "DELETE FROM workspace_imports WHERE workspace_id = 'primary'",
            [],
        )
        .unwrap();
    for fqdn in &["peer::x", "peer::y", "peer::z"] {
        let lookup = sample_lookup(fqdn, Language::Rust, "std::fmt");
        put_module_lookup(&peer_conn, "primary", &lookup).unwrap();
    }
    drop(peer_conn);

    // Second import overwrites cleanly.
    let stats2 = import_active_peer_workspaces(&mut primary).unwrap();
    assert_eq!(stats2[0].module_lookups_imported, 3);

    // Primary now has exactly 3 rows for this peer (not 1+3=4).
    let count: i64 = primary
        .query_row(
            "SELECT COUNT(*) FROM module_lookups WHERE workspace_id = ?1",
            params![&peer_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 3);
}

#[test]
fn stage3b7a_imported_workspace_imports_resolve_via_cross_workspace_query() {
    let peer = setup_peer_workspace(&[("peer::api", Language::Rust, "std::collections")]);
    let mut primary = fresh_mem_db();
    register_linked_workspace(
        &primary,
        peer.path().to_string_lossy().as_ref(),
        LinkDirection::In,
        IndexingMode::BlobImport,
    )
    .unwrap();
    import_active_peer_workspaces(&mut primary).unwrap();

    // workspace_imports_by_origin should return the peer's row.
    let rows =
        crate::storage::module_lookup::workspace_imports_by_origin(&primary, "std::collections")
            .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].module_fqdn, "peer::api");
    assert_ne!(rows[0].workspace_id, "primary");
}

#[test]
fn stage3b7a_updates_last_indexed_at_on_success() {
    let peer = setup_peer_workspace(&[("peer::x", Language::Rust, "std::fmt")]);
    let mut primary = fresh_mem_db();
    let peer_id = register_linked_workspace(
        &primary,
        peer.path().to_string_lossy().as_ref(),
        LinkDirection::In,
        IndexingMode::BlobImport,
    )
    .unwrap();

    // Pre-import: last_indexed_at is NULL.
    let pre: Option<i64> = primary
        .query_row(
            "SELECT last_indexed_at FROM workspace_catalog WHERE workspace_id = ?1",
            params![&peer_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(pre.is_none());

    import_active_peer_workspaces(&mut primary).unwrap();

    // Post-import: last_indexed_at is populated.
    let post: Option<i64> = primary
        .query_row(
            "SELECT last_indexed_at FROM workspace_catalog WHERE workspace_id = ?1",
            params![&peer_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(post.is_some());
    assert!(post.unwrap() > 0);
}

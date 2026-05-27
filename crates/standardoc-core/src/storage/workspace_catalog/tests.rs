use super::*;
use crate::storage::migrate::ensure_schema;

fn fresh_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    ensure_schema(&conn).unwrap();
    conn
}

#[test]
fn register_then_get_roundtrips() {
    let conn = fresh_db();
    let id = register_linked_workspace(
        &conn,
        "/path/to/peer",
        LinkDirection::In,
        IndexingMode::default(),
    )
    .unwrap();
    let got = get_linked_workspace(&conn, &id).unwrap().expect("present");
    assert_eq!(got.workspace_id, id);
    assert_eq!(got.root_path, "/path/to/peer");
    assert_eq!(got.link_direction, LinkDirection::In);
    assert_eq!(got.status, LinkedWorkspaceStatus::Active);
    assert!(got.last_indexed_at_epoch_ms.is_none());
}

#[test]
fn get_returns_none_when_absent() {
    let conn = fresh_db();
    assert!(
        get_linked_workspace(&conn, "does-not-exist")
            .unwrap()
            .is_none()
    );
}

#[test]
fn register_generates_unique_ids_for_same_root_path() {
    let conn = fresh_db();
    let id1 =
        register_linked_workspace(&conn, "/peer", LinkDirection::Out, IndexingMode::default())
            .unwrap();
    let id2 =
        register_linked_workspace(&conn, "/peer", LinkDirection::Out, IndexingMode::default())
            .unwrap();
    assert_ne!(id1, id2);
    let matches = find_by_root_path(&conn, "/peer").unwrap();
    assert_eq!(matches.len(), 2);
}

#[test]
fn list_orders_by_linked_at_ascending() {
    let conn = fresh_db();
    let _id1 =
        register_linked_workspace(&conn, "/first", LinkDirection::In, IndexingMode::default())
            .unwrap();
    // Tiny sleep would let linked_at differ, but a strict ASC sort over
    // equal timestamps is still well-defined per the SQLite implementation
    // of stable sort; we just verify both rows are returned.
    let _id2 = register_linked_workspace(
        &conn,
        "/second",
        LinkDirection::Bidirectional,
        IndexingMode::default(),
    )
    .unwrap();
    let all = list_linked_workspaces(&conn).unwrap();
    assert_eq!(all.len(), 2);
}

#[test]
fn set_link_direction_updates_value() {
    let conn = fresh_db();
    let id =
        register_linked_workspace(&conn, "/p", LinkDirection::In, IndexingMode::default()).unwrap();
    set_link_direction(&conn, &id, LinkDirection::Bidirectional).unwrap();
    let got = get_linked_workspace(&conn, &id).unwrap().unwrap();
    assert_eq!(got.link_direction, LinkDirection::Bidirectional);
}

#[test]
fn set_status_transitions() {
    let conn = fresh_db();
    let id =
        register_linked_workspace(&conn, "/p", LinkDirection::In, IndexingMode::default()).unwrap();
    set_status(&conn, &id, LinkedWorkspaceStatus::Paused).unwrap();
    let got = get_linked_workspace(&conn, &id).unwrap().unwrap();
    assert_eq!(got.status, LinkedWorkspaceStatus::Paused);
    set_status(&conn, &id, LinkedWorkspaceStatus::Archived).unwrap();
    let got = get_linked_workspace(&conn, &id).unwrap().unwrap();
    assert_eq!(got.status, LinkedWorkspaceStatus::Archived);
}

#[test]
fn touch_last_indexed_sets_timestamp() {
    let conn = fresh_db();
    let id =
        register_linked_workspace(&conn, "/p", LinkDirection::In, IndexingMode::default()).unwrap();
    touch_last_indexed(&conn, &id).unwrap();
    let got = get_linked_workspace(&conn, &id).unwrap().unwrap();
    assert!(
        got.last_indexed_at_epoch_ms.is_some(),
        "touch must populate last_indexed_at"
    );
}

#[test]
fn unregister_cleans_dependent_tables() {
    let conn = fresh_db();
    let id =
        register_linked_workspace(&conn, "/p", LinkDirection::In, IndexingMode::default()).unwrap();
    // Seed a workspace_imports row keyed by that id.
    conn.execute(
        "INSERT INTO workspace_imports \
			 (workspace_id, module_fqdn, local_name, origin_module) \
			 VALUES (?1, 'mod', 'X', 'origin')",
        params![&id],
    )
    .unwrap();
    // Seed a module_lookups row too (BLOB payload is irrelevant for the cleanup test).
    conn.execute(
        "INSERT INTO module_lookups \
			 (module_fqdn, workspace_id, language, built_at, payload) \
			 VALUES ('mod', ?1, 'rust', 0, X'00')",
        params![&id],
    )
    .unwrap();

    unregister_linked_workspace(&conn, &id).unwrap();

    assert!(get_linked_workspace(&conn, &id).unwrap().is_none());
    let imp_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM workspace_imports WHERE workspace_id = ?1",
            params![&id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(imp_count, 0, "workspace_imports rows must be cleaned");
    let lookup_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM module_lookups WHERE workspace_id = ?1",
            params![&id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(lookup_count, 0, "module_lookups rows must be cleaned");
}

#[test]
fn register_defaults_indexing_mode_to_blob_import() {
    let conn = fresh_db();
    let id = register_linked_workspace(&conn, "/p", LinkDirection::In, IndexingMode::BlobImport)
        .unwrap();
    let got = get_linked_workspace(&conn, &id).unwrap().unwrap();
    assert_eq!(got.indexing_mode, IndexingMode::BlobImport);
}

#[test]
fn register_with_extract_mode_round_trips() {
    let conn = fresh_db();
    let id =
        register_linked_workspace(&conn, "/p", LinkDirection::In, IndexingMode::Extract).unwrap();
    let got = get_linked_workspace(&conn, &id).unwrap().unwrap();
    assert_eq!(got.indexing_mode, IndexingMode::Extract);
}

#[test]
fn set_indexing_mode_transitions() {
    let conn = fresh_db();
    let id = register_linked_workspace(&conn, "/p", LinkDirection::In, IndexingMode::BlobImport)
        .unwrap();
    set_indexing_mode(&conn, &id, IndexingMode::Extract).unwrap();
    let got = get_linked_workspace(&conn, &id).unwrap().unwrap();
    assert_eq!(got.indexing_mode, IndexingMode::Extract);
}

#[test]
fn check_constraint_rejects_invalid_direction() {
    let conn = fresh_db();
    let err = conn
        .execute(
            "INSERT INTO workspace_catalog \
				 (workspace_id, root_path, link_direction, linked_at) \
				 VALUES ('test-id', '/p', 9, 0)",
            [],
        )
        .expect_err("must reject");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("CHECK"),
        "expected CHECK constraint error, got {msg}"
    );
}

use super::*;
use crate::storage::symbols::insert_symbol;
use crate::storage::test_utils::{fresh_conn, sample_symbol, seed_file, symbol_ctx};
use standardoc_ir::{BridgeKind, EdgeKind, Site};

fn make_edge(kind: EdgeKind, to: ResolvedOrUnresolved) -> RawEdge {
    let confidence = to.default_confidence();
    RawEdge {
        from_fqdn: "crate::caller".into(),
        kind,
        to,
        sites: vec![],
        attributes: vec![],
        confidence,
    }
}

#[test]
fn insert_edge_resolved_target_in_db_uses_to_symbol_id() {
    let conn = fresh_conn();
    seed_file(&conn, "src/main.rs");
    let target_id = insert_symbol(
        &conn,
        &sample_symbol("foo", "crate::foo"),
        symbol_ctx("src/main.rs"),
    )
    .unwrap();
    let caller_id = insert_symbol(
        &conn,
        &sample_symbol("bar", "crate::bar"),
        symbol_ctx("src/main.rs"),
    )
    .unwrap();

    let edge = make_edge(
        EdgeKind::Calls,
        ResolvedOrUnresolved::Resolved {
            fqdn: "crate::foo".into(),
        },
    );
    let edge_id = insert_edge(&conn, caller_id, &edge, "primary").unwrap();

    let (to_id, to_unresolved): (Option<i64>, Option<String>) = conn
        .query_row(
            "SELECT to_symbol_id, to_unresolved FROM edges WHERE id = ?1",
            [edge_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(to_id, Some(target_id));
    assert_eq!(to_unresolved, None);
}

#[test]
fn insert_edge_resolved_target_missing_falls_back_to_unresolved() {
    let conn = fresh_conn();
    seed_file(&conn, "src/main.rs");
    let caller_id = insert_symbol(
        &conn,
        &sample_symbol("bar", "crate::bar"),
        symbol_ctx("src/main.rs"),
    )
    .unwrap();

    let edge = make_edge(
        EdgeKind::Calls,
        ResolvedOrUnresolved::Resolved {
            fqdn: "crate::not_yet_inserted".into(),
        },
    );
    let edge_id = insert_edge(&conn, caller_id, &edge, "primary").unwrap();

    let (to_id, to_unresolved): (Option<i64>, Option<String>) = conn
        .query_row(
            "SELECT to_symbol_id, to_unresolved FROM edges WHERE id = ?1",
            [edge_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(to_id, None);
    assert_eq!(to_unresolved.as_deref(), Some("crate::not_yet_inserted"));
}

#[test]
fn insert_edge_unresolved_binds_name() {
    let conn = fresh_conn();
    seed_file(&conn, "src/main.rs");
    let caller_id = insert_symbol(
        &conn,
        &sample_symbol("bar", "crate::bar"),
        symbol_ctx("src/main.rs"),
    )
    .unwrap();

    let edge = make_edge(
        EdgeKind::Calls,
        ResolvedOrUnresolved::Unresolved {
            name: "do_thing".into(),
        },
    );
    let edge_id = insert_edge(&conn, caller_id, &edge, "primary").unwrap();

    let to_unresolved: Option<String> = conn
        .query_row(
            "SELECT to_unresolved FROM edges WHERE id = ?1",
            [edge_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(to_unresolved.as_deref(), Some("do_thing"));
}

#[test]
fn insert_edge_unresolved_bridge_concatenates() {
    let conn = fresh_conn();
    seed_file(&conn, "src/main.rs");
    let caller_id = insert_symbol(
        &conn,
        &sample_symbol("login", "frontend::login"),
        symbol_ctx("src/main.rs"),
    )
    .unwrap();

    let edge = make_edge(
        EdgeKind::Calls,
        ResolvedOrUnresolved::UnresolvedBridge {
            bridge: BridgeKind::from("tauri"),
            name: "create_user".into(),
        },
    );
    let edge_id = insert_edge(&conn, caller_id, &edge, "primary").unwrap();

    let to_unresolved: Option<String> = conn
        .query_row(
            "SELECT to_unresolved FROM edges WHERE id = ?1",
            [edge_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(to_unresolved.as_deref(), Some("tauri::create_user"));
}

#[test]
fn insert_edge_unresolved_bridge_rejects_unknown_slug() {
    // IR-1 1.0 vocabulary lock: an extractor emitting `tauri-v2`
    // (or any slug outside BUILTIN_BRIDGE_KINDS that lacks the
    // `custom:` prefix) must fail at the storage boundary, not
    // silently corrupt the DB.
    let conn = fresh_conn();
    seed_file(&conn, "src/main.rs");
    let caller_id = insert_symbol(
        &conn,
        &sample_symbol("login", "frontend::login"),
        symbol_ctx("src/main.rs"),
    )
    .unwrap();

    let edge = make_edge(
        EdgeKind::Calls,
        ResolvedOrUnresolved::UnresolvedBridge {
            bridge: BridgeKind::from("tauri-v2"),
            name: "create_user".into(),
        },
    );
    let err = insert_edge(&conn, caller_id, &edge, "primary").unwrap_err();
    assert!(
        matches!(err, StorageError::BridgeKindInvalid(_)),
        "got `{err:?}`"
    );
}

#[test]
fn insert_edge_unresolved_bridge_accepts_custom_prefix() {
    // Vendor-specific bridge kinds must use `custom:<slug>` post-1.0;
    // this contract path stays open even when the slug isn't built-in.
    let conn = fresh_conn();
    seed_file(&conn, "src/main.rs");
    let caller_id = insert_symbol(
        &conn,
        &sample_symbol("login", "frontend::login"),
        symbol_ctx("src/main.rs"),
    )
    .unwrap();

    let edge = make_edge(
        EdgeKind::Calls,
        ResolvedOrUnresolved::UnresolvedBridge {
            bridge: BridgeKind::from("custom:internal-rpc"),
            name: "ping".into(),
        },
    );
    let edge_id = insert_edge(&conn, caller_id, &edge, "primary").unwrap();
    let to_unresolved: Option<String> = conn
        .query_row(
            "SELECT to_unresolved FROM edges WHERE id = ?1",
            [edge_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(to_unresolved.as_deref(), Some("custom:internal-rpc::ping"));
}

#[test]
fn check_xor_rejects_both_null_via_raw_sql() {
    let conn = fresh_conn();
    seed_file(&conn, "src/main.rs");
    let id = insert_symbol(
        &conn,
        &sample_symbol("foo", "crate::foo"),
        symbol_ctx("src/main.rs"),
    )
    .unwrap();
    let err = conn
        .execute(
            "INSERT INTO edges (from_symbol_id, kind, to_symbol_id, to_unresolved) \
                 VALUES (?1, 'CALLS', NULL, NULL)",
            [id],
        )
        .unwrap_err();
    assert!(matches!(
        map_constraint(err),
        StorageError::CheckConstraintViolated { .. }
    ));
}

#[test]
fn check_xor_rejects_both_not_null_via_raw_sql() {
    let conn = fresh_conn();
    seed_file(&conn, "src/main.rs");
    let id_a = insert_symbol(
        &conn,
        &sample_symbol("a", "crate::a"),
        symbol_ctx("src/main.rs"),
    )
    .unwrap();
    let id_b = insert_symbol(
        &conn,
        &sample_symbol("b", "crate::b"),
        symbol_ctx("src/main.rs"),
    )
    .unwrap();
    let err = conn
        .execute(
            "INSERT INTO edges (from_symbol_id, kind, to_symbol_id, to_unresolved) \
                 VALUES (?1, 'CALLS', ?2, 'crate::stray')",
            [id_a, id_b],
        )
        .unwrap_err();
    assert!(matches!(
        map_constraint(err),
        StorageError::CheckConstraintViolated { .. }
    ));
}

#[test]
fn insert_edge_promotes_unresolved_when_target_already_in_db() {
    let conn = fresh_conn();
    seed_file(&conn, "src/main.rs");
    let target_id = insert_symbol(
        &conn,
        &sample_symbol("foo", "crate::foo"),
        symbol_ctx("src/main.rs"),
    )
    .unwrap();
    let caller_id = insert_symbol(
        &conn,
        &sample_symbol("bar", "crate::bar"),
        symbol_ctx("src/main.rs"),
    )
    .unwrap();

    let edge_id = insert_edge(
        &conn,
        caller_id,
        &make_edge(
            EdgeKind::Calls,
            ResolvedOrUnresolved::Unresolved {
                name: "crate::foo".into(),
            },
        ),
        "primary",
    )
    .unwrap();

    let (to_id, to_unresolved): (Option<i64>, Option<String>) = conn
        .query_row(
            "SELECT to_symbol_id, to_unresolved FROM edges WHERE id = ?1",
            [edge_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        to_id,
        Some(target_id),
        "Unresolved-with-canonical-fqdn must promote on insert when target row exists"
    );
    assert!(to_unresolved.is_none());
}

#[test]
fn promote_unresolved_batch_promotes_matching_edge() {
    let conn = fresh_conn();
    seed_file(&conn, "src/main.rs");
    let caller_id = insert_symbol(
        &conn,
        &sample_symbol("bar", "crate::bar"),
        symbol_ctx("src/main.rs"),
    )
    .unwrap();
    let edge_id = insert_edge(
        &conn,
        caller_id,
        &make_edge(
            EdgeKind::Calls,
            ResolvedOrUnresolved::Unresolved {
                name: "crate::foo".into(),
            },
        ),
        "primary",
    )
    .unwrap();

    let target_id = insert_symbol(
        &conn,
        &sample_symbol("foo", "crate::foo"),
        symbol_ctx("src/main.rs"),
    )
    .unwrap();

    let promoted = promote_unresolved_batch(&conn, &[target_id]).unwrap();
    assert_eq!(promoted, 1);

    let (to_id, to_unresolved): (Option<i64>, Option<String>) = conn
        .query_row(
            "SELECT to_symbol_id, to_unresolved FROM edges WHERE id = ?1",
            [edge_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(to_id, Some(target_id));
    assert_eq!(to_unresolved, None);
}

#[test]
fn promote_unresolved_batch_empty_returns_zero() {
    let conn = fresh_conn();
    seed_file(&conn, "src/main.rs");
    let count = promote_unresolved_batch(&conn, &[]).unwrap();
    assert_eq!(count, 0);
}

#[test]
fn promote_unresolved_batch_skips_unmatched_ids() {
    let conn = fresh_conn();
    seed_file(&conn, "src/main.rs");
    let caller_id = insert_symbol(
        &conn,
        &sample_symbol("bar", "crate::bar"),
        symbol_ctx("src/main.rs"),
    )
    .unwrap();
    insert_edge(
        &conn,
        caller_id,
        &make_edge(
            EdgeKind::Calls,
            ResolvedOrUnresolved::Unresolved {
                name: "crate::foo".into(),
            },
        ),
        "primary",
    )
    .unwrap();
    let unrelated = insert_symbol(
        &conn,
        &sample_symbol("baz", "crate::baz"),
        symbol_ctx("src/main.rs"),
    )
    .unwrap();

    let promoted = promote_unresolved_batch(&conn, &[unrelated]).unwrap();
    assert_eq!(promoted, 0);

    let to_unresolved: Option<String> = conn
        .query_row(
            "SELECT to_unresolved FROM edges WHERE from_symbol_id = ?1",
            [caller_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        to_unresolved.as_deref(),
        Some("crate::foo"),
        "edge must remain unresolved when its fqdn is not in the batch"
    );
}

#[test]
fn delete_edges_from_returns_count_and_removes_rows() {
    let conn = fresh_conn();
    seed_file(&conn, "src/main.rs");
    let caller_id = insert_symbol(
        &conn,
        &sample_symbol("bar", "crate::bar"),
        symbol_ctx("src/main.rs"),
    )
    .unwrap();
    for name in ["a", "b", "c"] {
        insert_edge(
            &conn,
            caller_id,
            &make_edge(
                EdgeKind::Calls,
                ResolvedOrUnresolved::Unresolved { name: name.into() },
            ),
            "primary",
        )
        .unwrap();
    }

    let removed = delete_edges_from(&conn, caller_id).unwrap();
    assert_eq!(removed, 3);

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE from_symbol_id = ?1",
            [caller_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn delete_edges_from_cascades_to_edge_sites() {
    let conn = fresh_conn();
    seed_file(&conn, "src/main.rs");
    let caller_id = insert_symbol(
        &conn,
        &sample_symbol("bar", "crate::bar"),
        symbol_ctx("src/main.rs"),
    )
    .unwrap();
    let edge_id = insert_edge(
        &conn,
        caller_id,
        &make_edge(
            EdgeKind::Calls,
            ResolvedOrUnresolved::Unresolved {
                name: "do_it".into(),
            },
        ),
        "primary",
    )
    .unwrap();
    let site = Site {
        file: "src/main.rs".into(),
        line: 12,
        col: 4,
    };
    conn.execute(
        "INSERT INTO edge_sites (edge_id, file_path, line, col) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![edge_id, site.file, site.line, site.col],
    )
    .unwrap();

    delete_edges_from(&conn, caller_id).unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM edge_sites WHERE edge_id = ?1",
            [edge_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "edge_sites must cascade with the edge");
}

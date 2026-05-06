use rusqlite::{Connection, OptionalExtension};
use standardoc_ir::{RawEdge, ResolvedOrUnresolved};

use crate::storage::conv::edge_kind_to_sql_text;
use crate::storage::error::{StorageError, map_constraint};

pub(crate) fn insert_edge(
    conn: &Connection,
    from_symbol_id: i64,
    edge: &RawEdge,
) -> Result<i64, StorageError> {
    let (to_symbol_id, to_unresolved) = resolve_target(conn, &edge.to)?;
    let attributes_json = serde_json::to_string(&edge.attributes)?;
    let id = conn
        .query_row(
            "INSERT INTO edges (from_symbol_id, kind, to_symbol_id, to_unresolved, attributes) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             RETURNING id",
            rusqlite::params![
                from_symbol_id,
                edge_kind_to_sql_text(edge.kind),
                to_symbol_id,
                to_unresolved,
                attributes_json,
            ],
            |row| row.get::<_, i64>(0),
        )
        .map_err(map_constraint)?;
    Ok(id)
}

/// Maps an edge target to its on-disk pair `(to_symbol_id, to_unresolved)`.
///
/// Both `Resolved { fqdn }` and `Unresolved { name }` carry a canonical FQDN
/// the provider believes is the intended target — `Resolved` adds the
/// guarantee that the FQDN was confirmed in the same provider scope (typically
/// the same file's `defined_fqdns`). At the storage layer they collapse to
/// the same operation: look the FQDN up in `symbols`, link by id when present,
/// fall back to `to_unresolved` otherwise. This is the symmetric counterpart
/// to [`promote_unresolved_batch`]: that handles "target arrives later",
/// this handles "target was already in DB when caller's edge was inserted".
fn resolve_target(
    conn: &Connection,
    target: &ResolvedOrUnresolved,
) -> Result<(Option<i64>, Option<String>), StorageError> {
    match target {
        ResolvedOrUnresolved::Resolved { fqdn } => lookup_or_fallback(conn, fqdn),
        ResolvedOrUnresolved::Unresolved { name } => lookup_or_fallback(conn, name),
        ResolvedOrUnresolved::UnresolvedBridge { bridge, name } => {
            lookup_or_fallback(conn, &format!("{}::{}", bridge.as_str(), name))
        }
    }
}

fn lookup_or_fallback(
    conn: &Connection,
    fqdn: &str,
) -> Result<(Option<i64>, Option<String>), StorageError> {
    let id: Option<i64> = conn
        .query_row("SELECT id FROM symbols WHERE fqdn = ?1", [fqdn], |row| {
            row.get(0)
        })
        .optional()?;
    Ok(match id {
        Some(rid) => (Some(rid), None),
        None => (None, Some(fqdn.to_string())),
    })
}

/// Promotes any edge whose `to_unresolved` matches the fqdn of one of the
/// freshly inserted symbols (DDL §3.3). Returns the number of edges promoted.
pub(crate) fn promote_unresolved_batch(
    conn: &Connection,
    new_symbol_ids: &[i64],
) -> Result<u64, StorageError> {
    if new_symbol_ids.is_empty() {
        return Ok(0);
    }
    let placeholders = (1..=new_symbol_ids.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "UPDATE edges \
         SET to_symbol_id = (SELECT id FROM symbols WHERE fqdn = edges.to_unresolved), \
             to_unresolved = NULL \
         WHERE to_unresolved IN ( \
             SELECT fqdn FROM symbols WHERE id IN ({placeholders}) \
         )"
    );
    let params = rusqlite::params_from_iter(new_symbol_ids.iter().copied());
    conn.execute(&sql, params)?;
    Ok(conn.changes())
}

pub(crate) fn delete_edges_from(
    conn: &Connection,
    from_symbol_id: i64,
) -> Result<u64, StorageError> {
    conn.execute(
        "DELETE FROM edges WHERE from_symbol_id = ?1",
        [from_symbol_id],
    )?;
    Ok(conn.changes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::symbols::insert_symbol;
    use crate::storage::test_utils::{fresh_conn, sample_symbol, seed_file, symbol_ctx};
    use standardoc_ir::{BridgeKind, EdgeKind, Site};

    fn make_edge(kind: EdgeKind, to: ResolvedOrUnresolved) -> RawEdge {
        RawEdge {
            from_fqdn: "crate::caller".into(),
            kind,
            to,
            sites: vec![],
            attributes: vec![],
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
        let edge_id = insert_edge(&conn, caller_id, &edge).unwrap();

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
        let edge_id = insert_edge(&conn, caller_id, &edge).unwrap();

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
        let edge_id = insert_edge(&conn, caller_id, &edge).unwrap();

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
        let edge_id = insert_edge(&conn, caller_id, &edge).unwrap();

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
}

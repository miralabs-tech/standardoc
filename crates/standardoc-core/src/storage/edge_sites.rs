use rusqlite::Connection;
use standardoc_ir::Site;

use crate::storage::error::StorageError;

pub(crate) fn insert_edge_sites(
    conn: &Connection,
    edge_id: i64,
    sites: &[Site],
) -> Result<(), StorageError> {
    if sites.is_empty() {
        return Ok(());
    }
    let mut stmt = conn.prepare(
        "INSERT OR IGNORE INTO edge_sites (edge_id, file_path, line, col) \
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    for site in sites {
        stmt.execute(rusqlite::params![edge_id, site.file, site.line, site.col])?;
    }
    Ok(())
}

pub(crate) fn delete_edge_sites_by_file(
    conn: &Connection,
    file_path: &str,
) -> Result<u64, StorageError> {
    conn.execute("DELETE FROM edge_sites WHERE file_path = ?1", [file_path])?;
    Ok(conn.changes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::edges::insert_edge;
    use crate::storage::symbols::insert_symbol;
    use crate::storage::test_utils::{fresh_conn, sample_symbol, seed_file, symbol_ctx};
    use standardoc_ir::{EdgeConfidence, EdgeKind, RawEdge, ResolvedOrUnresolved};

    fn seed_edge(conn: &Connection) -> i64 {
        seed_file(conn, "src/main.rs");
        let caller_id = insert_symbol(
            conn,
            &sample_symbol("bar", "crate::bar"),
            symbol_ctx("src/main.rs"),
        )
        .unwrap();
        insert_edge(
            conn,
            caller_id,
            &RawEdge {
                from_fqdn: "crate::bar".into(),
                kind: EdgeKind::Calls,
                to: ResolvedOrUnresolved::Unresolved {
                    name: "do_it".into(),
                },
                sites: vec![],
                attributes: vec![],
                confidence: EdgeConfidence::Ambiguous,
            },
        )
        .unwrap()
    }

    #[test]
    fn insert_edge_sites_round_trip() {
        let conn = fresh_conn();
        let edge_id = seed_edge(&conn);
        let sites = vec![
            Site {
                file: "src/main.rs".into(),
                line: 10,
                col: 4,
            },
            Site {
                file: "src/main.rs".into(),
                line: 20,
                col: 8,
            },
        ];
        insert_edge_sites(&conn, edge_id, &sites).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM edge_sites WHERE edge_id = ?1",
                [edge_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn insert_edge_sites_empty_is_noop() {
        let conn = fresh_conn();
        let edge_id = seed_edge(&conn);
        insert_edge_sites(&conn, edge_id, &[]).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM edge_sites WHERE edge_id = ?1",
                [edge_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn insert_edge_sites_or_ignore_swallows_duplicates() {
        let conn = fresh_conn();
        let edge_id = seed_edge(&conn);
        let site = Site {
            file: "src/main.rs".into(),
            line: 10,
            col: 4,
        };
        insert_edge_sites(&conn, edge_id, std::slice::from_ref(&site)).unwrap();
        insert_edge_sites(&conn, edge_id, std::slice::from_ref(&site)).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM edge_sites WHERE edge_id = ?1",
                [edge_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "PK collision must be ignored, not duplicated");
    }

    #[test]
    fn delete_edge_sites_by_file_targets_only_matching_path() {
        let conn = fresh_conn();
        let edge_id = seed_edge(&conn);
        let sites = vec![
            Site {
                file: "src/main.rs".into(),
                line: 1,
                col: 0,
            },
            Site {
                file: "src/lib.rs".into(),
                line: 2,
                col: 0,
            },
            Site {
                file: "src/main.rs".into(),
                line: 3,
                col: 0,
            },
        ];
        insert_edge_sites(&conn, edge_id, &sites).unwrap();

        let removed = delete_edge_sites_by_file(&conn, "src/main.rs").unwrap();
        assert_eq!(removed, 2);

        let remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM edge_sites WHERE edge_id = ?1",
                [edge_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 1);
    }

    #[test]
    fn delete_edge_sites_by_file_missing_returns_zero() {
        let conn = fresh_conn();
        let removed = delete_edge_sites_by_file(&conn, "ghost.rs").unwrap();
        assert_eq!(removed, 0);
    }
}

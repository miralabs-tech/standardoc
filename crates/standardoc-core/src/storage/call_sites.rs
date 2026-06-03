//! IR-4-f — storage layer for `RawCallSite` records.
//!
//! Mirrors the `documents` / `edges` modules' shape: insert + delete +
//! count helpers, no query helpers here (those live under `query/` once
//! we wire the MCP read-path in a follow-up).
//!
//! Lifecycle:
//! - `pipeline::batch::apply_upsert_file` drives the path. On every
//!   file upsert it calls `delete_call_sites_by_file(file_path)` then
//!   batch-inserts the freshly-extracted `extracted.call_sites` vec.
//! - File-level deletion / re-extraction is handled by SQLite's
//!   `ON DELETE CASCADE` on `file_path REFERENCES files(path)` —
//!   removing the parent `files` row drops the dependent call_sites in
//!   one stroke. No explicit cleanup needed in the watcher's delete
//!   branch.
//!
//! The `from_fqdn` column is intentionally NOT an FK to `symbols.fqdn`:
//! a call_site's enclosing FQDN may not have a symbol row yet (top-
//! level expressions, synthetic module scopes), same rationale as
//! `edges.to_unresolved`.

use rusqlite::Connection;
use standardoc_ir::RawCallSite;

use crate::storage::error::StorageError;

/// Insert one [`RawCallSite`] row keyed by `file_path`. The IR's
/// `site.file` field is ignored here — we trust the caller (batch
/// pipeline) to pass the canonical workspace-relative path so all
/// call_sites for a given file share the same `file_path` value and
/// the `idx_call_sites_file_path` index stays selective.
///
/// Returns the new row id. Errors propagate verbatim — the caller
/// decides whether to abort the file-level transaction or log and
/// continue.
pub(crate) fn insert_call_site(
    conn: &Connection,
    file_path: &str,
    cs: &RawCallSite,
) -> Result<i64, StorageError> {
    let args_json = serde_json::to_string(&cs.args)?;
    let receiver_chain_json = serde_json::to_string(&cs.receiver_chain)?;
    let id = conn.query_row(
        "INSERT INTO call_sites \
             (from_fqdn, callee_text, args_json, receiver_chain_json, file_path, line, col) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
             RETURNING id",
        rusqlite::params![
            cs.from_fqdn,
            cs.callee_text,
            args_json,
            receiver_chain_json,
            file_path,
            cs.site.line,
            cs.site.col,
        ],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(id)
}

/// Drop every call_site keyed by `file_path`. Called by the batch
/// pipeline before re-inserting the freshly-extracted set on each
/// file upsert.
///
/// Returns the count of removed rows (passes through `Connection::changes`).
pub(crate) fn delete_call_sites_by_file(
    conn: &Connection,
    file_path: &str,
) -> Result<u64, StorageError> {
    conn.execute("DELETE FROM call_sites WHERE file_path = ?1", [file_path])?;
    Ok(conn.changes())
}

/// Inventory helper used by tests + ops dashboards to assert the call_sites
/// vec produced by the extractor reached the DB intact. Not used on the
/// hot read path — keep it `pub(crate)` so the public surface stays
/// minimal until the plugin-layer query module lands.
#[cfg(test)]
pub(crate) fn count_call_sites_by_file(
    conn: &Connection,
    file_path: &str,
) -> Result<i64, StorageError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM call_sites WHERE file_path = ?1",
        [file_path],
        |row| row.get(0),
    )?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_utils::fresh_conn;
    use standardoc_ir::{RawCallArg, Site};

    fn seed_file(conn: &Connection, path: &str) {
        conn.execute(
            "INSERT INTO files (path, content_hash, language, last_scanned, byte_size) \
             VALUES (?1, 'aa', 'rust', 0, 0)",
            [path],
        )
        .unwrap();
    }

    fn sample_call_site(from_fqdn: &str, callee: &str) -> RawCallSite {
        RawCallSite {
            from_fqdn: from_fqdn.into(),
            callee_text: callee.into(),
            args: vec![RawCallArg {
                value: "hi".into(),
                is_string_literal: true,
            }],
            receiver_chain: vec!["obj".into()],
            site: Site {
                file: "src/lib.rs".into(),
                line: 42,
                col: 8,
            },
        }
    }

    #[test]
    fn insert_call_site_persists_all_fields() {
        let conn = fresh_conn();
        seed_file(&conn, "src/lib.rs");
        let cs = sample_call_site("crate::caller", "obj.bar");
        let id = insert_call_site(&conn, "src/lib.rs", &cs).unwrap();

        let (from_fqdn, callee, args_json, chain_json, file_path, line, col): (
            String,
            String,
            String,
            String,
            String,
            i64,
            i64,
        ) = conn
            .query_row(
                "SELECT from_fqdn, callee_text, args_json, receiver_chain_json, \
                        file_path, line, col \
                 FROM call_sites WHERE id = ?1",
                [id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(from_fqdn, "crate::caller");
        assert_eq!(callee, "obj.bar");
        assert!(args_json.contains("\"hi\""));
        assert!(args_json.contains("\"is_string_literal\":true"));
        assert!(chain_json.contains("\"obj\""));
        assert_eq!(file_path, "src/lib.rs");
        assert_eq!(line, 42);
        assert_eq!(col, 8);
    }

    #[test]
    fn insert_call_site_with_empty_vecs_serializes_to_empty_json_arrays() {
        // Free-fn `foo()` with no args, no receiver — the JSON columns
        // must land as `"[]"`, not NULL.
        let conn = fresh_conn();
        seed_file(&conn, "src/main.rs");
        let cs = RawCallSite {
            from_fqdn: "crate::caller".into(),
            callee_text: "foo".into(),
            args: vec![],
            receiver_chain: vec![],
            site: Site {
                file: "src/main.rs".into(),
                line: 1,
                col: 0,
            },
        };
        let id = insert_call_site(&conn, "src/main.rs", &cs).unwrap();
        let (args_json, chain_json): (String, String) = conn
            .query_row(
                "SELECT args_json, receiver_chain_json FROM call_sites WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(args_json, "[]");
        assert_eq!(chain_json, "[]");
    }

    #[test]
    fn delete_call_sites_by_file_removes_matching_rows_only() {
        let conn = fresh_conn();
        seed_file(&conn, "src/a.rs");
        seed_file(&conn, "src/b.rs");
        insert_call_site(&conn, "src/a.rs", &sample_call_site("c::a1", "x")).unwrap();
        insert_call_site(&conn, "src/a.rs", &sample_call_site("c::a2", "y")).unwrap();
        insert_call_site(&conn, "src/b.rs", &sample_call_site("c::b1", "z")).unwrap();

        let removed = delete_call_sites_by_file(&conn, "src/a.rs").unwrap();
        assert_eq!(removed, 2);

        assert_eq!(count_call_sites_by_file(&conn, "src/a.rs").unwrap(), 0);
        assert_eq!(count_call_sites_by_file(&conn, "src/b.rs").unwrap(), 1);
    }

    #[test]
    fn delete_call_sites_by_file_no_op_when_no_match() {
        let conn = fresh_conn();
        seed_file(&conn, "src/a.rs");
        let removed = delete_call_sites_by_file(&conn, "src/nowhere.rs").unwrap();
        assert_eq!(removed, 0);
    }

    #[test]
    fn file_delete_cascades_to_call_sites() {
        // The FK `file_path REFERENCES files(path) ON DELETE CASCADE`
        // means dropping a file row purges its call_sites without an
        // explicit DELETE FROM call_sites — important so the watcher's
        // delete-path doesn't have to know about every dependent table.
        let conn = fresh_conn();
        seed_file(&conn, "src/lib.rs");
        insert_call_site(&conn, "src/lib.rs", &sample_call_site("c::a", "x")).unwrap();
        assert_eq!(count_call_sites_by_file(&conn, "src/lib.rs").unwrap(), 1);

        conn.execute("DELETE FROM files WHERE path = ?1", ["src/lib.rs"])
            .unwrap();
        assert_eq!(count_call_sites_by_file(&conn, "src/lib.rs").unwrap(), 0);
    }

    #[test]
    fn insert_rejects_call_site_for_missing_file_via_fk() {
        // Defensive — caller is supposed to upsert the parent file row
        // first. If they don't, the FK constraint must fire so the bug
        // is loud, not silently inserting an orphan row.
        let conn = fresh_conn();
        let cs = sample_call_site("c::a", "x");
        let err = insert_call_site(&conn, "src/never_seeded.rs", &cs).unwrap_err();
        assert!(matches!(err, StorageError::Sqlite(_)), "got `{err:?}`");
    }
}

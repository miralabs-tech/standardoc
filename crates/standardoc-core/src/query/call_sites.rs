//! Read-side queries over the `call_sites` table populated by IR-4-f.
//!
//! The plugin layer (and any AI tool surface) reads textual call shapes
//! from here without re-parsing source. Three filter axes compose:
//!
//! - `from_fqdn` — exact match on the enclosing fn/method FQDN.
//!   Answers "what does X call?".
//! - `callee_text` — exact match on the dotted callee text
//!   (`tauri::invoke`, `obj.api.create`, `print`, `printlnǃ`).
//!   Answers "who calls Y?".
//! - `callee_pattern` — SQLite GLOB on the callee text
//!   (`*tauri.invoke*`, `*.create`, `M.api.*`). Useful for bridge
//!   scanning and method-name shape queries.
//!
//! All three filters AND-compose. Calling with none returns the most
//! recent N call_sites workspace-wide (LIMIT cap), useful for ops /
//! debugging dashboards.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use standardoc_ir::{RawCallArg, RawCallSite, Site};

use crate::storage::error::StorageError;
use crate::storage::handle::IndexHandle;

/// Hydrated [`RawCallSite`] re-read from the `call_sites` table.
/// Wraps the IR type as-is — the storage path round-trips identical
/// shapes via `args_json` / `receiver_chain_json`. Wrapped in a public
/// alias so callers don't import from `standardoc_ir` directly for what
/// is now a query-shape contract.
pub type CallSiteRow = RawCallSite;

/// Optional filter trio for [`find_call_sites`]. Each field AND-composes;
/// `None` means "no filter on this axis". The natural-language semantics:
/// - `from_fqdn: Some("crate::caller")` AND `callee_text: Some("tauri::invoke")`
///   → "where does `crate::caller` invoke Tauri?"
/// - `callee_pattern: Some("*tauri.invoke*")` alone → "every Tauri
///   invocation workspace-wide".
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallSiteFilters {
    pub from_fqdn: Option<String>,
    pub callee_text: Option<String>,
    pub callee_pattern: Option<String>,
}

impl CallSiteFilters {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.from_fqdn.is_none()
            && self.callee_text.is_none()
            && self.callee_pattern.is_none()
    }
}

/// Hard cap on result-set size, matching `find_symbol`'s ceiling so
/// downstream consumers have a uniform protection against runaway
/// queries on huge workspaces.
pub const FIND_CALL_SITES_MAX_LIMIT: u32 = 200;

/// Default returned size when the caller omits `limit`. Tuned for
/// interactive use (AI agent chat-style) — large enough to be useful,
/// small enough to keep response payloads under a few KB.
pub const FIND_CALL_SITES_DEFAULT_LIMIT: u32 = 50;

/// Find call_sites matching the AND-composition of `filters`.
///
/// `limit` is clamped to `[1, FIND_CALL_SITES_MAX_LIMIT]`; passing 0
/// returns the empty vec without hitting SQL. Ordering is by id ASC so
/// re-runs against an unchanged DB are deterministic — extractor emit
/// order is preserved per file, files are interleaved by insertion order.
pub fn find_call_sites(
    handle: &IndexHandle,
    filters: &CallSiteFilters,
    limit: u32,
) -> Result<Vec<CallSiteRow>, StorageError> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let limit = limit.min(FIND_CALL_SITES_MAX_LIMIT);
    let pool = handle.pool()?;
    let conn = pool.get()?;
    find_call_sites_conn(&conn, filters, limit)
}

/// File-scoped lookup — every call_site emitted by the extractor for
/// `file_path`, in source-emission order (id ASC). Useful for the
/// post-extract diagnostic dashboard and for plugins that index per-
/// file rather than per-FQDN.
pub fn call_sites_by_file(
    handle: &IndexHandle,
    file_path: &str,
) -> Result<Vec<CallSiteRow>, StorageError> {
    let pool = handle.pool()?;
    let conn = pool.get()?;
    let sql = "SELECT from_fqdn, callee_text, args_json, receiver_chain_json, \
                      file_path, line, col \
               FROM call_sites WHERE file_path = ?1 ORDER BY id ASC";
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map([file_path], read_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter().map(hydrate).collect()
}

fn find_call_sites_conn(
    conn: &Connection,
    filters: &CallSiteFilters,
    limit: u32,
) -> Result<Vec<CallSiteRow>, StorageError> {
    let mut sql = String::from(
        "SELECT from_fqdn, callee_text, args_json, receiver_chain_json, \
                file_path, line, col \
         FROM call_sites WHERE 1=1",
    );
    let mut params: Vec<rusqlite::types::Value> = Vec::new();
    if let Some(f) = filters.from_fqdn.as_deref() {
        sql.push_str(" AND from_fqdn = ?");
        params.push(f.to_string().into());
    }
    if let Some(c) = filters.callee_text.as_deref() {
        sql.push_str(" AND callee_text = ?");
        params.push(c.to_string().into());
    }
    if let Some(p) = filters.callee_pattern.as_deref() {
        sql.push_str(" AND callee_text GLOB ?");
        params.push(p.to_string().into());
    }
    sql.push_str(" ORDER BY id ASC LIMIT ?");
    params.push(i64::from(limit).into());

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), read_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter().map(hydrate).collect()
}

struct RawRow {
    from_fqdn: String,
    callee_text: String,
    args_json: String,
    receiver_chain_json: String,
    file_path: String,
    line: u32,
    col: u32,
}

fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawRow> {
    Ok(RawRow {
        from_fqdn: row.get(0)?,
        callee_text: row.get(1)?,
        args_json: row.get(2)?,
        receiver_chain_json: row.get(3)?,
        file_path: row.get(4)?,
        line: row.get(5)?,
        col: row.get(6)?,
    })
}

fn hydrate(row: RawRow) -> Result<CallSiteRow, StorageError> {
    let args: Vec<RawCallArg> = serde_json::from_str(&row.args_json)?;
    let receiver_chain: Vec<String> = serde_json::from_str(&row.receiver_chain_json)?;
    Ok(RawCallSite {
        from_fqdn: row.from_fqdn,
        callee_text: row.callee_text,
        args,
        receiver_chain,
        site: Site {
            file: row.file_path,
            line: row.line,
            col: row.col,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::call_sites::insert_call_site;
    use rusqlite::Connection;
    use standardoc_ir::Site as IrSite;
    use tempfile::tempdir;

    fn fresh_handle() -> (tempfile::TempDir, IndexHandle) {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        (dir, handle)
    }

    fn seed_file(handle: &IndexHandle, path: &str) {
        let conn = handle.pool().unwrap().get().unwrap();
        conn.execute(
            "INSERT INTO files (path, content_hash, language, last_scanned, byte_size) \
             VALUES (?1, 'aa', 'rust', 0, 0)",
            [path],
        )
        .unwrap();
    }

    fn cs(from: &str, callee: &str, file: &str, line: u32) -> CallSiteRow {
        RawCallSite {
            from_fqdn: from.into(),
            callee_text: callee.into(),
            args: vec![],
            receiver_chain: vec![],
            site: IrSite {
                file: file.into(),
                line,
                col: 0,
            },
        }
    }

    fn insert(handle: &IndexHandle, file_path: &str, cs: &CallSiteRow) {
        let conn = handle.pool().unwrap().get().unwrap();
        insert_call_site(&conn, file_path, cs).unwrap();
    }

    #[test]
    fn find_call_sites_no_filter_returns_up_to_limit_in_id_order() {
        let (_d, h) = fresh_handle();
        seed_file(&h, "src/a.rs");
        for i in 0..5 {
            insert(&h, "src/a.rs", &cs("c::caller", &format!("foo_{i}"), "src/a.rs", i));
        }
        let rows = find_call_sites(&h, &CallSiteFilters::default(), 3).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].callee_text, "foo_0");
        assert_eq!(rows[2].callee_text, "foo_2");
    }

    #[test]
    fn find_call_sites_filter_by_from_fqdn_matches_exact() {
        let (_d, h) = fresh_handle();
        seed_file(&h, "src/a.rs");
        insert(&h, "src/a.rs", &cs("c::a", "foo", "src/a.rs", 1));
        insert(&h, "src/a.rs", &cs("c::b", "foo", "src/a.rs", 2));
        let rows = find_call_sites(
            &h,
            &CallSiteFilters {
                from_fqdn: Some("c::a".into()),
                ..Default::default()
            },
            10,
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].from_fqdn, "c::a");
    }

    #[test]
    fn find_call_sites_filter_by_callee_text_matches_exact() {
        let (_d, h) = fresh_handle();
        seed_file(&h, "src/a.rs");
        insert(&h, "src/a.rs", &cs("c::a", "tauri::invoke", "src/a.rs", 1));
        insert(&h, "src/a.rs", &cs("c::a", "foo", "src/a.rs", 2));
        let rows = find_call_sites(
            &h,
            &CallSiteFilters {
                callee_text: Some("tauri::invoke".into()),
                ..Default::default()
            },
            10,
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].callee_text, "tauri::invoke");
    }

    #[test]
    fn find_call_sites_filter_by_callee_pattern_matches_glob() {
        let (_d, h) = fresh_handle();
        seed_file(&h, "src/a.rs");
        insert(&h, "src/a.rs", &cs("c::a", "M.api.create", "src/a.rs", 1));
        insert(&h, "src/a.rs", &cs("c::a", "M.api.delete", "src/a.rs", 2));
        insert(&h, "src/a.rs", &cs("c::a", "foo", "src/a.rs", 3));
        let rows = find_call_sites(
            &h,
            &CallSiteFilters {
                callee_pattern: Some("M.api.*".into()),
                ..Default::default()
            },
            10,
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|r| r.callee_text == "M.api.create"));
        assert!(rows.iter().any(|r| r.callee_text == "M.api.delete"));
    }

    #[test]
    fn find_call_sites_filters_compose_via_and() {
        let (_d, h) = fresh_handle();
        seed_file(&h, "src/a.rs");
        insert(&h, "src/a.rs", &cs("c::a", "tauri::invoke", "src/a.rs", 1));
        insert(&h, "src/a.rs", &cs("c::b", "tauri::invoke", "src/a.rs", 2));
        insert(&h, "src/a.rs", &cs("c::a", "foo", "src/a.rs", 3));
        let rows = find_call_sites(
            &h,
            &CallSiteFilters {
                from_fqdn: Some("c::a".into()),
                callee_text: Some("tauri::invoke".into()),
                ..Default::default()
            },
            10,
        )
        .unwrap();
        assert_eq!(
            rows.len(),
            1,
            "AND-composition must keep only the row matching both filters"
        );
        assert_eq!(rows[0].from_fqdn, "c::a");
        assert_eq!(rows[0].callee_text, "tauri::invoke");
    }

    #[test]
    fn find_call_sites_zero_limit_returns_empty_without_sql() {
        let (_d, h) = fresh_handle();
        seed_file(&h, "src/a.rs");
        insert(&h, "src/a.rs", &cs("c::a", "foo", "src/a.rs", 1));
        let rows = find_call_sites(&h, &CallSiteFilters::default(), 0).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn find_call_sites_limit_clamps_to_max() {
        let (_d, h) = fresh_handle();
        seed_file(&h, "src/a.rs");
        for i in 0..5 {
            insert(&h, "src/a.rs", &cs("c::a", &format!("f{i}"), "src/a.rs", i));
        }
        // Pass a deliberately-too-large limit; the helper must cap to
        // `FIND_CALL_SITES_MAX_LIMIT` so a single bad caller can't
        // smuggle through a 50_000-row scan.
        let rows = find_call_sites(&h, &CallSiteFilters::default(), 9999).unwrap();
        assert_eq!(rows.len(), 5, "all 5 rows surface (under the cap)");
    }

    #[test]
    fn call_sites_by_file_returns_rows_in_id_order() {
        let (_d, h) = fresh_handle();
        seed_file(&h, "src/a.rs");
        seed_file(&h, "src/b.rs");
        insert(&h, "src/a.rs", &cs("c::a", "alpha", "src/a.rs", 1));
        insert(&h, "src/b.rs", &cs("c::b", "beta", "src/b.rs", 2));
        insert(&h, "src/a.rs", &cs("c::a", "gamma", "src/a.rs", 3));
        let rows = call_sites_by_file(&h, "src/a.rs").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].callee_text, "alpha");
        assert_eq!(rows[1].callee_text, "gamma");
    }

    #[test]
    fn hydrate_round_trips_args_and_receiver_chain_through_json() {
        let (_d, h) = fresh_handle();
        seed_file(&h, "src/a.rs");
        let original = RawCallSite {
            from_fqdn: "c::caller".into(),
            callee_text: "obj.api.create".into(),
            args: vec![
                RawCallArg {
                    value: "hi".into(),
                    is_string_literal: true,
                },
                RawCallArg {
                    value: "42".into(),
                    is_string_literal: false,
                },
            ],
            receiver_chain: vec!["obj".into(), "api".into()],
            site: IrSite {
                file: "src/a.rs".into(),
                line: 12,
                col: 4,
            },
        };
        insert(&h, "src/a.rs", &original);
        let rows = call_sites_by_file(&h, "src/a.rs").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], original);
    }
}

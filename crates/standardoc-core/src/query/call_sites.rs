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
    pub const fn is_empty(&self) -> bool {
        self.from_fqdn.is_none() && self.callee_text.is_none() && self.callee_pattern.is_none()
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
mod tests;

//! Stage X — post-cold-start unresolved-edge sweep.
//!
//! The per-file `cross_workspace_post` invocation inside
//! `reindex::process_one` runs synchronously *before* later chunks
//! commit their `module_lookups` rows. Files extracted early therefore
//! see an incomplete cross-crate lookup state and leave edges as
//! `Unresolved` even when a sibling crate would have answered the
//! query later.
//!
//! This pass runs once at the end of `cold_start::run` (after every
//! module_lookup is committed) and re-runs the [`DbCrossWorkspaceResolver`]
//! against every `edges` row whose `to_unresolved IS NOT NULL`. Hits
//! get rewritten to `to_symbol_id` after looking up the resolved
//! FQDN in `symbols`. Best-effort: failures log and let cold_start
//! finish without blocking.
//!
//! Idempotent — running it twice on the same DB is a no-op because
//! the second pass sees no remaining unresolved-with-matching-symbol
//! edges to rewrite.

use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension};
use standardoc_ir::CrossWorkspaceLookup;

use crate::cross_workspace_resolver::DbCrossWorkspaceResolver;
use crate::pipeline::cross_workspace_post::resolve_with_suffix_chain;
use crate::storage::error::StorageError;
use crate::storage::handle::IndexHandle;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResolveReport {
    pub resolved: usize,
    pub still_unresolved: usize,
    pub duplicate_skipped: usize,
}

pub(crate) fn apply_resolve_unresolved_quietly(handle: &IndexHandle) {
    match apply_resolve_unresolved(handle) {
        Ok(report) => {
            if report.resolved > 0 || report.duplicate_skipped > 0 {
                eprintln!(
                    "standardoc unresolved-edge sweep: {} resolved, {} dup-skipped, {} still unresolved",
                    report.resolved, report.duplicate_skipped, report.still_unresolved,
                );
            }
        }
        Err(e) => eprintln!("standardoc unresolved-edge sweep: {e}"),
    }
}

fn apply_resolve_unresolved(handle: &IndexHandle) -> Result<ResolveReport, StorageError> {
    let resolver = DbCrossWorkspaceResolver::new(handle);
    let pool = handle.pool()?;
    let conn = pool.get().map_err(StorageError::from)?;

    // Pull every unresolved edge in one shot. ~30k rows on standardoc
    // — fits in memory comfortably and avoids holding a statement
    // borrow across the resolver calls (which would re-borrow the
    // same pool).
    let unresolved: Vec<(i64, String)> = {
        let mut stmt =
            conn.prepare("SELECT id, to_unresolved FROM edges WHERE to_unresolved IS NOT NULL")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<Result<_, _>>()?
    };

    // Compute resolutions FIRST (separate pass) so we don't interleave
    // resolver lookups with DB writes — resolver may borrow its own
    // conn from the pool.
    let mut id_to_symbol_id: Vec<(i64, i64)> = Vec::new();
    let mut still_unresolved = 0usize;
    let mut fqdn_cache: HashMap<String, Option<i64>> = HashMap::new();
    for (edge_id, raw_name) in unresolved {
        // Bug E-2: walk split points longest-module-first and append any
        // remaining tail to the resolver's hit FQDN. Without this, edges
        // pointing through a re-export (e.g. `lur_common::Span::new`
        // when `Span` is `pub use`-ed from `lur-common::span`) stay
        // unresolved because `rsplit_once` asks the resolver about a
        // non-module prefix.
        let Some(lookup) = resolve_with_suffix_chain(&resolver, &raw_name) else {
            still_unresolved += 1;
            continue;
        };
        let CrossWorkspaceLookup::Hit { fqdn, .. } = lookup else {
            still_unresolved += 1;
            continue;
        };
        let symbol_id = if let Some(cached) = fqdn_cache.get(&fqdn) {
            *cached
        } else {
            let fetched = lookup_symbol_id(&conn, &fqdn)?;
            fqdn_cache.insert(fqdn.clone(), fetched);
            fetched
        };
        match symbol_id {
            Some(sid) => id_to_symbol_id.push((edge_id, sid)),
            None => still_unresolved += 1,
        }
    }

    // Apply updates in a single transaction. `UPDATE OR IGNORE`
    // gracefully drops any row whose rewrite would collide with the
    // composite unique on (from_symbol_id, kind, to_symbol_id) — that
    // usually means another path already produced the same resolved
    // edge; we count it as a duplicate skip and leave the unresolved
    // row in place for a future cleanup pass to delete.
    let tx = conn.unchecked_transaction()?;
    #[allow(clippy::similar_names)]
    let mut resolved = 0usize;
    #[allow(clippy::similar_names)]
    let mut duplicate_skipped = 0usize;
    {
        let mut stmt = tx.prepare(
            "UPDATE OR IGNORE edges SET to_symbol_id = ?1, to_unresolved = NULL WHERE id = ?2",
        )?;
        for (edge_id, symbol_id) in id_to_symbol_id {
            let changed = stmt.execute((symbol_id, edge_id))?;
            if changed > 0 {
                resolved += 1;
            } else {
                duplicate_skipped += 1;
            }
        }
    }
    tx.commit()?;

    Ok(ResolveReport {
        resolved,
        still_unresolved,
        duplicate_skipped,
    })
}

fn lookup_symbol_id(conn: &Connection, fqdn: &str) -> Result<Option<i64>, StorageError> {
    let mut stmt = conn.prepare("SELECT id FROM symbols WHERE fqdn = ?1 LIMIT 1")?;
    let row = stmt
        .query_row([fqdn], |row| row.get::<_, i64>(0))
        .optional()?;
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn primary_handle() -> (tempfile::TempDir, IndexHandle) {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        (dir, handle)
    }

    #[test]
    fn noop_when_no_unresolved_edges() {
        // Fresh DB → no edges to sweep. The pass should succeed and
        // report all zeros.
        let (_dir, handle) = primary_handle();
        let report = apply_resolve_unresolved(&handle).unwrap();
        assert_eq!(report.resolved, 0);
        assert_eq!(report.still_unresolved, 0);
        assert_eq!(report.duplicate_skipped, 0);
    }
}

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
    /// Bug E-3 Phase 1: subset of `resolved` that came in through the
    /// `receiver_type`-prefixed lookup (instead of the legacy suffix-
    /// chain). Used to measure the Phase 1 gain in the eprintln log.
    pub resolved_via_receiver_type: usize,
    pub still_unresolved: usize,
    pub duplicate_skipped: usize,
}

pub(crate) fn apply_resolve_unresolved_quietly(handle: &IndexHandle) {
    match apply_resolve_unresolved(handle) {
        Ok(report) => {
            if report.resolved > 0 || report.duplicate_skipped > 0 {
                eprintln!(
                    "standardoc unresolved-edge sweep: {} resolved ({} via receiver_type), {} dup-skipped, {} still unresolved",
                    report.resolved,
                    report.resolved_via_receiver_type,
                    report.duplicate_skipped,
                    report.still_unresolved,
                );
            }
        }
        Err(e) => eprintln!("standardoc unresolved-edge sweep: {e}"),
    }
}

#[allow(clippy::similar_names)]
fn apply_resolve_unresolved(handle: &IndexHandle) -> Result<ResolveReport, StorageError> {
    let resolver = DbCrossWorkspaceResolver::new(handle);
    let pool = handle.pool()?;
    let conn = pool.get().map_err(StorageError::from)?;

    // Pull every unresolved edge in one shot. ~30k rows on standardoc
    // — fits in memory comfortably and avoids holding a statement
    // borrow across the resolver calls (which would re-borrow the
    // same pool).
    let unresolved: Vec<UnresolvedEdge> = {
        let mut stmt = conn.prepare(
            "SELECT id, to_unresolved, receiver_type, kind \
             FROM edges WHERE to_unresolved IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(UnresolvedEdge {
                edge_id: row.get(0)?,
                raw_name: row.get(1)?,
                receiver_type: row.get(2)?,
                kind: row.get(3)?,
            })
        })?;
        rows.collect::<Result<_, _>>()?
    };

    // Compute resolutions FIRST (separate pass) so we don't interleave
    // resolver lookups with DB writes — resolver may borrow its own
    // conn from the pool.
    let mut id_to_symbol_id: Vec<(i64, i64)> = Vec::new();
    let mut still_unresolved = 0usize;
    let mut resolved_via_receiver_type = 0usize;
    let mut fqdn_cache: HashMap<String, Option<i64>> = HashMap::new();
    for edge in unresolved {
        // Bug E-3 Phase 1: when the extractor attached a `receiver_type`
        // (only for Rust method calls today), try `<receiver_type>::<method>`
        // BEFORE the legacy suffix-chain. Exact-FQDN hits cover the
        // `self.method` case (receiver_type = full FQDN); a `LIKE`-suffix
        // fallback covers nominal receivers (`Vec`, `Foo`) inferred from
        // fn params / let bindings.
        if edge.kind == "CALLS"
            && let Some(rt) = edge.receiver_type.as_deref()
            && let Some(sid) = try_resolve_via_receiver_type(&conn, rt, &edge.raw_name)?
        {
            id_to_symbol_id.push((edge.edge_id, sid));
            resolved_via_receiver_type += 1;
            continue;
        }

        // Bug E-2: walk split points longest-module-first and append any
        // remaining tail to the resolver's hit FQDN. Without this, edges
        // pointing through a re-export (e.g. `lur_common::Span::new`
        // when `Span` is `pub use`-ed from `lur-common::span`) stay
        // unresolved because `rsplit_once` asks the resolver about a
        // non-module prefix.
        let Some(lookup) = resolve_with_suffix_chain(&resolver, &edge.raw_name) else {
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
            Some(sid) => id_to_symbol_id.push((edge.edge_id, sid)),
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
    let mut resolved = 0usize;
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
        resolved_via_receiver_type,
        still_unresolved,
        duplicate_skipped,
    })
}

struct UnresolvedEdge {
    edge_id: i64,
    raw_name: String,
    receiver_type: Option<String>,
    kind: String,
}

fn lookup_symbol_id(conn: &Connection, fqdn: &str) -> Result<Option<i64>, StorageError> {
    let mut stmt = conn.prepare("SELECT id FROM symbols WHERE fqdn = ?1 LIMIT 1")?;
    let row = stmt
        .query_row([fqdn], |row| row.get::<_, i64>(0))
        .optional()?;
    Ok(row)
}

/// Bug E-3 Phase 1: resolve `<receiver_type>::<method>` against the
/// `symbols` table. Two-tier lookup:
///   * Exact FQDN match — covers `self.method` calls where
///     `receiver_type` is the full impl-block FQDN (e.g. `crate::Foo`)
///     and the candidate `crate::Foo::method` is a workspace symbol.
///   * `LIKE '%::<receiver_type>::<method>'` — covers nominal short
///     receivers (e.g. `Vec` inferred from `let v = Vec::new()`).
///     Requires a unique suffix match to avoid arbitrarily picking
///     between two same-named methods in different modules; ambiguous
///     hits fall through to the legacy suffix-chain.
fn try_resolve_via_receiver_type(
    conn: &Connection,
    receiver_type: &str,
    method: &str,
) -> Result<Option<i64>, StorageError> {
    let candidate = format!("{receiver_type}::{method}");
    if let Some(sid) = lookup_symbol_id(conn, &candidate)? {
        return Ok(Some(sid));
    }
    if receiver_type.contains("::") {
        return Ok(None);
    }
    let pattern = format!("%::{candidate}");
    let mut stmt = conn.prepare("SELECT id FROM symbols WHERE fqdn LIKE ?1 LIMIT 2")?;
    let mut rows: Vec<i64> = stmt
        .query_map([&pattern], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<_>>()?;
    if rows.len() == 1 {
        return Ok(rows.pop());
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::symbols::insert_symbol;
    use crate::storage::test_utils::{fresh_conn, sample_symbol, seed_file, symbol_ctx};
    use tempfile::tempdir;

    fn primary_handle() -> (tempfile::TempDir, IndexHandle) {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        (dir, handle)
    }

    fn insert_sym(conn: &Connection, fqdn: &str) -> i64 {
        let name = fqdn.rsplit("::").next().unwrap_or(fqdn);
        let sym = sample_symbol(name, fqdn);
        let ctx = symbol_ctx("src/lib.rs");
        insert_symbol(conn, &sym, ctx).expect("insert symbol")
    }

    #[test]
    fn noop_when_no_unresolved_edges() {
        // Fresh DB → no edges to sweep. The pass should succeed and
        // report all zeros.
        let (_dir, handle) = primary_handle();
        let report = apply_resolve_unresolved(&handle).unwrap();
        assert_eq!(report.resolved, 0);
        assert_eq!(report.resolved_via_receiver_type, 0);
        assert_eq!(report.still_unresolved, 0);
        assert_eq!(report.duplicate_skipped, 0);
    }

    // --- Bug E-3 P1.5: receiver_type-prefixed lookup tests ---

    #[test]
    fn receiver_type_exact_fqdn_hits_workspace_symbol() {
        // self.method() — receiver_type is the full impl-block FQDN.
        // `<receiver_type>::<method>` exists as a workspace symbol.
        let conn = fresh_conn();
        seed_file(&conn, "src/lib.rs");
        let sid = insert_sym(&conn, "crate::Foo::run");
        let got = try_resolve_via_receiver_type(&conn, "crate::Foo", "run").unwrap();
        assert_eq!(got, Some(sid));
    }

    #[test]
    fn receiver_type_short_nominal_unique_suffix_hits() {
        // let v = Vec::new(); v.push(...) — receiver_type = "Vec" (short).
        // Only one workspace symbol ends with `::Vec::push` → resolve.
        let conn = fresh_conn();
        seed_file(&conn, "src/lib.rs");
        let sid = insert_sym(&conn, "crate::collections::Vec::push");
        let got = try_resolve_via_receiver_type(&conn, "Vec", "push").unwrap();
        assert_eq!(got, Some(sid));
    }

    #[test]
    fn receiver_type_short_nominal_ambiguous_falls_through() {
        // Two distinct `*::Vec::push` symbols → cannot pick. Caller
        // gets None and the legacy suffix-chain runs (or stays unresolved).
        let conn = fresh_conn();
        seed_file(&conn, "src/lib.rs");
        let _ = insert_sym(&conn, "crate::a::Vec::push");
        let _ = insert_sym(&conn, "crate::b::Vec::push");
        let got = try_resolve_via_receiver_type(&conn, "Vec", "push").unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn receiver_type_fqdn_no_match_no_suffix_fallback() {
        // FQDN-form receiver_type (contains `::`) skips the suffix
        // fallback — an exact miss returns None directly.
        let conn = fresh_conn();
        seed_file(&conn, "src/lib.rs");
        let _ = insert_sym(&conn, "crate::other::Foo::run");
        let got = try_resolve_via_receiver_type(&conn, "crate::Foo", "run").unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn receiver_type_no_match_returns_none() {
        let conn = fresh_conn();
        seed_file(&conn, "src/lib.rs");
        let _ = insert_sym(&conn, "crate::other::Bar::run");
        let got = try_resolve_via_receiver_type(&conn, "Vec", "push").unwrap();
        assert_eq!(got, None);
    }
}

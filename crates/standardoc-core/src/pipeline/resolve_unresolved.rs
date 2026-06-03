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
    /// Trait dispatch sprint: subset of `resolved` that came in through
    /// the `IMPLEMENTS`-walk fallback after a `receiver_type` miss.
    /// Covers derive-emitted edges (`#[derive(Clone)]` → builtin Clone).
    pub resolved_via_trait_dispatch: usize,
    /// Non-derive trait widening: subset of `resolved` that came in
    /// through the builtin-trait-method fallback (`<method>` matches a
    /// seeded `trait_method`-flagged builtin like `Into::into`).
    /// Fires after `try_resolve_via_trait_dispatch` misses.
    pub resolved_via_builtin_trait_method: usize,
    pub still_unresolved: usize,
    pub duplicate_skipped: usize,
}

pub(crate) fn apply_resolve_unresolved_quietly(handle: &IndexHandle) {
    match apply_resolve_unresolved(handle) {
        Ok(report) => {
            if report.resolved > 0 || report.duplicate_skipped > 0 {
                eprintln!(
                    "standardoc unresolved-edge sweep: {} resolved ({} via receiver_type, {} via trait dispatch, {} via builtin trait method), {} dup-skipped, {} still unresolved",
                    report.resolved,
                    report.resolved_via_receiver_type,
                    report.resolved_via_trait_dispatch,
                    report.resolved_via_builtin_trait_method,
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
    let mut resolved_via_trait_dispatch = 0usize;
    let mut resolved_via_builtin_trait_method = 0usize;
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
        {
            if let Some(sid) = try_resolve_via_receiver_type(&conn, rt, &edge.raw_name)? {
                id_to_symbol_id.push((edge.edge_id, sid));
                resolved_via_receiver_type += 1;
                continue;
            }
            // Trait dispatch fallback: walk IMPLEMENTS edges from the
            // receiver_type and try `<trait_fqdn>::<method>`. Resolves
            // derive-emitted method calls (`#[derive(Clone)]` →
            // `x.clone()`) that the inherent lookup missed.
            if let Some(sid) = try_resolve_via_trait_dispatch(&conn, rt, &edge.raw_name)? {
                id_to_symbol_id.push((edge.edge_id, sid));
                resolved_via_trait_dispatch += 1;
                continue;
            }
            // Non-derive trait widening: if the method name matches a
            // seeded builtin trait method (`Into::into`,
            // `Iterator::next`, `ToString::to_string`, …), use that as
            // a synthetic last-resort target. Fires only when both the
            // inherent and IMPLEMENTS-walk paths missed — workspace
            // and derive data always win.
            if let Some(sid) = try_resolve_via_builtin_trait_method(&conn, &edge.raw_name)? {
                id_to_symbol_id.push((edge.edge_id, sid));
                resolved_via_builtin_trait_method += 1;
                continue;
            }
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
        resolved_via_trait_dispatch,
        resolved_via_builtin_trait_method,
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

/// Bug E-3 Phase 1+2: resolve `<receiver_type>::<method>` against the
/// `symbols` table. Three-tier lookup (workspace wins over builtin):
///   1. Exact FQDN match — covers `self.method` calls where
///      `receiver_type` is the full impl-block FQDN (e.g. `crate::Foo`).
///   2. Workspace suffix `LIKE '%::<receiver_type>::<method>'` excluding
///      the `<builtin>::%` namespace — covers nominal short receivers
///      that match a workspace symbol uniquely.
///   3. Builtin direct lookup `<builtin>::rust::<receiver_type>::<method>`
///      (Phase 2) — covers stdlib method calls (Vec::push, Option::unwrap,
///      ...) seeded by `seed_methods_into` at cold-start.
fn try_resolve_via_receiver_type(
    conn: &Connection,
    receiver_type: &str,
    method: &str,
) -> Result<Option<i64>, StorageError> {
    let candidate = format!("{receiver_type}::{method}");
    if let Some(sid) = lookup_symbol_id(conn, &candidate)? {
        return Ok(Some(sid));
    }
    // FQDN-form receiver: no nominal-suffix fallback, but still try
    // the builtin path below in case a workspace shadowed something.
    if !receiver_type.contains("::") {
        let pattern = format!("%::{candidate}");
        let mut stmt = conn.prepare(
            "SELECT id FROM symbols \
             WHERE fqdn LIKE ?1 AND fqdn NOT LIKE '<builtin>::%' LIMIT 2",
        )?;
        let mut rows: Vec<i64> = stmt
            .query_map([&pattern], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<_>>()?;
        if rows.len() == 1 {
            return Ok(rows.pop());
        }
    }
    // Phase 2: stdlib method fallback. Receiver_type is purely nominal
    // here (`Vec`, `Option`, `HashMap`, ...). Only Rust populates
    // receiver_type today, so the `rust` slug is hardcoded; extend when
    // other extractors gain Phase 1.
    let builtin = format!("<builtin>::rust::{candidate}");
    if let Some(sid) = lookup_symbol_id(conn, &builtin)? {
        return Ok(Some(sid));
    }
    Ok(None)
}

/// Trait dispatch sprint: when `try_resolve_via_receiver_type` misses,
/// walk every `IMPLEMENTS` edge whose source matches `receiver_type` and
/// try `<trait_fqdn>::<method>` against `symbols`. Returns the first
/// hit, with traits visited in alphabetical FQDN order (the inherent
/// path already ran upstream, so this layer only fires for true
/// trait-only calls — `<Type>::clone` derived from `#[derive(Clone)]`).
///
/// Builtin sources are excluded from the IMPLEMENTS source side because
/// the resolver's job here is to convert workspace receiver types into
/// builtin trait method targets, not the other way around.
fn try_resolve_via_trait_dispatch(
    conn: &Connection,
    receiver_type: &str,
    method: &str,
) -> Result<Option<i64>, StorageError> {
    let is_fqdn = receiver_type.contains("::");
    let nominal_pattern = if is_fqdn {
        String::new()
    } else {
        format!("%::{receiver_type}")
    };
    let mut stmt = conn.prepare(
        "SELECT DISTINCT trait_sym.fqdn \
         FROM edges e \
         JOIN symbols src ON e.from_symbol_id = src.id \
         JOIN symbols trait_sym ON e.to_symbol_id = trait_sym.id \
         WHERE e.kind = 'IMPLEMENTS' \
           AND e.to_symbol_id IS NOT NULL \
           AND src.fqdn NOT LIKE '<builtin>::%' \
           AND (src.fqdn = ?1 OR (?2 != '' AND src.fqdn LIKE ?2)) \
         ORDER BY trait_sym.fqdn ASC",
    )?;
    let trait_fqdns: Vec<String> = stmt
        .query_map([receiver_type, nominal_pattern.as_str()], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<_>>()?;
    for trait_fqdn in trait_fqdns {
        let candidate = format!("{trait_fqdn}::{method}");
        if let Some(sid) = lookup_symbol_id(conn, &candidate)? {
            return Ok(Some(sid));
        }
    }
    Ok(None)
}

/// Non-derive trait widening: when the workspace has no inherent
/// `<receiver_type>::<method>` AND no IMPLEMENTS edge points the
/// receiver at a workspace-known trait, fall back to "is this method
/// name owned by a known builtin trait?".
///
/// Queries `symbols` for any synthetic whose fqdn ends in `::<method>`
/// AND carries the `trait_method` flag set by `seed_methods_into` for
/// `BuiltinMethodEntry` rows stamped `.with_trait()`. Multiple matches
/// (e.g. `eq` is on both `PartialEq` and other reflection traits) are
/// disambiguated alphabetically by fqdn — mirrors the policy chosen
/// for `try_resolve_via_trait_dispatch`.
fn try_resolve_via_builtin_trait_method(
    conn: &Connection,
    method: &str,
) -> Result<Option<i64>, StorageError> {
    let fqdn_pattern = format!("<builtin>::rust::%::{method}");
    let mut stmt = conn.prepare(
        "SELECT id FROM symbols \
         WHERE fqdn LIKE ?1 \
           AND name = ?2 \
           AND flags LIKE '%\"trait_method\"%' \
         ORDER BY fqdn ASC \
         LIMIT 1",
    )?;
    let sid = stmt
        .query_row([fqdn_pattern.as_str(), method], |row| row.get::<_, i64>(0))
        .optional()?;
    Ok(sid)
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

    // --- Bug E-3 P2.4: builtin method fallback tests ---

    #[test]
    fn receiver_type_builtin_method_fallback_hits_when_seeded() {
        // Phase 2 seeds <builtin>::rust::Vec::push as a synthetic symbol.
        // Receiver_type = "Vec", method = "push", no workspace symbol
        // matches → falls through to the builtin direct lookup.
        let conn = fresh_conn();
        seed_file(&conn, "src/lib.rs");
        let sid = insert_sym(&conn, "<builtin>::rust::Vec::push");
        let got = try_resolve_via_receiver_type(&conn, "Vec", "push").unwrap();
        assert_eq!(got, Some(sid));
    }

    #[test]
    fn receiver_type_workspace_wins_over_builtin() {
        // Both workspace and builtin Vec::push exist → workspace wins
        // (Phase 2 builtin only fires when workspace suffix is empty
        // or ambiguous-empty after the `NOT LIKE '<builtin>::%'`
        // filter).
        let conn = fresh_conn();
        seed_file(&conn, "src/lib.rs");
        let workspace_sid = insert_sym(&conn, "crate::my_collections::Vec::push");
        let _builtin_sid = insert_sym(&conn, "<builtin>::rust::Vec::push");
        let got = try_resolve_via_receiver_type(&conn, "Vec", "push").unwrap();
        assert_eq!(got, Some(workspace_sid));
    }

    #[test]
    fn receiver_type_ambiguous_workspace_falls_back_to_builtin() {
        // Two ambiguous workspace matches AND a builtin → workspace
        // suffix returns 2 rows (ambiguous), the function skips, then
        // hits the builtin direct lookup. Tradeoff: the builtin is
        // canonical, so resolving to it is the right call here.
        let conn = fresh_conn();
        seed_file(&conn, "src/lib.rs");
        let _ = insert_sym(&conn, "crate::a::Vec::push");
        let _ = insert_sym(&conn, "crate::b::Vec::push");
        let builtin_sid = insert_sym(&conn, "<builtin>::rust::Vec::push");
        let got = try_resolve_via_receiver_type(&conn, "Vec", "push").unwrap();
        assert_eq!(got, Some(builtin_sid));
    }

    #[test]
    fn receiver_type_fqdn_with_builtin_seeded_returns_none_when_no_match() {
        // FQDN-form receiver_type (`crate::Foo`) means the user has a
        // workspace impl block; the builtin Vec::push synthetic must
        // NOT match the unrelated `crate::Foo::push` lookup.
        let conn = fresh_conn();
        seed_file(&conn, "src/lib.rs");
        let _ = insert_sym(&conn, "<builtin>::rust::Vec::push");
        let got = try_resolve_via_receiver_type(&conn, "crate::Foo", "push").unwrap();
        assert_eq!(got, None);
    }

    // --- Trait dispatch sprint tests ---

    fn insert_implements(conn: &Connection, from_sid: i64, to_sid: i64) {
        conn.execute(
            "INSERT INTO edges (from_symbol_id, kind, to_symbol_id, attributes, confidence) \
             VALUES (?1, 'IMPLEMENTS', ?2, '[\"derive\",\"via-builtin\"]', 'extracted')",
            rusqlite::params![from_sid, to_sid],
        )
        .unwrap();
    }

    #[test]
    fn trait_dispatch_resolves_via_implements_walk_with_fqdn_receiver() {
        // Workspace struct `crate::Foo` IMPLEMENTS the seeded builtin
        // Clone trait; lookup `Foo::clone` misses inherent → trait
        // dispatch finds `<builtin>::rust::Clone::clone`.
        let conn = fresh_conn();
        seed_file(&conn, "src/lib.rs");
        let foo_sid = insert_sym(&conn, "crate::Foo");
        let clone_trait_sid = insert_sym(&conn, "<builtin>::rust::Clone");
        let clone_method_sid = insert_sym(&conn, "<builtin>::rust::Clone::clone");
        insert_implements(&conn, foo_sid, clone_trait_sid);
        let got = try_resolve_via_trait_dispatch(&conn, "crate::Foo", "clone").unwrap();
        assert_eq!(got, Some(clone_method_sid));
    }

    #[test]
    fn trait_dispatch_resolves_via_nominal_receiver_suffix() {
        // Receiver_type = "Foo" (short) finds `crate::a::Foo` via the
        // `%::Foo` suffix match, then walks IMPLEMENTS to Clone.
        let conn = fresh_conn();
        seed_file(&conn, "src/lib.rs");
        let foo_sid = insert_sym(&conn, "crate::a::Foo");
        let clone_trait_sid = insert_sym(&conn, "<builtin>::rust::Clone");
        let clone_method_sid = insert_sym(&conn, "<builtin>::rust::Clone::clone");
        insert_implements(&conn, foo_sid, clone_trait_sid);
        let got = try_resolve_via_trait_dispatch(&conn, "Foo", "clone").unwrap();
        assert_eq!(got, Some(clone_method_sid));
    }

    #[test]
    fn trait_dispatch_returns_none_without_implements_edge() {
        // No IMPLEMENTS edge from Foo → nothing to walk.
        let conn = fresh_conn();
        seed_file(&conn, "src/lib.rs");
        let _ = insert_sym(&conn, "crate::Foo");
        let _ = insert_sym(&conn, "<builtin>::rust::Clone::clone");
        let got = try_resolve_via_trait_dispatch(&conn, "crate::Foo", "clone").unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn trait_dispatch_picks_alphabetical_first_when_multiple_traits_match() {
        // Foo IMPLEMENTS both Clone and Debug; both expose a synthetic
        // method `dup`. ORDER BY trait_sym.fqdn ASC → Clone (< Debug)
        // wins.
        let conn = fresh_conn();
        seed_file(&conn, "src/lib.rs");
        let foo_sid = insert_sym(&conn, "crate::Foo");
        let clone_sid = insert_sym(&conn, "<builtin>::rust::Clone");
        let debug_sid = insert_sym(&conn, "<builtin>::rust::Debug");
        let clone_dup = insert_sym(&conn, "<builtin>::rust::Clone::dup");
        let _debug_dup = insert_sym(&conn, "<builtin>::rust::Debug::dup");
        insert_implements(&conn, foo_sid, clone_sid);
        insert_implements(&conn, foo_sid, debug_sid);
        let got = try_resolve_via_trait_dispatch(&conn, "crate::Foo", "dup").unwrap();
        assert_eq!(got, Some(clone_dup));
    }

    #[test]
    fn trait_dispatch_skips_implements_with_null_to_symbol_id() {
        // Insert an IMPLEMENTS edge whose to_symbol_id is NULL (the
        // trait target stayed unresolved). The walker must filter it
        // out — otherwise the JOIN would NULL-fail and skip silently
        // but the test cements the intent.
        let conn = fresh_conn();
        seed_file(&conn, "src/lib.rs");
        let foo_sid = insert_sym(&conn, "crate::Foo");
        conn.execute(
            "INSERT INTO edges (from_symbol_id, kind, to_unresolved, attributes, confidence) \
             VALUES (?1, 'IMPLEMENTS', 'UnresolvedTrait', '[]', 'extracted')",
            rusqlite::params![foo_sid],
        )
        .unwrap();
        let got = try_resolve_via_trait_dispatch(&conn, "crate::Foo", "clone").unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn trait_dispatch_excludes_builtin_source_types() {
        // An IMPLEMENTS edge whose source is itself a builtin must not
        // be walked — the resolver's role here is to bridge workspace
        // receivers to builtin methods.
        let conn = fresh_conn();
        seed_file(&conn, "src/lib.rs");
        let vec_sid = insert_sym(&conn, "<builtin>::rust::Vec");
        let clone_sid = insert_sym(&conn, "<builtin>::rust::Clone");
        let _ = insert_sym(&conn, "<builtin>::rust::Clone::clone");
        insert_implements(&conn, vec_sid, clone_sid);
        let got = try_resolve_via_trait_dispatch(&conn, "<builtin>::rust::Vec", "clone").unwrap();
        assert_eq!(got, None);
    }

    // --- Non-derive trait widening tests ---

    fn insert_trait_method_sym(conn: &Connection, fqdn: &str) -> i64 {
        let name = fqdn.rsplit("::").next().unwrap_or(fqdn);
        let mut sym = sample_symbol(name, fqdn);
        sym.flags = vec!["trait_method".to_string()];
        let ctx = symbol_ctx("src/lib.rs");
        insert_symbol(conn, &sym, ctx).expect("insert trait method symbol")
    }

    #[test]
    fn builtin_trait_method_resolves_when_method_is_unique_trait() {
        // `Into::into` is the only trait-flagged builtin that owns
        // `into`. Resolve picks it up.
        let conn = fresh_conn();
        seed_file(&conn, "src/lib.rs");
        let sid = insert_trait_method_sym(&conn, "<builtin>::rust::Into::into");
        let got = try_resolve_via_builtin_trait_method(&conn, "into").unwrap();
        assert_eq!(got, Some(sid));
    }

    #[test]
    fn builtin_trait_method_returns_none_when_no_seeded_match() {
        // The receiver is irrelevant — what matters is whether the method
        // name matches any seeded trait method. Without a row, None.
        let conn = fresh_conn();
        seed_file(&conn, "src/lib.rs");
        let got = try_resolve_via_builtin_trait_method(&conn, "into").unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn builtin_trait_method_ignores_unflagged_type_methods() {
        // `Vec::push` exists as a builtin method symbol but is NOT
        // flagged `trait_method` (it's a type-method, not a trait
        // dispatch). It must not bleed into this resolver step.
        let conn = fresh_conn();
        seed_file(&conn, "src/lib.rs");
        let _ = insert_sym(&conn, "<builtin>::rust::Vec::push");
        let got = try_resolve_via_builtin_trait_method(&conn, "push").unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn builtin_trait_method_picks_alphabetical_first_on_collision() {
        // `eq` exists on both `PartialEq` and a hypothetical `Reflexive`
        // trait. ORDER BY fqdn ASC → PartialEq (alphabetically first)
        // wins, matching the trait-dispatch ambiguity policy.
        let conn = fresh_conn();
        seed_file(&conn, "src/lib.rs");
        let partial_eq = insert_trait_method_sym(&conn, "<builtin>::rust::PartialEq::eq");
        let _reflexive = insert_trait_method_sym(&conn, "<builtin>::rust::Reflexive::eq");
        let got = try_resolve_via_builtin_trait_method(&conn, "eq").unwrap();
        assert_eq!(got, Some(partial_eq));
    }

    #[test]
    fn builtin_trait_method_filters_by_exact_name_not_just_fqdn_suffix() {
        // A trait method `Into::into` and an unrelated symbol whose
        // fqdn HAPPENS to end in `::into_something` would both match a
        // naive `LIKE %::into` filter. The `name = ?` filter cuts that.
        let conn = fresh_conn();
        seed_file(&conn, "src/lib.rs");
        let into_sid = insert_trait_method_sym(&conn, "<builtin>::rust::Into::into");
        let _decoy = insert_trait_method_sym(&conn, "<builtin>::rust::Foo::into_owned");
        let got = try_resolve_via_builtin_trait_method(&conn, "into").unwrap();
        assert_eq!(got, Some(into_sid));
    }

    #[test]
    fn apply_resolve_unresolved_increments_builtin_trait_method_counter() {
        // End-to-end: unresolved CALLS edge with receiver_type that
        // doesn't match any workspace inherent or IMPLEMENTS edge.
        // The new builtin-trait-method step must take it and bump
        // the counter.
        let (_dir, handle) = primary_handle();
        let pool = handle.pool().unwrap();
        let conn = pool.get().unwrap();
        seed_file(&conn, "src/lib.rs");
        let into_sid = insert_trait_method_sym(&conn, "<builtin>::rust::Into::into");
        let caller_sid = insert_sym(&conn, "crate::main");
        conn.execute(
            "INSERT INTO edges (from_symbol_id, kind, to_unresolved, attributes, confidence, receiver_type) \
             VALUES (?1, 'CALLS', 'into', '[]', 'extracted', 'str')",
            rusqlite::params![caller_sid],
        )
        .unwrap();
        drop(conn);

        let report = apply_resolve_unresolved(&handle).unwrap();
        assert_eq!(report.resolved, 1);
        assert_eq!(report.resolved_via_builtin_trait_method, 1);
        assert_eq!(report.resolved_via_trait_dispatch, 0);
        assert_eq!(report.resolved_via_receiver_type, 0);

        // Confirm the edge points at the Into::into symbol.
        let conn2 = pool.get().unwrap();
        let to_id: i64 = conn2
            .query_row(
                "SELECT to_symbol_id FROM edges WHERE to_unresolved IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(to_id, into_sid);
    }

    #[test]
    fn apply_resolve_unresolved_increments_trait_dispatch_counter() {
        // End-to-end: unresolved CALLS edge with receiver_type pointing
        // at a workspace struct that IMPLEMENTS Clone. Sweep must
        // resolve it via trait dispatch and bump the counter.
        let (_dir, handle) = primary_handle();
        let pool = handle.pool().unwrap();
        let conn = pool.get().unwrap();
        seed_file(&conn, "src/lib.rs");
        let foo_sid = insert_sym(&conn, "crate::Foo");
        let clone_trait_sid = insert_sym(&conn, "<builtin>::rust::Clone");
        let _clone_method_sid = insert_sym(&conn, "<builtin>::rust::Clone::clone");
        let caller_sid = insert_sym(&conn, "crate::main");
        insert_implements(&conn, foo_sid, clone_trait_sid);
        conn.execute(
            "INSERT INTO edges (from_symbol_id, kind, to_unresolved, attributes, confidence, receiver_type) \
             VALUES (?1, 'CALLS', 'clone', '[]', 'extracted', 'crate::Foo')",
            rusqlite::params![caller_sid],
        )
        .unwrap();
        drop(conn);

        let report = apply_resolve_unresolved(&handle).unwrap();
        assert_eq!(report.resolved, 1);
        assert_eq!(report.resolved_via_trait_dispatch, 1);
        assert_eq!(report.resolved_via_receiver_type, 0);
        assert_eq!(report.still_unresolved, 0);
    }
}

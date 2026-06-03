use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension};
use standardoc_ir::{ExtractedFile, RawDocument, RawEdge};

use crate::pipeline::diff::{DiffPlan, diff_symbols, fetch_existing_symbols};
use crate::storage::call_sites::{delete_call_sites_by_file, insert_call_site};
use crate::storage::documents::{DocumentInput, delete_document, upsert_document};
use crate::storage::edge_sites::{delete_edge_sites_by_file, insert_edge_sites};
use crate::storage::edges::{delete_edges_from, insert_edge, promote_unresolved_batch};
use crate::storage::error::StorageError;
use crate::storage::files::{FileInput, delete_file, upsert_file};
use crate::storage::module_lookup::put_module_lookup;
use crate::storage::symbol_ffi_binding::{delete_bindings_for_symbol, upsert_binding};
use crate::storage::symbols::{
    SymbolInsertContext, delete_symbol, insert_symbol, update_symbol_positions,
};

pub(crate) fn apply_upsert_file(
    conn: &Connection,
    extracted: &ExtractedFile,
    revision: u64,
    workspace_id: &str,
) -> Result<(), StorageError> {
    let path = extracted.file.as_str();
    upsert_file_row(conn, extracted)?;
    let existing = fetch_existing_symbols(conn, path)?;
    let plan = diff_symbols(&existing, &extracted.symbols);
    let ctx = SymbolInsertContext {
        file_path: path,
        language: extracted.language,
        is_external: extracted.is_external,
        source_origin: extracted.source_origin,
        revision,
        workspace_id,
    };

    apply_deletes(conn, &plan)?;
    apply_position_updates(conn, &plan, revision)?;
    let new_or_modified_ids = apply_inserts_and_modifications(conn, &plan, ctx)?;
    apply_edges(conn, &plan, &extracted.edges, workspace_id)?;
    promote_unresolved_batch(conn, &new_or_modified_ids)?;
    apply_documents(
        conn,
        &plan,
        &new_or_modified_ids,
        &extracted.documents,
        workspace_id,
    )?;
    // IR-4-f: persist the call_sites vec populated by the extractors
    // since IR-4-b/c/d. Delete-then-batch-insert mirrors the documents
    // pattern — re-extraction is the common path, and a full re-write
    // is cheaper than diffing per-call-site identity (which would need
    // a stable id we don't currently carry on the IR side).
    apply_call_sites(conn, path, &extracted.call_sites)?;
    // Stage 2 — persist FFI bindings emitted by the language tagger.
    // Delete-then-upsert pattern: drop every binding owned by any
    // symbol touched by this batch, then insert the freshly-extracted
    // set. Mirrors the call_sites / documents discipline so removing a
    // binding from source actually removes the row.
    apply_ffi_bindings(conn, &extracted.ffi_bindings, workspace_id)?;
    // Stage 3 final-mile (R1) — persist the AOT ModuleLookup so
    // cross-workspace queries (resolve_cross_workspace_import,
    // list_cross_workspace_providers) can see this workspace's modules
    // when it sits on the peer side of a link. `put_module_lookup`
    // upserts by (workspace_id, module_fqdn) so re-extracting the same
    // file replaces the prior payload in place. `None` for languages
    // without an AOT pass (Lua, C, externals) — nothing to persist.
    if let Some(lookup) = extracted.module_lookup.as_ref() {
        put_module_lookup(conn, workspace_id, lookup)?;
    }
    Ok(())
}

/// Stage 2 — re-sync `symbol_ffi_binding` rows for every symbol
/// referenced by `bindings`. For each binding, resolves
/// `symbol_fqdn` → `symbols.id` via a point lookup; symbols that
/// haven't materialised yet (cold_start ordering edge case) are
/// skipped silently — the next extraction batch will pick them up.
fn apply_ffi_bindings(
    conn: &Connection,
    bindings: &[standardoc_ir::RawFfiBinding],
    workspace_id: &str,
) -> Result<(), StorageError> {
    if bindings.is_empty() {
        return Ok(());
    }
    // Group by symbol_id so we delete-once-then-insert-all per symbol
    // (cheaper than N delete-one-then-insert-one and gives the right
    // semantics: a re-extraction with zero bindings on a symbol drops
    // its prior bindings).
    let mut by_symbol: std::collections::HashMap<i64, Vec<&standardoc_ir::RawFfiBinding>> =
        std::collections::HashMap::new();
    for b in bindings {
        let Some(id) = lookup_symbol_id_by_fqdn(conn, &b.symbol_fqdn, workspace_id)? else {
            continue;
        };
        by_symbol.entry(id).or_default().push(b);
    }
    for (sym_id, group) in by_symbol {
        delete_bindings_for_symbol(conn, sym_id)?;
        for b in group {
            upsert_binding(conn, sym_id, b)?;
        }
    }
    Ok(())
}

fn lookup_symbol_id_by_fqdn(
    conn: &Connection,
    fqdn: &str,
    workspace_id: &str,
) -> Result<Option<i64>, StorageError> {
    let id = conn
        .query_row(
            "SELECT id FROM symbols WHERE workspace_id = ?1 AND fqdn = ?2",
            rusqlite::params![workspace_id, fqdn],
            |r| r.get::<_, i64>(0),
        )
        .optional()?;
    Ok(id)
}

/// IR-4-f — re-sync the `call_sites` rows for `file_path` with the
/// freshly-extracted vec. Drops every existing row keyed by the file,
/// then inserts the new set. Idempotent: running with an empty vec
/// against a file that previously had call_sites cleanly purges them.
fn apply_call_sites(
    conn: &Connection,
    file_path: &str,
    call_sites: &[standardoc_ir::RawCallSite],
) -> Result<(), StorageError> {
    delete_call_sites_by_file(conn, file_path)?;
    for cs in call_sites {
        insert_call_site(conn, file_path, cs)?;
    }
    Ok(())
}

pub(crate) fn apply_delete_file(conn: &Connection, path: &str) -> Result<(), StorageError> {
    let symbol_ids = fetch_symbol_ids_by_file(conn, path)?;
    for id in symbol_ids {
        delete_symbol(conn, id)?;
    }
    delete_edge_sites_by_file(conn, path)?;
    delete_file(conn, path)?;
    Ok(())
}

pub(crate) fn record_parse_error(
    conn: &Connection,
    extracted_path: &str,
    language: standardoc_ir::Language,
    detail: &str,
) -> Result<(), StorageError> {
    let now = current_unix_ms()?;
    let existing = crate::storage::files::get_file(conn, extracted_path)?;
    let file = match existing {
        Some(mut prev) => {
            prev.last_scan_error = Some(detail.into());
            prev.last_scanned = now;
            prev
        }
        None => FileInput {
            path: extracted_path.into(),
            content_hash: standardoc_ir::Blake3Hash::default(),
            language,
            byte_size: 0,
            last_scanned: now,
            last_scan_error: Some(detail.into()),
            is_external: false,
        },
    };
    upsert_file(conn, &file)
}

fn apply_deletes(conn: &Connection, plan: &DiffPlan<'_>) -> Result<(), StorageError> {
    for id in &plan.deletions {
        delete_symbol(conn, *id)?;
    }
    Ok(())
}

fn apply_position_updates(
    conn: &Connection,
    plan: &DiffPlan<'_>,
    revision: u64,
) -> Result<(), StorageError> {
    for (id, sym) in &plan.position_updates {
        update_symbol_positions(conn, *id, &sym.location, revision)?;
    }
    Ok(())
}

fn apply_inserts_and_modifications(
    conn: &Connection,
    plan: &DiffPlan<'_>,
    ctx: SymbolInsertContext<'_>,
) -> Result<Vec<i64>, StorageError> {
    let mut ids = Vec::with_capacity(plan.inserts.len() + plan.modifications.len());
    for (id, sym) in &plan.modifications {
        let id_back = insert_symbol(conn, sym, ctx)?;
        debug_assert_eq!(id_back, *id, "UPSERT must preserve id by fqdn");
        delete_edges_from(conn, id_back)?;
        ids.push(id_back);
    }
    for sym in &plan.inserts {
        let id = insert_symbol(conn, sym, ctx)?;
        ids.push(id);
    }
    Ok(ids)
}

fn apply_edges(
    conn: &Connection,
    plan: &DiffPlan<'_>,
    edges: &[RawEdge],
    workspace_id: &str,
) -> Result<(), StorageError> {
    if edges.is_empty() {
        return Ok(());
    }
    let touched = touched_fqdns(plan);
    for edge in edges {
        if !touched.contains(edge.from_fqdn.as_str()) {
            continue;
        }
        let Some(from_id) = lookup_symbol_id_by_fqdn(conn, &edge.from_fqdn, workspace_id)? else {
            continue;
        };
        let edge_id = insert_edge(conn, from_id, edge, workspace_id)?;
        if !edge.sites.is_empty() {
            insert_edge_sites(conn, edge_id, &edge.sites)?;
        }
    }
    Ok(())
}

/// Replay user-authored documents for the touched symbol set.
///
/// Scoped to `new_or_modified_ids` (= inserts ∪ modifications) — unchanged
/// symbols keep their existing docs untouched. For each touched id we wipe
/// any stale `documents` row, then UPSERT for each `RawDocument` whose
/// `symbol_fqdn` lands in the touched set. A symbol whose `///`/JSDoc was
/// removed by the user has no matching `RawDocument` → its row stays
/// deleted (intentional). Symbol disappearance is handled by FK cascade
/// from `delete_symbol`.
fn apply_documents(
    conn: &Connection,
    plan: &DiffPlan<'_>,
    new_or_modified_ids: &[i64],
    documents: &[RawDocument],
    workspace_id: &str,
) -> Result<(), StorageError> {
    if new_or_modified_ids.is_empty() && documents.is_empty() {
        return Ok(());
    }
    for &id in new_or_modified_ids {
        delete_document(conn, id)?;
    }
    if documents.is_empty() {
        return Ok(());
    }
    let touched = touched_fqdns(plan);
    let now = current_unix_ms()?;
    for doc in documents {
        if !touched.contains(doc.symbol_fqdn.as_str()) {
            continue;
        }
        let Some(id) = lookup_symbol_id_by_fqdn(conn, &doc.symbol_fqdn, workspace_id)? else {
            continue;
        };
        upsert_document(
            conn,
            &DocumentInput {
                symbol_id: id,
                description: Some(doc.description.clone()),
                last_updated: now,
                ..DocumentInput::default()
            },
        )?;
    }
    Ok(())
}

fn touched_fqdns<'a>(plan: &'a DiffPlan<'_>) -> HashSet<&'a str> {
    let mut out: HashSet<&'a str> =
        HashSet::with_capacity(plan.inserts.len() + plan.modifications.len());
    for sym in &plan.inserts {
        out.insert(sym.fqdn.as_str());
    }
    for (_, sym) in &plan.modifications {
        out.insert(sym.fqdn.as_str());
    }
    out
}

fn fetch_symbol_ids_by_file(conn: &Connection, path: &str) -> Result<Vec<i64>, StorageError> {
    let mut stmt = conn.prepare("SELECT id FROM symbols WHERE file_path = ?1")?;
    let rows = stmt
        .query_map([path], |r| r.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn upsert_file_row(conn: &Connection, extracted: &ExtractedFile) -> Result<(), StorageError> {
    let last_scanned = current_unix_ms()?;
    let file = FileInput {
        path: extracted.file.clone(),
        content_hash: extracted.content_hash,
        language: extracted.language,
        byte_size: extracted.byte_size,
        last_scanned,
        last_scan_error: None,
        is_external: extracted.is_external,
    };
    upsert_file(conn, &file)
}

fn current_unix_ms() -> Result<i64, StorageError> {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| StorageError::Io(std::io::Error::other(e.to_string())))?
        .as_millis();
    i64::try_from(ms).map_err(|_| StorageError::InvalidStoredData {
        detail: format!("unix_ms {ms} exceeds i64::MAX"),
    })
}

#[cfg(test)]
mod tests;

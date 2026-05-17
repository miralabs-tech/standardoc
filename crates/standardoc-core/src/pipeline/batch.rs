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
use crate::storage::symbols::{
    SymbolInsertContext, delete_symbol, insert_symbol, update_symbol_positions,
};

pub(crate) fn apply_upsert_file(
    conn: &Connection,
    extracted: &ExtractedFile,
    revision: u64,
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
    };

    apply_deletes(conn, &plan)?;
    apply_position_updates(conn, &plan, revision)?;
    let new_or_modified_ids = apply_inserts_and_modifications(conn, &plan, ctx)?;
    apply_edges(conn, &plan, &extracted.edges)?;
    promote_unresolved_batch(conn, &new_or_modified_ids)?;
    apply_documents(conn, &plan, &new_or_modified_ids, &extracted.documents)?;
    // IR-4-f: persist the call_sites vec populated by the extractors
    // since IR-4-b/c/d. Delete-then-batch-insert mirrors the documents
    // pattern — re-extraction is the common path, and a full re-write
    // is cheaper than diffing per-call-site identity (which would need
    // a stable id we don't currently carry on the IR side).
    apply_call_sites(conn, path, &extracted.call_sites)?;
    Ok(())
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
) -> Result<(), StorageError> {
    if edges.is_empty() {
        return Ok(());
    }
    let touched = touched_fqdns(plan);
    for edge in edges {
        if !touched.contains(edge.from_fqdn.as_str()) {
            continue;
        }
        let Some(from_id) = lookup_symbol_id(conn, &edge.from_fqdn)? else {
            continue;
        };
        let edge_id = insert_edge(conn, from_id, edge)?;
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
        let Some(id) = lookup_symbol_id(conn, &doc.symbol_fqdn)? else {
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

fn lookup_symbol_id(conn: &Connection, fqdn: &str) -> Result<Option<i64>, StorageError> {
    let id = conn
        .query_row("SELECT id FROM symbols WHERE fqdn = ?1", [fqdn], |r| {
            r.get::<_, i64>(0)
        })
        .optional()?;
    Ok(id)
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
mod tests {
    use super::*;
    use crate::storage::documents::get_document;
    use crate::storage::files::get_file;
    use crate::storage::test_utils::fresh_conn;
    use standardoc_ir::{
        Blake3Hash, EdgeConfidence, EdgeKind, ExtractedFile, Kind, Language, LanguageKind,
        RawDocument, RawEdge, RawSymbol, ResolvedOrUnresolved, Site, SourceOrigin, SymbolLocation,
        Visibility,
    };

    fn sym(name: &str, fqdn: &str, hash_byte: u8, line: u32) -> RawSymbol {
        RawSymbol {
            name: name.into(),
            fqdn: fqdn.into(),
            kind: Kind::Function,
            language_kind: LanguageKind::from("fn_item"),
            module: None,
            visibility: Visibility::Public,
            location: SymbolLocation {
                file: "src/main.rs".into(),
                start_line: line,
                end_line: line + 4,
                start_col: 0,
                end_col: 1,
            },
            signature: None,
            body_hash: Some(Blake3Hash::new([hash_byte; 32])),
            attributes: vec![],
            flags: vec![],
        }
    }

    fn extracted(file: &str, symbols: Vec<RawSymbol>, edges: Vec<RawEdge>) -> ExtractedFile {
        ExtractedFile {
            file: file.into(),
            language: Language::Rust,
            source_origin: SourceOrigin::Workspace,
            is_external: false,
            content_hash: Blake3Hash::new([0xab; 32]),
            byte_size: 4096,
            symbols,
            edges,
            call_sites: vec![],
            documents: vec![],
        }
    }

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn upsert_file_inserts_new_symbols_and_creates_files_row() {
        let conn = fresh_conn();
        let ef = extracted(
            "src/main.rs",
            vec![
                sym("foo", "crate::foo", 0x01, 1),
                sym("bar", "crate::bar", 0x02, 10),
            ],
            vec![],
        );
        apply_upsert_file(&conn, &ef, 0).unwrap();

        assert_eq!(count(&conn, "SELECT COUNT(*) FROM symbols"), 2);
        let f = get_file(&conn, "src/main.rs").unwrap().unwrap();
        assert_eq!(f.content_hash, Blake3Hash::new([0xab; 32]));
        assert_eq!(f.last_scan_error, None);
    }

    #[test]
    fn upsert_file_unchanged_body_only_updates_positions() {
        let conn = fresh_conn();
        let ef1 = extracted(
            "src/main.rs",
            vec![sym("foo", "crate::foo", 0x01, 1)],
            vec![],
        );
        apply_upsert_file(&conn, &ef1, 0).unwrap();

        let ef2 = extracted(
            "src/main.rs",
            vec![sym("foo", "crate::foo", 0x01, 100)],
            vec![],
        );
        apply_upsert_file(&conn, &ef2, 0).unwrap();

        let (start_line, name): (i64, String) = conn
            .query_row(
                "SELECT start_line, name FROM symbols WHERE fqdn = 'crate::foo'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(start_line, 100);
        assert_eq!(name, "foo");
    }

    #[test]
    fn upsert_file_modified_body_replaces_outgoing_edges() {
        let conn = fresh_conn();
        let edge = RawEdge {
            from_fqdn: "crate::foo".into(),
            kind: EdgeKind::Calls,
            to: ResolvedOrUnresolved::Unresolved {
                name: "old_target".into(),
            },
            sites: vec![Site {
                file: "src/main.rs".into(),
                line: 2,
                col: 4,
            }],
            attributes: vec![],
            confidence: EdgeConfidence::Ambiguous,
        };
        let ef1 = extracted(
            "src/main.rs",
            vec![sym("foo", "crate::foo", 0x01, 1)],
            vec![edge],
        );
        apply_upsert_file(&conn, &ef1, 0).unwrap();
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM edges"), 1);

        let new_edge = RawEdge {
            from_fqdn: "crate::foo".into(),
            kind: EdgeKind::Calls,
            to: ResolvedOrUnresolved::Unresolved {
                name: "new_target".into(),
            },
            sites: vec![],
            attributes: vec![],
            confidence: EdgeConfidence::Ambiguous,
        };
        let ef2 = extracted(
            "src/main.rs",
            vec![sym("foo", "crate::foo", 0xee, 1)],
            vec![new_edge],
        );
        apply_upsert_file(&conn, &ef2, 0).unwrap();

        let to_unresolved: String = conn
            .query_row("SELECT to_unresolved FROM edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(to_unresolved, "new_target");
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM edges"), 1);
    }

    #[test]
    fn upsert_file_disappeared_symbol_is_deleted() {
        let conn = fresh_conn();
        let ef1 = extracted(
            "src/main.rs",
            vec![
                sym("foo", "crate::foo", 0x01, 1),
                sym("bar", "crate::bar", 0x02, 10),
            ],
            vec![],
        );
        apply_upsert_file(&conn, &ef1, 0).unwrap();

        let ef2 = extracted(
            "src/main.rs",
            vec![sym("foo", "crate::foo", 0x01, 1)],
            vec![],
        );
        apply_upsert_file(&conn, &ef2, 0).unwrap();

        let count_remaining = count(&conn, "SELECT COUNT(*) FROM symbols");
        assert_eq!(count_remaining, 1);
        let remaining_fqdn: String = conn
            .query_row("SELECT fqdn FROM symbols", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining_fqdn, "crate::foo");
    }

    #[test]
    fn upsert_file_promote_unresolved_within_batch() {
        let conn = fresh_conn();
        let edge = RawEdge {
            from_fqdn: "crate::caller".into(),
            kind: EdgeKind::Calls,
            to: ResolvedOrUnresolved::Resolved {
                fqdn: "crate::callee".into(),
            },
            sites: vec![],
            attributes: vec![],
            confidence: EdgeConfidence::Extracted,
        };
        let ef = extracted(
            "src/main.rs",
            vec![
                sym("caller", "crate::caller", 0x01, 1),
                sym("callee", "crate::callee", 0x02, 10),
            ],
            vec![edge],
        );
        apply_upsert_file(&conn, &ef, 0).unwrap();

        let (to_id, to_unresolved): (Option<i64>, Option<String>) = conn
            .query_row("SELECT to_symbol_id, to_unresolved FROM edges", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert!(to_id.is_some(), "in-batch promote must resolve to an id");
        assert!(to_unresolved.is_none());
    }

    #[test]
    fn upsert_file_skips_edges_from_unchanged_symbols() {
        let conn = fresh_conn();
        let edge = RawEdge {
            from_fqdn: "crate::foo".into(),
            kind: EdgeKind::Calls,
            to: ResolvedOrUnresolved::Unresolved {
                name: "target".into(),
            },
            sites: vec![],
            attributes: vec![],
            confidence: EdgeConfidence::Ambiguous,
        };
        let ef1 = extracted(
            "src/main.rs",
            vec![sym("foo", "crate::foo", 0x01, 1)],
            vec![edge.clone()],
        );
        apply_upsert_file(&conn, &ef1, 0).unwrap();
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM edges"), 1);

        let ef2 = extracted(
            "src/main.rs",
            vec![sym("foo", "crate::foo", 0x01, 50)],
            vec![edge],
        );
        apply_upsert_file(&conn, &ef2, 0).unwrap();
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM edges"),
            1,
            "edges from unchanged-body symbols must not be re-inserted"
        );
    }

    #[test]
    fn delete_file_removes_symbols_and_reverse_promotes_incoming_edges() {
        let conn = fresh_conn();
        let ef_caller = extracted(
            "src/caller.rs",
            vec![sym("caller", "crate::caller", 0x01, 1)],
            vec![RawEdge {
                from_fqdn: "crate::caller".into(),
                kind: EdgeKind::Calls,
                to: ResolvedOrUnresolved::Resolved {
                    fqdn: "crate::target".into(),
                },
                sites: vec![],
                attributes: vec![],
                confidence: EdgeConfidence::Extracted,
            }],
        );
        let ef_target = extracted(
            "src/target.rs",
            vec![sym("target", "crate::target", 0x02, 1)],
            vec![],
        );
        apply_upsert_file(&conn, &ef_target, 0).unwrap();
        apply_upsert_file(&conn, &ef_caller, 0).unwrap();

        apply_delete_file(&conn, "src/target.rs").unwrap();

        assert!(get_file(&conn, "src/target.rs").unwrap().is_none());
        let to_unresolved: Option<String> = conn
            .query_row("SELECT to_unresolved FROM edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(to_unresolved.as_deref(), Some("crate::target"));
    }

    #[test]
    fn delete_file_cleans_stale_edge_sites_in_other_files() {
        let conn = fresh_conn();
        let ef_target = extracted(
            "src/target.rs",
            vec![sym("target", "crate::target", 0x02, 1)],
            vec![],
        );
        let ef_caller = extracted(
            "src/caller.rs",
            vec![sym("caller", "crate::caller", 0x01, 1)],
            vec![RawEdge {
                from_fqdn: "crate::caller".into(),
                kind: EdgeKind::Calls,
                to: ResolvedOrUnresolved::Resolved {
                    fqdn: "crate::target".into(),
                },
                sites: vec![Site {
                    file: "src/main.rs".into(),
                    line: 7,
                    col: 4,
                }],
                attributes: vec![],
                confidence: EdgeConfidence::Extracted,
            }],
        );
        apply_upsert_file(&conn, &ef_target, 0).unwrap();
        apply_upsert_file(&conn, &ef_caller, 0).unwrap();

        apply_delete_file(&conn, "src/main.rs").unwrap();

        let remaining_main_sites: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM edge_sites WHERE file_path = 'src/main.rs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(remaining_main_sites, 0);
    }

    #[test]
    fn record_parse_error_sets_last_scan_error_on_existing_file() {
        let conn = fresh_conn();
        let ef = extracted(
            "src/main.rs",
            vec![sym("foo", "crate::foo", 0x01, 1)],
            vec![],
        );
        apply_upsert_file(&conn, &ef, 0).unwrap();

        record_parse_error(&conn, "src/main.rs", Language::Rust, "unexpected token").unwrap();

        let f = get_file(&conn, "src/main.rs").unwrap().unwrap();
        assert_eq!(f.last_scan_error.as_deref(), Some("unexpected token"));
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM symbols"),
            1,
            "old symbols must remain on parse error"
        );
    }

    #[test]
    fn record_parse_error_creates_files_row_for_unknown_path() {
        let conn = fresh_conn();
        record_parse_error(&conn, "src/new.rs", Language::Rust, "syntax error").unwrap();
        let f = get_file(&conn, "src/new.rs").unwrap().unwrap();
        assert_eq!(f.last_scan_error.as_deref(), Some("syntax error"));
        assert_eq!(f.byte_size, 0);
    }

    fn doc_for(fqdn: &str, description: &str) -> RawDocument {
        RawDocument {
            symbol_fqdn: fqdn.into(),
            description: description.into(),
        }
    }

    fn fetch_doc_description(conn: &Connection, fqdn: &str) -> Option<String> {
        let id: Option<i64> = conn
            .query_row("SELECT id FROM symbols WHERE fqdn = ?1", [fqdn], |r| {
                r.get(0)
            })
            .optional()
            .unwrap();
        let id = id?;
        let doc = get_document(conn, id).unwrap()?;
        doc.description
    }

    #[test]
    fn upsert_file_persists_documents_for_inserted_symbols() {
        let conn = fresh_conn();
        let mut ef = extracted(
            "src/main.rs",
            vec![sym("foo", "crate::foo", 0x01, 1)],
            vec![],
        );
        ef.documents = vec![doc_for("crate::foo", "Top-level helper.")];
        apply_upsert_file(&conn, &ef, 0).unwrap();

        let desc = fetch_doc_description(&conn, "crate::foo");
        assert_eq!(desc.as_deref(), Some("Top-level helper."));
    }

    #[test]
    fn upsert_file_replaces_documents_when_body_modified() {
        let conn = fresh_conn();
        let mut ef1 = extracted(
            "src/main.rs",
            vec![sym("foo", "crate::foo", 0x01, 1)],
            vec![],
        );
        ef1.documents = vec![doc_for("crate::foo", "v1 description.")];
        apply_upsert_file(&conn, &ef1, 0).unwrap();

        let mut ef2 = extracted(
            "src/main.rs",
            vec![sym("foo", "crate::foo", 0xee, 1)],
            vec![],
        );
        ef2.documents = vec![doc_for("crate::foo", "v2 description.")];
        apply_upsert_file(&conn, &ef2, 0).unwrap();

        assert_eq!(
            fetch_doc_description(&conn, "crate::foo").as_deref(),
            Some("v2 description.")
        );
    }

    #[test]
    fn upsert_file_removes_document_when_user_deletes_doc_comment() {
        let conn = fresh_conn();
        let mut ef1 = extracted(
            "src/main.rs",
            vec![sym("foo", "crate::foo", 0x01, 1)],
            vec![],
        );
        ef1.documents = vec![doc_for("crate::foo", "Original.")];
        apply_upsert_file(&conn, &ef1, 0).unwrap();
        assert!(fetch_doc_description(&conn, "crate::foo").is_some());

        // Body modified (different hash) AND no RawDocument provided → wipe the row.
        let ef2 = extracted(
            "src/main.rs",
            vec![sym("foo", "crate::foo", 0xee, 1)],
            vec![],
        );
        apply_upsert_file(&conn, &ef2, 0).unwrap();
        assert!(fetch_doc_description(&conn, "crate::foo").is_none());
    }

    #[test]
    fn delete_file_cascades_documents_via_fk() {
        let conn = fresh_conn();
        let mut ef = extracted(
            "src/main.rs",
            vec![sym("foo", "crate::foo", 0x01, 1)],
            vec![],
        );
        ef.documents = vec![doc_for("crate::foo", "Doc.")];
        apply_upsert_file(&conn, &ef, 0).unwrap();
        assert!(fetch_doc_description(&conn, "crate::foo").is_some());

        apply_delete_file(&conn, "src/main.rs").unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "FK ON DELETE CASCADE should wipe documents");
    }

    #[test]
    fn upsert_file_clears_last_scan_error_on_success() {
        let conn = fresh_conn();
        record_parse_error(&conn, "src/main.rs", Language::Rust, "syntax error").unwrap();
        let f1 = get_file(&conn, "src/main.rs").unwrap().unwrap();
        assert_eq!(f1.last_scan_error.as_deref(), Some("syntax error"));

        let ef = extracted(
            "src/main.rs",
            vec![sym("foo", "crate::foo", 0x01, 1)],
            vec![],
        );
        apply_upsert_file(&conn, &ef, 0).unwrap();

        let f2 = get_file(&conn, "src/main.rs").unwrap().unwrap();
        assert_eq!(f2.last_scan_error, None);
    }

    // --- IR-4-f: call_sites end-to-end through apply_upsert_file ---

    use crate::storage::call_sites::count_call_sites_by_file;
    use standardoc_ir::{RawCallArg, RawCallSite};

    fn extracted_with_call_sites(
        file: &str,
        symbols: Vec<RawSymbol>,
        call_sites: Vec<RawCallSite>,
    ) -> ExtractedFile {
        ExtractedFile {
            file: file.into(),
            language: Language::Rust,
            source_origin: SourceOrigin::Workspace,
            is_external: false,
            content_hash: Blake3Hash::new([0xab; 32]),
            byte_size: 4096,
            symbols,
            edges: vec![],
            call_sites,
            documents: vec![],
        }
    }

    fn cs(from_fqdn: &str, callee: &str, line: u32) -> RawCallSite {
        RawCallSite {
            from_fqdn: from_fqdn.into(),
            callee_text: callee.into(),
            args: vec![RawCallArg {
                value: "x".into(),
                is_string_literal: false,
            }],
            receiver_chain: vec![],
            site: Site {
                file: "src/main.rs".into(),
                line,
                col: 4,
            },
        }
    }

    #[test]
    fn ir4f_apply_upsert_file_persists_call_sites_vec_to_db() {
        // The IR-4-b/c/d extractors emit a `Vec<RawCallSite>` on
        // `ExtractedFile`; this test checks the batch pipeline actually
        // lands every record in the new `call_sites` table.
        let conn = fresh_conn();
        let ef = extracted_with_call_sites(
            "src/main.rs",
            vec![sym("caller", "crate::caller", 0x01, 1)],
            vec![
                cs("crate::caller", "foo", 5),
                cs("crate::caller", "bar", 7),
                cs("crate::caller", "baz", 9),
            ],
        );
        apply_upsert_file(&conn, &ef, 0).unwrap();
        assert_eq!(
            count_call_sites_by_file(&conn, "src/main.rs").unwrap(),
            3,
            "all three call_sites must reach the DB"
        );
    }

    #[test]
    fn ir4f_re_extract_replaces_call_sites_set() {
        // Second extract of the same file with a different call_sites
        // vec must DROP the old rows AND insert the new ones — no
        // accumulation, no stale entries.
        let conn = fresh_conn();
        let ef1 = extracted_with_call_sites(
            "src/main.rs",
            vec![sym("caller", "crate::caller", 0x01, 1)],
            vec![
                cs("crate::caller", "old_a", 5),
                cs("crate::caller", "old_b", 7),
            ],
        );
        apply_upsert_file(&conn, &ef1, 0).unwrap();
        assert_eq!(count_call_sites_by_file(&conn, "src/main.rs").unwrap(), 2);

        let ef2 = extracted_with_call_sites(
            "src/main.rs",
            vec![sym("caller", "crate::caller", 0x02, 1)],
            vec![cs("crate::caller", "new_one", 9)],
        );
        apply_upsert_file(&conn, &ef2, 1).unwrap();
        assert_eq!(
            count_call_sites_by_file(&conn, "src/main.rs").unwrap(),
            1,
            "re-extract must replace, not accumulate"
        );
        let callee: String = conn
            .query_row(
                "SELECT callee_text FROM call_sites WHERE file_path = ?1",
                ["src/main.rs"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(callee, "new_one");
    }

    #[test]
    fn ir4f_re_extract_with_empty_call_sites_clears_existing() {
        // If the extractor produces zero call_sites on a re-extract
        // (e.g. user deleted all function bodies), the table must be
        // purged for that file. Idempotency check for `apply_call_sites`.
        let conn = fresh_conn();
        let ef1 = extracted_with_call_sites(
            "src/main.rs",
            vec![sym("caller", "crate::caller", 0x01, 1)],
            vec![cs("crate::caller", "doomed", 5)],
        );
        apply_upsert_file(&conn, &ef1, 0).unwrap();
        assert_eq!(count_call_sites_by_file(&conn, "src/main.rs").unwrap(), 1);

        let ef2 = extracted_with_call_sites(
            "src/main.rs",
            vec![sym("caller", "crate::caller", 0x02, 1)],
            vec![],
        );
        apply_upsert_file(&conn, &ef2, 1).unwrap();
        assert_eq!(
            count_call_sites_by_file(&conn, "src/main.rs").unwrap(),
            0,
            "empty call_sites vec must purge existing rows"
        );
    }
}

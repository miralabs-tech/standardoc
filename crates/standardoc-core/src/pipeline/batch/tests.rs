use super::*;
use crate::storage::documents::get_document;
use crate::storage::files::get_file;
use crate::storage::module_lookup::PRIMARY_WORKSPACE_ID;
use crate::storage::test_utils::fresh_conn;
use standardoc_ir::{
    Blake3Hash, EdgeConfidence, EdgeKind, ExtractedFile, Kind, Language, LanguageKind, RawDocument,
    RawEdge, RawSymbol, ResolvedOrUnresolved, Site, SourceOrigin, SymbolLocation, Visibility,
};

fn sym(name: &str, fqdn: &str, hash_byte: u8, line: u32) -> RawSymbol {
    RawSymbol {
        decl_kind: None,
        implements_trait: None,
        receiver_type: None,
        entry_point: None,
        name: name.into(),
        fqdn: fqdn.into(),
        kind: Kind::Callable,
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
        ffi_bindings: vec![],
        module_lookup: None,
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
    apply_upsert_file(&conn, &ef, 0, PRIMARY_WORKSPACE_ID).unwrap();

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
    apply_upsert_file(&conn, &ef1, 0, PRIMARY_WORKSPACE_ID).unwrap();

    let ef2 = extracted(
        "src/main.rs",
        vec![sym("foo", "crate::foo", 0x01, 100)],
        vec![],
    );
    apply_upsert_file(&conn, &ef2, 0, PRIMARY_WORKSPACE_ID).unwrap();

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
    apply_upsert_file(&conn, &ef1, 0, PRIMARY_WORKSPACE_ID).unwrap();
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
    apply_upsert_file(&conn, &ef2, 0, PRIMARY_WORKSPACE_ID).unwrap();

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
    apply_upsert_file(&conn, &ef1, 0, PRIMARY_WORKSPACE_ID).unwrap();

    let ef2 = extracted(
        "src/main.rs",
        vec![sym("foo", "crate::foo", 0x01, 1)],
        vec![],
    );
    apply_upsert_file(&conn, &ef2, 0, PRIMARY_WORKSPACE_ID).unwrap();

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
    apply_upsert_file(&conn, &ef, 0, PRIMARY_WORKSPACE_ID).unwrap();

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
    apply_upsert_file(&conn, &ef1, 0, PRIMARY_WORKSPACE_ID).unwrap();
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM edges"), 1);

    let ef2 = extracted(
        "src/main.rs",
        vec![sym("foo", "crate::foo", 0x01, 50)],
        vec![edge],
    );
    apply_upsert_file(&conn, &ef2, 0, PRIMARY_WORKSPACE_ID).unwrap();
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
    apply_upsert_file(&conn, &ef_target, 0, PRIMARY_WORKSPACE_ID).unwrap();
    apply_upsert_file(&conn, &ef_caller, 0, PRIMARY_WORKSPACE_ID).unwrap();

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
    apply_upsert_file(&conn, &ef_target, 0, PRIMARY_WORKSPACE_ID).unwrap();
    apply_upsert_file(&conn, &ef_caller, 0, PRIMARY_WORKSPACE_ID).unwrap();

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
    apply_upsert_file(&conn, &ef, 0, PRIMARY_WORKSPACE_ID).unwrap();

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
    apply_upsert_file(&conn, &ef, 0, PRIMARY_WORKSPACE_ID).unwrap();

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
    apply_upsert_file(&conn, &ef1, 0, PRIMARY_WORKSPACE_ID).unwrap();

    let mut ef2 = extracted(
        "src/main.rs",
        vec![sym("foo", "crate::foo", 0xee, 1)],
        vec![],
    );
    ef2.documents = vec![doc_for("crate::foo", "v2 description.")];
    apply_upsert_file(&conn, &ef2, 0, PRIMARY_WORKSPACE_ID).unwrap();

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
    apply_upsert_file(&conn, &ef1, 0, PRIMARY_WORKSPACE_ID).unwrap();
    assert!(fetch_doc_description(&conn, "crate::foo").is_some());

    // Body modified (different hash) AND no RawDocument provided → wipe the row.
    let ef2 = extracted(
        "src/main.rs",
        vec![sym("foo", "crate::foo", 0xee, 1)],
        vec![],
    );
    apply_upsert_file(&conn, &ef2, 0, PRIMARY_WORKSPACE_ID).unwrap();
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
    apply_upsert_file(&conn, &ef, 0, PRIMARY_WORKSPACE_ID).unwrap();
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
    apply_upsert_file(&conn, &ef, 0, PRIMARY_WORKSPACE_ID).unwrap();

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
        ffi_bindings: vec![],
        module_lookup: None,
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
    apply_upsert_file(&conn, &ef, 0, PRIMARY_WORKSPACE_ID).unwrap();
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
    apply_upsert_file(&conn, &ef1, 0, PRIMARY_WORKSPACE_ID).unwrap();
    assert_eq!(count_call_sites_by_file(&conn, "src/main.rs").unwrap(), 2);

    let ef2 = extracted_with_call_sites(
        "src/main.rs",
        vec![sym("caller", "crate::caller", 0x02, 1)],
        vec![cs("crate::caller", "new_one", 9)],
    );
    apply_upsert_file(&conn, &ef2, 1, PRIMARY_WORKSPACE_ID).unwrap();
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
    apply_upsert_file(&conn, &ef1, 0, PRIMARY_WORKSPACE_ID).unwrap();
    assert_eq!(count_call_sites_by_file(&conn, "src/main.rs").unwrap(), 1);

    let ef2 = extracted_with_call_sites(
        "src/main.rs",
        vec![sym("caller", "crate::caller", 0x02, 1)],
        vec![],
    );
    apply_upsert_file(&conn, &ef2, 1, PRIMARY_WORKSPACE_ID).unwrap();
    assert_eq!(
        count_call_sites_by_file(&conn, "src/main.rs").unwrap(),
        0,
        "empty call_sites vec must purge existing rows"
    );
}

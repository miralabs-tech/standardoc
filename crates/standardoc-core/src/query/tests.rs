use super::body::strip_inline_comments_in_body;
use super::*;
use crate::commands::IngestCommand;

#[test]
fn strip_inline_comments_rust_line() {
    let input = "let x = 1; // trailing comment\n";
    assert_eq!(strip_inline_comments_in_body(input), "let x = 1; \n");
}

#[test]
fn strip_inline_comments_rust_block_inline() {
    let input = "fn foo(/* x */) {}";
    assert_eq!(strip_inline_comments_in_body(input), "fn foo() {}");
}

#[test]
fn strip_inline_comments_block_preserves_newlines() {
    let input = "fn foo() {\n    /* this is\n       a block */\n    x\n}";
    // Two newlines from the block contents survive so line numbers stay
    // aligned with the unstripped source.
    assert_eq!(
        strip_inline_comments_in_body(input),
        "fn foo() {\n    \n\n    x\n}"
    );
}

#[test]
fn strip_inline_comments_skips_inside_double_quoted_string() {
    let input = "let s = \"// not a comment\"; // real one\n";
    assert_eq!(
        strip_inline_comments_in_body(input),
        "let s = \"// not a comment\"; \n"
    );
}

#[test]
fn strip_inline_comments_skips_inside_raw_string() {
    let input = "let s = r#\"// not a comment\"#; // real one\n";
    assert_eq!(
        strip_inline_comments_in_body(input),
        "let s = r#\"// not a comment\"#; \n"
    );
}

#[test]
fn strip_inline_comments_skips_inside_ts_template_literal() {
    let input = "const url = `https://example.com`; // tail\n";
    assert_eq!(
        strip_inline_comments_in_body(input),
        "const url = `https://example.com`; \n"
    );
}

#[test]
fn strip_inline_comments_handles_consecutive_line_comments() {
    let input = "// one\n// two\nlet x = 1;\n";
    assert_eq!(strip_inline_comments_in_body(input), "\n\nlet x = 1;\n");
}

use crate::storage::edge_sites::insert_edge_sites;
use crate::storage::edges::insert_edge;
use crate::storage::files::{FileInput, upsert_file};
use crate::storage::module_lookup::PRIMARY_WORKSPACE_ID;
use crate::storage::symbols::{SymbolInsertContext, insert_symbol};
use rusqlite::Connection;
use standardoc_ir::{
    Blake3Hash, EdgeConfidence, EdgeKind, ExtractedFile, Kind, LanguageKind, Modifiers, Param,
    RawEdge, RawSymbol, Signature, Site, SourceOrigin, SymbolLocation, TypeRef, Visibility,
};
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn open_handle() -> (TempDir, IndexHandle) {
    let dir = tempfile::tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    (dir, handle)
}

fn wait_revision_at_least(handle: &IndexHandle, target: u64) {
    let start = Instant::now();
    while handle.revision() < target {
        assert!(
            start.elapsed() <= Duration::from_secs(5),
            "revision did not reach {target} (was {})",
            handle.revision()
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn seed_file(conn: &Connection, path: &str) {
    upsert_file(
        conn,
        &FileInput {
            path: path.into(),
            content_hash: Blake3Hash::default(),
            language: Language::Rust,
            byte_size: 100,
            last_scanned: 1_700_000_000_000,
            last_scan_error: None,
            is_external: false,
        },
    )
    .unwrap();
}

fn seed_symbol(
    conn: &Connection,
    file: &str,
    name: &str,
    fqdn: &str,
    line: u32,
) -> (i64, RawSymbol) {
    let sym = RawSymbol {
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
            file: file.into(),
            start_line: line,
            end_line: line + 5,
            start_col: 0,
            end_col: 1,
        },
        signature: None,
        body_hash: Some(Blake3Hash::new([0xab; 32])),
        attributes: vec![],
        flags: vec![],
    };
    let id = insert_symbol(
        conn,
        &sym,
        SymbolInsertContext {
            file_path: file,
            language: Language::Rust,
            is_external: false,
            source_origin: SourceOrigin::Workspace,
            revision: 0,
            workspace_id: PRIMARY_WORKSPACE_ID,
        },
    )
    .unwrap();
    (id, sym)
}

#[test]
fn symbol_by_fqdn_returns_some_when_present() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol(&conn, "src/main.rs", "foo", "crate::foo", 10);
    }
    let got = symbol_by_fqdn(&handle, "crate::foo").unwrap().unwrap();
    assert_eq!(got.name, "foo");
    assert_eq!(got.fqdn, "crate::foo");
    assert_eq!(got.kind, Kind::Callable);
    assert_eq!(got.location.start_line, 10);
}

#[test]
fn symbol_by_fqdn_returns_none_when_absent() {
    let (_dir, handle) = open_handle();
    assert_eq!(symbol_by_fqdn(&handle, "crate::ghost").unwrap(), None);
}

#[test]
fn symbol_by_fqdn_round_trips_signature_and_body_hash() {
    let (_dir, handle) = open_handle();
    let body = Blake3Hash::new([0xcd; 32]);
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        let sym = RawSymbol {
            decl_kind: None,
            implements_trait: None,
            receiver_type: None,
            entry_point: None,
            name: "f".into(),
            fqdn: "crate::f".into(),
            kind: Kind::Callable,
            language_kind: LanguageKind::from("fn_item"),
            module: Some("m".into()),
            visibility: Visibility::Crate,
            location: SymbolLocation {
                file: "src/main.rs".into(),
                start_line: 1,
                end_line: 2,
                start_col: 0,
                end_col: 1,
            },
            signature: Some(Signature {
                params: vec![Param {
                    name: "x".into(),
                    ty: TypeRef::new("u32"),
                    default: None,
                }],
                returns: Some(TypeRef::new("u32")),
                modifiers: Modifiers {
                    is_async: true,
                    ..Default::default()
                },
                ..Default::default()
            }),
            body_hash: Some(body),
            attributes: vec![],
            flags: vec![],
        };
        insert_symbol(
            &conn,
            &sym,
            SymbolInsertContext {
                file_path: "src/main.rs",
                language: Language::Rust,
                is_external: false,
                source_origin: SourceOrigin::Workspace,
                revision: 0,
                workspace_id: PRIMARY_WORKSPACE_ID,
            },
        )
        .unwrap();
    }
    let got = symbol_by_fqdn(&handle, "crate::f").unwrap().unwrap();
    assert_eq!(got.module.as_deref(), Some("m"));
    assert_eq!(got.visibility, Visibility::Crate);
    assert_eq!(got.body_hash, Some(body));
    let sig = got.signature.expect("signature must round-trip");
    assert!(sig.modifiers.is_async);
    assert_eq!(sig.params[0].name, "x");
}

// --- Stage 3b-7-b Layer 2: scope-aware lookup ---

/// Helper: insert a symbol with an explicit workspace_id tag.
/// Layer-2 tests need this because `seed_symbol` always stamps
/// `PRIMARY_WORKSPACE_ID`; isolation tests must stamp peer UUIDs.
fn seed_symbol_in_workspace(
    conn: &Connection,
    file: &str,
    name: &str,
    fqdn: &str,
    line: u32,
    workspace_id: &str,
) -> i64 {
    let sym = RawSymbol {
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
            file: file.into(),
            start_line: line,
            end_line: line + 5,
            start_col: 0,
            end_col: 1,
        },
        signature: None,
        body_hash: Some(Blake3Hash::new([0xab; 32])),
        attributes: vec![],
        flags: vec![],
    };
    insert_symbol(
        conn,
        &sym,
        SymbolInsertContext {
            file_path: file,
            language: Language::Rust,
            is_external: false,
            source_origin: SourceOrigin::Workspace,
            revision: 0,
            workspace_id,
        },
    )
    .unwrap()
}

#[test]
fn symbol_by_fqdn_in_workspace_returns_match_for_primary_scope() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol(&conn, "src/main.rs", "foo", "crate::foo", 10);
    }
    let got = symbol_by_fqdn_in_workspace(&handle, "crate::foo", PRIMARY_WORKSPACE_ID)
        .unwrap()
        .unwrap();
    assert_eq!(got.fqdn, "crate::foo");
}

#[test]
fn symbol_by_fqdn_in_workspace_returns_none_for_mismatched_scope() {
    // Primary row exists; lookup under a different workspace_id
    // must NOT see it — that's the whole point of scope-aware queries.
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol(&conn, "src/main.rs", "foo", "crate::foo", 10);
    }
    assert_eq!(
        symbol_by_fqdn_in_workspace(&handle, "crate::foo", "peer-uuid-abc").unwrap(),
        None
    );
}

#[test]
fn symbol_by_fqdn_in_workspace_isolates_peer_from_primary() {
    // Layer-2 isolation smoke: a primary row and a peer row with
    // distinct fqdns must each be visible only under their own
    // workspace scope. (Same-fqdn collision needs Layer 3's
    // UNIQUE(workspace_id, fqdn) — not in scope here.)
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol(&conn, "src/main.rs", "alpha", "primary::alpha", 1);
        seed_symbol_in_workspace(
            &conn,
            "src/main.rs",
            "beta",
            "peer::beta",
            2,
            "peer-uuid-xyz",
        );
    }
    // Primary scope sees alpha, not beta.
    assert!(
        symbol_by_fqdn_in_workspace(&handle, "primary::alpha", PRIMARY_WORKSPACE_ID)
            .unwrap()
            .is_some()
    );
    assert!(
        symbol_by_fqdn_in_workspace(&handle, "peer::beta", PRIMARY_WORKSPACE_ID)
            .unwrap()
            .is_none()
    );
    // Peer scope sees beta, not alpha.
    assert!(
        symbol_by_fqdn_in_workspace(&handle, "peer::beta", "peer-uuid-xyz")
            .unwrap()
            .is_some()
    );
    assert!(
        symbol_by_fqdn_in_workspace(&handle, "primary::alpha", "peer-uuid-xyz")
            .unwrap()
            .is_none()
    );
}

#[test]
fn symbols_by_name_returns_matches_ordered_by_fqdn_with_limit() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/a.rs");
        seed_file(&conn, "src/b.rs");
        seed_symbol(&conn, "src/b.rs", "tick", "crate::b::tick", 1);
        seed_symbol(&conn, "src/a.rs", "tick", "crate::a::tick", 1);
        seed_symbol(&conn, "src/a.rs", "other", "crate::a::other", 2);
    }
    let got = symbols_by_name(&handle, "tick", 50).unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].fqdn, "crate::a::tick");
    assert_eq!(got[1].fqdn, "crate::b::tick");

    let got_limited = symbols_by_name(&handle, "tick", 1).unwrap();
    assert_eq!(got_limited.len(), 1);
    assert_eq!(got_limited[0].fqdn, "crate::a::tick");
}

#[test]
fn symbols_by_name_empty_when_no_match() {
    let (_dir, handle) = open_handle();
    let got = symbols_by_name(&handle, "ghost", 50).unwrap();
    assert!(got.is_empty());
}

#[test]
fn symbols_by_file_orders_by_position() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol(&conn, "src/main.rs", "c", "crate::c", 30);
        seed_symbol(&conn, "src/main.rs", "a", "crate::a", 1);
        seed_symbol(&conn, "src/main.rs", "b", "crate::b", 15);
    }
    let got = symbols_by_file(&handle, "src/main.rs").unwrap();
    let fqdns: Vec<_> = got.iter().map(|s| s.fqdn.as_str()).collect();
    assert_eq!(fqdns, vec!["crate::a", "crate::b", "crate::c"]);
}

#[test]
fn edges_from_returns_resolved_and_unresolved_targets() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        let (caller_id, _) = seed_symbol(&conn, "src/main.rs", "caller", "crate::caller", 1);
        seed_symbol(&conn, "src/main.rs", "callee", "crate::callee", 10);
        insert_edge(
            &conn,
            caller_id,
            &RawEdge {
                from_fqdn: "crate::caller".into(),
                kind: EdgeKind::Calls,
                to: ResolvedOrUnresolved::Resolved {
                    fqdn: "crate::callee".into(),
                },
                sites: vec![],
                attributes: vec![],
                confidence: EdgeConfidence::default(),
                receiver_type: None,
            },
            "primary",
        )
        .unwrap();
        insert_edge(
            &conn,
            caller_id,
            &RawEdge {
                from_fqdn: "crate::caller".into(),
                kind: EdgeKind::Calls,
                to: ResolvedOrUnresolved::Unresolved {
                    name: "external::thing".into(),
                },
                sites: vec![],
                attributes: vec![],
                confidence: EdgeConfidence::default(),
                receiver_type: None,
            },
            "primary",
        )
        .unwrap();
    }
    let edges = edges_from(&handle, "crate::caller").unwrap();
    assert_eq!(edges.len(), 2);
    assert_eq!(edges[0].from_fqdn, "crate::caller");
    assert!(matches!(
        &edges[0].to,
        ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "crate::callee"
    ));
    assert!(matches!(
        &edges[1].to,
        ResolvedOrUnresolved::Unresolved { name } if name == "external::thing"
    ));
}

#[test]
fn edges_from_empty_when_symbol_unknown() {
    let (_dir, handle) = open_handle();
    let got = edges_from(&handle, "crate::ghost").unwrap();
    assert!(got.is_empty());
}

#[test]
fn edges_from_loads_sites_ordered() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        let (caller_id, _) = seed_symbol(&conn, "src/main.rs", "caller", "crate::caller", 1);
        let edge_id = insert_edge(
            &conn,
            caller_id,
            &RawEdge {
                from_fqdn: "crate::caller".into(),
                kind: EdgeKind::Calls,
                to: ResolvedOrUnresolved::Unresolved {
                    name: "thing".into(),
                },
                sites: vec![],
                attributes: vec![],
                confidence: EdgeConfidence::default(),
                receiver_type: None,
            },
            "primary",
        )
        .unwrap();
        insert_edge_sites(
            &conn,
            edge_id,
            &[
                Site {
                    file: "src/main.rs".into(),
                    line: 20,
                    col: 4,
                },
                Site {
                    file: "src/main.rs".into(),
                    line: 5,
                    col: 0,
                },
            ],
        )
        .unwrap();
    }
    let edges = edges_from(&handle, "crate::caller").unwrap();
    assert_eq!(edges.len(), 1);
    let sites = &edges[0].sites;
    assert_eq!(sites.len(), 2);
    assert_eq!(sites[0].line, 5);
    assert_eq!(sites[1].line, 20);
}

#[test]
fn edges_to_finds_resolved_inbound() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        let (caller_id, _) = seed_symbol(&conn, "src/main.rs", "caller", "crate::caller", 1);
        seed_symbol(&conn, "src/main.rs", "callee", "crate::callee", 10);
        insert_edge(
            &conn,
            caller_id,
            &RawEdge {
                from_fqdn: "crate::caller".into(),
                kind: EdgeKind::Calls,
                to: ResolvedOrUnresolved::Resolved {
                    fqdn: "crate::callee".into(),
                },
                sites: vec![],
                attributes: vec![],
                confidence: EdgeConfidence::default(),
                receiver_type: None,
            },
            "primary",
        )
        .unwrap();
    }
    let edges = edges_to(&handle, "crate::callee").unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].from_fqdn, "crate::caller");
    assert!(matches!(
        &edges[0].to,
        ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "crate::callee"
    ));
}

#[test]
fn edges_to_finds_unresolved_inbound_for_unknown_fqdn() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        let (caller_id, _) = seed_symbol(&conn, "src/main.rs", "caller", "crate::caller", 1);
        insert_edge(
            &conn,
            caller_id,
            &RawEdge {
                from_fqdn: "crate::caller".into(),
                kind: EdgeKind::Calls,
                to: ResolvedOrUnresolved::Unresolved {
                    name: "external::thing".into(),
                },
                sites: vec![],
                attributes: vec![],
                confidence: EdgeConfidence::default(),
                receiver_type: None,
            },
            "primary",
        )
        .unwrap();
    }
    let edges = edges_to(&handle, "external::thing").unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].from_fqdn, "crate::caller");
}

#[test]
fn sanitize_fts5_query_strips_hyphen_and_joins_with_space() {
    assert_eq!(sanitize_fts5_query("standardoc-cli"), "standardoc cli");
}

#[test]
fn sanitize_fts5_query_replaces_double_colon_with_space() {
    assert_eq!(sanitize_fts5_query("Type::method"), "Type method");
}

#[test]
fn sanitize_fts5_query_collapses_consecutive_specials() {
    assert_eq!(sanitize_fts5_query("foo---bar::baz"), "foo bar baz");
}

#[test]
fn sanitize_fts5_query_preserves_alphanumeric_and_underscore_unchanged() {
    assert_eq!(sanitize_fts5_query("my_func2"), "my_func2");
}

#[test]
fn sanitize_fts5_query_empty_for_only_special_chars() {
    assert_eq!(sanitize_fts5_query("---"), "");
    assert_eq!(sanitize_fts5_query(""), "");
    assert_eq!(sanitize_fts5_query("   "), "");
}

#[test]
fn or_fallback_expr_none_for_single_token() {
    assert_eq!(or_fallback_expr("merge_mcp_config"), None);
    assert_eq!(or_fallback_expr(""), None);
}

#[test]
fn or_fallback_expr_quotes_and_or_joins_multiple_tokens() {
    assert_eq!(
        or_fallback_expr("register_with_existing probe_existing_proxy").as_deref(),
        Some(r#""register_with_existing" OR "probe_existing_proxy""#),
    );
}

#[test]
fn search_text_matches_hyphenated_query() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol(
            &conn,
            "src/main.rs",
            "cli_entry",
            "standardoc_cli::cli_entry",
            1,
        );
    }
    let results = search_text(&handle, "standardoc-cli", 10, &SymbolFilter::default()).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].fqdn, "standardoc_cli::cli_entry");
}

#[test]
fn search_text_returns_empty_for_only_special_chars_query() {
    let (_dir, handle) = open_handle();
    let results = search_text(&handle, "---", 10, &SymbolFilter::default()).unwrap();
    assert!(results.is_empty());
}

#[test]
fn search_text_returns_match_via_fts() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol(
            &conn,
            "src/main.rs",
            "create_user",
            "crate::user::create_user",
            1,
        );
        seed_symbol(
            &conn,
            "src/main.rs",
            "delete_user",
            "crate::user::delete_user",
            5,
        );
        seed_symbol(&conn, "src/main.rs", "noise", "crate::noise", 10);
    }
    let got = search_text(&handle, "create_user", 50, &SymbolFilter::default()).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].name, "create_user");
}

#[test]
fn search_text_respects_limit() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol(&conn, "src/main.rs", "tick_one", "crate::tick_one", 1);
        seed_symbol(&conn, "src/main.rs", "tick_two", "crate::tick_two", 2);
    }
    let got = search_text(&handle, "tick_one OR tick_two", 1, &SymbolFilter::default()).unwrap();
    assert_eq!(got.len(), 1);
}

fn seed_symbol_full(
    conn: &Connection,
    file: &str,
    name: &str,
    fqdn: &str,
    kind: Kind,
    visibility: Visibility,
    module: Option<&str>,
) -> i64 {
    let sym = RawSymbol {
        decl_kind: None,
        implements_trait: None,
        receiver_type: None,
        entry_point: None,
        name: name.into(),
        fqdn: fqdn.into(),
        kind,
        language_kind: LanguageKind::from("fn_item"),
        module: module.map(str::to_string),
        visibility,
        location: SymbolLocation {
            file: file.into(),
            start_line: 1,
            end_line: 5,
            start_col: 0,
            end_col: 1,
        },
        signature: None,
        body_hash: Some(Blake3Hash::new([0xab; 32])),
        attributes: vec![],
        flags: vec![],
    };
    insert_symbol(
        conn,
        &sym,
        SymbolInsertContext {
            file_path: file,
            language: Language::Rust,
            is_external: false,
            source_origin: SourceOrigin::Workspace,
            revision: 0,
            workspace_id: PRIMARY_WORKSPACE_ID,
        },
    )
    .unwrap()
}

#[test]
fn search_text_filter_by_kind_excludes_other_kinds() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "marker",
            "crate::marker_fn",
            Kind::Callable,
            Visibility::Public,
            None,
        );
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "marker",
            "crate::marker_ty",
            Kind::Type,
            Visibility::Public,
            None,
        );
    }
    let only_types = SymbolFilter {
        kind: Some(Kind::Type),
        ..Default::default()
    };
    let got = search_text(&handle, "marker", 50, &only_types).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].fqdn, "crate::marker_ty");
}

#[test]
fn search_text_filter_by_visibility_excludes_other_vis() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "thing",
            "crate::thing_pub",
            Kind::Callable,
            Visibility::Public,
            None,
        );
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "thing",
            "crate::thing_priv",
            Kind::Callable,
            Visibility::Private,
            None,
        );
    }
    let only_private = SymbolFilter {
        visibility: Some(Visibility::Private),
        ..Default::default()
    };
    let got = search_text(&handle, "thing", 50, &only_private).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].fqdn, "crate::thing_priv");
}

#[test]
fn list_symbols_returns_all_when_filter_empty() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "a",
            "crate::a",
            Kind::Callable,
            Visibility::Public,
            None,
        );
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "b",
            "crate::b",
            Kind::Type,
            Visibility::Private,
            None,
        );
    }
    let got = list_symbols(&handle, &SymbolFilter::default(), 50, None).unwrap();
    assert_eq!(got.items.len(), 2);
    // Ordered by fqdn for stability.
    assert_eq!(got.items[0].fqdn, "crate::a");
    assert_eq!(got.items[1].fqdn, "crate::b");
    // Page wasn't full → no more pages.
    assert!(got.next_cursor.is_none());
}

#[test]
fn list_symbols_filter_by_visibility_private() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "pub_one",
            "crate::pub_one",
            Kind::Callable,
            Visibility::Public,
            None,
        );
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "priv_one",
            "crate::priv_one",
            Kind::Callable,
            Visibility::Private,
            None,
        );
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "priv_two",
            "crate::priv_two",
            Kind::Callable,
            Visibility::Private,
            None,
        );
    }
    let filter = SymbolFilter {
        visibility: Some(Visibility::Private),
        ..Default::default()
    };
    let got = list_symbols(&handle, &filter, 50, None).unwrap();
    assert_eq!(got.items.len(), 2);
    assert!(
        got.items
            .iter()
            .all(|s| s.visibility == Visibility::Private)
    );
    assert!(got.next_cursor.is_none());
}

#[test]
fn list_symbols_filter_by_module_exact_match() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "f1",
            "crate::a::f1",
            Kind::Callable,
            Visibility::Public,
            Some("crate::a"),
        );
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "f2",
            "crate::b::f2",
            Kind::Callable,
            Visibility::Public,
            Some("crate::b"),
        );
    }
    let filter = SymbolFilter {
        module: Some("crate::a".into()),
        ..Default::default()
    };
    let got = list_symbols(&handle, &filter, 50, None).unwrap();
    assert_eq!(got.items.len(), 1);
    assert_eq!(got.items[0].fqdn, "crate::a::f1");
    assert!(got.next_cursor.is_none());
}

#[test]
fn list_symbols_respects_limit() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        for i in 0..5 {
            seed_symbol_full(
                &conn,
                "src/main.rs",
                &format!("f{i}"),
                &format!("crate::f{i}"),
                Kind::Callable,
                Visibility::Public,
                None,
            );
        }
    }
    let got = list_symbols(&handle, &SymbolFilter::default(), 3, None).unwrap();
    assert_eq!(got.items.len(), 3);
    // Full page → cursor points at the last item, signalling more.
    assert_eq!(got.next_cursor.as_deref(), Some("crate::f2"));
}

#[test]
fn list_symbols_cursor_walks_full_set() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        for i in 0..5 {
            seed_symbol_full(
                &conn,
                "src/main.rs",
                &format!("f{i}"),
                &format!("crate::f{i}"),
                Kind::Callable,
                Visibility::Public,
                None,
            );
        }
    }
    // Walk every page until the cursor is exhausted, collecting
    // each fqdn exactly once.
    let mut cursor: Option<String> = None;
    let mut seen: Vec<String> = Vec::new();
    let mut iterations = 0_usize;
    loop {
        iterations += 1;
        assert!(iterations < 100, "pagination loop did not terminate");
        let page = list_symbols(&handle, &SymbolFilter::default(), 2, cursor.as_deref()).unwrap();
        for s in page.items {
            seen.push(s.fqdn);
        }
        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(
        seen,
        vec![
            "crate::f0",
            "crate::f1",
            "crate::f2",
            "crate::f3",
            "crate::f4"
        ],
    );
}

#[test]
fn list_symbols_cursor_skips_already_seen() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        for i in 0..4 {
            seed_symbol_full(
                &conn,
                "src/main.rs",
                &format!("f{i}"),
                &format!("crate::f{i}"),
                Kind::Callable,
                Visibility::Public,
                None,
            );
        }
    }
    // Start past the second item — cursor uses strict `>` so the
    // anchor fqdn itself is NOT included in the next page.
    let page = list_symbols(&handle, &SymbolFilter::default(), 10, Some("crate::f1")).unwrap();
    let fqdns: Vec<&str> = page.items.iter().map(|s| s.fqdn.as_str()).collect();
    assert_eq!(fqdns, vec!["crate::f2", "crate::f3"]);
    assert!(page.next_cursor.is_none());
}

#[test]
fn find_by_pattern_glob_matches_name_wildcard() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "strip_rs_extension",
            "crate::a::strip_rs_extension",
            Kind::Callable,
            Visibility::Private,
            None,
        );
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "strip_ts_extension",
            "crate::b::strip_ts_extension",
            Kind::Callable,
            Visibility::Private,
            None,
        );
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "compute_path",
            "crate::c::compute_path",
            Kind::Callable,
            Visibility::Public,
            None,
        );
    }
    let got = find_by_pattern(&handle, "strip_*_extension", &SymbolFilter::default(), 50).unwrap();
    let names: Vec<&str> = got.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["strip_rs_extension", "strip_ts_extension"]);
}

#[test]
fn find_by_pattern_glob_matches_fqdn_path() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "do_a",
            "myapp::utils::do_a",
            Kind::Callable,
            Visibility::Public,
            None,
        );
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "do_b",
            "myapp::utils::do_b",
            Kind::Callable,
            Visibility::Public,
            None,
        );
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "do_c",
            "other::do_c",
            Kind::Callable,
            Visibility::Public,
            None,
        );
    }
    let got = find_by_pattern(&handle, "myapp::utils::*", &SymbolFilter::default(), 50).unwrap();
    assert_eq!(got.len(), 2);
    assert!(got.iter().all(|s| s.fqdn.starts_with("myapp::utils::")));
}

#[test]
fn find_by_pattern_combines_pattern_and_visibility_filter() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "helper_one",
            "crate::helper_one",
            Kind::Callable,
            Visibility::Private,
            None,
        );
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "helper_two",
            "crate::helper_two",
            Kind::Callable,
            Visibility::Public,
            None,
        );
    }
    let filter = SymbolFilter {
        visibility: Some(Visibility::Private),
        ..Default::default()
    };
    let got = find_by_pattern(&handle, "helper_*", &filter, 50).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].name, "helper_one");
}

#[test]
fn find_by_pattern_no_match_returns_empty() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "foo",
            "crate::foo",
            Kind::Callable,
            Visibility::Public,
            None,
        );
    }
    let got = find_by_pattern(&handle, "nope_*", &SymbolFilter::default(), 50).unwrap();
    assert!(got.is_empty());
}

#[test]
fn find_similar_ranks_template_family_above_unrelated() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "strip_rs_extension",
            "crate::a::strip_rs_extension",
            Kind::Callable,
            Visibility::Public,
            None,
        );
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "strip_ts_extension",
            "crate::b::strip_ts_extension",
            Kind::Callable,
            Visibility::Public,
            None,
        );
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "strip_lua_extension",
            "crate::c::strip_lua_extension",
            Kind::Callable,
            Visibility::Public,
            None,
        );
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "render_widget",
            "crate::d::render_widget",
            Kind::Callable,
            Visibility::Public,
            None,
        );
    }
    let got = find_similar(
        &handle,
        "strip_rs_extension",
        0.8,
        &SymbolFilter::default(),
        50,
    )
    .unwrap();
    let names: Vec<&str> = got.iter().map(|(s, _)| s.name.as_str()).collect();
    assert!(
        names.contains(&"strip_ts_extension") && names.contains(&"strip_lua_extension"),
        "expected templated family in result, got {names:?}"
    );
    assert!(
        !names.contains(&"render_widget"),
        "unrelated name must be filtered by threshold, got {names:?}"
    );
}

#[test]
fn find_similar_self_skips_anchor_by_name() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "strip_rs_extension",
            "crate::a::strip_rs_extension",
            Kind::Callable,
            Visibility::Public,
            None,
        );
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "strip_rs_extension",
            "crate::b::strip_rs_extension",
            Kind::Callable,
            Visibility::Public,
            None,
        );
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "strip_ts_extension",
            "crate::c::strip_ts_extension",
            Kind::Callable,
            Visibility::Public,
            None,
        );
    }
    let got = find_similar(
        &handle,
        "strip_rs_extension",
        0.5,
        &SymbolFilter::default(),
        50,
    )
    .unwrap();
    // Both `strip_rs_extension` collisions are skipped (case-insensitive
    // self-skip); only the templated cousin remains.
    let names: Vec<&str> = got.iter().map(|(s, _)| s.name.as_str()).collect();
    assert_eq!(names, vec!["strip_ts_extension"]);
}

#[test]
fn find_similar_orders_by_score_descending_then_fqdn() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        // Closest cousin: 1-char-diff
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "strip_ts_extension",
            "crate::a::strip_ts_extension",
            Kind::Callable,
            Visibility::Public,
            None,
        );
        // Slightly weaker cousin: 3-chars-diff
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "strip_lua_extension",
            "crate::b::strip_lua_extension",
            Kind::Callable,
            Visibility::Public,
            None,
        );
    }
    let got = find_similar(
        &handle,
        "strip_rs_extension",
        0.5,
        &SymbolFilter::default(),
        50,
    )
    .unwrap();
    assert_eq!(got.len(), 2);
    assert!(got[0].1 >= got[1].1, "results must be sorted by score desc");
    assert_eq!(got[0].0.name, "strip_ts_extension");
    assert_eq!(got[1].0.name, "strip_lua_extension");
}

#[test]
fn find_similar_threshold_filters_low_scores() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "buy_apple",
            "crate::a::buy_apple",
            Kind::Callable,
            Visibility::Public,
            None,
        );
    }
    let got = find_similar(&handle, "render_widget", 0.95, &SymbolFilter::default(), 50).unwrap();
    assert!(got.is_empty(), "high threshold must drop unrelated names");
}

#[test]
fn find_similar_filter_applied_before_scoring() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "strip_ts_extension",
            "crate::a::strip_ts_extension",
            Kind::Callable,
            Visibility::Public,
            None,
        );
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "strip_lua_extension",
            "crate::b::strip_lua_extension",
            Kind::Callable,
            Visibility::Private,
            None,
        );
    }
    let filter = SymbolFilter {
        visibility: Some(Visibility::Public),
        ..Default::default()
    };
    let got = find_similar(&handle, "strip_rs_extension", 0.5, &filter, 50).unwrap();
    let names: Vec<&str> = got.iter().map(|(s, _)| s.name.as_str()).collect();
    assert_eq!(names, vec!["strip_ts_extension"]);
}

#[test]
fn find_similar_respects_limit_after_sort() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        for label in ["ts", "lua", "py", "go", "js"] {
            let name = format!("strip_{label}_extension");
            let fqdn = format!("crate::{label}::strip_{label}_extension");
            seed_symbol_full(
                &conn,
                "src/main.rs",
                &name,
                &fqdn,
                Kind::Callable,
                Visibility::Public,
                None,
            );
        }
    }
    let got = find_similar(
        &handle,
        "strip_rs_extension",
        0.0,
        &SymbolFilter::default(),
        2,
    )
    .unwrap();
    assert_eq!(got.len(), 2);
}

#[test]
fn find_similar_empty_index_returns_empty() {
    let (_dir, handle) = open_handle();
    let got = find_similar(&handle, "anything", 0.5, &SymbolFilter::default(), 50).unwrap();
    assert!(got.is_empty());
}

#[test]
fn find_similar_module_filter_scopes_search() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "strip_ts_extension",
            "crate::a::strip_ts_extension",
            Kind::Callable,
            Visibility::Public,
            Some("crate::a"),
        );
        seed_symbol_full(
            &conn,
            "src/main.rs",
            "strip_lua_extension",
            "crate::b::strip_lua_extension",
            Kind::Callable,
            Visibility::Public,
            Some("crate::b"),
        );
    }
    let filter = SymbolFilter {
        module: Some("crate::a".into()),
        ..Default::default()
    };
    let got = find_similar(&handle, "strip_rs_extension", 0.5, &filter, 50).unwrap();
    let names: Vec<&str> = got.iter().map(|(s, _)| s.name.as_str()).collect();
    assert_eq!(names, vec!["strip_ts_extension"]);
}

#[test]
fn file_info_returns_some_with_data() {
    let (_dir, handle) = open_handle();
    let hash_hex = Blake3Hash::new([0xab; 32]).to_hex();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        upsert_file(
            &conn,
            &FileInput {
                path: "src/main.rs".into(),
                content_hash: Blake3Hash::new([0xab; 32]),
                language: Language::TypeScript,
                byte_size: 4096,
                last_scanned: 1_700_000_000_000,
                last_scan_error: Some("boom".into()),
                is_external: false,
            },
        )
        .unwrap();
    }
    let got = file_info(&handle, "src/main.rs").unwrap().unwrap();
    assert_eq!(got.path, "src/main.rs");
    assert_eq!(got.content_hash, hash_hex);
    assert_eq!(got.language, Language::TypeScript);
    assert_eq!(got.byte_size, 4096);
    assert_eq!(got.last_scanned_ms, 1_700_000_000_000);
    assert_eq!(got.last_scan_error.as_deref(), Some("boom"));
}

#[test]
fn file_info_returns_none_when_absent() {
    let (_dir, handle) = open_handle();
    assert_eq!(file_info(&handle, "no/such.rs").unwrap(), None);
}

#[test]
fn query_observes_writer_thread_upsert() {
    let (_dir, handle) = open_handle();
    let extracted = ExtractedFile {
        file: "src/main.rs".into(),
        language: Language::Rust,
        source_origin: SourceOrigin::Workspace,
        is_external: false,
        content_hash: Blake3Hash::new([0xee; 32]),
        byte_size: 100,
        module_lookup: None,
        symbols: vec![RawSymbol {
            decl_kind: None,
            implements_trait: None,
            receiver_type: None,
            entry_point: None,
            name: "boot".into(),
            fqdn: "crate::boot".into(),
            kind: Kind::Callable,
            language_kind: LanguageKind::from("fn_item"),
            module: None,
            visibility: Visibility::Public,
            location: SymbolLocation {
                file: "src/main.rs".into(),
                start_line: 1,
                end_line: 2,
                start_col: 0,
                end_col: 1,
            },
            signature: None,
            body_hash: Some(Blake3Hash::new([0x02; 32])),
            attributes: vec![],
            flags: vec![],
        }],
        edges: vec![],
        call_sites: vec![],
        documents: vec![],
        ffi_bindings: vec![],
    };
    handle
        .submit_blocking(IngestCommand::UpsertFile {
            path: "src/main.rs".into(),
            extracted,
        })
        .unwrap();
    wait_revision_at_least(&handle, 1);

    let got = symbol_by_fqdn(&handle, "crate::boot").unwrap().unwrap();
    assert_eq!(got.name, "boot");
}

fn ranged_loc(
    file: &str,
    start_line: u32,
    end_line: u32,
    start_col: u32,
    end_col: u32,
) -> SymbolLocation {
    SymbolLocation {
        file: file.into(),
        start_line,
        end_line,
        start_col,
        end_col,
    }
}

fn insert_ranged_symbol(conn: &Connection, fqdn: &str, kind: Kind, location: SymbolLocation) {
    let file = location.file.clone();
    let sym = RawSymbol {
        decl_kind: None,
        implements_trait: None,
        receiver_type: None,
        entry_point: None,
        name: fqdn.rsplit("::").next().unwrap_or(fqdn).into(),
        fqdn: fqdn.into(),
        kind,
        language_kind: LanguageKind::from("fn_item"),
        module: None,
        visibility: Visibility::Public,
        location,
        signature: None,
        body_hash: None,
        attributes: vec![],
        flags: vec![],
    };
    insert_symbol(
        conn,
        &sym,
        SymbolInsertContext {
            file_path: &file,
            language: Language::Rust,
            is_external: false,
            source_origin: SourceOrigin::Workspace,
            revision: 0,
            workspace_id: PRIMARY_WORKSPACE_ID,
        },
    )
    .unwrap();
}

#[test]
fn symbol_at_position_returns_match_when_in_range() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        insert_ranged_symbol(
            &conn,
            "crate::foo",
            Kind::Callable,
            ranged_loc("src/main.rs", 10, 20, 0, 1),
        );
    }
    let got = symbol_at_position(&handle, "src/main.rs", 15, 0)
        .unwrap()
        .expect("position 15:0 lies inside the function body");
    assert_eq!(got.fqdn, "crate::foo");
}

#[test]
fn symbol_at_position_picks_smallest_range_when_nested() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/lib.rs");
        insert_ranged_symbol(
            &conn,
            "crate::outer",
            Kind::Module,
            ranged_loc("src/lib.rs", 1, 100, 0, 0),
        );
        insert_ranged_symbol(
            &conn,
            "crate::outer::inner",
            Kind::Callable,
            ranged_loc("src/lib.rs", 10, 20, 4, 1),
        );
    }
    let got = symbol_at_position(&handle, "src/lib.rs", 15, 0)
        .unwrap()
        .expect("position lies inside both module and inner fn");
    assert_eq!(got.fqdn, "crate::outer::inner");
}

#[test]
fn symbol_at_position_returns_none_when_out_of_range() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        insert_ranged_symbol(
            &conn,
            "crate::foo",
            Kind::Callable,
            ranged_loc("src/main.rs", 10, 20, 0, 1),
        );
    }
    assert_eq!(
        symbol_at_position(&handle, "src/main.rs", 100, 0).unwrap(),
        None
    );
}

#[test]
fn context_for_symbol_returns_none_when_unknown() {
    let (_dir, handle) = open_handle();
    assert_eq!(context_for_symbol(&handle, "crate::ghost").unwrap(), None);
}

#[test]
fn context_for_symbol_returns_symbol_only_when_no_metadata() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol(&conn, "src/main.rs", "foo", "crate::foo", 1);
    }
    let ctx = context_for_symbol(&handle, "crate::foo")
        .unwrap()
        .expect("symbol exists");
    assert_eq!(ctx.symbol.fqdn, "crate::foo");
    assert_eq!(ctx.enrichment_description, None);
    assert_eq!(ctx.document_description, None);
}

#[test]
fn context_for_symbol_aggregates_enrichment_and_document() {
    use crate::storage::documents::{DocumentInput, upsert_document};
    use crate::storage::enrichments::{ConfidenceLevel, EnrichmentInput, upsert_enrichment};

    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        let (id, _) = seed_symbol(&conn, "src/main.rs", "foo", "crate::foo", 1);
        upsert_enrichment(
            &conn,
            &EnrichmentInput {
                symbol_id: id,
                description: Some("inferred summary".into()),
                params_json: None,
                returns_json: None,
                modifiers_json: None,
                confidence: ConfidenceLevel::High,
                sources_json: "[]".into(),
                last_updated: 0,
            },
        )
        .unwrap();
        upsert_document(
            &conn,
            &DocumentInput {
                symbol_id: id,
                description: Some("user-authored doc".into()),
                ..DocumentInput::default()
            },
        )
        .unwrap();
    }
    let ctx = context_for_symbol(&handle, "crate::foo")
        .unwrap()
        .expect("symbol exists");
    assert_eq!(ctx.symbol.fqdn, "crate::foo");
    assert_eq!(
        ctx.enrichment_description.as_deref(),
        Some("inferred summary")
    );
    assert_eq!(
        ctx.document_description.as_deref(),
        Some("user-authored doc")
    );
}

fn seed_call_edge(conn: &Connection, from_id: i64, from_fqdn: &str, to: ResolvedOrUnresolved) {
    insert_edge(
        conn,
        from_id,
        &RawEdge {
            from_fqdn: from_fqdn.into(),
            kind: EdgeKind::Calls,
            to,
            sites: vec![],
            attributes: vec![],
            confidence: EdgeConfidence::default(),
            receiver_type: None,
        },
        "primary",
    )
    .unwrap();
}

fn seed_import_edge(conn: &Connection, from_id: i64, from_fqdn: &str, to: ResolvedOrUnresolved) {
    insert_edge(
        conn,
        from_id,
        &RawEdge {
            from_fqdn: from_fqdn.into(),
            kind: EdgeKind::Imports,
            to,
            sites: vec![],
            attributes: vec![],
            confidence: EdgeConfidence::default(),
            receiver_type: None,
        },
        "primary",
    )
    .unwrap();
}

#[test]
fn context_with_neighbors_returns_none_when_unknown() {
    let (_dir, handle) = open_handle();
    assert_eq!(
        context_for_symbol_with_neighbors(&handle, "crate::ghost", 1).unwrap(),
        None
    );
}

#[test]
fn context_with_neighbors_groups_edges_by_kind_and_direction() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        let (foo_id, _) = seed_symbol(&conn, "src/main.rs", "foo", "crate::foo", 1);
        let (bar_id, _) = seed_symbol(&conn, "src/main.rs", "bar", "crate::bar", 10);
        seed_symbol(&conn, "src/main.rs", "baz", "crate::baz", 20);
        seed_call_edge(
            &conn,
            foo_id,
            "crate::foo",
            ResolvedOrUnresolved::Resolved {
                fqdn: "crate::baz".into(),
            },
        );
        seed_call_edge(
            &conn,
            foo_id,
            "crate::foo",
            ResolvedOrUnresolved::Unresolved {
                name: "external::thing".into(),
            },
        );
        seed_import_edge(
            &conn,
            foo_id,
            "crate::foo",
            ResolvedOrUnresolved::Resolved {
                fqdn: "crate::baz".into(),
            },
        );
        seed_call_edge(
            &conn,
            bar_id,
            "crate::bar",
            ResolvedOrUnresolved::Resolved {
                fqdn: "crate::foo".into(),
            },
        );
        seed_import_edge(
            &conn,
            bar_id,
            "crate::bar",
            ResolvedOrUnresolved::Resolved {
                fqdn: "crate::foo".into(),
            },
        );
    }
    let ctx = context_for_symbol_with_neighbors(&handle, "crate::foo", 1)
        .unwrap()
        .expect("foo exists");
    assert_eq!(ctx.context.symbol.fqdn, "crate::foo");
    assert_eq!(
        ctx.callees.len(),
        2,
        "callees include resolved + unresolved"
    );
    assert_eq!(ctx.imports.len(), 1);
    assert_eq!(ctx.callers.len(), 1);
    assert_eq!(ctx.imported_by.len(), 1);
    assert!(matches!(
        &ctx.callers[0].target,
        ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "crate::bar"
    ));
    assert!(matches!(
        &ctx.imported_by[0].target,
        ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "crate::bar"
    ));
}

#[test]
fn context_with_neighbors_depth_one_omits_resolved_symbol() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        let (foo_id, _) = seed_symbol(&conn, "src/main.rs", "foo", "crate::foo", 1);
        seed_symbol(&conn, "src/main.rs", "baz", "crate::baz", 20);
        seed_call_edge(
            &conn,
            foo_id,
            "crate::foo",
            ResolvedOrUnresolved::Resolved {
                fqdn: "crate::baz".into(),
            },
        );
    }
    let ctx = context_for_symbol_with_neighbors(&handle, "crate::foo", 1)
        .unwrap()
        .unwrap();
    assert_eq!(ctx.callees.len(), 1);
    assert!(
        ctx.callees[0].resolved_symbol.is_none(),
        "depth=1 must keep resolved_symbol = None even for Resolved targets"
    );
}

#[test]
fn context_with_neighbors_depth_two_populates_resolved_symbol_for_resolved_only() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        let (foo_id, _) = seed_symbol(&conn, "src/main.rs", "foo", "crate::foo", 1);
        seed_symbol(&conn, "src/main.rs", "baz", "crate::baz", 20);
        seed_call_edge(
            &conn,
            foo_id,
            "crate::foo",
            ResolvedOrUnresolved::Resolved {
                fqdn: "crate::baz".into(),
            },
        );
        seed_call_edge(
            &conn,
            foo_id,
            "crate::foo",
            ResolvedOrUnresolved::Unresolved {
                name: "external::thing".into(),
            },
        );
    }
    let ctx = context_for_symbol_with_neighbors(&handle, "crate::foo", 2)
        .unwrap()
        .unwrap();
    assert_eq!(ctx.callees.len(), 2);
    let (resolved, unresolved): (Vec<_>, Vec<_>) = ctx
        .callees
        .iter()
        .partition(|n| matches!(n.target, ResolvedOrUnresolved::Resolved { .. }));
    let baz = resolved.first().expect("Resolved neighbor present");
    assert_eq!(
        baz.resolved_symbol.as_ref().map(|s| s.fqdn.as_str()),
        Some("crate::baz")
    );
    let external = unresolved.first().expect("Unresolved neighbor present");
    assert!(
        external.resolved_symbol.is_none(),
        "Unresolved targets stay None even at depth=2"
    );
}

#[test]
fn context_with_neighbors_clamps_depth_above_two() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        let (foo_id, _) = seed_symbol(&conn, "src/main.rs", "foo", "crate::foo", 1);
        seed_symbol(&conn, "src/main.rs", "baz", "crate::baz", 20);
        seed_call_edge(
            &conn,
            foo_id,
            "crate::foo",
            ResolvedOrUnresolved::Resolved {
                fqdn: "crate::baz".into(),
            },
        );
    }
    let ctx_clamped = context_for_symbol_with_neighbors(&handle, "crate::foo", 99)
        .unwrap()
        .unwrap();
    let ctx_two = context_for_symbol_with_neighbors(&handle, "crate::foo", 2)
        .unwrap()
        .unwrap();
    assert_eq!(ctx_clamped, ctx_two, "depth >= 2 must collapse to depth=2");
}

#[test]
fn context_with_neighbors_skips_other_edge_kinds() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        let (foo_id, _) = seed_symbol(&conn, "src/main.rs", "foo", "crate::foo", 1);
        seed_symbol(&conn, "src/main.rs", "trait_t", "crate::T", 30);
        insert_edge(
            &conn,
            foo_id,
            &RawEdge {
                from_fqdn: "crate::foo".into(),
                kind: EdgeKind::Implements,
                to: ResolvedOrUnresolved::Resolved {
                    fqdn: "crate::T".into(),
                },
                sites: vec![],
                attributes: vec![],
                confidence: EdgeConfidence::default(),
                receiver_type: None,
            },
            "primary",
        )
        .unwrap();
    }
    let ctx = context_for_symbol_with_neighbors(&handle, "crate::foo", 2)
        .unwrap()
        .unwrap();
    assert!(ctx.callees.is_empty());
    assert!(ctx.imports.is_empty());
    assert!(ctx.callers.is_empty());
    assert!(ctx.imported_by.is_empty());
}

// ────────────────────────────────────────────────────────────────
// L3e-1: scope-aware query layer (workspace_id filter)
// ────────────────────────────────────────────────────────────────

#[test]
fn search_text_defaults_to_primary_scope_when_workspace_id_is_none() {
    // L3e-1: with `workspace_id=None` the FTS query narrows to
    // primary, peer rows are invisible. Matches "give me MY
    // symbols" default.
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol(&conn, "src/main.rs", "alpha", "primary::alpha", 1);
        seed_symbol_in_workspace(
            &conn,
            "src/main.rs",
            "alpha",
            "peer::alpha",
            10,
            "peer-uuid-l3e1",
        );
    }
    let got = search_text(&handle, "alpha", 50, &SymbolFilter::default()).unwrap();
    let fqdns: Vec<&str> = got.iter().map(|s| s.fqdn.as_str()).collect();
    assert_eq!(
        fqdns,
        vec!["primary::alpha"],
        "default scope must hide peer rows"
    );
}

#[test]
fn search_text_explicit_workspace_id_returns_peer_rows_only() {
    // L3e-1: with `workspace_id=Some(peer)`, primary rows are
    // invisible and only the matching peer row surfaces.
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol(&conn, "src/main.rs", "alpha", "primary::alpha", 1);
        seed_symbol_in_workspace(
            &conn,
            "src/main.rs",
            "alpha",
            "peer::alpha",
            10,
            "peer-uuid-l3e2",
        );
    }
    let filter = SymbolFilter {
        workspace_id: Some("peer-uuid-l3e2".into()),
        ..Default::default()
    };
    let got = search_text(&handle, "alpha", 50, &filter).unwrap();
    let fqdns: Vec<&str> = got.iter().map(|s| s.fqdn.as_str()).collect();
    assert_eq!(fqdns, vec!["peer::alpha"], "peer scope must hide primary");
}

#[test]
fn find_by_pattern_defaults_to_primary_scope() {
    // L3e-1: same default-primary semantics for GLOB pattern queries.
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol(&conn, "src/main.rs", "helper_a", "primary::helper_a", 1);
        seed_symbol_in_workspace(
            &conn,
            "src/main.rs",
            "helper_b",
            "peer::helper_b",
            10,
            "peer-uuid-l3e3",
        );
    }
    let got = find_by_pattern(&handle, "helper_*", &SymbolFilter::default(), 50).unwrap();
    let fqdns: Vec<&str> = got.iter().map(|s| s.fqdn.as_str()).collect();
    assert_eq!(fqdns, vec!["primary::helper_a"]);
}

#[test]
fn find_by_pattern_explicit_workspace_id_returns_peer_match_only() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol(&conn, "src/main.rs", "helper_a", "primary::helper_a", 1);
        seed_symbol_in_workspace(
            &conn,
            "src/main.rs",
            "helper_b",
            "peer::helper_b",
            10,
            "peer-uuid-l3e4",
        );
    }
    let filter = SymbolFilter {
        workspace_id: Some("peer-uuid-l3e4".into()),
        ..Default::default()
    };
    let got = find_by_pattern(&handle, "helper_*", &filter, 50).unwrap();
    let fqdns: Vec<&str> = got.iter().map(|s| s.fqdn.as_str()).collect();
    assert_eq!(fqdns, vec!["peer::helper_b"]);
}

#[test]
fn list_symbols_defaults_to_primary_scope() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol(&conn, "src/main.rs", "alpha", "primary::alpha", 1);
        seed_symbol_in_workspace(
            &conn,
            "src/main.rs",
            "beta",
            "peer::beta",
            10,
            "peer-uuid-l3e5",
        );
    }
    let page = list_symbols(&handle, &SymbolFilter::default(), 50, None).unwrap();
    let fqdns: Vec<&str> = page.items.iter().map(|s| s.fqdn.as_str()).collect();
    assert_eq!(fqdns, vec!["primary::alpha"]);
}

#[test]
fn list_symbols_explicit_workspace_id_returns_peer_rows_only() {
    let (_dir, handle) = open_handle();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        seed_file(&conn, "src/main.rs");
        seed_symbol(&conn, "src/main.rs", "alpha", "primary::alpha", 1);
        seed_symbol_in_workspace(
            &conn,
            "src/main.rs",
            "beta",
            "peer::beta",
            10,
            "peer-uuid-l3e6",
        );
    }
    let filter = SymbolFilter {
        workspace_id: Some("peer-uuid-l3e6".into()),
        ..Default::default()
    };
    let page = list_symbols(&handle, &filter, 50, None).unwrap();
    let fqdns: Vec<&str> = page.items.iter().map(|s| s.fqdn.as_str()).collect();
    assert_eq!(fqdns, vec!["peer::beta"]);
}

#[test]
fn symbol_filter_effective_workspace_id_defaults_to_primary() {
    // Pure unit test for the helper — None resolves to the
    // PRIMARY_WORKSPACE_ID sentinel, Some round-trips through.
    let default = SymbolFilter::default();
    assert_eq!(default.effective_workspace_id(), PRIMARY_WORKSPACE_ID);

    let explicit = SymbolFilter {
        workspace_id: Some("abc".into()),
        ..Default::default()
    };
    assert_eq!(explicit.effective_workspace_id(), "abc");
}

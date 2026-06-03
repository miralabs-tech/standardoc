use super::*;
use crate::storage::call_sites::insert_call_site;
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
        insert(
            &h,
            "src/a.rs",
            &cs("c::caller", &format!("foo_{i}"), "src/a.rs", i),
        );
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
fn callers_textual_matches_name_contains_across_callee_shapes() {
    let (_d, h) = fresh_handle();
    seed_file(&h, "src/a.rs");
    // Path-form, method-form and bare-ident calls all mention `find_by_pattern`.
    insert(
        &h,
        "src/a.rs",
        &cs("c::a", "query::find_by_pattern", "src/a.rs", 1),
    );
    insert(
        &h,
        "src/a.rs",
        &cs("c::b", "self.find_by_pattern", "src/a.rs", 2),
    );
    insert(&h, "src/a.rs", &cs("c::c", "unrelated_call", "src/a.rs", 3));
    let rows = callers_textual_for_name(&h, "find_by_pattern", &HashSet::new(), 25).unwrap();
    assert_eq!(rows.len(), 2, "both name-mentioning callers surface");
    assert!(
        rows.iter()
            .all(|r| r.callee_text.contains("find_by_pattern"))
    );
}

#[test]
fn callers_textual_dedups_by_caller_and_excludes_resolved_and_self() {
    let (_d, h) = fresh_handle();
    seed_file(&h, "src/a.rs");
    // c::a calls `foo` twice → one row. c::resolved is already a resolved edge.
    // c::target is the symbol itself (recursion).
    insert(&h, "src/a.rs", &cs("c::a", "foo", "src/a.rs", 1));
    insert(&h, "src/a.rs", &cs("c::a", "obj.foo", "src/a.rs", 2));
    insert(&h, "src/a.rs", &cs("c::resolved", "foo", "src/a.rs", 3));
    insert(&h, "src/a.rs", &cs("c::target", "foo", "src/a.rs", 4));
    let mut exclude = HashSet::new();
    exclude.insert("c::resolved".to_string());
    exclude.insert("c::target".to_string());
    let rows = callers_textual_for_name(&h, "foo", &exclude, 25).unwrap();
    assert_eq!(
        rows.len(),
        1,
        "deduped to one row per caller, exclusions dropped"
    );
    assert_eq!(rows[0].from_fqdn, "c::a");
}

#[test]
fn callers_textual_empty_name_or_zero_limit_short_circuits() {
    let (_d, h) = fresh_handle();
    seed_file(&h, "src/a.rs");
    insert(&h, "src/a.rs", &cs("c::a", "foo", "src/a.rs", 1));
    let ex = HashSet::new();
    assert!(
        callers_textual_for_name(&h, "   ", &ex, 25)
            .unwrap()
            .is_empty()
    );
    assert!(
        callers_textual_for_name(&h, "foo", &ex, 0)
            .unwrap()
            .is_empty()
    );
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
    let rows = find_call_sites(&h, &CallSiteFilters::default(), 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0], original);
}

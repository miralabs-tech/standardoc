use super::*;
use crate::storage::migrate::ensure_schema;
use standardoc_ir::{
    BindingSource, IdentResolution, ImportRecord, Language, LocalDeclKind, ModuleLookup, ScopeKind,
    ScopeRange,
};

fn fresh_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    ensure_schema(&conn).unwrap();
    conn
}

fn sample_lookup(fqdn: &str, language: Language) -> ModuleLookup {
    let mut m = ModuleLookup::new(fqdn.into(), language);
    let inner = m.push_scope(ScopeRange {
        start_line: 10,
        end_line: 20,
        parent: Some(ModuleLookup::ROOT_SCOPE),
        kind: ScopeKind::Function,
    });
    m.push_binding(IdentResolution {
        name: "Foo".into(),
        source: BindingSource::LocalDecl {
            decl_kind: LocalDeclKind::Struct,
        },
        resolved_fqdn: None,
        aliases_to: None,
        mutability: None,
        scope_idx: ModuleLookup::ROOT_SCOPE,
        attributes: vec![],
        ir_kind: None,
    });
    m.push_binding(IdentResolution {
        name: "local".into(),
        source: BindingSource::LocalDecl {
            decl_kind: LocalDeclKind::Let,
        },
        resolved_fqdn: None,
        aliases_to: None,
        mutability: None,
        scope_idx: inner,
        attributes: vec![],
        ir_kind: None,
    });
    m.push_import(ImportRecord {
        local_name: "HashMap".into(),
        origin_module: "std::collections".into(),
        origin_symbol: Some("HashMap".into()),
        is_type_only: false,
        is_re_export: false,
    });
    m.push_import(ImportRecord {
        local_name: "Mod".into(),
        origin_module: "other::pkg".into(),
        origin_symbol: Some("Module".into()),
        is_type_only: true,
        is_re_export: false,
    });
    m
}

#[test]
fn put_then_get_roundtrips_exactly() {
    let conn = fresh_db();
    let lookup = sample_lookup("my_crate::module", Language::Rust);

    // Sanity: pure in-memory bincode roundtrip (no SQLite in the loop).
    let bytes = bincode::serde::encode_to_vec(&lookup, bincode::config::standard()).expect("encode");
    let (decoded, _): (ModuleLookup, _) =
        bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).expect("decode");
    assert_eq!(decoded, lookup, "in-memory roundtrip must work");

    put_module_lookup(&conn, PRIMARY_WORKSPACE_ID, &lookup).unwrap();
    let back = get_module_lookup(&conn, PRIMARY_WORKSPACE_ID, "my_crate::module")
        .unwrap()
        .expect("row present");
    assert_eq!(back, lookup);
}

#[test]
fn get_returns_none_when_absent() {
    let conn = fresh_db();
    assert!(
        get_module_lookup(&conn, PRIMARY_WORKSPACE_ID, "nope::nope")
            .unwrap()
            .is_none()
    );
}

#[test]
fn put_syncs_workspace_imports_table() {
    let conn = fresh_db();
    let lookup = sample_lookup("my_crate::module", Language::Rust);
    put_module_lookup(&conn, PRIMARY_WORKSPACE_ID, &lookup).unwrap();

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM workspace_imports WHERE workspace_id = ?1 AND module_fqdn = ?2",
            params![PRIMARY_WORKSPACE_ID, "my_crate::module"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);

    // is_type_only flag is preserved.
    let type_only: i64 = conn
        .query_row(
            "SELECT is_type_only FROM workspace_imports \
                 WHERE workspace_id = ?1 AND module_fqdn = ?2 AND local_name = 'Mod'",
            params![PRIMARY_WORKSPACE_ID, "my_crate::module"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(type_only, 1);
}

#[test]
fn put_dedupes_imports_with_identical_local_name_and_origin() {
    // Extractor side cannot prune cfg arms; two `use foo::bar;` on
    // different cfg gates arrive as separate ImportRecord entries
    // with the same (local_name, origin_module). put_module_lookup
    // collapses them so the PRIMARY KEY doesn't fire.
    let conn = fresh_db();
    let mut lookup = ModuleLookup::new("my_crate::dup".into(), Language::Rust);
    for _ in 0..3 {
        lookup.push_import(ImportRecord {
            local_name: "bar".into(),
            origin_module: "foo".into(),
            origin_symbol: Some("bar".into()),
            is_type_only: false,
            is_re_export: false,
        });
    }
    // Distinct origin must still be inserted alongside the first.
    lookup.push_import(ImportRecord {
        local_name: "bar".into(),
        origin_module: "other".into(),
        origin_symbol: Some("bar".into()),
        is_type_only: false,
        is_re_export: false,
    });
    put_module_lookup(&conn, PRIMARY_WORKSPACE_ID, &lookup).unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM workspace_imports WHERE module_fqdn = 'my_crate::dup'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 2, "duplicates collapse, distinct origin survives");
}

#[test]
fn put_replaces_existing_payload_and_imports() {
    let conn = fresh_db();
    let original = sample_lookup("my_crate::module", Language::Rust);
    put_module_lookup(&conn, PRIMARY_WORKSPACE_ID, &original).unwrap();

    // Replace with a lookup that has only one import.
    let mut shrunk = ModuleLookup::new("my_crate::module".into(), Language::Rust);
    shrunk.push_import(ImportRecord {
        local_name: "OnlyOne".into(),
        origin_module: "alone::pkg".into(),
        origin_symbol: None,
        is_type_only: false,
        is_re_export: false,
    });
    put_module_lookup(&conn, PRIMARY_WORKSPACE_ID, &shrunk).unwrap();

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM workspace_imports WHERE workspace_id = ?1 AND module_fqdn = ?2",
            params![PRIMARY_WORKSPACE_ID, "my_crate::module"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "old import rows must have been pruned");
}

#[test]
fn delete_removes_both_payload_and_import_rows() {
    let conn = fresh_db();
    let lookup = sample_lookup("my_crate::module", Language::Rust);
    put_module_lookup(&conn, PRIMARY_WORKSPACE_ID, &lookup).unwrap();
    delete_module_lookup(&conn, PRIMARY_WORKSPACE_ID, "my_crate::module").unwrap();

    assert!(
        get_module_lookup(&conn, PRIMARY_WORKSPACE_ID, "my_crate::module")
            .unwrap()
            .is_none()
    );
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM workspace_imports WHERE workspace_id = ?1 AND module_fqdn = ?2",
            params![PRIMARY_WORKSPACE_ID, "my_crate::module"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn workspace_imports_by_origin_returns_matches_across_workspaces() {
    let conn = fresh_db();
    // Primary workspace imports from std::collections.
    let l1 = sample_lookup("ws_a::module", Language::Rust);
    put_module_lookup(&conn, PRIMARY_WORKSPACE_ID, &l1).unwrap();
    // Simulate a linked workspace also importing from std::collections.
    let l2 = sample_lookup("ws_b::module", Language::Rust);
    put_module_lookup(&conn, "linked-uuid-1234", &l2).unwrap();

    let rows = workspace_imports_by_origin(&conn, "std::collections").unwrap();
    assert_eq!(rows.len(), 2);
    let workspaces: std::collections::HashSet<_> =
        rows.iter().map(|r| r.workspace_id.clone()).collect();
    assert!(workspaces.contains(PRIMARY_WORKSPACE_ID));
    assert!(workspaces.contains("linked-uuid-1234"));
}

#[test]
fn separate_workspace_ids_do_not_collide() {
    let conn = fresh_db();
    let primary = sample_lookup("module", Language::Rust);
    let mut linked = sample_lookup("module", Language::TypeScript);
    linked.push_binding(IdentResolution {
        name: "OnlyInLinked".into(),
        source: BindingSource::LocalDecl {
            decl_kind: LocalDeclKind::Function,
        },
        resolved_fqdn: None,
        aliases_to: None,
        mutability: None,
        scope_idx: ModuleLookup::ROOT_SCOPE,
        attributes: vec![],
        ir_kind: None,
    });

    put_module_lookup(&conn, PRIMARY_WORKSPACE_ID, &primary).unwrap();
    put_module_lookup(&conn, "linked-uuid", &linked).unwrap();

    let back_primary = get_module_lookup(&conn, PRIMARY_WORKSPACE_ID, "module")
        .unwrap()
        .expect("primary present");
    assert_eq!(back_primary.language, Language::Rust);
    assert!(!back_primary.bindings.contains_key("OnlyInLinked"));

    let back_linked = get_module_lookup(&conn, "linked-uuid", "module")
        .unwrap()
        .expect("linked present");
    assert_eq!(back_linked.language, Language::TypeScript);
    assert!(back_linked.bindings.contains_key("OnlyInLinked"));
}

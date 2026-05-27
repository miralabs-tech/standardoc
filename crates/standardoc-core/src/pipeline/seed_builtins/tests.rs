use super::*;
use crate::storage::test_utils::fresh_conn;
use standardoc_ir::{BuiltinTag, BuiltinTier, Kind};

fn sample_edge_entry(name: &str, lang: Language) -> BuiltinEntry {
    BuiltinEntry::new(
        name,
        lang,
        Kind::Callable,
        BuiltinTag::Console,
        BuiltinTier::Edge,
    )
}

#[test]
fn seed_into_empty_input_inserts_nothing() {
    let conn = fresh_conn();
    let n = seed_into(&conn, &[]).expect("seed with empty input");
    assert_eq!(n, 0);
}

#[test]
fn seed_into_creates_synthetic_file_per_language() {
    let conn = fresh_conn();
    let entries = vec![
        sample_edge_entry("print", Language::Lua),
        sample_edge_entry("console", Language::TypeScript),
        sample_edge_entry("Math", Language::TypeScript),
    ];
    let n = seed_into(&conn, &entries).expect("seed batch");
    assert_eq!(n, 3);
    let ts_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path = ?1",
            ["<builtin>/ts"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ts_count, 1, "one synthetic file per language");
    let lua_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path = ?1",
            ["<builtin>/lua"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(lua_count, 1);
}

#[test]
fn seed_into_is_idempotent_across_calls() {
    let conn = fresh_conn();
    let entries = vec![
        sample_edge_entry("print", Language::Lua),
        sample_edge_entry("Math", Language::TypeScript),
    ];
    let n1 = seed_into(&conn, &entries).unwrap();
    let n2 = seed_into(&conn, &entries).unwrap();
    assert_eq!(n1, 2);
    assert_eq!(n2, 2, "second call still reports 2 (UPSERT)");
    // Row count must stay at 2 — UPSERT must not duplicate.
    let total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM symbols WHERE is_external = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(total, 2, "no duplicate rows after re-seeding");
}

#[test]
fn seed_into_persists_synthetic_fqdn_module_and_is_external_flag() {
    let conn = fresh_conn();
    let entries = vec![sample_edge_entry("print", Language::Lua)];
    seed_into(&conn, &entries).unwrap();
    let (fqdn, module, file_path, is_external): (String, Option<String>, String, i64) = conn
        .query_row(
            "SELECT fqdn, module, file_path, is_external \
                 FROM symbols WHERE name = ?1",
            ["print"],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(fqdn, "<builtin>::lua::print");
    assert_eq!(module.as_deref(), Some("<builtin>::lua"));
    assert_eq!(file_path, "<builtin>/lua");
    assert_eq!(is_external, 1);
}

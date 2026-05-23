use std::collections::HashMap;

use rusqlite::Connection;
use standardoc_ir::{
    Blake3Hash, BuiltinEntry, Language, LanguageKind, RawSymbol, SourceOrigin, SymbolLocation,
    Visibility,
};

use crate::storage::error::StorageError;
use crate::storage::files::{FileInput, upsert_file};
use crate::storage::handle::IndexHandle;
use crate::storage::symbols::{SymbolInsertContext, insert_symbol};

/// Virtual-file prefix for every synthetic builtin symbol row. Combined
/// with the per-language slug it yields paths like `<builtin>/ts` /
/// `<builtin>/lua` — never collides with real workspace paths thanks to
/// the `<` / `>` characters (the same rationale the synthetic FQDN
/// scheme in `standardoc-ir` relies on).
const BUILTIN_FILE_PREFIX: &str = "<builtin>";

/// Stage 3e-1: best-effort cold-start seeding. Pull the per-handle DB
/// pool, get a connection, and persist Edge-tier builtin entries.
/// Errors are swallowed (matching `discover_projects_quietly` /
/// `reconcile_projects_quietly`): failure mode is just "tier-edge
/// builtin edges stay unresolved", which is the pre-3e-1 behaviour
/// anyway, so cold start must keep going regardless.
pub(crate) fn seed_quietly(handle: &IndexHandle, entries: &[BuiltinEntry]) {
    if entries.is_empty() {
        return;
    }
    let pool = match handle.pool() {
        Ok(p) => p,
        Err(_) => return,
    };
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return,
    };
    let _ = seed_into(&conn, entries);
}

/// Persist `entries` into the SQLite store. Groups by language so we
/// upsert ONE virtual file per language under `<builtin>/<lang>` then
/// insert each builtin as a synthetic `RawSymbol` pointing at it.
/// `insert_symbol` is UPSERT-by-fqdn so the operation is fully
/// idempotent — calling it on every cold start keeps the seed in sync
/// with whatever the live registry currently exposes.
pub(crate) fn seed_into(
    conn: &Connection,
    entries: &[BuiltinEntry],
) -> Result<usize, StorageError> {
    let mut by_lang: HashMap<Language, Vec<&BuiltinEntry>> = HashMap::new();
    for entry in entries {
        by_lang.entry(entry.language).or_default().push(entry);
    }
    let mut inserted = 0_usize;
    for (lang, lang_entries) in by_lang {
        let file_path = synthetic_file_path(lang);
        upsert_file(
            conn,
            &FileInput {
                path: file_path.clone(),
                content_hash: Blake3Hash::default(),
                language: lang,
                byte_size: 0,
                last_scanned: 0,
                last_scan_error: None,
                is_external: true,
            },
        )?;
        let module = synthetic_module(lang);
        for entry in lang_entries {
            let sym = RawSymbol {
                decl_kind: None,
                implements_trait: None,
                receiver_type: None,
                name: entry.name.clone(),
                fqdn: entry.synthetic_fqdn.clone(),
                kind: entry.kind,
                language_kind: LanguageKind::from("builtin"),
                module: Some(module.clone()),
                visibility: Visibility::Public,
                location: SymbolLocation {
                    file: file_path.clone(),
                    start_line: 0,
                    end_line: 0,
                    start_col: 0,
                    end_col: 0,
                },
                signature: None,
                body_hash: None,
                attributes: vec![],
                flags: vec![],
            };
            insert_symbol(
                conn,
                &sym,
                SymbolInsertContext {
                    file_path: &file_path,
                    language: lang,
                    is_external: true,
                    source_origin: SourceOrigin::ManualExternal,
                    revision: 0,
                    workspace_id: crate::storage::module_lookup::PRIMARY_WORKSPACE_ID,
                },
            )?;
            inserted += 1;
        }
    }
    Ok(inserted)
}

fn synthetic_file_path(lang: Language) -> String {
    format!("{}/{}", BUILTIN_FILE_PREFIX, lang_slug(lang))
}

fn synthetic_module(lang: Language) -> String {
    format!("{}::{}", BUILTIN_FILE_PREFIX, lang_slug(lang))
}

fn lang_slug(lang: Language) -> &'static str {
    match lang {
        Language::Rust => "rust",
        Language::TypeScript => "ts",
        Language::JavaScript => "js",
        Language::Lua => "lua",
        Language::Vue => "vue",
        Language::Svelte => "svelte",
        Language::C => "c",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_utils::fresh_conn;
    use standardoc_ir::{BuiltinTag, BuiltinTier, Kind};

    fn sample_edge_entry(name: &str, lang: Language) -> BuiltinEntry {
        BuiltinEntry::new(
            name,
            lang,
            Kind::Function,
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
}

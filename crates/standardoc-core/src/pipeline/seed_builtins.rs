use std::collections::HashMap;

use rusqlite::Connection;
use standardoc_ir::{
    Blake3Hash, BuiltinEntry, BuiltinMethodEntry, Kind, Language, LanguageKind, RawSymbol,
    SourceOrigin, SymbolLocation, TypeRef, Visibility,
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

/// Bug E-3 Phase 2 sibling of `seed_quietly` for method entries. Best-
/// effort: failure leaves stdlib method edges unresolved, same as the
/// pre-Phase-2 behaviour.
pub(crate) fn seed_methods_quietly(handle: &IndexHandle, methods: &[BuiltinMethodEntry]) {
    if methods.is_empty() {
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
    let _ = seed_methods_into(&conn, methods);
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
                entry_point: None,
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

/// Bug E-3 Phase 2: persist `methods` as synthetic `RawSymbol` rows so
/// the resolver's `<receiver_type>::<method>` lookup lands on a real
/// `symbols.id` for stdlib method calls. Each method's `module` is
/// `<builtin>::<lang>::<parent_type>` so symbols stay grouped per type
/// in any `module`-filtered query. Idempotent like `seed_into`.
pub(crate) fn seed_methods_into(
    conn: &Connection,
    methods: &[BuiltinMethodEntry],
) -> Result<usize, StorageError> {
    let mut by_lang: HashMap<Language, Vec<&BuiltinMethodEntry>> = HashMap::new();
    for method in methods {
        by_lang.entry(method.language).or_default().push(method);
    }
    let mut inserted = 0_usize;
    for (lang, lang_methods) in by_lang {
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
        let lang_module = synthetic_module(lang);
        for method in lang_methods {
            let module = format!("{lang_module}::{}", method.parent_type);
            let sym = RawSymbol {
                decl_kind: None,
                implements_trait: None,
                receiver_type: Some(TypeRef::new(method.parent_type.clone())),
                entry_point: None,
                name: method.method.clone(),
                fqdn: method.synthetic_fqdn.clone(),
                kind: Kind::Callable,
                language_kind: LanguageKind::from("builtin_method"),
                module: Some(module),
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
                // Trait dispatch widening: stamp `trait_method` on the
                // synthetic so `try_resolve_via_builtin_trait_method`
                // can SELECT trait-method targets without distinguishing
                // them from type methods at SQL time.
                flags: if method.is_trait {
                    vec!["trait_method".to_string()]
                } else {
                    vec![]
                },
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

const fn lang_slug(lang: Language) -> &'static str {
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
mod tests;

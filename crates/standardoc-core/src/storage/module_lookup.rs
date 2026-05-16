//! Persistence layer for Stage 3a `ModuleLookup` payloads.
//!
//! Each ModuleLookup is bincode-encoded into the `module_lookups`
//! table's `payload` BLOB column, keyed by `(workspace_id, module_fqdn)`.
//! The flat `workspace_imports` table is kept in sync transactionally
//! so cross-workspace import resolution can SQL-join on `origin_module`
//! without deserialising every blob.
//!
//! The `'primary'` sentinel workspace_id is reserved for the current
//! workspace; linked workspaces use UUID v4 ids registered in
//! `workspace_catalog` (Stage 3b-3).

use rusqlite::{Connection, OptionalExtension, params};
use standardoc_ir::{Language, ModuleLookup};

use crate::storage::error::StorageError;

/// Sentinel workspace_id for the current (primary) workspace.
pub(crate) const PRIMARY_WORKSPACE_ID: &str = "primary";

/// Canonical lowercase language slug stored in `module_lookups.language`.
/// Matches the IR's `Language` `serde(rename_all = "lowercase")` shape so
/// the DB string is always deserialisable back via serde.
fn language_storage_slug(lang: Language) -> &'static str {
    match lang {
        Language::Rust => "rust",
        Language::TypeScript => "typescript",
        Language::JavaScript => "javascript",
        Language::Lua => "lua",
        Language::Vue => "vue",
        Language::Svelte => "svelte",
    }
}

fn bincode_to_storage<E: std::fmt::Display>(detail: &str, e: E) -> StorageError {
    StorageError::InvalidStoredData {
        detail: format!("{detail}: {e}"),
    }
}

/// Insert or replace the persisted ModuleLookup payload for
/// `(workspace_id, module_fqdn)`, transactionally rebuilding the
/// corresponding `workspace_imports` rows in the same call.
pub(crate) fn put_module_lookup(
    conn: &Connection,
    workspace_id: &str,
    lookup: &ModuleLookup,
) -> Result<(), StorageError> {
    let payload =
        bincode::serialize(lookup).map_err(|e| bincode_to_storage("ModuleLookup encode", e))?;
    let language = language_storage_slug(lookup.language);
    let built_at = i64::try_from(lookup.built_at_epoch_ms).unwrap_or(i64::MAX);

    conn.execute(
        "INSERT OR REPLACE INTO module_lookups
         (module_fqdn, workspace_id, language, built_at, payload)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![&lookup.module_fqdn, workspace_id, language, built_at, payload],
    )?;

    conn.execute(
        "DELETE FROM workspace_imports WHERE workspace_id = ?1 AND module_fqdn = ?2",
        params![workspace_id, &lookup.module_fqdn],
    )?;

    for import in &lookup.imports {
        conn.execute(
            "INSERT INTO workspace_imports
             (workspace_id, module_fqdn, local_name, origin_module, origin_symbol, \
              is_type_only, is_re_export)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                workspace_id,
                &lookup.module_fqdn,
                &import.local_name,
                &import.origin_module,
                &import.origin_symbol,
                i64::from(import.is_type_only),
                i64::from(import.is_re_export),
            ],
        )?;
    }
    Ok(())
}

/// Fetch and decode the persisted ModuleLookup for
/// `(workspace_id, module_fqdn)`. Returns `Ok(None)` when no row exists.
pub(crate) fn get_module_lookup(
    conn: &Connection,
    workspace_id: &str,
    module_fqdn: &str,
) -> Result<Option<ModuleLookup>, StorageError> {
    let payload: Option<Vec<u8>> = conn
        .query_row(
            "SELECT payload FROM module_lookups WHERE workspace_id = ?1 AND module_fqdn = ?2",
            params![workspace_id, module_fqdn],
            |row| row.get(0),
        )
        .optional()?;

    payload
        .map(|bytes| {
            bincode::deserialize::<ModuleLookup>(&bytes)
                .map_err(|e| bincode_to_storage("ModuleLookup decode", e))
        })
        .transpose()
}

/// Remove the ModuleLookup row + its workspace_imports rows for
/// `(workspace_id, module_fqdn)`.
pub(crate) fn delete_module_lookup(
    conn: &Connection,
    workspace_id: &str,
    module_fqdn: &str,
) -> Result<(), StorageError> {
    conn.execute(
        "DELETE FROM workspace_imports WHERE workspace_id = ?1 AND module_fqdn = ?2",
        params![workspace_id, module_fqdn],
    )?;
    conn.execute(
        "DELETE FROM module_lookups WHERE workspace_id = ?1 AND module_fqdn = ?2",
        params![workspace_id, module_fqdn],
    )?;
    Ok(())
}

/// Iterate `workspace_imports` rows whose `origin_module` matches.
/// Returns `(workspace_id, module_fqdn, local_name, origin_symbol,
/// is_type_only, is_re_export)` tuples. Used by Stage 3b-4
/// cross-workspace resolver to find which modules import a given
/// origin and therefore depend on a peer workspace's exports.
pub(crate) fn workspace_imports_by_origin(
    conn: &Connection,
    origin_module: &str,
) -> Result<Vec<WorkspaceImportRow>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT workspace_id, module_fqdn, local_name, origin_symbol, is_type_only, is_re_export \
         FROM workspace_imports WHERE origin_module = ?1",
    )?;
    let rows = stmt
        .query_map(params![origin_module], |row| {
            Ok(WorkspaceImportRow {
                workspace_id: row.get(0)?,
                module_fqdn: row.get(1)?,
                local_name: row.get(2)?,
                origin_symbol: row.get(3)?,
                is_type_only: row.get::<_, i64>(4)? != 0,
                is_re_export: row.get::<_, i64>(5)? != 0,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceImportRow {
    pub workspace_id: String,
    pub module_fqdn: String,
    pub local_name: String,
    pub origin_symbol: Option<String>,
    pub is_type_only: bool,
    pub is_re_export: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrate::ensure_schema;
    use standardoc_ir::{
        BindingSource, IdentResolution, ImportRecord, Language, LocalDeclKind, ModuleLookup,
        ScopeKind, ScopeRange,
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
        let bytes = bincode::serialize(&lookup).expect("encode");
        let decoded: ModuleLookup = bincode::deserialize(&bytes).expect("decode");
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
}

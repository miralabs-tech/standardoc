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
const fn language_storage_slug(lang: Language) -> &'static str {
    match lang {
        Language::Rust => "rust",
        Language::TypeScript => "typescript",
        Language::JavaScript => "javascript",
        Language::Lua => "lua",
        Language::Vue => "vue",
        Language::Svelte => "svelte",
        Language::C => "c",
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
    let payload = bincode::serde::encode_to_vec(lookup, bincode::config::standard())
        .map_err(|e| bincode_to_storage("ModuleLookup encode", e))?;
    let language = language_storage_slug(lookup.language);
    let built_at = i64::try_from(lookup.built_at_epoch_ms).unwrap_or(i64::MAX);

    conn.execute(
        "INSERT OR REPLACE INTO module_lookups
         (module_fqdn, workspace_id, language, built_at, payload)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            &lookup.module_fqdn,
            workspace_id,
            language,
            built_at,
            payload
        ],
    )?;

    conn.execute(
        "DELETE FROM workspace_imports WHERE workspace_id = ?1 AND module_fqdn = ?2",
        params![workspace_id, &lookup.module_fqdn],
    )?;

    // Dedup by the canonical workspace_imports PK before insert. The
    // extractor doesn't prune cfg-gated branches, so the same module
    // can carry two ImportRecord entries with identical
    // (local_name, origin_module) — e.g.
    //   #[cfg(unix)]    use foo::bar;
    //   #[cfg(windows)] use foo::bar;
    // Likewise for accidental duplicate `use` lines during refactors.
    // First-write-wins on tie: the secondary attrs (is_type_only /
    // is_re_export) of cfg-duals are typically identical anyway.
    let mut seen: std::collections::HashSet<(&str, &str)> =
        std::collections::HashSet::with_capacity(lookup.imports.len());
    for import in &lookup.imports {
        if !seen.insert((import.local_name.as_str(), import.origin_module.as_str())) {
            continue;
        }
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
            bincode::serde::decode_from_slice::<ModuleLookup, _>(
                &bytes,
                bincode::config::standard(),
            )
            .map(|(value, _)| value)
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
mod tests;

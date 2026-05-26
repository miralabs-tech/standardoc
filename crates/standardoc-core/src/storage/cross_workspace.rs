//! Cross-workspace import resolver (Stage 3b-4).
//!
//! Given an `ImportRecord` produced by the primary workspace (e.g.
//! `import { Foo } from "ws_b::lib"`), this module locates the
//! `ModuleLookup` of `ws_b::lib` registered under a linked workspace
//! id, deserialises it, and checks whether `Foo` is a top-level
//! (ROOT-scope) binding. If so, returns the synthesised
//! `<module>::<symbol>` fqdn together with the providing workspace id
//! and the binding source for downstream attribution.
//!
//! Strategy: SQL filter on `module_lookups` by `module_fqdn` excluding
//! the `'primary'` sentinel, then bincode-decode each match. The MVP
//! returns the first match. Stage 3b-5 (MCP) layers ranking / multi-
//! match enumeration on top via a dedicated tool. A `workspace_exports`
//! table indexing top-level bindings without blob scans is a future
//! optimisation when the persisted catalog grows.

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use standardoc_ir::{BindingSource, ModuleLookup};

use crate::storage::error::StorageError;
use crate::storage::module_lookup::PRIMARY_WORKSPACE_ID;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CrossWorkspaceResolution {
    /// The linked workspace that provides the symbol.
    pub workspace_id: String,
    /// The fqdn the import resolves to in the providing workspace,
    /// formatted `<origin_module>::<origin_symbol>`.
    pub resolved_fqdn: String,
    /// The binding source of the resolved symbol (Import / LocalDecl /
    /// Builtin / Bridge) — surfaced so callers can attribute the edge
    /// or filter (e.g., type-only re-exports).
    pub binding_source: BindingSource,
}

fn decode_module_lookup(bytes: &[u8]) -> Result<ModuleLookup, StorageError> {
    bincode::deserialize::<ModuleLookup>(bytes).map_err(|e| StorageError::InvalidStoredData {
        detail: format!("ModuleLookup decode (cross-workspace): {e}"),
    })
}

/// Resolve an import `<origin_module>::<origin_symbol>` against the
/// linked workspaces' persisted `ModuleLookup`s.
///
/// Returns `Ok(None)` when no linked workspace contains a matching
/// top-level (ROOT-scope) binding. Returns `Ok(Some(_))` for the first
/// hit — iteration order matches insertion order in `module_lookups`
/// (SQLite-defined; effectively rowid ASC).
pub(crate) fn resolve_cross_workspace_import(
    conn: &Connection,
    origin_module: &str,
    origin_symbol: &str,
) -> Result<Option<CrossWorkspaceResolution>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT workspace_id, payload FROM module_lookups \
		 WHERE module_fqdn = ?1 AND workspace_id != ?2",
    )?;
    let rows = stmt.query_map(params![origin_module, PRIMARY_WORKSPACE_ID], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
    })?;
    for row in rows {
        let (workspace_id, payload) = row?;
        let lookup = decode_module_lookup(&payload)?;
        let Some(entries) = lookup.bindings.get(origin_symbol) else {
            continue;
        };
        let Some(root_binding) = entries
            .iter()
            .find(|b| b.scope_idx == ModuleLookup::ROOT_SCOPE)
        else {
            continue;
        };
        // Prefer the binding's `resolved_fqdn` (set by the extractor
        // when it followed the re-export chain to the canonical
        // definition) over `<origin_module>::<origin_symbol>`. Without
        // this the LuaProvider → `use standardoc_core::LanguageProvider`
        // edge would resolve to the re-export FQDN
        // `standardoc-core::pipeline::LanguageProvider` and miss the
        // canonical trait at `standardoc-core::pipeline::provider::LanguageProvider`
        // when consumers (viz / MCP) query its dependents.
        let resolved_fqdn = root_binding
            .resolved_fqdn
            .clone()
            .unwrap_or_else(|| format!("{origin_module}::{origin_symbol}"));
        return Ok(Some(CrossWorkspaceResolution {
            workspace_id,
            resolved_fqdn,
            binding_source: root_binding.source.clone(),
        }));
    }
    Ok(None)
}

/// Stage 3 R3 — peer-presence probe for the extract-time resolver.
/// Returns the first linked workspace_id whose `module_lookups` row
/// matches `origin_module`, or `None` when no peer owns the module.
///
/// Cheaper than [`resolve_cross_workspace_import`] (no blob decode):
/// the resolver uses it to distinguish "no peer owns this module"
/// (caller falls through to local-unresolved) from "peer owns the
/// module but doesn't export the symbol" (caller emits a typed
/// `UnresolvedBridge` with the `custom:cross-workspace` slug).
pub(crate) fn peer_workspace_for_module(
    conn: &Connection,
    origin_module: &str,
) -> Result<Option<String>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT workspace_id FROM module_lookups \
         WHERE module_fqdn = ?1 AND workspace_id != ?2 LIMIT 1",
    )?;
    let row = stmt
        .query_row(params![origin_module, PRIMARY_WORKSPACE_ID], |row| {
            row.get::<_, String>(0)
        })
        .optional()?;
    Ok(row)
}

/// Enumerate every linked workspace that provides a matching top-level
/// binding for `(origin_module, origin_symbol)`. Useful when a symbol
/// is re-exported by multiple peers and the consumer (Stage 3b-5 MCP)
/// wants to surface all of them rather than just the first.
pub(crate) fn list_cross_workspace_providers(
    conn: &Connection,
    origin_module: &str,
    origin_symbol: &str,
) -> Result<Vec<CrossWorkspaceResolution>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT workspace_id, payload FROM module_lookups \
		 WHERE module_fqdn = ?1 AND workspace_id != ?2",
    )?;
    let rows = stmt.query_map(params![origin_module, PRIMARY_WORKSPACE_ID], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (workspace_id, payload) = row?;
        let lookup = decode_module_lookup(&payload)?;
        let Some(entries) = lookup.bindings.get(origin_symbol) else {
            continue;
        };
        let Some(root_binding) = entries
            .iter()
            .find(|b| b.scope_idx == ModuleLookup::ROOT_SCOPE)
        else {
            continue;
        };
        out.push(CrossWorkspaceResolution {
            workspace_id,
            resolved_fqdn: format!("{origin_module}::{origin_symbol}"),
            binding_source: root_binding.source.clone(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrate::ensure_schema;
    use crate::storage::module_lookup::put_module_lookup;
    use standardoc_ir::{
        BindingSource, IdentResolution, Language, LocalDeclKind, ModuleLookup, ScopeKind,
        ScopeRange,
    };

    fn fresh_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        ensure_schema(&conn).unwrap();
        conn
    }

    fn lookup_with_top_level_symbol(
        fqdn: &str,
        symbol: &str,
        decl_kind: LocalDeclKind,
    ) -> ModuleLookup {
        let mut m = ModuleLookup::new(fqdn.into(), Language::Rust);
        m.push_binding(IdentResolution {
            name: symbol.into(),
            source: BindingSource::LocalDecl { decl_kind },
            resolved_fqdn: None,
            aliases_to: None,
            mutability: None,
            scope_idx: ModuleLookup::ROOT_SCOPE,
            attributes: vec![],
            ir_kind: None,
        });
        m
    }

    #[test]
    fn resolves_when_linked_workspace_provides_symbol() {
        let conn = fresh_db();
        let linked = lookup_with_top_level_symbol("ws_b::lib", "Foo", LocalDeclKind::Struct);
        put_module_lookup(&conn, "ws_b-uuid", &linked).unwrap();

        let hit = resolve_cross_workspace_import(&conn, "ws_b::lib", "Foo")
            .unwrap()
            .expect("resolved");
        assert_eq!(hit.workspace_id, "ws_b-uuid");
        assert_eq!(hit.resolved_fqdn, "ws_b::lib::Foo");
        assert!(matches!(
            hit.binding_source,
            BindingSource::LocalDecl {
                decl_kind: LocalDeclKind::Struct
            }
        ));
    }

    #[test]
    fn returns_none_when_no_linked_workspace_has_module() {
        let conn = fresh_db();
        let primary = lookup_with_top_level_symbol("my_crate::lib", "Foo", LocalDeclKind::Function);
        put_module_lookup(&conn, PRIMARY_WORKSPACE_ID, &primary).unwrap();

        assert!(
            resolve_cross_workspace_import(&conn, "ws_b::lib", "Foo")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn primary_workspace_lookups_are_excluded_from_resolution() {
        let conn = fresh_db();
        // A primary-workspace module with matching symbol — must NOT
        // be returned as a cross-workspace resolution.
        let primary = lookup_with_top_level_symbol("ws_b::lib", "Foo", LocalDeclKind::Struct);
        put_module_lookup(&conn, PRIMARY_WORKSPACE_ID, &primary).unwrap();

        assert!(
            resolve_cross_workspace_import(&conn, "ws_b::lib", "Foo")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn returns_none_when_module_exists_but_symbol_missing() {
        let conn = fresh_db();
        let linked = lookup_with_top_level_symbol("ws_b::lib", "Bar", LocalDeclKind::Function);
        put_module_lookup(&conn, "ws_b-uuid", &linked).unwrap();

        assert!(
            resolve_cross_workspace_import(&conn, "ws_b::lib", "Foo")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn nested_scope_bindings_do_not_count_as_exports() {
        let conn = fresh_db();
        let mut linked = ModuleLookup::new("ws_b::lib".into(), Language::Rust);
        let inner = linked.push_scope(ScopeRange {
            start_line: 10,
            end_line: 20,
            parent: Some(ModuleLookup::ROOT_SCOPE),
            kind: ScopeKind::Function,
        });
        // `Foo` bound INSIDE a function — not a module export.
        linked.push_binding(IdentResolution {
            name: "Foo".into(),
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
        put_module_lookup(&conn, "ws_b-uuid", &linked).unwrap();

        assert!(
            resolve_cross_workspace_import(&conn, "ws_b::lib", "Foo")
                .unwrap()
                .is_none(),
            "nested bindings must not be visible cross-workspace",
        );
    }

    #[test]
    fn list_returns_all_providers() {
        let conn = fresh_db();
        let l1 = lookup_with_top_level_symbol("shared::api", "Foo", LocalDeclKind::Struct);
        let l2 = lookup_with_top_level_symbol("shared::api", "Foo", LocalDeclKind::TypeAlias);
        put_module_lookup(&conn, "ws-a", &l1).unwrap();
        put_module_lookup(&conn, "ws-b", &l2).unwrap();

        let all = list_cross_workspace_providers(&conn, "shared::api", "Foo").unwrap();
        assert_eq!(all.len(), 2);
        let ids: std::collections::HashSet<_> =
            all.iter().map(|r| r.workspace_id.clone()).collect();
        assert!(ids.contains("ws-a"));
        assert!(ids.contains("ws-b"));
    }

    #[test]
    fn list_returns_empty_when_no_match() {
        let conn = fresh_db();
        let result = list_cross_workspace_providers(&conn, "no::such", "Symbol").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn resolves_import_binding_as_re_export_chain() {
        // Linked workspace re-exports `Foo` from another module via
        // BindingSource::Import { is_re_export: true } — the resolver
        // surfaces the re-export so the caller can chase the chain.
        let conn = fresh_db();
        let mut linked = ModuleLookup::new("ws_b::index".into(), Language::TypeScript);
        linked.push_binding(IdentResolution {
            name: "Foo".into(),
            source: BindingSource::Import {
                module_path: "./internal".into(),
                original_name: Some("FooImpl".into()),
                is_type_only: false,
                is_re_export: true,
            },
            resolved_fqdn: None,
            aliases_to: None,
            mutability: None,
            scope_idx: ModuleLookup::ROOT_SCOPE,
            attributes: vec!["re-export".into()],
            ir_kind: None,
        });
        put_module_lookup(&conn, "ws_b-uuid", &linked).unwrap();

        let hit = resolve_cross_workspace_import(&conn, "ws_b::index", "Foo")
            .unwrap()
            .expect("re-export surfaces");
        assert_eq!(hit.workspace_id, "ws_b-uuid");
        match &hit.binding_source {
            BindingSource::Import {
                is_re_export,
                original_name,
                ..
            } => {
                assert!(is_re_export);
                assert_eq!(original_name.as_deref(), Some("FooImpl"));
            }
            other => panic!("expected Import re-export, got {other:?}"),
        }
    }
}

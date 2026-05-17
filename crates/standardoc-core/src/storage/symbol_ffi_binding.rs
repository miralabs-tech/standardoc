use rusqlite::Connection;
use standardoc_ir::{FfiAbi, FfiDirection, RawFfiBinding};

use crate::storage::error::StorageError;

/// Insert or replace a single FFI binding row. The composite primary key
/// `(symbol_id, abi, direction, abi_name)` is exact-match — re-inserting
/// the same tuple just refreshes the optional `convention` hint.
pub(crate) fn upsert_binding(
    conn: &Connection,
    symbol_id: i64,
    binding: &RawFfiBinding,
) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO symbol_ffi_binding (symbol_id, abi, direction, abi_name, convention) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(symbol_id, abi, direction, abi_name) DO UPDATE SET \
            convention = excluded.convention",
        rusqlite::params![
            symbol_id,
            binding.abi.as_slug(),
            binding.direction.as_slug(),
            binding.abi_name,
            binding.convention,
        ],
    )?;
    Ok(())
}

/// Drop every binding row owned by a symbol. Mostly redundant with
/// `ON DELETE CASCADE`, but exposed so the watcher can purge stale
/// bindings during an in-place re-extraction (where the symbol row
/// is upserted, not deleted, but the bindings need to refresh).
pub(crate) fn delete_bindings_for_symbol(
    conn: &Connection,
    symbol_id: i64,
) -> Result<(), StorageError> {
    conn.execute(
        "DELETE FROM symbol_ffi_binding WHERE symbol_id = ?1",
        [symbol_id],
    )?;
    Ok(())
}

/// List all bindings persisted for a symbol — debugging / introspection.
pub(crate) fn list_for_symbol(
    conn: &Connection,
    symbol_id: i64,
) -> Result<Vec<RawFfiBinding>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT abi, direction, abi_name, convention \
         FROM symbol_ffi_binding WHERE symbol_id = ?1",
    )?;
    let mapped = stmt.query_map([symbol_id], |r| {
        let abi: String = r.get(0)?;
        let direction: String = r.get(1)?;
        let abi_name: String = r.get(2)?;
        let convention: Option<String> = r.get(3)?;
        Ok((abi, direction, abi_name, convention))
    })?;
    let mut out = Vec::new();
    for row in mapped {
        let (abi, direction, abi_name, convention) = row?;
        let Some(direction) = FfiDirection::from_slug(&direction) else {
            return Err(StorageError::InvalidStoredData {
                detail: format!("unknown ffi direction `{direction}` for symbol {symbol_id}"),
            });
        };
        out.push(RawFfiBinding {
            symbol_fqdn: String::new(), // not stored on row; caller joins via id
            abi: FfiAbi::from_slug(&abi),
            direction,
            abi_name,
            convention,
        });
    }
    Ok(out)
}

/// A side of a binding observed in the DB, keyed for matching across
/// languages. The `fqdn` is what consumers eventually want for emitting
/// edges; it's joined from `symbols.fqdn` at query time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BindingSide {
    pub symbol_id: i64,
    pub fqdn: String,
    pub abi_name: String,
    pub abi: String,
    pub workspace_id: String,
}

/// Fetch every binding row of a given direction across all workspaces.
/// Used by the resolve pass — it streams both exports + imports once
/// and groups them in memory to find unique cross-language matches.
pub(crate) fn fetch_all_sides(
    conn: &Connection,
    direction: FfiDirection,
) -> Result<Vec<BindingSide>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT b.symbol_id, s.fqdn, b.abi, b.abi_name, s.workspace_id \
         FROM symbol_ffi_binding b \
         JOIN symbols s ON s.id = b.symbol_id \
         WHERE b.direction = ?1",
    )?;
    let mapped = stmt.query_map([direction.as_slug()], |r| {
        Ok(BindingSide {
            symbol_id: r.get(0)?,
            fqdn: r.get(1)?,
            abi: r.get(2)?,
            abi_name: r.get(3)?,
            workspace_id: r.get(4)?,
        })
    })?;
    let mut out = Vec::new();
    for row in mapped {
        out.push(row?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrate::ensure_schema;

    fn fresh_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        ensure_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO files (path, content_hash, language, last_scanned, byte_size, is_external) \
             VALUES ('src/x.c', 'h', 'c', 0, 0, 0)",
            [],
        )
        .unwrap();
        conn
    }

    fn seed_sym(conn: &Connection, fqdn: &str, name: &str) -> i64 {
        conn.query_row(
            "INSERT INTO symbols (\
                fqdn, name, kind, language_kind, language, module, visibility, \
                file_path, start_line, end_line, start_col, end_col, \
                signature_json, body_hash, is_external, source_origin, \
                last_modified_revision, flags, workspace_id\
             ) VALUES (?1, ?2, 'function', 'fn', 'c', NULL, 'public', \
                'src/x.c', 1, 5, 0, 1, NULL, NULL, 0, 'workspace', 0, '[]', 'primary') \
             RETURNING id",
            rusqlite::params![fqdn, name],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    }

    fn export_binding(abi: FfiAbi, name: &str) -> RawFfiBinding {
        RawFfiBinding {
            symbol_fqdn: String::new(),
            abi,
            direction: FfiDirection::Export,
            abi_name: name.into(),
            convention: None,
        }
    }

    #[test]
    fn upsert_inserts_then_updates_convention() {
        let conn = fresh_conn();
        let sid = seed_sym(&conn, "x::foo", "foo");
        let mut b = export_binding(FfiAbi::C, "foo");
        upsert_binding(&conn, sid, &b).unwrap();
        b.convention = Some("manual".into());
        upsert_binding(&conn, sid, &b).unwrap();
        let list = list_for_symbol(&conn, sid).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].convention.as_deref(), Some("manual"));
    }

    #[test]
    fn multiple_bindings_per_symbol_allowed() {
        let conn = fresh_conn();
        let sid = seed_sym(&conn, "x::dual", "dual");
        upsert_binding(&conn, sid, &export_binding(FfiAbi::C, "dual")).unwrap();
        let import = RawFfiBinding {
            symbol_fqdn: String::new(),
            abi: FfiAbi::C,
            direction: FfiDirection::Import,
            abi_name: "dual".into(),
            convention: None,
        };
        upsert_binding(&conn, sid, &import).unwrap();
        let list = list_for_symbol(&conn, sid).unwrap();
        assert_eq!(list.len(), 2, "export + import on same symbol must coexist");
    }

    #[test]
    fn delete_bindings_for_symbol_clears_rows() {
        let conn = fresh_conn();
        let sid = seed_sym(&conn, "x::foo", "foo");
        upsert_binding(&conn, sid, &export_binding(FfiAbi::C, "foo")).unwrap();
        upsert_binding(&conn, sid, &export_binding(FfiAbi::Lua, "luaopen_foo")).unwrap();
        delete_bindings_for_symbol(&conn, sid).unwrap();
        assert!(list_for_symbol(&conn, sid).unwrap().is_empty());
    }

    #[test]
    fn fetch_all_sides_returns_joined_fqdn() {
        let conn = fresh_conn();
        let s1 = seed_sym(&conn, "x::a", "a");
        let s2 = seed_sym(&conn, "x::b", "b");
        upsert_binding(&conn, s1, &export_binding(FfiAbi::C, "a")).unwrap();
        upsert_binding(&conn, s2, &export_binding(FfiAbi::C, "b")).unwrap();
        let mut exports = fetch_all_sides(&conn, FfiDirection::Export).unwrap();
        exports.sort_by(|a, b| a.fqdn.cmp(&b.fqdn));
        assert_eq!(exports.len(), 2);
        assert_eq!(exports[0].fqdn, "x::a");
        assert_eq!(exports[0].abi, "c");
        assert_eq!(exports[1].fqdn, "x::b");
        assert!(
            fetch_all_sides(&conn, FfiDirection::Import)
                .unwrap()
                .is_empty(),
            "imports table empty since we only inserted exports"
        );
    }

    #[test]
    fn cascade_drops_bindings_when_symbol_deleted() {
        let conn = fresh_conn();
        let sid = seed_sym(&conn, "x::foo", "foo");
        upsert_binding(&conn, sid, &export_binding(FfiAbi::C, "foo")).unwrap();
        conn.execute("DELETE FROM symbols WHERE id = ?1", [sid])
            .unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbol_ffi_binding", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
}

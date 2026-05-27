//! Stage 3b-7-a — import a linked peer workspace's `module_lookups` +
//! `workspace_imports` rows into the primary DB at cold-start.
//!
//! Prerequisite : the peer workspace must have its own `.standardoc/index.db`
//! already populated (i.e. standardoc was run on the peer at least once).
//! 3b-7-a does NOT extract the peer's source from scratch — that's 3b-7-b
//! territory.
//!
//! Mechanics : open the peer's index DB in `READ_ONLY` mode, read its
//! primary-workspace rows (`workspace_id = 'primary'` in the peer DB),
//! copy them into the master DB tagged with the peer's UUID (registered
//! in `workspace_catalog`). The `cross_workspace::resolve_cross_workspace_import`
//! resolver then naturally walks all workspace_ids on lookup.
//!
//! Best-effort : failures on a single peer are logged + skipped ; cold-start
//! continues for other peers and the primary itself remains usable.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags, params};
use standardoc_ir::LinkedWorkspaceStatus;

use crate::storage::error::StorageError;
use crate::storage::workspace_catalog::LinkedWorkspace;
#[cfg(test)]
use crate::storage::workspace_catalog::list_linked_workspaces;

/// Outcome of attempting to import a single peer workspace's lookup data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PeerImportStats {
    pub workspace_id: String,
    pub root_path: String,
    pub status: PeerImportStatus,
    pub module_lookups_imported: usize,
    pub workspace_imports_imported: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PeerImportStatus {
    /// Peer DB found, rows copied successfully.
    Imported,
    /// Peer DB file does not exist at `<peer_root>/.standardoc/index.db`.
    /// Caller likely never ran standardoc on the peer.
    SkippedMissing,
    /// Peer's `workspace_catalog.status` is paused or archived.
    SkippedInactive,
    /// IO / SQLite / deserialization failure ; carries a short description
    /// for diagnostics. Treated as non-fatal for the cold-start sequence.
    Failed(String),
}

/// Iterate every linked workspace in the primary's `workspace_catalog` and
/// import its lookup rows. Returns the per-peer outcome vector for
/// telemetry / diagnostic reporting.
///
/// Best-effort : a single peer failure does not abort the sweep. The caller
/// (cold-start orchestrator) is expected to log the result and move on.
#[cfg(test)]
fn import_active_peer_workspaces(
    primary_conn: &mut Connection,
) -> Result<Vec<PeerImportStats>, StorageError> {
    let peers = list_linked_workspaces(primary_conn)?;
    let mut stats = Vec::with_capacity(peers.len());
    for peer in peers {
        let outcome = import_peer_workspace(primary_conn, &peer);
        stats.push(outcome);
    }
    Ok(stats)
}

/// Import one peer's `module_lookups` + `workspace_imports` rows.
///
/// The peer's BLOB payloads are copied byte-for-byte (no deserialize / re-
/// serialize round-trip) — assumes both workspaces run a compatible
/// standardoc-ir version. If the peer's payload encoding is older, later
/// `get_module_lookup` calls will surface the decode error naturally ; the
/// import itself stays cheap.
pub(crate) fn import_peer_workspace(
    primary_conn: &mut Connection,
    peer: &LinkedWorkspace,
) -> PeerImportStats {
    let base = PeerImportStats {
        workspace_id: peer.workspace_id.clone(),
        root_path: peer.root_path.clone(),
        status: PeerImportStatus::Imported,
        module_lookups_imported: 0,
        workspace_imports_imported: 0,
    };

    if peer.status != LinkedWorkspaceStatus::Active {
        return PeerImportStats {
            status: PeerImportStatus::SkippedInactive,
            ..base
        };
    }

    let peer_db_path = Path::new(&peer.root_path)
        .join(".standardoc")
        .join("index.db");
    if !peer_db_path.exists() {
        return PeerImportStats {
            status: PeerImportStatus::SkippedMissing,
            ..base
        };
    }

    let peer_conn = match Connection::open_with_flags(
        &peer_db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(c) => c,
        Err(e) => {
            return PeerImportStats {
                status: PeerImportStatus::Failed(format!("open peer DB: {e}")),
                ..base
            };
        }
    };

    match do_import(primary_conn, &peer_conn, &peer.workspace_id) {
        Ok((m_count, i_count)) => PeerImportStats {
            module_lookups_imported: m_count,
            workspace_imports_imported: i_count,
            ..base
        },
        Err(e) => PeerImportStats {
            status: PeerImportStatus::Failed(format!("import: {e}")),
            ..base
        },
    }
}

/// (module_fqdn, language, built_at, payload) — one row of `module_lookups`.
type ModuleLookupRow = (String, String, i64, Vec<u8>);

/// (module_fqdn, local_name, origin_module, origin_symbol, is_type_only, is_re_export)
/// — one row of `workspace_imports`.
type WorkspaceImportRow = (String, String, String, Option<String>, i64, i64);

fn do_import(
    primary: &mut Connection,
    peer: &Connection,
    peer_workspace_id: &str,
) -> Result<(usize, usize), StorageError> {
    // Read peer's primary-tagged module_lookups.
    let lookup_rows = read_peer_module_lookups(peer)?;
    // Read peer's primary-tagged workspace_imports.
    let import_rows = read_peer_workspace_imports(peer)?;

    let now = epoch_ms_now();
    let tx = primary.transaction()?;

    // Clear any prior copy tagged with this peer UUID so the import is
    // a clean overwrite (mirror of put_module_lookup's "INSERT OR REPLACE
    // + delete imports" semantic, applied at peer-workspace granularity).
    tx.execute(
        "DELETE FROM workspace_imports WHERE workspace_id = ?1",
        params![peer_workspace_id],
    )?;
    tx.execute(
        "DELETE FROM module_lookups WHERE workspace_id = ?1",
        params![peer_workspace_id],
    )?;

    let m_count = lookup_rows.len();
    for (module_fqdn, language, built_at, payload) in lookup_rows {
        tx.execute(
            "INSERT INTO module_lookups \
             (module_fqdn, workspace_id, language, built_at, payload) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![module_fqdn, peer_workspace_id, language, built_at, payload],
        )?;
    }

    let i_count = import_rows.len();
    for (module_fqdn, local_name, origin_module, origin_symbol, is_type_only, is_re_export) in
        import_rows
    {
        tx.execute(
            "INSERT INTO workspace_imports \
             (workspace_id, module_fqdn, local_name, origin_module, origin_symbol, \
              is_type_only, is_re_export) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                peer_workspace_id,
                module_fqdn,
                local_name,
                origin_module,
                origin_symbol,
                is_type_only,
                is_re_export,
            ],
        )?;
    }

    tx.execute(
        "UPDATE workspace_catalog SET last_indexed_at = ?1 WHERE workspace_id = ?2",
        params![now, peer_workspace_id],
    )?;

    tx.commit()?;
    Ok((m_count, i_count))
}

fn read_peer_module_lookups(peer: &Connection) -> Result<Vec<ModuleLookupRow>, StorageError> {
    let mut stmt = peer.prepare(
        "SELECT module_fqdn, language, built_at, payload \
         FROM module_lookups WHERE workspace_id = 'primary'",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn read_peer_workspace_imports(peer: &Connection) -> Result<Vec<WorkspaceImportRow>, StorageError> {
    let mut stmt = peer.prepare(
        "SELECT module_fqdn, local_name, origin_module, origin_symbol, \
                is_type_only, is_re_export \
         FROM workspace_imports WHERE workspace_id = 'primary'",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn epoch_ms_now() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis()),
    )
    .unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrate::ensure_schema;
    use crate::storage::module_lookup::put_module_lookup;
    use crate::storage::workspace_catalog::register_linked_workspace;
    use rusqlite::Connection;
    use standardoc_ir::{ImportRecord, IndexingMode, Language, LinkDirection, ModuleLookup};
    use tempfile::TempDir;

    fn fresh_db_at(path: &Path) -> Connection {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        ensure_schema(&conn).unwrap();
        conn
    }

    fn fresh_mem_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        ensure_schema(&conn).unwrap();
        conn
    }

    fn sample_lookup(fqdn: &str, language: Language, origin: &str) -> ModuleLookup {
        let mut m = ModuleLookup::new(fqdn.into(), language);
        m.push_import(ImportRecord {
            local_name: "HashMap".into(),
            origin_module: origin.into(),
            origin_symbol: Some("HashMap".into()),
            is_type_only: false,
            is_re_export: false,
        });
        m
    }

    /// Set up a temporary peer workspace dir with its own `.standardoc/index.db`
    /// populated with `n` sample module_lookups under `workspace_id='primary'`.
    fn setup_peer_workspace(modules: &[(&str, Language, &str)]) -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let stdoc_dir = dir.path().join(".standardoc");
        std::fs::create_dir_all(&stdoc_dir).unwrap();
        let db_path = stdoc_dir.join("index.db");
        let conn = fresh_db_at(&db_path);
        for (fqdn, lang, origin) in modules {
            let lookup = sample_lookup(fqdn, *lang, origin);
            put_module_lookup(&conn, "primary", &lookup).unwrap();
        }
        dir
    }

    #[test]
    fn stage3b7a_skips_peer_with_missing_standardoc_db() {
        let peer_dir = tempfile::tempdir().unwrap();
        let mut primary = fresh_mem_db();
        let peer_id = register_linked_workspace(
            &primary,
            peer_dir.path().to_string_lossy().as_ref(),
            LinkDirection::In,
            IndexingMode::BlobImport,
        )
        .unwrap();

        let stats = import_active_peer_workspaces(&mut primary).unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].workspace_id, peer_id);
        assert_eq!(stats[0].status, PeerImportStatus::SkippedMissing);
        assert_eq!(stats[0].module_lookups_imported, 0);
    }

    #[test]
    fn stage3b7a_imports_peer_module_lookups_under_peer_workspace_id() {
        let peer = setup_peer_workspace(&[
            ("peer::mod_a", Language::Rust, "std::collections"),
            ("peer::mod_b", Language::TypeScript, "react"),
        ]);
        let mut primary = fresh_mem_db();
        let peer_id = register_linked_workspace(
            &primary,
            peer.path().to_string_lossy().as_ref(),
            LinkDirection::In,
            IndexingMode::BlobImport,
        )
        .unwrap();

        let stats = import_active_peer_workspaces(&mut primary).unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].status, PeerImportStatus::Imported);
        assert_eq!(stats[0].module_lookups_imported, 2);
        assert_eq!(stats[0].workspace_imports_imported, 2);

        // Verify the rows are tagged with the peer's UUID, not 'primary'.
        let count: i64 = primary
            .query_row(
                "SELECT COUNT(*) FROM module_lookups WHERE workspace_id = ?1",
                params![&peer_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);

        // Primary's own lookups are untouched (zero — fresh DB).
        let primary_count: i64 = primary
            .query_row(
                "SELECT COUNT(*) FROM module_lookups WHERE workspace_id = 'primary'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(primary_count, 0);
    }

    #[test]
    fn stage3b7a_re_import_overwrites_prior_copy_idempotently() {
        let peer = setup_peer_workspace(&[("peer::initial", Language::Rust, "std::collections")]);
        let mut primary = fresh_mem_db();
        let peer_id = register_linked_workspace(
            &primary,
            peer.path().to_string_lossy().as_ref(),
            LinkDirection::In,
            IndexingMode::BlobImport,
        )
        .unwrap();

        // First import.
        let stats1 = import_active_peer_workspaces(&mut primary).unwrap();
        assert_eq!(stats1[0].module_lookups_imported, 1);

        // Repopulate the peer DB with different rows.
        let peer_db = peer.path().join(".standardoc").join("index.db");
        let peer_conn = Connection::open(&peer_db).unwrap();
        // Delete the prior, add 3 new rows.
        peer_conn
            .execute(
                "DELETE FROM module_lookups WHERE workspace_id = 'primary'",
                [],
            )
            .unwrap();
        peer_conn
            .execute(
                "DELETE FROM workspace_imports WHERE workspace_id = 'primary'",
                [],
            )
            .unwrap();
        for fqdn in &["peer::x", "peer::y", "peer::z"] {
            let lookup = sample_lookup(fqdn, Language::Rust, "std::fmt");
            put_module_lookup(&peer_conn, "primary", &lookup).unwrap();
        }
        drop(peer_conn);

        // Second import overwrites cleanly.
        let stats2 = import_active_peer_workspaces(&mut primary).unwrap();
        assert_eq!(stats2[0].module_lookups_imported, 3);

        // Primary now has exactly 3 rows for this peer (not 1+3=4).
        let count: i64 = primary
            .query_row(
                "SELECT COUNT(*) FROM module_lookups WHERE workspace_id = ?1",
                params![&peer_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn stage3b7a_imported_workspace_imports_resolve_via_cross_workspace_query() {
        let peer = setup_peer_workspace(&[("peer::api", Language::Rust, "std::collections")]);
        let mut primary = fresh_mem_db();
        register_linked_workspace(
            &primary,
            peer.path().to_string_lossy().as_ref(),
            LinkDirection::In,
            IndexingMode::BlobImport,
        )
        .unwrap();
        import_active_peer_workspaces(&mut primary).unwrap();

        // workspace_imports_by_origin should return the peer's row.
        let rows = crate::storage::module_lookup::workspace_imports_by_origin(
            &primary,
            "std::collections",
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].module_fqdn, "peer::api");
        assert_ne!(rows[0].workspace_id, "primary");
    }

    #[test]
    fn stage3b7a_updates_last_indexed_at_on_success() {
        let peer = setup_peer_workspace(&[("peer::x", Language::Rust, "std::fmt")]);
        let mut primary = fresh_mem_db();
        let peer_id = register_linked_workspace(
            &primary,
            peer.path().to_string_lossy().as_ref(),
            LinkDirection::In,
            IndexingMode::BlobImport,
        )
        .unwrap();

        // Pre-import: last_indexed_at is NULL.
        let pre: Option<i64> = primary
            .query_row(
                "SELECT last_indexed_at FROM workspace_catalog WHERE workspace_id = ?1",
                params![&peer_id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(pre.is_none());

        import_active_peer_workspaces(&mut primary).unwrap();

        // Post-import: last_indexed_at is populated.
        let post: Option<i64> = primary
            .query_row(
                "SELECT last_indexed_at FROM workspace_catalog WHERE workspace_id = ?1",
                params![&peer_id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(post.is_some());
        assert!(post.unwrap() > 0);
    }
}

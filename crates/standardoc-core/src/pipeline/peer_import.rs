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
mod tests;

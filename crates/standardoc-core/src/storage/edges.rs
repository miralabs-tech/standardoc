use rusqlite::{Connection, OptionalExtension};
use standardoc_ir::{RawEdge, ResolvedOrUnresolved};

use crate::storage::conv::{edge_confidence_to_sql_text, edge_kind_to_sql_text};
use crate::storage::error::{StorageError, map_constraint};

pub(crate) fn insert_edge(
    conn: &Connection,
    from_symbol_id: i64,
    edge: &RawEdge,
    workspace_id: &str,
) -> Result<i64, StorageError> {
    let (to_symbol_id, to_unresolved) = resolve_target(conn, &edge.to, workspace_id)?;
    let attributes_json = serde_json::to_string(&edge.attributes)?;
    let id = conn
        .query_row(
            "INSERT INTO edges (from_symbol_id, kind, to_symbol_id, to_unresolved, attributes, confidence) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             RETURNING id",
            rusqlite::params![
                from_symbol_id,
                edge_kind_to_sql_text(edge.kind),
                to_symbol_id,
                to_unresolved,
                attributes_json,
                edge_confidence_to_sql_text(edge.confidence),
            ],
            |row| row.get::<_, i64>(0),
        )
        .map_err(map_constraint)?;
    Ok(id)
}

/// Maps an edge target to its on-disk pair `(to_symbol_id, to_unresolved)`.
///
/// Both `Resolved { fqdn }` and `Unresolved { name }` carry a canonical FQDN
/// the provider believes is the intended target — `Resolved` adds the
/// guarantee that the FQDN was confirmed in the same provider scope (typically
/// the same file's `defined_fqdns`). At the storage layer they collapse to
/// the same operation: look the FQDN up in `symbols`, link by id when present,
/// fall back to `to_unresolved` otherwise. This is the symmetric counterpart
/// to [`promote_unresolved_batch`]: that handles "target arrives later",
/// this handles "target was already in DB when caller's edge was inserted".
fn resolve_target(
    conn: &Connection,
    target: &ResolvedOrUnresolved,
    workspace_id: &str,
) -> Result<(Option<i64>, Option<String>), StorageError> {
    match target {
        ResolvedOrUnresolved::Resolved { fqdn } => lookup_or_fallback(conn, fqdn, workspace_id),
        ResolvedOrUnresolved::Unresolved { name } => lookup_or_fallback(conn, name, workspace_id),
        ResolvedOrUnresolved::UnresolvedBridge { bridge, name } => {
            // IR-1 1.0 vocabulary lock: refuse extractor-emitted slugs
            // outside `BUILTIN_BRIDGE_KINDS` that lack the `custom:`
            // prefix. Bubbles up as `StorageError::BridgeKindInvalid`.
            bridge.try_validate()?;
            lookup_or_fallback(
                conn,
                &format!("{}::{}", bridge.as_str(), name),
                workspace_id,
            )
        }
    }
}

fn lookup_or_fallback(
    conn: &Connection,
    fqdn: &str,
    workspace_id: &str,
) -> Result<(Option<i64>, Option<String>), StorageError> {
    let id: Option<i64> = conn
        .query_row(
            "SELECT id FROM symbols WHERE workspace_id = ?1 AND fqdn = ?2",
            rusqlite::params![workspace_id, fqdn],
            |row| row.get(0),
        )
        .optional()?;
    Ok(match id {
        Some(rid) => (Some(rid), None),
        None => (None, Some(fqdn.to_string())),
    })
}

/// Promotes any edge whose `to_unresolved` matches the fqdn of one of the
/// freshly inserted symbols (DDL §3.3). Returns the number of edges promoted.
///
/// Workspace scoping: edges resolve to a target symbol that lives in the
/// SAME workspace as the edge's `from_symbol_id`. This prevents an edge
/// from one workspace silently resolving against a peer workspace's
/// symbol that happens to share the FQDN (`UNIQUE(workspace_id, fqdn)`
/// allows collisions across workspaces).
pub(crate) fn promote_unresolved_batch(
    conn: &Connection,
    new_symbol_ids: &[i64],
) -> Result<u64, StorageError> {
    if new_symbol_ids.is_empty() {
        return Ok(0);
    }
    let placeholders = (1..=new_symbol_ids.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "UPDATE edges \
         SET to_symbol_id = ( \
                 SELECT t.id FROM symbols t \
                 JOIN symbols f ON f.id = edges.from_symbol_id \
                 WHERE t.fqdn = edges.to_unresolved \
                   AND t.workspace_id = f.workspace_id \
             ), \
             to_unresolved = NULL \
         WHERE to_unresolved IN ( \
             SELECT fqdn FROM symbols WHERE id IN ({placeholders}) \
         ) \
         AND ( \
             SELECT t.id FROM symbols t \
             JOIN symbols f ON f.id = edges.from_symbol_id \
             WHERE t.fqdn = edges.to_unresolved \
               AND t.workspace_id = f.workspace_id \
         ) IS NOT NULL"
    );
    let params = rusqlite::params_from_iter(new_symbol_ids.iter().copied());
    conn.execute(&sql, params)?;
    Ok(conn.changes())
}

pub(crate) fn delete_edges_from(
    conn: &Connection,
    from_symbol_id: i64,
) -> Result<u64, StorageError> {
    conn.execute(
        "DELETE FROM edges WHERE from_symbol_id = ?1",
        [from_symbol_id],
    )?;
    Ok(conn.changes())
}

#[cfg(test)]
mod tests;

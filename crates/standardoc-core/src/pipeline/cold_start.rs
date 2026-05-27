use std::path::{Path, PathBuf};

use rayon::prelude::*;
use rusqlite::TransactionBehavior;
use walkdir::{DirEntry, WalkDir};

use standardoc_ir::IndexingMode;

use crate::pipeline::batch::apply_delete_file;
use crate::pipeline::filters::ScanFilters;
use crate::pipeline::paths::{has_supported_extension, to_workspace_relative};
use crate::pipeline::peer_extract;
use crate::pipeline::peer_import;
use crate::pipeline::projects::{discover_and_persist_projects, reconcile_files_project_id};
use crate::pipeline::provider::LanguageProvider;
use crate::pipeline::reindex::{Outcome, commit_outcomes, process_one};
use crate::pipeline::seed_builtins;
use crate::storage::error::StorageError;
use crate::storage::handle::IndexHandle;
use crate::storage::workspace_catalog::list_linked_workspaces;

pub use crate::pipeline::reindex::ColdStartError;

const SUB_BATCH_SIZE: usize = 100;

/// Runs the eager cold start: walks the workspace, extracts symbols for any
/// file whose `files.content_hash` does not match disk, and removes files
/// that disappeared since the last run.
///
/// Blocks the calling thread. The caller orchestrates threading (the server
/// crate post-beta.1 will spawn this on a tokio blocking thread). Must run
/// BEFORE `spawn_watcher` at boot — otherwise live FS events race the scan.
///
/// On crash mid-scan, `schema_meta.cold_start_progress` is left stale until
/// the next invocation, which resets it to `0/<total>` before walking. Skip
/// via `files.content_hash` makes the resume natural: already-ingested files
/// are detected and skipped without re-extraction.
///
/// Pause check at every chunk boundary returns `Ok(())` cleanly when
/// `handle.is_paused()` flips: progress stays stale, the next call resumes
/// via the `content_hash` skip path.
pub fn run(
    handle: &IndexHandle,
    provider: &dyn LanguageProvider,
    filters: &ScanFilters,
) -> Result<(), ColdStartError> {
    let workspace_root = handle.workspace_root().to_path_buf();

    // Stage 3d-2 — discover projects BEFORE walking files so the
    // post-walk reconcile step has everything it needs in one pass.
    // Detection errors are non-fatal: an empty `projects` table just
    // means `files.project_id` stays NULL and consumers degrade
    // gracefully (treat the workspace as a single anonymous project).
    discover_projects_quietly(handle, &workspace_root);

    // Stage 3e-1 — eagerly seed Edge-tier builtin symbols so the
    // synthetic FQDNs emitted by tier-aware resolvers
    // (`<builtin>::ts::Math`, `<builtin>::lua::print`, …) resolve
    // against a real `symbols.id` instead of staying as unresolved
    // canonicals. Best-effort: failures fall back to the pre-3e-1
    // behaviour (edges remain unresolved) without blocking cold start.
    let edge_builtins = provider.edge_builtins();
    seed_builtins::seed_quietly(handle, &edge_builtins);

    let candidates = collect_candidates(&workspace_root, filters)?;
    let total = u64_of(candidates.len());

    set_progress(handle, 0, total)?;

    let mut seen: Vec<String> = Vec::with_capacity(candidates.len());
    let mut done: u64 = 0;

    for chunk in candidates.chunks(SUB_BATCH_SIZE) {
        if handle.is_paused() {
            return Ok(());
        }
        let outcomes = process_chunk(chunk, &workspace_root, handle, provider)?;
        commit_outcomes(handle, &outcomes)?;

        for outcome in &outcomes {
            seen.push(outcome.rel().to_string());
        }

        done = done.saturating_add(u64_of(chunk.len()));
        set_progress(handle, done, total)?;
    }

    cleanup_unseen(handle, &seen)?;
    reconcile_projects_quietly(handle);
    // Stage 3b-7-b Layer 3c — per-peer dispatch. Runs AFTER primary
    // indexing is done so primary's data is always load-bearing; peer
    // ingestion (whether via 3b-7-a blob import or 3b-7-b source
    // extraction) enriches cross-workspace resolution as a bonus.
    // Best-effort like the discover / reconcile steps: failures don't
    // block cold start.
    process_peers_quietly(handle, provider);
    // Stage 1c — C provider `.h` ↔ `.c` join pass. Best-effort: a
    // failure logs to stderr and leaves the raw fn_decl / fn rows in
    // place, which is still a useful (if redundant) index.
    super::c_join::apply_c_join_quietly(handle);
    // Stage 2 — cross-language FFI resolve pass. Reads
    // `symbol_ffi_binding` rows seeded by per-provider taggers and
    // emits IMPORTS edges across exact `(abi, abi_name)` matches.
    super::ffi_resolve::apply_ffi_resolve_quietly(handle);
    // Stage X — second-pass unresolved-edge sweep. The per-file
    // cross_workspace_post invocation inside reindex::process_one
    // runs before later chunks commit their module_lookups, so
    // early-extracted files leave intra-workspace cross-crate edges
    // as Unresolved even though a sibling crate would have answered
    // the query later. Now that every module_lookup is in DB, rerun
    // the resolver across the unresolved edge set and rewrite hits.
    super::resolve_unresolved::apply_resolve_unresolved_quietly(handle);
    clear_progress(handle)?;
    Ok(())
}

/// Stage 3d-2 — best-effort discover + persist. Logged as a warning on
/// failure (typically a transient FS error during the walk); never
/// blocks cold start.
fn discover_projects_quietly(handle: &IndexHandle, workspace_root: &Path) {
    let pool = match handle.pool() {
        Ok(p) => p,
        Err(_) => return,
    };
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return,
    };
    let _ = discover_and_persist_projects(&conn, workspace_root);
}

/// Stage 3d-2 — best-effort reconcile of `files.project_id`. Same
/// graceful-degradation rationale as `discover_projects_quietly`.
fn reconcile_projects_quietly(handle: &IndexHandle) {
    let pool = match handle.pool() {
        Ok(p) => p,
        Err(_) => return,
    };
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return,
    };
    let _ = reconcile_files_project_id(&conn);
}

/// Stage 3b-7-b Layer 3c — best-effort per-peer dispatch.
///
/// Walks every linked workspace registered in `workspace_catalog` and
/// routes each peer through the pipeline its `indexing_mode` selects:
///
/// - [`IndexingMode::BlobImport`] (Stage 3b-7-a, default for legacy
///   rows) — copies the peer's pre-built `module_lookups` +
///   `workspace_imports` blobs into primary's DB. Cheap, assumes the
///   peer's DB is fresh + schema-compatible.
/// - [`IndexingMode::Extract`] (Stage 3b-7-b) — primary walks the
///   peer's source files via `peer_extract::extract_peer_workspace`
///   and indexes them autonomously under the peer's `workspace_id`.
///   Authoritative, no peer-side schema-version assumption.
///
/// Best-effort: a peer whose DB / source is missing or unreadable
/// gets logged as a no-op for that peer; other peers and the primary's
/// cold_start finish untouched.
fn process_peers_quietly(handle: &IndexHandle, provider: &dyn LanguageProvider) {
    let peers = {
        let Ok(pool) = handle.pool() else { return };
        let Ok(conn) = pool.get() else { return };
        match list_linked_workspaces(&conn) {
            Ok(p) => p,
            Err(_) => return,
        }
    };

    for peer in peers {
        match peer.indexing_mode {
            IndexingMode::BlobImport => {
                let Ok(pool) = handle.pool() else { continue };
                let Ok(mut conn) = pool.get() else { continue };
                let _ = peer_import::import_peer_workspace(&mut conn, &peer);
            }
            IndexingMode::Extract => {
                let _ = peer_extract::extract_peer_workspace(handle, &peer, provider);
            }
        }
    }
}

fn u64_of(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

pub(crate) fn collect_candidates(
    workspace_root: &Path,
    filters: &ScanFilters,
) -> Result<Vec<PathBuf>, ColdStartError> {
    let mut out = Vec::new();
    let walker = WalkDir::new(workspace_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !is_dir_excluded(entry, workspace_root, filters));

    for entry in walker {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        if !has_supported_extension(entry.path()) {
            continue;
        }
        let Some(rel) = to_workspace_relative(entry.path(), workspace_root) else {
            continue;
        };
        if filters.is_skipped(&rel) {
            continue;
        }
        out.push(entry.into_path());
    }
    Ok(out)
}

fn is_dir_excluded(entry: &DirEntry, workspace_root: &Path, filters: &ScanFilters) -> bool {
    if entry.depth() == 0 || !entry.file_type().is_dir() {
        return false;
    }
    let Some(rel) = to_workspace_relative(entry.path(), workspace_root) else {
        return false;
    };
    filters.is_skipped(&rel)
}

fn process_chunk(
    chunk: &[PathBuf],
    workspace_root: &Path,
    handle: &IndexHandle,
    provider: &dyn LanguageProvider,
) -> Result<Vec<Outcome>, ColdStartError> {
    let nested = chunk
        .par_iter()
        .map(|abs| process_one(abs, workspace_root, handle, provider))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(nested.into_iter().flatten().collect())
}

fn cleanup_unseen(handle: &IndexHandle, seen: &[String]) -> Result<(), ColdStartError> {
    let pool = handle.pool()?;
    let mut conn = pool.get().map_err(StorageError::from)?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(StorageError::from)?;
    tx.execute("DROP TABLE IF EXISTS seen_paths", [])
        .map_err(StorageError::from)?;
    tx.execute("CREATE TEMP TABLE seen_paths (path TEXT PRIMARY KEY)", [])
        .map_err(StorageError::from)?;
    {
        let mut stmt = tx
            .prepare("INSERT OR IGNORE INTO seen_paths (path) VALUES (?1)")
            .map_err(StorageError::from)?;
        for path in seen {
            stmt.execute([path]).map_err(StorageError::from)?;
        }
    }
    // Route every missing path through `apply_delete_file` so the per-symbol
    // reverse-promote step (`delete_symbol`) runs BEFORE the cascade FK
    // would set `edges.to_symbol_id = NULL`. A raw `DELETE FROM files`
    // here would let the cascade fire `ON DELETE SET NULL` on
    // `edges.to_symbol_id` while leaving `to_unresolved` untouched —
    // immediately violating the XOR CHECK constraint.
    //
    // `is_external = 0` filter (S3-G): external resolvers populate rows
    // whose path lives OUTSIDE the workspace tree (`~/.cargo/registry/...`,
    // `node_modules/...`). The workspace walk never sees those paths so a
    // naive cleanup would purge every cached external on every daemon
    // boot. The flag is set by external resolvers when they submit their
    // `ExtractedFile` and read here to skip cleanup.
    let missing: Vec<String> = {
        let mut stmt = tx
            .prepare(
                "SELECT path FROM files \
                 WHERE is_external = 0 \
                   AND path NOT IN (SELECT path FROM seen_paths)",
            )
            .map_err(StorageError::from)?;
        stmt.query_map([], |row| row.get::<_, String>(0))
            .map_err(StorageError::from)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StorageError::from)?
    };
    for path in &missing {
        apply_delete_file(&tx, path)?;
    }
    let removed = missing.len();
    tx.commit().map_err(StorageError::from)?;
    conn.execute("DROP TABLE seen_paths", [])
        .map_err(StorageError::from)?;
    if removed > 0 {
        handle.bump_revision();
    }
    Ok(())
}

fn set_progress(handle: &IndexHandle, done: u64, total: u64) -> Result<(), ColdStartError> {
    write_progress_value(handle, &format!("{done}/{total}"))
}

fn clear_progress(handle: &IndexHandle) -> Result<(), ColdStartError> {
    write_progress_value(handle, "")
}

fn write_progress_value(handle: &IndexHandle, value: &str) -> Result<(), ColdStartError> {
    let pool = handle.pool()?;
    let conn = pool.get().map_err(StorageError::from)?;
    conn.execute(
        "UPDATE schema_meta SET value = ?1 WHERE key = 'cold_start_progress'",
        [value],
    )
    .map_err(StorageError::from)?;
    Ok(())
}

#[cfg(test)]
mod tests;

//! Stage 3b-7-b Layer 3b: autonomous peer workspace extractor.
//!
//! Walks peer source files (vs reading peer's pre-built DB the way
//! `peer_import` does) and indexes their symbols under the peer's
//! `workspace_id` into primary's DB. Designed to coexist with
//! `peer_import` — the latter stays useful for trusted-peer +
//! schema-matched scenarios where blob copy is cheaper. The
//! `workspace_catalog.indexing_mode` flag (Layer 3c) picks between the
//! two paths per linked workspace; this extractor is wired into
//! cold_start (`process_peers_quietly`, `IndexingMode::Extract`) and the
//! `refresh_peer` MCP handler.
//!
//! Path scoping (Option E from the L3a-bis design discussion):
//! - `files.path` PK is global. Peer files with the same rel-path as
//!   primary (e.g. both have `src/lib.rs`) would collide on insert.
//! - We resolve this at the storage boundary by prefixing peer paths
//!   with `ws:<workspace_id>:`:
//!     primary "src/lib.rs" → stored as `src/lib.rs`        (unchanged)
//!     peer    "src/lib.rs" → stored as `ws:<uuid>:src/lib.rs`
//! - `symbols.workspace_id` (Layer 3a) remains the canonical scope
//!   filter for symbol queries; `file_path` is just a string FK target
//!   that happens to be prefixed for peer rows.
//! - Centralised in `scope_extracted_paths` so the convention has ONE
//!   write site — consumers reading from storage see the prefix
//!   verbatim, which is preferable to a leaky implicit assumption.

use std::path::Path;

use rusqlite::{OptionalExtension, TransactionBehavior};
use serde::Serialize;
use standardoc_ir::{Blake3Hash, ExtractedFile};

use standardoc_ir::LinkedWorkspaceStatus;

use crate::pipeline::batch::{apply_upsert_file, record_parse_error};
use crate::pipeline::cold_start::collect_candidates;
use crate::pipeline::filters::ScanFilters;
use crate::pipeline::paths::{guess_language, to_workspace_relative};
use crate::pipeline::provider::{ExtractContext, ExtractError, LanguageProvider};
use crate::pipeline::reindex::ColdStartError;
use crate::storage::error::StorageError;
use crate::storage::handle::IndexHandle;
use crate::storage::module_lookup::PRIMARY_WORKSPACE_ID;
use crate::storage::workspace_catalog::LinkedWorkspace;

/// Outcome of a single `extract_peer_workspace` invocation. Mirrors
/// the shape of `peer_import::PeerImportStats` so future consumers
/// (cold_start aggregator, MCP responses) can present a unified view
/// of both extraction paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PeerExtractStats {
    pub workspace_id: String,
    pub root_path: String,
    pub status: PeerExtractStatus,
    pub files_extracted: usize,
    pub files_skipped_unchanged: usize,
    pub files_parse_errors: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum PeerExtractStatus {
    Ok,
    SkippedInactive,
    SkippedMissing,
    Failed(String),
}

/// Return the storage-scoped path for a file owned by `workspace_id`.
/// Primary files (`workspace_id == "primary"`) round-trip unchanged so
/// the existing watcher / cold_start / writer code paths keep working
/// without any scoping awareness. Peer files are prefixed with
/// `ws:<workspace_id>:` to dodge the `files.path` PK collision when
/// peer + primary share rel-paths.
pub(crate) fn peer_path(workspace_id: &str, rel: &str) -> String {
    if workspace_id == PRIMARY_WORKSPACE_ID {
        rel.to_string()
    } else {
        format!("ws:{workspace_id}:{rel}")
    }
}

/// Rewrite every embedded path inside `extracted` to its scoped form.
/// Called once per file after the lang-provider returns its result —
/// providers are workspace-unaware and emit unscoped rel paths, so we
/// adapt at the boundary just before handing to `apply_upsert_file`.
pub(crate) fn scope_extracted_paths(extracted: &mut ExtractedFile, workspace_id: &str) {
    if workspace_id == PRIMARY_WORKSPACE_ID {
        return;
    }
    let scoped = peer_path(workspace_id, &extracted.file);
    extracted.file.clone_from(&scoped);
    for sym in &mut extracted.symbols {
        sym.location.file.clone_from(&scoped);
    }
    for edge in &mut extracted.edges {
        for site in &mut edge.sites {
            site.file.clone_from(&scoped);
        }
    }
    for cs in &mut extracted.call_sites {
        cs.site.file.clone_from(&scoped);
    }
}

/// Walk `peer.root_path` and extract its source files into primary's
/// DB tagged with `peer.workspace_id`.
///
/// - Sequential (not concurrent) on purpose: peer extraction is
///   bonus work; primary indexing is load-bearing. A future Layer 3d
///   that runs peers in parallel can revisit if profiling shows it
///   matters.
/// - Wrapped in a single immediate-transaction so the peer's rows
///   land atomically or not at all — partial peer state would leave
///   cross-workspace resolution in an awkward half-imported world.
/// - `files.content_hash` skip path uses the SCOPED path so peer
///   files get the same hit/miss caching behaviour as primary.
/// - `Active` status + an existing `root_path` are pre-flight
///   checked; both failure modes are captured as `PeerExtractStatus`
///   variants rather than errors so a caller iterating linked peers
///   doesn't abort the whole sweep on one bad peer.
pub(crate) fn extract_peer_workspace(
    primary_handle: &IndexHandle,
    peer: &LinkedWorkspace,
    provider: &dyn LanguageProvider,
) -> Result<PeerExtractStats, ColdStartError> {
    let base = PeerExtractStats {
        workspace_id: peer.workspace_id.clone(),
        root_path: peer.root_path.clone(),
        status: PeerExtractStatus::Ok,
        files_extracted: 0,
        files_skipped_unchanged: 0,
        files_parse_errors: 0,
    };

    if peer.status != LinkedWorkspaceStatus::Active {
        return Ok(PeerExtractStats {
            status: PeerExtractStatus::SkippedInactive,
            ..base
        });
    }

    let peer_root = Path::new(&peer.root_path);
    if !peer_root.exists() {
        return Ok(PeerExtractStats {
            status: PeerExtractStatus::SkippedMissing,
            ..base
        });
    }

    let filters = ScanFilters::load(peer_root);
    let candidates = collect_candidates(peer_root, &filters)?;

    let pool = primary_handle.pool()?;
    let mut conn = pool.get().map_err(StorageError::from)?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(StorageError::from)?;
    let next_revision = primary_handle.revision().saturating_add(1);

    let mut extracted_count = 0usize;
    let mut skipped_count = 0usize;
    let mut parse_error_count = 0usize;

    for abs in &candidates {
        let Some(rel) = to_workspace_relative(abs, peer_root) else {
            continue;
        };
        let scoped = peer_path(&peer.workspace_id, &rel);

        let bytes = match std::fs::read(abs) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("standardoc peer_extract: read failed for {scoped}: {e}");
                continue;
            }
        };
        let hash = Blake3Hash::new(*blake3::hash(&bytes).as_bytes());
        if peer_hash_matches_db(&tx, &scoped, hash)? {
            skipped_count += 1;
            continue;
        }
        let Ok(content) = String::from_utf8(bytes) else {
            eprintln!("standardoc peer_extract: non-utf8 content for {scoped}, skipping");
            continue;
        };

        let ctx = ExtractContext {
            workspace_root: peer_root,
            cross_workspace: None,
        };
        match provider.extract(&content, &rel, &ctx) {
            Ok(mut extracted) => {
                extracted.content_hash = hash;
                scope_extracted_paths(&mut extracted, &peer.workspace_id);
                apply_upsert_file(&tx, &extracted, next_revision, &peer.workspace_id)?;
                extracted_count += 1;
            }
            Err(ExtractError::Parse { detail, .. }) => {
                if let Some(lang) = guess_language(&rel) {
                    record_parse_error(&tx, &scoped, lang, &detail)?;
                }
                parse_error_count += 1;
            }
            Err(ExtractError::Io(e)) => {
                eprintln!("standardoc peer_extract: provider io error on {scoped}: {e}");
            }
            Err(ExtractError::UnsupportedLanguage { .. }) => {
                // Silently skip peer files whose extension primary's
                // lang-provider doesn't know — the cold_start walk
                // would have already filtered most of these via
                // `has_supported_extension`, but a SFC or other
                // dispatch-time failure can still land here.
            }
        }
    }

    tx.commit().map_err(StorageError::from)?;
    if extracted_count > 0 || parse_error_count > 0 {
        primary_handle.bump_revision();
    }

    Ok(PeerExtractStats {
        files_extracted: extracted_count,
        files_skipped_unchanged: skipped_count,
        files_parse_errors: parse_error_count,
        ..base
    })
}

/// Transaction-scoped variant of `reindex::hash_matches_db` — the
/// public helper opens its own pool connection (so it sees only
/// committed state), but peer extraction runs inside an immediate
/// transaction so the skip-on-unchanged check must read through the
/// same `tx`.
fn peer_hash_matches_db(
    conn: &rusqlite::Connection,
    scoped_path: &str,
    new_hash: Blake3Hash,
) -> Result<bool, StorageError> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT content_hash FROM files WHERE path = ?1",
            [scoped_path],
            |r| r.get(0),
        )
        .optional()?;
    Ok(stored.is_some_and(|hex| hex == new_hash.to_hex()))
}

#[cfg(test)]
mod tests;

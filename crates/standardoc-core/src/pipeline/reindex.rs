//! Per-path ingestion primitives shared between `cold_start::run` (full
//! workspace scan) and the watcher (`.stdignore` pattern-removal subtree
//! re-index). `reindex_paths` is the public re-index entrypoint; `cold_start`
//! reuses the same `process_one` + `commit_outcomes` helpers per chunk.

use std::path::Path;

use rusqlite::{OptionalExtension, TransactionBehavior};
use standardoc_ir::{Blake3Hash, ExtractedFile, Language};
use thiserror::Error;

use crate::pipeline::batch::{apply_upsert_file, record_parse_error};
use crate::pipeline::filters::ScanFilters;
use crate::pipeline::paths::{guess_language, to_workspace_relative};
use crate::pipeline::provider::{ExtractContext, ExtractError, LanguageProvider};
use crate::storage::error::StorageError;
use crate::storage::handle::IndexHandle;
use crate::storage::module_lookup::PRIMARY_WORKSPACE_ID;

#[derive(Debug, Error)]
pub enum ColdStartError {
    #[error("storage: {0}")]
    Storage(#[from] StorageError),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("walk: {0}")]
    Walk(#[from] walkdir::Error),
}

pub(crate) enum Outcome {
    Upsert {
        rel: String,
        extracted: ExtractedFile,
    },
    ParseError {
        rel: String,
        language: Language,
        detail: String,
    },
    Skip {
        rel: String,
    },
}

impl Outcome {
    pub(crate) fn rel(&self) -> &str {
        match self {
            Self::Upsert { rel, .. } | Self::ParseError { rel, .. } | Self::Skip { rel } => rel,
        }
    }
}

/// Re-indexes a list of workspace-relative paths in a single transaction.
/// Used by the watcher when a `.stdignore` pattern is removed: the formerly
/// excluded subtree must come back into the index. Filters are consulted
/// defensively — any path still excluded is silently skipped. The pause flag
/// short-circuits the call with `Ok(())` (the watcher will retry naturally on
/// the next FS event after `resume`).
pub(crate) fn reindex_paths(
    handle: &IndexHandle,
    provider: &dyn LanguageProvider,
    paths: &[String],
    filters: &ScanFilters,
) -> Result<(), ColdStartError> {
    if paths.is_empty() || handle.is_paused() {
        return Ok(());
    }
    let workspace_root = handle.workspace_root().to_path_buf();
    let mut outcomes = Vec::with_capacity(paths.len());
    for rel in paths {
        if filters.is_skipped(rel) {
            continue;
        }
        let abs = workspace_root.join(rel);
        if let Some(outcome) = process_one(&abs, &workspace_root, handle, provider)? {
            outcomes.push(outcome);
        }
    }
    commit_outcomes(handle, &outcomes)
}

pub(crate) fn process_one(
    abs: &Path,
    workspace_root: &Path,
    handle: &IndexHandle,
    provider: &dyn LanguageProvider,
) -> Result<Option<Outcome>, ColdStartError> {
    let Some(rel) = to_workspace_relative(abs, workspace_root) else {
        return Ok(None);
    };

    let bytes = match std::fs::read(abs) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("standardoc ingest: read failed for {rel}: {e}");
            return Ok(None);
        }
    };

    let hash = Blake3Hash::new(*blake3::hash(&bytes).as_bytes());
    if hash_matches_db(handle, &rel, hash)? {
        return Ok(Some(Outcome::Skip { rel }));
    }

    let Ok(content) = String::from_utf8(bytes) else {
        eprintln!("standardoc ingest: non-utf8 content for {rel}, skipping");
        return Ok(None);
    };

    let resolver = crate::cross_workspace_resolver::DbCrossWorkspaceResolver::new(handle);
    let ctx = ExtractContext {
        workspace_root,
        cross_workspace: Some(&resolver),
    };
    match provider.extract(&content, &rel, &ctx) {
        Ok(mut extracted) => {
            extracted.content_hash = hash;
            crate::pipeline::cross_workspace_post::rewrite_cross_workspace_edges(
                &mut extracted,
                &resolver,
            );
            Ok(Some(Outcome::Upsert { rel, extracted }))
        }
        Err(ExtractError::Parse { detail, .. }) => {
            let Some(language) = guess_language(&rel) else {
                eprintln!(
                    "standardoc ingest: parse error on {rel} but extension is unknown: {detail}"
                );
                return Ok(None);
            };
            Ok(Some(Outcome::ParseError {
                rel,
                language,
                detail,
            }))
        }
        Err(ExtractError::Io(e)) => {
            eprintln!("standardoc ingest: provider io error on {rel}: {e}");
            Ok(None)
        }
        Err(ExtractError::UnsupportedLanguage { .. }) => {
            eprintln!("standardoc ingest: unsupported language for {rel}");
            Ok(None)
        }
    }
}

pub(crate) fn hash_matches_db(
    handle: &IndexHandle,
    rel: &str,
    new_hash: Blake3Hash,
) -> Result<bool, ColdStartError> {
    let pool = handle.pool()?;
    let conn = pool.get().map_err(StorageError::from)?;
    let stored: Option<String> = conn
        .query_row(
            "SELECT content_hash FROM files WHERE path = ?1",
            [rel],
            |r| r.get(0),
        )
        .optional()
        .map_err(StorageError::from)?;
    Ok(stored.is_some_and(|hex| hex == new_hash.to_hex()))
}

pub(crate) fn commit_outcomes(
    handle: &IndexHandle,
    outcomes: &[Outcome],
) -> Result<(), ColdStartError> {
    let has_writes = outcomes.iter().any(|o| !matches!(o, Outcome::Skip { .. }));
    if !has_writes {
        return Ok(());
    }
    let pool = handle.pool()?;
    let mut conn = pool.get().map_err(StorageError::from)?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(StorageError::from)?;
    // All upserts in this batch share the post-commit revision (handle.revision
    // is bumped once after the transaction succeeds).
    let next_revision = handle.revision().saturating_add(1);
    for outcome in outcomes {
        match outcome {
            Outcome::Upsert { extracted, .. } => {
                apply_upsert_file(&tx, extracted, next_revision, PRIMARY_WORKSPACE_ID)?;
            }
            Outcome::ParseError {
                rel,
                language,
                detail,
            } => record_parse_error(&tx, rel, *language, detail)?,
            Outcome::Skip { .. } => {}
        }
    }
    tx.commit().map_err(StorageError::from)?;
    handle.bump_revision();
    Ok(())
}

#![allow(
    clippy::significant_drop_tightening,
    clippy::significant_drop_in_scrutinee
)]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

use notify::{EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{
    DebounceEventResult, DebouncedEvent, Debouncer, RecommendedCache, new_debouncer,
};
use walkdir::WalkDir;

use crate::commands::IngestCommand;
use crate::pipeline::external_invalidation;
use crate::pipeline::filters::ScanFilters;
use crate::pipeline::manifest_invalidation;
use crate::pipeline::paths::{guess_language, has_supported_extension, to_workspace_relative};
use crate::pipeline::provider::{ExtractContext, ExtractError, LanguageProvider};
use crate::pipeline::reindex::reindex_paths;
use crate::storage::error::StorageError;
use crate::storage::handle::IndexHandle;

#[derive(Debug, thiserror::Error)]
pub enum WatcherError {
    #[error("storage: {0}")]
    Storage(#[from] StorageError),

    #[error("notify: {0}")]
    Notify(#[from] notify::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// One peer workspace the watcher is actively observing. The `root` is
/// canonicalised at `add_peer` time so the dispatch loop's
/// `path.starts_with(root)` routing matches what `notify` reports.
#[derive(Debug, Clone)]
pub struct PeerRoot {
    pub workspace_id: String,
    pub root: PathBuf,
}

/// Field order is load-bearing: `debouncer` MUST be dropped before
/// `dispatch_thread`. Dropping the debouncer closes the internal notify
/// channel, which lets the dispatch loop's `Receiver::recv` return `Err`
/// and the thread exit on its own. Reversing the fields would block the
/// manual `Drop` join forever on a live channel.
pub struct WatcherHandle {
    debouncer: Option<Debouncer<RecommendedWatcher, RecommendedCache>>,
    dispatch_thread: Option<JoinHandle<()>>,
    /// L3d-2: live registry of peer roots routed through the same
    /// dispatch thread. Mutable via [`Self::add_peer`] /
    /// [`Self::remove_peer`] while the watcher is running.
    peers: Arc<RwLock<Vec<PeerRoot>>>,
}

impl Drop for WatcherHandle {
    fn drop(&mut self) {
        self.debouncer = None;
        if let Some(t) = self.dispatch_thread.take() {
            let _ = t.join();
        }
    }
}

impl WatcherHandle {
    /// Start watching `root` and route its events through the dispatch
    /// thread tagged with `workspace_id`. Idempotent — adding the same
    /// `workspace_id` twice is a no-op (avoids double-watching). Returns
    /// `Err` if the path cannot be canonicalised or if `notify` refuses
    /// the new watch (e.g. exceeded inotify limits on Linux).
    pub fn add_peer(&mut self, workspace_id: String, root: &Path) -> Result<(), WatcherError> {
        let Some(d) = self.debouncer.as_mut() else {
            return Err(WatcherError::Storage(StorageError::InvalidStoredData {
                detail: "watcher debouncer already dropped".into(),
            }));
        };
        let canonical = std::fs::canonicalize(root).map_err(WatcherError::Io)?;

        {
            let guard = self
                .peers
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if guard.iter().any(|p| p.workspace_id == workspace_id) {
                return Ok(());
            }
        }

        d.watch(&canonical, RecursiveMode::Recursive)
            .map_err(WatcherError::Notify)?;

        let mut guard = self
            .peers
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.push(PeerRoot {
            workspace_id,
            root: canonical,
        });
        Ok(())
    }

    /// Stop watching a previously-registered peer. Idempotent — removing
    /// an unknown id is a no-op. Removing succeeds even if the underlying
    /// `unwatch` fails (the registry entry is gone, so events won't route).
    pub fn remove_peer(&mut self, workspace_id: &str) -> Result<(), WatcherError> {
        let Some(d) = self.debouncer.as_mut() else {
            return Err(WatcherError::Storage(StorageError::InvalidStoredData {
                detail: "watcher debouncer already dropped".into(),
            }));
        };

        let removed = {
            let mut guard = self
                .peers
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard
                .iter()
                .position(|p| p.workspace_id == workspace_id)
                .map(|i| guard.remove(i))
        };

        if let Some(p) = removed {
            d.unwatch(&p.root).map_err(WatcherError::Notify)?;
        }
        Ok(())
    }

    /// Snapshot of currently-watched peers. Test / introspection helper —
    /// the registry is mutable so the result is point-in-time.
    pub fn peers_snapshot(&self) -> Vec<PeerRoot> {
        self.peers
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

pub fn spawn_watcher(
    handle: IndexHandle,
    provider: Arc<dyn LanguageProvider>,
    filters: Arc<RwLock<ScanFilters>>,
) -> Result<WatcherHandle, WatcherError> {
    let workspace_root = handle.workspace_root().to_path_buf();
    let debounce_ms = read_debounce_ms(&handle)?;

    let (tx, rx) = channel::<DebounceEventResult>();
    let mut debouncer = new_debouncer(Duration::from_millis(debounce_ms), None, tx)?;
    debouncer
        .watch(&workspace_root, RecursiveMode::Recursive)
        .map_err(WatcherError::Notify)?;

    let peers: Arc<RwLock<Vec<PeerRoot>>> = Arc::new(RwLock::new(Vec::new()));
    let thread_root = workspace_root.clone();
    let thread_peers = Arc::clone(&peers);
    let dispatch_thread = std::thread::Builder::new()
        .name("standardoc-watcher".into())
        .spawn(move || {
            dispatch_loop(
                &rx,
                &handle,
                provider.as_ref(),
                &thread_root,
                &filters,
                &thread_peers,
            );
        })?;

    Ok(WatcherHandle {
        debouncer: Some(debouncer),
        dispatch_thread: Some(dispatch_thread),
        peers,
    })
}

fn read_debounce_ms(handle: &IndexHandle) -> Result<u64, WatcherError> {
    let pool = handle.pool()?;
    let conn = pool.get().map_err(StorageError::from)?;
    let value: String = conn
        .query_row(
            "SELECT value FROM schema_meta WHERE key = 'watcher_debounce_ms'",
            [],
            |row| row.get(0),
        )
        .map_err(StorageError::from)?;
    value.parse::<u64>().map_err(|_| {
        WatcherError::Storage(StorageError::InvalidStoredData {
            detail: format!("malformed watcher_debounce_ms: {value}"),
        })
    })
}

fn dispatch_loop(
    rx: &Receiver<DebounceEventResult>,
    handle: &IndexHandle,
    provider: &dyn LanguageProvider,
    workspace_root: &Path,
    filters: &Arc<RwLock<ScanFilters>>,
    peers: &Arc<RwLock<Vec<PeerRoot>>>,
) {
    while let Ok(result) = rx.recv() {
        if handle.is_paused() {
            continue;
        }
        match result {
            Ok(events) => {
                for event in events {
                    process_event(&event, handle, provider, workspace_root, filters, peers);
                }
            }
            Err(errors) => {
                for e in errors {
                    eprintln!("standardoc watcher: notify error: {e}");
                }
            }
        }
    }
}

fn process_event(
    event: &DebouncedEvent,
    handle: &IndexHandle,
    provider: &dyn LanguageProvider,
    workspace_root: &Path,
    filters: &Arc<RwLock<ScanFilters>>,
    peers: &Arc<RwLock<Vec<PeerRoot>>>,
) {
    if !is_dispatchable(&event.event.kind) {
        return;
    }
    for path in &event.event.paths {
        match resolve_owner(path, workspace_root, peers) {
            Some(Owner::Primary) => {
                process_primary_path(path, handle, provider, workspace_root, filters);
            }
            Some(Owner::Peer {
                workspace_id,
                peer_root,
            }) => {
                process_peer_path(path, handle, provider, &workspace_id, &peer_root);
            }
            None => {
                // Silently skip — path lives under neither the primary
                // root nor any registered peer root (spurious / symlink
                // edge case). Matches the existing "skip unsupported"
                // posture; no log to keep the dispatch loop quiet.
            }
        }
    }
}

/// Routing key — which workspace owns the event path. Resolved per-event
/// in [`process_event`].
enum Owner {
    Primary,
    Peer {
        workspace_id: String,
        peer_root: PathBuf,
    },
}

fn resolve_owner(
    abs_path: &Path,
    primary_root: &Path,
    peers: &Arc<RwLock<Vec<PeerRoot>>>,
) -> Option<Owner> {
    if abs_path.starts_with(primary_root) {
        return Some(Owner::Primary);
    }
    let guard = peers
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for p in guard.iter() {
        if abs_path.starts_with(&p.root) {
            return Some(Owner::Peer {
                workspace_id: p.workspace_id.clone(),
                peer_root: p.root.clone(),
            });
        }
    }
    None
}

fn process_primary_path(
    abs_path: &Path,
    handle: &IndexHandle,
    provider: &dyn LanguageProvider,
    workspace_root: &Path,
    filters: &Arc<RwLock<ScanFilters>>,
) {
    let Some(rel) = to_workspace_relative(abs_path, workspace_root) else {
        return;
    };

    // Bug E-3 follow-up P2 — `standardoc.sxd` is the new config source.
    // Edits trigger a full re-load (ignore + projects + groups). The
    // legacy `.stdignore` is no longer watched at the root ; nested
    // `.stdignore` files in subdirectories still cascade (see
    // `GitignoreStack::collect_layers`) for per-subfolder excludes.
    if abs_path.file_name() == Some(OsStr::new(crate::config::SXD_CONFIG_FILENAME)) {
        if let Err(e) = handle_sxd_change(handle, provider, workspace_root, filters) {
            eprintln!("standardoc watcher: standardoc.sxd reload failed: {e}");
        }
        return;
    }

    // External resolver invalidation: when one of the tracked lockfiles
    // (`Cargo.lock`, `package-lock.json`, `pnpm-lock.yaml`, `yarn.lock`,
    // `.pnp.cjs`) changes, drop the corresponding `is_external = 1` bucket
    // so the next `resolve_external` call repopulates from the new
    // dependency tree. Returns early because lockfiles are never indexed
    // as workspace symbols (no supported extension anyway).
    match external_invalidation::handle_lockfile_change(handle, workspace_root, abs_path) {
        Ok(Some(_)) => return,
        Ok(None) => {}
        Err(e) => {
            eprintln!("standardoc watcher: lockfile invalidation failed: {e}");
            return;
        }
    }

    // Stage 3d-5: workspace manifest re-detection. When a manifest file
    // (`Cargo.toml`, `package.json`, `pnpm-workspace.yaml`, `*.sxb`, …)
    // changes, re-run `discover_and_persist_projects` so the `projects`
    // table + `schema_meta.workspace_kind` stay in sync. Returns early
    // because manifest files are not indexed as workspace source.
    match manifest_invalidation::handle_manifest_change(handle, workspace_root, abs_path) {
        Ok(Some(())) => return,
        Ok(None) => {}
        Err(e) => {
            eprintln!("standardoc watcher: manifest re-detection failed: {e}");
            return;
        }
    }

    if filters_skipped(filters, &rel) {
        return;
    }

    if !has_supported_extension(abs_path) {
        return;
    }

    // Only file-granular events reach here — the extension gate above drops
    // directory events. A recursive directory deletion that `notify`
    // coalesces into a single dir-level Remove therefore leaves the file
    // rows beneath it orphaned; they are reconciled away on the next
    // `cold_start::cleanup_unseen` sweep (DB paths the walk no longer sees
    // are deleted).
    if abs_path.is_file() {
        upsert_path(handle, provider, abs_path, &rel, workspace_root);
    } else if !abs_path.exists() {
        delete_path(handle, &rel);
    }
}

/// Peer counterpart of [`process_primary_path`]. Deliberately leaner —
/// no `.stdignore` reload, no lockfile/manifest invalidation, no scan
/// filters (those are primary-scoped concerns). Just: extension gate +
/// supported-language gate + scoped upsert/delete via the peer
/// `IngestCommand` variants (L3d-1).
fn process_peer_path(
    abs_path: &Path,
    handle: &IndexHandle,
    provider: &dyn LanguageProvider,
    workspace_id: &str,
    peer_root: &Path,
) {
    let Some(rel) = to_workspace_relative(abs_path, peer_root) else {
        return;
    };

    if !has_supported_extension(abs_path) {
        return;
    }

    if abs_path.is_file() {
        upsert_peer_path(handle, provider, abs_path, &rel, peer_root, workspace_id);
    } else if !abs_path.exists() {
        delete_peer_path(handle, &rel, workspace_id);
    }
}

fn handle_sxd_change(
    handle: &IndexHandle,
    provider: &dyn LanguageProvider,
    workspace_root: &Path,
    filters: &Arc<RwLock<ScanFilters>>,
) -> Result<(), WatcherError> {
    // `ScanFilters::load` consults `standardoc.sxd` first (ignore block
    // wins) and falls back to nested `.stdignore` cascade. Reading
    // `GitignoreStack::build` directly would miss the sxd block.
    let new_filters = ScanFilters::load(workspace_root);

    let newly_excluded = collect_newly_excluded_db_paths(handle, filters, &new_filters)?;
    let newly_allowed =
        collect_newly_allowed_workspace_paths(workspace_root, filters, &new_filters);

    {
        let mut guard = filters
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = new_filters;
    }

    if !newly_excluded.is_empty() {
        eprintln!(
            "standardoc: {} indexed path{} now match `standardoc.sxd` ignore exclusions — \
             run `standardoc purge-excluded` to remove",
            newly_excluded.len(),
            if newly_excluded.len() == 1 { "" } else { "s" }
        );
    }

    if !newly_allowed.is_empty() {
        let guard = filters
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Err(e) = reindex_paths(handle, provider, &newly_allowed, &guard) {
            eprintln!("standardoc watcher: re-index failed for newly-allowed paths: {e}");
        }
    }

    Ok(())
}

fn collect_newly_excluded_db_paths(
    handle: &IndexHandle,
    old_filters: &Arc<RwLock<ScanFilters>>,
    new_filters: &ScanFilters,
) -> Result<Vec<String>, WatcherError> {
    let paths = handle.list_all_file_paths()?;
    let guard = old_filters
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut out = Vec::new();
    for path in paths {
        if new_filters.is_skipped(&path) && !guard.is_skipped(&path) {
            out.push(path);
        }
    }
    Ok(out)
}

fn collect_newly_allowed_workspace_paths(
    workspace_root: &Path,
    old_filters: &Arc<RwLock<ScanFilters>>,
    new_filters: &ScanFilters,
) -> Vec<String> {
    let guard = old_filters
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let walker = WalkDir::new(workspace_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !new_filters.is_dir_excluded(entry, workspace_root));

    let mut out = Vec::new();
    for entry in walker.flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        if !has_supported_extension(entry.path()) {
            continue;
        }
        let Some(rel) = to_workspace_relative(entry.path(), workspace_root) else {
            continue;
        };
        if guard.is_skipped(&rel) && !new_filters.is_skipped(&rel) {
            out.push(rel);
        }
    }
    out
}

fn filters_skipped(filters: &Arc<RwLock<ScanFilters>>, rel: &str) -> bool {
    let guard = filters
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.is_skipped(rel)
}

fn upsert_path(
    handle: &IndexHandle,
    provider: &dyn LanguageProvider,
    abs_path: &Path,
    rel: &str,
    workspace_root: &Path,
) {
    let content = match std::fs::read_to_string(abs_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("standardoc watcher: read failed for {rel}: {e}");
            return;
        }
    };
    let resolver = crate::cross_workspace_resolver::DbCrossWorkspaceResolver::new(handle);
    let ctx = ExtractContext {
        workspace_root,
        cross_workspace: Some(&resolver),
    };
    match provider.extract(&content, rel, &ctx) {
        Ok(mut extracted) => {
            crate::pipeline::cross_workspace_post::rewrite_cross_workspace_edges(
                &mut extracted,
                &resolver,
            );
            try_send_command(
                handle,
                IngestCommand::UpsertFile {
                    path: rel.to_string(),
                    extracted,
                },
            );
        }
        Err(ExtractError::Parse { detail, .. }) => {
            if let Some(language) = guess_language(rel) {
                try_send_command(
                    handle,
                    IngestCommand::RecordParseError {
                        path: rel.to_string(),
                        language,
                        detail,
                    },
                );
            } else {
                eprintln!(
                    "standardoc watcher: parse error on {rel} but extension is unknown: {detail}"
                );
            }
        }
        Err(ExtractError::Io(e)) => {
            eprintln!("standardoc watcher: provider io error on {rel}: {e}");
        }
        Err(ExtractError::UnsupportedLanguage { .. }) => {
            eprintln!("standardoc watcher: unsupported language for {rel}");
        }
    }
}

fn delete_path(handle: &IndexHandle, rel: &str) {
    try_send_command(
        handle,
        IngestCommand::DeleteFile {
            path: rel.to_string(),
        },
    );
}

fn upsert_peer_path(
    handle: &IndexHandle,
    provider: &dyn LanguageProvider,
    abs_path: &Path,
    rel: &str,
    peer_root: &Path,
    workspace_id: &str,
) {
    let content = match std::fs::read_to_string(abs_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("standardoc watcher: read failed for peer {workspace_id}/{rel}: {e}");
            return;
        }
    };
    let ctx = ExtractContext {
        workspace_root: peer_root,
        cross_workspace: None,
    };
    match provider.extract(&content, rel, &ctx) {
        Ok(extracted) => try_send_command(
            handle,
            IngestCommand::UpsertPeerFile {
                workspace_id: workspace_id.to_string(),
                path: rel.to_string(),
                extracted,
            },
        ),
        Err(ExtractError::Parse { detail, .. }) => {
            if let Some(language) = guess_language(rel) {
                try_send_command(
                    handle,
                    IngestCommand::RecordPeerParseError {
                        workspace_id: workspace_id.to_string(),
                        path: rel.to_string(),
                        language,
                        detail,
                    },
                );
            } else {
                eprintln!(
                    "standardoc watcher: parse error on peer {workspace_id}/{rel} \
                     but extension is unknown: {detail}"
                );
            }
        }
        Err(ExtractError::Io(e)) => {
            eprintln!("standardoc watcher: provider io error on peer {workspace_id}/{rel}: {e}");
        }
        Err(ExtractError::UnsupportedLanguage { .. }) => {
            eprintln!("standardoc watcher: unsupported language for peer {workspace_id}/{rel}");
        }
    }
}

fn delete_peer_path(handle: &IndexHandle, rel: &str, workspace_id: &str) {
    try_send_command(
        handle,
        IngestCommand::DeletePeerFile {
            workspace_id: workspace_id.to_string(),
            path: rel.to_string(),
        },
    );
}

fn try_send_command(handle: &IndexHandle, cmd: IngestCommand) {
    if let Err(e) = handle.try_submit(cmd) {
        eprintln!("standardoc watcher: failed to submit ingest command: {e}");
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_dispatchable(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

#[cfg(test)]
mod tests;

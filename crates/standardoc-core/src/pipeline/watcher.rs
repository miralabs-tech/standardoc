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
use walkdir::{DirEntry, WalkDir};

use crate::commands::IngestCommand;
use crate::pipeline::external_invalidation;
use crate::pipeline::filters::{GitignoreStack, STDIGNORE_FILENAME, ScanFilters};
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
    pub fn add_peer(
        &mut self,
        workspace_id: String,
        root: &Path,
    ) -> Result<(), WatcherError> {
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

    if abs_path.file_name() == Some(OsStr::new(STDIGNORE_FILENAME)) {
        if let Err(e) = handle_stdignore_change(handle, provider, workspace_root, filters) {
            eprintln!("standardoc watcher: .stdignore reload failed: {e}");
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
        Ok(Some(_)) => return,
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

fn handle_stdignore_change(
    handle: &IndexHandle,
    provider: &dyn LanguageProvider,
    workspace_root: &Path,
    filters: &Arc<RwLock<ScanFilters>>,
) -> Result<(), WatcherError> {
    let new_filters = ScanFilters::from_stack(GitignoreStack::build(workspace_root));

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
            "standardoc: {} indexed path{} now match `.stdignore` exclusions — \
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
        .filter_entry(|entry| !is_dir_excluded(entry, workspace_root, new_filters));

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

fn is_dir_excluded(entry: &DirEntry, workspace_root: &Path, filters: &ScanFilters) -> bool {
    if entry.depth() == 0 || !entry.file_type().is_dir() {
        return false;
    }
    let Some(rel) = to_workspace_relative(entry.path(), workspace_root) else {
        return false;
    };
    filters.is_skipped(&rel)
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
    let ctx = ExtractContext { workspace_root };
    match provider.extract(&content, rel, &ctx) {
        Ok(extracted) => try_send_command(
            handle,
            IngestCommand::UpsertFile {
                path: rel.to_string(),
                extracted,
            },
        ),
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
mod tests {
    use std::time::{Duration, Instant};

    use notify::event::{CreateKind, ModifyKind, RemoveKind};
    use standardoc_ir::{
        Blake3Hash, ExtractedFile, Kind, Language, LanguageKind, RawSymbol, SourceOrigin,
        SymbolLocation, Visibility,
    };
    use tempfile::tempdir;

    use super::*;
    use crate::pipeline::provider::mock::{MockProvider, MockResponse};

    fn sample_extracted(file: &str, fqdn: &str) -> ExtractedFile {
        ExtractedFile {
            file: file.into(),
            language: Language::Rust,
            source_origin: SourceOrigin::Workspace,
            is_external: false,
            content_hash: Blake3Hash::new([0xab; 32]),
            byte_size: 100,
            symbols: vec![RawSymbol {
                name: fqdn.rsplit("::").next().unwrap_or(fqdn).into(),
                fqdn: fqdn.into(),
                kind: Kind::Function,
                language_kind: LanguageKind::from("fn_item"),
                module: None,
                visibility: Visibility::Public,
                location: SymbolLocation {
                    file: file.into(),
                    start_line: 1,
                    end_line: 5,
                    start_col: 0,
                    end_col: 1,
                },
                signature: None,
                body_hash: Some(Blake3Hash::new([0x01; 32])),
                attributes: vec![],
                flags: vec![],
            }],
            edges: vec![],
            call_sites: vec![],
            documents: vec![],
        }
    }

    fn set_debounce_ms(handle: &IndexHandle, ms: u64) {
        let conn = handle.pool().unwrap().get().unwrap();
        conn.execute(
            "UPDATE schema_meta SET value = ?1 WHERE key = 'watcher_debounce_ms'",
            [ms.to_string()],
        )
        .unwrap();
    }

    fn wait_revision_at_least(handle: &IndexHandle, target: u64, timeout: Duration) {
        let start = Instant::now();
        while handle.revision() < target {
            assert!(
                start.elapsed() <= timeout,
                "revision did not reach {target} within {timeout:?} (was {})",
                handle.revision()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn fresh_filters(handle: &IndexHandle) -> Arc<RwLock<ScanFilters>> {
        Arc::new(RwLock::new(ScanFilters::load(handle.workspace_root())))
    }

    #[test]
    fn read_debounce_ms_returns_seeded_default_500() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        assert_eq!(read_debounce_ms(&handle).unwrap(), 500);
    }

    #[test]
    fn read_debounce_ms_returns_invalid_stored_data_on_garbage() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        let conn = handle.pool().unwrap().get().unwrap();
        conn.execute(
            "UPDATE schema_meta SET value = 'lol' WHERE key = 'watcher_debounce_ms'",
            [],
        )
        .unwrap();
        drop(conn);
        let err = read_debounce_ms(&handle).unwrap_err();
        assert!(matches!(
            err,
            WatcherError::Storage(StorageError::InvalidStoredData { .. })
        ));
    }

    #[test]
    fn is_dispatchable_filters_event_kinds() {
        assert!(is_dispatchable(&EventKind::Create(CreateKind::File)));
        assert!(is_dispatchable(&EventKind::Modify(ModifyKind::Any)));
        assert!(is_dispatchable(&EventKind::Remove(RemoveKind::File)));
        assert!(!is_dispatchable(&EventKind::Any));
        assert!(!is_dispatchable(&EventKind::Other));
    }

    #[test]
    fn watcher_indexes_new_file_via_provider() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        set_debounce_ms(&handle, 50);

        let mock = Arc::new(MockProvider::new());
        mock.set(
            "src/lib.rs",
            MockResponse::Ok(sample_extracted("src/lib.rs", "crate::foo")),
        );
        let provider: Arc<dyn LanguageProvider> = mock;
        let filters = fresh_filters(&handle);

        // Pre-create src/ before spawning the watcher so inotify (Linux) watches
        // it from the start. Creating the directory after spawn races with
        // inotify adding the recursive watch for the new subdirectory.
        let src_dir = handle.workspace_root().join("src");
        std::fs::create_dir_all(&src_dir).unwrap();

        let _watcher = spawn_watcher(handle.clone(), provider, filters).unwrap();

        std::fs::write(src_dir.join("lib.rs"), b"fn foo() {}").unwrap();

        wait_revision_at_least(&handle, 1, Duration::from_secs(15));

        let conn = handle.pool().unwrap().get().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn watcher_records_parse_error_through_command() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        set_debounce_ms(&handle, 50);

        let mock = Arc::new(MockProvider::new());
        mock.set(
            "src/bad.rs",
            MockResponse::ParseError("unexpected token".into()),
        );
        let provider: Arc<dyn LanguageProvider> = mock;
        let filters = fresh_filters(&handle);

        let src_dir = handle.workspace_root().join("src");
        std::fs::create_dir_all(&src_dir).unwrap();

        let _watcher = spawn_watcher(handle.clone(), provider, filters).unwrap();

        std::fs::write(src_dir.join("bad.rs"), b"fn ???").unwrap();

        wait_revision_at_least(&handle, 1, Duration::from_secs(15));

        let conn = handle.pool().unwrap().get().unwrap();
        let last_error: Option<String> = conn
            .query_row(
                "SELECT last_scan_error FROM files WHERE path = 'src/bad.rs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(last_error.as_deref(), Some("unexpected token"));
    }

    #[test]
    fn watcher_deletes_file_on_remove_event() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        set_debounce_ms(&handle, 50);

        let mock = Arc::new(MockProvider::new());
        mock.set(
            "src/lib.rs",
            MockResponse::Ok(sample_extracted("src/lib.rs", "crate::foo")),
        );
        let provider: Arc<dyn LanguageProvider> = mock;
        let filters = fresh_filters(&handle);

        let src_dir = handle.workspace_root().join("src");
        std::fs::create_dir_all(&src_dir).unwrap();

        let _watcher = spawn_watcher(handle.clone(), provider, filters).unwrap();

        let file_path = src_dir.join("lib.rs");
        std::fs::write(&file_path, b"fn foo() {}").unwrap();

        wait_revision_at_least(&handle, 1, Duration::from_secs(15));
        let revision_after_create = handle.revision();

        std::fs::remove_file(&file_path).unwrap();
        wait_revision_at_least(&handle, revision_after_create + 1, Duration::from_secs(15));

        let conn = handle.pool().unwrap().get().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE path = 'src/lib.rs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn watcher_skips_filtered_subtree() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join(".stdignore"),
            "target/\nvendored/\n.git/\nnode_modules/\n",
        )
        .unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        set_debounce_ms(&handle, 50);

        let mock = Arc::new(MockProvider::new());
        let provider: Arc<dyn LanguageProvider> = mock;
        let filters = fresh_filters(&handle);

        let _watcher = spawn_watcher(handle.clone(), provider, filters).unwrap();

        let vendored = handle.workspace_root().join("vendored");
        std::fs::create_dir_all(&vendored).unwrap();
        std::fs::write(vendored.join("noise.rs"), b"fn noise() {}").unwrap();

        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            handle.revision(),
            0,
            "events on filtered paths must never bump the revision"
        );
    }

    #[test]
    fn watcher_ignores_unsupported_extensions() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        set_debounce_ms(&handle, 50);

        let mock = Arc::new(MockProvider::new());
        let provider: Arc<dyn LanguageProvider> = mock;
        let filters = fresh_filters(&handle);

        let _watcher = spawn_watcher(handle.clone(), provider, filters).unwrap();

        std::fs::write(handle.workspace_root().join("notes.txt"), b"hello").unwrap();

        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            handle.revision(),
            0,
            "unsupported extensions must never bump the revision"
        );
    }

    #[test]
    fn watcher_skips_paused_events() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        set_debounce_ms(&handle, 50);

        let mock = Arc::new(MockProvider::new());
        mock.set(
            "src/lib.rs",
            MockResponse::Ok(sample_extracted("src/lib.rs", "crate::foo")),
        );
        let provider: Arc<dyn LanguageProvider> = mock;
        let filters = fresh_filters(&handle);

        let _watcher = spawn_watcher(handle.clone(), provider, filters).unwrap();

        handle.pause();
        let src_dir = handle.workspace_root().join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::write(src_dir.join("lib.rs"), b"fn foo() {}").unwrap();

        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(
            handle.revision(),
            0,
            "paused watcher must not dispatch events"
        );
    }

    #[test]
    fn watcher_warns_when_pattern_added_marks_existing_rows() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        set_debounce_ms(&handle, 50);

        // Seed an indexed file in `vendored/` while no such pattern exists.
        let conn = handle.pool().unwrap().get().unwrap();
        conn.execute(
            "INSERT INTO files (path, content_hash, language, last_scanned, byte_size) \
             VALUES ('vendored/keep.rs', 'aa', 'rust', 0, 0)",
            [],
        )
        .unwrap();
        drop(conn);

        let mock = Arc::new(MockProvider::new());
        let provider: Arc<dyn LanguageProvider> = mock;
        let filters = fresh_filters(&handle);
        let filters_clone = Arc::clone(&filters);

        let _watcher = spawn_watcher(handle.clone(), provider, filters).unwrap();

        // Append a pattern that now excludes `vendored/`.
        let stdignore_path = handle.workspace_root().join(".stdignore");
        let mut body = std::fs::read_to_string(&stdignore_path).unwrap();
        body.push_str("\nvendored/\n");
        std::fs::write(&stdignore_path, body).unwrap();

        // Wait for the swap to land.
        let start = Instant::now();
        loop {
            let guard = filters_clone.read().unwrap();
            if guard.is_skipped("vendored/keep.rs") {
                break;
            }
            drop(guard);
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "filters were never reloaded after .stdignore change"
            );
            std::thread::sleep(Duration::from_millis(20));
        }

        // Row must remain in DB until the user purges (lock 21 Q5 add path).
        let conn = handle.pool().unwrap().get().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE path = 'vendored/keep.rs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "newly-excluded rows are not auto-purged (Q5 add path)"
        );
    }

    #[test]
    fn watcher_dispatches_peer_event_with_workspace_id() {
        // L3d-2: a file change inside an `add_peer`'d root produces a
        // scoped `files.path` row + a symbol row tagged with the peer's
        // workspace_id, NOT the 'primary' default.
        let primary_dir = tempdir().unwrap();
        let peer_dir = tempdir().unwrap();
        let handle = IndexHandle::open(primary_dir.path()).unwrap();
        set_debounce_ms(&handle, 50);

        let mock = Arc::new(MockProvider::new());
        mock.set(
            "src/peer.rs",
            MockResponse::Ok(sample_extracted("src/peer.rs", "peer::foo")),
        );
        let provider: Arc<dyn LanguageProvider> = mock;
        let filters = fresh_filters(&handle);

        let peer_src = peer_dir.path().join("src");
        std::fs::create_dir_all(&peer_src).unwrap();

        let mut watcher = spawn_watcher(handle.clone(), provider, filters).unwrap();
        watcher
            .add_peer("peer-aaa".into(), peer_dir.path())
            .unwrap();

        std::fs::write(peer_src.join("peer.rs"), b"fn foo() {}").unwrap();
        wait_revision_at_least(&handle, 1, Duration::from_secs(15));

        let conn = handle.pool().unwrap().get().unwrap();
        let file_path: String = conn
            .query_row(
                "SELECT path FROM files WHERE path LIKE 'ws:peer-aaa:%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(file_path, "ws:peer-aaa:src/peer.rs");

        let sym_ws_id: String = conn
            .query_row(
                "SELECT workspace_id FROM symbols WHERE fqdn = 'peer::foo'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sym_ws_id, "peer-aaa");
    }

    #[test]
    fn watcher_remove_peer_stops_dispatching_subsequent_events() {
        // L3d-2: after `remove_peer`, a subsequent change in the unwatched
        // peer root must NOT bump the revision. Use a sleep-then-check
        // pattern rather than `wait_revision_at_least` to assert the
        // negative outcome.
        let primary_dir = tempdir().unwrap();
        let peer_dir = tempdir().unwrap();
        let handle = IndexHandle::open(primary_dir.path()).unwrap();
        set_debounce_ms(&handle, 50);

        let mock = Arc::new(MockProvider::new());
        mock.set(
            "src/peer.rs",
            MockResponse::Ok(sample_extracted("src/peer.rs", "peer::foo")),
        );
        let provider: Arc<dyn LanguageProvider> = mock;
        let filters = fresh_filters(&handle);

        let peer_src = peer_dir.path().join("src");
        std::fs::create_dir_all(&peer_src).unwrap();

        let mut watcher = spawn_watcher(handle.clone(), provider, filters).unwrap();
        watcher
            .add_peer("peer-bbb".into(), peer_dir.path())
            .unwrap();
        watcher.remove_peer("peer-bbb").unwrap();
        assert!(watcher.peers_snapshot().is_empty());

        std::fs::write(peer_src.join("peer.rs"), b"fn foo() {}").unwrap();

        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(
            handle.revision(),
            0,
            "removed peer's events must not bump the revision"
        );
    }

    #[test]
    fn watcher_add_peer_is_idempotent() {
        // L3d-2: re-adding the same workspace_id must not double-register
        // and must not error. The peer snapshot stays at length 1.
        let primary_dir = tempdir().unwrap();
        let peer_dir = tempdir().unwrap();
        let handle = IndexHandle::open(primary_dir.path()).unwrap();
        set_debounce_ms(&handle, 50);

        let mock = Arc::new(MockProvider::new());
        let provider: Arc<dyn LanguageProvider> = mock;
        let filters = fresh_filters(&handle);

        let mut watcher = spawn_watcher(handle.clone(), provider, filters).unwrap();
        watcher
            .add_peer("peer-ccc".into(), peer_dir.path())
            .unwrap();
        watcher
            .add_peer("peer-ccc".into(), peer_dir.path())
            .unwrap();
        assert_eq!(watcher.peers_snapshot().len(), 1);
    }

    #[test]
    fn watcher_routes_primary_and_peer_events_concurrently() {
        // L3d-2: with both a primary file change AND a peer file change
        // submitted, both result in their respective scoped rows. Guards
        // against routing regressions where every event collapses to
        // primary or peer.
        let primary_dir = tempdir().unwrap();
        let peer_dir = tempdir().unwrap();
        let handle = IndexHandle::open(primary_dir.path()).unwrap();
        set_debounce_ms(&handle, 50);

        let mock = Arc::new(MockProvider::new());
        mock.set(
            "src/lib.rs",
            MockResponse::Ok(sample_extracted("src/lib.rs", "crate::main")),
        );
        mock.set(
            "src/lib.rs",
            MockResponse::Ok(sample_extracted("src/lib.rs", "crate::main")),
        );
        let provider: Arc<dyn LanguageProvider> = mock;
        let filters = fresh_filters(&handle);

        let primary_src = primary_dir.path().join("src");
        let peer_src = peer_dir.path().join("src");
        std::fs::create_dir_all(&primary_src).unwrap();
        std::fs::create_dir_all(&peer_src).unwrap();

        let mut watcher = spawn_watcher(handle.clone(), provider, filters).unwrap();
        watcher
            .add_peer("peer-ddd".into(), peer_dir.path())
            .unwrap();

        std::fs::write(primary_src.join("lib.rs"), b"fn main() {}").unwrap();
        std::fs::write(peer_src.join("lib.rs"), b"fn main() {}").unwrap();

        wait_revision_at_least(&handle, 2, Duration::from_secs(15));

        let conn = handle.pool().unwrap().get().unwrap();
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 2, "primary + peer rows must coexist");

        let primary_present: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE path = 'src/lib.rs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(primary_present, 1);

        let peer_present: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE path = 'ws:peer-ddd:src/lib.rs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(peer_present, 1);
    }

    #[test]
    fn watcher_reindexes_subtree_when_pattern_removed() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join(".stdignore"),
            "target/\nvendored/\n.git/\nnode_modules/\n",
        )
        .unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        set_debounce_ms(&handle, 50);

        // Drop a file in the (currently) excluded subtree.
        let vendored_dir = handle.workspace_root().join("vendored");
        std::fs::create_dir_all(&vendored_dir).unwrap();
        std::fs::write(vendored_dir.join("lib.rs"), b"fn vendored_fn() {}").unwrap();

        let mock = Arc::new(MockProvider::new());
        mock.set(
            "vendored/lib.rs",
            MockResponse::Ok(sample_extracted("vendored/lib.rs", "vendored::vendored_fn")),
        );
        let provider: Arc<dyn LanguageProvider> = mock;
        let filters = fresh_filters(&handle);

        let _watcher = spawn_watcher(handle.clone(), provider, filters).unwrap();

        // Confirm the file is NOT indexed yet.
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(handle.revision(), 0);

        // Remove the `vendored/` pattern.
        std::fs::write(
            handle.workspace_root().join(".stdignore"),
            "target/\n.git/\nnode_modules/\n",
        )
        .unwrap();

        wait_revision_at_least(&handle, 1, Duration::from_secs(15));

        let conn = handle.pool().unwrap().get().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE path = 'vendored/lib.rs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "newly-allowed file must be re-indexed");
    }
}

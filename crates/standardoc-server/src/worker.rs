//! Worker thread consuming `Watcher` batches and updating index.
//!
//! Behavior:
//! - **Coalesce**: drain all queued batches before rescan to avoid repeated
//!   rescans on bursts (git checkout, save-all).
//! - **Pause-aware**: if `watch_paused` is `true`, drain and ignore.
//! - **Incremental**: one rescan per changed file via `scan_and_extract_file`,
//!   pas un full rescan — sauf `ConfigChanged` qui force un rescan complet.
//! - **Parse-error preservation**: files that fail parsing keep previous
//!   blocks in index. No temporary gaps while editing.
//! - **Atomicity**: all mutations in a cycle happen under a single write lock
//!   -> MCP clients see either before or after state, never intermediate.
//! - **Auto-pause heuristic**: if one file repeatedly fails parsing in a short
//!   window, watcher is paused and logged.
//!
//! `needless_pass_by_value` is silenced module-wide: `Arc<...>` and
//! `Receiver` are intentionally passed by value to the worker thread that
//! owns them for its full lifetime.

#![allow(clippy::needless_pass_by_value)]

use crate::state::{build_registry_with_workspace, IndexState, SharedStdout};
use serde_json::json;
use standardoc_core::model::{Diagnostic, DocBlock};
use standardoc_core::pipeline::{scan_and_extract, scan_and_extract_file, FileScanOutcome};
use standardoc_core::validator::validate;
use standardoc_core::watcher::WatcherEvent;
use standardoc_web::state::IndexEvent;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// Worker-side `recv_timeout` timeout. Trade-off: shorter = faster shutdown,
/// longer = fewer CPU wakeups. 500ms is a good compromise.
const RECV_POLL: Duration = Duration::from_millis(500);

/// All parameters required by worker. Wrapper to avoid an 8-arg `spawn`.
pub(crate) struct WorkerConfig {
    pub workspace_root: PathBuf,
    pub index: Arc<RwLock<IndexState>>,
    pub revision: Arc<AtomicU64>,
    pub watch_paused: Arc<AtomicBool>,
    pub shutdown: Arc<AtomicBool>,
    pub rx: Receiver<Vec<WatcherEvent>>,
    /// Number of parse errors on same file within window before auto-pause.
    /// `0` disables auto-pause.
    pub auto_pause_parse_errors: u32,
    pub auto_pause_window: Duration,
    /// Stdout shared with MCP dispatcher. Used to push
    /// `notifications/standardoc/index_changed` when index changes without
    /// extra client request. `None` in web mode (no stdio JSON-RPC).
    pub stdout: Option<SharedStdout>,
    /// Broadcast channel for SSE events. Always present — without subscribers,
    /// `send` is a silent no-op.
    pub events: broadcast::Sender<IndexEvent>,
}

pub(crate) fn spawn(cfg: WorkerConfig) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("standardoc-watcher-worker".to_owned())
        .spawn(move || {
            run(cfg);
        })
        .expect("failed to spawn watcher worker thread")
}

fn run(cfg: WorkerConfig) {
    let WorkerConfig {
        workspace_root,
        index,
        revision,
        watch_paused,
        shutdown,
        rx,
        auto_pause_parse_errors,
        auto_pause_window,
        stdout,
        events,
    } = cfg;

    let registry = build_registry_with_workspace(Some(&workspace_root));
    let mut parse_errors: HashMap<PathBuf, Vec<Instant>> = HashMap::new();

    // Per-path diagnostics cache, keyed by relative path. JSON-encoded so a
    // simple string equality detects any change (severity, message, range,
    // …). Seeded at boot from current state so the first watcher cycle emits
    // only real deltas, not the full diagnostic set.
    let mut prev_diag_json: HashMap<PathBuf, String> = seed_diag_cache(&workspace_root, &index);

    while !shutdown.load(Ordering::Acquire) {
        let Ok(first_batch) = rx.recv_timeout(RECV_POLL) else {
            match rx.recv_timeout(Duration::from_millis(0)) {
                Err(RecvTimeoutError::Disconnected) => break,
                _ => continue,
            }
        };

        if watch_paused.load(Ordering::Acquire) {
            drain(&rx);
            continue;
        }

        let mut paths: HashSet<PathBuf> = HashSet::new();
        let mut config_changed = false;
        extract_into(first_batch, &mut paths, &mut config_changed);
        while let Ok(next) = rx.try_recv() {
            extract_into(next, &mut paths, &mut config_changed);
        }

        if config_changed {
            let before_keys = snapshot_keys(&index);
            if let Err(err) = do_full_rescan(&workspace_root, &registry, &index, &revision) {
                eprintln!("watcher-worker: full rescan failed: {err}");
                continue;
            }
            let after_keys = snapshot_keys(&index);
            push_index_changed(
                stdout.as_ref(),
                &events,
                &revision,
                &before_keys,
                &after_keys,
            );
            push_config_reloaded(stdout.as_ref(), &workspace_root);
            push_diagnostics_changes(
                stdout.as_ref(),
                &mut prev_diag_json,
                &compute_diagnostics_grouped(&workspace_root, &index),
            );
            parse_errors.clear();
            continue;
        }

        let before_keys = snapshot_keys(&index);
        let changed = do_incremental_rescan(
            &paths,
            &workspace_root,
            &registry,
            &index,
            &revision,
            &mut parse_errors,
            &watch_paused,
            auto_pause_parse_errors,
            auto_pause_window,
        );
        if changed {
            let after_keys = snapshot_keys(&index);
            push_index_changed(
                stdout.as_ref(),
                &events,
                &revision,
                &before_keys,
                &after_keys,
            );
            push_diagnostics_changes(
                stdout.as_ref(),
                &mut prev_diag_json,
                &compute_diagnostics_grouped(&workspace_root, &index),
            );
        }
    }
}

/// Seed the diagnostics cache once at worker boot. Without this, the first
/// real watcher event would push notifications for every diagnostic in the
/// workspace as if they had just appeared.
fn seed_diag_cache(
    workspace_root: &Path,
    index: &Arc<RwLock<IndexState>>,
) -> HashMap<PathBuf, String> {
    let grouped = compute_diagnostics_grouped(workspace_root, index);
    let mut cache = HashMap::new();
    for (path, diags) in grouped {
        if let Ok(json_str) = serde_json::to_string(&diags) {
            cache.insert(path, json_str);
        }
    }
    cache
}

/// Run the validator against current index state, group diagnostics by their
/// source path (relative to workspace). Re-loads config from disk so a live
/// `.standardoc.json` edit takes effect on the next cycle.
fn compute_diagnostics_grouped(
    workspace_root: &Path,
    index: &Arc<RwLock<IndexState>>,
) -> BTreeMap<PathBuf, Vec<Diagnostic>> {
    let config = standardoc_core::config::Config::load_from_workspace_or_default(workspace_root);
    // Scoped read lock: validate borrows the index, but we drop the guard
    // before the grouping loop so MCP/LSP readers aren't blocked needlessly.
    let diags = {
        let guard = index
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        validate(&guard.blocks, &guard.collisions, &guard.pages, &config)
    };
    let mut grouped: BTreeMap<PathBuf, Vec<Diagnostic>> = BTreeMap::new();
    for d in diags {
        grouped.entry(d.path.clone()).or_default().push(d);
    }
    grouped
}

/// Push `notifications/standardoc/diagnostics` for every path whose
/// diagnostic set changed since the previous cycle. Also emits an empty list
/// for paths that had diagnostics last time but are now clean — clients can
/// clear their per-file UI without a separate "cleared" message.
///
/// Comparison uses JSON-encoded strings so any change in severity, code,
/// message, range, or related info is detected without per-field equality.
fn push_diagnostics_changes(
    stdout: Option<&SharedStdout>,
    prev_json: &mut HashMap<PathBuf, String>,
    grouped: &BTreeMap<PathBuf, Vec<Diagnostic>>,
) {
    let Some(stdout) = stdout else {
        return;
    };

    let mut current_paths: HashSet<PathBuf> = HashSet::new();
    for (path, diags) in grouped {
        current_paths.insert(path.clone());
        let new_json = serde_json::to_string(diags).unwrap_or_default();
        if prev_json.get(path).map(String::as_str) == Some(new_json.as_str()) {
            continue;
        }
        emit_diagnostics_notification(stdout, path, diags);
        prev_json.insert(path.clone(), new_json);
    }

    let cleared: Vec<PathBuf> = prev_json
        .keys()
        .filter(|p| !current_paths.contains(*p))
        .cloned()
        .collect();
    for path in cleared {
        emit_diagnostics_notification(stdout, &path, &[]);
        prev_json.remove(&path);
    }
}

fn emit_diagnostics_notification(stdout: &SharedStdout, path: &Path, diags: &[Diagnostic]) {
    let msg = json!({
        "jsonrpc": "2.0",
        "method": "notifications/standardoc/diagnostics",
        "params": {
            "path": path.to_string_lossy(),
            "diagnostics": diags,
        }
    });
    if let Ok(line) = serde_json::to_string(&msg) {
        if let Ok(mut w) = stdout.lock() {
            let _ = writeln!(w, "{line}");
            let _ = w.flush();
        }
    }
}

/// Push `notifications/standardoc/config_reloaded` after the worker detects a
/// `.standardoc.json` change and runs the full rescan that applies the new
/// settings. Re-reads config from disk so the payload reflects the version
/// the worker just used (not whatever stale copy `ServerState::config` may
/// hold — that one is set at boot).
fn push_config_reloaded(stdout: Option<&SharedStdout>, workspace_root: &Path) {
    let Some(stdout) = stdout else {
        return;
    };
    let config = standardoc_core::config::Config::load_from_workspace_or_default(workspace_root);
    let Ok(config_value) = serde_json::to_value(&config) else {
        return;
    };
    let msg = json!({
        "jsonrpc": "2.0",
        "method": "notifications/standardoc/config_reloaded",
        "params": {
            "config": config_value,
        }
    });
    if let Ok(line) = serde_json::to_string(&msg) {
        if let Ok(mut w) = stdout.lock() {
            let _ = writeln!(w, "{line}");
            let _ = w.flush();
        }
    }
}

fn snapshot_keys(index: &Arc<RwLock<IndexState>>) -> HashSet<String> {
    let guard = index
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.blocks.keys().cloned().collect()
}

/// Push `notifications/standardoc/index_changed` on available channels:
/// stdout (MCP JSON-RPC) and/or broadcast (web SSE).
///
/// Same payload on both sides: `{ revision, added, removed }`. Clients can
/// refetch details via `get_doc` as needed — no full blocks in notifications
/// to avoid stream bloat.
fn push_index_changed(
    stdout: Option<&SharedStdout>,
    events: &broadcast::Sender<IndexEvent>,
    revision: &Arc<AtomicU64>,
    before: &HashSet<String>,
    after: &HashSet<String>,
) {
    let added: Vec<&String> = after.difference(before).collect();
    let removed: Vec<&String> = before.difference(after).collect();
    let rev = revision.load(Ordering::Acquire);

    if let Some(stdout) = stdout {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": "notifications/standardoc/index_changed",
            "params": {
                "revision": rev,
                "added": added,
                "removed": removed,
            }
        });
        if let Ok(line) = serde_json::to_string(&msg) {
            if let Ok(mut w) = stdout.lock() {
                let _ = writeln!(w, "{line}");
                let _ = w.flush();
            }
        }
    }

    // `send` returns `Err` when there is no receiver — ignored, expected in
    // MCP-only mode.
    let _ = events.send(IndexEvent::IndexChanged { revision: rev });
}

fn drain(rx: &Receiver<Vec<WatcherEvent>>) {
    while rx.try_recv().is_ok() {}
}

fn extract_into(batch: Vec<WatcherEvent>, paths: &mut HashSet<PathBuf>, config_changed: &mut bool) {
    for ev in batch {
        match ev {
            WatcherEvent::Created(p) | WatcherEvent::Modified(p) | WatcherEvent::Removed(p) => {
                paths.insert(p);
            }
            WatcherEvent::Renamed { from, to } => {
                paths.insert(from);
                paths.insert(to);
            }
            WatcherEvent::ConfigChanged => {
                *config_changed = true;
            }
        }
    }
}

fn do_full_rescan(
    workspace_root: &Path,
    registry: &standardoc_core::scanner::Registry,
    index: &Arc<RwLock<IndexState>>,
    revision: &Arc<AtomicU64>,
) -> Result<(), std::io::Error> {
    // Reload from disk so live `.standardoc.json` edits (e.g. admin exclusion
    // patterns) are applied without server restart. Watcher already emits
    // `config_changed` to trigger full rescan — this is where new config
    // actually takes effect.
    let config = standardoc_core::config::Config::load_from_workspace_or_default(workspace_root);
    let report = scan_and_extract(workspace_root, registry, &config)?;
    let key_locations = crate::state::build_key_locations(&report.blocks);
    let incoming = crate::state::build_incoming_index(&report.blocks);
    let new_state = IndexState {
        blocks: report.blocks,
        pages: report.pages,
        collisions: report.collisions,
        error_count: report.errors.len(),
        key_locations,
        incoming,
    };
    {
        let mut guard = index
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = new_state;
    }
    revision.fetch_add(1, Ordering::Release);
    Ok(())
}

/// Returns `true` if index was mutated during this cycle, so caller can decide
/// whether it should push a notification.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn do_incremental_rescan(
    paths: &HashSet<PathBuf>,
    workspace_root: &Path,
    registry: &standardoc_core::scanner::Registry,
    index: &Arc<RwLock<IndexState>>,
    revision: &Arc<AtomicU64>,
    parse_errors: &mut HashMap<PathBuf, Vec<Instant>>,
    watch_paused: &Arc<AtomicBool>,
    auto_pause_parse_errors: u32,
    auto_pause_window: Duration,
) -> bool {
    let config = standardoc_core::config::Config::load_from_workspace_or_default(workspace_root);

    // Phase 0: detect changes under `.standardoc/pages/`. If a .md/.mdx moved
    // there, rescan all pages (cheap, usually ~10 files) without touching
    // blocks. Must run **before** `registry.resolve` filter that drops
    // non-code files.
    let pages_changed = paths.iter().any(|p| is_under_pages_dir(p, workspace_root));

    // Phase 1 (without lock): scan each path individually and classify.
    let mut new_blocks_per_path: HashMap<PathBuf, Vec<DocBlock>> = HashMap::new();
    let mut successfully_rescanned: HashSet<PathBuf> = HashSet::new();
    let mut removed_paths: HashSet<PathBuf> = HashSet::new();
    let mut failed_paths: HashSet<PathBuf> = HashSet::new();

    for p in paths {
        if registry.resolve(p).is_none() {
            continue;
        }
        if !p.exists() {
            removed_paths.insert(p.clone());
            continue;
        }
        match scan_and_extract_file(p, workspace_root, registry, &config) {
            FileScanOutcome::Ok(blocks) => {
                successfully_rescanned.insert(p.clone());
                new_blocks_per_path.insert(p.clone(), blocks);
                parse_errors.remove(p);
            }
            FileScanOutcome::ParseError(_) | FileScanOutcome::IoError(_) => {
                failed_paths.insert(p.clone());
                record_parse_error(parse_errors, p.clone(), auto_pause_window);
            }
            FileScanOutcome::NoProvider => {}
        }
    }

    // Auto-pause heuristic (if enabled by config).
    if auto_pause_parse_errors > 0
        && should_auto_pause(parse_errors, auto_pause_parse_errors, auto_pause_window)
    {
        watch_paused.store(true, Ordering::Release);
        eprintln!(
            "watcher-worker: auto-paused after repeated parse errors — resume via \
             the MCP tool `set_watch_paused` with false once you're done editing"
        );
        parse_errors.clear();
        return false;
    }

    let to_purge_from_index: HashSet<PathBuf> = successfully_rescanned
        .iter()
        .chain(removed_paths.iter())
        .map(|p| relative_to_workspace(p, workspace_root))
        .collect();

    if to_purge_from_index.is_empty() && new_blocks_per_path.is_empty() && !pages_changed {
        return false;
    }

    // If only pages changed, patch only `IndexState.pages` and bump revision.
    // Otherwise continue with block rebuild below and include page rescan in
    // same write transaction.
    if pages_changed {
        let new_pages = standardoc_core::pages::scan_pages(workspace_root);
        let mut guard = index
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.pages = new_pages;
        if to_purge_from_index.is_empty() && new_blocks_per_path.is_empty() {
            drop(guard);
            revision.fetch_add(1, Ordering::Release);
            return true;
        }
    }

    // Phase 2 (under write lock): atomic mutations + collision recompute
    // + maintenance of reverse `incoming` index.
    {
        let mut guard = index
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Identify keys to remove/replace: all blocks whose path is in
        // `to_purge_from_index`. Needed to scrub reverse `incoming` index.
        let purged_keys: HashSet<String> = guard
            .blocks
            .iter()
            .filter(|(_, b)| to_purge_from_index.contains(&b.meta.path))
            .map(|(k, _)| k.clone())
            .collect();

        // Retirer ces blocs.
        guard
            .blocks
            .retain(|_, block| !to_purge_from_index.contains(&block.meta.path));

        // Purge `key_locations` entries for same paths.
        for entries in guard.key_locations.values_mut() {
            entries.retain(|loc| !to_purge_from_index.contains(&loc.path));
        }
        guard.key_locations.retain(|_, v| !v.is_empty());

        // Purge reverse `incoming` index entries pointing **from** removed keys
        // (`from_key` values that no longer exist).
        for entries in guard.incoming.values_mut() {
            entries.retain(|inc| !purged_keys.contains(&inc.from_key));
        }
        guard.incoming.retain(|_, v| !v.is_empty());

        // Insert new blocks + track their locations + feed incoming index.
        for blocks in new_blocks_per_path.into_values() {
            for block in blocks {
                let key = block.key.as_str().to_owned();
                guard.key_locations.entry(key.clone()).or_default().push(
                    standardoc_core::pipeline::PathLine {
                        path: block.meta.path.clone(),
                        line: block.meta.line_start,
                    },
                );
                if let Some(symbol) = &block.symbol {
                    for sref in &symbol.references.outgoing {
                        guard.incoming.entry(sref.target.clone()).or_default().push(
                            standardoc_core::model::IncomingRef {
                                from_key: key.clone(),
                                kind: sref.kind,
                                line: sref.line,
                            },
                        );
                    }
                }
                guard.blocks.insert(key, block);
            }
        }

        // Recompute collisions from updated `key_locations`.
        guard.collisions = compute_collisions(&guard.key_locations);
        guard.error_count = failed_paths.len();
    }

    revision.fetch_add(1, Ordering::Release);
    true
}

fn compute_collisions(
    locations: &BTreeMap<String, Vec<standardoc_core::pipeline::PathLine>>,
) -> Vec<standardoc_core::pipeline::KeyCollision> {
    locations
        .iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|(key, v)| {
            // Last insertion = winner (consistent with `BTreeMap::insert`).
            let mut v = v.clone();
            let kept = v.pop().expect("filtered to len > 1");
            standardoc_core::pipeline::KeyCollision {
                key: key.clone(),
                kept,
                dropped: v,
            }
        })
        .collect()
}

/// `true` if `abs` is under `<workspace_root>/.standardoc/pages/`.
/// Check is best effort on path-strip — if relative derivation fails
/// (e.g. abs on another Windows drive), return false.
fn is_under_pages_dir(abs: &Path, workspace_root: &Path) -> bool {
    let Ok(rel) = abs.strip_prefix(workspace_root) else {
        return false;
    };
    let mut comps = rel.components();
    matches!(
        (
            comps.next().and_then(component_str),
            comps.next().and_then(component_str)
        ),
        (Some(".standardoc"), Some("pages"))
    )
}

fn component_str(c: std::path::Component<'_>) -> Option<&str> {
    match c {
        std::path::Component::Normal(s) => s.to_str(),
        _ => None,
    }
}

fn relative_to_workspace(abs: &Path, workspace_root: &Path) -> PathBuf {
    let rel = abs
        .strip_prefix(workspace_root)
        .map_or_else(|_| abs.to_path_buf(), Path::to_path_buf);
    let normalized: String = rel
        .to_string_lossy()
        .chars()
        .map(|c| if c == '\\' { '/' } else { c })
        .collect();
    PathBuf::from(normalized)
}

fn record_parse_error(errs: &mut HashMap<PathBuf, Vec<Instant>>, path: PathBuf, window: Duration) {
    let now = Instant::now();
    let entry = errs.entry(path).or_default();
    entry.retain(|t| now.duration_since(*t) <= window);
    entry.push(now);
}

fn should_auto_pause(
    errs: &HashMap<PathBuf, Vec<Instant>>,
    threshold: u32,
    window: Duration,
) -> bool {
    let cutoff = Instant::now()
        .checked_sub(window)
        .unwrap_or_else(Instant::now);
    errs.values()
        .any(|times| times.iter().filter(|t| **t >= cutoff).count() >= threshold as usize)
}

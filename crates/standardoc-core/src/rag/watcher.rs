//! Standalone `notify`-backed watcher for `*.md` prose files. Mirrors
//! the AST `pipeline::watcher` pattern : `notify-debouncer-full` collects
//! file-system events, a dispatch thread filters to markdown, and each
//! changed source is fed back through [`RagPipeline::run_for_source`].
//!
//! Runs as a sibling of the AST watcher — same workspace root, same
//! debounce config — so a single file save can trigger both an AST
//! re-extract and a RAG re-embed. The two watchers are independent
//! (separate `notify` channels, separate dispatch threads) to avoid
//! coupling RAG latency to AST latency or vice-versa.

use std::path::Path;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

use notify::{EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{
    DebounceEventResult, DebouncedEvent, Debouncer, RecommendedCache, new_debouncer,
};

use crate::pipeline::ScanFilters;
use crate::pipeline::WatcherError;
use crate::rag::discovery::{FrontmatterDirective, is_convention_path, read_frontmatter_directive};
use crate::rag::pipeline::RagPipeline;
use crate::storage::handle::IndexHandle;

/// Default debounce window for prose file events. Shorter than the AST
/// 500ms because the cascade chunker + embedder are CPU-bound and small
/// `.md` files are cheap to re-process ; debouncing further would only
/// add lag without saving meaningful work.
const RAG_DEBOUNCE_MS: u64 = 250;

/// Field order matters : `debouncer` MUST drop before `dispatch_thread`
/// so the channel closes and the loop exits.
pub struct RagWatcherHandle {
    debouncer: Option<Debouncer<RecommendedWatcher, RecommendedCache>>,
    dispatch_thread: Option<JoinHandle<()>>,
}

impl Drop for RagWatcherHandle {
    fn drop(&mut self) {
        self.debouncer = None;
        if let Some(t) = self.dispatch_thread.take() {
            let _ = t.join();
        }
    }
}

/// Boots a watcher dedicated to `*.md` prose events. The same
/// `ScanFilters` instance is shared with the AST watcher — `.stdignore`
/// hot-reloads benefit both sides.
pub fn spawn_rag_watcher(
    handle: IndexHandle,
    rag_pipeline: Arc<RagPipeline>,
    filters: Arc<RwLock<ScanFilters>>,
) -> Result<RagWatcherHandle, WatcherError> {
    let workspace_root = handle.workspace_root().to_path_buf();

    let (tx, rx) = channel::<DebounceEventResult>();
    let mut debouncer = new_debouncer(Duration::from_millis(RAG_DEBOUNCE_MS), None, tx)?;
    debouncer
        .watch(&workspace_root, RecursiveMode::Recursive)
        .map_err(WatcherError::Notify)?;

    let thread_root = workspace_root.clone();
    let dispatch_thread = std::thread::Builder::new()
        .name("standardoc-rag-watcher".into())
        .spawn(move || dispatch_loop(&rx, &handle, &rag_pipeline, &thread_root, &filters))?;

    Ok(RagWatcherHandle {
        debouncer: Some(debouncer),
        dispatch_thread: Some(dispatch_thread),
    })
}

fn dispatch_loop(
    rx: &Receiver<DebounceEventResult>,
    handle: &IndexHandle,
    pipeline: &RagPipeline,
    workspace_root: &Path,
    filters: &Arc<RwLock<ScanFilters>>,
) {
    while let Ok(batch) = rx.recv() {
        let Ok(events) = batch else {
            continue;
        };
        for event in events {
            handle_event(&event, handle, pipeline, workspace_root, filters);
        }
    }
}

fn handle_event(
    event: &DebouncedEvent,
    handle: &IndexHandle,
    pipeline: &RagPipeline,
    workspace_root: &Path,
    filters: &Arc<RwLock<ScanFilters>>,
) {
    if !is_relevant_kind(event.event.kind) {
        return;
    }
    for path in &event.event.paths {
        let Some(rel) = to_workspace_relative_md(path, workspace_root) else {
            continue;
        };
        let skipped = filters.read().is_ok_and(|guard| guard.is_skipped(&rel));
        if skipped {
            continue;
        }
        if !path.exists() {
            // Deletion → purge from rag store.
            let _ = pipeline.store().replace_chunks_for_source(&rel, &[], &[]);
            continue;
        }
        if !should_index(path, &rel) {
            continue;
        }
        if let Err(e) = pipeline.run_for_source(workspace_root, &rel, handle) {
            eprintln!("standardoc rag watcher: {rel}: {e}");
        }
    }
}

const fn is_relevant_kind(kind: EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

fn to_workspace_relative_md(path: &Path, workspace_root: &Path) -> Option<String> {
    let rel = path.strip_prefix(workspace_root).ok()?;
    let s = rel.to_string_lossy().replace('\\', "/");
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("md"))
    {
        return None;
    }
    Some(s)
}

fn should_index(path: &Path, rel: &str) -> bool {
    if is_convention_path(rel) {
        // Convention path : opt-out via frontmatter directive only.
        let dir = read_frontmatter_directive(path).unwrap_or(FrontmatterDirective::Absent);
        return !matches!(dir, FrontmatterDirective::Disabled);
    }
    matches!(
        read_frontmatter_directive(path).unwrap_or(FrontmatterDirective::Absent),
        FrontmatterDirective::Rag,
    )
}

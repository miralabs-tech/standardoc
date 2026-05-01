use std::path::Path;
use std::sync::{Arc, RwLock};

use standardoc_core::{
    IndexHandle, LanguageProvider, ScanFilters, WatcherHandle, cold_start, spawn_watcher,
};

use crate::error::ServerError;

/// Field order is load-bearing: `watcher` MUST be dropped before `handle`.
/// Dropping the watcher closes its notify debouncer and joins its dispatch
/// thread; only then does `handle` (the last clone in the running process)
/// drop and trigger the writer thread join via `IndexHandleInner::drop`.
/// Reversing the fields would race watcher commands against writer teardown.
pub struct RunningServer {
    /// Held only for its `Drop`: closes the notify debouncer and joins the
    /// dispatch thread. Reading it is intentionally not exposed.
    #[allow(dead_code)]
    watcher: WatcherHandle,
    handle: IndexHandle,
}

impl RunningServer {
    pub const fn handle(&self) -> &IndexHandle {
        &self.handle
    }
}

/// Full daemon mode: open the workspace, run cold start eagerly to
/// completion, then spawn the live watcher. Blocks the caller for the
/// duration of cold start. Returns a `RunningServer` that must outlive
/// any query the caller intends to issue.
pub fn open_workspace(
    root: &Path,
    provider: Arc<dyn LanguageProvider>,
) -> Result<RunningServer, ServerError> {
    let handle = IndexHandle::open(root)?;
    let filters = Arc::new(RwLock::new(ScanFilters::load(handle.workspace_root())));
    {
        let guard = filters.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        cold_start::run(&handle, provider.as_ref(), &guard)?;
    }
    let watcher = spawn_watcher(handle.clone(), provider, filters)?;
    Ok(RunningServer { watcher, handle })
}

/// One-shot indexing: open + cold start, return the handle, drop = exit.
/// The caller is free to issue read queries before dropping. No watcher.
pub fn index_once(
    root: &Path,
    provider: &dyn LanguageProvider,
) -> Result<IndexHandle, ServerError> {
    let handle = IndexHandle::open(root)?;
    let filters = ScanFilters::load(handle.workspace_root());
    cold_start::run(&handle, provider, &filters)?;
    Ok(handle)
}

/// Drop the existing index DB and re-build it from scratch via cold start.
/// The caller owns the `IndexHandle` and is responsible for dropping it
/// after the rescan completes.
pub fn rescan(handle: &IndexHandle, provider: &dyn LanguageProvider) -> Result<(), ServerError> {
    handle.rescan_from_scratch()?;
    let filters = ScanFilters::load(handle.workspace_root());
    cold_start::run(handle, provider, &filters)?;
    Ok(())
}

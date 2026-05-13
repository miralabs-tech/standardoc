//! Revision-driven re-link watcher. Observes the `IndexHandle.revision`
//! counter (bumped on every AST cold-start commit, watcher batch, and
//! external invalidation) and triggers [`RagPipeline::relink_all`] once
//! the revision stabilises.
//!
//! The relink early-exits at near-zero cost via the
//! `workspace_fqdns_hash` stored in `rag.db`, so polling once every
//! `POLL_INTERVAL` while the workspace sits idle is essentially free
//! (one `SELECT value FROM schema_meta` per tick when stable).
//!
//! Coordination with the AST watcher is deliberately loose : we wait
//! for the revision to be unchanged for one debounce tick before
//! relinking, so an active batch of AST mutations doesn't trigger a
//! relink per intermediate state — only after the batch settles.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::rag::pipeline::RagPipeline;
use crate::storage::handle::IndexHandle;

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const DEBOUNCE_INTERVAL: Duration = Duration::from_secs(2);
const STOP_CHECK_INTERVAL: Duration = Duration::from_millis(250);

/// Handle to the revision-relink watcher thread. Setting `stop` and
/// dropping the handle terminates the loop at the next iteration.
pub struct RevisionRelinkHandle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl RevisionRelinkHandle {
    /// Signals the watcher to exit and joins the thread. Equivalent to
    /// `drop` but with the join surfaced so the caller can wait
    /// synchronously on shutdown.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(h) = self.join.take() {
            let _ = h.join();
        }
    }
}

impl Drop for RevisionRelinkHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(h) = self.join.take() {
            let _ = h.join();
        }
    }
}

/// Spawns the revision-relink watcher thread. The thread polls
/// `handle.revision()` every [`POLL_INTERVAL`] ; on a bump it waits one
/// [`DEBOUNCE_INTERVAL`] to absorb burst mutations, then calls
/// [`RagPipeline::relink_all`]. The internal sleeps yield to the stop
/// flag every [`STOP_CHECK_INTERVAL`] for responsive shutdown.
pub fn spawn_revision_relink_watcher(
    handle: IndexHandle,
    pipeline: Arc<RagPipeline>,
) -> std::io::Result<RevisionRelinkHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = Arc::clone(&stop);
    let join = std::thread::Builder::new()
        .name("standardoc-rag-relink".into())
        .spawn(move || run_loop(&handle, &pipeline, &stop_for_thread))?;
    Ok(RevisionRelinkHandle {
        stop,
        join: Some(join),
    })
}

fn run_loop(handle: &IndexHandle, pipeline: &Arc<RagPipeline>, stop: &Arc<AtomicBool>) {
    let mut last_seen = handle.revision();
    loop {
        if !sleep_responsive(POLL_INTERVAL, stop) {
            return;
        }
        let observed = handle.revision();
        if observed == last_seen {
            continue;
        }
        // Debounce : require the revision to stabilise for one tick
        // before relinking. If it keeps moving we'll come back next
        // poll and try again.
        if !sleep_responsive(DEBOUNCE_INTERVAL, stop) {
            return;
        }
        let settled = handle.revision();
        if settled != observed {
            last_seen = settled;
            continue;
        }
        last_seen = settled;
        if let Err(e) = pipeline.relink_all(handle.workspace_root(), handle) {
            eprintln!("standardoc rag: relink_all (revision watcher) failed: {e}");
        }
    }
}

/// Sleeps for `total` while checking the stop flag every
/// [`STOP_CHECK_INTERVAL`]. Returns `false` when stop was signalled —
/// the caller should exit.
fn sleep_responsive(total: Duration, stop: &Arc<AtomicBool>) -> bool {
    let mut remaining = total;
    while !remaining.is_zero() {
        if stop.load(Ordering::Acquire) {
            return false;
        }
        let slice = remaining.min(STOP_CHECK_INTERVAL);
        std::thread::sleep(slice);
        remaining = remaining.saturating_sub(slice);
    }
    !stop.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_terminates_thread_quickly() {
        // Without a pipeline / handle to exercise, we can still assert
        // the stop flag wires through the sleep loop : a dedicated
        // RevisionRelinkHandle::stop should join under the poll
        // interval.
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop);
        let join = std::thread::spawn(move || {
            for _ in 0..100 {
                if !sleep_responsive(Duration::from_mins(1), &stop_for_thread) {
                    return;
                }
            }
        });
        // Yield a moment so the thread enters its sleep.
        std::thread::sleep(Duration::from_millis(50));
        stop.store(true, Ordering::Release);
        let start = std::time::Instant::now();
        let _ = join.join();
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "stop should be honoured within one STOP_CHECK_INTERVAL — took {:?}",
            start.elapsed()
        );
    }
}

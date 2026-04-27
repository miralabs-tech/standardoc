//! Filesystem watcher that keeps index in sync with workspace.
//!
//! Wraps `notify` + `notify-debouncer-full` to produce **batches** of events
//! aggregated over a debounce window. One batch = one message on consumer
//! `Receiver`, not one event per keypress. A `git checkout` touching 500 files
//! should ideally produce a single batch.
//!
//! This module is an **autonomous building block** — it does not touch the
//! `Index`, update core state, or talk to MCP. Integration (worker thread that
//! rescans per batch and bumps revision) comes separately once concurrency
//! choices on `ServerState` are settled.

use notify::{EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{
    new_debouncer, DebounceEventResult, DebouncedEvent, Debouncer, RecommendedCache,
};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;
use thiserror::Error;

/// Config filename that triggers `ConfigChanged` (full rescan).
pub const CONFIG_FILE: &str = ".standardoc.json";

/// Default debounce when consumer has no preference.
/// 100 ms: short enough to stay reactive, long enough to aggregate
/// `format-on-save` touching multiple files at once.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(100);

/// Event produced by watcher, translating `notify` primitives into something
/// directly actionable by consumer (core pipeline, LSP...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatcherEvent {
    Created(PathBuf),
    Modified(PathBuf),
    Removed(PathBuf),
    /// Detected only when OS correlates from/to reliably (modern Linux/macOS).
    /// On Windows and degraded paths we receive `Removed + Created`.
    Renamed {
        from: PathBuf,
        to: PathBuf,
    },
    /// `.standardoc.json` changed (create / modify / delete).
    /// Consumer must trigger full rescan + config reload.
    ConfigChanged,
}

#[derive(Debug, Error)]
pub enum WatchError {
    #[error("notify error: {0}")]
    Notify(#[from] notify::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Opaque handle keeping watcher alive in memory. Drop = stop watching.
pub struct Watcher {
    _debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
}

impl Watcher {
    /// Start recursive watcher on `root`.
    ///
    /// Returns `(handle, receiver)`. `receiver` emits `Vec<WatcherEvent>` per
    /// debounced batch. Dropping handle stops observation and closes sender side.
    ///
    /// Uses `DEFAULT_DEBOUNCE`; use [`Self::start_with_debounce`] to customize.
    pub fn start(root: &Path) -> Result<(Self, Receiver<Vec<WatcherEvent>>), WatchError> {
        Self::start_with_debounce(root, DEFAULT_DEBOUNCE)
    }

    pub fn start_with_debounce(
        root: &Path,
        debounce: Duration,
    ) -> Result<(Self, Receiver<Vec<WatcherEvent>>), WatchError> {
        let (tx, rx) = channel::<Vec<WatcherEvent>>();

        let mut debouncer =
            new_debouncer(
                debounce,
                None,
                move |result: DebounceEventResult| match result {
                    Ok(events) => {
                        let translated = translate_events(events);
                        if !translated.is_empty() {
                            // Silent drop if receiver is closed — watcher does not
                            // crash when consumer goes away.
                            let _ = tx.send(translated);
                        }
                    }
                    Err(errors) => {
                        // `notify` errors are non-fatal (transient OS I/O, permission
                        // denied on subfolder, etc.). Log to stderr instead of aborting.
                        for err in errors {
                            eprintln!("watcher: notify error: {err}");
                        }
                    }
                },
            )?;

        debouncer.watch(root, RecursiveMode::Recursive)?;

        Ok((
            Self {
                _debouncer: debouncer,
            },
            rx,
        ))
    }
}

fn translate_events(events: Vec<DebouncedEvent>) -> Vec<WatcherEvent> {
    let mut out = Vec::new();
    for ev in events {
        let paths = ev.event.paths.clone();

        if paths.iter().any(|p| is_config_file(p)) {
            // One `ConfigChanged` per batch — no need to push multiple times
            // since consumer will perform full rescan anyway.
            if !out.contains(&WatcherEvent::ConfigChanged) {
                out.push(WatcherEvent::ConfigChanged);
            }
            continue;
        }

        match ev.event.kind {
            EventKind::Create(_) => {
                for p in paths {
                    out.push(WatcherEvent::Created(p));
                }
            }
            EventKind::Modify(modify_kind) => {
                use notify::event::{ModifyKind, RenameMode};
                match modify_kind {
                    ModifyKind::Name(RenameMode::Both) if paths.len() >= 2 => {
                        out.push(WatcherEvent::Renamed {
                            from: paths[0].clone(),
                            to: paths[1].clone(),
                        });
                    }
                    _ => {
                        for p in paths {
                            out.push(WatcherEvent::Modified(p));
                        }
                    }
                }
            }
            EventKind::Remove(_) => {
                for p in paths {
                    out.push(WatcherEvent::Removed(p));
                }
            }
            // Access / Other / Any: ignored.
            _ => {}
        }
    }
    out
}

fn is_config_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| name == CONFIG_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::mpsc::RecvTimeoutError;
    use std::thread;
    use tempfile::tempdir;

    /// Wait for batch with generous timeout (1s). On loaded Windows/macOS CI,
    /// FS events can arrive late.
    fn recv_batch(rx: &Receiver<Vec<WatcherEvent>>) -> Vec<WatcherEvent> {
        let mut collected = Vec::new();
        // Premier event : timeout long
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(batch) => collected.extend(batch),
            Err(RecvTimeoutError::Timeout) => {
                panic!("timeout waiting for initial watcher batch");
            }
            Err(RecvTimeoutError::Disconnected) => {
                panic!("watcher sender closed unexpectedly");
            }
        }
        // Collect extra batches that may arrive right after
        // (imperfect debounce on some platforms).
        while let Ok(batch) = rx.recv_timeout(Duration::from_millis(200)) {
            collected.extend(batch);
        }
        collected
    }

    fn has_kind_for(
        events: &[WatcherEvent],
        path: &Path,
        pred: impl Fn(&WatcherEvent) -> bool,
    ) -> bool {
        events.iter().any(|e| match e {
            WatcherEvent::Created(p) | WatcherEvent::Modified(p) | WatcherEvent::Removed(p) => {
                p == path && pred(e)
            }
            WatcherEvent::Renamed { from, to } => (from == path || to == path) && pred(e),
            WatcherEvent::ConfigChanged => false,
        })
    }

    #[test]
    fn detects_file_creation() {
        let dir = tempdir().unwrap();
        // Canonicalize to resolve symlinks (macOS routes /var → /private/var,
        // and the notify backend reports the canonical path).
        let dir_path = dir.path().canonicalize().unwrap();
        let (_watcher, rx) = Watcher::start(&dir_path).unwrap();
        // Delay for notify backend initialization before modifications.
        thread::sleep(Duration::from_millis(150));

        let file = dir_path.join("hello.rs");
        fs::write(&file, b"fn main() {}").unwrap();

        let events = recv_batch(&rx);
        assert!(
            has_kind_for(&events, &file, |e| matches!(
                e,
                WatcherEvent::Created(_) | WatcherEvent::Modified(_)
            )),
            "expected Created or Modified for {file:?}, got: {events:?}"
        );
    }

    #[test]
    fn detects_file_modification() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path().canonicalize().unwrap();
        let file = dir_path.join("mod.rs");
        fs::write(&file, b"// initial").unwrap();

        let (_watcher, rx) = Watcher::start(&dir_path).unwrap();
        thread::sleep(Duration::from_millis(150));

        fs::write(&file, b"// updated").unwrap();

        let events = recv_batch(&rx);
        assert!(
            has_kind_for(&events, &file, |e| matches!(
                e,
                WatcherEvent::Modified(_) | WatcherEvent::Created(_)
            )),
            "expected Modified for {file:?}, got: {events:?}"
        );
    }

    #[test]
    fn detects_file_removal() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path().canonicalize().unwrap();
        let file = dir_path.join("rm.rs");
        fs::write(&file, b"x").unwrap();

        let (_watcher, rx) = Watcher::start(&dir_path).unwrap();
        thread::sleep(Duration::from_millis(150));

        fs::remove_file(&file).unwrap();

        let events = recv_batch(&rx);
        // Linux (inotify) and Windows (ReadDirectoryChangesW) report Removed
        // reliably; macOS FSEvents sometimes coalesces a remove into Modified
        // inside its batching window. Per-platform predicate keeps Linux and
        // Windows strict so a real regression (no event at all, or wrong path)
        // still fails the test. Drop the cfg once notify upstream guarantees
        // Removed on macOS too.
        #[cfg(target_os = "macos")]
        let acceptable = |e: &WatcherEvent| {
            matches!(e, WatcherEvent::Removed(_) | WatcherEvent::Modified(_))
        };
        #[cfg(not(target_os = "macos"))]
        let acceptable = |e: &WatcherEvent| matches!(e, WatcherEvent::Removed(_));
        assert!(
            has_kind_for(&events, &file, acceptable),
            "expected Removed for {file:?}, got: {events:?}"
        );
    }

    #[test]
    fn config_file_emits_config_changed() {
        let dir = tempdir().unwrap();
        let (_watcher, rx) = Watcher::start(dir.path()).unwrap();
        thread::sleep(Duration::from_millis(150));

        let config = dir.path().join(CONFIG_FILE);
        fs::write(&config, b"{}").unwrap();

        let events = recv_batch(&rx);
        assert!(
            events.contains(&WatcherEvent::ConfigChanged),
            "expected ConfigChanged in batch, got: {events:?}"
        );
    }

    #[test]
    fn drop_closes_receiver() {
        let dir = tempdir().unwrap();
        let (watcher, rx) = Watcher::start(dir.path()).unwrap();
        drop(watcher);
        // Once watcher is dropped, sender is also dropped and `recv` should
        // return `Disconnected` instead of blocking.
        match rx.recv_timeout(Duration::from_secs(2)) {
            Err(RecvTimeoutError::Disconnected) | Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => panic!("receiver did not close after watcher drop"),
        }
    }
}

//! Retry helper for transient storage open failures.
//!
//! Motivating case: on Windows the OS releases SQLite WAL / SHM file
//! handles and fs4 advisory locks AFTER the holding process exits, but
//! the next process trying to open the same database may attempt this
//! before that cleanup has propagated. Result: a fast restart hits a
//! `locking protocol` (`SQLITE_PROTOCOL`, code 15) error or the fs4
//! `LockHeld` variant. The condition clears in well under a second once
//! the kernel finishes releasing handles.
//!
//! `retry_with_backoff` wraps a fallible open closure with exponential
//! backoff for ~6.5 s total, retrying only when the error is in the
//! transient set ([`is_transient`]). Permanent errors (schema too new,
//! corruption, IO-not-found, …) propagate immediately.

use std::thread;
use std::time::Duration;

use crate::storage::error::StorageError;

/// Exponential backoff schedule in milliseconds. Cumulative ~1.55 s.
/// Sized for the Windows lock-release race observed in practice (well
/// under 500 ms in the field) plus headroom. Tighter than a generic
/// network retry on purpose : a longer schedule balloons test
/// run-times on the rare permanent failure that slips past
/// [`is_transient`].
const BACKOFF_SCHEDULE_MS: &[u64] = &[50, 100, 200, 400, 800];

/// Runs `f` with exponential backoff on transient open failures. The
/// closure may run up to `BACKOFF_SCHEDULE_MS.len() + 1` times; the
/// final error is returned verbatim once the schedule is exhausted.
///
/// `f: FnMut()` so callers can rebuild a borrowed lock-file path inside
/// the closure on each attempt (the fs4 file handle is short-lived).
pub(crate) fn retry_with_backoff<T, F>(mut f: F) -> Result<T, StorageError>
where
    F: FnMut() -> Result<T, StorageError>,
{
    let mut idx = 0usize;
    loop {
        match f() {
            Ok(value) => return Ok(value),
            Err(err) => {
                if !is_transient(&err) || idx >= BACKOFF_SCHEDULE_MS.len() {
                    return Err(err);
                }
                let delay = BACKOFF_SCHEDULE_MS[idx];
                idx += 1;
                thread::sleep(Duration::from_millis(delay));
            }
        }
    }
}

/// Returns `true` when `err` reflects a transient open failure that
/// will most likely clear within a few hundred milliseconds — fs4 lock
/// contention from a sibling process, SQLite `locking protocol` /
/// `database is locked` / `database is busy` while a prior connection's
/// WAL handles are still being released by the OS.
pub(crate) fn is_transient(err: &StorageError) -> bool {
    match err {
        StorageError::LockHeld { .. } => true,
        StorageError::Pool(inner) => is_transient_message(&inner.to_string()),
        StorageError::Sqlite(inner) => is_transient_sqlite(inner),
        _ => false,
    }
}

fn is_transient_sqlite(err: &rusqlite::Error) -> bool {
    if let rusqlite::Error::SqliteFailure(code, _) = err {
        if matches!(
            code.code,
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked,
        ) {
            return true;
        }
        // SQLITE_PROTOCOL extended code (15) surfaces on WAL handshake races.
        if code.extended_code == rusqlite::ffi::SQLITE_PROTOCOL {
            return true;
        }
    }
    is_transient_message(&err.to_string())
}

fn is_transient_message(msg: &str) -> bool {
    let lowered = msg.to_ascii_lowercase();
    lowered.contains("locking protocol")
        || lowered.contains("database is locked")
        || lowered.contains("database is busy")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::path::PathBuf;

    #[test]
    fn succeeds_on_first_try_when_no_error() {
        let calls = RefCell::new(0);
        let res: Result<u32, StorageError> = retry_with_backoff(|| {
            *calls.borrow_mut() += 1;
            Ok(42)
        });
        assert_eq!(res.unwrap(), 42);
        assert_eq!(*calls.borrow(), 1);
    }

    #[test]
    fn retries_on_lock_held_then_succeeds() {
        let calls = RefCell::new(0);
        let res: Result<u32, StorageError> = retry_with_backoff(|| {
            *calls.borrow_mut() += 1;
            if *calls.borrow() < 3 {
                Err(StorageError::LockHeld {
                    path: PathBuf::from("/tmp/x"),
                })
            } else {
                Ok(7)
            }
        });
        assert_eq!(res.unwrap(), 7);
        assert_eq!(*calls.borrow(), 3);
    }

    #[test]
    fn does_not_retry_on_permanent_error() {
        let calls = RefCell::new(0);
        let res: Result<u32, StorageError> = retry_with_backoff(|| {
            *calls.borrow_mut() += 1;
            Err(StorageError::SchemaVersionTooNew {
                db: 99,
                supported: 1,
            })
        });
        assert!(matches!(res, Err(StorageError::SchemaVersionTooNew { .. })));
        assert_eq!(*calls.borrow(), 1);
    }

    #[test]
    fn exhausts_schedule_and_returns_final_error() {
        let calls = RefCell::new(0);
        let res: Result<u32, StorageError> = retry_with_backoff(|| {
            *calls.borrow_mut() += 1;
            Err(StorageError::LockHeld {
                path: PathBuf::from("/tmp/x"),
            })
        });
        assert!(matches!(res, Err(StorageError::LockHeld { .. })));
        // One initial attempt + every retry in the schedule.
        assert_eq!(*calls.borrow(), BACKOFF_SCHEDULE_MS.len() + 1);
    }

    #[test]
    fn is_transient_message_matches_known_sqlite_phrases() {
        assert!(is_transient_message("database is locked"));
        assert!(is_transient_message("Database is locked"));
        assert!(is_transient_message(
            "connection pool error: locking protocol"
        ));
        assert!(is_transient_message("database is busy"));
        assert!(!is_transient_message("schema mismatch"));
        assert!(!is_transient_message("disk full"));
    }
}

//! Session handoff DB (`.standardoc-sessions/sessions.db`).
//!
//! Lives at a path **distinct** from `.standardoc/` so a workspace reset
//! (`standardoc rescan --from-scratch`, deleting `.standardoc/`, etc.) does NOT
//! kill the operator's accumulated session memos. Schema is versioned via
//! `schema_meta` mirroring the main index, but evolves independently.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension};

mod markdown;
mod schema;

pub use markdown::dump_sessions_to_markdown;

#[derive(Debug, thiserror::Error)]
pub enum SessionsError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("sessions handle is poisoned")]
    Poisoned,
    #[error(
        "session schema version v{db} is newer than supported v{supported} — \
         upgrade the binary"
    )]
    SchemaVersionTooNew { db: u32, supported: u32 },
    #[error("invalid schema metadata: {key} = {value}")]
    InvalidSchemaMetadata { key: String, value: String },
    #[error("invalid stored data: {detail}")]
    InvalidStoredData { detail: String },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionRow {
    pub id: i64,
    pub slug: String,
    pub body_md: String,
    pub supersedes: Option<String>,
    pub status: SessionStatus,
    pub created_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Active,
    Superseded,
}

impl SessionStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Superseded => "superseded",
        }
    }

    pub fn from_sql(s: &str) -> Result<Self, SessionsError> {
        match s {
            "active" => Ok(Self::Active),
            "superseded" => Ok(Self::Superseded),
            other => Err(SessionsError::InvalidStoredData {
                detail: format!("unknown session status: {other:?}"),
            }),
        }
    }
}

/// Workspace-scoped handle to the sessions DB. The connection is wrapped in a
/// `Mutex` to serialize access — sessions write volume is tiny (a few
/// operations per chat turn at most) so coarse locking is fine.
pub struct SessionsHandle {
    conn: Mutex<Connection>,
    db_path: PathBuf,
}

const SESSIONS_DIR: &str = ".standardoc-sessions";
const SESSIONS_DB: &str = "sessions.db";

const fn is_transient_sessions_err(err: &SessionsError) -> bool {
    let SessionsError::Sqlite(rusqlite::Error::SqliteFailure(code, _)) = err else {
        return false;
    };
    matches!(
        code.code,
        rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked,
    ) || code.extended_code == rusqlite::ffi::SQLITE_PROTOCOL
}

/// Backoff schedule (cumulative ~1.55 s) for retrying sessions DB open
/// when SQLite reports a transient busy/locked condition. Concrete races
/// in the field: CLI + daemon both opening on first ever creation, and
/// the fire-and-forget usage logger racing the next tool call's open.
/// Both compete on the exclusive lock taken by `PRAGMA journal_mode =
/// WAL` until the first creator finishes the conversion.
const OPEN_BACKOFF_MS: &[u64] = &[50, 100, 200, 400, 800];

impl SessionsHandle {
    /// Opens (creating if absent) `<workspace>/.standardoc-sessions/sessions.db`.
    /// Schema is initialised + migrated automatically. Transient SQLite
    /// busy/locked errors trigger an exponential-backoff retry; permanent
    /// errors (schema too new, IO-not-found, …) propagate immediately.
    pub fn open(workspace_root: &Path) -> Result<Self, SessionsError> {
        let dir = workspace_root.join(SESSIONS_DIR);
        std::fs::create_dir_all(&dir)?;
        let db_path = dir.join(SESSIONS_DB);
        let mut idx = 0usize;
        loop {
            match Self::try_open_once(&db_path) {
                Ok(handle) => return Ok(handle),
                Err(err) => {
                    if !is_transient_sessions_err(&err) || idx >= OPEN_BACKOFF_MS.len() {
                        return Err(err);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(OPEN_BACKOFF_MS[idx]));
                    idx += 1;
                }
            }
        }
    }

    fn try_open_once(db_path: &Path) -> Result<Self, SessionsError> {
        let conn = Connection::open(db_path)?;
        // Set the busy timeout at the C level BEFORE any PRAGMA so that
        // PRAGMA journal_mode = WAL (which takes an exclusive lock) waits
        // for concurrent openers instead of failing immediately with
        // SQLITE_BUSY. The outer retry catches the case where this still
        // surfaces as busy on tight races.
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; \
             PRAGMA synchronous = NORMAL;",
        )?;
        schema::ensure_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            db_path: db_path.to_path_buf(),
        })
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Insert (or update by slug) a session. If `supersedes` is provided and
    /// matches an existing slug, that row's `status` is set to `superseded`.
    /// The new row's status is always `active`.
    ///
    /// `#[allow]` rationale: the `MutexGuard` (`guard`) is held until the
    /// transaction commits — `tx` borrows from it. Splitting into a
    /// sub-scope to drop the guard earlier would force the `Ok(id)` to
    /// duplicate the variable across scopes for no real win.
    #[allow(clippy::significant_drop_tightening)]
    pub fn save(
        &self,
        slug: &str,
        body_md: &str,
        supersedes: Option<&str>,
    ) -> Result<i64, SessionsError> {
        let mut guard = self.conn.lock().map_err(|_| SessionsError::Poisoned)?;
        let tx = guard.transaction()?;
        let now = current_unix_seconds();
        tx.execute(
            "INSERT INTO sessions (slug, body_md, supersedes, status, created_at) \
             VALUES (?1, ?2, ?3, 'active', ?4) \
             ON CONFLICT(slug) DO UPDATE SET \
                body_md     = excluded.body_md, \
                supersedes  = excluded.supersedes, \
                status      = 'active', \
                created_at  = excluded.created_at",
            rusqlite::params![slug, body_md, supersedes, now],
        )?;
        let id: i64 = tx.query_row("SELECT id FROM sessions WHERE slug = ?1", [slug], |r| {
            r.get(0)
        })?;
        if let Some(prev) = supersedes {
            tx.execute(
                "UPDATE sessions SET status = 'superseded' WHERE slug = ?1",
                [prev],
            )?;
        }
        tx.commit()?;
        Ok(id)
    }

    /// Lists sessions newest first. `active_only` filters `status = 'active'`.
    #[allow(clippy::significant_drop_tightening)]
    pub fn list(&self, active_only: bool) -> Result<Vec<SessionRow>, SessionsError> {
        let guard = self.conn.lock().map_err(|_| SessionsError::Poisoned)?;
        let sql = if active_only {
            "SELECT id, slug, body_md, supersedes, status, created_at FROM sessions \
             WHERE status = 'active' ORDER BY created_at DESC, id DESC"
        } else {
            "SELECT id, slug, body_md, supersedes, status, created_at FROM sessions \
             ORDER BY created_at DESC, id DESC"
        };
        let mut stmt = guard.prepare(sql)?;
        let rows = stmt
            .query_map([], read_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().collect::<Result<Vec<_>, _>>()
    }

    #[allow(clippy::significant_drop_tightening)]
    pub fn get_by_slug(&self, slug: &str) -> Result<Option<SessionRow>, SessionsError> {
        let guard = self.conn.lock().map_err(|_| SessionsError::Poisoned)?;
        let raw = guard
            .query_row(
                "SELECT id, slug, body_md, supersedes, status, created_at \
                 FROM sessions WHERE slug = ?1",
                [slug],
                read_row,
            )
            .optional()?;
        raw.transpose()
    }

    /// Most recent active row (the natural reentry point for a new chat).
    #[allow(clippy::significant_drop_tightening)]
    pub fn latest(&self) -> Result<Option<SessionRow>, SessionsError> {
        let guard = self.conn.lock().map_err(|_| SessionsError::Poisoned)?;
        let raw = guard
            .query_row(
                "SELECT id, slug, body_md, supersedes, status, created_at \
                 FROM sessions WHERE status = 'active' \
                 ORDER BY created_at DESC, id DESC LIMIT 1",
                [],
                read_row,
            )
            .optional()?;
        raw.transpose()
    }

    /// Records a single tool invocation in the `usage_stats` table. The `fqdn`
    /// is `None` for tools that operate on the whole index rather than a
    /// specific symbol (e.g. `find_symbol`, `list_symbols`). `baseline_bytes`
    /// is the sum of file sizes of distinct source files referenced by the
    /// response — the honest "what an AI would have read raw" floor. Returns
    /// the inserted row id. Callers typically swallow errors (best-effort
    /// metric, never fail a tool call because of it).
    #[allow(clippy::significant_drop_tightening)]
    pub fn log_usage(
        &self,
        tool_name: &str,
        fqdn: Option<&str>,
        bytes_out: i64,
        baseline_bytes: i64,
    ) -> Result<i64, SessionsError> {
        let guard = self.conn.lock().map_err(|_| SessionsError::Poisoned)?;
        let now = current_unix_seconds();
        guard.execute(
            "INSERT INTO usage_stats (tool_name, fqdn, bytes_out, baseline_bytes, ts_unix) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![tool_name, fqdn, bytes_out, baseline_bytes, now],
        )?;
        Ok(guard.last_insert_rowid())
    }

    /// Aggregates `usage_stats` rows scoped to `period`. Returns the rolled-up
    /// counters plus a `ratio` of `bytes_out / baseline_bytes` (0.0 when the
    /// baseline is empty — fresh install, no calls yet).
    #[allow(clippy::significant_drop_tightening)]
    pub fn query_usage_stats(&self, period: UsagePeriod) -> Result<UsageStatsRow, SessionsError> {
        let guard = self.conn.lock().map_err(|_| SessionsError::Poisoned)?;
        let now = current_unix_seconds();
        let since = period.since(now);
        let (calls, bytes_out_total, baseline_bytes_total): (i64, i64, i64) = guard.query_row(
            "SELECT COUNT(*), COALESCE(SUM(bytes_out), 0), COALESCE(SUM(baseline_bytes), 0) \
                 FROM usage_stats WHERE ts_unix >= ?1",
            [since],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let bytes_saved = baseline_bytes_total - bytes_out_total;
        let ratio = if baseline_bytes_total > 0 {
            #[allow(clippy::cast_precision_loss)]
            let r = bytes_out_total as f64 / baseline_bytes_total as f64;
            r
        } else {
            0.0
        };
        Ok(UsageStatsRow {
            period: period.as_str().to_string(),
            calls,
            bytes_out_total,
            baseline_bytes_total,
            bytes_saved,
            ratio,
        })
    }

    /// Deletes `usage_stats` rows scoped to `period`. Returns the number of
    /// rows actually removed. `Day`/`Week` wipe only the trailing window;
    /// `All` empties the table. Used by the CLI `reset-usage` subcommand
    /// (and the VSCode "Reset token savings" wrapper) to baseline before a
    /// measurement run — never logged itself.
    pub fn reset_usage(&self, period: UsagePeriod) -> Result<u64, SessionsError> {
        let now = current_unix_seconds();
        let since = period.since(now);
        let deleted = self
            .conn
            .lock()
            .map_err(|_| SessionsError::Poisoned)?
            .execute("DELETE FROM usage_stats WHERE ts_unix >= ?1", [since])?;
        Ok(deleted as u64)
    }
}

/// Period filter for `query_usage_stats`. `Day` / `Week` are sliding windows
/// anchored at `now`; `All` returns the full table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsagePeriod {
    Day,
    Week,
    All,
}

impl UsagePeriod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::All => "all",
        }
    }

    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "day" | "d" | "today" => Some(Self::Day),
            "week" | "w" | "7d" => Some(Self::Week),
            "all" | "" => Some(Self::All),
            _ => None,
        }
    }

    pub const fn since(self, now: i64) -> i64 {
        match self {
            Self::Day => now - 86_400,
            Self::Week => now - 604_800,
            Self::All => 0,
        }
    }
}

/// Aggregated counters returned by `query_usage_stats`. Ratio is `bytes_out /
/// baseline_bytes` (0.0 when no data) — the honest compression factor.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UsageStatsRow {
    pub period: String,
    pub calls: i64,
    pub bytes_out_total: i64,
    pub baseline_bytes_total: i64,
    pub bytes_saved: i64,
    pub ratio: f64,
}

type RawRow = (i64, String, String, Option<String>, String, i64);

fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<SessionRow, SessionsError>> {
    let raw: RawRow = (
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
    );
    Ok(build_session_row(raw))
}

fn build_session_row(raw: RawRow) -> Result<SessionRow, SessionsError> {
    let (id, slug, body_md, supersedes, status_text, created_at) = raw;
    let status = SessionStatus::from_sql(&status_text)?;
    Ok(SessionRow {
        id,
        slug,
        body_md,
        supersedes,
        status,
        created_at,
    })
}

fn current_unix_seconds() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fresh_handle() -> (TempDir, SessionsHandle) {
        let dir = tempfile::tempdir().unwrap();
        let handle = SessionsHandle::open(dir.path()).unwrap();
        (dir, handle)
    }

    #[test]
    fn open_creates_db_under_separate_dir() {
        let (dir, handle) = fresh_handle();
        let expected = dir.path().join(SESSIONS_DIR).join(SESSIONS_DB);
        assert_eq!(handle.db_path(), expected);
        assert!(expected.exists());
    }

    #[test]
    fn save_then_get_round_trips() {
        let (_dir, handle) = fresh_handle();
        let id = handle.save("s1", "## body 1", None).unwrap();
        assert!(id > 0);
        let got = handle.get_by_slug("s1").unwrap().unwrap();
        assert_eq!(got.slug, "s1");
        assert_eq!(got.body_md, "## body 1");
        assert_eq!(got.status, SessionStatus::Active);
        assert!(got.supersedes.is_none());
    }

    #[test]
    fn save_with_same_slug_updates_in_place() {
        let (_dir, handle) = fresh_handle();
        let id1 = handle.save("s1", "first", None).unwrap();
        let id2 = handle.save("s1", "second", None).unwrap();
        assert_eq!(id1, id2, "UPSERT must preserve the id");
        let got = handle.get_by_slug("s1").unwrap().unwrap();
        assert_eq!(got.body_md, "second");
    }

    #[test]
    fn supersedes_marks_previous_row_as_superseded() {
        let (_dir, handle) = fresh_handle();
        handle.save("s1", "first", None).unwrap();
        handle.save("s2", "second", Some("s1")).unwrap();
        let s1 = handle.get_by_slug("s1").unwrap().unwrap();
        let s2 = handle.get_by_slug("s2").unwrap().unwrap();
        assert_eq!(s1.status, SessionStatus::Superseded);
        assert_eq!(s2.status, SessionStatus::Active);
        assert_eq!(s2.supersedes.as_deref(), Some("s1"));
    }

    #[test]
    fn list_returns_newest_first() {
        let (_dir, handle) = fresh_handle();
        handle.save("s1", "first", None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        handle.save("s2", "second", None).unwrap();
        let all = handle.list(false).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].slug, "s2");
        assert_eq!(all[1].slug, "s1");
    }

    #[test]
    fn list_active_only_filters_superseded() {
        let (_dir, handle) = fresh_handle();
        handle.save("old", "old body", None).unwrap();
        handle.save("new", "new body", Some("old")).unwrap();
        let active = handle.list(true).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].slug, "new");
    }

    #[test]
    fn latest_returns_most_recent_active() {
        let (_dir, handle) = fresh_handle();
        handle.save("s1", "first", None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        handle.save("s2", "second", None).unwrap();
        let latest = handle.latest().unwrap().unwrap();
        assert_eq!(latest.slug, "s2");
    }

    #[test]
    fn latest_skips_superseded_even_if_more_recent_active_exists() {
        let (_dir, handle) = fresh_handle();
        handle.save("active", "still here", None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        handle.save("newer", "now superseded", None).unwrap();
        // Mark `newer` as superseded by inserting another row that supersedes it.
        handle.save("newest", "live", Some("newer")).unwrap();
        let latest = handle.latest().unwrap().unwrap();
        assert_eq!(latest.slug, "newest");
    }

    #[test]
    fn get_by_unknown_slug_returns_none() {
        let (_dir, handle) = fresh_handle();
        let got = handle.get_by_slug("missing").unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn open_is_idempotent_across_reopens() {
        let dir = tempfile::tempdir().unwrap();
        {
            let h = SessionsHandle::open(dir.path()).unwrap();
            h.save("s1", "first", None).unwrap();
        }
        let h2 = SessionsHandle::open(dir.path()).unwrap();
        let got = h2.get_by_slug("s1").unwrap().unwrap();
        assert_eq!(got.body_md, "first");
    }

    #[test]
    fn log_usage_inserts_row() {
        let (_dir, handle) = fresh_handle();
        let id = handle
            .log_usage("get_context", Some("crate::foo"), 250, 1800)
            .unwrap();
        assert!(id > 0);
        let stats = handle.query_usage_stats(UsagePeriod::All).unwrap();
        assert_eq!(stats.calls, 1);
        assert_eq!(stats.bytes_out_total, 250);
        assert_eq!(stats.baseline_bytes_total, 1800);
        assert_eq!(stats.bytes_saved, 1550);
        assert!((stats.ratio - (250.0 / 1800.0)).abs() < 1e-9);
    }

    #[test]
    fn log_usage_accepts_null_fqdn() {
        let (_dir, handle) = fresh_handle();
        handle.log_usage("find_symbol", None, 80, 0).unwrap();
        let stats = handle.query_usage_stats(UsagePeriod::All).unwrap();
        assert_eq!(stats.calls, 1);
        assert_eq!(stats.bytes_out_total, 80);
        assert_eq!(stats.baseline_bytes_total, 0);
        assert!((stats.ratio - 0.0).abs() < 1e-9);
    }

    #[test]
    fn query_usage_stats_aggregates_multiple_rows() {
        let (_dir, handle) = fresh_handle();
        handle
            .log_usage("get_context", Some("a"), 100, 1000)
            .unwrap();
        handle
            .log_usage("get_context", Some("b"), 200, 2000)
            .unwrap();
        handle.log_usage("find_symbol", None, 50, 0).unwrap();
        let stats = handle.query_usage_stats(UsagePeriod::All).unwrap();
        assert_eq!(stats.calls, 3);
        assert_eq!(stats.bytes_out_total, 350);
        assert_eq!(stats.baseline_bytes_total, 3000);
        assert_eq!(stats.bytes_saved, 2650);
    }

    #[test]
    fn query_usage_stats_period_filters_old_rows() {
        let (_dir, handle) = fresh_handle();
        handle
            .log_usage("get_context", Some("recent"), 100, 1000)
            .unwrap();
        // Backdate one row by 8 days so it falls outside the week window.
        {
            let guard = handle.conn.lock().unwrap();
            let cutoff = current_unix_seconds() - 8 * 86_400;
            guard
                .execute(
                    "INSERT INTO usage_stats (tool_name, fqdn, bytes_out, baseline_bytes, ts_unix) \
                     VALUES ('get_context', 'old', 999, 9999, ?1)",
                    [cutoff],
                )
                .unwrap();
        }
        let day = handle.query_usage_stats(UsagePeriod::Day).unwrap();
        assert_eq!(day.calls, 1);
        assert_eq!(day.bytes_out_total, 100);
        let week = handle.query_usage_stats(UsagePeriod::Week).unwrap();
        assert_eq!(
            week.calls, 1,
            "week window still excludes the 8-day-old row"
        );
        let all = handle.query_usage_stats(UsagePeriod::All).unwrap();
        assert_eq!(all.calls, 2);
    }

    #[test]
    fn query_usage_stats_empty_returns_zero_ratio() {
        let (_dir, handle) = fresh_handle();
        let stats = handle.query_usage_stats(UsagePeriod::All).unwrap();
        assert_eq!(stats.calls, 0);
        assert_eq!(stats.bytes_out_total, 0);
        assert_eq!(stats.baseline_bytes_total, 0);
        assert_eq!(stats.bytes_saved, 0);
        assert!((stats.ratio - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn reset_usage_all_wipes_every_row_and_returns_count() {
        let (_dir, handle) = fresh_handle();
        handle.log_usage("get_context", Some("a"), 10, 100).unwrap();
        handle.log_usage("find_symbol", Some("b"), 20, 200).unwrap();
        let deleted = handle.reset_usage(UsagePeriod::All).unwrap();
        assert_eq!(deleted, 2);
        let stats = handle.query_usage_stats(UsagePeriod::All).unwrap();
        assert_eq!(stats.calls, 0);
    }

    #[test]
    fn reset_usage_day_keeps_rows_older_than_window() {
        let (_dir, handle) = fresh_handle();
        handle
            .log_usage("get_context", Some("recent"), 100, 1000)
            .unwrap();
        // Backdate a row by 8 days so it falls outside the day window.
        {
            let guard = handle.conn.lock().unwrap();
            let cutoff = current_unix_seconds() - 8 * 86_400;
            guard
                .execute(
                    "INSERT INTO usage_stats (tool_name, fqdn, bytes_out, baseline_bytes, ts_unix) \
                     VALUES ('get_context', 'old', 999, 9999, ?1)",
                    [cutoff],
                )
                .unwrap();
        }
        let deleted = handle.reset_usage(UsagePeriod::Day).unwrap();
        assert_eq!(deleted, 1, "only the recent row falls inside the day window");
        let all = handle.query_usage_stats(UsagePeriod::All).unwrap();
        assert_eq!(all.calls, 1, "the 8-day-old row survives a Day reset");
    }

    #[test]
    fn reset_usage_returns_zero_on_empty_table() {
        let (_dir, handle) = fresh_handle();
        let deleted = handle.reset_usage(UsagePeriod::All).unwrap();
        assert_eq!(deleted, 0);
    }

    #[test]
    fn usage_period_parse_accepts_aliases() {
        assert_eq!(UsagePeriod::from_str_loose("day"), Some(UsagePeriod::Day));
        assert_eq!(UsagePeriod::from_str_loose("D"), Some(UsagePeriod::Day));
        assert_eq!(UsagePeriod::from_str_loose("Today"), Some(UsagePeriod::Day));
        assert_eq!(UsagePeriod::from_str_loose("week"), Some(UsagePeriod::Week));
        assert_eq!(UsagePeriod::from_str_loose("7d"), Some(UsagePeriod::Week));
        assert_eq!(UsagePeriod::from_str_loose("all"), Some(UsagePeriod::All));
        assert_eq!(UsagePeriod::from_str_loose(""), Some(UsagePeriod::All));
        assert_eq!(UsagePeriod::from_str_loose("xxx"), None);
    }

    #[test]
    fn usage_period_since_matches_window() {
        assert_eq!(UsagePeriod::Day.since(1_000_000), 1_000_000 - 86_400);
        assert_eq!(UsagePeriod::Week.since(1_000_000), 1_000_000 - 604_800);
        assert_eq!(UsagePeriod::All.since(1_000_000), 0);
    }
}

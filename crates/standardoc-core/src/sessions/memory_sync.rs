//! Bridge between Claude's per-workspace memory dir
//! (`~/.claude/projects/<hash>/memory/`) and the standardoc sessions DB.
//!
//! Claude's harness persists three sorts of memos as standalone `.md` files
//! with a tiny YAML frontmatter (`name`, `description`, `type`) plus a free
//! body. The `MEMORY.md` index lists them, one bullet per file. We mirror
//! this convention so an `import_memory_dir` pass UPSERTs every relevant
//! file into the sessions table tagged by [`SessionKind`], and
//! `export_memory_dir` reconstructs the same layout from the DB — making
//! the sessions DB the portable source of truth across machines while
//! Claude's local dir stays the working surface.
//!
//! Mapping rules (`type` frontmatter → [`SessionKind`]):
//!
//! | Frontmatter `type` | Maps to            |
//! | ------------------ | ------------------ |
//! | `user`             | `SessionKind::Profile`  |
//! | `feedback`         | `SessionKind::Feedback` |
//! | `project`          | `SessionKind::Lock`     |
//! | `reference`        | `SessionKind::Profile`  |
//! | anything else      | `SessionKind::Session`  |
//!
//! The `MEMORY.md` index file itself is never imported as a row — it's
//! regenerated on export from the DB contents.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use super::{
    SessionKind, SessionRow, SessionStatus, SessionsError, SessionsHandle, current_unix_seconds,
};

/// File name of the per-workspace index Claude maintains alongside the
/// individual memo files. Skipped on import, regenerated on export.
pub const MEMORY_INDEX_NAME: &str = "MEMORY.md";

#[derive(Debug, thiserror::Error)]
pub enum MemorySyncError {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("malformed frontmatter at {path}: {detail}")]
    MalformedFrontmatter { path: PathBuf, detail: String },
    #[error("sessions DB error: {0}")]
    Sessions(#[from] SessionsError),
}

/// One memo's worth of structured content as parsed off a memory `.md` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryEntry {
    pub slug: String,
    pub kind: SessionKind,
    pub name: String,
    pub description: String,
    pub body: String,
    pub supersedes: Option<String>,
    pub status: SessionStatus,
    pub created_at: i64,
}

/// Counters returned by [`import_memory_dir`]. Useful for surfacing a
/// concise "X memos imported, Y skipped" line in the CLI/MCP response.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct ImportReport {
    pub imported: usize,
    pub skipped: usize,
    pub errors: usize,
}

/// Counters returned by [`export_memory_dir`]. `index_written` is `true`
/// when the `MEMORY.md` index file was (re)written.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct ExportReport {
    pub exported: usize,
    pub index_written: bool,
}

/// Parses a single memory `.md` file into a [`MemoryEntry`]. Frontmatter is
/// expected as a YAML-ish block delimited by `---` on its own line; absence
/// of frontmatter is treated as an error (returns `MalformedFrontmatter`).
///
/// Missing optional keys (`status`, `supersedes`, `created_at`) fall back to
/// `Active` / `None` / `current_unix_seconds()` so hand-written memory files
/// that predate the extended schema import cleanly.
pub fn parse_memory_file(path: &Path) -> Result<MemoryEntry, MemorySyncError> {
    let raw = std::fs::read_to_string(path).map_err(|e| MemorySyncError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let (front, body) = split_frontmatter(&raw, path)?;
    let mut name = String::new();
    let mut description = String::new();
    let mut type_str = String::new();
    let mut status_str = String::new();
    let mut supersedes_str = String::new();
    let mut created_at_str = String::new();
    for line in front.lines() {
        if let Some((key, value)) = split_kv(line) {
            match key.as_str() {
                "name" => name = value,
                "description" => description = value,
                "type" => type_str = value,
                "status" => status_str = value,
                "supersedes" => supersedes_str = value,
                "created_at" => created_at_str = value,
                _ => {}
            }
        }
    }
    let slug = slug_from_path(path);
    let kind = kind_from_type_str(&type_str);
    let status = status_from_str(&status_str);
    let supersedes = if supersedes_str.is_empty() {
        None
    } else {
        Some(supersedes_str)
    };
    let created_at = created_at_str
        .parse::<i64>()
        .unwrap_or_else(|_| current_unix_seconds());
    Ok(MemoryEntry {
        slug,
        kind,
        name,
        description,
        body: body.to_string(),
        supersedes,
        status,
        created_at,
    })
}

/// Walks `dir` (one level deep) and UPSERTs every `.md` file except
/// `MEMORY.md` into the sessions DB. Returns counts; failures on
/// individual files are tallied into `errors` and skipped rather than
/// aborting the whole pass.
pub fn import_memory_dir(
    handle: &SessionsHandle,
    dir: &Path,
) -> Result<ImportReport, MemorySyncError> {
    let mut report = ImportReport::default();
    let read = std::fs::read_dir(dir).map_err(|e| MemorySyncError::Io {
        path: dir.to_path_buf(),
        source: e,
    })?;
    for entry in read {
        let entry = if let Ok(e) = entry {
            e
        } else {
            report.errors += 1;
            continue;
        };
        let path = entry.path();
        if !is_importable_memory_file(&path) {
            report.skipped += 1;
            continue;
        }
        match parse_memory_file(&path) {
            Ok(memo) => match handle.save_full(
                &memo.slug,
                &memo.body,
                memo.supersedes.as_deref(),
                memo.kind,
                memo.status,
                memo.created_at,
            ) {
                Ok(_) => report.imported += 1,
                Err(_) => report.errors += 1,
            },
            Err(_) => report.errors += 1,
        }
    }
    Ok(report)
}

/// Writes every session memo as `<slug>.md` with a regenerated frontmatter
/// under `dir`, then rewrites `MEMORY.md` as a one-line-per-memo index.
/// `dir` is created if missing. Existing files with the same names are
/// overwritten (idempotent re-export).
pub fn export_memory_dir(
    handle: &SessionsHandle,
    dir: &Path,
) -> Result<ExportReport, MemorySyncError> {
    std::fs::create_dir_all(dir).map_err(|e| MemorySyncError::Io {
        path: dir.to_path_buf(),
        source: e,
    })?;
    let rows = handle.list(false)?;
    let mut report = ExportReport::default();
    for row in &rows {
        let path = dir.join(format!("{}.md", row.slug));
        let content = render_memo_file(row);
        std::fs::write(&path, content).map_err(|e| MemorySyncError::Io {
            path: path.clone(),
            source: e,
        })?;
        report.exported += 1;
    }
    let index_path = dir.join(MEMORY_INDEX_NAME);
    std::fs::write(&index_path, render_index(&rows)).map_err(|e| MemorySyncError::Io {
        path: index_path,
        source: e,
    })?;
    report.index_written = true;
    Ok(report)
}

fn split_frontmatter<'a>(raw: &'a str, path: &Path) -> Result<(&'a str, &'a str), MemorySyncError> {
    let mut stripped = raw
        .strip_prefix("---\n")
        .or_else(|| raw.strip_prefix("---\r\n"));
    if stripped.is_none() {
        return Err(MemorySyncError::MalformedFrontmatter {
            path: path.to_path_buf(),
            detail: "missing leading `---` delimiter".into(),
        });
    }
    let after_open = stripped.take().unwrap_or(raw);
    let end_idx = find_closing_delimiter(after_open).ok_or_else(|| {
        MemorySyncError::MalformedFrontmatter {
            path: path.to_path_buf(),
            detail: "missing closing `---` delimiter".into(),
        }
    })?;
    let front = &after_open[..end_idx];
    let rest = after_open[end_idx..]
        .trim_start_matches("---")
        .trim_start_matches('\r')
        .trim_start_matches('\n');
    Ok((front, rest))
}

fn find_closing_delimiter(s: &str) -> Option<usize> {
    let mut offset = 0usize;
    for line in s.split_inclusive('\n') {
        let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
        if trimmed == "---" {
            return Some(offset);
        }
        offset += line.len();
    }
    None
}

fn split_kv(line: &str) -> Option<(String, String)> {
    let (key, value) = line.split_once(':')?;
    let key = key.trim().to_string();
    let value = value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string();
    if key.is_empty() {
        return None;
    }
    Some((key, value))
}

fn slug_from_path(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("memo");
    stem.replace('_', "-").to_ascii_lowercase()
}

fn kind_from_type_str(s: &str) -> SessionKind {
    match s.trim().to_ascii_lowercase().as_str() {
        "user" | "reference" => SessionKind::Profile,
        "feedback" => SessionKind::Feedback,
        "project" => SessionKind::Lock,
        _ => SessionKind::Session,
    }
}

fn status_from_str(s: &str) -> SessionStatus {
    SessionStatus::from_sql(s.trim().to_ascii_lowercase().as_str()).unwrap_or(SessionStatus::Active)
}

const fn type_str_from_kind(kind: SessionKind) -> &'static str {
    match kind {
        SessionKind::Profile => "user",
        SessionKind::Feedback => "feedback",
        SessionKind::Lock => "project",
        SessionKind::Session => "session",
    }
}

fn is_importable_memory_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    if name.eq_ignore_ascii_case(MEMORY_INDEX_NAME) {
        return false;
    }
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md"))
}

fn render_memo_file(row: &SessionRow) -> String {
    let mut out = String::from("---\n");
    let _ = writeln!(out, "name: {}", row.slug);
    let _ = writeln!(
        out,
        "description: exported from sessions DB ({})",
        row.kind.as_str()
    );
    let _ = writeln!(out, "type: {}", type_str_from_kind(row.kind));
    let _ = writeln!(out, "status: {}", row.status.as_str());
    if let Some(prev) = &row.supersedes {
        let _ = writeln!(out, "supersedes: {prev}");
    }
    let _ = writeln!(out, "created_at: {}", row.created_at);
    out.push_str("---\n\n");
    out.push_str(&row.body_md);
    if !row.body_md.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn render_index(rows: &[SessionRow]) -> String {
    let mut out = String::from(
        "# Memory index\n\nRegenerated by `standardoc session sync-out`. One line per memo.\n\n",
    );
    for row in rows {
        let summary = first_line_of_body(&row.body_md);
        let _ = writeln!(
            out,
            "- [{slug}]({slug}.md) [{kind}] — {summary}",
            slug = row.slug,
            kind = row.kind.as_str(),
        );
    }
    out
}

fn first_line_of_body(body: &str) -> String {
    let line = body
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let truncated: String = line.chars().take(120).collect();
    truncated
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

    fn write_memo(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn parse_extracts_frontmatter_and_body() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_memo(
            dir.path(),
            "feedback_strict_mcp.md",
            "---\nname: strict mcp\ndescription: prefer MCP over grep\ntype: feedback\n---\n\nBody text.\nNext line.\n",
        );
        let entry = parse_memory_file(&path).unwrap();
        assert_eq!(entry.slug, "feedback-strict-mcp");
        assert_eq!(entry.kind, SessionKind::Feedback);
        assert_eq!(entry.name, "strict mcp");
        assert_eq!(entry.description, "prefer MCP over grep");
        assert_eq!(entry.body, "Body text.\nNext line.\n");
    }

    #[test]
    fn parse_maps_user_type_to_profile_kind() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_memo(
            dir.path(),
            "user_profile.md",
            "---\nname: profile\ndescription: x\ntype: user\n---\n\nbody",
        );
        let entry = parse_memory_file(&path).unwrap();
        assert_eq!(entry.kind, SessionKind::Profile);
    }

    #[test]
    fn parse_maps_project_type_to_lock_kind() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_memo(
            dir.path(),
            "standardoc_storage_v1.md",
            "---\nname: storage v1\ndescription: lock\ntype: project\n---\n\nbody",
        );
        let entry = parse_memory_file(&path).unwrap();
        assert_eq!(entry.kind, SessionKind::Lock);
        assert_eq!(entry.slug, "standardoc-storage-v1");
    }

    #[test]
    fn parse_rejects_missing_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_memo(dir.path(), "no_front.md", "just body, no frontmatter\n");
        let err = parse_memory_file(&path).unwrap_err();
        assert!(matches!(err, MemorySyncError::MalformedFrontmatter { .. }));
    }

    #[test]
    fn import_persists_each_memo_with_mapped_kind() {
        let (_dir, handle) = fresh_handle();
        let src = tempfile::tempdir().unwrap();
        write_memo(
            src.path(),
            "feedback_one.md",
            "---\nname: f1\ndescription: d\ntype: feedback\n---\nbody1",
        );
        write_memo(
            src.path(),
            "user_profile.md",
            "---\nname: p\ndescription: d\ntype: user\n---\nbody2",
        );
        write_memo(
            src.path(),
            "standardoc_lock_x.md",
            "---\nname: l\ndescription: d\ntype: project\n---\nbody3",
        );
        // MEMORY.md is the index, must be skipped.
        write_memo(src.path(), "MEMORY.md", "- index entry\n");
        let report = import_memory_dir(&handle, src.path()).unwrap();
        assert_eq!(report.imported, 3);
        assert_eq!(report.skipped, 1, "MEMORY.md is skipped");
        let rows = handle.list(false).unwrap();
        assert_eq!(rows.len(), 3);
        let kinds: std::collections::BTreeSet<_> = rows.iter().map(|r| r.kind).collect();
        assert!(kinds.contains(&SessionKind::Feedback));
        assert!(kinds.contains(&SessionKind::Profile));
        assert!(kinds.contains(&SessionKind::Lock));
    }

    #[test]
    fn import_is_idempotent_via_upsert() {
        let (_dir, handle) = fresh_handle();
        let src = tempfile::tempdir().unwrap();
        write_memo(
            src.path(),
            "feedback_one.md",
            "---\nname: f1\ndescription: d\ntype: feedback\n---\nfirst body",
        );
        let r1 = import_memory_dir(&handle, src.path()).unwrap();
        write_memo(
            src.path(),
            "feedback_one.md",
            "---\nname: f1\ndescription: d\ntype: feedback\n---\nsecond body",
        );
        let r2 = import_memory_dir(&handle, src.path()).unwrap();
        assert_eq!(r1.imported, 1);
        assert_eq!(r2.imported, 1);
        let rows = handle.list(false).unwrap();
        assert_eq!(rows.len(), 1, "UPSERT preserves the unique slug");
        assert_eq!(rows[0].body_md, "second body");
    }

    #[test]
    fn export_writes_each_memo_and_index() {
        let (_dir, handle) = fresh_handle();
        handle
            .save_with_kind("alpha", "Body of alpha\nline 2", None, SessionKind::Lock)
            .unwrap();
        handle
            .save_with_kind("beta", "Body of beta", None, SessionKind::Feedback)
            .unwrap();
        let target = tempfile::tempdir().unwrap();
        let report = export_memory_dir(&handle, target.path()).unwrap();
        assert_eq!(report.exported, 2);
        assert!(report.index_written);
        let alpha = std::fs::read_to_string(target.path().join("alpha.md")).unwrap();
        assert!(alpha.starts_with("---\n"));
        assert!(alpha.contains("type: project"));
        assert!(alpha.contains("Body of alpha"));
        let beta = std::fs::read_to_string(target.path().join("beta.md")).unwrap();
        assert!(beta.contains("type: feedback"));
        let index = std::fs::read_to_string(target.path().join(MEMORY_INDEX_NAME)).unwrap();
        assert!(index.contains("[alpha](alpha.md)"));
        assert!(index.contains("[beta](beta.md)"));
    }

    #[test]
    fn export_then_import_roundtrips_kinds() {
        let (_dir, source_handle) = fresh_handle();
        source_handle
            .save_with_kind("ranger", "lock body", None, SessionKind::Lock)
            .unwrap();
        source_handle
            .save_with_kind("scout", "profile body", None, SessionKind::Profile)
            .unwrap();
        let mid = tempfile::tempdir().unwrap();
        export_memory_dir(&source_handle, mid.path()).unwrap();
        let (_dir2, target_handle) = fresh_handle();
        let report = import_memory_dir(&target_handle, mid.path()).unwrap();
        assert_eq!(report.imported, 2);
        let rows = target_handle.list(false).unwrap();
        let by_slug: std::collections::HashMap<_, _> =
            rows.into_iter().map(|r| (r.slug.clone(), r)).collect();
        assert_eq!(by_slug["ranger"].kind, SessionKind::Lock);
        assert_eq!(by_slug["scout"].kind, SessionKind::Profile);
    }

    #[test]
    fn is_importable_skips_directories_and_non_md() {
        let dir = tempfile::tempdir().unwrap();
        write_memo(dir.path(), "note.txt", "");
        std::fs::create_dir(dir.path().join("subdir")).unwrap();
        assert!(!is_importable_memory_file(&dir.path().join("note.txt")));
        assert!(!is_importable_memory_file(&dir.path().join("subdir")));
        assert!(!is_importable_memory_file(&dir.path().join("MEMORY.md")));
    }

    #[test]
    fn parse_reads_extended_frontmatter_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_memo(
            dir.path(),
            "extended.md",
            "---\nname: x\ndescription: d\ntype: session\nstatus: superseded\nsupersedes: prior\ncreated_at: 12345\n---\nbody\n",
        );
        let entry = parse_memory_file(&path).unwrap();
        assert_eq!(entry.status, SessionStatus::Superseded);
        assert_eq!(entry.supersedes.as_deref(), Some("prior"));
        assert_eq!(entry.created_at, 12345);
    }

    #[test]
    fn parse_defaults_when_extended_keys_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_memo(
            dir.path(),
            "legacy.md",
            "---\nname: x\ndescription: d\ntype: feedback\n---\nbody\n",
        );
        let entry = parse_memory_file(&path).unwrap();
        assert_eq!(entry.status, SessionStatus::Active);
        assert!(entry.supersedes.is_none());
        assert!(
            entry.created_at > 0,
            "missing created_at falls back to now()"
        );
    }

    #[test]
    fn export_writes_status_and_created_at_always() {
        let (_dir, handle) = fresh_handle();
        handle
            .save_with_kind("alpha", "body", None, SessionKind::Lock)
            .unwrap();
        let target = tempfile::tempdir().unwrap();
        export_memory_dir(&handle, target.path()).unwrap();
        let content = std::fs::read_to_string(target.path().join("alpha.md")).unwrap();
        assert!(content.contains("status: active"));
        assert!(content.contains("created_at: "));
        assert!(
            !content.contains("supersedes:"),
            "supersedes line omitted when None"
        );
    }

    #[test]
    fn export_includes_supersedes_when_present() {
        let (_dir, handle) = fresh_handle();
        handle.save("prev", "old body", None).unwrap();
        handle.save("new", "new body", Some("prev")).unwrap();
        let target = tempfile::tempdir().unwrap();
        export_memory_dir(&handle, target.path()).unwrap();
        let content = std::fs::read_to_string(target.path().join("new.md")).unwrap();
        assert!(content.contains("supersedes: prev"));
    }

    #[test]
    fn roundtrip_preserves_all_db_fields_strict() {
        let (_dir, source) = fresh_handle();
        source
            .save_full(
                "alpha",
                "body alpha\n",
                None,
                SessionKind::Session,
                SessionStatus::Active,
                1_000_000_000,
            )
            .unwrap();
        source
            .save_full(
                "beta",
                "body beta\n",
                Some("alpha"),
                SessionKind::Feedback,
                SessionStatus::Superseded,
                1_000_000_100,
            )
            .unwrap();

        let mid = tempfile::tempdir().unwrap();
        export_memory_dir(&source, mid.path()).unwrap();

        let (_dir2, target) = fresh_handle();
        let report = import_memory_dir(&target, mid.path()).unwrap();
        assert_eq!(report.imported, 2);

        let alpha = target.get_by_slug("alpha").unwrap().unwrap();
        let beta = target.get_by_slug("beta").unwrap().unwrap();
        assert_eq!(alpha.body_md, "body alpha\n");
        assert_eq!(alpha.kind, SessionKind::Session);
        assert_eq!(alpha.status, SessionStatus::Active);
        assert!(alpha.supersedes.is_none());
        assert_eq!(alpha.created_at, 1_000_000_000);
        assert_eq!(beta.body_md, "body beta\n");
        assert_eq!(beta.kind, SessionKind::Feedback);
        assert_eq!(beta.status, SessionStatus::Superseded);
        assert_eq!(beta.supersedes.as_deref(), Some("alpha"));
        assert_eq!(beta.created_at, 1_000_000_100);
    }

    #[test]
    fn roundtrip_preserves_supersedes_chain_without_cascade() {
        let (_dir, source) = fresh_handle();
        source
            .save_full(
                "a",
                "a body",
                None,
                SessionKind::Session,
                SessionStatus::Superseded,
                100,
            )
            .unwrap();
        source
            .save_full(
                "b",
                "b body",
                Some("a"),
                SessionKind::Session,
                SessionStatus::Superseded,
                200,
            )
            .unwrap();
        source
            .save_full(
                "c",
                "c body",
                Some("b"),
                SessionKind::Session,
                SessionStatus::Active,
                300,
            )
            .unwrap();

        let mid = tempfile::tempdir().unwrap();
        export_memory_dir(&source, mid.path()).unwrap();
        let (_dir2, target) = fresh_handle();
        import_memory_dir(&target, mid.path()).unwrap();

        let a = target.get_by_slug("a").unwrap().unwrap();
        let b = target.get_by_slug("b").unwrap().unwrap();
        let c = target.get_by_slug("c").unwrap().unwrap();
        assert_eq!(a.status, SessionStatus::Superseded);
        assert!(a.supersedes.is_none());
        assert_eq!(b.status, SessionStatus::Superseded);
        assert_eq!(b.supersedes.as_deref(), Some("a"));
        assert_eq!(c.status, SessionStatus::Active);
        assert_eq!(c.supersedes.as_deref(), Some("b"));
    }
}

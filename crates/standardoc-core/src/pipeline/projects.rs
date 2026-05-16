//! Stage 3d-2 — workspace project discovery + persistence.
//!
//! Wraps `standarbuild_detect::discover_with()` against a
//! [`DetectorRegistry`] with the IR mapping + storage upsert. Called
//! once at cold-start to seed the `projects` table; later refreshed
//! by the watcher when a manifest file changes.
//!
//! The file→project_id resolution happens via a single batch SQL
//! UPDATE at the end of cold-start (see [`reconcile_files_project_id`])
//! — cheaper than threading project lookup through the walker's hot
//! path, and naturally handles re-runs after manifest churn.
//!
//! ## Extensibility
//!
//! [`discover_and_persist_projects_with`] takes an explicit
//! [`DetectorRegistry`] so future providers (WGSL, Move, Solidity, …)
//! can register custom kinds without touching this crate. The
//! [`discover_and_persist_projects`] shortcut wires the built-in
//! registry (Rust/Node/Bun/Deno/Python/Lua/C/Cpp).

use std::path::Path;

use rusqlite::Connection;
use standarbuild_detect::{DetectorRegistry, KindId};
use standardoc_ir::{ProjectInfo, ProjectKind};

use crate::storage::error::StorageError;
use crate::storage::projects;

pub use standarbuild_detect::{Detector, DetectorRegistry as ProjectDetectorRegistry};

/// Convert the detector's opaque [`KindId`] to the IR's `ProjectKind`.
/// Built-in slugs map to their named variants; anything else becomes
/// [`ProjectKind::Custom`] so user-registered detectors (e.g. a future
/// WGSL detector) round-trip cleanly.
fn from_kind_id(id: &KindId) -> ProjectKind {
    match id.as_str() {
        "rust" => ProjectKind::Rust,
        "node" => ProjectKind::Node,
        "bun" => ProjectKind::Bun,
        "deno" => ProjectKind::Deno,
        "python" => ProjectKind::Python,
        "lua" => ProjectKind::Lua,
        "c" => ProjectKind::C,
        "cpp" => ProjectKind::Cpp,
        "unknown" => ProjectKind::Unknown,
        other => ProjectKind::Custom(other.to_string()),
    }
}

/// Normalise a `rel_path` from `standarbuild_detect::Discovered` for
/// storage: `"."` becomes `""` (root sentinel) so SQL prefix matching
/// against `files.path` works without special-casing the root case.
/// Strips leading `"./"`.
fn normalise_rel_path(rel: &str) -> String {
    if rel == "." {
        return String::new();
    }
    rel.strip_prefix("./").unwrap_or(rel).to_string()
}

/// Run `standarbuild_detect::discover` against `workspace_root` with
/// defaults, UPSERT every detected project into `projects`, and return
/// the canonicalised list (with assigned project_ids).
///
/// Idempotent: re-running picks up new projects + updates label/kind
/// drift on existing roots without losing project_id continuity.
/// Run discovery with the built-in detector registry (Rust / Node /
/// Bun / Deno / Python / Lua / C / Cpp). Shortcut for
/// [`discover_and_persist_projects_with`].
pub fn discover_and_persist_projects(
    conn: &Connection,
    workspace_root: &Path,
) -> Result<Vec<ProjectInfo>, StorageError> {
    discover_and_persist_projects_with(conn, workspace_root, &DetectorRegistry::with_builtins())
}

/// Run discovery against a custom [`DetectorRegistry`], UPSERTing every
/// detected project into `projects`. Use this to extend the detection
/// space with user-registered detectors (e.g. a WGSL detector that
/// matches on `shaders/` directories with `priority() > 100` to
/// override built-ins).
pub fn discover_and_persist_projects_with(
    conn: &Connection,
    workspace_root: &Path,
    registry: &DetectorRegistry,
) -> Result<Vec<ProjectInfo>, StorageError> {
    let opts = standarbuild_detect::DiscoverOptions::default();
    let detected = standarbuild_detect::discover_with(workspace_root, &opts, registry);
    let mut out = Vec::with_capacity(detected.len());
    for d in detected {
        // Filter out the `Unknown` sentinel — the detector's
        // `include_unknown_at_depth_one` default surfaces depth-1
        // dirs as Unknown for UI bootstrap contexts, but we'd
        // mis-attribute every file under a Unknown `src/` if we
        // persisted them (deepest-match would prefer it over the
        // workspace-root Rust project).
        if d.kind == KindId::UNKNOWN {
            continue;
        }
        let kind = from_kind_id(&d.kind);
        let root_path = d.absolute_path.to_string_lossy().into_owned();
        let rel_path = normalise_rel_path(&d.rel_path);
        let project_id =
            projects::upsert_project(conn, &d.label, &kind, &root_path, &rel_path)?;
        out.push(ProjectInfo {
            project_id,
            label: d.label,
            kind,
            root_path,
            rel_path,
        });
    }
    Ok(out)
}

/// Single SQL pass that assigns `files.project_id` based on the deepest
/// project whose `rel_path` matches the file's path. Called once at the
/// end of cold-start (after both files and projects are populated) and
/// again whenever the watcher triggers a manifest re-detection.
///
/// `files.path` is the POSIX-style relative path to `workspace_root`;
/// `projects.rel_path` uses the same convention (empty string for the
/// workspace-root project, `"crates/foo"` for sub-projects). The match
/// picks the longest `rel_path` so a file under `ext/vscode/src/` lands
/// on the `ext/vscode` Bun project, not the workspace-root Rust one.
pub fn reconcile_files_project_id(conn: &Connection) -> Result<usize, StorageError> {
    // Two SQL statements: clear first (so unlinked projects' files
    // become NULL again), then re-assign by deepest match.
    conn.execute("UPDATE files SET project_id = NULL", [])?;
    let updated = conn.execute(
        "UPDATE files SET project_id = ( \
            SELECT p.project_id FROM projects p \
            WHERE p.rel_path = '' \
               OR files.path = p.rel_path \
               OR files.path LIKE p.rel_path || '/%' \
            ORDER BY length(p.rel_path) DESC \
            LIMIT 1 \
         )",
        [],
    )?;
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrate::ensure_schema;
    use rusqlite::params;
    use std::fs;
    use tempfile::tempdir;

    fn fresh_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        ensure_schema(&conn).unwrap();
        conn
    }

    fn seed_file(conn: &Connection, path: &str) {
        conn.execute(
            "INSERT INTO files (path, content_hash, language, last_scanned, byte_size) \
             VALUES (?1, 'h', 'rust', 0, 0)",
            params![path],
        )
        .unwrap();
    }

    #[test]
    fn normalise_rel_path_handles_root_and_dot_prefix() {
        assert_eq!(normalise_rel_path("."), "");
        assert_eq!(normalise_rel_path("./crates/foo"), "crates/foo");
        assert_eq!(normalise_rel_path("crates/foo"), "crates/foo");
    }

    #[test]
    fn discover_and_persist_picks_up_a_rust_project() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        let conn = fresh_db();
        let projects = discover_and_persist_projects(&conn, dir.path()).unwrap();
        assert!(
            projects.iter().any(|p| matches!(p.kind, ProjectKind::Rust)),
            "expected the fixture Rust project, got {projects:?}",
        );
    }

    #[test]
    fn discover_and_persist_recovers_polyglot_monorepo() {
        let dir = tempdir().unwrap();
        // Root: Rust workspace
        fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n",
        )
        .unwrap();
        // Sub: Rust crate
        let core_dir = dir.path().join("crates").join("core");
        fs::create_dir_all(&core_dir).unwrap();
        fs::write(
            core_dir.join("Cargo.toml"),
            "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        // Sub: Bun project
        let ext_dir = dir.path().join("ext").join("vscode");
        fs::create_dir_all(&ext_dir).unwrap();
        fs::write(ext_dir.join("package.json"), "{\"name\":\"ext\"}").unwrap();
        fs::write(ext_dir.join("bun.lock"), "").unwrap();

        let conn = fresh_db();
        let projects = discover_and_persist_projects(&conn, dir.path()).unwrap();

        let kinds: Vec<&ProjectKind> = projects.iter().map(|p| &p.kind).collect();
        assert!(
            kinds.contains(&&ProjectKind::Rust),
            "expected Rust kind, got {kinds:?}"
        );
        assert!(
            kinds.contains(&&ProjectKind::Bun),
            "expected Bun kind, got {kinds:?}"
        );
    }

    #[test]
    fn reconcile_assigns_deepest_project_to_each_file() {
        let conn = fresh_db();
        let root_id = projects::insert_project(
            &conn,
            "root",
            &ProjectKind::Rust,
            "/r",
            "",
        )
        .unwrap();
        let ext_id = projects::insert_project(
            &conn,
            "ext-vscode",
            &ProjectKind::Bun,
            "/r/ext/vscode",
            "ext/vscode",
        )
        .unwrap();

        seed_file(&conn, "crates/core/src/lib.rs");
        seed_file(&conn, "ext/vscode/src/extension.ts");
        seed_file(&conn, "ext/vscode/package.json");

        let updated = reconcile_files_project_id(&conn).unwrap();
        assert_eq!(updated, 3);

        let core_pid: Option<i64> = conn
            .query_row(
                "SELECT project_id FROM files WHERE path = 'crates/core/src/lib.rs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let ext_ts_pid: Option<i64> = conn
            .query_row(
                "SELECT project_id FROM files WHERE path = 'ext/vscode/src/extension.ts'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let ext_pkg_pid: Option<i64> = conn
            .query_row(
                "SELECT project_id FROM files WHERE path = 'ext/vscode/package.json'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        assert_eq!(core_pid, Some(i64::from(root_id)));
        assert_eq!(ext_ts_pid, Some(i64::from(ext_id)));
        assert_eq!(ext_pkg_pid, Some(i64::from(ext_id)));
    }

    /// Custom detector that matches a directory containing a
    /// `shaders/` subfolder. Mirrors the WGSL example in the
    /// `standarbuild-detect` 0.2 docstrings.
    struct ShadersDetector;
    impl standarbuild_detect::Detector for ShadersDetector {
        fn kind(&self) -> KindId {
            KindId::custom("wgsl")
        }
        fn priority(&self) -> i32 {
            120 // beats every built-in (max is Rust=100)
        }
        fn detect(
            &self,
            dir: &std::path::Path,
        ) -> Option<standarbuild_detect::DetectMatch> {
            dir.join("shaders")
                .is_dir()
                .then(|| standarbuild_detect::DetectMatch {
                    kind: KindId::custom("wgsl"),
                    signals: vec!["shaders/".into()],
                })
        }
    }

    #[test]
    fn discover_with_custom_registry_persists_custom_kind() {
        let dir = tempdir().unwrap();
        // Mark the workspace root as a Rust crate so the built-in
        // detector also matches; the WGSL detector's higher priority
        // must win.
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"rs+wgsl\"\nversion = \"0.0.1\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::create_dir(dir.path().join("shaders")).unwrap();

        let mut registry = DetectorRegistry::with_builtins();
        registry.add(ShadersDetector);

        let conn = fresh_db();
        let projects =
            discover_and_persist_projects_with(&conn, dir.path(), &registry).unwrap();
        let kinds: Vec<&ProjectKind> = projects.iter().map(|p| &p.kind).collect();
        assert!(
            kinds.contains(&&ProjectKind::Custom("wgsl".into())),
            "expected the custom WGSL kind to win on priority, got {kinds:?}"
        );
        // Round-trip through storage: re-read by root_path should also
        // carry the Custom variant.
        let row = projects::find_by_root_path(
            &conn,
            &dir.path().to_string_lossy(),
        )
        .unwrap()
        .expect("row present");
        assert_eq!(row.kind, ProjectKind::Custom("wgsl".into()));
    }

    #[test]
    fn reconcile_clears_stale_project_ids_when_projects_removed() {
        let conn = fresh_db();
        let pid = projects::insert_project(
            &conn,
            "old",
            &ProjectKind::Rust,
            "/r/old",
            "old",
        )
        .unwrap();
        seed_file(&conn, "old/lib.rs");
        reconcile_files_project_id(&conn).unwrap();
        // Sanity — the file points at `old` now.
        let pre: Option<i64> = conn
            .query_row(
                "SELECT project_id FROM files WHERE path = 'old/lib.rs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pre, Some(i64::from(pid)));

        // Drop the project, re-run reconcile — the file's project_id
        // must go NULL.
        projects::delete_project(&conn, pid).unwrap();
        reconcile_files_project_id(&conn).unwrap();
        let post: Option<i64> = conn
            .query_row(
                "SELECT project_id FROM files WHERE path = 'old/lib.rs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(post.is_none(), "stale project_id must be cleared");
    }
}

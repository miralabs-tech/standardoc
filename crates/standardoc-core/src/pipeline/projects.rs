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
use standarbuild_detect::{Detector, DetectorHit, DetectorRegistry, KindId, WorkspaceKindId};
use standardoc_ir::{ProjectInfo, ProjectKind, WorkspaceKind};

use crate::storage::error::StorageError;
use crate::storage::projects;
use crate::storage::schema_meta;

/// Re-export of 0.3's [`standarbuild_detect::DetectionResult`] for
/// callers that want both the project list and the workspace-manifest
/// list from a single discovery scan. Stage 3e-3 consumes `.workspaces`
/// at cold-start to persist the primary workspace kind in `schema_meta`.
pub(crate) use standarbuild_detect::DetectionResult;

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

/// Stage 3e-3 — convert the detector's [`WorkspaceKindId`] to the IR's
/// [`WorkspaceKind`]. Built-in variants map 1:1; `Custom(slug)` round-
/// trips verbatim.
fn from_workspace_kind_id(id: &WorkspaceKindId) -> WorkspaceKind {
    match id {
        WorkspaceKindId::Cargo => WorkspaceKind::Cargo,
        WorkspaceKindId::Npm => WorkspaceKind::Npm,
        WorkspaceKindId::Pnpm => WorkspaceKind::Pnpm,
        WorkspaceKindId::Yarn => WorkspaceKind::Yarn,
        WorkspaceKindId::Bun => WorkspaceKind::Bun,
        WorkspaceKindId::Deno => WorkspaceKind::Deno,
        WorkspaceKindId::Go => WorkspaceKind::Go,
        WorkspaceKindId::Lerna => WorkspaceKind::Lerna,
        WorkspaceKindId::Nx => WorkspaceKind::Nx,
        WorkspaceKindId::Turborepo => WorkspaceKind::Turborepo,
        WorkspaceKindId::Mira => WorkspaceKind::Mira,
        WorkspaceKindId::Custom(s) => WorkspaceKind::Custom(s.clone()),
    }
}

/// Stage 3e-3 — pick the primary workspace kind from a [`DetectionResult`].
/// "Primary" = the first detected workspace manifest whose `root` equals
/// the scan `workspace_root`. When multiple workspace kinds coexist at
/// the same root (Tauri = Cargo + Npm), the detector's registration
/// order determines which wins — typically the higher-priority detector
/// fires first. Returns `None` when no workspace manifest is detected
/// at the workspace root (loose project tree or single-crate layout) —
/// the storage layer then clears the `workspace_kind` row rather than
/// recording a sentinel, so `current_revision` can distinguish
/// "detection ran, found no workspace organizer" from "detected as X".
fn primary_workspace_kind(
    detected: &DetectionResult,
    workspace_root: &Path,
) -> Option<WorkspaceKind> {
    detected
        .workspaces
        .iter()
        .find(|w| w.root.as_path() == workspace_root)
        .map(|w| from_workspace_kind_id(&w.kind))
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

/// Discovery with the built-in detector registry (Rust / Node / Bun /
/// Deno / Python / Lua / C / Cpp) plus our locally-registered
/// [`CMakeSubdirDetector`] that recognises canonical CMake layouts
/// (`CMakeLists.txt` + sources under `src/`, `include/`, `tests/`,
/// `lib/`). Shortcut for [`discover_and_persist_projects_with`].
pub(crate) fn discover_and_persist_projects(
    conn: &Connection,
    workspace_root: &Path,
) -> Result<Vec<ProjectInfo>, StorageError> {
    discover_and_persist_projects_with(conn, workspace_root, &default_registry())
}

/// The detector registry used by [`discover_and_persist_projects`] —
/// built-ins (Rust / Node / Bun / Deno / Python / Lua / C / Cpp) plus
/// our [`CMakeSubdirDetector`] overlay.
pub(crate) fn default_registry() -> DetectorRegistry {
    let mut r = DetectorRegistry::with_builtins();
    r.add(CMakeSubdirDetector);
    r
}

/// Custom CMake detector covering the canonical layout that the
/// built-in `CDetector` / `CppDetector` miss:
///
/// ```text
/// project/
/// ├── CMakeLists.txt
/// ├── include/    ← .h / .hpp
/// ├── src/        ← .c / .cpp / .cc / .cxx
/// └── tests/      ← .c / .cpp
/// ```
///
/// Built-in C/Cpp detectors call `has_extension(dir, ext)` which only
/// scans `dir`'s immediate children. A project whose `.c` / `.cpp`
/// sources live exclusively in `src/` (the dominant CMake convention)
/// is therefore invisible to them. This detector peeks the standard
/// subdirectories and emits `KindId::CPP` if any C++ source is seen,
/// `KindId::C` otherwise.
///
/// Priority 25 — beats built-in CppDetector (20) and CDetector (10) so
/// the canonical-layout path wins when both fire; loses to higher-tier
/// language detectors (Rust=100, Bun=80, ...) so a `Cargo.toml +
/// CMakeLists.txt` repo stays primarily Rust.
pub(crate) struct CMakeSubdirDetector;

const CMAKE_CONVENTIONAL_SUBDIRS: &[&str] = &[
    "src", "source", "sources", "include", "lib", "tests", "test",
];
const CPP_EXTS: &[&str] = &["cpp", "cc", "cxx", "C", "c++", "hpp", "hxx", "h++"];
const C_EXTS: &[&str] = &["c", "h"];

impl Detector for CMakeSubdirDetector {
    fn name(&self) -> &'static str {
        "cmake-subdir"
    }

    fn priority(&self) -> i32 {
        25
    }

    fn detect(&self, dir: &Path) -> Option<DetectorHit> {
        if !dir.join("CMakeLists.txt").is_file() {
            return None;
        }
        let mut signals = vec!["CMakeLists.txt".to_string()];
        let mut has_cpp = false;
        let mut has_c = false;
        for sub in CMAKE_CONVENTIONAL_SUBDIRS {
            let p = dir.join(sub);
            if !p.is_dir() {
                continue;
            }
            if subtree_has_any_ext(&p, CPP_EXTS) {
                has_cpp = true;
                signals.push(format!("{sub}/<*.cpp|cc|cxx|hpp>"));
            }
            if subtree_has_any_ext(&p, C_EXTS) {
                has_c = true;
                if !signals
                    .iter()
                    .any(|s| s.starts_with(&format!("{sub}/<*.c")))
                {
                    signals.push(format!("{sub}/<*.c|h>"));
                }
            }
        }
        if !(has_cpp || has_c) {
            return None;
        }
        let kind = if has_cpp { KindId::CPP } else { KindId::C };
        Some(DetectorHit::Project { kind, signals })
    }
}

/// Recursive scan of `dir`'s subtree (bounded depth) for any file
/// whose extension matches one of `exts`. Used by
/// [`CMakeSubdirDetector`] to recognise CMake projects that nest
/// sources under `src/` / `include/` / `tests/`.
fn subtree_has_any_ext(dir: &Path, exts: &[&str]) -> bool {
    // Bounded depth: 4 levels under each canonical subdir keeps the
    // probe cheap even on very large include trees. Most CMake
    // projects nest at most 2 levels (`src/foo/bar.cpp`).
    fn walk(dir: &Path, exts: &[&str], depth: u32) -> bool {
        if depth == 0 {
            return false;
        }
        let Ok(read) = std::fs::read_dir(dir) else {
            return false;
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if walk(&path, exts, depth - 1) {
                    return true;
                }
                continue;
            }
            if let Some(ext) = path.extension().and_then(|e| e.to_str())
                && exts.contains(&ext)
            {
                return true;
            }
        }
        false
    }
    walk(dir, exts, 4)
}

/// Run discovery against a custom [`DetectorRegistry`], UPSERTing every
/// detected project into `projects` and returning the canonicalised list
/// (with assigned `project_id`s). Use this to extend the detection space
/// with user-registered detectors (e.g. a WGSL detector that matches on
/// `shaders/` directories with `priority() > 100` to override built-ins).
///
/// Idempotent: re-running picks up new projects + updates label/kind
/// drift on existing roots without losing `project_id` continuity.
pub(crate) fn discover_and_persist_projects_with(
    conn: &Connection,
    workspace_root: &Path,
    registry: &DetectorRegistry,
) -> Result<Vec<ProjectInfo>, StorageError> {
    // Bug E-3 follow-up — when `standardoc.sxd` declares explicit
    // `project` blocks, REPLACE the mechanical detection entirely. The
    // declared paths become the authoritative project list ; mechanical
    // workspace_kind sniffing is skipped (no manifest is implied). The
    // workspace is still organized — by the config itself — so record
    // `Custom("sxd")` rather than clearing the row, so `current_revision`
    // surfaces `workspace.kind = "custom:sxd"` instead of a bare `null`
    // that reads as "nothing detected".
    if let Ok(Some(cfg)) = crate::config::load_workspace_config(workspace_root)
        && !cfg.projects.is_empty()
    {
        let _ = schema_meta::write_workspace_kind(conn, &WorkspaceKind::Custom("sxd".into()));
        return persist_sxd_projects(conn, workspace_root, &cfg.projects);
    }

    let detected = discover_workspace_with(workspace_root, registry);
    // Stage 3e-3: sync the persisted `workspace_kind` row to the detected
    // state BEFORE returning so a partial early-exit (e.g. cold_start
    // crash mid-walk) still leaves the kind recorded for next-boot
    // diagnostics. Some → upsert ; None → delete (clears legacy
    // `"single"` rows from pre-revert 3e-3 builds AND records the
    // "no organizer detected" signal as row-absence). Best-effort: an
    // error here doesn't fail the whole discovery pass, projects are
    // the load-bearing output.
    match primary_workspace_kind(&detected, workspace_root) {
        Some(kind) => {
            let _ = schema_meta::write_workspace_kind(conn, &kind);
        }
        None => {
            let _ = schema_meta::delete_workspace_kind(conn);
        }
    }
    persist_projects(conn, detected.projects)
}

/// Bug E-3 follow-up — sxd-driven project persistence. One `projects`
/// row per `(block, path)` pair ; `label` is shared across paths of
/// the same block so downstream consumers (viz Overview) can collapse
/// them by label. `kind` defaults to `Custom(slug)` since the user
/// asserted these paths as projects regardless of inner detection.
fn persist_sxd_projects(
    conn: &Connection,
    workspace_root: &Path,
    blocks: &[crate::config::ProjectBlock],
) -> Result<Vec<ProjectInfo>, StorageError> {
    let mut out: Vec<ProjectInfo> = Vec::new();
    for block in blocks {
        let label = block.label.clone().unwrap_or_else(|| block.slug.clone());
        let kind = ProjectKind::Custom(block.slug.clone());
        for path in &block.paths {
            let rel_path = normalise_rel_path(path);
            let abs = workspace_root.join(&rel_path);
            let root_path = abs.to_string_lossy().into_owned();
            let project_id = projects::upsert_project(conn, &label, &kind, &root_path, &rel_path)?;
            out.push(ProjectInfo {
                project_id,
                label: label.clone(),
                kind: kind.clone(),
                root_path,
                rel_path,
            });
        }
    }
    Ok(out)
}

/// Run `standarbuild_detect::discover` against `workspace_root` and return
/// the full [`DetectionResult`] — both projects and workspace manifests.
/// [`discover_and_persist_projects_with`] consumes `.workspaces` to seed
/// the primary `workspace_kind` in `schema_meta`, then persists `.projects`.
pub(crate) fn discover_workspace_with(
    workspace_root: &Path,
    registry: &DetectorRegistry,
) -> DetectionResult {
    let opts = standarbuild_detect::DiscoverOptions::default();
    standarbuild_detect::discover_with(workspace_root, &opts, registry)
}

/// Persist a list of [`standarbuild_detect::ProjectInfo`] entries into
/// the `projects` table, returning the IR-flavored `ProjectInfo` with
/// freshly-assigned `project_id`s. Filters out the `UNKNOWN` sentinel
/// (see [`discover_and_persist_projects_with`] for why).
fn persist_projects(
    conn: &Connection,
    projects_in: Vec<standarbuild_detect::ProjectInfo>,
) -> Result<Vec<ProjectInfo>, StorageError> {
    let mut out = Vec::with_capacity(projects_in.len());
    for d in projects_in {
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
        let project_id = projects::upsert_project(conn, &d.label, &kind, &root_path, &rel_path)?;
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
pub(crate) fn reconcile_files_project_id(conn: &Connection) -> Result<usize, StorageError> {
    // Two SQL statements: clear first (so unlinked projects' files
    // become NULL again), then re-assign by deepest match. The prefix
    // test uses substr (not LIKE) so `_` / `%` in a project rel_path
    // can't act as wildcards and capture sibling directories.
    conn.execute("UPDATE files SET project_id = NULL", [])?;
    let updated = conn.execute(
        "UPDATE files SET project_id = ( \
            SELECT p.project_id FROM projects p \
            WHERE p.rel_path = '' \
               OR files.path = p.rel_path \
               OR substr(files.path, 1, length(p.rel_path) + 1) = p.rel_path || '/' \
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
    fn stage3e3_cargo_workspace_root_persists_workspace_kind_cargo() {
        // Root with `[workspace] members = [...]` → primary workspace
        // kind = Cargo, persisted to `schema_meta.workspace_kind`.
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n",
        )
        .unwrap();
        let core_dir = dir.path().join("crates").join("core");
        fs::create_dir_all(&core_dir).unwrap();
        fs::write(
            core_dir.join("Cargo.toml"),
            "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();

        let conn = fresh_db();
        discover_and_persist_projects(&conn, dir.path()).unwrap();

        let persisted = schema_meta::read_workspace_kind(&conn)
            .unwrap()
            .expect("workspace_kind must be persisted post-discovery");
        assert_eq!(persisted, WorkspaceKind::Cargo);
    }

    #[test]
    fn stage3e3_loose_project_tree_persists_no_workspace_kind() {
        // No workspace manifest at root — single-crate layout records
        // `None` rather than a sentinel variant. The row is intentionally
        // absent so `current_revision.workspace.kind == null` becomes the
        // signal "detection ran, found no organizer". (Post-revert of
        // `WorkspaceKind::Single` — aligns with standarbuild-detect 0.3
        // which has no `Single` variant.)
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"solo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();

        let conn = fresh_db();
        discover_and_persist_projects(&conn, dir.path()).unwrap();

        let persisted = schema_meta::read_workspace_kind(&conn).unwrap();
        assert!(
            persisted.is_none(),
            "loose project tree must leave workspace_kind row absent, got {persisted:?}"
        );
    }

    #[test]
    fn stage3e3_legacy_single_row_is_purged_on_rediscovery() {
        // Simulates a pre-revert DB carrying `workspace_kind = "single"`.
        // Re-running discovery against a loose tree must DELETE the row
        // (not leave it stuck as `Custom("single")`).
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"solo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();

        let conn = fresh_db();
        conn.execute(
            "INSERT INTO schema_meta (key, value) VALUES ('workspace_kind', 'single')",
            [],
        )
        .unwrap();
        discover_and_persist_projects(&conn, dir.path()).unwrap();
        let persisted = schema_meta::read_workspace_kind(&conn).unwrap();
        assert!(
            persisted.is_none(),
            "legacy `single` row must be purged on rediscovery, got {persisted:?}"
        );
    }

    #[test]
    fn reconcile_assigns_deepest_project_to_each_file() {
        let conn = fresh_db();
        let root_id =
            projects::insert_project(&conn, "root", &ProjectKind::Rust, "/r", "").unwrap();
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
    /// `standarbuild-detect` 0.3 docstrings (post-API rename: `kind()`
    /// became `name()`, `DetectMatch` became `DetectorHit::Project`).
    struct ShadersDetector;
    impl Detector for ShadersDetector {
        fn name(&self) -> &'static str {
            "wgsl"
        }
        fn priority(&self) -> i32 {
            120 // beats every built-in (max is Rust=100)
        }
        fn detect(&self, dir: &Path) -> Option<DetectorHit> {
            dir.join("shaders").is_dir().then(|| DetectorHit::Project {
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
        let projects = discover_and_persist_projects_with(&conn, dir.path(), &registry).unwrap();
        let kinds: Vec<&ProjectKind> = projects.iter().map(|p| &p.kind).collect();
        assert!(
            kinds.contains(&&ProjectKind::Custom("wgsl".into())),
            "expected the custom WGSL kind to win on priority, got {kinds:?}"
        );
        // Round-trip through storage: re-read by root_path should also
        // carry the Custom variant.
        let row = projects::find_by_root_path(&conn, &dir.path().to_string_lossy())
            .unwrap()
            .expect("row present");
        assert_eq!(row.kind, ProjectKind::Custom("wgsl".into()));
    }

    #[test]
    fn cmake_detector_recognises_runtime_layout_with_sources_under_src() {
        // The canonical LurLang runtime/ layout: CMakeLists.txt at
        // root + sources nested under src/include/tests. The built-in
        // C/Cpp detectors miss this (their has_extension is
        // immediate-children-only); the local CMakeSubdirDetector
        // wired into default_registry() picks it up.
        let dir = tempdir().unwrap();
        let root = dir.path().join("runtime");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("include")).unwrap();
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::write(root.join("CMakeLists.txt"), "project(runtime C)").unwrap();
        fs::write(root.join("src").join("vm.c"), "int main(){return 0;}").unwrap();
        fs::write(root.join("include").join("vm.h"), "#pragma once").unwrap();
        fs::write(root.join("tests").join("smoke.c"), "int main(){return 0;}").unwrap();

        let conn = fresh_db();
        let projects = discover_and_persist_projects(&conn, dir.path()).unwrap();
        let runtime = projects
            .iter()
            .find(|p| p.rel_path == "runtime")
            .unwrap_or_else(|| panic!("runtime/ not detected: {projects:?}"));
        assert_eq!(runtime.kind, ProjectKind::C);
    }

    #[test]
    fn cmake_detector_picks_cpp_when_any_cpp_source_present() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("engine");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("CMakeLists.txt"), "project(engine CXX)").unwrap();
        fs::write(
            root.join("src").join("renderer.cpp"),
            "int main(){return 0;}",
        )
        .unwrap();
        fs::write(root.join("src").join("util.c"), "int x;").unwrap();

        let conn = fresh_db();
        let projects = discover_and_persist_projects(&conn, dir.path()).unwrap();
        let engine = projects
            .iter()
            .find(|p| p.rel_path == "engine")
            .unwrap_or_else(|| panic!("engine/ not detected: {projects:?}"));
        assert_eq!(engine.kind, ProjectKind::Cpp);
    }

    #[test]
    fn cmake_detector_does_not_fire_without_cmakelists() {
        // src/ with .c files but no CMakeLists.txt at root => not a
        // CMake project. The built-in C detector also requires the
        // manifest, so this stays Unknown / unmatched.
        let dir = tempdir().unwrap();
        let root = dir.path().join("loose");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src").join("orphan.c"), "int x;").unwrap();

        let conn = fresh_db();
        let projects = discover_and_persist_projects(&conn, dir.path()).unwrap();
        assert!(
            !projects.iter().any(|p| p.rel_path == "loose"),
            "loose/ must not be detected as a project without CMakeLists.txt: {projects:?}"
        );
    }

    #[test]
    fn cmake_detector_does_not_fire_when_no_sources_in_canonical_subdirs() {
        // CMakeLists.txt but nothing in src/include/tests => header-
        // only library with weird layout, or stale scaffolding. We
        // intentionally don't claim it: the built-in detectors won't
        // either, so the project stays unknown rather than producing a
        // mis-typed C/Cpp row.
        let dir = tempdir().unwrap();
        let root = dir.path().join("empty");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("CMakeLists.txt"), "project(empty)").unwrap();

        let conn = fresh_db();
        let projects = discover_and_persist_projects(&conn, dir.path()).unwrap();
        assert!(
            !projects.iter().any(|p| p.rel_path == "empty"),
            "empty CMake project must not be detected as C or Cpp: {projects:?}"
        );
    }

    #[test]
    fn reconcile_clears_stale_project_ids_when_projects_removed() {
        let conn = fresh_db();
        let pid =
            projects::insert_project(&conn, "old", &ProjectKind::Rust, "/r/old", "old").unwrap();
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

    #[test]
    fn reconcile_does_not_misattribute_underscore_wildcard_siblings() {
        // PR1 regression: `rel_path` must be matched as a literal prefix,
        // not a LIKE pattern. A project at `my_lib` must NOT capture a
        // sibling `myXlib/` — under the old `LIKE rel_path || '/%'` the
        // `_` matched any char (`X`) and mis-attributed the sibling.
        let conn = fresh_db();
        let lib_id =
            projects::insert_project(&conn, "my_lib", &ProjectKind::Rust, "/r/my_lib", "my_lib")
                .unwrap();
        seed_file(&conn, "my_lib/src/lib.rs");
        seed_file(&conn, "myXlib/src/lib.rs");

        reconcile_files_project_id(&conn).unwrap();

        let inside: Option<i64> = conn
            .query_row(
                "SELECT project_id FROM files WHERE path = 'my_lib/src/lib.rs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let sibling: Option<i64> = conn
            .query_row(
                "SELECT project_id FROM files WHERE path = 'myXlib/src/lib.rs'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        assert_eq!(
            inside,
            Some(i64::from(lib_id)),
            "file under my_lib must link"
        );
        assert!(
            sibling.is_none(),
            "myXlib/ sibling must NOT be captured by my_lib's rel_path (no LIKE wildcard)"
        );
    }

    // --- Bug E-3 follow-up : sxd-driven REPLACE semantics ---

    fn write_sxd(root: &Path, body: &str) {
        fs::write(root.join("standardoc.sxd"), body).unwrap();
    }

    #[test]
    fn sxd_with_projects_short_circuits_mechanical_detection() {
        let dir = tempdir().unwrap();
        // Mechanical detection would pick up this Rust crate.
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"unwanted\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        // Sxd declares ONE custom project that doesn't match the cargo manifest.
        write_sxd(
            dir.path(),
            r#"version "0.1.0"
project "custom" {
  label "Custom Bundle"
  path "subdir"
}
"#,
        );

        let conn = fresh_db();
        let projects = discover_and_persist_projects(&conn, dir.path()).unwrap();

        assert_eq!(projects.len(), 1, "exactly one sxd-declared project");
        let p = &projects[0];
        assert_eq!(p.label, "Custom Bundle");
        assert_eq!(p.rel_path, "subdir");
        assert!(matches!(&p.kind, ProjectKind::Custom(slug) if slug == "custom"));
        // Mechanical Rust kind absent — proves the bypass.
        assert!(
            !projects.iter().any(|p| matches!(p.kind, ProjectKind::Rust)),
            "mechanical detection must NOT run when sxd has projects, got {projects:?}",
        );
    }

    #[test]
    fn sxd_multi_path_project_yields_one_row_per_path_shared_label() {
        let dir = tempdir().unwrap();
        write_sxd(
            dir.path(),
            r#"version "0.1.0"
project "standardoc" {
  label "Standardoc"
  paths ["crates" "ext/vscode"]
}
"#,
        );

        let conn = fresh_db();
        let projects = discover_and_persist_projects(&conn, dir.path()).unwrap();

        assert_eq!(projects.len(), 2);
        assert!(projects.iter().all(|p| p.label == "Standardoc"));
        let mut paths: Vec<&str> = projects.iter().map(|p| p.rel_path.as_str()).collect();
        paths.sort_unstable();
        assert_eq!(paths, vec!["crates", "ext/vscode"]);
    }

    #[test]
    fn sxd_label_fallback_to_slug_when_absent() {
        let dir = tempdir().unwrap();
        write_sxd(
            dir.path(),
            r#"version "0.1.0"
project "naked" { path "x" }
"#,
        );

        let conn = fresh_db();
        let projects = discover_and_persist_projects(&conn, dir.path()).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].label, "naked");
    }

    #[test]
    fn sxd_without_projects_falls_back_to_mechanical_detection() {
        let dir = tempdir().unwrap();
        // Cargo manifest present — mechanical detection should fire.
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        // Sxd with ONLY ignore block — no project blocks → no override.
        write_sxd(
            dir.path(),
            r#"version "0.1.0"
ignore { patterns ```
.git/
``` }
"#,
        );

        let conn = fresh_db();
        let projects = discover_and_persist_projects(&conn, dir.path()).unwrap();
        assert!(
            projects.iter().any(|p| matches!(p.kind, ProjectKind::Rust)),
            "mechanical Rust detection must still fire, got {projects:?}",
        );
    }

    #[test]
    fn sxd_reconcile_files_attaches_to_sxd_project_id() {
        let dir = tempdir().unwrap();
        write_sxd(
            dir.path(),
            r#"version "0.1.0"
project "standardoc" {
  label "Standardoc"
  paths ["crates" "ext/vscode"]
}
"#,
        );
        let conn = fresh_db();
        let projects = discover_and_persist_projects(&conn, dir.path()).unwrap();
        assert_eq!(projects.len(), 2);
        seed_file(&conn, "crates/foo/src/lib.rs");
        seed_file(&conn, "ext/vscode/src/extension.ts");
        reconcile_files_project_id(&conn).unwrap();

        let pid_for = |path: &str| -> Option<u32> {
            conn.query_row(
                "SELECT project_id FROM files WHERE path = ?1",
                params![path],
                |r| r.get::<_, Option<u32>>(0),
            )
            .unwrap()
        };
        let labels_pid: std::collections::HashMap<u32, String> = projects
            .iter()
            .map(|p| (p.project_id, p.label.clone()))
            .collect();

        let crates_pid = pid_for("crates/foo/src/lib.rs").expect("crates file linked");
        let ext_pid = pid_for("ext/vscode/src/extension.ts").expect("ext file linked");
        assert_eq!(labels_pid[&crates_pid], "Standardoc");
        assert_eq!(labels_pid[&ext_pid], "Standardoc");
    }
}

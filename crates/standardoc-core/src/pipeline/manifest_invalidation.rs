//! Stage 3d-5 — manifest-driven workspace project re-detection.
//!
//! When a project / workspace manifest changes on disk (a new sub-crate is
//! added, `package.json` workspaces field is edited, `pnpm-workspace.yaml`
//! drops a member, …), the `projects` table + `schema_meta.workspace_kind`
//! must be re-synced so the watcher's file→project reconciliation lands on
//! the right rows and `current_revision.workspace.kind` keeps reporting the
//! truth.
//!
//! ## Wired into
//!
//! `pipeline::watcher::process_path` calls [`handle_manifest_change`]
//! after the lockfile invalidation step (which handles dependency-tree
//! changes, see [`crate::pipeline::external_invalidation`]) and BEFORE
//! the supported-extension filter. Returns `Ok(Some(()))` to signal a
//! manifest hit so the caller short-circuits — manifest files are never
//! indexed as workspace source.
//!
//! ## Filename list
//!
//! Sourced from `standarbuild-detect 0.3` built-in detectors plus the
//! Mira `*.sxb` marker. Adding a new manifest filename is additive: edit
//! [`MANIFEST_FILENAMES`] / [`MANIFEST_EXTENSIONS`], no migration needed.
//!
//! ## Scope (v1)
//!
//! Any change to any matching filename anywhere in the workspace tree
//! triggers a full re-run of [`crate::pipeline::projects::discover_and_persist_projects`].
//! The detection is workspace-wide by design — even a sub-project manifest
//! change can reshape the project graph (new member, kind drift). The
//! debouncer collapses bursts of writes so a `cargo new` (which writes
//! `Cargo.toml` + scaffolds) only fires one detection pass.

use std::path::Path;

use crate::pipeline::projects::{discover_and_persist_projects, reconcile_files_project_id};
use crate::storage::error::StorageError;
use crate::storage::handle::IndexHandle;

/// Manifest filenames (exact match) that trigger workspace re-detection.
/// Sourced from `standarbuild-detect 0.3` built-in detectors + workspace
/// orchestrators.
pub(crate) const MANIFEST_FILENAMES: &[&str] = &[
    // Rust — workspace + project marker (same file, both roles).
    "Cargo.toml",
    // JS/TS — npm / pnpm / yarn / bun project + workspace markers.
    "package.json",
    "pnpm-workspace.yaml",
    "bun.lock",
    "bun.lockb",
    "bunfig.toml",
    // Deno — project marker (both extensions).
    "deno.json",
    "deno.jsonc",
    // Go — workspace marker.
    "go.work",
    // JS monorepo orchestrators — workspace markers.
    "lerna.json",
    "nx.json",
    "turbo.json",
    // Python — project markers.
    "pyproject.toml",
    "requirements.txt",
    "setup.py",
    "setup.cfg",
    // Lua — workspace marker (LuaCATS / EmmyLua config).
    ".luarc.json",
];

/// Suffix patterns for manifests with variable basenames. Currently:
/// `*.sxb` (Mira `standarbuild` manifest, ID Stage 3e-3-bis).
pub(crate) const MANIFEST_EXTENSIONS: &[&str] = &["sxb"];

/// `true` when `path`'s file-name matches one of [`MANIFEST_FILENAMES`]
/// or its extension matches one of [`MANIFEST_EXTENSIONS`]. The caller
/// owns the path-resolution + IO; this is pure name inspection.
#[must_use]
pub(crate) fn is_manifest_file(path: &Path) -> bool {
    if let Some(name) = path.file_name().and_then(|n| n.to_str())
        && MANIFEST_FILENAMES.contains(&name)
    {
        return true;
    }
    if let Some(ext) = path.extension().and_then(|e| e.to_str())
        && MANIFEST_EXTENSIONS.contains(&ext)
    {
        return true;
    }
    false
}

/// Re-run workspace project discovery + file→project reconciliation when
/// `abs_path` matches a known manifest filename. Returns `Ok(Some(()))`
/// to signal the watcher to short-circuit (manifests aren't source),
/// `Ok(None)` when the path doesn't match a manifest filename so the
/// caller can fall through to the normal source-file handling.
///
/// Failure modes:
/// - `Err(StorageError::Pool)` when the connection pool is exhausted /
///   poisoned — the caller logs and returns; the watcher loop continues.
/// - `Err(StorageError::Sqlite)` from the underlying re-detection SQL
///   (UPSERT into `projects`, schema_meta write). Same handling.
pub(crate) fn handle_manifest_change(
    handle: &IndexHandle,
    workspace_root: &Path,
    abs_path: &Path,
) -> Result<Option<()>, StorageError> {
    if !is_manifest_file(abs_path) {
        return Ok(None);
    }
    let pool = handle.pool()?;
    let conn = pool.get()?;
    discover_and_persist_projects(&conn, workspace_root)?;
    reconcile_files_project_id(&conn)?;
    Ok(Some(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn is_manifest_file_matches_one_filename_per_ecosystem() {
        // Coverage check — one positive per ecosystem so a slip in
        // [`MANIFEST_FILENAMES`] (lowercased typo, dropped entry) surfaces
        // fast in CI.
        assert!(is_manifest_file(&PathBuf::from("/ws/Cargo.toml")));
        assert!(is_manifest_file(&PathBuf::from("/ws/package.json")));
        assert!(is_manifest_file(&PathBuf::from("/ws/pnpm-workspace.yaml")));
        assert!(is_manifest_file(&PathBuf::from("/ws/bun.lock")));
        assert!(is_manifest_file(&PathBuf::from("/ws/deno.json")));
        assert!(is_manifest_file(&PathBuf::from("/ws/go.work")));
        assert!(is_manifest_file(&PathBuf::from("/ws/turbo.json")));
        assert!(is_manifest_file(&PathBuf::from("/ws/pyproject.toml")));
        assert!(is_manifest_file(&PathBuf::from("/ws/.luarc.json")));
    }

    #[test]
    fn is_manifest_file_matches_sxb_extension() {
        // `*.sxb` is the only variable-basename manifest in v1. Any
        // basename qualifies as long as the extension is `sxb`.
        assert!(is_manifest_file(&PathBuf::from("/ws/standardoc.sxb")));
        assert!(is_manifest_file(&PathBuf::from("/ws/sub/nested.sxb")));
    }

    #[test]
    fn is_manifest_file_rejects_source_files() {
        // Source files MUST NOT trigger manifest re-detection — they
        // route through `has_supported_extension` instead.
        assert!(!is_manifest_file(&PathBuf::from("/ws/src/lib.rs")));
        assert!(!is_manifest_file(&PathBuf::from("/ws/src/index.ts")));
        assert!(!is_manifest_file(&PathBuf::from("/ws/src/main.lua")));
        assert!(!is_manifest_file(&PathBuf::from("/ws/App.vue")));
    }

    #[test]
    fn is_manifest_file_rejects_lockfiles() {
        // Lockfiles are handled by `external_invalidation::handle_lockfile_change`,
        // NOT by manifest re-detection. The two handlers have orthogonal
        // semantics: lockfile changes purge cached externals; manifest
        // changes reshape the projects table.
        assert!(!is_manifest_file(&PathBuf::from("/ws/Cargo.lock")));
        assert!(!is_manifest_file(&PathBuf::from("/ws/package-lock.json")));
        assert!(!is_manifest_file(&PathBuf::from("/ws/yarn.lock")));
        assert!(!is_manifest_file(&PathBuf::from("/ws/pnpm-lock.yaml")));
    }

    #[test]
    fn is_manifest_file_is_case_sensitive() {
        // The detector lib matches case-sensitive — mirror that here so
        // a Windows user-typed `cargo.toml` doesn't silently trigger
        // false positives.
        assert!(!is_manifest_file(&PathBuf::from("/ws/CARGO.TOML")));
        assert!(!is_manifest_file(&PathBuf::from("/ws/Package.json")));
    }

    // --- e2e: handle_manifest_change roundtrip against a real IndexHandle ---

    use crate::storage::handle::IndexHandle;
    use crate::storage::projects::find_by_root_path;
    use crate::storage::schema_meta::read_workspace_kind;
    use standardoc_ir::WorkspaceKind;
    use std::fs;
    use tempfile::tempdir;

    fn fresh_handle() -> (tempfile::TempDir, IndexHandle) {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        (dir, handle)
    }

    #[test]
    fn stage3d5_non_manifest_path_returns_none_and_does_not_re_detect() {
        // `src/lib.rs` is a source file — handler must short-circuit to
        // `None` so the watcher falls through to source upsert.
        let (dir, handle) = fresh_handle();
        let abs_path = dir.path().join("src/lib.rs");
        let result = handle_manifest_change(&handle, dir.path(), &abs_path).unwrap();
        assert!(result.is_none());
        // Nothing got written — workspace_kind is still absent.
        let conn = handle.pool().unwrap().get().unwrap();
        assert!(read_workspace_kind(&conn).unwrap().is_none());
    }

    #[test]
    fn stage3d5_cargo_workspace_manifest_change_re_runs_discovery() {
        // A `Cargo.toml` with `[workspace]` lands in the projects table
        // and `schema_meta.workspace_kind = "cargo"` after the handler
        // fires. Mirrors the cold-start contract.
        let (_dir, handle) = fresh_handle();
        // Use the IndexHandle's canonicalized workspace_root so the
        // discover-side absolute paths match the test-side lookup paths
        // (Windows: tempdir() returns an uncanonicalized path).
        let root = handle.workspace_root().to_path_buf();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("crates/core")).unwrap();
        fs::write(
            root.join("crates/core/Cargo.toml"),
            "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();

        let result = handle_manifest_change(&handle, &root, &root.join("Cargo.toml")).unwrap();
        assert_eq!(result, Some(()));

        let conn = handle.pool().unwrap().get().unwrap();
        assert_eq!(
            read_workspace_kind(&conn).unwrap(),
            Some(WorkspaceKind::Cargo)
        );
        // The sub-crate should land as a `Rust` project keyed by its
        // absolute path.
        let core_root = root.join("crates/core").to_string_lossy().into_owned();
        let row = find_by_root_path(&conn, &core_root).unwrap();
        assert!(
            row.is_some(),
            "sub-crate must be persisted after manifest re-detection"
        );
    }

    #[test]
    fn stage3d5_newly_added_subproject_manifest_is_picked_up_on_re_run() {
        // Simulate the "user runs `cargo new crates/foo`" flow: cold-
        // start sees workspace + 1 member, then a new sub-crate manifest
        // is created, watcher fires the handler, projects table picks
        // up the new project on the second pass.
        let (_dir, handle) = fresh_handle();
        let root = handle.workspace_root().to_path_buf();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("crates/alpha")).unwrap();
        fs::write(
            root.join("crates/alpha/Cargo.toml"),
            "[package]\nname = \"alpha\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        handle_manifest_change(&handle, &root, &root.join("Cargo.toml")).unwrap();

        // Pre-condition: only `alpha` is known.
        let conn = handle.pool().unwrap().get().unwrap();
        let alpha_root = root.join("crates/alpha").to_string_lossy().into_owned();
        let beta_root = root.join("crates/beta").to_string_lossy().into_owned();
        assert!(find_by_root_path(&conn, &alpha_root).unwrap().is_some());
        assert!(find_by_root_path(&conn, &beta_root).unwrap().is_none());
        drop(conn);

        // Add `beta` and re-fire the handler against its new manifest.
        fs::create_dir_all(root.join("crates/beta")).unwrap();
        let beta_manifest = root.join("crates/beta/Cargo.toml");
        fs::write(
            &beta_manifest,
            "[package]\nname = \"beta\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        let result = handle_manifest_change(&handle, &root, &beta_manifest).unwrap();
        assert_eq!(result, Some(()));

        let conn = handle.pool().unwrap().get().unwrap();
        assert!(
            find_by_root_path(&conn, &beta_root).unwrap().is_some(),
            "beta sub-crate must surface after the second handler fire"
        );
    }

    #[test]
    fn stage3d5_sxb_manifest_triggers_re_detection() {
        // `*.sxb` is the variable-basename manifest case. Any `.sxb`
        // file in the workspace must route through the handler so Mira
        // self-hosting workspaces stay in sync.
        let (dir, handle) = fresh_handle();
        let root = dir.path();
        fs::write(root.join("standardoc.sxb"), b"# stub Mira manifest\n").unwrap();
        let result = handle_manifest_change(&handle, root, &root.join("standardoc.sxb")).unwrap();
        // The handler fires; whether `standarbuild-detect` actually
        // recognises this synthetic .sxb stub is detector-side. We
        // just assert the routing decision.
        assert_eq!(result, Some(()));
    }

    #[test]
    fn stage3d5_lockfile_paths_route_to_none() {
        // Defensive check — lockfiles MUST NOT be handled by this
        // module (they have their own pipeline in `external_invalidation`).
        // Same overlap rationale as the unit-level `is_manifest_file_rejects_lockfiles`
        // test, but verified at the handler level.
        let (dir, handle) = fresh_handle();
        let root = dir.path();
        let cargo_lock = root.join("Cargo.lock");
        assert!(
            handle_manifest_change(&handle, root, &cargo_lock)
                .unwrap()
                .is_none()
        );
    }
}

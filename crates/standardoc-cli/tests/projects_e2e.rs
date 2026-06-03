//! Stage 3d-3 — End-to-end dogfood of the polyglot project detection
//! + persistence chain.
//!
//! Builds a fixture workspace mirroring the standardoc repo shape
//! (Rust root crate + Bun-flavoured ext/vscode sub-project), runs the
//! full `cold_start::run` pipeline (via a mock provider so we don't
//! need actual source files extracted), then exercises the public
//! `query::projects::*` surface that the Stage 3d-3 MCP tools wire on
//! top.

use std::fs;
use std::sync::Arc;

use standardoc_core::{
    IndexHandle, LanguageProvider, ScanFilters, cold_start,
    query::projects::{list_projects, project_for_file},
};
use standardoc_ir::ProjectKind;
use standardoc_lang_provider::WorkspaceProvider;

#[test]
fn cold_start_populates_projects_and_attaches_files_in_polyglot_monorepo() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    // Root: Rust crate (workspace-root project).
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"root-rust\"\nversion = \"0.0.1\"\nedition = \"2024\"\n\n\
         [lib]\npath = \"src/lib.rs\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn nothing() {}\n").unwrap();

    // Sub: Bun-flavoured VSCode extension.
    let ext_dir = root.join("ext").join("vscode");
    fs::create_dir_all(&ext_dir).unwrap();
    fs::write(
        ext_dir.join("package.json"),
        "{\"name\":\"ext-vscode\",\"version\":\"0.0.1\"}",
    )
    .unwrap();
    fs::write(ext_dir.join("bun.lock"), "").unwrap();

    // Run the real cold-start pipeline. We use the WorkspaceProvider
    // (real Rust/TS extractors); the .rs file gets indexed and
    // attached to its project. Errors would surface here.
    let handle = IndexHandle::open(root).expect("open IndexHandle");
    let provider: Arc<dyn LanguageProvider> = Arc::new(WorkspaceProvider::new());
    let filters = ScanFilters::load(root);
    cold_start::run(&handle, provider.as_ref(), &filters).expect("cold_start");

    // Project listing reports both Rust + Bun (Unknown variants filtered).
    let projects = list_projects(&handle).expect("list_projects");
    let kinds: Vec<&ProjectKind> = projects.iter().map(|p| &p.kind).collect();
    assert!(
        kinds.contains(&&ProjectKind::Rust),
        "expected Rust project in {projects:?}"
    );
    assert!(
        kinds.contains(&&ProjectKind::Bun),
        "expected Bun project in {projects:?}"
    );
    // Root project's rel_path is the empty string (workspace root).
    assert!(
        projects.iter().any(|p| p.rel_path.is_empty()),
        "expected a workspace-root project with rel_path = '', got {projects:?}"
    );

    // `project_for_file` returns the deepest ancestor — a path inside
    // ext/vscode/ lands on the Bun project, not the root Rust one.
    let canonical_root = fs::canonicalize(root).unwrap();
    let ext_file = canonical_root.join("ext/vscode/package.json");
    let hit = project_for_file(&handle, &ext_file.to_string_lossy())
        .expect("project_for_file")
        .expect("ext/vscode/package.json must resolve");
    assert_eq!(hit.kind, ProjectKind::Bun, "deepest-match must pick Bun");

    // A path inside src/ lands on the workspace-root Rust project.
    let rust_file = canonical_root.join("src/lib.rs");
    let hit = project_for_file(&handle, &rust_file.to_string_lossy())
        .expect("project_for_file")
        .expect("src/lib.rs must resolve");
    assert_eq!(hit.kind, ProjectKind::Rust);
    assert!(hit.rel_path.is_empty(), "root project rel_path");

    // Path outside any project returns None.
    let outsider = project_for_file(&handle, "/no/such/file.rs").expect("project_for_file");
    assert!(outsider.is_none());
}

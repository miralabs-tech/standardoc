use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use standardoc_lang_provider::WorkspaceProvider;
use standardoc_server::{index_once, open_workspace, query, rescan};

fn write_sample_workspace(root: &Path) {
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/lib.rs"),
        "pub fn alpha() {}\npub fn beta() { alpha(); }\n",
    )
    .unwrap();
}

#[test]
fn index_once_indexes_workspace_and_returns_queryable_handle() {
    let dir = tempfile::tempdir().unwrap();
    write_sample_workspace(dir.path());

    let provider = WorkspaceProvider::new();
    let handle = index_once(dir.path(), &provider).unwrap();

    let symbols = query::symbols_by_file(&handle, "src/lib.rs").unwrap();
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"alpha"), "got {names:?}");
    assert!(names.contains(&"beta"), "got {names:?}");
}

#[test]
fn index_once_resolves_intra_file_call_edge() {
    let dir = tempfile::tempdir().unwrap();
    write_sample_workspace(dir.path());

    let provider = WorkspaceProvider::new();
    let handle = index_once(dir.path(), &provider).unwrap();

    let edges = query::edges_from(&handle, "sample::beta").unwrap();
    let alpha_call = edges
        .iter()
        .find(|e| matches!(&e.to, standardoc_ir::ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "sample::alpha"));
    assert!(
        alpha_call.is_some(),
        "expected resolved CALL beta -> alpha, got {edges:#?}"
    );
}

#[test]
fn rescan_recreates_index_from_scratch() {
    let dir = tempfile::tempdir().unwrap();
    write_sample_workspace(dir.path());

    let provider = WorkspaceProvider::new();
    let handle = index_once(dir.path(), &provider).unwrap();
    assert!(
        query::symbol_by_fqdn(&handle, "sample::alpha")
            .unwrap()
            .is_some()
    );

    rescan(&handle, &provider).unwrap();
    assert!(
        query::symbol_by_fqdn(&handle, "sample::alpha")
            .unwrap()
            .is_some()
    );
}

#[test]
fn open_workspace_returns_running_server_with_queryable_handle() {
    let dir = tempfile::tempdir().unwrap();
    write_sample_workspace(dir.path());

    let provider: Arc<dyn standardoc_core::LanguageProvider> = Arc::new(WorkspaceProvider::new());
    let server = open_workspace(dir.path(), provider).unwrap();
    let symbol = query::symbol_by_fqdn(server.handle(), "sample::alpha")
        .unwrap()
        .expect("alpha must be indexed at boot");
    assert_eq!(symbol.name, "alpha");
}

#[test]
fn open_workspace_indexes_a_new_file_through_the_watcher() {
    let dir = tempfile::tempdir().unwrap();
    write_sample_workspace(dir.path());

    let provider: Arc<dyn standardoc_core::LanguageProvider> = Arc::new(WorkspaceProvider::new());
    let server = open_workspace(dir.path(), provider).unwrap();
    let baseline_revision = server.handle().revision();

    fs::write(dir.path().join("src/extra.rs"), "pub fn gamma() {}\n").unwrap();

    wait_revision_at_least(
        server.handle(),
        baseline_revision + 1,
        Duration::from_secs(10),
    );
    let extra_symbols = query::symbols_by_file(server.handle(), "src/extra.rs").unwrap();
    let names: Vec<&str> = extra_symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"gamma"), "got {names:?}");
}

fn wait_revision_at_least(handle: &standardoc_core::IndexHandle, target: u64, timeout: Duration) {
    let start = Instant::now();
    while handle.revision() < target {
        assert!(
            start.elapsed() <= timeout,
            "revision did not reach {target} within {timeout:?} (was {})",
            handle.revision()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn index_once_respects_stdignore_workspace_exclusions() {
    let dir = tempfile::tempdir().unwrap();
    write_sample_workspace(dir.path());

    // Add a vendored subtree and exclude it before indexing.
    fs::create_dir_all(dir.path().join("vendored")).unwrap();
    fs::write(
        dir.path().join("vendored/lib.rs"),
        "pub fn vendored_fn() {}\n",
    )
    .unwrap();
    fs::write(
        dir.path().join(".stdignore"),
        "vendored/\n.git/\ntarget/\nnode_modules/\n",
    )
    .unwrap();

    let provider = WorkspaceProvider::new();
    let handle = index_once(dir.path(), &provider).unwrap();

    let alpha = query::symbol_by_fqdn(&handle, "sample::alpha").unwrap();
    let vendored = query::symbol_by_fqdn(&handle, "vendored::vendored_fn").unwrap();

    assert!(alpha.is_some(), "non-excluded symbol must be indexed");
    assert!(
        vendored.is_none(),
        "excluded subtree must not be indexed (got {vendored:#?})"
    );
}

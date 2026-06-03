use std::sync::{Arc, RwLock};

use rmcp::ServerHandler;
use standardoc_core::{IndexHandle, LanguageProvider, ScanFilters};
use standardoc_lang_provider::WorkspaceProvider;
use standardoc_server::{StandardocMcp, build_mcp_handler, kick_off_indexing};
use tempfile::TempDir;

fn open_workspace() -> (
    TempDir,
    IndexHandle,
    Arc<dyn LanguageProvider>,
    Arc<RwLock<ScanFilters>>,
) {
    let dir = tempfile::tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    let filters = Arc::new(RwLock::new(ScanFilters::load(handle.workspace_root())));
    let provider: Arc<dyn LanguageProvider> = Arc::new(WorkspaceProvider::new());
    (dir, handle, provider, filters)
}

fn build() -> (TempDir, StandardocMcp) {
    let (dir, handle, provider, filters) = open_workspace();
    (dir, build_mcp_handler(handle, provider, filters))
}

#[test]
fn build_mcp_handler_constructs_a_clonable_server() {
    let (_dir, mcp) = build();
    // rmcp `ServiceExt::serve` requires `Clone` on the handler — verify we
    // didn't regress the bound by accident, and that both clones see the
    // same `index_ready` flip.
    let twin = mcp.clone();
    assert!(Arc::ptr_eq(&mcp.index_ready(), &twin.index_ready()));
}

#[test]
fn get_info_advertises_the_tools_capability() {
    let (_dir, mcp) = build();
    let info = mcp.get_info();
    assert!(
        info.capabilities.tools.is_some(),
        "tools capability must be enabled (#[tool_handler] dispatch path)"
    );
    assert!(
        info.instructions
            .as_deref()
            .is_some_and(|s| s.contains("Standardoc")),
        "ServerInfo must carry usage instructions for the LLM client"
    );
}

#[test]
fn get_info_uses_implementation_metadata_from_build_env() {
    let (_dir, mcp) = build();
    let info = mcp.get_info();
    assert!(
        !info.server_info.name.is_empty(),
        "Implementation::from_build_env() should fill `name`"
    );
    assert!(
        !info.server_info.version.is_empty(),
        "Implementation::from_build_env() should fill `version`"
    );
}

#[test]
fn index_ready_starts_false_and_is_independently_observable() {
    let (_dir, mcp) = build();
    let r1 = mcp.index_ready();
    let r2 = mcp.index_ready();
    assert!(!r1.load(std::sync::atomic::Ordering::Acquire));
    r1.store(true, std::sync::atomic::Ordering::Release);
    assert!(
        r2.load(std::sync::atomic::Ordering::Acquire),
        "the two Arcs must observe the same flip (shared state)"
    );
}

#[test]
fn watcher_slot_is_empty_at_construction_and_shared() {
    let (_dir, mcp) = build();
    let s1 = mcp.watcher_slot();
    let s2 = mcp.watcher_slot();
    assert!(s1.lock().unwrap().is_none());
    assert!(s2.lock().unwrap().is_none());
    // sanity: the two clones point at the same Mutex
    assert!(Arc::ptr_eq(&s1, &s2));
}

#[tokio::test(flavor = "multi_thread")]
async fn kick_off_indexing_readonly_flips_index_ready_synchronously_and_skips_watcher() {
    let dir = tempfile::tempdir().unwrap();
    {
        let _writer = IndexHandle::open(dir.path()).unwrap();
    }

    let reader = IndexHandle::open_readonly(dir.path()).unwrap();
    assert!(reader.is_readonly());

    let provider: Arc<dyn LanguageProvider> = Arc::new(WorkspaceProvider::new());
    let filters = Arc::new(RwLock::new(ScanFilters::load(reader.workspace_root())));
    let mcp = build_mcp_handler(reader.clone(), Arc::clone(&provider), Arc::clone(&filters));
    let index_ready = mcp.index_ready();
    let watcher_slot = mcp.watcher_slot();

    assert!(
        !index_ready.load(std::sync::atomic::Ordering::Acquire),
        "fresh handler must report index_ready=false"
    );

    kick_off_indexing(&mcp, reader, provider, filters);

    assert!(
        index_ready.load(std::sync::atomic::Ordering::Acquire),
        "readonly path must flip index_ready synchronously (no cold_start spawn)"
    );
    assert!(
        watcher_slot.lock().unwrap().is_none(),
        "readonly path must not boot a watcher"
    );
}

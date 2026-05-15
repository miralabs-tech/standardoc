//! End-to-end MCP integration test for `resolve_external`.
//!
//! Drives the `StandardocMcp::resolve_external` async tool method via the
//! `build_mcp_handler` factory — skips stdio JSON-RPC framing but exercises
//! the full tokio rt + `spawn_blocking` + `ResolverRegistry` chain plus the
//! envelope serialization that the real MCP daemon emits over rmcp.
//!
//! `#[ignore]` by default — requires `cargo` on `PATH` plus either network
//! or a warm `~/.cargo/registry/` cache for `serde`. Run explicitly with:
//!
//! ```sh
//! cargo test -p standardoc-server --test e2e_mcp_resolve_external -- --ignored --nocapture
//! ```

use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, RawContent};
use standardoc_core::{IndexHandle, LanguageProvider, ScanFilters};
use standardoc_lang_provider::WorkspaceProvider;
use standardoc_server::{ResolveExternalJson, ResolveExternalParams, build_mcp_handler};

const FIXTURE_MANIFEST: &str = "[package]
name = \"e2e-mcp-resolve\"
version = \"0.0.1\"
edition = \"2024\"

[lib]
path = \"src/lib.rs\"

[dependencies]
serde = \"1\"
";

fn collect_text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| match &c.raw {
            RawContent::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires cargo binary + warm `~/.cargo/registry` cache (or network) for serde"]
async fn mcp_resolve_external_returns_resolved_envelope_for_serde() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    std::fs::write(root.join("Cargo.toml"), FIXTURE_MANIFEST).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "").unwrap();

    let status = Command::new("cargo")
        .arg("generate-lockfile")
        .current_dir(root)
        .status()
        .expect("cargo binary on PATH required");
    assert!(status.success(), "cargo generate-lockfile failed");

    let handle = IndexHandle::open(root).expect("open IndexHandle");
    let provider: Arc<dyn LanguageProvider> = Arc::new(WorkspaceProvider::new());
    let filters = Arc::new(RwLock::new(ScanFilters::load(handle.workspace_root())));
    let mcp = build_mcp_handler(handle, provider, filters);

    // Skip cold start — resolve_external doesn't depend on a populated
    // workspace index, only on the external resolver registry. Flipping
    // index_ready manually bypasses the "Workspace indexing in progress"
    // graceful-degradation branch (Q5).
    mcp.index_ready().store(true, Ordering::Release);

    let result = mcp
        .resolve_external(Parameters(ResolveExternalParams {
            fqdn: "serde::Deserialize".to_string(),
        }))
        .await
        .expect("resolve_external tool must not error");

    let text = collect_text(&result);
    let envelope: ResolveExternalJson = serde_json::from_str(&text).unwrap_or_else(|e| {
        panic!("envelope must parse as ResolveExternalJson; got `{text}` (err: {e})")
    });

    assert_eq!(
        envelope.status, "resolved",
        "expected resolved status, envelope=`{text}`"
    );
    assert_eq!(envelope.fqdn, "serde::Deserialize");
    assert_eq!(
        envelope.source_origin.as_deref(),
        Some("cargo_registry"),
        "source_origin must identify the cargo resolver"
    );
    let symbol = envelope
        .symbol
        .expect("symbol must be populated on resolved status");
    assert_eq!(symbol.fqdn, "serde::Deserialize");
    assert_eq!(symbol.name, "Deserialize");
}

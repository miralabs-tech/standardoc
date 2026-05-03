//! End-to-end tests over a mixed Rust + TypeScript workspace.
//!
//! Exercises the cold_start + watcher + filters + queries backbone with the
//! `WorkspaceProvider` dispatching both `RustProvider` and `TsProvider` from
//! a single tempdir. Cross-language Rust↔TS edges are out of scope (bridge
//! SDK lands post-beta.1, lock bridges §4) — each language is exercised
//! independently inside the same workspace.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use standardoc_core::{IndexHandle, ScanFilters, cold_start, query};
use standardoc_ir::{EdgeKind, ResolvedOrUnresolved};
use standardoc_lang_provider::WorkspaceProvider;
use standardoc_server::{index_once, open_workspace, rescan};

const RUST_LIB_RS: &str = "\
pub mod helpers;

/// Alpha entry point.
pub fn alpha() {}

pub fn beta() {
    alpha();
}
";

const RUST_HELPERS_RS: &str = "pub fn helper() {}\n";

const TS_INDEX_TS: &str = "\
import { login } from \"./auth\";

export function main() {
    login();
}
";

const TS_AUTH_TS: &str = "\
/**
 * Authenticate the current user.
 */
export function login(): string {
    return \"ok\";
}
";

fn write(root: &Path, rel: &str, content: &str) {
    let abs = root.join(rel);
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(abs, content).unwrap();
}

fn seed_mix_workspace(root: &Path) {
    write(
        root,
        "src-tauri/Cargo.toml",
        "[package]\nname = \"sample-api\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(root, "src-tauri/src/lib.rs", RUST_LIB_RS);
    write(root, "src-tauri/src/helpers.rs", RUST_HELPERS_RS);

    write(
        root,
        "package.json",
        "{\"name\":\"@app/web\",\"version\":\"0.1.0\"}",
    );
    write(root, "src/index.ts", TS_INDEX_TS);
    write(root, "src/auth.ts", TS_AUTH_TS);
}

fn wait_revision_at_least(handle: &IndexHandle, target: u64, timeout: Duration) {
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

fn wait_until<F: FnMut() -> bool>(mut cond: F, timeout: Duration, message: &str) {
    let start = Instant::now();
    while !cond() {
        assert!(start.elapsed() <= timeout, "{message} (waited {timeout:?})");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn cold_start_indexes_both_rust_and_ts() {
    let dir = tempfile::tempdir().unwrap();
    seed_mix_workspace(dir.path());

    let provider = WorkspaceProvider::new();
    let handle = index_once(dir.path(), &provider).unwrap();

    let rust_lib = query::symbols_by_file(&handle, "src-tauri/src/lib.rs").unwrap();
    let rust_names: Vec<&str> = rust_lib.iter().map(|s| s.name.as_str()).collect();
    assert!(
        rust_names.contains(&"alpha"),
        "rust alpha missing: {rust_names:?}"
    );
    assert!(
        rust_names.contains(&"beta"),
        "rust beta missing: {rust_names:?}"
    );

    let ts_index = query::symbols_by_file(&handle, "src/index.ts").unwrap();
    let ts_names: Vec<&str> = ts_index.iter().map(|s| s.name.as_str()).collect();
    assert!(ts_names.contains(&"main"), "ts main missing: {ts_names:?}");

    let ts_auth = query::symbols_by_file(&handle, "src/auth.ts").unwrap();
    let auth_names: Vec<&str> = ts_auth.iter().map(|s| s.name.as_str()).collect();
    assert!(
        auth_names.contains(&"login"),
        "ts login missing: {auth_names:?}"
    );
}

#[test]
fn intra_rust_call_resolves_in_mixed_workspace() {
    let dir = tempfile::tempdir().unwrap();
    seed_mix_workspace(dir.path());

    let provider = WorkspaceProvider::new();
    let handle = index_once(dir.path(), &provider).unwrap();

    let edges = query::edges_from(&handle, "sample-api::beta").unwrap();
    let alpha_call = edges.iter().find(|e| {
        e.kind == EdgeKind::Calls
            && matches!(
                &e.to,
                ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "sample-api::alpha"
            )
    });
    assert!(
        alpha_call.is_some(),
        "expected resolved CALL beta -> alpha in mixed workspace, got {edges:#?}"
    );
}

#[test]
fn intra_ts_import_resolves_within_package() {
    let dir = tempfile::tempdir().unwrap();
    seed_mix_workspace(dir.path());

    let provider = WorkspaceProvider::new();
    let handle = index_once(dir.path(), &provider).unwrap();

    let edges = query::edges_from(&handle, "@app/web::src").unwrap();
    let import = edges
        .iter()
        .find(|e| e.kind == EdgeKind::Imports)
        .expect("imports edge from index.ts module not found");
    match &import.to {
        ResolvedOrUnresolved::Resolved { fqdn } => {
            assert_eq!(fqdn, "@app/web::src::auth::login");
        }
        other => panic!("import should be resolved post-pipeline-promote, got {other:?}"),
    }
}

#[test]
fn intra_ts_call_resolves_across_files() {
    let dir = tempfile::tempdir().unwrap();
    seed_mix_workspace(dir.path());

    let provider = WorkspaceProvider::new();
    let handle = index_once(dir.path(), &provider).unwrap();

    let edges = query::edges_from(&handle, "@app/web::src::main").unwrap();
    let login_call = edges.iter().find(|e| {
        e.kind == EdgeKind::Calls
            && matches!(
                &e.to,
                ResolvedOrUnresolved::Resolved { fqdn } if fqdn == "@app/web::src::auth::login"
            )
    });
    assert!(
        login_call.is_some(),
        "expected resolved CALL main -> login (cross-file via alias + promote), got {edges:#?}"
    );
}

#[test]
fn watcher_indexes_new_rust_file() {
    let dir = tempfile::tempdir().unwrap();
    seed_mix_workspace(dir.path());

    let provider: Arc<dyn standardoc_core::LanguageProvider> = Arc::new(WorkspaceProvider::new());
    let server = open_workspace(dir.path(), provider).unwrap();
    let baseline = server.handle().revision();

    write(dir.path(), "src-tauri/src/extra.rs", "pub fn gamma() {}\n");

    wait_revision_at_least(server.handle(), baseline + 1, Duration::from_secs(10));
    let names: Vec<String> = query::symbols_by_file(server.handle(), "src-tauri/src/extra.rs")
        .unwrap()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(names.iter().any(|n| n == "gamma"), "got {names:?}");
}

#[test]
fn watcher_indexes_new_ts_file() {
    let dir = tempfile::tempdir().unwrap();
    seed_mix_workspace(dir.path());

    let provider: Arc<dyn standardoc_core::LanguageProvider> = Arc::new(WorkspaceProvider::new());
    let server = open_workspace(dir.path(), provider).unwrap();
    let baseline = server.handle().revision();

    write(
        dir.path(),
        "src/extra.ts",
        "export function gamma(): number { return 1; }\n",
    );

    wait_revision_at_least(server.handle(), baseline + 1, Duration::from_secs(10));
    let names: Vec<String> = query::symbols_by_file(server.handle(), "src/extra.ts")
        .unwrap()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(names.iter().any(|n| n == "gamma"), "got {names:?}");
}

#[test]
fn watcher_deletes_ts_file_clears_symbols() {
    let dir = tempfile::tempdir().unwrap();
    seed_mix_workspace(dir.path());

    let provider: Arc<dyn standardoc_core::LanguageProvider> = Arc::new(WorkspaceProvider::new());
    let server = open_workspace(dir.path(), provider).unwrap();

    assert!(
        query::symbol_by_fqdn(server.handle(), "@app/web::src::auth::login")
            .unwrap()
            .is_some(),
        "login should be present before delete"
    );
    let baseline = server.handle().revision();

    fs::remove_file(dir.path().join("src/auth.ts")).unwrap();

    wait_revision_at_least(server.handle(), baseline + 1, Duration::from_secs(10));
    wait_until(
        || {
            query::symbol_by_fqdn(server.handle(), "@app/web::src::auth::login")
                .unwrap()
                .is_none()
        },
        Duration::from_secs(10),
        "login symbol was not cleared after auth.ts delete",
    );
}

#[test]
fn watcher_hot_reload_excludes_new_subtree() {
    let dir = tempfile::tempdir().unwrap();
    seed_mix_workspace(dir.path());

    let provider: Arc<dyn standardoc_core::LanguageProvider> = Arc::new(WorkspaceProvider::new());
    let server = open_workspace(dir.path(), provider).unwrap();

    let stdignore_path = dir.path().join(".stdignore");
    let mut body = fs::read_to_string(&stdignore_path).unwrap();
    body.push_str("excluded/\n");
    fs::write(&stdignore_path, body).unwrap();

    // Allow the watcher to debounce + swap filters (default debounce 500 ms,
    // budget 1.5 s for the reload to land — see lock pause-exclude-22 §1.8).
    std::thread::sleep(Duration::from_millis(1500));

    let baseline = server.handle().revision();
    write(
        dir.path(),
        "excluded/foo.ts",
        "export function shouldNotIndex() {}\n",
    );

    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(
        server.handle().revision(),
        baseline,
        "revision bumped on file inside newly-excluded subtree"
    );
    let rows = query::symbols_by_file(server.handle(), "excluded/foo.ts").unwrap();
    assert!(
        rows.is_empty(),
        "excluded/foo.ts should not be indexed, got {rows:?}"
    );
}

#[test]
fn rescan_recreates_mixed_workspace_index() {
    let dir = tempfile::tempdir().unwrap();
    seed_mix_workspace(dir.path());

    let provider = WorkspaceProvider::new();
    let handle = index_once(dir.path(), &provider).unwrap();

    assert!(
        query::symbol_by_fqdn(&handle, "sample-api::alpha")
            .unwrap()
            .is_some()
    );
    assert!(
        query::symbol_by_fqdn(&handle, "@app/web::src::auth::login")
            .unwrap()
            .is_some()
    );

    rescan(&handle, &provider).unwrap();

    assert!(
        query::symbol_by_fqdn(&handle, "sample-api::alpha")
            .unwrap()
            .is_some(),
        "Rust symbol gone after rescan"
    );
    assert!(
        query::symbol_by_fqdn(&handle, "@app/web::src::auth::login")
            .unwrap()
            .is_some(),
        "TS symbol gone after rescan"
    );
}

#[test]
fn pause_blocks_cold_start_then_resume_completes() {
    let dir = tempfile::tempdir().unwrap();
    seed_mix_workspace(dir.path());

    let provider = WorkspaceProvider::new();
    let handle = IndexHandle::open(dir.path()).unwrap();
    let filters = ScanFilters::load(handle.workspace_root());

    handle.pause();
    cold_start::run(&handle, &provider, &filters).unwrap();
    assert!(
        query::symbol_by_fqdn(&handle, "sample-api::alpha")
            .unwrap()
            .is_none(),
        "paused cold_start indexed Rust symbols"
    );
    assert!(
        query::symbol_by_fqdn(&handle, "@app/web::src::auth::login")
            .unwrap()
            .is_none(),
        "paused cold_start indexed TS symbols"
    );

    handle.resume();
    cold_start::run(&handle, &provider, &filters).unwrap();
    assert!(
        query::symbol_by_fqdn(&handle, "sample-api::alpha")
            .unwrap()
            .is_some(),
        "resumed cold_start did not index Rust"
    );
    assert!(
        query::symbol_by_fqdn(&handle, "@app/web::src::auth::login")
            .unwrap()
            .is_some(),
        "resumed cold_start did not index TS"
    );
}

#[test]
fn cold_start_persists_user_documents_for_both_languages() {
    let dir = tempfile::tempdir().unwrap();
    seed_mix_workspace(dir.path());

    let provider = WorkspaceProvider::new();
    let handle = index_once(dir.path(), &provider).unwrap();

    let alpha_ctx = query::context_for_symbol(&handle, "sample-api::alpha")
        .unwrap()
        .expect("alpha context");
    assert_eq!(
        alpha_ctx.document_description.as_deref(),
        Some("Alpha entry point."),
        "Rust /// did not persist on alpha"
    );

    let login_ctx = query::context_for_symbol(&handle, "@app/web::src::auth::login")
        .unwrap()
        .expect("login context");
    assert_eq!(
        login_ctx.document_description.as_deref(),
        Some("Authenticate the current user."),
        "TS /** */ did not persist on login"
    );
}

const LUA_LIB_LUA: &str = "\
local M = {}

--- public trim helper
function M.trim(s)
    return s
end

local function private_helper(x) return x end

return M
";

#[test]
fn cold_start_indexes_lua_alongside_rust_and_ts() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    seed_mix_workspace(root);
    write(root, "scripts.rockspec", "package = \"scripts\"\n");
    write(root, "scripts/lib.lua", LUA_LIB_LUA);

    let provider = WorkspaceProvider::new();
    let handle = index_once(root, &provider).unwrap();

    let lua_syms = query::symbols_by_file(&handle, "scripts/lib.lua").unwrap();
    let names: Vec<&str> = lua_syms.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"trim"), "lua trim missing: {names:?}");
    assert!(
        names.contains(&"private_helper"),
        "lua private_helper missing: {names:?}"
    );

    // The module-pattern post-process must promote `M.trim` to Public
    // (table M is returned at file end) while `private_helper` stays Private.
    let trim = lua_syms.iter().find(|s| s.name == "trim").unwrap();
    assert_eq!(trim.visibility, standardoc_ir::Visibility::Public);
    assert_eq!(trim.fqdn, "scripts::scripts::lib::M::trim");

    let private = lua_syms
        .iter()
        .find(|s| s.name == "private_helper")
        .unwrap();
    assert_eq!(private.visibility, standardoc_ir::Visibility::Private);

    // The Rust + TS sides of the same workspace must still index correctly
    // — Lua doesn't disturb the existing dispatchers.
    let rust_lib = query::symbols_by_file(&handle, "src-tauri/src/lib.rs").unwrap();
    assert!(rust_lib.iter().any(|s| s.name == "alpha"));
    let ts_index = query::symbols_by_file(&handle, "src/index.ts").unwrap();
    assert!(ts_index.iter().any(|s| s.name == "main"));

    // Lua doc must persist through the pipeline (Rust /// + TS /** */
    // already covered above; Lua --- gets the same treatment via
    // apply_documents).
    let trim_ctx = query::context_for_symbol(&handle, "scripts::scripts::lib::M::trim")
        .unwrap()
        .expect("trim context");
    assert_eq!(
        trim_ctx.document_description.as_deref(),
        Some("public trim helper"),
        "Lua --- did not persist on M.trim"
    );
}

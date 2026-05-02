use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::thread;

use standardoc_core::{ExtractContext, LanguageProvider};
use standardoc_ir::{EdgeKind, Kind, ResolvedOrUnresolved};
use standardoc_lang_provider::{RustProvider, TsProvider, WorkspaceProvider};
use tempfile::tempdir;

fn write(root: &Path, rel: &str, content: &str) {
    let abs = root.join(rel);
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(abs, content).unwrap();
}

fn fresh_workspace(crate_name: &str) -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    write(
        dir.path(),
        "Cargo.toml",
        &format!("[package]\nname = \"{crate_name}\"\nversion = \"0.1.0\"\n"),
    );
    dir
}

#[test]
fn workspace_provider_extracts_a_realistic_lib_rs() {
    let dir = fresh_workspace("mycrate");
    let root = dir.path();
    let lib_rs = "\
pub mod foo;

pub trait Greeter {
    fn greet(&self) -> String;
}

pub struct Hello;

impl Greeter for Hello {
    fn greet(&self) -> String {
        helper()
    }
}

fn helper() -> String {
    String::new()
}
";
    write(root, "src/lib.rs", lib_rs);
    write(root, "src/foo.rs", "pub fn bar() {}\n");

    let provider = WorkspaceProvider::new();
    let ctx = ExtractContext {
        workspace_root: root,
    };

    let extracted = provider.extract(lib_rs, "src/lib.rs", &ctx).unwrap();

    // file Module symbol named after crate.
    let module = &extracted.symbols[0];
    assert_eq!(module.kind, Kind::Module);
    assert_eq!(module.fqdn, "mycrate");

    // Trait + trait_fn + struct + impl_fn + free fn helper = 5 sub-symbols.
    let fqdns: Vec<&str> = extracted.symbols.iter().map(|s| s.fqdn.as_str()).collect();
    assert!(fqdns.contains(&"mycrate::Greeter"));
    assert!(fqdns.contains(&"mycrate::Greeter::greet"));
    assert!(fqdns.contains(&"mycrate::Hello"));
    assert!(fqdns.contains(&"mycrate::Hello::greet"));
    assert!(fqdns.contains(&"mycrate::helper"));

    // IMPLEMENTS edge: from Hello to Greeter (same file, so resolved-canonical).
    let imp = extracted
        .edges
        .iter()
        .find(|e| e.kind == EdgeKind::Implements)
        .expect("implements edge");
    assert_eq!(imp.from_fqdn, "mycrate::Hello");
    match &imp.to {
        ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "mycrate::Greeter"),
        other => panic!("expected resolved (same-file trait), got {other:?}"),
    }

    // CALLS edge: Hello::greet calls helper (resolved as mycrate::helper).
    let call = extracted
        .edges
        .iter()
        .find(|e| e.kind == EdgeKind::Calls && e.from_fqdn == "mycrate::Hello::greet")
        .expect("calls edge from Hello::greet");
    match &call.to {
        ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "mycrate::helper"),
        other => panic!("expected resolved local call, got {other:?}"),
    }
}

#[test]
fn extracts_foo_rs_module_symbol_with_correct_fqdn() {
    let dir = fresh_workspace("mycrate");
    let root = dir.path();
    let foo_rs = "pub fn bar() { baz(); } pub fn baz() {}\n";
    write(root, "src/foo.rs", foo_rs);

    let provider = WorkspaceProvider::new();
    let ctx = ExtractContext {
        workspace_root: root,
    };
    let extracted = provider.extract(foo_rs, "src/foo.rs", &ctx).unwrap();

    let module = &extracted.symbols[0];
    assert_eq!(module.fqdn, "mycrate::foo");
    assert_eq!(module.module.as_deref(), Some("mycrate"));

    // bar calls baz (defined in same file → Resolved).
    let call = extracted
        .edges
        .iter()
        .find(|e| e.kind == EdgeKind::Calls)
        .expect("calls edge");
    assert_eq!(call.from_fqdn, "mycrate::foo::bar");
    match &call.to {
        ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "mycrate::foo::baz"),
        other => panic!("expected resolved, got {other:?}"),
    }
}

#[test]
fn unknown_extension_returns_unsupported_language() {
    let dir = fresh_workspace("mycrate");
    let root = dir.path();

    let provider = WorkspaceProvider::new();
    let ctx = ExtractContext {
        workspace_root: root,
    };
    let err = provider
        .extract("# heading", "docs/notes.md", &ctx)
        .expect_err("md not dispatched to any provider");
    match err {
        standardoc_core::ExtractError::UnsupportedLanguage { file } => {
            assert_eq!(file, "docs/notes.md");
        }
        other => panic!("expected unsupported, got {other:?}"),
    }
}

#[test]
fn concurrent_extracts_share_crate_name_cache_safely() {
    let dir = fresh_workspace("multithread");
    let root = dir.path().to_path_buf();

    // 4 sibling rs files all under the same Cargo.toml.
    for i in 0..4 {
        write(&root, &format!("src/m{i}.rs"), "pub fn x() {}\n");
    }

    let provider = Arc::new(RustProvider::new());
    let mut handles = Vec::new();
    for i in 0..4 {
        let provider = Arc::clone(&provider);
        let root = root.clone();
        handles.push(thread::spawn(move || {
            let ctx = ExtractContext {
                workspace_root: &root,
            };
            let path = format!("src/m{i}.rs");
            let extracted = provider.extract("pub fn x() {}\n", &path, &ctx).unwrap();
            extracted.symbols[0].fqdn.clone()
        }));
    }

    for (i, h) in handles.into_iter().enumerate() {
        let fqdn = h.join().unwrap();
        assert_eq!(fqdn, format!("multithread::m{i}"));
    }
}

#[test]
fn content_hash_stable_across_provider_instances() {
    let dir = fresh_workspace("foo");
    let root = dir.path();
    let body = "pub fn a() {}\n";
    write(root, "src/lib.rs", body);

    let p1 = RustProvider::new();
    let p2 = RustProvider::new();
    let ctx = ExtractContext {
        workspace_root: root,
    };

    let r1 = p1.extract(body, "src/lib.rs", &ctx).unwrap();
    let r2 = p2.extract(body, "src/lib.rs", &ctx).unwrap();
    assert_eq!(r1.content_hash, r2.content_hash);
    assert_eq!(r1.symbols[0].body_hash, r2.symbols[0].body_hash);
}

fn fresh_ts_package(package_name: &str) -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    write(
        dir.path(),
        "package.json",
        &format!("{{\"name\":\"{package_name}\",\"version\":\"0.1.0\"}}"),
    );
    dir
}

#[test]
fn workspace_provider_extracts_a_realistic_lib_ts() {
    let dir = fresh_ts_package("@myorg/api");
    let root = dir.path();
    let src = "\
import { logger } from './logger';

export interface User { id: string; }

export function makeUser(id: string): User {
    logger();
    return { id };
}

export class UserService {
    create(id: string): User {
        return makeUser(id);
    }
}
";
    write(root, "src/user/service.ts", src);
    write(root, "src/user/logger.ts", "export function logger() {}\n");

    let provider = WorkspaceProvider::new();
    let ctx = ExtractContext {
        workspace_root: root,
    };
    let extracted = provider.extract(src, "src/user/service.ts", &ctx).unwrap();

    let module = &extracted.symbols[0];
    assert_eq!(module.kind, Kind::Module);
    assert_eq!(module.fqdn, "@myorg/api::src::user::service");

    let fqdns: Vec<&str> = extracted.symbols.iter().map(|s| s.fqdn.as_str()).collect();
    assert!(fqdns.contains(&"@myorg/api::src::user::service::User"));
    assert!(fqdns.contains(&"@myorg/api::src::user::service::makeUser"));
    assert!(fqdns.contains(&"@myorg/api::src::user::service::UserService"));
    assert!(fqdns.contains(&"@myorg/api::src::user::service::UserService::create"));

    // Imports edge points at the relative resolved canonical FQDN.
    let imp = extracted
        .edges
        .iter()
        .find(|e| e.kind == EdgeKind::Imports)
        .expect("imports edge");
    match &imp.to {
        ResolvedOrUnresolved::Unresolved { name } => {
            assert_eq!(name, "@myorg/api::src::user::logger::logger");
        }
        other => panic!("expected unresolved canonical import, got {other:?}"),
    }

    // CALLS edge: makeUser → logger (via alias-table).
    let call = extracted
        .edges
        .iter()
        .find(|e| {
            e.kind == EdgeKind::Calls && e.from_fqdn == "@myorg/api::src::user::service::makeUser"
        })
        .expect("calls edge from makeUser");
    match &call.to {
        ResolvedOrUnresolved::Unresolved { name } => {
            assert_eq!(name, "@myorg/api::src::user::logger::logger");
        }
        other => panic!("expected unresolved canonical via alias, got {other:?}"),
    }

    // CALLS edge: UserService.create → makeUser (resolved, same file).
    let call = extracted
        .edges
        .iter()
        .find(|e| {
            e.kind == EdgeKind::Calls
                && e.from_fqdn == "@myorg/api::src::user::service::UserService::create"
        })
        .expect("calls edge from create");
    match &call.to {
        ResolvedOrUnresolved::Resolved { fqdn } => {
            assert_eq!(fqdn, "@myorg/api::src::user::service::makeUser");
        }
        other => panic!("expected resolved local call, got {other:?}"),
    }
}

#[test]
fn tsx_extracts_jsx_components_without_error() {
    let dir = fresh_ts_package("@myorg/ui");
    let root = dir.path();
    let src = "export const Hello = () => <div>Hi</div>;\n";
    write(root, "src/Hello.tsx", src);

    let provider = WorkspaceProvider::new();
    let ctx = ExtractContext {
        workspace_root: root,
    };
    let extracted = provider.extract(src, "src/Hello.tsx", &ctx).unwrap();

    let hello = extracted
        .symbols
        .iter()
        .find(|s| s.fqdn == "@myorg/ui::src::Hello::Hello")
        .expect("Hello arrow const");
    assert_eq!(hello.kind, Kind::Function);
}

#[test]
fn js_legacy_file_extracts_minimal_symbols() {
    let dir = fresh_ts_package("legacy");
    let root = dir.path();
    let src = "function legacyHelper() {}\nfunction caller() { legacyHelper(); }\n";
    write(root, "scripts/build.js", src);

    let provider = WorkspaceProvider::new();
    let ctx = ExtractContext {
        workspace_root: root,
    };
    let extracted = provider.extract(src, "scripts/build.js", &ctx).unwrap();

    assert_eq!(extracted.language, standardoc_ir::Language::JavaScript);
    let names: Vec<&str> = extracted.symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"legacyHelper"));
    assert!(names.contains(&"caller"));
    let call = extracted
        .edges
        .iter()
        .find(|e| e.kind == EdgeKind::Calls)
        .expect("calls edge");
    match &call.to {
        ResolvedOrUnresolved::Resolved { fqdn } => {
            assert_eq!(fqdn, "legacy::scripts::build::legacyHelper");
        }
        other => panic!("expected resolved local call, got {other:?}"),
    }
}

#[test]
fn concurrent_ts_extracts_share_package_name_cache_safely() {
    let dir = fresh_ts_package("multi-ts");
    let root = dir.path().to_path_buf();
    for i in 0..4 {
        write(&root, &format!("src/m{i}.ts"), "export const x = 1;\n");
    }

    let provider = Arc::new(TsProvider::new());
    let mut handles = Vec::new();
    for i in 0..4 {
        let provider = Arc::clone(&provider);
        let root = root.clone();
        handles.push(thread::spawn(move || {
            let ctx = ExtractContext {
                workspace_root: &root,
            };
            let path = format!("src/m{i}.ts");
            let extracted = provider
                .extract("export const x = 1;\n", &path, &ctx)
                .unwrap();
            extracted.symbols[0].fqdn.clone()
        }));
    }

    for (i, h) in handles.into_iter().enumerate() {
        let fqdn = h.join().unwrap();
        assert_eq!(fqdn, format!("multi-ts::src::m{i}"));
    }
}

#[test]
fn rust_outer_doc_comment_captured_as_raw_document() {
    let dir = fresh_workspace("docs_rust");
    let root = dir.path();
    let src = "\
/// Top-level helper.
pub fn helper() {}

/// User-facing record.
pub struct User { pub id: u32 }
";
    write(root, "src/lib.rs", src);
    let provider = WorkspaceProvider::new();
    let ctx = ExtractContext {
        workspace_root: root,
    };
    let extracted = provider.extract(src, "src/lib.rs", &ctx).unwrap();

    assert_eq!(extracted.documents.len(), 2);
    let helper_doc = extracted
        .documents
        .iter()
        .find(|d| d.symbol_fqdn == "docs_rust::helper")
        .expect("helper doc");
    assert_eq!(helper_doc.description, "Top-level helper.");
    let user_doc = extracted
        .documents
        .iter()
        .find(|d| d.symbol_fqdn == "docs_rust::User")
        .expect("User doc");
    assert_eq!(user_doc.description, "User-facing record.");
}

#[test]
fn rust_inner_doc_attaches_to_module_symbol() {
    let dir = fresh_workspace("docs_inner");
    let root = dir.path();
    let src = "\
//! Crate-level docs for the foo module.

pub fn x() {}
";
    write(root, "src/foo.rs", src);
    let provider = WorkspaceProvider::new();
    let ctx = ExtractContext {
        workspace_root: root,
    };
    let extracted = provider.extract(src, "src/foo.rs", &ctx).unwrap();

    let module_doc = extracted
        .documents
        .iter()
        .find(|d| d.symbol_fqdn == "docs_inner::foo")
        .expect("module doc");
    assert_eq!(
        module_doc.description,
        "Crate-level docs for the foo module."
    );
}

#[test]
fn ts_jsdoc_block_captured_as_raw_document() {
    let dir = fresh_ts_package("@app/docs");
    let root = dir.path();
    let src = "\
/**
 * Creates a new user.
 */
export function makeUser(): void {}

/**
 * The user record.
 */
export interface User { id: string }
";
    write(root, "src/index.ts", src);
    let provider = WorkspaceProvider::new();
    let ctx = ExtractContext {
        workspace_root: root,
    };
    let extracted = provider.extract(src, "src/index.ts", &ctx).unwrap();

    let make_user_doc = extracted
        .documents
        .iter()
        .find(|d| d.symbol_fqdn == "@app/docs::src::makeUser")
        .expect("makeUser doc");
    assert_eq!(make_user_doc.description, "Creates a new user.");
    let user_doc = extracted
        .documents
        .iter()
        .find(|d| d.symbol_fqdn == "@app/docs::src::User")
        .expect("User doc");
    assert_eq!(user_doc.description, "The user record.");
}

#[test]
fn ts_top_of_file_jsdoc_attaches_to_module_symbol() {
    let dir = fresh_ts_package("@app/top");
    let root = dir.path();
    let src = "\
/**
 * Top-of-file module description.
 */
export const N = 1;
";
    write(root, "src/lib.ts", src);
    let provider = WorkspaceProvider::new();
    let ctx = ExtractContext {
        workspace_root: root,
    };
    let extracted = provider.extract(src, "src/lib.ts", &ctx).unwrap();

    let module_doc = extracted
        .documents
        .iter()
        .find(|d| d.symbol_fqdn == "@app/top::src::lib")
        .expect("module-level JSDoc");
    assert_eq!(module_doc.description, "Top-of-file module description.");
}

#[test]
fn ts_content_hash_stable_across_provider_instances() {
    let dir = fresh_ts_package("foo");
    let root = dir.path();
    let body = "export function a(): void {}\n";
    write(root, "src/index.ts", body);

    let p1 = TsProvider::new();
    let p2 = TsProvider::new();
    let ctx = ExtractContext {
        workspace_root: root,
    };

    let r1 = p1.extract(body, "src/index.ts", &ctx).unwrap();
    let r2 = p2.extract(body, "src/index.ts", &ctx).unwrap();
    assert_eq!(r1.content_hash, r2.content_hash);
    assert_eq!(r1.symbols[0].body_hash, r2.symbols[0].body_hash);
}

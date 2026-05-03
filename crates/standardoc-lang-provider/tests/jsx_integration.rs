#![allow(
    clippy::needless_raw_string_hashes,
    clippy::missing_const_for_fn,
    clippy::match_same_arms
)]

use std::fs;
use std::path::Path;

use standardoc_core::{ExtractContext, LanguageProvider};
use standardoc_ir::{EdgeKind, Language, ResolvedOrUnresolved};
use standardoc_lang_provider::WorkspaceProvider;
use tempfile::tempdir;

fn write(root: &Path, rel: &str, content: &str) {
    let abs = root.join(rel);
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(abs, content).unwrap();
}

fn refs_with_attr<'a>(
    edges: &'a [standardoc_ir::RawEdge],
    attr: &str,
) -> Vec<&'a standardoc_ir::RawEdge> {
    edges
        .iter()
        .filter(|e| e.kind == EdgeKind::References && e.attributes.iter().any(|a| a == attr))
        .collect()
}

fn ref_name(edge: &standardoc_ir::RawEdge) -> &str {
    match &edge.to {
        ResolvedOrUnresolved::Resolved { fqdn } => fqdn.as_str(),
        ResolvedOrUnresolved::Unresolved { name } => name.as_str(),
        ResolvedOrUnresolved::UnresolvedBridge { name, .. } => name.as_str(),
    }
}

#[test]
fn tsx_component_ref_in_function_body() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    write(root, "package.json", r#"{"name":"@app/web"}"#);
    let src = r#"import Header from './Header';
export function App() {
    return <Header />;
}
"#;
    write(root, "src/App.tsx", src);
    let provider = WorkspaceProvider::new();
    let ctx = ExtractContext { workspace_root: root };
    let extracted = provider.extract(src, "src/App.tsx", &ctx).unwrap();

    assert_eq!(extracted.language, Language::TypeScript);
    let comps = refs_with_attr(&extracted.edges, "template-component-ref");
    assert!(!comps.is_empty(), "expected at least one component-ref edge");
    let names: Vec<&str> = comps.iter().map(|e| ref_name(e)).collect();
    // `Header` is a default import — TsProvider resolves through the
    // import alias table to `@app/web::src::Header::default` (the
    // canonical default-export FQDN).
    assert!(
        names.iter().any(|n| n.contains("Header")),
        "expected a component-ref pointing through the Header import; names: {names:?}"
    );
}

#[test]
fn tsx_attribute_expression_emits_template_bind() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    write(root, "package.json", r#"{"name":"@app/web"}"#);
    let src = r#"export function App() {
    const value = 'hi';
    return <input value={value} />;
}
"#;
    write(root, "src/App.tsx", src);
    let provider = WorkspaceProvider::new();
    let ctx = ExtractContext { workspace_root: root };
    let extracted = provider.extract(src, "src/App.tsx", &ctx).unwrap();

    let bind = refs_with_attr(&extracted.edges, "template-bind");
    let names: Vec<&str> = bind.iter().map(|e| ref_name(e)).collect();
    assert!(names.iter().any(|n| n.ends_with("value")));
}

#[test]
fn jsx_child_interpolation_emits_template_interpolation() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    write(root, "package.json", r#"{"name":"@app/web"}"#);
    let src = r#"export function App() {
    const message = 'hi';
    return <p>{message}</p>;
}
"#;
    write(root, "src/App.jsx", src);
    let provider = WorkspaceProvider::new();
    let ctx = ExtractContext { workspace_root: root };
    let extracted = provider.extract(src, "src/App.jsx", &ctx).unwrap();

    assert_eq!(extracted.language, Language::JavaScript);
    let interp = refs_with_attr(&extracted.edges, "template-interpolation");
    let names: Vec<&str> = interp.iter().map(|e| ref_name(e)).collect();
    assert!(names.iter().any(|n| n.ends_with("message")));
}

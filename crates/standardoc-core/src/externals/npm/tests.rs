
use super::*;
use tempfile::tempdir;

#[test]
fn new_probes_node_when_pnp_cjs_present() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join(".pnp.cjs"), "// pnp").unwrap();
    let resolver = NpmResolver::new(dir.path().to_path_buf());
    assert!(!matches!(
        resolver.binary_availability(),
        BinaryAvailability::NotApplicable
    ));
}

#[test]
fn new_marks_node_not_applicable_without_pnp_cjs() {
    let dir = tempdir().unwrap();
    let resolver = NpmResolver::new(dir.path().to_path_buf());
    assert_eq!(
        resolver.binary_availability(),
        BinaryAvailability::NotApplicable
    );
}

#[test]
fn name_is_stable() {
    let dir = tempdir().unwrap();
    let resolver = NpmResolver::new(dir.path().to_path_buf());
    assert_eq!(resolver.name(), "npm");
    assert_eq!(resolver.source_origin(), SourceOrigin::NodeModulesDts);
}

#[test]
fn package_name_of_fqdn_handles_scoped_packages() {
    assert_eq!(
        package_name_of_fqdn("@types/react::Component"),
        Some("@types/react")
    );
    assert_eq!(package_name_of_fqdn("react::useEffect"), Some("react"));
}

#[test]
fn package_name_of_fqdn_returns_none_on_bare_symbol() {
    assert_eq!(package_name_of_fqdn("bare"), None);
    assert_eq!(package_name_of_fqdn(""), None);
}

#[test]
fn has_npm_extension_recognises_d_ts() {
    assert!(has_npm_extension(Path::new("foo.d.ts")));
    assert!(has_npm_extension(Path::new("a/b/index.d.ts")));
}

#[test]
fn has_npm_extension_recognises_classic_extensions() {
    for ext in ["ts", "tsx", "js", "jsx", "mjs", "cjs"] {
        assert!(has_npm_extension(Path::new(&format!("file.{ext}"))));
    }
}

#[test]
fn has_npm_extension_rejects_unrelated() {
    assert!(!has_npm_extension(Path::new("README.md")));
    assert!(!has_npm_extension(Path::new("Cargo.toml")));
    assert!(!has_npm_extension(Path::new("file.css")));
}

#[test]
fn is_minified_js_detects_long_lines() {
    let content = format!("var x={};", "1".repeat(1500));
    assert!(is_minified_js(Path::new("bundle.js"), &content));
}

#[test]
fn is_minified_js_passes_normal_code() {
    let content = "function foo() {\n  return 42;\n}\n";
    assert!(!is_minified_js(Path::new("foo.js"), content));
}

#[test]
fn is_minified_js_only_applies_to_js_extensions() {
    let content = format!("// {}", "x".repeat(1500));
    assert!(!is_minified_js(Path::new("types.d.ts"), &content));
    assert!(!is_minified_js(Path::new("source.ts"), &content));
}

#[test]
fn is_skip_npm_dir_filters_well_known_noise() {
    assert!(is_skip_npm_dir("node_modules"));
    assert!(is_skip_npm_dir("test"));
    assert!(is_skip_npm_dir("tests"));
    assert!(is_skip_npm_dir("__tests__"));
    assert!(is_skip_npm_dir("examples"));
    assert!(is_skip_npm_dir(".git"));
    assert!(!is_skip_npm_dir("src"));
    assert!(!is_skip_npm_dir("dist"));
}

#[test]
fn rewrite_for_external_stamps_sentinel_path_for_npm() {
    let mut extracted = ExtractedFile {
        file: "index.d.ts".into(),
        language: standardoc_ir::Language::TypeScript,
        source_origin: SourceOrigin::Workspace,
        is_external: false,
        content_hash: standardoc_ir::Blake3Hash::default(),
        byte_size: 0,
        symbols: vec![],
        edges: vec![],
        call_sites: vec![],
        documents: vec![],
        ffi_bindings: vec![],
        module_lookup: None,
    };
    rewrite_for_external(
        &mut extracted,
        "@types/react",
        "index.d.ts",
        SourceOrigin::NodeModulesDts,
    );
    assert_eq!(extracted.file, "external://npm/@types/react/index.d.ts");
    assert!(extracted.is_external);
    assert_eq!(extracted.source_origin, SourceOrigin::NodeModulesDts);
}

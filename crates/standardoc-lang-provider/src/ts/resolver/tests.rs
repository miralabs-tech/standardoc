use super::*;
use std::fs;
use tempfile::tempdir;

#[test]
fn parse_tsconfig_extracts_base_url_and_paths() {
    let json = r#"
        {
          "compilerOptions": {
            "baseUrl": "./src",
            "paths": {
              "@app/*": ["app/*"],
              "@lib": ["lib/index.ts"]
            }
          }
        }
        "#;
    let cfg = parse_tsconfig(json).expect("parse ok");
    assert_eq!(cfg.base_url.as_deref(), Some("./src"));
    assert_eq!(cfg.paths.len(), 2);
    let app = cfg.paths.iter().find(|(k, _)| k == "@app/*").unwrap();
    assert_eq!(app.1, vec!["app/*".to_string()]);
    let lib = cfg.paths.iter().find(|(k, _)| k == "@lib").unwrap();
    assert_eq!(lib.1, vec!["lib/index.ts".to_string()]);
}

#[test]
fn parse_tsconfig_tolerates_line_comments() {
    let json = r#"
        {
          // line
          "compilerOptions": {
            "baseUrl": "src" // trailing
          }
        }
        "#;
    let cfg = parse_tsconfig(json).expect("parse ok");
    assert_eq!(cfg.base_url.as_deref(), Some("src"));
}

#[test]
fn parse_tsconfig_tolerates_block_comments() {
    let json = r#"
        {
          /* leading */
          "compilerOptions": {
            "baseUrl": "src" /* trailing */
          }
        }
        "#;
    let cfg = parse_tsconfig(json).expect("parse ok");
    assert_eq!(cfg.base_url.as_deref(), Some("src"));
}

#[test]
fn parse_tsconfig_preserves_multibyte_utf8_in_string_values() {
    // Regression for TS-RESOLVER-JSONC-UTF8: `strip_jsonc` used to rebuild
    // its output with `c as char` per byte, splitting multi-byte sequences
    // into Latin-1 mojibake. A non-ASCII path must survive verbatim.
    let json = r#"
        {
          // force the comment-stripping byte path
          "compilerOptions": {
            "baseUrl": "src/é-modulé/café"
          }
        }
        "#;
    let cfg = parse_tsconfig(json).expect("parse ok");
    assert_eq!(cfg.base_url.as_deref(), Some("src/é-modulé/café"));
}

#[test]
fn parse_tsconfig_returns_none_on_missing_compiler_options() {
    assert!(parse_tsconfig("{}").is_none());
}

#[test]
fn parse_tsconfig_returns_none_on_invalid_json() {
    assert!(parse_tsconfig("not json").is_none());
}

#[test]
fn strip_jsonc_keeps_strings_with_slash() {
    let s = strip_jsonc(r#"{"path": "//not/a/comment"}"#);
    assert!(s.contains("//not/a/comment"));
}

#[test]
fn strip_jsonc_strips_block_comments_inside_objects() {
    let s = strip_jsonc(r#"{"a": 1 /* skip */, "b": 2}"#);
    assert!(!s.contains("skip"));
    assert!(s.contains("\"a\""));
    assert!(s.contains("\"b\""));
}

#[test]
fn package_dir_of_specifier_handles_scoped() {
    assert_eq!(package_dir_of_specifier("@scope/pkg"), "@scope/pkg");
    assert_eq!(package_dir_of_specifier("@scope/pkg/sub"), "@scope/pkg");
}

#[test]
fn package_dir_of_specifier_handles_unscoped() {
    assert_eq!(package_dir_of_specifier("lodash"), "lodash");
    assert_eq!(package_dir_of_specifier("lodash/fp/take"), "lodash");
}

#[test]
fn lexical_normalise_resolves_parent_dir() {
    let p = Path::new("a/b/../c");
    assert_eq!(lexical_normalise(p), Path::new("a/c"));
}

#[test]
fn lexical_normalise_resolves_current_dir() {
    let p = Path::new("a/./b");
    assert_eq!(lexical_normalise(p), Path::new("a/b"));
}

#[test]
fn join_fqdn_with_empty_module_path() {
    assert_eq!(join_fqdn("foo", ""), "foo");
}

#[test]
fn join_fqdn_prefixes_package_name() {
    // `compute_module_path` now returns `::`-separated module paths
    // already; `join_fqdn` simply prepends the package name.
    assert_eq!(join_fqdn("foo", "src::auth"), "foo::src::auth");
}

#[test]
fn resolve_relative_in_same_dir() {
    let dir = tempdir().unwrap();
    let pkg = dir.path();
    let from = pkg.join("src/auth/login.ts");
    fs::create_dir_all(from.parent().unwrap()).unwrap();
    fs::write(&from, b"// dummy").unwrap();
    let canonical = resolve_relative("./helper", &from, pkg, "@app").expect("relative ok");
    assert_eq!(canonical, "@app::src::auth::helper");
}

#[test]
fn resolve_relative_parent_segment() {
    let dir = tempdir().unwrap();
    let pkg = dir.path();
    let from = pkg.join("src/auth/login.ts");
    fs::create_dir_all(from.parent().unwrap()).unwrap();
    fs::write(&from, b"// dummy").unwrap();
    let canonical = resolve_relative("../user", &from, pkg, "@app").expect("relative ok");
    assert_eq!(canonical, "@app::src::user");
}

#[test]
fn resolve_relative_collapses_index() {
    let dir = tempdir().unwrap();
    let pkg = dir.path();
    let from = pkg.join("src/auth/login.ts");
    fs::create_dir_all(from.parent().unwrap()).unwrap();
    fs::write(&from, b"// dummy").unwrap();
    let canonical = resolve_relative("./index", &from, pkg, "@app").expect("relative ok");
    assert_eq!(canonical, "@app::src::auth");
}

#[test]
fn resolve_via_tsconfig_pattern_with_wildcard() {
    let cfg = TsConfigPaths {
        base_url: Some("./".into()),
        paths: vec![("@app/*".into(), vec!["src/*".into()])],
    };
    let canonical = resolve_via_tsconfig("@app/auth/login", &cfg, "myorg-api").expect("hit");
    assert_eq!(canonical, "myorg-api::src::auth::login");
}

#[test]
fn resolve_via_tsconfig_exact_match() {
    let cfg = TsConfigPaths {
        base_url: None,
        paths: vec![("@core".into(), vec!["lib/core.ts".into()])],
    };
    let canonical = resolve_via_tsconfig("@core", &cfg, "pkg").expect("hit");
    assert_eq!(canonical, "pkg::lib::core");
}

#[test]
fn resolve_via_tsconfig_no_match_returns_none() {
    let cfg = TsConfigPaths {
        base_url: None,
        paths: vec![("@app/*".into(), vec!["src/*".into()])],
    };
    assert!(resolve_via_tsconfig("lodash", &cfg, "pkg").is_none());
}

#[test]
fn resolve_import_relative_specifier() {
    let dir = tempdir().unwrap();
    let pkg = dir.path();
    let from = pkg.join("src/auth/login.ts");
    fs::create_dir_all(from.parent().unwrap()).unwrap();
    fs::write(&from, b"// dummy").unwrap();
    let canonical = resolve_import("./helper", &from, pkg, "@app", None).expect("relative");
    assert_eq!(canonical, "@app::src::auth::helper");
}

#[test]
fn resolve_import_via_tsconfig_when_provided() {
    let dir = tempdir().unwrap();
    let pkg = dir.path();
    let from = pkg.join("src/auth/login.ts");
    fs::create_dir_all(from.parent().unwrap()).unwrap();
    fs::write(&from, b"// dummy").unwrap();
    let cfg = TsConfigPaths {
        base_url: None,
        paths: vec![("@lib/*".into(), vec!["lib/*".into()])],
    };
    let canonical = resolve_import("@lib/utils", &from, pkg, "@app", Some(&cfg)).expect("tsconfig");
    assert_eq!(canonical, "@app::lib::utils");
}

#[test]
fn resolve_import_via_node_modules_lookup() {
    let dir = tempdir().unwrap();
    let pkg = dir.path();
    let from = pkg.join("src/auth/login.ts");
    fs::create_dir_all(from.parent().unwrap()).unwrap();
    fs::write(&from, b"// dummy").unwrap();
    let nm = pkg.join("node_modules/lodash");
    fs::create_dir_all(&nm).unwrap();
    fs::write(
        nm.join("package.json"),
        br#"{"name":"lodash","main":"lodash.js","types":"lodash.d.ts"}"#,
    )
    .unwrap();
    let canonical = resolve_import("lodash", &from, pkg, "@app", None).expect("nm hit");
    assert_eq!(canonical, "lodash::lodash");
}

#[test]
fn resolve_import_via_node_modules_with_subpath() {
    let dir = tempdir().unwrap();
    let pkg = dir.path();
    let from = pkg.join("src/auth/login.ts");
    fs::create_dir_all(from.parent().unwrap()).unwrap();
    fs::write(&from, b"// dummy").unwrap();
    let nm = pkg.join("node_modules/lodash");
    fs::create_dir_all(&nm).unwrap();
    fs::write(
        nm.join("package.json"),
        br#"{"name":"lodash","main":"lodash.js"}"#,
    )
    .unwrap();
    let canonical = resolve_import("lodash/fp/take", &from, pkg, "@app", None).expect("nm subpath");
    assert_eq!(canonical, "lodash::fp::take");
}

#[test]
fn resolve_import_via_node_modules_scoped_package() {
    let dir = tempdir().unwrap();
    let pkg = dir.path();
    let from = pkg.join("src/auth/login.ts");
    fs::create_dir_all(from.parent().unwrap()).unwrap();
    fs::write(&from, b"// dummy").unwrap();
    let nm = pkg.join("node_modules/@scope/sdk");
    fs::create_dir_all(&nm).unwrap();
    fs::write(
        nm.join("package.json"),
        br#"{"name":"@scope/sdk","types":"dist/index.d.ts"}"#,
    )
    .unwrap();
    let canonical = resolve_import("@scope/sdk", &from, pkg, "@app", None).expect("scoped hit");
    assert_eq!(canonical, "@scope/sdk::dist");
}

#[test]
fn resolve_import_returns_none_when_unresolvable() {
    let dir = tempdir().unwrap();
    let pkg = dir.path();
    let from = pkg.join("src/auth/login.ts");
    fs::create_dir_all(from.parent().unwrap()).unwrap();
    fs::write(&from, b"// dummy").unwrap();
    assert!(resolve_import("rxjs", &from, pkg, "@app", None).is_none());
}

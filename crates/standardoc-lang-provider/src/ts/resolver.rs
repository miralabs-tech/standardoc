use std::path::{Path, PathBuf};

use super::helpers::compute_module_path;

/// Subset of `tsconfig.json` `compilerOptions` consumed by the resolver.
///
/// Day-1 we honour `baseUrl` + `paths` (with single-fallback target only
/// when the value array contains 2+ entries — first match wins, log warn).
/// `extends` is followed exactly one level (no recursive chain). Project
/// `references` are PUNTed post-beta.1.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TsConfigPaths {
    pub(crate) base_url: Option<String>,
    pub(crate) paths: Vec<(String, Vec<String>)>,
}

/// Parse a `tsconfig.json` blob (JSONC: comments + trailing commas tolerated)
/// and extract `compilerOptions.baseUrl` / `compilerOptions.paths`.
/// `extends` is dropped — callers stitch single-level extends manually.
pub(crate) fn parse_tsconfig(jsonc_content: &str) -> Option<TsConfigPaths> {
    let stripped = strip_jsonc(jsonc_content);
    let value: serde_json::Value = serde_json::from_str(&stripped).ok()?;
    let opts = value.get("compilerOptions")?;
    let base_url = opts
        .get("baseUrl")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let paths = opts
        .get("paths")
        .and_then(|v| v.as_object())
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| {
                    let arr = v.as_array()?;
                    let strs: Vec<String> = arr
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_string))
                        .collect();
                    Some((k.clone(), strs))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some(TsConfigPaths { base_url, paths })
}

/// Strip `//` line comments and `/* */` block comments from a JSONC blob.
/// Trailing commas are not removed — serde_json tolerates them only if
/// the input does, but real tsconfigs we've seen typically render without.
/// String contents are preserved verbatim (no comment recognition inside
/// quoted strings).
fn strip_jsonc(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    let mut in_string = false;
    let mut escape = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            out.push(c as char);
            if escape {
                escape = false;
            } else if c == b'\\' {
                escape = true;
            } else if c == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == b'"' {
            in_string = true;
            out.push('"');
            i += 1;
            continue;
        }
        if c == b'/' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'/' => {
                    i += 2;
                    while i < bytes.len() && bytes[i] != b'\n' {
                        i += 1;
                    }
                    continue;
                }
                b'*' => {
                    i += 2;
                    while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                        i += 1;
                    }
                    i = (i + 2).min(bytes.len());
                    continue;
                }
                _ => {}
            }
        }
        out.push(c as char);
        i += 1;
    }
    out
}

/// Resolve a TS/JS import specifier to a best-effort canonical FQDN.
///
/// Resolution order, day-1:
/// 1. Relative (`./` / `../`) → file resolution (lexical normalise) +
///    `<current_package_name>::<module_path>`
/// 2. tsconfig `paths` patterns → first match wins; replacement
///    re-canonicalised through `compute_module_path`
/// 3. `node_modules` ancestor walk → `<remote_package_name>::<entry_module>`
///    where entry comes from `package.json#types` or `#main` (no
///    `exports` field support day-1)
/// 4. None of the above → `None` (caller falls back to the raw specifier)
///
/// `from_file` is the absolute path of the file containing the import;
/// `package_root` is the canonical directory of its owning `package.json`.
pub(crate) fn resolve_import(
    spec: &str,
    from_file: &Path,
    package_root: &Path,
    current_package_name: &str,
    tsconfig: Option<&TsConfigPaths>,
) -> Option<String> {
    if spec.starts_with("./") || spec.starts_with("../") {
        return resolve_relative(spec, from_file, package_root, current_package_name);
    }
    if let Some(cfg) = tsconfig
        && let Some(canonical) = resolve_via_tsconfig(spec, cfg, current_package_name)
    {
        return Some(canonical);
    }
    resolve_via_node_modules(spec, from_file)
}

fn resolve_relative(
    spec: &str,
    from_file: &Path,
    package_root: &Path,
    current_package_name: &str,
) -> Option<String> {
    let parent = from_file.parent()?;
    let joined = parent.join(spec);
    let normalised = lexical_normalise(&joined);
    let rel = normalised.strip_prefix(package_root).ok()?;
    let rel_str = rel.to_string_lossy().replace('\\', "/");
    let module_path = compute_module_path(&rel_str);
    Some(join_fqdn(current_package_name, &module_path))
}

fn resolve_via_tsconfig(
    spec: &str,
    cfg: &TsConfigPaths,
    current_package_name: &str,
) -> Option<String> {
    let mut warned_multi = false;
    for (pattern, targets) in &cfg.paths {
        let Some(replacement) = expand_pattern(pattern, targets, spec, &mut warned_multi) else {
            continue;
        };
        let module_path = compute_module_path(&replacement);
        return Some(join_fqdn(current_package_name, &module_path));
    }
    None
}

fn expand_pattern(
    pattern: &str,
    targets: &[String],
    spec: &str,
    warned_multi: &mut bool,
) -> Option<String> {
    let target = targets.first()?;
    if targets.len() > 1 && !*warned_multi {
        eprintln!("[standardoc] tsconfig paths: multiple targets for {pattern}, taking first");
        *warned_multi = true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        let suffix = spec.strip_prefix(prefix)?;
        return Some(target.replace('*', suffix));
    }
    if pattern == spec {
        return Some(target.clone());
    }
    None
}

fn resolve_via_node_modules(spec: &str, from_file: &Path) -> Option<String> {
    let pkg_dir = package_dir_of_specifier(spec);
    let sub_path = spec
        .strip_prefix(&pkg_dir)
        .unwrap_or("")
        .trim_start_matches('/');
    let mut current = from_file.parent()?;
    loop {
        let candidate = current
            .join("node_modules")
            .join(&pkg_dir)
            .join("package.json");
        if candidate.is_file() {
            return canonical_from_node_modules_pkg(&candidate, sub_path);
        }
        current = current.parent()?;
    }
}

fn canonical_from_node_modules_pkg(pkg_json: &Path, sub_path: &str) -> Option<String> {
    let content = std::fs::read_to_string(pkg_json).ok()?;
    let value: serde_json::Value = serde_json::from_str(&content).ok()?;
    let name = value.get("name")?.as_str()?.to_string();
    let module_path = if sub_path.is_empty() {
        let entry = value
            .get("types")
            .or_else(|| value.get("main"))
            .and_then(|v| v.as_str())
            .unwrap_or("index.js");
        compute_module_path(entry)
    } else {
        compute_module_path(sub_path)
    };
    Some(join_fqdn(&name, &module_path))
}

fn package_dir_of_specifier(spec: &str) -> String {
    if let Some(rest) = spec.strip_prefix('@') {
        let mut parts = rest.splitn(3, '/');
        let scope = parts.next().unwrap_or_default();
        let pkg = parts.next().unwrap_or_default();
        return format!("@{scope}/{pkg}");
    }
    spec.split('/').next().unwrap_or(spec).to_string()
}

fn join_fqdn(package_name: &str, module_path: &str) -> String {
    if module_path.is_empty() {
        package_name.to_string()
    } else {
        format!("{package_name}::{}", module_path.replace('/', "::"))
    }
}

fn lexical_normalise(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
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
    fn join_fqdn_collapses_slashes_to_double_colon() {
        assert_eq!(join_fqdn("foo", "src/auth"), "foo::src::auth");
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
        let canonical =
            resolve_import("@lib/utils", &from, pkg, "@app", Some(&cfg)).expect("tsconfig");
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
        let canonical =
            resolve_import("lodash/fp/take", &from, pkg, "@app", None).expect("nm subpath");
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
}

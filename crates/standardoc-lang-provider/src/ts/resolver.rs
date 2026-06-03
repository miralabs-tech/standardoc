use std::path::{Path, PathBuf};

use super::helpers::compute_module_path;

/// Subset of `tsconfig.json` `compilerOptions` consumed by the resolver.
///
/// Resolution honours `paths` only (single target — first match wins when
/// the value array has 2+ entries, log warn). `baseUrl` is parsed and
/// retained but NOT wired into resolution: a lexical baseUrl lookup would
/// shadow real `node_modules` packages (no fs-existence check to tell
/// `baseUrl/<spec>` apart from `node_modules/<spec>`), so bare specifiers
/// fall through to the node_modules walk. Wiring baseUrl (with existence
/// probing) is deferred. `extends` is followed exactly one level (no
/// recursive chain). Project `references` are PUNTed post-beta.1.
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
    let mut out: Vec<u8> = Vec::with_capacity(input.len());
    let mut i = 0;
    let mut in_string = false;
    let mut escape = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            out.push(c);
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
            out.push(b'"');
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
        out.push(c);
        i += 1;
    }
    // Comments are delimited by ASCII (`/`, `*`), so removing them never
    // splits a multi-byte UTF-8 sequence — the retained bytes stay valid
    // UTF-8 (input was `&str`). Pushing bytes rather than `c as char`
    // keeps multi-byte chars in string values intact.
    String::from_utf8(out).unwrap_or_default()
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
        format!("{package_name}::{module_path}")
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
mod tests;

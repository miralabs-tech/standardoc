//! Walk `.standardoc/pages/` and load pages.
//!
//! Intentionally simple: we use `WalkBuilder` with `**/*.{md,mdx}` overrides
//! since we only want these extensions. `.standardoc/` is hidden, so we must
//! include it explicitly (`hidden(false)` on the builder).
//!
//! YAML frontmatter is parsed manually (3-7 lines, no need for a full YAML
//! dependency): detect `---\n...\n---\n` fence at file start, parse
//! `key: value` lines, and stop there. Enough for 99% of cases
//! (title, order, hide, icon, etc.).

use super::{order_from_prefix, slug_from_relative, DocPage, PageKind, PAGES_DIR};
use ignore::WalkBuilder;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

/// Scan `.standardoc/pages/` at workspace root.
///
/// Return all found pages keyed by slug. Best effort: unreadable files or
/// malformed frontmatter are logged to stderr and do not block full scan.
pub fn scan_pages(workspace_root: &Path) -> BTreeMap<String, DocPage> {
    let pages_root = workspace_root.join(PAGES_DIR);
    if !pages_root.is_dir() {
        return BTreeMap::new();
    }

    let mut out = BTreeMap::new();
    let walker = WalkBuilder::new(&pages_root)
        .follow_links(false)
        // Hidden folders are ignored by default — disable that to allow
        // subfolders starting with `.` (rare but possible, e.g. `.drafts/`).
        // Root `.standardoc/` folder itself is not hidden here because we
        // pass it explicitly as root argument.
        .hidden(false)
        .git_ignore(false)
        .ignore(false)
        .build();

    for entry in walker.filter_map(Result::ok) {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let Some(kind) = page_kind(path) else {
            continue;
        };
        let Ok(rel) = path.strip_prefix(&pages_root) else {
            continue;
        };
        let rel = rel.to_path_buf();
        match load_page(workspace_root, &pages_root, &rel, kind) {
            Ok(page) => {
                out.insert(page.slug.clone(), page);
            }
            Err(err) => {
                eprintln!("standardoc pages: skipping {}: {err}", path.display());
            }
        }
    }
    out
}

fn page_kind(path: &Path) -> Option<PageKind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "md" => Some(PageKind::Md),
        "mdx" => Some(PageKind::Mdx),
        _ => None,
    }
}

fn load_page(
    workspace_root: &Path,
    pages_root: &Path,
    rel: &Path,
    kind: PageKind,
) -> Result<DocPage, std::io::Error> {
    let abs = pages_root.join(rel);
    let raw = std::fs::read_to_string(&abs)?;
    let (frontmatter, body) = split_frontmatter(&raw);

    let slug = slug_from_relative(rel);
    let section = derive_section(rel);
    let order = derive_order(rel, &frontmatter);
    let title = derive_title(rel, &frontmatter, body);

    let workspace_rel = abs
        .strip_prefix(workspace_root)
        .map_or_else(|_| abs.clone(), Path::to_path_buf);

    Ok(DocPage {
        slug,
        path: workspace_rel,
        title,
        order,
        section,
        frontmatter,
        raw_body: body.to_owned(),
        kind,
    })
}

fn derive_section(rel: &Path) -> Vec<String> {
    rel.parent()
        .map(|p| {
            p.components()
                .filter_map(|c| match c {
                    std::path::Component::Normal(s) => s.to_str(),
                    _ => None,
                })
                .map(super::strip_order_prefix)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn derive_order(rel: &Path, frontmatter: &BTreeMap<String, Value>) -> Option<i32> {
    if let Some(v) = frontmatter.get("order").and_then(Value::as_i64) {
        return i32::try_from(v).ok();
    }
    let stem = rel.file_stem().and_then(|s| s.to_str())?;
    order_from_prefix(stem)
}

fn derive_title(rel: &Path, frontmatter: &BTreeMap<String, Value>, body: &str) -> String {
    if let Some(t) = frontmatter.get("title").and_then(Value::as_str) {
        return t.to_owned();
    }
    if let Some(h1) = first_h1(body) {
        return h1;
    }
    rel.file_stem().and_then(|s| s.to_str()).map_or_else(
        || "Untitled".to_owned(),
        |s| prettify_filename(super::strip_order_prefix(s)),
    )
}

fn first_h1(body: &str) -> Option<String> {
    for line in body.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("# ") {
            return Some(rest.trim().to_owned());
        }
        if !t.is_empty() {
            // First non-empty content is not an H1 — no derivable title.
            return None;
        }
    }
    None
}

fn prettify_filename(stem: &str) -> String {
    // `getting-started` → `Getting Started`. Conservatif : on capitalize
    // each word separated by `-` or `_`.
    let mut out = String::with_capacity(stem.len());
    let mut at_word_start = true;
    for ch in stem.chars() {
        if ch == '-' || ch == '_' {
            out.push(' ');
            at_word_start = true;
        } else if at_word_start {
            out.extend(ch.to_uppercase());
            at_word_start = false;
        } else {
            out.push(ch);
        }
    }
    out
}

/// Split YAML frontmatter fence from markdown file. If file does not start
/// with `---\n`, return full content as body and empty frontmatter.
fn split_frontmatter(raw: &str) -> (BTreeMap<String, Value>, &str) {
    let Some(rest) = raw
        .strip_prefix("---\n")
        .or_else(|| raw.strip_prefix("---\r\n"))
    else {
        return (BTreeMap::new(), raw);
    };
    // Search for closing fence at start of a line.
    let close = rest.find("\n---\n").or_else(|| rest.find("\n---\r\n"));
    let Some(close_idx) = close else {
        return (BTreeMap::new(), raw);
    };
    let yaml = &rest[..close_idx];
    let body_start = close_idx
        + if rest[close_idx..].starts_with("\n---\r\n") {
            "\n---\r\n".len()
        } else {
            "\n---\n".len()
        };
    let body = &rest[body_start..];

    let map = parse_yaml_simple(yaml);
    (map, body)
}

/// Scalar-only YAML parser — handles `key: value`, `key: "value"`, `key: 42`,
/// `key: true`, `key: false`, `key: null`. Silently ignores nested YAML /
/// lists — this parser is for simple metadata, not full YAML support.
/// If advanced YAML is needed later, replace with `serde_yaml`.
fn parse_yaml_simple(input: &str) -> BTreeMap<String, Value> {
    let mut map = BTreeMap::new();
    for line in input.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key.trim().to_owned();
        let value = value.trim();
        let parsed = parse_yaml_scalar(value);
        map.insert(key, parsed);
    }
    map
}

fn parse_yaml_scalar(raw: &str) -> Value {
    if raw.is_empty() {
        return Value::Null;
    }
    if raw == "null" || raw == "~" {
        return Value::Null;
    }
    if raw == "true" {
        return Value::Bool(true);
    }
    if raw == "false" {
        return Value::Bool(false);
    }
    if let Ok(n) = raw.parse::<i64>() {
        return Value::Number(n.into());
    }
    if let Ok(f) = raw.parse::<f64>() {
        if let Some(n) = serde_json::Number::from_f64(f) {
            return Value::Number(n);
        }
    }
    // Strip optional surrounding quotes for strings.
    let stripped = raw
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| raw.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(raw);
    Value::String(stripped.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_roundtrip() {
        let raw = "---\ntitle: Hello\norder: 5\nhide: false\n---\nbody here\n";
        let (fm, body) = split_frontmatter(raw);
        assert_eq!(fm.get("title"), Some(&Value::String("Hello".into())));
        assert_eq!(fm.get("order"), Some(&Value::Number(5.into())));
        assert_eq!(fm.get("hide"), Some(&Value::Bool(false)));
        assert_eq!(body, "body here\n");
    }

    #[test]
    fn no_frontmatter_returns_full_body() {
        let raw = "# Hello\n\nbody\n";
        let (fm, body) = split_frontmatter(raw);
        assert!(fm.is_empty());
        assert_eq!(body, raw);
    }

    #[test]
    fn first_h1_extraction() {
        assert_eq!(first_h1("# Hi\n\nbody"), Some("Hi".into()));
        assert_eq!(first_h1("\n# Hi"), Some("Hi".into()));
        assert_eq!(first_h1("body\n# Hi"), None);
        assert_eq!(first_h1(""), None);
    }

    #[test]
    fn quoted_string_scalar() {
        assert_eq!(parse_yaml_scalar("\"foo\""), Value::String("foo".into()));
        assert_eq!(parse_yaml_scalar("'bar'"), Value::String("bar".into()));
        assert_eq!(parse_yaml_scalar("baz"), Value::String("baz".into()));
    }
}

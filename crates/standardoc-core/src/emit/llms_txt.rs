//! `llms.txt` generator — short, link-based index.
//!
//! Layout :
//! ```text
//! # <project_name>
//!
//! > <tagline>
//!
//! ## <crate_or_top_segment>
//!
//! - [<label>](<link>): <kind>. <description>
//! - ...
//! ```
//!
//! Grouping by first FQN segment maps well to Cargo crates / npm packages
//! without parsing manifests. Blocks are sorted alphabetically inside each
//! section to remain deterministic (clean diffs).

use super::EmitOptions;
use crate::model::{BlockOrigin, DocBlock, SymbolKind};
use std::collections::BTreeMap;
use std::fmt::Write;

pub fn emit_llms_txt(blocks: &BTreeMap<String, DocBlock>, opts: &EmitOptions) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# {}", opts.project_name_or_default());
    if let Some(tagline) = &opts.tagline {
        let _ = writeln!(out, "\n> {tagline}");
    }
    out.push('\n');

    if blocks.is_empty() {
        let _ = writeln!(out, "_No documentable blocks found yet._");
        return out;
    }

    let grouped = group_by_top_segment(blocks);
    for (section, entries) in &grouped {
        let _ = writeln!(out, "## {section}\n");
        for block in entries {
            let _ = writeln!(out, "- {}", format_entry(block, opts));
        }
        out.push('\n');
    }
    out
}

fn group_by_top_segment(blocks: &BTreeMap<String, DocBlock>) -> BTreeMap<String, Vec<&DocBlock>> {
    let mut grouped: BTreeMap<String, Vec<&DocBlock>> = BTreeMap::new();
    for block in blocks.values() {
        let section = top_segment(block.key.as_str()).to_owned();
        grouped.entry(section).or_default().push(block);
    }
    // Stable sorting of entries in each section (alphabetical key order).
    for entries in grouped.values_mut() {
        entries.sort_by(|a, b| a.key.as_str().cmp(b.key.as_str()));
    }
    grouped
}

fn top_segment(key: &str) -> &str {
    key.split('.').next().unwrap_or(key)
}

fn format_entry(block: &DocBlock, opts: &EmitOptions) -> String {
    let label = if block.label.is_empty() {
        block.key.as_str()
    } else {
        block.label.as_str()
    };
    let link = build_link(block, opts);
    let kind = block.symbol.as_ref().map_or("symbol", |s| kind_str(s.kind));
    let description = first_description(block).unwrap_or_default();
    let suffix = if description.is_empty() {
        format!("`{}`", block.key.as_str())
    } else {
        format!("`{}` — {description}", block.key.as_str())
    };
    if link.is_empty() {
        format!("**{label}** ({kind}): {suffix}")
    } else {
        format!("[{label}]({link}) ({kind}): {suffix}")
    }
}

fn build_link(block: &DocBlock, opts: &EmitOptions) -> String {
    let path = block.meta.path.to_string_lossy();
    let line = block.meta.line_start;
    if let Some(base) = &opts.link_base {
        let trimmed = base.trim_end_matches('/');
        if line > 0 {
            format!("{trimmed}/{path}#L{line}")
        } else {
            format!("{trimmed}/{path}")
        }
    } else if line > 0 {
        format!("{path}#L{line}")
    } else {
        path.into_owned()
    }
}

fn first_description(block: &DocBlock) -> Option<String> {
    block
        .tags
        .get("description")
        .and_then(|occurrences| occurrences.first())
        .and_then(|fields| fields.first())
        .map(|s| {
            // Compact to one line — index stays readable even when original
            // description spans multiple lines.
            s.replace('\n', " ").trim().to_owned()
        })
        .filter(|s| !s.is_empty())
}

const fn kind_str(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Function => "function",
        SymbolKind::Method => "method",
        SymbolKind::Class => "class",
        SymbolKind::Struct => "struct",
        SymbolKind::Enum => "enum",
        SymbolKind::Trait => "trait",
        SymbolKind::Interface => "interface",
        SymbolKind::TypeAlias => "type alias",
        SymbolKind::Const => "const",
        SymbolKind::Static => "static",
        SymbolKind::Module => "module",
        SymbolKind::Macro => "macro",
        SymbolKind::Field => "field",
        SymbolKind::Variant => "variant",
        SymbolKind::Other => "symbol",
    }
}

#[allow(dead_code)]
const fn origin_label(origin: BlockOrigin) -> &'static str {
    match origin {
        BlockOrigin::Inferred => "inferred",
        BlockOrigin::Annotated => "annotated",
        BlockOrigin::Hybrid => "hybrid",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DocKey, DocMeta, References, SymbolInfo, Visibility};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn block(key: &str, label: &str, kind: SymbolKind, description: Option<&str>) -> DocBlock {
        let mut tags = BTreeMap::new();
        if let Some(d) = description {
            tags.insert("description".to_owned(), vec![vec![d.to_owned()]]);
        }
        DocBlock {
            key: DocKey::new(key),
            label: label.to_owned(),
            origin: BlockOrigin::Inferred,
            tags,
            symbol: Some(SymbolInfo {
                kind,
                visibility: Visibility::Public,
                signature: format!("/* {label} */"),
                params: vec![],
                returns: None,
                generics: vec![],
                decorators: vec![],
                is_async: false,
                is_deprecated: false,
                references: References::default(),
            }),
            meta: DocMeta {
                path: PathBuf::from(format!("{}.rs", key.replace('.', "/"))),
                line_start: 10,
                line_end: 12,
                column: 1,
                file_ext: "rs".to_owned(),
                comment_style: crate::model::CommentStyle::DocSingle,
                last_indexed: 0,
                source_mtime: 0,
            },
            body_hash: 0,
            diagnostics: vec![],
            virtual_tags: BTreeMap::new(),
            virtual_confidence: None,
            virtual_sources: Vec::new(),
        }
    }

    #[test]
    fn empty_index_produces_placeholder() {
        let blocks = BTreeMap::new();
        let opts = EmitOptions {
            project_name: Some("My Project".to_owned()),
            tagline: Some("a tagline".to_owned()),
            ..Default::default()
        };
        let output = emit_llms_txt(&blocks, &opts);
        assert!(output.contains("# My Project"));
        assert!(output.contains("> a tagline"));
        assert!(output.contains("No documentable blocks"));
    }

    #[test]
    fn groups_by_top_segment_sorted() {
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "foo.bar".to_owned(),
            block("foo.bar", "bar", SymbolKind::Function, Some("a fn")),
        );
        blocks.insert(
            "alpha.beta".to_owned(),
            block("alpha.beta", "beta", SymbolKind::Struct, Some("a struct")),
        );
        blocks.insert(
            "foo.baz".to_owned(),
            block("foo.baz", "baz", SymbolKind::Function, None),
        );
        let opts = EmitOptions::default();
        let output = emit_llms_txt(&blocks, &opts);
        assert!(output.contains("## alpha"));
        assert!(output.contains("## foo"));
        // alpha section comes before foo section (BTreeMap order).
        let alpha_pos = output.find("## alpha").unwrap();
        let foo_pos = output.find("## foo").unwrap();
        assert!(alpha_pos < foo_pos);
        // Inside foo, `bar` comes before `baz`.
        let first = output.find("foo.bar").unwrap();
        let second = output.find("foo.baz").unwrap();
        assert!(first < second);
    }

    #[test]
    fn link_base_prefixes_paths() {
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "foo.bar".to_owned(),
            block("foo.bar", "bar", SymbolKind::Function, Some("desc")),
        );
        let opts = EmitOptions {
            link_base: Some("https://github.com/me/project/blob/main".to_owned()),
            ..Default::default()
        };
        let output = emit_llms_txt(&blocks, &opts);
        assert!(output.contains("https://github.com/me/project/blob/main/foo/bar.rs#L10"));
    }

    #[test]
    fn includes_description_when_present() {
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "x".to_owned(),
            block("x", "x", SymbolKind::Function, Some("the x function")),
        );
        let opts = EmitOptions::default();
        let output = emit_llms_txt(&blocks, &opts);
        assert!(output.contains("the x function"));
    }
}

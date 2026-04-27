//! `llms-full.txt` generator — full inlined content, no hyperlinks.
//!
//! Designed for agents that ingest in bulk rather than following links.
//! Everything is inline: signature, description, `@param` / `@returns`,
//! examples when present.
//!
//! Layout :
//! ```text
//! # <project_name>
//!
//! > <tagline>
//!
//! ## <crate_or_top_segment>
//!
//! ### <key> (<kind>)
//!
//! `<signature>`
//!
//! <description>
//!
//! **Parameters**
//! - `<name>` (`<type>`): <description>
//!
//! **Returns** (`<type>`): <description>
//! ```

use super::EmitOptions;
use crate::model::{DocBlock, SymbolKind};
use std::collections::BTreeMap;
use std::fmt::Write;

pub fn emit_llms_full(blocks: &BTreeMap<String, DocBlock>, opts: &EmitOptions) -> String {
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
            write_block(&mut out, block);
        }
    }
    out
}

fn group_by_top_segment(blocks: &BTreeMap<String, DocBlock>) -> BTreeMap<String, Vec<&DocBlock>> {
    let mut grouped: BTreeMap<String, Vec<&DocBlock>> = BTreeMap::new();
    for block in blocks.values() {
        let section = top_segment(block.key.as_str()).to_owned();
        grouped.entry(section).or_default().push(block);
    }
    for entries in grouped.values_mut() {
        entries.sort_by(|a, b| a.key.as_str().cmp(b.key.as_str()));
    }
    grouped
}

fn top_segment(key: &str) -> &str {
    key.split('.').next().unwrap_or(key)
}

fn write_block(out: &mut String, block: &DocBlock) {
    let kind = block.symbol.as_ref().map_or("symbol", |s| kind_str(s.kind));
    let _ = writeln!(out, "### `{}` ({kind})", block.key.as_str());
    out.push('\n');

    // Signature in a code fence — prettyplease already formatted it.
    if let Some(symbol) = &block.symbol {
        if !symbol.signature.is_empty() {
            let _ = writeln!(out, "```rust");
            let _ = writeln!(out, "{}", symbol.signature);
            let _ = writeln!(out, "```");
            out.push('\n');
        }
    }

    if let Some(desc) = first_string(block, "description") {
        let _ = writeln!(out, "{desc}");
        out.push('\n');
    }

    write_params(out, block);
    write_returns(out, block);
    write_example(out, block);
    out.push('\n');
}

fn write_params(out: &mut String, block: &DocBlock) {
    let Some(params) = block.tags.get("param") else {
        return;
    };
    if params.is_empty() {
        return;
    }
    let _ = writeln!(out, "**Parameters**");
    out.push('\n');
    for occurrence in params {
        let name = occurrence.first().map_or("", String::as_str);
        let ty = occurrence.get(1).map_or("", String::as_str);
        let desc = occurrence.get(2).map_or("", String::as_str);
        if ty.is_empty() && desc.is_empty() {
            let _ = writeln!(out, "- `{name}`");
        } else if desc.is_empty() {
            let _ = writeln!(out, "- `{name}` (`{ty}`)");
        } else {
            let _ = writeln!(out, "- `{name}` (`{ty}`): {desc}");
        }
    }
    out.push('\n');
}

fn write_returns(out: &mut String, block: &DocBlock) {
    let Some(returns) = block
        .tags
        .get("returns")
        .or_else(|| block.tags.get("return"))
    else {
        return;
    };
    let Some(occurrence) = returns.first() else {
        return;
    };
    let ty = occurrence.first().map_or("", String::as_str);
    let desc = occurrence.get(1).map_or("", String::as_str);
    if ty.is_empty() && desc.is_empty() {
        return;
    }
    if desc.is_empty() {
        let _ = writeln!(out, "**Returns** (`{ty}`)");
    } else {
        let _ = writeln!(out, "**Returns** (`{ty}`): {desc}");
    }
    out.push('\n');
}

fn write_example(out: &mut String, block: &DocBlock) {
    let Some(example) = first_string(block, "example") else {
        return;
    };
    let _ = writeln!(out, "**Example**");
    out.push('\n');
    let _ = writeln!(out, "```");
    let _ = writeln!(out, "{example}");
    let _ = writeln!(out, "```");
    out.push('\n');
}

fn first_string(block: &DocBlock, tag: &str) -> Option<String> {
    block
        .tags
        .get(tag)
        .and_then(|occurrences| occurrences.first())
        .and_then(|fields| fields.first())
        .filter(|s| !s.is_empty())
        .cloned()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        BlockOrigin, CommentStyle, DocKey, DocMeta, References, SymbolInfo, Visibility,
    };
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn build_block(
        key: &str,
        label: &str,
        signature: &str,
        description: Option<&str>,
        params: Vec<(&str, &str, &str)>,
        ret: Option<(&str, &str)>,
    ) -> DocBlock {
        let mut tags: BTreeMap<String, Vec<Vec<String>>> = BTreeMap::new();
        if let Some(d) = description {
            tags.insert("description".to_owned(), vec![vec![d.to_owned()]]);
        }
        if !params.is_empty() {
            let occurrences = params
                .into_iter()
                .map(|(n, t, d)| vec![n.to_owned(), t.to_owned(), d.to_owned()])
                .collect();
            tags.insert("param".to_owned(), occurrences);
        }
        if let Some((t, d)) = ret {
            tags.insert("returns".to_owned(), vec![vec![t.to_owned(), d.to_owned()]]);
        }
        DocBlock {
            key: DocKey::new(key),
            label: label.to_owned(),
            origin: BlockOrigin::Annotated,
            tags,
            symbol: Some(SymbolInfo {
                kind: SymbolKind::Function,
                visibility: Visibility::Public,
                signature: signature.to_owned(),
                params: vec![],
                returns: None,
                generics: vec![],
                decorators: vec![],
                is_async: false,
                is_deprecated: false,
                references: References::default(),
            }),
            meta: DocMeta {
                path: PathBuf::from("src/lib.rs"),
                line_start: 1,
                line_end: 1,
                column: 1,
                file_ext: "rs".to_owned(),
                comment_style: CommentStyle::DocSingle,
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
    fn full_format_includes_signature_description_params_returns() {
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "math.add".to_owned(),
            build_block(
                "math.add",
                "add",
                "pub fn add(a: i32, b: i32) -> i32",
                Some("Adds two integers."),
                vec![
                    ("a", "i32", "first operand"),
                    ("b", "i32", "second operand"),
                ],
                Some(("i32", "the sum")),
            ),
        );
        let opts = EmitOptions {
            project_name: Some("MyLib".to_owned()),
            ..Default::default()
        };
        let out = emit_llms_full(&blocks, &opts);
        assert!(out.contains("# MyLib"));
        assert!(out.contains("`math.add`"));
        assert!(out.contains("pub fn add(a: i32, b: i32) -> i32"));
        assert!(out.contains("Adds two integers."));
        assert!(out.contains("- `a` (`i32`): first operand"));
        assert!(out.contains("**Returns** (`i32`): the sum"));
    }

    #[test]
    fn returns_falls_back_to_singular_return_tag() {
        let mut tags: BTreeMap<String, Vec<Vec<String>>> = BTreeMap::new();
        tags.insert(
            "return".to_owned(),
            vec![vec!["bool".to_owned(), "true if ok".to_owned()]],
        );
        let block = DocBlock {
            key: DocKey::new("x"),
            label: "x".to_owned(),
            origin: BlockOrigin::Annotated,
            tags,
            symbol: None,
            meta: DocMeta {
                path: PathBuf::from("x.rs"),
                line_start: 1,
                line_end: 1,
                column: 1,
                file_ext: "rs".to_owned(),
                comment_style: CommentStyle::DocSingle,
                last_indexed: 0,
                source_mtime: 0,
            },
            body_hash: 0,
            diagnostics: vec![],
            virtual_tags: BTreeMap::new(),
            virtual_confidence: None,
            virtual_sources: Vec::new(),
        };
        let mut blocks = BTreeMap::new();
        blocks.insert("x".to_owned(), block);
        let out = emit_llms_full(&blocks, &EmitOptions::default());
        assert!(out.contains("**Returns** (`bool`): true if ok"));
    }
}

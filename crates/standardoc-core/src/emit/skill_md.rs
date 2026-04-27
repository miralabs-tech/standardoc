//! `skill.md` generator — Claude Code "skill" format with YAML front-matter.
//!
//! Goal: an agent loading this file should gain the "navigate/use this project"
//! skill. More narrative than `llms.txt`/`llms-full.txt`, less exhaustive.
//! We expose **logical entry points** (traits, main types, public functions)
//! and skip internal method details — agent can still fetch `get_doc(key)`
//! when it needs to dig deeper.
//!
//! Layout :
//! ```text
//! ---
//! name: <slug>
//! description: <one-liner>
//! ---
//!
//! # <project_name>
//!
//! <tagline>
//!
//! ## Key types
//! - `Foo` (struct): description
//!
//! ## Key traits
//! - `Bar` (trait): description
//!   Implementors: Baz, Qux
//!
//! ## Public functions
//! - `do_thing` — description
//! ```

use super::EmitOptions;
use crate::model::{DocBlock, SymbolKind, Visibility};
use std::collections::BTreeMap;
use std::fmt::Write;

pub fn emit_skill_md(blocks: &BTreeMap<String, DocBlock>, opts: &EmitOptions) -> String {
    let project = opts.project_name_or_default();
    let slug = slugify(project);
    let description = opts
        .tagline
        .clone()
        .unwrap_or_else(|| format!("Auto-generated skill for the {project} codebase"));

    let mut out = String::new();
    let _ = writeln!(out, "---");
    let _ = writeln!(out, "name: {slug}");
    let _ = writeln!(out, "description: {description}");
    let _ = writeln!(out, "---");
    out.push('\n');
    let _ = writeln!(out, "# {project}");
    out.push('\n');
    if let Some(tagline) = &opts.tagline {
        let _ = writeln!(out, "{tagline}");
        out.push('\n');
    }

    if blocks.is_empty() {
        let _ = writeln!(out, "_No documentable blocks found yet._");
        return out;
    }

    write_section(&mut out, "Key types", blocks, |b| {
        matches!(
            b.symbol.as_ref().map(|s| s.kind),
            Some(
                SymbolKind::Struct
                    | SymbolKind::Class
                    | SymbolKind::Enum
                    | SymbolKind::Interface
                    | SymbolKind::TypeAlias
            )
        ) && is_public(b)
    });

    write_traits_section(&mut out, blocks);

    write_section(&mut out, "Public functions", blocks, |b| {
        b.symbol.as_ref().map(|s| s.kind) == Some(SymbolKind::Function) && is_public(b)
    });

    out
}

fn write_section<F>(
    out: &mut String,
    title: &str,
    blocks: &BTreeMap<String, DocBlock>,
    mut filter: F,
) where
    F: FnMut(&DocBlock) -> bool,
{
    let mut entries: Vec<&DocBlock> = blocks.values().filter(|b| filter(b)).collect();
    entries.sort_by(|a, b| a.key.as_str().cmp(b.key.as_str()));
    if entries.is_empty() {
        return;
    }
    let _ = writeln!(out, "## {title}");
    out.push('\n');
    for block in entries {
        let kind = block.symbol.as_ref().map_or("symbol", |s| kind_str(s.kind));
        let label = if block.label.is_empty() {
            block.key.as_str()
        } else {
            block.label.as_str()
        };
        let desc = first_description(block);
        if let Some(d) = desc {
            let _ = writeln!(out, "- `{label}` ({kind}, `{}`): {d}", block.key.as_str());
        } else {
            let _ = writeln!(out, "- `{label}` ({kind}, `{}`)", block.key.as_str());
        }
    }
    out.push('\n');
}

/// Special section annotating each trait with known implementors.
fn write_traits_section(out: &mut String, blocks: &BTreeMap<String, DocBlock>) {
    let mut traits: Vec<&DocBlock> = blocks
        .values()
        .filter(|b| {
            matches!(
                b.symbol.as_ref().map(|s| s.kind),
                Some(SymbolKind::Trait | SymbolKind::Interface)
            ) && is_public(b)
        })
        .collect();
    traits.sort_by(|a, b| a.key.as_str().cmp(b.key.as_str()));
    if traits.is_empty() {
        return;
    }

    let _ = writeln!(out, "## Key traits");
    out.push('\n');

    // Inline reverse lookup: for each trait, find blocks whose outgoing
    // references contain `Implements -> trait.label`. Not optimal (O(n*m)),
    // but `skill.md` is generated rarely, so it remains acceptable even on
    // large projects.
    for tr in traits {
        let kind = kind_str(tr.symbol.as_ref().map_or(SymbolKind::Other, |s| s.kind));
        let label = if tr.label.is_empty() {
            tr.key.as_str()
        } else {
            tr.label.as_str()
        };
        let desc = first_description(tr);
        let _ = match desc {
            Some(d) => writeln!(out, "- `{label}` ({kind}, `{}`): {d}", tr.key.as_str()),
            None => writeln!(out, "- `{label}` ({kind}, `{}`)", tr.key.as_str()),
        };

        let implementors = collect_implementors(blocks, label);
        if !implementors.is_empty() {
            let _ = writeln!(out, "  - Implementors: {}", implementors.join(", "));
        }
    }
    out.push('\n');
}

fn collect_implementors(blocks: &BTreeMap<String, DocBlock>, trait_label: &str) -> Vec<String> {
    use crate::model::RefKind;
    let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for block in blocks.values() {
        let Some(symbol) = &block.symbol else {
            continue;
        };
        let has_implements_ref = symbol
            .references
            .outgoing
            .iter()
            .any(|r| r.kind == RefKind::Implements && r.target == trait_label);
        if has_implements_ref {
            // Infer implementor type from method's parent FQN.
            let parent = parent_fqn_segment(block.key.as_str());
            if !parent.is_empty() {
                set.insert(parent);
            }
        }
    }
    set.into_iter().collect()
}

/// For `crate.module.<X as Trait>.method`, returns `X`.
/// For `crate.module.X.method`, returns `X`.
fn parent_fqn_segment(key: &str) -> String {
    let without_method = key.rsplit_once('.').map_or(key, |(parent, _)| parent);
    if let Some(start) = without_method.rfind('<') {
        if let Some(end) = without_method[start..].find(" as ") {
            return without_method[start + 1..start + end].to_owned();
        }
    }
    without_method
        .rsplit_once('.')
        .map_or(without_method, |(_, last)| last)
        .to_owned()
}

fn is_public(block: &DocBlock) -> bool {
    block
        .symbol
        .as_ref()
        .is_some_and(|s| matches!(s.visibility, Visibility::Public | Visibility::Crate))
}

fn first_description(block: &DocBlock) -> Option<String> {
    block
        .tags
        .get("description")
        .and_then(|occurrences| occurrences.first())
        .and_then(|fields| fields.first())
        .map(|s| s.replace('\n', " ").trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if matches!(c, '-' | '_') {
            out.push(c);
        } else if c.is_whitespace() {
            out.push('-');
        }
    }
    if out.is_empty() {
        "project".to_owned()
    } else {
        out
    }
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
        BlockOrigin, CommentStyle, DocKey, DocMeta, RefKind, References, SymbolInfo,
    };
    use std::path::PathBuf;

    fn make_block(
        key: &str,
        label: &str,
        kind: SymbolKind,
        vis: Visibility,
        description: Option<&str>,
        outgoing: Vec<(RefKind, &str)>,
    ) -> DocBlock {
        let mut tags = BTreeMap::new();
        if let Some(d) = description {
            tags.insert("description".to_owned(), vec![vec![d.to_owned()]]);
        }
        let mut refs = References::default();
        for (k, target) in outgoing {
            refs.push(k, target.to_owned(), 0);
        }
        DocBlock {
            key: DocKey::new(key),
            label: label.to_owned(),
            origin: BlockOrigin::Inferred,
            tags,
            symbol: Some(SymbolInfo {
                kind,
                visibility: vis,
                signature: String::new(),
                params: vec![],
                returns: None,
                generics: vec![],
                decorators: vec![],
                is_async: false,
                is_deprecated: false,
                references: refs,
            }),
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
        }
    }

    #[test]
    fn front_matter_and_sections_are_present() {
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "MyTrait".to_owned(),
            make_block(
                "MyTrait",
                "MyTrait",
                SymbolKind::Trait,
                Visibility::Public,
                Some("a trait"),
                vec![],
            ),
        );
        blocks.insert(
            "MyStruct".to_owned(),
            make_block(
                "MyStruct",
                "MyStruct",
                SymbolKind::Struct,
                Visibility::Public,
                Some("a struct"),
                vec![],
            ),
        );
        blocks.insert(
            "do_thing".to_owned(),
            make_block(
                "do_thing",
                "do_thing",
                SymbolKind::Function,
                Visibility::Public,
                Some("does it"),
                vec![],
            ),
        );
        let out = emit_skill_md(
            &blocks,
            &EmitOptions {
                project_name: Some("Cool Lib".to_owned()),
                tagline: Some("a cool lib".to_owned()),
                ..Default::default()
            },
        );
        assert!(out.starts_with("---\n"));
        assert!(out.contains("name: cool-lib"));
        assert!(out.contains("description: a cool lib"));
        assert!(out.contains("# Cool Lib"));
        assert!(out.contains("## Key types"));
        assert!(out.contains("MyStruct"));
        assert!(out.contains("## Key traits"));
        assert!(out.contains("MyTrait"));
        assert!(out.contains("## Public functions"));
        assert!(out.contains("do_thing"));
    }

    #[test]
    fn implementors_are_listed_under_traits() {
        let mut blocks = BTreeMap::new();
        // Defined trait.
        blocks.insert(
            "lib.MyTrait".to_owned(),
            make_block(
                "lib.MyTrait",
                "MyTrait",
                SymbolKind::Trait,
                Visibility::Public,
                None,
                vec![],
            ),
        );
        // Method of `impl Trait for Foo` — tagged Implements -> MyTrait.
        blocks.insert(
            "lib.<Foo as MyTrait>.bar".to_owned(),
            make_block(
                "lib.<Foo as MyTrait>.bar",
                "bar",
                SymbolKind::Method,
                Visibility::Public,
                None,
                vec![(RefKind::Implements, "MyTrait")],
            ),
        );
        let out = emit_skill_md(&blocks, &EmitOptions::default());
        assert!(out.contains("Implementors: Foo"));
    }

    #[test]
    fn private_symbols_are_excluded() {
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "secret".to_owned(),
            make_block(
                "secret",
                "secret",
                SymbolKind::Function,
                Visibility::Private,
                None,
                vec![],
            ),
        );
        let out = emit_skill_md(&blocks, &EmitOptions::default());
        assert!(!out.contains("secret"));
    }

    #[test]
    fn slugify_preserves_dashes_and_underscores() {
        assert_eq!(slugify("My Cool Lib"), "my-cool-lib");
        assert_eq!(slugify("my_lib-v2"), "my_lib-v2");
        assert_eq!(slugify("!"), "project");
    }
}

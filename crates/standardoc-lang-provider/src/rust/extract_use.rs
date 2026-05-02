use proc_macro2::Span;
use standardoc_ir::{EdgeKind, RawEdge, ResolvedOrUnresolved, Site};
use syn::spanned::Spanned;

use super::walk::{WalkContext, col_from_span, line_from_span};

pub(crate) fn process_use(ctx: &mut WalkContext, item: &syn::ItemUse, current_module: &str) {
    let span = item.span();
    let mut prefix: Vec<String> = Vec::new();
    walk_tree(ctx, &item.tree, &mut prefix, current_module, span);
}

fn walk_tree(
    ctx: &mut WalkContext,
    tree: &syn::UseTree,
    prefix: &mut Vec<String>,
    current_module: &str,
    span: Span,
) {
    match tree {
        syn::UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            walk_tree(ctx, &path.tree, prefix, current_module, span);
            prefix.pop();
        }
        syn::UseTree::Name(name) => {
            let leaf = name.ident.to_string();
            // `use foo::{self};` → import the prefix itself, alias = last segment of prefix.
            if leaf == "self" {
                if let Some(last) = prefix.last().cloned() {
                    emit_import(ctx, prefix, &last, current_module, span);
                }
                return;
            }
            let mut full = prefix.clone();
            full.push(leaf.clone());
            emit_import(ctx, &full, &leaf, current_module, span);
        }
        syn::UseTree::Rename(rename) => {
            let leaf = rename.ident.to_string();
            let alias = rename.rename.to_string();
            if leaf == "self" {
                // `use foo::{self as bar};`
                emit_import(ctx, prefix, &alias, current_module, span);
                return;
            }
            let mut full = prefix.clone();
            full.push(leaf);
            emit_import(ctx, &full, &alias, current_module, span);
        }
        syn::UseTree::Glob(_) => {
            if prefix.is_empty() {
                return;
            }
            emit_glob_import(ctx, prefix, current_module, span);
        }
        syn::UseTree::Group(group) => {
            for sub in &group.items {
                walk_tree(ctx, sub, prefix, current_module, span);
            }
        }
    }
}

fn emit_import(
    ctx: &mut WalkContext,
    full_path: &[String],
    alias_name: &str,
    current_module: &str,
    span: Span,
) {
    let raw = full_path.join("::");
    let canonical = ctx
        .canonicalize(&raw, current_module)
        .unwrap_or_else(|| raw.clone());
    ctx.add_alias(alias_name.to_string(), canonical.clone());
    let to = import_target(ctx, &canonical);
    let from = ctx.core.file_module_fqdn.clone();
    let file = ctx.core.file_path.clone();
    ctx.push_edge(RawEdge {
        from_fqdn: from,
        kind: EdgeKind::Imports,
        to,
        sites: vec![Site {
            file,
            line: line_from_span(span),
            col: col_from_span(span),
        }],
    });
}

fn emit_glob_import(ctx: &mut WalkContext, prefix: &[String], current_module: &str, span: Span) {
    let raw = prefix.join("::");
    let canonical = ctx
        .canonicalize(&raw, current_module)
        .unwrap_or_else(|| raw.clone());
    let to = import_target(ctx, &canonical);
    let from = ctx.core.file_module_fqdn.clone();
    let file = ctx.core.file_path.clone();
    ctx.push_edge(RawEdge {
        from_fqdn: from,
        kind: EdgeKind::Imports,
        to,
        sites: vec![Site {
            file,
            line: line_from_span(span),
            col: col_from_span(span),
        }],
    });
}

fn import_target(ctx: &WalkContext, canonical: &str) -> ResolvedOrUnresolved {
    if ctx.core.defined_fqdns.contains(canonical) {
        ResolvedOrUnresolved::Resolved {
            fqdn: canonical.to_string(),
        }
    } else {
        ResolvedOrUnresolved::Unresolved {
            name: canonical.to_string(),
        }
    }
}

pub(crate) fn process_extern_crate(ctx: &mut WalkContext, item: &syn::ItemExternCrate) {
    let crate_name = item.ident.to_string();
    let alias = item
        .rename
        .as_ref()
        .map_or_else(|| crate_name.clone(), |(_, r)| r.to_string());
    ctx.add_alias(alias, crate_name.clone());
    let span = item.span();
    let from = ctx.core.file_module_fqdn.clone();
    let file = ctx.core.file_path.clone();
    ctx.push_edge(RawEdge {
        from_fqdn: from,
        kind: EdgeKind::Imports,
        to: ResolvedOrUnresolved::Unresolved { name: crate_name },
        sites: vec![Site {
            file,
            line: line_from_span(span),
            col: col_from_span(span),
        }],
    });
}

#[cfg(test)]
mod tests {
    use super::super::walk::walk;
    use standardoc_ir::{EdgeKind, ResolvedOrUnresolved};

    fn parse(src: &str) -> syn::File {
        syn::parse_file(src).expect("test source not parsable")
    }

    fn imports(edges: &[standardoc_ir::RawEdge]) -> Vec<&standardoc_ir::RawEdge> {
        edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Imports)
            .collect()
    }

    #[test]
    fn simple_use_emits_one_import_edge() {
        let parsed = parse("use std::collections::HashMap;");
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let imp = imports(&edges);
        assert_eq!(imp.len(), 1);
        assert_eq!(imp[0].from_fqdn, "c");
        match &imp[0].to {
            ResolvedOrUnresolved::Unresolved { name } => {
                assert_eq!(name, "std::collections::HashMap");
            }
            other => panic!("expected unresolved, got {other:?}"),
        }
    }

    #[test]
    fn use_group_emits_one_import_per_leaf() {
        let parsed = parse("use foo::{a, b, c};");
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let imp = imports(&edges);
        assert_eq!(imp.len(), 3);
        let names: Vec<_> = imp
            .iter()
            .map(|e| match &e.to {
                ResolvedOrUnresolved::Unresolved { name } => name.clone(),
                other => panic!("expected unresolved, got {other:?}"),
            })
            .collect();
        assert!(names.contains(&"foo::a".to_string()));
        assert!(names.contains(&"foo::b".to_string()));
        assert!(names.contains(&"foo::c".to_string()));
    }

    #[test]
    fn use_glob_emits_import_to_prefix() {
        let parsed = parse("use foo::*;");
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let imp = imports(&edges);
        assert_eq!(imp.len(), 1);
        match &imp[0].to {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "foo"),
            other => panic!("expected unresolved, got {other:?}"),
        }
    }

    #[test]
    fn use_rename_populates_alias() {
        let parsed = parse("use foo::Bar as B; fn use_it() { B::new(); }");
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let imp = imports(&edges);
        assert_eq!(imp.len(), 1);
        match &imp[0].to {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "foo::Bar"),
            other => panic!("expected unresolved, got {other:?}"),
        }
        // The CALLS edge should resolve B::new through the alias.
        let calls: Vec<_> = edges.iter().filter(|e| e.kind == EdgeKind::Calls).collect();
        assert_eq!(calls.len(), 1);
        match &calls[0].to {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "foo::Bar::new"),
            other => panic!("expected unresolved canonical via alias, got {other:?}"),
        }
    }

    #[test]
    fn use_crate_relative_canonicalizes_against_crate_name() {
        let parsed = parse("use crate::foo::bar;");
        let (_, edges, _) = walk(&parsed, "mycrate", "src/lib.rs", "mycrate");
        let imp = imports(&edges);
        assert_eq!(imp.len(), 1);
        match &imp[0].to {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "mycrate::foo::bar"),
            other => panic!("expected unresolved canonical, got {other:?}"),
        }
    }

    #[test]
    fn use_self_relative_canonicalizes_against_current_module() {
        let parsed = parse("use self::sub::thing;");
        let (_, edges, _) = walk(&parsed, "mycrate::a", "src/a.rs", "mycrate");
        let imp = imports(&edges);
        assert_eq!(imp.len(), 1);
        match &imp[0].to {
            ResolvedOrUnresolved::Unresolved { name } => {
                assert_eq!(name, "mycrate::a::sub::thing");
            }
            other => panic!("expected unresolved canonical, got {other:?}"),
        }
    }

    #[test]
    fn use_super_pops_one_module_level() {
        let parsed = parse("use super::sibling;");
        let (_, edges, _) = walk(&parsed, "mycrate::a::b", "src/a/b.rs", "mycrate");
        let imp = imports(&edges);
        assert_eq!(imp.len(), 1);
        match &imp[0].to {
            ResolvedOrUnresolved::Unresolved { name } => {
                assert_eq!(name, "mycrate::a::sibling");
            }
            other => panic!("expected unresolved canonical, got {other:?}"),
        }
    }

    #[test]
    fn extern_crate_emits_import_and_alias() {
        let parsed = parse("extern crate alloc as a; fn use_it() { a::vec::Vec::new(); }");
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let imp = imports(&edges);
        assert_eq!(imp.len(), 1);
        match &imp[0].to {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "alloc"),
            other => panic!("expected unresolved, got {other:?}"),
        }
        // CALLS through the alias.
        let calls: Vec<_> = edges.iter().filter(|e| e.kind == EdgeKind::Calls).collect();
        assert_eq!(calls.len(), 1);
        match &calls[0].to {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "alloc::vec::Vec::new"),
            other => panic!("expected unresolved via alias, got {other:?}"),
        }
    }

    #[test]
    fn import_resolved_when_target_defined_in_same_file() {
        let parsed = parse("pub mod foo { pub fn bar() {} } use crate::foo::bar;");
        let (_, edges, _) = walk(&parsed, "mycrate", "src/lib.rs", "mycrate");
        let imp = imports(&edges);
        assert_eq!(imp.len(), 1);
        match &imp[0].to {
            ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "mycrate::foo::bar"),
            other => panic!("expected resolved (defined locally), got {other:?}"),
        }
    }

    #[test]
    fn nested_use_groups_emit_one_import_per_leaf() {
        let parsed = parse("use std::{io::{Read, Write}, fmt};");
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let imp = imports(&edges);
        let names: Vec<String> = imp
            .iter()
            .map(|e| match &e.to {
                ResolvedOrUnresolved::Unresolved { name } => name.clone(),
                _ => panic!("expected unresolved"),
            })
            .collect();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"std::io::Read".to_string()));
        assert!(names.contains(&"std::io::Write".to_string()));
        assert!(names.contains(&"std::fmt".to_string()));
    }

    #[test]
    fn use_self_in_group_imports_the_prefix_itself() {
        let parsed = parse("use foo::{self, bar};");
        let (_, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");
        let imp = imports(&edges);
        let names: Vec<String> = imp
            .iter()
            .map(|e| match &e.to {
                ResolvedOrUnresolved::Unresolved { name } => name.clone(),
                _ => panic!("expected unresolved"),
            })
            .collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"foo".to_string()));
        assert!(names.contains(&"foo::bar".to_string()));
    }
}

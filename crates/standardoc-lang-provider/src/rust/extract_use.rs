use proc_macro2::Span;
use standardoc_ir::{
    EdgeKind, Kind, LanguageKind, RawEdge, RawSymbol, ResolvedOrUnresolved, Site, Visibility,
};
use syn::spanned::Spanned;

use super::walk::{WalkContext, col_from_span, line_from_span, span_to_location};

pub(crate) fn process_use(ctx: &mut WalkContext, item: &syn::ItemUse, current_module: &str) {
    let span = item.span();
    let is_public = matches!(item.vis, syn::Visibility::Public(_));
    let mut prefix: Vec<String> = Vec::new();
    walk_tree(
        ctx,
        &item.tree,
        &mut prefix,
        current_module,
        span,
        is_public,
    );
}

fn walk_tree(
    ctx: &mut WalkContext,
    tree: &syn::UseTree,
    prefix: &mut Vec<String>,
    current_module: &str,
    span: Span,
    is_public: bool,
) {
    match tree {
        syn::UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            walk_tree(ctx, &path.tree, prefix, current_module, span, is_public);
            prefix.pop();
        }
        syn::UseTree::Name(name) => {
            let leaf = name.ident.to_string();
            // `use foo::{self};` → import the prefix itself, alias = last segment of prefix.
            if leaf == "self" {
                if let Some(last) = prefix.last().cloned() {
                    emit_import(ctx, prefix, &last, current_module, span, is_public);
                }
                return;
            }
            let mut full = prefix.clone();
            full.push(leaf.clone());
            emit_import(ctx, &full, &leaf, current_module, span, is_public);
        }
        syn::UseTree::Rename(rename) => {
            let leaf = rename.ident.to_string();
            let alias = rename.rename.to_string();
            if leaf == "self" {
                // `use foo::{self as bar};`
                emit_import(ctx, prefix, &alias, current_module, span, is_public);
                return;
            }
            let mut full = prefix.clone();
            full.push(leaf);
            emit_import(ctx, &full, &alias, current_module, span, is_public);
        }
        syn::UseTree::Glob(_) => {
            if prefix.is_empty() {
                return;
            }
            emit_glob_import(ctx, prefix, current_module, span, is_public);
        }
        syn::UseTree::Group(group) => {
            for sub in &group.items {
                walk_tree(ctx, sub, prefix, current_module, span, is_public);
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
    is_public: bool,
) {
    let raw = full_path.join("::");
    let canonical = ctx
        .canonicalize(&raw, current_module)
        .unwrap_or_else(|| raw.clone());
    ctx.add_alias(alias_name.to_string(), canonical.clone());
    let to = import_target(ctx, &canonical);
    let confidence = to.default_confidence();
    let from = ctx.core.file_module_fqdn.clone();
    let file = ctx.core.file_path.clone();
    let attributes = if is_public {
        vec!["re-export".to_string()]
    } else {
        vec![]
    };
    ctx.push_edge(RawEdge {
        from_fqdn: from.clone(),
        kind: EdgeKind::Imports,
        to,
        sites: vec![Site {
            file: file.clone(),
            line: line_from_span(span),
            col: col_from_span(span),
        }],
        attributes,
        confidence,
    });

    // Phantom symbol so `query::symbol_by_fqdn("<current_module>::<alias>")`
    // matches API-surface names that are re-exported under a shorter path
    // (e.g. `serde::Deserialize` re-exported from `serde::de::Deserialize`).
    // Item-level only — module re-exports (`pub use foo;` flattening a whole
    // module) are punt beta.2. Kind=Type by default since we cannot know
    // the target kind without resolving cross-file.
    if is_public {
        let phantom_fqdn = format!("{from}::{alias_name}");
        ctx.push_symbol(RawSymbol {
            name: alias_name.to_string(),
            fqdn: phantom_fqdn,
            kind: Kind::Type,
            language_kind: LanguageKind::from("re_export"),
            module: Some(from),
            visibility: Visibility::Public,
            location: span_to_location(span, &file),
            signature: None,
            body_hash: None,
            attributes: vec![],
        });
    }
}

fn emit_glob_import(
    ctx: &mut WalkContext,
    prefix: &[String],
    current_module: &str,
    span: Span,
    is_public: bool,
) {
    let raw = prefix.join("::");
    let canonical = ctx
        .canonicalize(&raw, current_module)
        .unwrap_or_else(|| raw.clone());
    let to = import_target(ctx, &canonical);
    let confidence = to.default_confidence();
    let from = ctx.core.file_module_fqdn.clone();
    let file = ctx.core.file_path.clone();
    // Module-level wildcard re-export. We do not synthesize phantom symbols
    // here — we would need to walk the target module to know what items it
    // exposes (chicken-and-egg in a single-file pass). The edge attribute
    // is enough for downstream resolvers to follow the wildcard if needed.
    let attributes = if is_public {
        vec!["re-export".to_string(), "wildcard".to_string()]
    } else {
        vec![]
    };
    ctx.push_edge(RawEdge {
        from_fqdn: from,
        kind: EdgeKind::Imports,
        to,
        sites: vec![Site {
            file,
            line: line_from_span(span),
            col: col_from_span(span),
        }],
        attributes,
        confidence,
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
    let to = ResolvedOrUnresolved::Unresolved { name: crate_name };
    let confidence = to.default_confidence();
    ctx.push_edge(RawEdge {
        from_fqdn: from,
        kind: EdgeKind::Imports,
        to,
        sites: vec![Site {
            file,
            line: line_from_span(span),
            col: col_from_span(span),
        }],
        attributes: vec![],
        confidence,
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
    fn pub_use_emits_phantom_symbol_and_marks_edge_as_re_export() {
        let parsed = parse("pub use foo::Bar;");
        let (symbols, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");

        let phantom = symbols
            .iter()
            .find(|s| s.fqdn == "c::Bar")
            .expect("phantom re-export symbol must be emitted at the short fqdn");
        assert_eq!(phantom.name, "Bar");
        assert!(matches!(phantom.kind, standardoc_ir::Kind::Type));
        assert_eq!(phantom.language_kind.as_str(), "re_export");
        assert!(matches!(
            phantom.visibility,
            standardoc_ir::Visibility::Public
        ));

        let imp = imports(&edges);
        assert_eq!(imp.len(), 1);
        assert!(
            imp[0].attributes.contains(&"re-export".to_string()),
            "edge attributes must mark this as a re-export, got {:?}",
            imp[0].attributes
        );
    }

    #[test]
    fn non_pub_use_does_not_emit_phantom_or_re_export_attribute() {
        let parsed = parse("use foo::Bar;");
        let (symbols, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");

        assert!(
            !symbols.iter().any(|s| s.fqdn == "c::Bar"),
            "non-pub `use` must not produce a phantom symbol"
        );
        let imp = imports(&edges);
        assert_eq!(imp.len(), 1);
        assert!(
            imp[0].attributes.is_empty(),
            "private use must not carry re-export attribute, got {:?}",
            imp[0].attributes
        );
    }

    #[test]
    fn pub_use_with_alias_emits_phantom_at_alias_fqdn() {
        let parsed = parse("pub use foo::Bar as B;");
        let (symbols, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");

        assert!(
            symbols.iter().any(|s| s.fqdn == "c::B"),
            "phantom must use the alias name, not the original"
        );
        assert!(
            !symbols.iter().any(|s| s.fqdn == "c::Bar"),
            "original name must not leak when an alias is given"
        );
        let imp = imports(&edges);
        assert!(imp[0].attributes.contains(&"re-export".to_string()));
    }

    #[test]
    fn pub_use_glob_emits_wildcard_re_export_edge_no_phantom() {
        let parsed = parse("pub use foo::*;");
        let (symbols, edges, _) = walk(&parsed, "c", "src/lib.rs", "c");

        // No phantom symbol for wildcard re-exports — we cannot enumerate
        // the target module's items in a single-file pass.
        assert!(
            symbols.is_empty() || !symbols.iter().any(|s| s.fqdn.contains('*')),
            "wildcard re-export must not synthesize a phantom"
        );
        let imp = imports(&edges);
        assert_eq!(imp.len(), 1);
        assert!(imp[0].attributes.contains(&"re-export".to_string()));
        assert!(imp[0].attributes.contains(&"wildcard".to_string()));
    }

    #[test]
    fn pub_use_group_emits_one_phantom_per_leaf() {
        let parsed = parse("pub use foo::{a, b};");
        let (symbols, _, _) = walk(&parsed, "c", "src/lib.rs", "c");

        assert!(symbols.iter().any(|s| s.fqdn == "c::a"));
        assert!(symbols.iter().any(|s| s.fqdn == "c::b"));
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

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
        receiver_type: None,
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
            decl_kind: None,
            implements_trait: None,
            receiver_type: None,
            entry_point: None,
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
            flags: vec![],
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
        receiver_type: None,
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
        receiver_type: None,
    });
}

#[cfg(test)]
mod tests;

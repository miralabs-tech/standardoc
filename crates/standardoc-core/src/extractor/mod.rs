//! Turns parsed comments and discovered symbols into `DocBlock`s.
//!
//! Bridges the output of language providers (AST symbols + leading comments)
//! with the annotation parser (`@doc`, `@param`, `@returns`, custom tags) and
//! emits canonical `DocBlock`s ready to be inserted into the index.

pub mod annotation;
pub mod comment_scan;

use crate::config::{key_matches_any_exclude, Config, DiscoveryMode, KeyStrategy, SymbolInclusion};
use crate::lang::DiscoveredSymbol;
use crate::model::{
    BlockOrigin, CommentStyle, Diagnostic, DiagnosticCode, DocBlock, DocKey, DocMeta, SourceRange,
    SymbolInfo, TagFields, TagName, Visibility,
};
use crate::validator::{STD002, STD013};
use annotation::{AnnotationWarning, Annotations};
use comment_scan::CommentSpan;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// Per-file context needed to turn a `DiscoveredSymbol` into a `DocBlock`.
#[derive(Debug, Clone)]
pub struct ExtractContext<'a> {
    pub workspace_root: &'a Path,
    pub file_path: PathBuf,
    pub file_ext: String,
    pub comment_style: CommentStyle,
    pub source_mtime: i64,
    pub last_indexed: i64,
}

/// Extracts a `DocBlock` from a single discovered symbol, or returns `None`
/// if the current discovery/inclusion policy excludes it.
pub fn extract_block(
    discovered: DiscoveredSymbol,
    ctx: &ExtractContext<'_>,
    config: &Config,
) -> Option<DocBlock> {
    let annotations = annotation::parse(
        discovered.leading_comment.as_deref().unwrap_or(""),
        &config.doc_tag,
    );

    // Source-level opt-out: `@hide` (or whatever `config.hide_tag` points
    // to) on the comment is enough to exclude the block, regardless of
    // the rest of the pipeline. Short-circuit before any other work.
    if annotations.has(&config.hide_tag) {
        return None;
    }

    let has_doc_tag = annotations.has(&config.doc_tag);

    if !should_emit(
        config.discovery.mode,
        config.discovery.include,
        &discovered.symbol,
        has_doc_tag,
        config.discovery.include_private_with_doc,
    ) {
        return None;
    }

    let relative_path = discovered_relative_path(&ctx.file_path, ctx.workspace_root);
    let origin = decide_origin(config.discovery.mode, has_doc_tag);
    let key = derive_key(
        &discovered,
        &annotations,
        &config.doc_tag,
        config.discovery.key_strategy,
    );

    // External filter via key pattern. Comes AFTER derivation so the user
    // can exclude by canonical key (including any explicit overrides via
    // `@doc` annotation).
    if key_matches_any_exclude(key.as_str(), &config.discovery.exclude) {
        return None;
    }

    let label = derive_label(&discovered, &annotations, &config.doc_tag);

    let tags = annotations.tags;
    let warnings = annotations.warnings;
    let meta = DocMeta {
        path: relative_path,
        line_start: discovered.source_range.line_start,
        line_end: discovered.source_range.line_end,
        column: discovered.source_range.column_start,
        file_ext: ctx.file_ext.clone(),
        comment_style: ctx.comment_style,
        last_indexed: ctx.last_indexed,
        source_mtime: ctx.source_mtime,
    };

    let body_hash = compute_body_hash(&tags, Some(&discovered.symbol));
    let mut diagnostics = build_std002_diagnostics(&warnings, &meta);

    // STD013: explicit `@<doc_tag> <key>` is redundant when the value matches
    // what the configured `KeyStrategy` would have inferred from the FQN.
    // Hint-severity so it doesn't break CI by default — agents and humans
    // can act on it to keep annotations terse.
    if has_doc_tag {
        let inferred_key = match config.discovery.key_strategy {
            KeyStrategy::Fqn => DocKey::new(discovered.fqn.join(".")),
            KeyStrategy::NameOnly => {
                DocKey::new(discovered.fqn.last().cloned().unwrap_or_default())
            }
            KeyStrategy::PathBased => DocKey::new(discovered.fqn.join("::")),
        };
        if key == inferred_key {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::new(STD013.code),
                severity: STD013.severity,
                message: format!(
                    "explicit '@{} {}' is redundant — drop it and let standardoc infer the key from the FQN",
                    config.doc_tag,
                    key.as_str(),
                ),
                path: meta.path.clone(),
                range: SourceRange {
                    line_start: meta.line_start,
                    line_end: meta.line_start,
                    column_start: meta.column,
                    column_end: meta.column,
                },
                related: Vec::new(),
            });
        }
    }

    Some(DocBlock {
        key,
        label,
        origin,
        tags,
        symbol: Some(discovered.symbol),
        meta,
        body_hash,
        diagnostics,
        virtual_tags: BTreeMap::new(),
        virtual_confidence: None,
        virtual_sources: Vec::new(),
    })
}

/// Walk a file's comment spans and emit one satellite `DocBlock` per span
/// carrying `@doc-extend ANCHOR EXTENDED`. Satellite keys take the form
/// `ANCHOR::EXTENDED`, with `symbol = None` (satellites carry no AST info;
/// they are pure tag contributions to an anchor).
///
/// The directive is stripped from the emitted block's `tags` map — it's
/// routing metadata, not a user-facing tag.
///
/// Spans missing the directive, or with fewer than two arguments, are
/// silently skipped; the validator surfaces the malformed cases through
/// dedicated STD codes.
pub fn extract_satellite_blocks(
    spans: &[CommentSpan],
    ctx: &ExtractContext<'_>,
    config: &Config,
) -> Vec<DocBlock> {
    let mut out = Vec::new();
    for span in spans {
        let annotations = annotation::parse(&span.body, &config.doc_tag);
        let Some(extend_fields) = annotations.first("doc-extend") else {
            continue;
        };
        let Some(anchor_key) = extend_fields.first().filter(|s| !s.is_empty()) else {
            continue;
        };
        let Some(ext_key) = extend_fields.get(1).filter(|s| !s.is_empty()) else {
            continue;
        };
        let key = DocKey::new(format!("{anchor_key}::{ext_key}"));
        let label = ext_key.clone();
        let mut tags = annotations.tags;
        tags.remove("doc-extend");

        let meta = DocMeta {
            path: discovered_relative_path(&ctx.file_path, ctx.workspace_root),
            line_start: span.line_start,
            line_end: span.line_end,
            column: span.column,
            file_ext: ctx.file_ext.clone(),
            comment_style: span.style,
            last_indexed: ctx.last_indexed,
            source_mtime: ctx.source_mtime,
        };
        let body_hash = compute_body_hash(&tags, None);

        out.push(DocBlock {
            key,
            label,
            origin: BlockOrigin::Annotated,
            tags,
            symbol: None,
            meta,
            body_hash,
            diagnostics: Vec::new(),
            virtual_tags: BTreeMap::new(),
            virtual_confidence: None,
            virtual_sources: Vec::new(),
        });
    }
    out
}

/// Walk a file's comment spans and emit one anchor `DocBlock` per span
/// carrying a free-floating `@doc K` (i.e. an anchor declared in a comment
/// that is NOT attached to any AST symbol). Use case: declaring documentation
/// keys that don't correspond to a single Rust/TS/Python item — e.g. an MCP
/// tool whose handler is shared across multiple tool names, a virtual concept,
/// or an external API surface.
///
/// The caller is expected to pre-filter `spans` to exclude those leading a
/// discovered symbol — that path is handled by `extract_block`, and including
/// them here would emit duplicate anchor blocks at the same key.
///
/// Spans with `@doc-extend` are skipped (those are satellites, handled by
/// `extract_satellite_blocks`). Spans with no `@doc K` directive are skipped.
pub fn extract_free_floating_anchors(
    spans: &[CommentSpan],
    ctx: &ExtractContext<'_>,
    config: &Config,
) -> Vec<DocBlock> {
    let mut out = Vec::new();
    for span in spans {
        let annotations = annotation::parse(&span.body, &config.doc_tag);
        if annotations.has("doc-extend") {
            continue;
        }
        let Some(doc_fields) = annotations.first(&config.doc_tag) else {
            continue;
        };
        let Some(key_str) = doc_fields.first().filter(|s| !s.is_empty()) else {
            continue;
        };
        let key = DocKey::new(key_str.clone());
        let label = doc_fields.get(1).cloned().unwrap_or_else(|| {
            key_str
                .rsplit('.')
                .next()
                .unwrap_or(key_str.as_str())
                .to_owned()
        });
        let mut tags = annotations.tags;
        tags.remove(config.doc_tag.as_str());

        let meta = DocMeta {
            path: discovered_relative_path(&ctx.file_path, ctx.workspace_root),
            line_start: span.line_start,
            line_end: span.line_end,
            column: span.column,
            file_ext: ctx.file_ext.clone(),
            comment_style: span.style,
            last_indexed: ctx.last_indexed,
            source_mtime: ctx.source_mtime,
        };
        let body_hash = compute_body_hash(&tags, None);

        out.push(DocBlock {
            key,
            label,
            origin: BlockOrigin::Annotated,
            tags,
            symbol: None,
            meta,
            body_hash,
            diagnostics: Vec::new(),
            virtual_tags: BTreeMap::new(),
            virtual_confidence: None,
            virtual_sources: Vec::new(),
        });
    }
    out
}

/// Promotes the parser's `AnnotationWarning`s into STD002 diagnostics
/// attached to the block. The validator will surface them in its output.
/// Position: we point at the symbol line — we don't have the absolute
/// line of the comment in the file, and reconstructing it would require
/// providers to expose the comment's start offset (TODO).
fn build_std002_diagnostics(warnings: &[AnnotationWarning], meta: &DocMeta) -> Vec<Diagnostic> {
    warnings
        .iter()
        .map(|w| Diagnostic {
            code: DiagnosticCode::new(STD002.code),
            severity: STD002.severity,
            message: w.message.clone(),
            path: meta.path.clone(),
            range: SourceRange {
                line_start: meta.line_start,
                line_end: meta.line_start,
                column_start: meta.column,
                column_end: meta.column,
            },
            related: Vec::new(),
        })
        .collect()
}

fn should_emit(
    mode: DiscoveryMode,
    inclusion: SymbolInclusion,
    symbol: &SymbolInfo,
    has_doc_tag: bool,
    include_private_with_doc: bool,
) -> bool {
    if mode == DiscoveryMode::Annotation && !has_doc_tag {
        return false;
    }
    match inclusion {
        SymbolInclusion::All => true,
        SymbolInclusion::AnnotatedOnly => has_doc_tag,
        // `Public` accepts both fully-public items and crate-public items.
        // For binary crates (our CLI + server), everything is `pub(crate)` at
        // best, and excluding those would render half of the project invisible
        // to audits — see the dogfooding findings that motivated this rule.
        SymbolInclusion::Public => match symbol.visibility {
            Visibility::Public | Visibility::Crate => true,
            _ => has_doc_tag && include_private_with_doc,
        },
    }
}

const fn decide_origin(mode: DiscoveryMode, has_doc_tag: bool) -> BlockOrigin {
    match (mode, has_doc_tag) {
        (DiscoveryMode::Annotation, _) => BlockOrigin::Annotated,
        (DiscoveryMode::Hybrid, true) => BlockOrigin::Hybrid,
        (DiscoveryMode::Ast, _) | (DiscoveryMode::Hybrid, false) => BlockOrigin::Inferred,
    }
}

fn derive_key(
    discovered: &DiscoveredSymbol,
    annotations: &Annotations,
    doc_tag: &str,
    strategy: KeyStrategy,
) -> DocKey {
    if let Some(fields) = annotations.first(doc_tag) {
        if let Some(explicit) = fields.first() {
            if !explicit.is_empty() {
                return DocKey::new(explicit.clone());
            }
        }
    }
    match strategy {
        KeyStrategy::Fqn => DocKey::new(discovered.fqn.join(".")),
        KeyStrategy::NameOnly => DocKey::new(discovered.fqn.last().cloned().unwrap_or_default()),
        KeyStrategy::PathBased => DocKey::new(discovered.fqn.join("::")),
    }
}

fn derive_label(discovered: &DiscoveredSymbol, annotations: &Annotations, doc_tag: &str) -> String {
    if let Some(fields) = annotations.first(doc_tag) {
        if let Some(label) = fields.get(1) {
            if !label.is_empty() {
                return label.clone();
            }
        }
    }
    discovered.fqn.last().cloned().unwrap_or_default()
}

fn discovered_relative_path(file: &Path, workspace_root: &Path) -> PathBuf {
    let relative = file
        .strip_prefix(workspace_root)
        .map_or_else(|_| file.to_path_buf(), Path::to_path_buf);
    // Normalize to forward slashes so output JSON / DSL renders are identical
    // on Windows and Unix. `PathBuf` preserves separators, so we rebuild.
    let normalized: String = relative
        .to_string_lossy()
        .chars()
        .map(|c| if c == '\\' { '/' } else { c })
        .collect();
    PathBuf::from(normalized)
}

fn compute_body_hash(tags: &BTreeMap<TagName, Vec<TagFields>>, symbol: Option<&SymbolInfo>) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for (name, occurrences) in tags {
        name.hash(&mut hasher);
        for fields in occurrences {
            for f in fields {
                f.hash(&mut hasher);
            }
        }
    }
    if let Some(s) = symbol {
        s.signature.hash(&mut hasher);
        for p in &s.params {
            p.name.hash(&mut hasher);
            p.type_repr.hash(&mut hasher);
        }
        if let Some(ret) = &s.returns {
            ret.repr.hash(&mut hasher);
        }
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::DiscoveredSymbol;
    use crate::model::{SourceRange, SymbolKind};

    fn ctx() -> ExtractContext<'static> {
        static ROOT: &str = "/workspace";
        ExtractContext {
            workspace_root: Path::new(ROOT),
            file_path: PathBuf::from("/workspace/src/lib.rs"),
            file_ext: "rs".to_owned(),
            comment_style: CommentStyle::DocSingle,
            source_mtime: 0,
            last_indexed: 0,
        }
    }

    fn sym(vis: Visibility, kind: SymbolKind) -> SymbolInfo {
        SymbolInfo {
            kind,
            visibility: vis,
            signature: "fn stub()".to_owned(),
            ..SymbolInfo::default()
        }
    }

    fn discovered(vis: Visibility, comment: Option<&str>) -> DiscoveredSymbol {
        DiscoveredSymbol {
            fqn: vec!["foo".to_owned(), "bar".to_owned()],
            symbol: sym(vis, SymbolKind::Function),
            source_range: SourceRange::single_line(10, 1, 20),
            leading_comment: comment.map(ToOwned::to_owned),
            leading_comment_line_start: None,
        }
    }

    #[test]
    fn hybrid_mode_emits_inferred_without_doc_tag() {
        let config = Config::default();
        let d = discovered(Visibility::Public, None);
        let block = extract_block(d, &ctx(), &config).unwrap();
        assert_eq!(block.origin, BlockOrigin::Inferred);
        assert_eq!(block.key.as_str(), "foo.bar");
        assert!(block.tags.is_empty());
    }

    #[test]
    fn hybrid_mode_emits_hybrid_with_doc_tag() {
        let config = Config::default();
        let d = discovered(
            Visibility::Public,
            Some("@doc custom_key My Label\n@param a i32 first"),
        );
        let block = extract_block(d, &ctx(), &config).unwrap();
        assert_eq!(block.origin, BlockOrigin::Hybrid);
        assert_eq!(block.key.as_str(), "custom_key");
        assert_eq!(block.label, "My Label");
        assert!(block.tags.contains_key("param"));
    }

    #[test]
    fn annotation_mode_skips_symbols_without_doc_tag() {
        let mut config = Config::default();
        config.discovery.mode = DiscoveryMode::Annotation;
        let d = discovered(Visibility::Public, None);
        assert!(extract_block(d, &ctx(), &config).is_none());
    }

    #[test]
    fn annotation_mode_emits_annotated_with_doc_tag() {
        let mut config = Config::default();
        config.discovery.mode = DiscoveryMode::Annotation;
        let d = discovered(Visibility::Public, Some("@doc my_key"));
        let block = extract_block(d, &ctx(), &config).unwrap();
        assert_eq!(block.origin, BlockOrigin::Annotated);
        assert_eq!(block.key.as_str(), "my_key");
    }

    #[test]
    fn public_inclusion_skips_private_without_doc() {
        let config = Config::default();
        let d = discovered(Visibility::Private, None);
        assert!(extract_block(d, &ctx(), &config).is_none());
    }

    #[test]
    fn public_inclusion_keeps_private_with_doc_when_allowed() {
        let config = Config::default();
        let d = discovered(Visibility::Private, Some("@doc private_but_docced"));
        let block = extract_block(d, &ctx(), &config).unwrap();
        assert_eq!(block.key.as_str(), "private_but_docced");
    }

    #[test]
    fn public_inclusion_drops_private_with_doc_when_disabled() {
        let mut config = Config::default();
        config.discovery.include_private_with_doc = false;
        let d = discovered(Visibility::Private, Some("@doc private_but_docced"));
        assert!(extract_block(d, &ctx(), &config).is_none());
    }

    #[test]
    fn key_strategy_name_only_uses_last_segment() {
        let mut config = Config::default();
        config.discovery.key_strategy = KeyStrategy::NameOnly;
        let d = discovered(Visibility::Public, None);
        let block = extract_block(d, &ctx(), &config).unwrap();
        assert_eq!(block.key.as_str(), "bar");
    }

    #[test]
    fn explicit_doc_key_overrides_strategy() {
        let mut config = Config::default();
        config.discovery.key_strategy = KeyStrategy::NameOnly;
        let d = discovered(Visibility::Public, Some("@doc forced_key"));
        let block = extract_block(d, &ctx(), &config).unwrap();
        assert_eq!(block.key.as_str(), "forced_key");
    }

    #[test]
    fn meta_path_is_relative_to_workspace() {
        let config = Config::default();
        let d = discovered(Visibility::Public, None);
        let block = extract_block(d, &ctx(), &config).unwrap();
        assert_eq!(block.meta.path, PathBuf::from("src/lib.rs"));
    }

    #[test]
    fn body_hash_differs_when_tags_change() {
        let config = Config::default();
        let a = extract_block(discovered(Visibility::Public, None), &ctx(), &config).unwrap();
        let b = extract_block(
            discovered(Visibility::Public, Some("@since 2.0.0")),
            &ctx(),
            &config,
        )
        .unwrap();
        assert_ne!(a.body_hash, b.body_hash);
    }

    // ---- @hide source tag + discovery.exclude config (Phase 1) ----

    #[test]
    fn hide_tag_skips_block_entirely() {
        let config = Config::default();
        let d = discovered(Visibility::Public, Some("@hide"));
        assert!(extract_block(d, &ctx(), &config).is_none());
    }

    #[test]
    fn hide_tag_short_circuits_even_with_doc_annotation() {
        // If the user writes both `@doc foo` AND `@hide`, hide wins —
        // it's the final opt-out, never returned.
        let config = Config::default();
        let d = discovered(
            Visibility::Public,
            Some("@doc forced_key\n@hide\n@param a i32 desc"),
        );
        assert!(extract_block(d, &ctx(), &config).is_none());
    }

    #[test]
    fn custom_hide_tag_is_respected() {
        let config = Config {
            hide_tag: "internal".to_owned(),
            ..Config::default()
        };
        let d = discovered(Visibility::Public, Some("@internal"));
        assert!(extract_block(d, &ctx(), &config).is_none());
        // And `@hide` (the default) must NO LONGER exclude when the tag
        // has been renamed.
        let d2 = discovered(Visibility::Public, Some("@hide"));
        assert!(extract_block(d2, &ctx(), &config).is_some());
    }

    #[test]
    fn discovery_exclude_exact_key_match() {
        let mut config = Config::default();
        config.discovery.exclude.push("foo.bar".to_owned());
        let d = discovered(Visibility::Public, None);
        assert!(extract_block(d, &ctx(), &config).is_none());
    }

    #[test]
    fn discovery_exclude_wildcard_descendants() {
        // `foo.*` matches strict descendants (not the `foo` key itself).
        let mut config = Config::default();
        config.discovery.exclude.push("foo.*".to_owned());
        let d = discovered(Visibility::Public, None); // FQN `foo.bar` -> matches
        assert!(extract_block(d, &ctx(), &config).is_none());
    }

    #[test]
    fn discovery_exclude_wildcard_descendants_does_not_match_parent() {
        // `foo.bar.*` must NOT match `foo.bar` (strict descendants).
        let mut config = Config::default();
        config.discovery.exclude.push("foo.bar.*".to_owned());
        let d = discovered(Visibility::Public, None); // key = "foo.bar"
        assert!(extract_block(d, &ctx(), &config).is_some());
    }

    #[test]
    fn discovery_exclude_string_prefix_no_dot() {
        // `foo.b*` matches `foo.bar`, `foo.baz`, etc.
        let mut config = Config::default();
        config.discovery.exclude.push("foo.b*".to_owned());
        let d = discovered(Visibility::Public, None); // key = "foo.bar"
        assert!(extract_block(d, &ctx(), &config).is_none());
    }

    #[test]
    fn discovery_exclude_no_match_keeps_block() {
        let mut config = Config::default();
        config.discovery.exclude.push("other.module.*".to_owned());
        let d = discovered(Visibility::Public, None); // key = "foo.bar"
        assert!(extract_block(d, &ctx(), &config).is_some());
    }

    #[test]
    fn discovery_exclude_applies_to_overridden_doc_key() {
        // When user overrides key via `@doc`, exclusion applies to the new key,
        // which matches the "exclude = view-time policy based on canonical key"
        // principle.
        let mut config = Config::default();
        config.discovery.exclude.push("forced_key".to_owned());
        let d = discovered(Visibility::Public, Some("@doc forced_key"));
        assert!(extract_block(d, &ctx(), &config).is_none());
    }

    // -------- Satellite extraction --------

    fn span(body: &str) -> CommentSpan {
        CommentSpan {
            line_start: 10,
            line_end: 12,
            column: 1,
            style: CommentStyle::SingleLine,
            body: body.to_owned(),
        }
    }

    #[test]
    fn satellite_emits_block_with_double_colon_key() {
        let s = span("@doc-extend mcp.tools.get_doc schema\n@description Schema details.");
        let blocks = extract_satellite_blocks(&[s], &ctx(), &Config::default());
        assert_eq!(blocks.len(), 1);
        let b = &blocks[0];
        assert_eq!(b.key.as_str(), "mcp.tools.get_doc::schema");
        assert_eq!(b.label, "schema");
        assert!(b.symbol.is_none());
        assert_eq!(b.origin, BlockOrigin::Annotated);
        assert!(!b.tags.contains_key("doc-extend"));
        assert_eq!(
            b.tags.get("description").unwrap()[0][0],
            "Schema details."
        );
    }

    #[test]
    fn satellite_skipped_when_doc_extend_has_only_one_arg() {
        let s = span("@doc-extend mcp.tools.get_doc\n@description Half-formed satellite.");
        let blocks = extract_satellite_blocks(&[s], &ctx(), &Config::default());
        assert!(blocks.is_empty());
    }

    #[test]
    fn satellite_meta_uses_span_lines_and_column() {
        let mut s = span("@doc-extend k sat");
        s.line_start = 42;
        s.line_end = 43;
        s.column = 5;
        let blocks = extract_satellite_blocks(&[s], &ctx(), &Config::default());
        assert_eq!(blocks[0].meta.line_start, 42);
        assert_eq!(blocks[0].meta.line_end, 43);
        assert_eq!(blocks[0].meta.column, 5);
    }

    #[test]
    fn satellite_preserves_arbitrary_user_tags() {
        let s = span(
            "@doc-extend mcp.tools.get_doc schema\n@args-schema {\"type\":\"object\"}\n@since 1.0",
        );
        let blocks = extract_satellite_blocks(&[s], &ctx(), &Config::default());
        let b = &blocks[0];
        assert!(b.tags.contains_key("args-schema"));
        assert_eq!(
            b.tags.get("args-schema").unwrap()[0][0],
            "{\"type\":\"object\"}"
        );
        assert_eq!(b.tags.get("since").unwrap()[0][0], "1.0");
    }

    #[test]
    fn no_spans_returns_empty() {
        let blocks = extract_satellite_blocks(&[], &ctx(), &Config::default());
        assert!(blocks.is_empty());
    }

    #[test]
    fn free_floating_anchor_emits_block_with_symbol_none() {
        let s = span("@doc mcp.tools.emit_llms_txt\n@description Generate llms.txt.");
        let blocks = extract_free_floating_anchors(&[s], &ctx(), &Config::default());
        assert_eq!(blocks.len(), 1);
        let b = &blocks[0];
        assert_eq!(b.key.as_str(), "mcp.tools.emit_llms_txt");
        assert_eq!(b.label, "emit_llms_txt");
        assert!(b.symbol.is_none());
        assert_eq!(b.origin, BlockOrigin::Annotated);
        assert!(!b.tags.contains_key("doc"));
        assert_eq!(
            b.tags.get("description").unwrap()[0][0],
            "Generate llms.txt."
        );
    }

    #[test]
    fn free_floating_anchor_skips_satellite_directives() {
        let s = span("@doc-extend mcp.tools.foo args\n@schema {}");
        let blocks = extract_free_floating_anchors(&[s], &ctx(), &Config::default());
        assert!(
            blocks.is_empty(),
            "satellites must be ignored by free-floating anchor extraction"
        );
    }

    #[test]
    fn free_floating_anchor_skips_spans_without_doc_tag() {
        let s = span("@description orphan prose.\n@since 1.0");
        let blocks = extract_free_floating_anchors(&[s], &ctx(), &Config::default());
        assert!(blocks.is_empty());
    }

    #[test]
    fn free_floating_anchor_uses_explicit_label_when_provided() {
        let s = span("@doc mcp.tools.foo Friendly Label\n@description …");
        let blocks = extract_free_floating_anchors(&[s], &ctx(), &Config::default());
        assert_eq!(blocks[0].label, "Friendly Label");
    }
}

//! Web mode: `standardoc-server --web --port <N>` boots a `ServerState`
//! without MCP stdout and exposes the `standardoc-web` HTTP router.
//!
//! `WebStateAdapter` adds no business logic: it maps `WebState` trait methods
//! (`standardoc-web` side) onto `ServerState` operations. It is a pure
//! translation layer between canonical index and REST API wire format.
//!
//! `significant_drop_tightening` is silenced module-wide: each method opens a
//! read lock on index once and keeps it alive for the response duration.
//! This is intentional to guarantee cross-read coherence.

#![allow(clippy::significant_drop_tightening)]

use crate::state::ServerState;
use pulldown_cmark::{html, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use standardoc_core::dsl::render_string;
use standardoc_core::model::{BlockOrigin, DocBlock, RefKind, SymbolKind, Visibility};
use standardoc_core::pages::{DocPage, PageKind};
use standardoc_web::state::{IndexEvent, WebState};
use standardoc_web::types::{
    BlockSummary, DeletePageError, DocExample, DocParam, DocRef, DocReferences, DocResponse,
    DocResponseMeta, DocReturns, PageResponse, PageSummary, ReorderPageError, ResolvedSourceConfig,
    SavePageError, SearchMatch,
};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::broadcast;

const DSL_REFERENCE_MD: &str = include_str!("mcp/dsl_reference.md");

pub(crate) struct WebStateAdapter {
    state: Arc<ServerState>,
}

impl WebStateAdapter {
    pub(crate) const fn new(state: Arc<ServerState>) -> Self {
        Self { state }
    }
}

impl WebState for WebStateAdapter {
    fn revision(&self) -> u64 {
        self.state.revision()
    }

    fn list_blocks(&self) -> Vec<BlockSummary> {
        let idx = self.state.index();
        idx.blocks.values().map(block_summary).collect()
    }

    fn get_doc(&self, key: &str) -> Option<DocResponse> {
        let idx = self.state.index();
        let block = idx.blocks.get(key)?;
        let outgoing = block
            .symbol
            .as_ref()
            .map(|s| {
                s.references
                    .outgoing
                    .iter()
                    .map(|r| DocRef {
                        key: r.target.clone(),
                        label: r.target.clone(),
                        kind: ref_kind_str(r.kind).to_owned(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let incoming = idx
            .incoming
            .get(label_short(&block.label))
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|r| DocRef {
                key: r.from_key.clone(),
                label: r.from_key,
                kind: ref_kind_str(r.kind).to_owned(),
            })
            .collect();
        Some(doc_response(block, DocReferences { outgoing, incoming }))
    }

    fn search(&self, query: &str, limit: usize) -> Vec<SearchMatch> {
        let q_lc = query.to_ascii_lowercase();
        if q_lc.is_empty() {
            return Vec::new();
        }
        let idx = self.state.index();
        let mut scored: Vec<(i32, &DocBlock)> = idx
            .blocks
            .values()
            .filter_map(|b| {
                let mut score = 0;
                if b.key.as_str().to_ascii_lowercase().contains(&q_lc) {
                    score += 3;
                }
                if b.label.to_ascii_lowercase().contains(&q_lc) {
                    score += 2;
                }
                if description_text(b).is_some_and(|t| t.to_ascii_lowercase().contains(&q_lc)) {
                    score += 1;
                }
                if score > 0 {
                    Some((score, b))
                } else {
                    None
                }
            })
            .collect();
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.key.as_str().cmp(b.1.key.as_str()))
        });
        scored
            .into_iter()
            .take(limit)
            .map(|(score, b)| SearchMatch {
                key: b.key.as_str().to_owned(),
                label: b.label.clone(),
                kind: b
                    .symbol
                    .as_ref()
                    .map(|s| symbol_kind_str(s.kind).to_owned()),
                path: b.meta.path.display().to_string(),
                snippet: description_text(b).map(snippet),
                #[allow(clippy::cast_precision_loss)]
                score: score as f32,
            })
            .collect()
    }

    fn dsl_reference_markdown(&self) -> &str {
        DSL_REFERENCE_MD
    }

    fn list_pages(&self) -> Vec<PageSummary> {
        let idx = self.state.index();
        // Phase 3: no longer generate `reference/<key>` auto-pages here.
        // References are rendered by frontend route `/reference/<key>`
        // directly from DocBlock (read-only, immutable). If a user still has
        // a legacy `reference/*` shadow file on disk, we still return it to
        // avoid breakage, but edit flow will refuse persisting it.
        idx.pages.values().map(page_summary_from_disk).collect()
    }

    fn get_page(&self, slug: &str) -> Option<PageResponse> {
        let idx = self.state.index();
        // On-disk page is absolute source of truth. No more server-side
        // `reference/<key>` auto-pages — frontend `/reference/<key>` route
        // renders directly from `get_doc()`.
        idx.pages
            .get(slug)
            .map(|page| page_response_from_disk(page, &self.state, &idx.blocks))
    }

    fn subscribe(&self) -> broadcast::Receiver<IndexEvent> {
        self.state.subscribe_events()
    }

    fn workspace_root(&self) -> &Path {
        self.state.workspace_root()
    }

    fn source_config(&self, is_static_export: bool) -> ResolvedSourceConfig {
        use standardoc_core::config::SourceMode;
        let cfg = &self.state.config().source;
        // Windows: strip the `\\?\` long-path prefix so the resulting
        // `vscode://file/...` URL is well-formed. Forward slashes are added
        // client-side in `buildSourceUrl`.
        let workspace_root_str = {
            let s = self.state.workspace_root().display().to_string();
            s.strip_prefix(r"\\?\").map(str::to_owned).unwrap_or(s)
        };

        // Auto resolution:
        //   daemon          → vscode (local editor)
        //   static export   → github if configured, else source-view
        let effective = match cfg.mode {
            SourceMode::Auto if is_static_export => {
                if cfg.github.is_some() {
                    SourceMode::Github
                } else {
                    SourceMode::SourceView
                }
            }
            SourceMode::Auto => SourceMode::Vscode,
            other => other,
        };

        match effective {
            SourceMode::Vscode | SourceMode::Auto => ResolvedSourceConfig::Vscode {
                workspace_root: workspace_root_str,
            },
            SourceMode::Github => match &cfg.github {
                Some(gh) => ResolvedSourceConfig::Github {
                    repo: gh.repo.clone(),
                    branch: gh.branch.clone(),
                },
                None => ResolvedSourceConfig::SourceView,
            },
            SourceMode::SourceView => ResolvedSourceConfig::SourceView,
        }
    }

    fn save_page(&self, slug: &str, source: &str) -> Result<PageResponse, SavePageError> {
        validate_slug(slug).map_err(|()| SavePageError::InvalidSlug)?;

        // If a page already exists on disk for this slug, write into **its**
        // file (preserves potential `NN-` ordering prefix in filename).
        // Otherwise create `<slug>.md`.
        let target_rel = {
            let idx = self.state.index();
            idx.pages.get(slug).map(|p| p.path.clone())
        };
        let target_abs = target_rel.map_or_else(
            || page_target_path(self.state.workspace_root(), slug),
            |rel| self.state.workspace_root_join(&rel),
        );

        if let Some(parent) = target_abs.parent() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                eprintln!("save_page: mkdir {} failed: {err}", parent.display());
                return Err(SavePageError::IoError);
            }
        }
        if let Err(err) = std::fs::write(&target_abs, source) {
            eprintln!("save_page: write {} failed: {err}", target_abs.display());
            return Err(SavePageError::IoError);
        }

        // Force immediate page rescan so `IndexState.pages` sees new content
        // before watcher fires. Without this, a GET right after PUT could
        // return stale auto-template data (race condition). Also emits SSE
        // event for other connected clients.
        self.state.rescan_pages_now();
        Ok(synthesize_saved_response(
            slug,
            source,
            &target_abs,
            self.state.workspace_root(),
        ))
    }

    fn delete_page(&self, slug: &str) -> Result<(), DeletePageError> {
        validate_slug(slug).map_err(|()| DeletePageError::InvalidSlug)?;

        let target_rel = {
            let idx = self.state.index();
            idx.pages.get(slug).map(|p| p.path.clone())
        };
        let Some(rel) = target_rel else {
            return Err(DeletePageError::NotOnDisk);
        };
        let abs = self.state.workspace_root_join(&rel);
        if !abs.exists() {
            return Err(DeletePageError::NotFound);
        }
        std::fs::remove_file(&abs).map_err(|err| {
            eprintln!("delete_page: remove {} failed: {err}", abs.display());
            DeletePageError::IoError
        })?;
        self.state.rescan_pages_now();
        Ok(())
    }

    fn reorder_page(&self, slug: &str, order: i32) -> Result<(), ReorderPageError> {
        validate_slug(slug).map_err(|()| ReorderPageError::InvalidSlug)?;

        let target_rel = {
            let idx = self.state.index();
            idx.pages.get(slug).map(|p| p.path.clone())
        };
        let Some(rel) = target_rel else {
            return Err(ReorderPageError::NotOnDisk);
        };
        let abs = self.state.workspace_root_join(&rel);
        let source = std::fs::read_to_string(&abs).map_err(|err| {
            eprintln!("reorder_page: read {} failed: {err}", abs.display());
            ReorderPageError::IoError
        })?;

        let new_source = set_frontmatter_order(&source, order);
        std::fs::write(&abs, new_source).map_err(|err| {
            eprintln!("reorder_page: write {} failed: {err}", abs.display());
            ReorderPageError::IoError
        })?;
        self.state.rescan_pages_now();
        Ok(())
    }
}

/// Validate slug for disk writes. Rejects:
/// - `..` segments (path traversal)
/// - empty segments (`/foo//bar`, leading/trailing `/`)
/// - characters outside `[A-Za-z0-9_.\-]`
/// - absolute paths (starting with `/` or `\`)
///
/// Empty slug `""` (home) is valid — maps to `index.md`.
fn validate_slug(slug: &str) -> Result<(), ()> {
    if slug.is_empty() {
        return Ok(());
    }
    if slug.starts_with('/') || slug.starts_with('\\') {
        return Err(());
    }
    // Phase 3: `reference/*` is reserved for the auto-generated reference
    // route. Refuse to persist any user page under that prefix — they should
    // create a guide page that embeds `<Reference key="..." />` instead.
    if slug == "reference" || slug.starts_with("reference/") {
        return Err(());
    }
    for segment in slug.split('/') {
        if segment.is_empty() || segment == ".." || segment == "." {
            return Err(());
        }
        for ch in segment.chars() {
            let allowed = ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.');
            if !allowed {
                return Err(());
            }
        }
    }
    Ok(())
}

/// Map validated slug to absolute write path. `""` -> `index.md`,
/// `"foo/bar"` -> `foo/bar.md`. Extension is always `.md` for new pages —
/// UI v0 cannot create `.mdx` pages (user can rename manually later).
fn page_target_path(workspace_root: &Path, slug: &str) -> std::path::PathBuf {
    let pages_root = workspace_root.join(standardoc_core::pages::PAGES_DIR);
    if slug.is_empty() {
        return pages_root.join("index.md");
    }
    let mut p = pages_root;
    for segment in slug.split('/') {
        p.push(segment);
    }
    p.set_extension("md");
    p
}

fn synthesize_saved_response(
    slug: &str,
    source: &str,
    abs_path: &Path,
    workspace_root: &Path,
) -> PageResponse {
    // Best-effort title extraction from markdown just written.
    // Use frontmatter `title:` or H1 when available, else slug fallback.
    let title = sniff_title(source).unwrap_or_else(|| {
        if slug.is_empty() {
            "Welcome".to_owned()
        } else {
            slug.rsplit('/').next().unwrap_or(slug).to_owned()
        }
    });
    let html = render_markdown(source);
    let path_rel = abs_path
        .strip_prefix(workspace_root)
        .map_or_else(|_| abs_path.to_path_buf(), Path::to_path_buf);
    PageResponse {
        slug: slug.to_owned(),
        title,
        kind: "md".to_owned(),
        frontmatter: BTreeMap::new(),
        source: source.to_owned(),
        html: Some(html),
        on_disk: true,
        path: Some(path_rel.display().to_string()),
        section: section_from_slug(slug),
        order: None,
    }
}

/// Insert or update `order:` field in YAML frontmatter of a markdown file.
/// If frontmatter exists, replace or add the line.
/// If none exists, prepend with `---\norder: N\n---\n\n`.
fn set_frontmatter_order(source: &str, order: i32) -> String {
    if let Some(rest) = source.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            let fm = &rest[..end];
            let after = &rest[end + 5..]; // skip '\n---\n'
                                          // Replace existing `order:` or append it.
            let new_fm = if fm.lines().any(|l| l.starts_with("order:")) {
                fm.lines()
                    .map(|l| {
                        if l.starts_with("order:") {
                            format!("order: {order}")
                        } else {
                            l.to_owned()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                format!("{fm}\norder: {order}")
            };
            return format!("---\n{new_fm}\n---\n{after}");
        }
    }
    // No frontmatter — prepend one.
    format!("---\norder: {order}\n---\n\n{source}")
}

fn sniff_title(source: &str) -> Option<String> {
    // Look for `title:` in frontmatter, then fall back to first H1.
    if let Some(rest) = source.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            for line in rest[..end].lines() {
                if let Some(v) = line.strip_prefix("title:") {
                    let v = v.trim();
                    let unquoted = v
                        .strip_prefix('"')
                        .and_then(|s| s.strip_suffix('"'))
                        .or_else(|| v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
                        .unwrap_or(v);
                    if !unquoted.is_empty() {
                        return Some(unquoted.to_owned());
                    }
                }
            }
        }
    }
    for line in source.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("# ") {
            return Some(rest.trim().to_owned());
        }
        if !t.is_empty() && !t.starts_with("---") {
            break;
        }
    }
    None
}

fn section_from_slug(slug: &str) -> Vec<String> {
    if slug.is_empty() {
        return Vec::new();
    }
    let parts: Vec<&str> = slug.split('/').collect();
    if parts.len() <= 1 {
        return Vec::new();
    }
    parts[..parts.len() - 1]
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
}

fn page_summary_from_disk(page: &DocPage) -> PageSummary {
    PageSummary {
        slug: page.slug.clone(),
        title: page.title.clone(),
        section: page.section.clone(),
        order: page.order,
        kind: match page.kind {
            PageKind::Md => "md".to_owned(),
            PageKind::Mdx => "mdx".to_owned(),
        },
        on_disk: true,
    }
}

fn page_response_from_disk(
    page: &DocPage,
    state: &ServerState,
    blocks: &BTreeMap<String, DocBlock>,
) -> PageResponse {
    // 1. DSL eval against index -> resolved markdown. Expressions like
    //    `{{ @doc.X:label }}` become concrete values.
    let dsl_resolved = render_string(&page.raw_body, blocks, &state.config().tags)
        .unwrap_or_else(|err| format!("<!-- standardoc DSL error: {err} -->\n\n{}", page.raw_body));

    // 2. For `Md`, render server-side HTML (pulldown-cmark). For `Mdx`,
    //    let client compile it — it has access to `source`.
    let html = if page.kind == PageKind::Md {
        Some(render_markdown(&dsl_resolved))
    } else {
        None
    };

    PageResponse {
        slug: page.slug.clone(),
        title: page.title.clone(),
        kind: match page.kind {
            PageKind::Md => "md".to_owned(),
            PageKind::Mdx => "mdx".to_owned(),
        },
        frontmatter: page.frontmatter.clone(),
        source: dsl_resolved,
        html,
        on_disk: true,
        path: Some(page.path.display().to_string()),
        section: page.section.clone(),
        order: page.order,
    }
}

fn block_summary(block: &DocBlock) -> BlockSummary {
    BlockSummary {
        key: block.key.as_str().to_owned(),
        label: block.label.clone(),
        kind: block
            .symbol
            .as_ref()
            .map(|s| symbol_kind_str(s.kind).to_owned()),
        path: block.meta.path.display().to_string(),
        line_start: block.meta.line_start,
        signature: block.symbol.as_ref().map(|s| s.signature.clone()),
        has_description: description_text(block).is_some(),
        module_path: split_key_segments(block.key.as_str()),
        deprecated: block.symbol.as_ref().is_some_and(|s| s.is_deprecated)
            || block.tags.contains_key("deprecated"),
    }
}

/// Split a `DocKey` into hierarchical segments for the sidebar.
///
/// Canonical separator is `.`. But we must ignore dots inside Rust-style impl
/// blocks like `<Type as Trait>` or generics `Foo<A, B>`. So we scan while
/// tracking `<...>` depth — a `.` at depth 0 is a separator, otherwise it is
/// part of current segment.
///
/// Exemples :
/// - `api.users.create` → `["api", "users", "create"]`
/// - `standardoc_core.config.<Config as Default>.default`
///   → `["standardoc_core", "config", "<Config as Default>", "default"]`
/// - `<BTreeMap<DocKey,DocBlock> as BlockSource>.get`
///   → `["<BTreeMap<DocKey,DocBlock> as BlockSource>", "get"]`
fn split_key_segments(key: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut depth: i32 = 0;
    for ch in key.chars() {
        match ch {
            '<' => {
                depth += 1;
                current.push(ch);
            }
            '>' => {
                depth -= 1;
                current.push(ch);
            }
            '.' if depth == 0 => {
                if !current.is_empty() {
                    segments.push(std::mem::take(&mut current));
                }
            }
            other => current.push(other),
        }
    }
    if !current.is_empty() {
        segments.push(current);
    }
    segments
}

fn doc_response(block: &DocBlock, references: DocReferences) -> DocResponse {
    let description_md = description_text(block).map(ToOwned::to_owned);
    let description_html = description_md.as_deref().map(render_markdown);
    let params = build_params(block);
    let returns = build_returns(block);
    let examples = block
        .tags
        .get("example")
        .map(|occs| occs.iter().map(|fields| build_example(fields)).collect())
        .unwrap_or_default();
    let see = block
        .tags
        .get("see")
        .map(|occs| {
            occs.iter()
                .filter_map(|fields| fields.first().cloned())
                .collect()
        })
        .unwrap_or_default();
    let deprecated = block
        .tags
        .get("deprecated")
        .and_then(|occs| occs.first())
        .and_then(|fields| fields.first().cloned());
    let since = block
        .tags
        .get("since")
        .and_then(|occs| occs.first())
        .and_then(|fields| fields.first().cloned());

    let custom_tags = collect_custom_tags(block);

    DocResponse {
        key: block.key.as_str().to_owned(),
        label: block.label.clone(),
        origin: origin_str(block.origin).to_owned(),
        kind: block
            .symbol
            .as_ref()
            .map(|s| symbol_kind_str(s.kind).to_owned()),
        visibility: block
            .symbol
            .as_ref()
            .map(|s| visibility_str(s.visibility).to_owned()),
        signature: block.symbol.as_ref().map(|s| s.signature.clone()),
        description_md,
        description_html,
        params,
        returns,
        examples,
        see,
        deprecated,
        since,
        meta: DocResponseMeta {
            path: block.meta.path.display().to_string(),
            line_start: block.meta.line_start,
            line_end: block.meta.line_end,
            file_ext: block.meta.file_ext.clone(),
        },
        custom_tags,
        references,
    }
}

fn build_params(block: &DocBlock) -> Vec<DocParam> {
    // Source of truth: if AST symbol exists, start from its ParamInfo and
    // enrich with descriptions from `@param` tags. Otherwise build only
    // from tags.
    let tag_descriptions: BTreeMap<String, String> = block
        .tags
        .get("param")
        .map(|occs| {
            occs.iter()
                .filter_map(|fields| {
                    let name = fields.first()?.clone();
                    let description = fields.get(2).cloned().or_else(|| fields.get(1).cloned());
                    Some((name, description.unwrap_or_default()))
                })
                .collect()
        })
        .unwrap_or_default();

    if let Some(sym) = &block.symbol {
        if !sym.params.is_empty() {
            return sym
                .params
                .iter()
                .map(|p| DocParam {
                    name: p.name.clone(),
                    type_repr: p.type_repr.clone(),
                    description: tag_descriptions.get(&p.name).cloned(),
                    is_optional: p.is_optional,
                    is_variadic: p.is_variadic,
                })
                .collect();
        }
    }

    block
        .tags
        .get("param")
        .map(|occs| {
            occs.iter()
                .filter_map(|fields| {
                    let name = fields.first()?.clone();
                    Some(DocParam {
                        name,
                        type_repr: fields.get(1).cloned(),
                        description: fields.get(2).cloned(),
                        is_optional: false,
                        is_variadic: false,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn build_returns(block: &DocBlock) -> Option<DocReturns> {
    let symbol_type = block
        .symbol
        .as_ref()
        .and_then(|s| s.returns.as_ref())
        .map(|t| t.repr.clone());
    let tag = block
        .tags
        .get("returns")
        .or_else(|| block.tags.get("return"))
        .and_then(|occs| occs.first());
    let tag_description = tag.and_then(|fields| {
        if fields.len() >= 2 {
            Some(fields[1..].join(" "))
        } else {
            fields.first().cloned()
        }
    });
    let tag_type = tag.and_then(|fields| fields.first().cloned());
    let type_repr = symbol_type.or(tag_type);
    if type_repr.is_none() && tag_description.is_none() {
        None
    } else {
        Some(DocReturns {
            type_repr,
            description: tag_description,
        })
    }
}

fn build_example(fields: &[String]) -> DocExample {
    let title = if fields.len() > 1 {
        fields.first().cloned()
    } else {
        None
    };
    let code = fields.last().cloned().unwrap_or_default();
    let language: Option<String> = None;
    let code_html = standardoc_web::highlight::highlight_code(&code, language.as_deref());
    DocExample {
        title,
        language,
        code,
        code_html,
    }
}

fn collect_custom_tags(block: &DocBlock) -> BTreeMap<String, Vec<Vec<String>>> {
    const BUILTIN: &[&str] = &[
        "description",
        "param",
        "return",
        "returns",
        "example",
        "see",
        "since",
        "deprecated",
    ];
    block
        .tags
        .iter()
        .filter(|(name, _)| !BUILTIN.contains(&name.as_str()))
        .map(|(name, occs)| (name.clone(), occs.clone()))
        .collect()
}

/// Markdown rendering -> HTML with syntect for code blocks.
///
/// We intercept `CodeBlock` events in pulldown-cmark stream and route them to
/// `standardoc_web::highlight::highlight_code`, which emits
/// `<span class="hl-*">` recognized by `/api/syntax.css`. The rest of markdown
/// (paragraphs, lists, tables, ...) goes through standard push_html.
fn render_markdown(src: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_TASKLISTS);

    let parser = Parser::new_ext(src, opts);
    let mut events: Vec<Event<'_>> = Vec::new();
    let mut code_lang: Option<String> = None;
    let mut code_buf = String::new();
    let mut in_code = false;

    for event in parser {
        match event {
            Event::Start(Tag::CodeBlock(ref kind)) => {
                in_code = true;
                code_lang = match kind {
                    CodeBlockKind::Fenced(lang) => {
                        let l = lang.trim();
                        if l.is_empty() {
                            None
                        } else {
                            Some(l.to_owned())
                        }
                    }
                    CodeBlockKind::Indented => None,
                };
                code_buf.clear();
            }
            Event::Text(ref text) if in_code => {
                code_buf.push_str(text);
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code = false;
                let highlighted =
                    standardoc_web::highlight::highlight_code(&code_buf, code_lang.as_deref());
                events.push(Event::Html(highlighted.into()));
                code_lang = None;
                code_buf.clear();
            }
            other if !in_code => events.push(other),
            _ => {}
        }
    }

    let mut html_buf = String::with_capacity(src.len());
    html::push_html(&mut html_buf, events.into_iter());
    html_buf
}

fn description_text(block: &DocBlock) -> Option<&str> {
    block
        .tags
        .get("description")
        .and_then(|occs| occs.first())
        .and_then(|fields| fields.first())
        .map(String::as_str)
}

fn snippet(text: &str) -> String {
    const MAX: usize = 200;
    if text.len() <= MAX {
        text.to_owned()
    } else {
        let mut out = text[..MAX].to_owned();
        out.push('…');
        out
    }
}

fn label_short(label: &str) -> &str {
    label
        .rsplit_once("::")
        .map_or(label, |(_, tail)| tail)
        .rsplit_once('.')
        .map_or_else(
            || label.rsplit_once("::").map_or(label, |(_, tail)| tail),
            |(_, tail)| tail,
        )
}

const fn origin_str(o: BlockOrigin) -> &'static str {
    match o {
        BlockOrigin::Inferred => "inferred",
        BlockOrigin::Annotated => "annotated",
        BlockOrigin::Hybrid => "hybrid",
    }
}

const fn symbol_kind_str(k: SymbolKind) -> &'static str {
    match k {
        SymbolKind::Function => "function",
        SymbolKind::Method => "method",
        SymbolKind::Class => "class",
        SymbolKind::Struct => "struct",
        SymbolKind::Enum => "enum",
        SymbolKind::Trait => "trait",
        SymbolKind::Interface => "interface",
        SymbolKind::TypeAlias => "type-alias",
        SymbolKind::Const => "const",
        SymbolKind::Static => "static",
        SymbolKind::Module => "module",
        SymbolKind::Macro => "macro",
        SymbolKind::Field => "field",
        SymbolKind::Variant => "variant",
        SymbolKind::Other => "other",
    }
}

const fn visibility_str(v: Visibility) -> &'static str {
    match v {
        Visibility::Public => "public",
        Visibility::Private => "private",
        Visibility::Crate => "crate",
        Visibility::Internal => "internal",
        Visibility::Inherited => "inherited",
    }
}

const fn ref_kind_str(k: RefKind) -> &'static str {
    match k {
        RefKind::Call => "call",
        RefKind::ParamType => "param-type",
        RefKind::ReturnType => "return-type",
        RefKind::FieldType => "field-type",
        RefKind::Implements => "implements",
        RefKind::Extends => "extends",
        RefKind::GenericArg => "generic-arg",
        RefKind::Other => "other",
    }
}

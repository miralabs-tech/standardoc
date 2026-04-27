//! The v1 tool set.
//!
//! Every tool receives `(state, params)` and returns `Result<Value, JsonRpcError>`.
//! Successful outcomes are wrapped as MCP `text` content; tool-level failures
//! (e.g. key not found, bad DSL) are wrapped with `isError: true` so the agent
//! sees them without the whole JSON-RPC call failing.
//!
//! `significant_drop_tightening` is silenced module-wide: each handler opens
//! a read-lock on the index once and keeps it alive for the duration of the
//! call. That's intentional — we want a coherent view across all sub-reads.

#![allow(clippy::significant_drop_tightening, clippy::format_push_string)]

use super::protocol::{text_content, tool_error_content, JsonRpcError};
use crate::state::ServerState;
use serde_json::{json, Value};
use standardoc_core::dsl::{parse, render_string};
use standardoc_core::emit::{
    emit_llms_full, emit_llms_txt, emit_openapi, emit_skill_md, EmitOptions, OpenApiOptions,
};
use standardoc_core::extractor::annotation;
use standardoc_core::model::{BlockOrigin, DocBlock, RefKind, Severity};
use standardoc_core::validator::validate;
use std::collections::BTreeMap;
use std::path::Path;

pub(crate) fn call(state: &ServerState, params: &Value) -> Result<Value, JsonRpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| JsonRpcError::invalid_params("missing 'name'"))?;
    let args = params.get("arguments").unwrap_or(&Value::Null);

    match name {
        "list_docs" => Ok(list_docs(state, args)),
        "get_doc" => Ok(get_doc(state, args)),
        "search_docs" => Ok(search_docs(state, args)),
        "evaluate_dsl" => Ok(evaluate_dsl(state, args)),
        "render_markdown" => Ok(render_markdown(state, args)),
        "get_dsl_reference" => Ok(get_dsl_reference()),
        "find_undocumented" => Ok(find_undocumented(state)),
        "rescan" => Ok(rescan(state)),
        "validate_doc_syntax" => Ok(validate_doc_syntax(state, args)),
        "coverage_report" => Ok(coverage_report(state)),
        "list_collisions" => Ok(list_collisions(state)),
        "set_watch_paused" => Ok(set_watch_paused(state, args)),
        "get_watch_status" => Ok(get_watch_status(state)),
        "find_usages" => Ok(find_usages(state, args)),
        "find_implementations" => Ok(find_implementations(state, args)),
        "search_by_return_type" => Ok(search_by_kind(state, args, RefKind::ReturnType)),
        "search_by_param_type" => Ok(search_by_kind(state, args, RefKind::ParamType)),
        "get_type_hierarchy" => Ok(get_type_hierarchy(state, args)),
        "emit_llms_txt" => Ok(emit_format(state, args, EmitFormat::LlmsTxt)),
        "emit_llms_full" => Ok(emit_format(state, args, EmitFormat::LlmsFull)),
        "emit_skill_md" => Ok(emit_format(state, args, EmitFormat::SkillMd)),
        "emit_openapi" => Ok(emit_openapi_tool(state, args)),
        "resolve_symbol" => Ok(resolve_symbol(state, args)),
        "get_definition" => Ok(get_definition(state, args)),
        "get_hover" => Ok(get_hover(state, args)),
        "find_references" => Ok(find_references(state, args)),
        "get_comments" => Ok(get_comments(state, args)),
        "list_diagnostics" => Ok(list_diagnostics(state, args)),
        other => Err(JsonRpcError::invalid_params(format!(
            "unknown tool: {other}"
        ))),
    }
}

/// @doc mcp.tools.resolve_symbol
/// @description Resolve a short symbol name (label or FQN suffix) to one or more doc blocks. Prefers exact label matches, then FQN-suffix matches. Use this before `get_doc` when you only know the short name (e.g. 'LanguageProvider') and need the canonical key. Optional `path_prefix` scopes results to a subtree.
/// @category lsp
fn resolve_symbol(state: &ServerState, args: &Value) -> Value {
    let Some(name) = args.get("name").and_then(Value::as_str) else {
        return tool_error_content("missing 'name' argument");
    };
    let path_prefix = args.get("path_prefix").and_then(Value::as_str);
    let idx = state.index();

    // Priority: exact label > FQN suffix > FQN contains
    let suffix_pat = format!(".{name}");
    let mut exact: Vec<&DocBlock> = Vec::new();
    let mut suffix: Vec<&DocBlock> = Vec::new();
    let mut contains: Vec<&DocBlock> = Vec::new();

    for b in idx.blocks.values() {
        if let Some(prefix) = path_prefix {
            if !b.meta.path.to_string_lossy().starts_with(prefix) {
                continue;
            }
        }
        if b.label == name {
            exact.push(b);
        } else if b.key.as_str().ends_with(&suffix_pat) || b.key.as_str() == name {
            suffix.push(b);
        } else if b.key.as_str().contains(name) {
            contains.push(b);
        }
    }

    let results: Vec<&DocBlock> = if !exact.is_empty() {
        exact
    } else if !suffix.is_empty() {
        suffix
    } else {
        contains
    };

    let entries: Vec<Value> = results
        .iter()
        .map(|b| {
            json!({
                "key": b.key.as_str(),
                "label": b.label,
                "kind": b.symbol.as_ref().map(|s| format!("{:?}", s.kind).to_lowercase()),
                "path": b.meta.path,
                "line": b.meta.line_start,
            })
        })
        .collect();

    text_content(
        json!({
            "name": name,
            "count": entries.len(),
            "candidates": entries,
        })
        .to_string(),
    )
}

/// @doc mcp.tools.get_definition
/// @description Return the source file location (path + line) for a doc block key. Lighter than `get_doc` when you only need to navigate to the definition, not read the full block.
/// @category lsp
fn get_definition(state: &ServerState, args: &Value) -> Value {
    let Some(key) = args.get("key").and_then(Value::as_str) else {
        return tool_error_content("missing 'key' argument");
    };
    let idx = state.index();
    let Some(block) = idx.blocks.get(key) else {
        return tool_error_content(format!("unknown key: {key}"));
    };
    let abs_path = state.workspace_root_join(&block.meta.path);
    text_content(
        json!({
            "key": key,
            "path": block.meta.path,
            "abs_path": abs_path,
            "line": block.meta.line_start,
        })
        .to_string(),
    )
}

/// @doc mcp.tools.get_hover
/// @description Return a formatted Markdown summary for a doc block (label, signature, description, source location) — the same text the LSP shows on hover. Use this for a quick human-readable snapshot of a symbol without parsing the full `get_doc` JSON.
/// @category lsp
fn get_hover(state: &ServerState, args: &Value) -> Value {
    let Some(key) = args.get("key").and_then(Value::as_str) else {
        return tool_error_content("missing 'key' argument");
    };
    let idx = state.index();
    let block = idx
        .blocks
        .get(key)
        .or_else(|| idx.blocks.values().find(|b| b.label == key))
        .or_else(|| {
            let suffix = format!(".{key}");
            idx.blocks
                .values()
                .find(|b| b.key.as_str().ends_with(&suffix))
        });
    let Some(block) = block else {
        return tool_error_content(format!("no block found for '{key}'"));
    };
    let actual_key = block.key.as_str();
    let mut md = format!("**{}** — `{}`", block.label, actual_key);
    if let Some(symbol) = &block.symbol {
        md.push_str(&format!("\n\n```\n{}\n```", symbol.signature));
    }
    if let Some(desc) = description_text(block) {
        md.push_str("\n\n");
        md.push_str(&desc);
    }
    md.push_str(&format!(
        "\n\n_source: `{}:{}`_",
        block.meta.path.display(),
        block.meta.line_start,
    ));
    text_content(json!({ "key": actual_key, "markdown": md }).to_string())
}

/// @doc mcp.tools.find_references
/// @description Scan narrative `.md` pages for occurrences of `@doc.KEY`. Distinct from `find_usages` (which follows type-graph edges in source): this finds where the key is *mentioned* in documentation pages. Returns [{page, line, context}].
/// @category lsp
fn find_references(state: &ServerState, args: &Value) -> Value {
    let Some(key) = args.get("key").and_then(Value::as_str) else {
        return tool_error_content("missing 'key' argument");
    };
    // Build the set of needles: canonical key + short name (last FQN segment).
    let short = key.rsplit_once('.').map_or(key, |(_, s)| s);
    let needles: Vec<&str> = if short == key {
        vec![key]
    } else {
        vec![key, short]
    };

    let workspace_root = state.workspace_root().to_path_buf();
    let pages: Vec<(std::path::PathBuf, String)> = {
        let idx = state.index();
        idx.pages
            .values()
            .map(|p| (workspace_root.join(&p.path), p.raw_body.clone()))
            .collect()
    };

    let mut entries: Vec<Value> = Vec::new();
    for (abs_path, body) in &pages {
        let rel_path = abs_path
            .strip_prefix(&workspace_root)
            .unwrap_or(abs_path)
            .to_string_lossy()
            .to_string();
        for (line_idx, line) in body.lines().enumerate() {
            let matches_any = needles.iter().any(|needle| {
                // Match `@doc.NEEDLE` or `@docs.module(NEEDLE,...)` patterns.
                line.contains(&format!("@doc.{needle}"))
                    || line.contains(&format!("({needle}"))
                    || line.contains(&format!(" {needle}"))
            });
            if matches_any {
                entries.push(json!({
                    "page": rel_path,
                    "line": line_idx + 1,
                    "context": line.trim(),
                }));
            }
        }
    }

    text_content(
        json!({
            "key": key,
            "count": entries.len(),
            "references": entries,
        })
        .to_string(),
    )
}

/// @doc mcp.tools.get_comments
/// @description Extract all comment nodes from a single source file using tree-sitter. Returns [{line, text}] for every comment in the file (single-line, multi-line, and doc comments). Works for any language with a registered tree-sitter grammar (e.g. Lua, Teal, and any language defined in .standardoc/languages/). Does not require the file to be indexed or annotated — useful for auditing, translation, or exploring undocumented code.
/// @category navigation
fn get_comments(state: &ServerState, args: &Value) -> Value {
    let Some(file_str) = args.get("file").and_then(Value::as_str) else {
        return tool_error_content("missing 'file' argument");
    };
    let path: std::path::PathBuf = if Path::new(file_str).is_absolute() {
        file_str.into()
    } else {
        state.workspace_root_join(Path::new(file_str))
    };
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    let Some(lang_fn) = state.ts_language_for_extension(&ext) else {
        return tool_error_content(format!(
            "no tree-sitter grammar registered for '{ext}' — supported extensions come from built-in and .standardoc/languages/ providers"
        ));
    };
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => return tool_error_content(format!("could not read '{}': {e}", path.display())),
    };
    let comments = standardoc_lang_tree_sitter::extract_comments(lang_fn, &content);
    let entries: Vec<Value> = comments
        .iter()
        .map(|c| json!({ "line": c.line + 1, "text": c.text }))
        .collect();
    text_content(
        json!({
            "file": file_str,
            "language": &ext[1..],
            "count": entries.len(),
            "comments": entries,
        })
        .to_string(),
    )
}

/// @doc mcp.tools.list_diagnostics
/// @description Run all validator rules against the current index and return diagnostics. Codes shipped: STD001 (dup key), STD002 (malformed @tag — missing key/name/type), STD003 (param missing description), STD004 (DSL ref to unknown DocKey in narrative pages), STD005 (no block description), STD006 (public symbol without @doc), STD007 (DSL syntax error in narrative pages), STD008 (param name not in signature), STD012 (param type mismatch). Filter by `severity` ('error'/'warning'/'info'/'hint') or `code` ('STD001', etc.).
/// @category quality
fn list_diagnostics(state: &ServerState, args: &Value) -> Value {
    let severity_filter = args
        .get("severity")
        .and_then(Value::as_str)
        .and_then(parse_severity);
    let code_filter = args.get("code").and_then(Value::as_str);

    let idx = state.index();
    let diagnostics = validate(&idx.blocks, &idx.collisions, &idx.pages, state.config());

    let entries: Vec<Value> = diagnostics
        .iter()
        .filter(|d| severity_filter.is_none_or(|s| d.severity == s))
        .filter(|d| code_filter.is_none_or(|c| d.code.as_str() == c))
        .map(|d| {
            json!({
                "code": d.code.as_str(),
                "severity": severity_str(d.severity),
                "message": &d.message,
                "path": &d.path,
                "line": d.range.line_start,
            })
        })
        .collect();

    let mut by_severity: BTreeMap<&str, usize> = BTreeMap::new();
    for d in &diagnostics {
        *by_severity.entry(severity_str(d.severity)).or_insert(0) += 1;
    }

    text_content(
        json!({
            "count": entries.len(),
            "total": diagnostics.len(),
            "by_severity": by_severity,
            "entries": entries,
        })
        .to_string(),
    )
}

fn parse_severity(s: &str) -> Option<Severity> {
    match s {
        "error" => Some(Severity::Error),
        "warning" => Some(Severity::Warning),
        "info" => Some(Severity::Info),
        "hint" => Some(Severity::Hint),
        _ => None,
    }
}

const fn severity_str(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
        Severity::Hint => "hint",
    }
}

#[derive(Clone, Copy)]
enum EmitFormat {
    LlmsTxt,
    LlmsFull,
    SkillMd,
}

fn emit_format(state: &ServerState, args: &Value, format: EmitFormat) -> Value {
    let opts = EmitOptions {
        project_name: args
            .get("name")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        tagline: args
            .get("tagline")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        link_base: args
            .get("link_base")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
    };
    let idx = state.index();
    let output = match format {
        EmitFormat::LlmsTxt => emit_llms_txt(&idx.blocks, &opts),
        EmitFormat::LlmsFull => emit_llms_full(&idx.blocks, &opts),
        EmitFormat::SkillMd => emit_skill_md(&idx.blocks, &opts),
    };
    text_content(output)
}

/// @doc mcp.tools.emit_openapi emit_openapi
/// @description Generate an OpenAPI 3.0 spec from blocks tagged with `@route METHOD PATH`. Recognized companion tags: `@param NAME TYPE description` (path param if `{NAME}` is in the route, else query), `@response CODE description`, `@request_body TYPE description`. Blocks without `@route` are ignored — empty `paths` is the expected output for projects that don't expose an HTTP API.
/// @category emit
fn emit_openapi_tool(state: &ServerState, args: &Value) -> Value {
    let opts = OpenApiOptions {
        title: args
            .get("title")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        version: args
            .get("version")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        description: args
            .get("description")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        servers: args
            .get("servers")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(ToOwned::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
    };
    let idx = state.index();
    let spec = emit_openapi(&idx.blocks, &opts);
    // Return pretty-printed JSON so copy/paste stays readable for agents.
    // `to_string_pretty` can fail on exotic values (cycles, NaN) — unlikely
    // here, but we fall back to compact serialization on error.
    let serialized = serde_json::to_string_pretty(&spec).unwrap_or_else(|_| spec.to_string());
    text_content(serialized)
}

/// @doc mcp.tools.list_docs
/// @description List all documentable blocks in the workspace. Supports substring filtering on key/label and pagination.
/// @category navigation
fn list_docs(state: &ServerState, args: &Value) -> Value {
    let filter = args.get("filter").and_then(Value::as_str).unwrap_or("");
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map_or(200, |n| usize::try_from(n).unwrap_or(usize::MAX));
    let offset = args
        .get("offset")
        .and_then(Value::as_u64)
        .map_or(0, |n| usize::try_from(n).unwrap_or(0));

    let filter_lc = filter.to_ascii_lowercase();
    let idx = state.index();
    let matches: Vec<&DocBlock> = idx
        .blocks
        .values()
        .filter(|b| {
            filter_lc.is_empty()
                || b.key.as_str().to_ascii_lowercase().contains(&filter_lc)
                || b.label.to_ascii_lowercase().contains(&filter_lc)
        })
        .collect();
    let total = matches.len();
    let entries: Vec<Value> = matches
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|b| {
            json!({
                "key": b.key.as_str(),
                "label": b.label,
                "kind": b.symbol.as_ref().map(|s| format!("{:?}", s.kind).to_lowercase()),
                "origin": origin_str(b.origin),
                "path": b.meta.path,
                "line": b.meta.line_start,
            })
        })
        .collect();

    let payload = json!({
        "entries": entries,
        "total": total,
        "revision": state.revision(),
    });
    text_content(payload.to_string())
}

/// @doc mcp.tools.get_doc
/// @description Fetch a single DocBlock by its key.
/// @category navigation
fn get_doc(state: &ServerState, args: &Value) -> Value {
    let Some(key) = args.get("key").and_then(Value::as_str) else {
        return tool_error_content("missing 'key' argument");
    };
    let idx = state.index();
    let Some(block) = idx.blocks.get(key) else {
        return tool_error_content(format!("unknown key: {key}"));
    };
    match serde_json::to_string(block) {
        Ok(json) => text_content(json),
        Err(err) => tool_error_content(format!("serialize failed: {err}")),
    }
}

/// @doc mcp.tools.search_docs
/// @description Search the doc index by query. First tries exact substring matching across key, label, and description. If that returns nothing, automatically falls back to token-based fuzzy search across all fields (signature, params, return type, all tags). The response includes `"mode": "exact" | "fuzzy"` so callers know which path was taken.
/// @category navigation
fn search_docs(state: &ServerState, args: &Value) -> Value {
    let Some(query) = args.get("query").and_then(Value::as_str) else {
        return tool_error_content("missing 'query' argument");
    };
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map_or(20, |n| usize::try_from(n).unwrap_or(usize::MAX));
    let q_lc = query.to_ascii_lowercase();

    let idx = state.index();

    let mut scored: Vec<(i32, &DocBlock)> = idx
        .blocks
        .values()
        .filter_map(|b| {
            let mut score = 0i32;
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

    let mode = if scored.is_empty() {
        // Auto-fallback: split the query into tokens and search all fields.
        scored = fuzzy_score_blocks(idx.blocks.values(), &q_lc);
        "fuzzy"
    } else {
        "exact"
    };

    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.key.as_str().cmp(b.1.key.as_str()))
    });

    let hits: Vec<Value> = scored
        .into_iter()
        .take(limit)
        .map(|(score, b)| {
            json!({
                "key": b.key.as_str(),
                "label": b.label,
                "score": score,
                "snippet": description_text(b).unwrap_or_default(),
            })
        })
        .collect();

    text_content(
        json!({
            "mode": mode,
            "count": hits.len(),
            "hits": hits,
        })
        .to_string(),
    )
}

/// @doc mcp.tools.evaluate_dsl
/// @description Evaluate a single DSL expression (e.g. '@doc.foo:label') against the index. Returns the rendered string.
/// @category navigation
fn evaluate_dsl(state: &ServerState, args: &Value) -> Value {
    let Some(expr) = args.get("expression").and_then(Value::as_str) else {
        return tool_error_content("missing 'expression' argument");
    };
    let template_src = format!("{{{{ {expr} }}}}");
    let idx = state.index();
    match render_string(&template_src, &idx.blocks, &state.config().tags) {
        Ok(result) => text_content(result),
        Err(err) => tool_error_content(format!("{err}")),
    }
}

/// @doc mcp.tools.render_markdown
/// @description Render a markdown template containing DSL expressions against the current index.
/// @category navigation
fn render_markdown(state: &ServerState, args: &Value) -> Value {
    let Some(content) = args.get("content").and_then(Value::as_str) else {
        return tool_error_content("missing 'content' argument");
    };
    let idx = state.index();
    match render_string(content, &idx.blocks, &state.config().tags) {
        Ok(rendered) => text_content(rendered),
        Err(err) => tool_error_content(format!("{err}")),
    }
}

/// @doc mcp.tools.get_dsl_reference
/// @description Return the Standardoc DSL syntax reference (accessors, block directives, functions). Use this to produce valid templates without guessing.
/// @category lifecycle
fn get_dsl_reference() -> Value {
    let reference = include_str!("dsl_reference.md");
    text_content(reference)
}

/// @doc mcp.tools.find_undocumented
/// @description List blocks that were auto-inferred from AST but have no `@doc` annotation — candidates for documentation.
/// @category quality
fn find_undocumented(state: &ServerState) -> Value {
    let idx = state.index();
    let entries: Vec<Value> = idx
        .blocks
        .values()
        .filter(|b| b.origin == BlockOrigin::Inferred)
        .map(|b| {
            json!({
                "key": b.key.as_str(),
                "label": b.label,
                "kind": b.symbol.as_ref().map(|s| format!("{:?}", s.kind).to_lowercase()),
                "path": b.meta.path,
                "line": b.meta.line_start,
            })
        })
        .collect();
    let payload = json!({
        "count": entries.len(),
        "entries": entries,
    });
    text_content(payload.to_string())
}

/// @doc mcp.tools.rescan
/// @description Re-scan the workspace from disk. Bumps the index revision.
/// @category lifecycle
fn rescan(state: &ServerState) -> Value {
    match state.rescan() {
        Ok(count) => {
            let error_count = state.index().error_count;
            text_content(
                json!({
                    "blocks_count": count,
                    "revision": state.revision(),
                    "errors": error_count,
                })
                .to_string(),
            )
        }
        Err(err) => tool_error_content(format!("rescan failed: {err}")),
    }
}

/// @doc mcp.tools.validate_doc_syntax
/// @description Parse a string as a `@doc` annotation body and report whether it is syntactically well-formed. Used by agents before they write annotations into source.
/// @category quality
fn validate_doc_syntax(state: &ServerState, args: &Value) -> Value {
    let Some(content) = args.get("content").and_then(Value::as_str) else {
        return tool_error_content("missing 'content' argument");
    };
    // We delegate to the annotation parser. It never fails structurally — it
    // simply emits the tags it can recognise. To detect "invalid syntax" we
    // report the parsed tag names and the count, plus any DSL-like expression
    // inside the content that fails to parse.
    let annotations = annotation::parse(content, &state.config().doc_tag);
    let tags: Vec<String> = annotations.tags.keys().cloned().collect();
    let dsl_check = parse(content).err().map(|e| e.to_string());

    text_content(
        json!({
            "valid": dsl_check.is_none(),
            "tags_found": tags,
            "dsl_parse_error": dsl_check,
        })
        .to_string(),
    )
}

/// @doc mcp.tools.coverage_report
/// @description Summarize documentation coverage: total blocks, how many are inferred/annotated/hybrid, distribution by kind.
/// @category quality
fn coverage_report(state: &ServerState) -> Value {
    use standardoc_core::model::VirtualConfidence;
    let mut total: usize = 0;
    let mut inferred: usize = 0;
    let mut annotated: usize = 0;
    let mut hybrid: usize = 0;
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    let mut virtual_total: usize = 0;
    let mut virtual_high: usize = 0;
    let mut virtual_medium: usize = 0;
    let mut virtual_low: usize = 0;

    let idx = state.index();
    for b in idx.blocks.values() {
        total += 1;
        match b.origin {
            BlockOrigin::Inferred => inferred += 1,
            BlockOrigin::Annotated => annotated += 1,
            BlockOrigin::Hybrid => hybrid += 1,
        }
        if let Some(sym) = &b.symbol {
            let k = format!("{:?}", sym.kind).to_lowercase();
            *by_kind.entry(k).or_insert(0) += 1;
        }
        match b.virtual_confidence {
            Some(VirtualConfidence::High) => {
                virtual_total += 1;
                virtual_high += 1;
            }
            Some(VirtualConfidence::Medium) => {
                virtual_total += 1;
                virtual_medium += 1;
            }
            Some(VirtualConfidence::Low) => {
                virtual_total += 1;
                virtual_low += 1;
            }
            None => {}
        }
    }

    text_content(
        json!({
            "total_blocks": total,
            "by_origin": {
                "inferred": inferred,
                "annotated": annotated,
                "hybrid": hybrid,
            },
            "by_kind": by_kind,
            "virtual_count": virtual_total,
            "by_virtual_confidence": {
                "high": virtual_high,
                "medium": virtual_medium,
                "low": virtual_low,
            },
            "revision": state.revision(),
        })
        .to_string(),
    )
}

/// @doc mcp.tools.list_collisions
/// @description List key collisions detected during the last scan. Each entry shows the key, the winning location, and every location that was silently dropped. Surfaces what would otherwise be invisible data loss.
/// @category quality
fn list_collisions(state: &ServerState) -> Value {
    let idx = state.index();
    let entries: Vec<Value> = idx
        .collisions
        .iter()
        .map(|c| {
            json!({
                "key": c.key,
                "kept": {
                    "path": c.kept.path,
                    "line": c.kept.line,
                },
                "dropped": c.dropped.iter().map(|d| json!({
                    "path": d.path,
                    "line": d.line,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    text_content(
        json!({
            "count": entries.len(),
            "entries": entries,
        })
        .to_string(),
    )
}

/// @doc mcp.tools.set_watch_paused
/// @description Pause or resume the filesystem watcher. While paused, FS changes are drained without triggering re-scans — useful during heavy refactors, initial scaffolding, or when you want a frozen snapshot of the index. Call again with false to resume. The auto-pause heuristic also flips this flag when it detects repeated parse errors.
/// @category lifecycle
fn set_watch_paused(state: &ServerState, args: &Value) -> Value {
    let Some(paused) = args.get("paused").and_then(Value::as_bool) else {
        return tool_error_content("missing 'paused' argument (boolean)");
    };
    state.set_watch_paused(paused);
    text_content(
        json!({
            "paused": paused,
            "has_watcher": state.has_watcher(),
        })
        .to_string(),
    )
}

/// @doc mcp.tools.get_watch_status
/// @description Report the current watcher state: whether a watcher is active, whether it's paused, and the current index revision. Call this after set_watch_paused or when you want to check freshness.
/// @category lifecycle
fn get_watch_status(state: &ServerState) -> Value {
    text_content(
        json!({
            "has_watcher": state.has_watcher(),
            "paused": state.watch_paused(),
            "revision": state.revision(),
        })
        .to_string(),
    )
}

/// @doc mcp.tools.find_usages
/// @description List every block that references the given short name through its outgoing references — param types, return types, field types, trait implementations. Match is on **short name** (last FQN segment), so an ambiguous name returns all candidates; use the disambiguation filters below to scope the results.
///
/// - `kind` restricts the relation ('param-type', 'return-type', 'field-type', 'implements', 'extends', 'generic-arg', 'call', 'other').
/// - `from_path_prefix` restricts to the subtree of the referrer file (e.g. 'crates/standardoc-core/' to see only usages in that crate).
/// - `from_key_prefix` restricts to the subtree of the referrer's FQN module (e.g. 'matchigo.parser' to see only usages in that module). More precise than `from_path_prefix` when file paths don't track the FQN hierarchy.
/// @category cross-ref
fn find_usages(state: &ServerState, args: &Value) -> Value {
    let Some(name) = args.get("name").and_then(Value::as_str) else {
        return tool_error_content("missing 'name' argument");
    };
    let kind_filter = args
        .get("kind")
        .and_then(Value::as_str)
        .and_then(parse_kind);
    let from_path_prefix = args.get("from_path_prefix").and_then(Value::as_str);
    let from_key_prefix = args.get("from_key_prefix").and_then(Value::as_str);

    let idx = state.index();
    let Some(refs) = idx.incoming.get(name) else {
        return text_content(json!({ "name": name, "count": 0, "entries": [] }).to_string());
    };

    let entries: Vec<Value> = refs
        .iter()
        .filter(|r| kind_filter.is_none_or(|k| r.kind == k))
        .filter(|r| from_key_prefix.is_none_or(|p| key_matches_prefix(&r.from_key, p)))
        .filter_map(|r| {
            let block = idx.blocks.get(&r.from_key);
            // `from_path_prefix` is applied via the `block.meta.path` —
            // we skip orphaned refs (block not found) when a path filter
            // is requested, otherwise we can't tell whether to keep them.
            if let Some(prefix) = from_path_prefix {
                let path_str = block?.meta.path.to_string_lossy();
                if !path_str.starts_with(prefix) {
                    return None;
                }
            }
            Some(json!({
                "from_key": r.from_key,
                "kind": kind_str(r.kind),
                "line": r.line,
                "from_label": block.map(|b| b.label.clone()),
                "from_path": block.map(|b| b.meta.path.clone()),
                "from_origin": block.map(|b| origin_str(b.origin)),
            }))
        })
        .collect();

    text_content(
        json!({
            "name": name,
            "count": entries.len(),
            "entries": entries,
        })
        .to_string(),
    )
}

/// Strict descendant match: `foo.bar` matches `foo.bar` (equality) and
/// `foo.bar.x` (descendant) — but **not** `foo.barber`. Uses `.` as the
/// separator to avoid false positives on raw string prefix.
fn key_matches_prefix(key: &str, prefix: &str) -> bool {
    if key == prefix {
        return true;
    }
    let with_sep = format!("{prefix}.");
    key.starts_with(&with_sep)
}

/// @doc mcp.tools.find_implementations
/// @description List every type that implements the given trait/interface (Rust `impl Trait for X`, TS `class X implements I`). Returns the implementor types deduplicated, with the keys of their associated trait methods.
/// @category cross-ref
fn find_implementations(state: &ServerState, args: &Value) -> Value {
    let Some(trait_name) = args.get("trait_name").and_then(Value::as_str) else {
        return tool_error_content("missing 'trait_name' argument");
    };

    let idx = state.index();
    let Some(refs) = idx.incoming.get(trait_name) else {
        return text_content(
            json!({ "trait_name": trait_name, "count": 0, "implementors": [] }).to_string(),
        );
    };

    // Filter on RefKind::Implements and group by implementor type
    // (inferred from parent FQN segment of the tagged method).
    let mut by_implementor: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for r in refs.iter().filter(|r| r.kind == RefKind::Implements) {
        let implementor = parent_fqn_segment(&r.from_key);
        by_implementor
            .entry(implementor)
            .or_default()
            .push(r.from_key.clone());
    }

    let implementors: Vec<Value> = by_implementor
        .into_iter()
        .map(|(implementor, methods)| {
            json!({
                "implementor": implementor,
                "method_keys": methods,
            })
        })
        .collect();

    text_content(
        json!({
            "trait_name": trait_name,
            "count": implementors.len(),
            "implementors": implementors,
        })
        .to_string(),
    )
}

fn search_by_kind(state: &ServerState, args: &Value, kind: RefKind) -> Value {
    let Some(name) = args.get("name").and_then(Value::as_str) else {
        return tool_error_content("missing 'name' argument");
    };
    let from_path_prefix = args.get("from_path_prefix").and_then(Value::as_str);
    let from_key_prefix = args.get("from_key_prefix").and_then(Value::as_str);
    let idx = state.index();
    let Some(refs) = idx.incoming.get(name) else {
        return text_content(
            json!({ "name": name, "kind": kind_str(kind), "count": 0, "entries": [] }).to_string(),
        );
    };

    let entries: Vec<Value> = refs
        .iter()
        .filter(|r| r.kind == kind)
        .filter(|r| from_key_prefix.is_none_or(|p| key_matches_prefix(&r.from_key, p)))
        .filter_map(|r| {
            let block = idx.blocks.get(&r.from_key);
            if let Some(prefix) = from_path_prefix {
                let path_str = block?.meta.path.to_string_lossy();
                if !path_str.starts_with(prefix) {
                    return None;
                }
            }
            Some(json!({
                "from_key": r.from_key,
                "from_label": block.map(|b| b.label.clone()),
                "signature": block.and_then(|b| b.symbol.as_ref().map(|s| s.signature.clone())),
                "from_path": block.map(|b| b.meta.path.clone()),
                "line": r.line,
            }))
        })
        .collect();

    text_content(
        json!({
            "name": name,
            "kind": kind_str(kind),
            "count": entries.len(),
            "entries": entries,
        })
        .to_string(),
    )
}

/// @doc mcp.tools.get_type_hierarchy
/// @description Walk the inheritance graph of a type. Returns the ancestors (transitively `extends`-ed types) and descendants (types that `extends` the given type), plus the interfaces it implements (`implements_outgoing`) and the types that implement it (`implementors_incoming`). Match is on **short name**.
/// @category cross-ref
fn get_type_hierarchy(state: &ServerState, args: &Value) -> Value {
    let Some(name) = args.get("name").and_then(Value::as_str) else {
        return tool_error_content("missing 'name' argument");
    };

    let idx = state.index();

    // 1) Find blocks whose label matches — there can be multiple for an
    //    ambiguous name (two `User` types in different modules). We process
    //    them all; the agent can filter by path.
    let target_blocks: Vec<&DocBlock> = idx.blocks.values().filter(|b| b.label == name).collect();

    if target_blocks.is_empty() {
        return text_content(json!({ "name": name, "found": false, "candidates": [] }).to_string());
    }

    // 2) For each match, compute ancestors (Extends out), descendants
    //    (Extends in), implements out, implementors in.
    let candidates: Vec<Value> = target_blocks
        .iter()
        .map(|block| {
            let key = block.key.as_str();

            // Ancestors: follow Extends.outgoing transitively (BFS).
            let ancestors = walk_ancestors(&idx.blocks, key);

            // Descendants: from Extends.incoming, BFS over transitive
            // referrers.
            let descendants = walk_descendants(&idx.blocks, &idx.incoming, name);

            // Outgoing implements: interfaces this block implements.
            let implements_outgoing: Vec<String> = block
                .symbol
                .as_ref()
                .map(|s| {
                    s.references
                        .outgoing
                        .iter()
                        .filter(|r| r.kind == RefKind::Implements)
                        .map(|r| r.target.clone())
                        .collect()
                })
                .unwrap_or_default();

            // Incoming implementors: types with `Implements -> name`,
            // deduplicated per implementor (consistent with
            // `find_implementations`). Edges point from impl trait methods;
            // we walk back to the parent type via `parent_fqn_segment`.
            let mut implementors_set: std::collections::BTreeSet<String> =
                std::collections::BTreeSet::new();
            if let Some(refs) = idx.incoming.get(name) {
                for r in refs.iter().filter(|r| r.kind == RefKind::Implements) {
                    implementors_set.insert(parent_fqn_segment(&r.from_key));
                }
            }
            let implementors_incoming: Vec<String> = implementors_set.into_iter().collect();

            json!({
                "key": key,
                "label": &block.label,
                "kind": block.symbol.as_ref().map(|s| format!("{:?}", s.kind).to_lowercase()),
                "path": &block.meta.path,
                "ancestors": ancestors,
                "descendants": descendants,
                "implements_outgoing": implements_outgoing,
                "implementors_incoming": implementors_incoming,
            })
        })
        .collect();

    text_content(
        json!({
            "name": name,
            "found": true,
            "count": candidates.len(),
            "candidates": candidates,
        })
        .to_string(),
    )
}

/// Ancestor BFS: for block `key`, follow `Extends.outgoing` up the chain.
/// Returns ancestor **labels**.
fn walk_ancestors(blocks: &BTreeMap<String, DocBlock>, start_key: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut queue: Vec<String> = vec![start_key.to_owned()];

    while let Some(key) = queue.pop() {
        if !visited.insert(key.clone()) {
            continue;
        }
        let Some(block) = blocks.get(&key) else {
            continue;
        };
        let Some(symbol) = &block.symbol else {
            continue;
        };
        for sref in &symbol.references.outgoing {
            if sref.kind == RefKind::Extends {
                out.push(sref.target.clone());
                // Walk up the chain: find the block whose label is `target`.
                if let Some((next_key, _)) = blocks.iter().find(|(_, b)| b.label == sref.target) {
                    queue.push(next_key.clone());
                }
            }
        }
    }
    out
}

/// Descendant BFS: for label `name`, find all blocks with
/// `Extends -> name` (incoming), then recurse on their labels.
fn walk_descendants(
    blocks: &BTreeMap<String, DocBlock>,
    incoming: &BTreeMap<String, Vec<standardoc_core::model::IncomingRef>>,
    name: &str,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut queue: Vec<String> = vec![name.to_owned()];

    while let Some(label) = queue.pop() {
        if !visited.insert(label.clone()) {
            continue;
        }
        let Some(refs) = incoming.get(&label) else {
            continue;
        };
        for r in refs {
            if r.kind == RefKind::Extends {
                if let Some(child_block) = blocks.get(&r.from_key) {
                    out.push(child_block.label.clone());
                    queue.push(child_block.label.clone());
                }
            }
        }
    }
    out
}

fn parse_kind(s: &str) -> Option<RefKind> {
    match s {
        "call" => Some(RefKind::Call),
        "param-type" => Some(RefKind::ParamType),
        "return-type" => Some(RefKind::ReturnType),
        "field-type" => Some(RefKind::FieldType),
        "implements" => Some(RefKind::Implements),
        "extends" => Some(RefKind::Extends),
        "generic-arg" => Some(RefKind::GenericArg),
        "other" => Some(RefKind::Other),
        _ => None,
    }
}

const fn kind_str(k: RefKind) -> &'static str {
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

/// For a key like `standardoc_lang_rust.<RustProvider as LanguageProvider>.id`,
/// return the segment designating implementor type — in this example,
/// `<RustProvider as LanguageProvider>` (or simply `RustProvider` after
/// simplification). For `Calculator.add`, returns `Calculator`.
fn parent_fqn_segment(key: &str) -> String {
    // Remove the last segment (the method).
    let without_method = key.rsplit_once('.').map_or(key, |(parent, _)| parent);
    // If parent contains `<X as Trait>`, extract only `X`.
    if let Some(start) = without_method.rfind('<') {
        if let Some(end) = without_method[start..].find(" as ") {
            return without_method[start + 1..start + end].to_owned();
        }
    }
    // Sinon, dernier segment du parent.
    without_method
        .rsplit_once('.')
        .map_or(without_method, |(_, last)| last)
        .to_owned()
}

const fn origin_str(origin: BlockOrigin) -> &'static str {
    match origin {
        BlockOrigin::Inferred => "inferred",
        BlockOrigin::Annotated => "annotated",
        BlockOrigin::Hybrid => "hybrid",
    }
}

fn description_text(block: &DocBlock) -> Option<String> {
    block
        .tags
        .get("description")
        .and_then(|occurrences| occurrences.first())
        .and_then(|fields| fields.first())
        .cloned()
}

/// Build a flat lowercase string from all searchable text in a block.
/// Used by the fuzzy fallback in `search_docs`.
fn block_search_text(block: &DocBlock) -> String {
    let mut parts: Vec<String> = vec![
        block.key.as_str().to_ascii_lowercase(),
        block.label.to_ascii_lowercase(),
    ];
    if let Some(sym) = &block.symbol {
        parts.push(sym.signature.to_ascii_lowercase());
        for p in &sym.params {
            parts.push(p.name.to_ascii_lowercase());
            if let Some(ty) = &p.type_repr {
                parts.push(ty.to_ascii_lowercase());
            }
        }
        if let Some(ret) = &sym.returns {
            parts.push(ret.repr.to_ascii_lowercase());
        }
        for g in &sym.generics {
            parts.push(g.to_ascii_lowercase());
        }
    }
    for occurrences in block.tags.values() {
        for fields in occurrences {
            for field in fields {
                parts.push(field.to_ascii_lowercase());
            }
        }
    }
    parts.join(" ")
}

/// Token-based scoring across all block fields.
/// Each query token that appears in the block's full text contributes:
///   - 3 points if found in key or label
///   - 2 points if found in signature or params
///   - 1 point if found only in the full text (tags, description, etc.)
fn fuzzy_score_blocks<'a>(
    blocks: impl Iterator<Item = &'a DocBlock>,
    q_lc: &str,
) -> Vec<(i32, &'a DocBlock)> {
    let tokens: Vec<&str> = q_lc.split_whitespace().collect();
    if tokens.is_empty() {
        return Vec::new();
    }
    blocks
        .filter_map(|b| {
            let key_lc = b.key.as_str().to_ascii_lowercase();
            let label_lc = b.label.to_ascii_lowercase();
            let sig_lc = b
                .symbol
                .as_ref()
                .map(|s| s.signature.to_ascii_lowercase())
                .unwrap_or_default();
            let full_text = block_search_text(b);
            let mut score = 0i32;
            for token in &tokens {
                if key_lc.contains(token) || label_lc.contains(token) {
                    score += 3;
                } else if sig_lc.contains(token) {
                    score += 2;
                } else if full_text.contains(token) {
                    score += 1;
                }
            }
            if score > 0 {
                Some((score, b))
            } else {
                None
            }
        })
        .collect()
}

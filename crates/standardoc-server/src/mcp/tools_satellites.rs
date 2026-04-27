//! Free-floating doc annotations for MCP tools.
//!
//! Two kinds of blocks live here:
//!
//! 1. **Anchor annotations** (`@doc mcp.tools.X` + `@description` + `@category`)
//!    for tools backed by a *shared* handler (`emit_format` dispatches to three
//!    tools, `search_by_kind` to two). A `///` doc-comment on those handler
//!    fns can only carry one `@doc` key, so the tool-level anchors live here
//!    instead. Tools backed by a 1:1 handler keep their anchor on the fn.
//!
//! 2. **Argument-schema satellites** (`@doc-extend mcp.tools.X args` +
//!    `@schema {...}` + `@category`) for every tool that accepts arguments.
//!    The `@category` mirrors the anchor's category so a single each-loop
//!    in the docs template can render anchor + its args together under the
//!    correct section. Tools with no arguments don't need a satellite; the
//!    runtime falls back to `{"type":"object","properties":{}}`.
//!
//! The runtime (`tools_index.rs::tools()`) parses this file plus `tools.rs`
//! at server startup using standardoc-core's own comment scanner — same
//! code path as documenting any third-party crate. There is zero parallel
//! runtime structure: the annotations ARE the declaration.
//!
//! This file deliberately contains no Rust items — it carries doc comments
//! only.

#![allow(dead_code)]

// ----------------------------------------------------------------------------
// Anchors for shared-handler tools
// ----------------------------------------------------------------------------

// @doc mcp.tools.emit_llms_txt
// @description Generate the `llms.txt` file content for the current index — Jeremy Howard's link-based Markdown index for LLMs. Optional `name` (project), `tagline`, and `link_base` (URL prefix for hyperlinks).
// @category emit

// @doc mcp.tools.emit_llms_full
// @description Generate the `llms-full.txt` file content — same coverage as `llms.txt` but inlined: signatures + descriptions + params + returns in one file. For agents that ingest in bulk rather than follow links.
// @category emit

// @doc mcp.tools.emit_skill_md
// @description Generate a Claude Code-style `skill.md` with YAML front-matter — describes the project's key types, traits (with implementors), and public functions as an acquireable skill for agents.
// @category emit

// @doc mcp.tools.search_by_return_type
// @description List functions/methods whose return type contains the given short name. Use this to answer 'where does X come from?' or 'what produces a Foo?'. Match is on **short name** (last FQN segment). Optional `from_path_prefix` / `from_key_prefix` scope the results — same semantics as `find_usages`.
// @category cross-ref

// @doc mcp.tools.search_by_param_type
// @description List functions/methods whose parameter list contains the given short name. Use this to answer 'who accepts a Foo?'. Match is on **short name** (last FQN segment). Optional `from_path_prefix` / `from_key_prefix` scope the results — same semantics as `find_usages`.
// @category cross-ref

// ----------------------------------------------------------------------------
// Argument schemas — one satellite per tool that takes args
// ----------------------------------------------------------------------------

// @doc-extend mcp.tools.list_docs args
// @schema {"type":"object","properties":{"filter":{"type":"string","description":"Substring matched against key or label."},"limit":{"type":"integer","minimum":1},"offset":{"type":"integer","minimum":0}}}
// @category navigation

// @doc-extend mcp.tools.get_doc args
// @schema {"type":"object","properties":{"key":{"type":"string"}},"required":["key"]}
// @category navigation

// @doc-extend mcp.tools.search_docs args
// @schema {"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer","minimum":1}},"required":["query"]}
// @category navigation

// @doc-extend mcp.tools.evaluate_dsl args
// @schema {"type":"object","properties":{"expression":{"type":"string"}},"required":["expression"]}
// @category navigation

// @doc-extend mcp.tools.render_markdown args
// @schema {"type":"object","properties":{"content":{"type":"string"}},"required":["content"]}
// @category navigation

// @doc-extend mcp.tools.validate_doc_syntax args
// @schema {"type":"object","properties":{"content":{"type":"string"}},"required":["content"]}
// @category quality

// @doc-extend mcp.tools.set_watch_paused args
// @schema {"type":"object","properties":{"paused":{"type":"boolean"}},"required":["paused"]}
// @category lifecycle

// @doc-extend mcp.tools.find_usages args
// @schema {"type":"object","properties":{"name":{"type":"string","description":"Short symbol name to look up (e.g. 'ParseError')."},"kind":{"type":"string","description":"Optional ref kind to filter on."},"from_path_prefix":{"type":"string","description":"Optional path prefix on the referrer file (relative to workspace root)."},"from_key_prefix":{"type":"string","description":"Optional FQN prefix on the referrer key — matches the key itself or a strict descendant ('foo.bar' matches 'foo.bar' and 'foo.bar.baz' but not 'foo.barber')."}},"required":["name"]}
// @category cross-ref

// @doc-extend mcp.tools.find_implementations args
// @schema {"type":"object","properties":{"trait_name":{"type":"string"}},"required":["trait_name"]}
// @category cross-ref

// @doc-extend mcp.tools.search_by_return_type args
// @schema {"type":"object","properties":{"name":{"type":"string"},"from_path_prefix":{"type":"string"},"from_key_prefix":{"type":"string"}},"required":["name"]}
// @category cross-ref

// @doc-extend mcp.tools.search_by_param_type args
// @schema {"type":"object","properties":{"name":{"type":"string"},"from_path_prefix":{"type":"string"},"from_key_prefix":{"type":"string"}},"required":["name"]}
// @category cross-ref

// @doc-extend mcp.tools.get_type_hierarchy args
// @schema {"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}
// @category cross-ref

// @doc-extend mcp.tools.emit_llms_txt args
// @schema {"type":"object","properties":{"name":{"type":"string"},"tagline":{"type":"string"},"link_base":{"type":"string"}}}
// @category emit

// @doc-extend mcp.tools.emit_llms_full args
// @schema {"type":"object","properties":{"name":{"type":"string"},"tagline":{"type":"string"}}}
// @category emit

// @doc-extend mcp.tools.emit_skill_md args
// @schema {"type":"object","properties":{"name":{"type":"string"},"tagline":{"type":"string"}}}
// @category emit

// @doc-extend mcp.tools.emit_openapi args
// @schema {"type":"object","properties":{"title":{"type":"string","description":"API title for `info.title`. Default 'API'."},"version":{"type":"string","description":"API version for `info.version`. Default '0.1.0'."},"description":{"type":"string","description":"Optional `info.description`."},"servers":{"type":"array","items":{"type":"string"},"description":"Optional list of server URLs (added as `servers[]`)."}}}
// @category emit

// @doc-extend mcp.tools.resolve_symbol args
// @schema {"type":"object","properties":{"name":{"type":"string","description":"Short symbol name or label to resolve."},"path_prefix":{"type":"string","description":"Optional path prefix to restrict results (relative to workspace root)."}},"required":["name"]}
// @category lsp

// @doc-extend mcp.tools.get_definition args
// @schema {"type":"object","properties":{"key":{"type":"string"}},"required":["key"]}
// @category lsp

// @doc-extend mcp.tools.get_hover args
// @schema {"type":"object","properties":{"key":{"type":"string"}},"required":["key"]}
// @category lsp

// @doc-extend mcp.tools.find_references args
// @schema {"type":"object","properties":{"key":{"type":"string","description":"Canonical doc key to search for (also matches short-name occurrences)."}},"required":["key"]}
// @category lsp

// @doc-extend mcp.tools.get_comments args
// @schema {"type":"object","properties":{"file":{"type":"string","description":"Path to the source file. Absolute or relative to the workspace root."}},"required":["file"]}
// @category navigation

// @doc-extend mcp.tools.list_diagnostics args
// @schema {"type":"object","properties":{"severity":{"type":"string","description":"Optional severity filter."},"code":{"type":"string","description":"Optional diagnostic code filter (e.g. 'STD006')."}}}
// @category quality

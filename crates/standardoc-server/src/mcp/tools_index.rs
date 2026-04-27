//! Build the MCP `tools/list` payload from the doc annotations baked into
//! `tools.rs` and `tools_satellites.rs`.
//!
//! The runtime parses its own embedded source through standardoc-core's
//! comment scanner + annotation parser — same code path that documents any
//! third-party crate. There is no parallel runtime structure: the
//! annotations ARE the declaration. To add a new tool, annotate its handler
//! in `tools.rs` and (if it takes arguments) drop a satellite into
//! `tools_satellites.rs`.
//!
//! Cost: one parse pass on the first call, cached for the process lifetime
//! via `OnceLock`. Tools embed at compile time via `include_str!`.

use serde_json::{json, Value};
use standardoc_core::extractor::annotation;
use standardoc_core::extractor::comment_scan;
use standardoc_core::model::{CommentDelimiters, CommentStyles};
use std::collections::BTreeMap;
use std::sync::OnceLock;

const TOOLS_RS: &str = include_str!("tools.rs");
const TOOLS_SATELLITES_RS: &str = include_str!("tools_satellites.rs");
const TOOL_PREFIX: &str = "mcp.tools.";

static TOOLS: OnceLock<Vec<Value>> = OnceLock::new();

/// Tools derived from doc annotations. The slice is built on first call
/// and cached for the lifetime of the process.
pub(crate) fn tools() -> &'static [Value] {
    TOOLS.get_or_init(build)
}

fn build() -> Vec<Value> {
    let styles = rust_comment_styles();
    let mut anchors: BTreeMap<String, String> = BTreeMap::new();
    let mut satellites: BTreeMap<(String, String), BTreeMap<String, String>> = BTreeMap::new();

    for source in [TOOLS_RS, TOOLS_SATELLITES_RS] {
        for span in comment_scan::scan(source, &styles) {
            let anno = annotation::parse(&span.body, "doc");

            if let Some(doc_fields) = anno.first("doc") {
                if let Some(key) = doc_fields.first().filter(|s| s.starts_with(TOOL_PREFIX)) {
                    let description = anno
                        .first("description")
                        .and_then(|fields| fields.first())
                        .cloned()
                        .unwrap_or_default();
                    anchors.insert(key.clone(), description);
                }
            }

            if let Some(extend_fields) = anno.first("doc-extend") {
                let anchor = extend_fields
                    .first()
                    .filter(|s| s.starts_with(TOOL_PREFIX))
                    .cloned();
                let extended = extend_fields.get(1).cloned();
                if let (Some(anchor), Some(extended)) = (anchor, extended) {
                    let entry = satellites.entry((anchor, extended)).or_default();
                    for (tag, occurrences) in &anno.tags {
                        if tag == "doc-extend" {
                            continue;
                        }
                        if let Some(value) =
                            occurrences.first().and_then(|fields| fields.first())
                        {
                            entry.insert(tag.clone(), value.clone());
                        }
                    }
                }
            }
        }
    }

    anchors
        .into_iter()
        .map(|(key, description)| {
            let name = key
                .strip_prefix(TOOL_PREFIX)
                .expect("anchor key is filtered to the prefix at insertion");
            let args_schema = satellites
                .get(&(key.clone(), "args".to_owned()))
                .and_then(|tags| tags.get("schema"))
                .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                .unwrap_or_else(empty_schema);
            json!({
                "name": name,
                "description": description,
                "inputSchema": args_schema,
            })
        })
        .collect()
}

fn empty_schema() -> Value {
    json!({ "type": "object", "properties": {} })
}

fn rust_comment_styles() -> CommentStyles {
    CommentStyles {
        single: vec!["//".to_owned()],
        multi: Some(CommentDelimiters {
            start: "/*".to_owned(),
            end: "*/".to_owned(),
        }),
        doc_single: vec!["///".to_owned(), "//!".to_owned()],
        doc_multi: Some(CommentDelimiters {
            start: "/**".to_owned(),
            end: "*/".to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const EXPECTED_TOOL_NAMES: &[&str] = &[
        "coverage_report",
        "emit_llms_full",
        "emit_llms_txt",
        "emit_openapi",
        "emit_skill_md",
        "evaluate_dsl",
        "find_implementations",
        "find_references",
        "find_undocumented",
        "find_usages",
        "get_comments",
        "get_definition",
        "get_doc",
        "get_dsl_reference",
        "get_hover",
        "get_type_hierarchy",
        "get_watch_status",
        "list_collisions",
        "list_diagnostics",
        "list_docs",
        "render_markdown",
        "rescan",
        "resolve_symbol",
        "search_by_param_type",
        "search_by_return_type",
        "search_docs",
        "set_watch_paused",
        "validate_doc_syntax",
    ];

    #[test]
    fn derived_tool_set_covers_all_expected_names() {
        let mut names: Vec<&str> = tools()
            .iter()
            .filter_map(|t| t.get("name").and_then(Value::as_str))
            .collect();
        names.sort_unstable();
        let mut expected: Vec<&str> = EXPECTED_TOOL_NAMES.to_vec();
        expected.sort_unstable();
        assert_eq!(names, expected, "MCP tools/list surface drifted");
    }

    #[test]
    fn every_derived_tool_has_non_empty_description() {
        for tool in tools() {
            let name = tool.get("name").and_then(Value::as_str).unwrap_or("?");
            let desc = tool
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                !desc.is_empty(),
                "tool '{name}' has empty description — annotation missing or malformed"
            );
        }
    }

    #[test]
    fn every_derived_tool_has_object_input_schema() {
        for tool in tools() {
            let name = tool.get("name").and_then(Value::as_str).unwrap_or("?");
            let schema = tool
                .get("inputSchema")
                .expect("missing inputSchema");
            assert_eq!(
                schema.get("type").and_then(Value::as_str),
                Some("object"),
                "tool '{name}' inputSchema is not type:object"
            );
        }
    }
}

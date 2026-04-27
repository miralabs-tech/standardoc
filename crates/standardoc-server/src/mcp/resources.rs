//! MCP resources — passive, read-only exposure of workspace state.
//!
//! Each resource is declared as an annotated `ResourceDef` const so the
//! catalog stays in sync with the doc-renderer (no parallel table).

#![allow(clippy::significant_drop_tightening)]

use super::protocol::JsonRpcError;
use crate::state::ServerState;
use serde_json::{json, Value};

/// Metadata for one MCP resource. URI is the stable identifier; the rest
/// drives the protocol-level `resources/list` payload and (via `@doc`) the
/// rendered docs table.
pub(crate) struct ResourceDef {
    pub uri: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub mime_type: &'static str,
}

/// @doc mcp.resources.index
/// @uri standardoc://index
/// @mime application/json
/// @description Every DocBlock discovered in the workspace, as a JSON array.
pub(crate) const INDEX: ResourceDef = ResourceDef {
    uri: "standardoc://index",
    name: "Canonical index",
    description: "Every DocBlock discovered in the workspace, as a JSON array.",
    mime_type: "application/json",
};

/// @doc mcp.resources.config
/// @uri standardoc://config
/// @mime application/json
/// @description Resolved `.standardoc.json` (or defaults) currently in use.
pub(crate) const CONFIG: ResourceDef = ResourceDef {
    uri: "standardoc://config",
    name: "Active configuration",
    description: "The resolved .standardoc.json (or defaults) in use.",
    mime_type: "application/json",
};

/// @doc mcp.resources.schema_dsl
/// @uri standardoc://schema/dsl
/// @mime text/markdown
/// @description Standardoc DSL grammar reference — feed this to an agent so it writes valid templates.
pub(crate) const SCHEMA_DSL: ResourceDef = ResourceDef {
    uri: "standardoc://schema/dsl",
    name: "DSL reference",
    description: "Standardoc DSL grammar — feed this to an agent so it writes valid templates.",
    mime_type: "text/markdown",
};

/// @doc mcp.resources.schema_tags
/// @uri standardoc://schema/tags
/// @mime application/json
/// @description Built-in plus user tag schemas (name → [fields], [required fields]).
pub(crate) const SCHEMA_TAGS: ResourceDef = ResourceDef {
    uri: "standardoc://schema/tags",
    name: "Tag schemas",
    description: "Built-in plus user tag schemas (name → [fields], [required fields]).",
    mime_type: "application/json",
};

const ALL: &[&ResourceDef] = &[&INDEX, &CONFIG, &SCHEMA_DSL, &SCHEMA_TAGS];

pub(crate) fn list() -> Vec<Value> {
    ALL.iter()
        .map(|r| {
            json!({
                "uri": r.uri,
                "name": r.name,
                "description": r.description,
                "mimeType": r.mime_type,
            })
        })
        .collect()
}

pub(crate) fn read(state: &ServerState, params: &Value) -> Result<Value, JsonRpcError> {
    let uri = params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| JsonRpcError::invalid_params("missing 'uri'"))?;

    let (text, mime) = if uri == INDEX.uri {
        let idx = state.index();
        let blocks: Vec<_> = idx.blocks.values().collect();
        (
            serde_json::to_string_pretty(&blocks)
                .map_err(|e| JsonRpcError::internal(format!("serialize: {e}")))?,
            INDEX.mime_type,
        )
    } else if uri == CONFIG.uri {
        (
            serde_json::to_string_pretty(state.config())
                .map_err(|e| JsonRpcError::internal(format!("serialize: {e}")))?,
            CONFIG.mime_type,
        )
    } else if uri == SCHEMA_DSL.uri {
        (
            include_str!("dsl_reference.md").to_owned(),
            SCHEMA_DSL.mime_type,
        )
    } else if uri == SCHEMA_TAGS.uri {
        (
            serde_json::to_string_pretty(state.schemas())
                .map_err(|e| JsonRpcError::internal(format!("serialize: {e}")))?,
            SCHEMA_TAGS.mime_type,
        )
    } else {
        return Err(JsonRpcError::invalid_params(format!(
            "unknown resource uri: {uri}"
        )));
    };

    Ok(json!({
        "contents": [{
            "uri": uri.to_owned(),
            "mimeType": mime,
            "text": text,
        }],
    }))
}

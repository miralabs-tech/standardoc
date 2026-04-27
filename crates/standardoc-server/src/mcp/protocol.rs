//! JSON-RPC 2.0 wire types and helpers.
//!
//! We keep this minimal and untyped on the request side — the dispatcher
//! decodes params per-method. Responses are always built via the two helpers
//! `success_response` / `error_response`.

use serde::Deserialize;
use serde_json::{json, Value};

/// A request or notification on its way in.
/// `id` is `None` for notifications and some malformed inputs.
#[derive(Debug, Deserialize)]
pub(crate) struct RawMessage {
    #[serde(default)]
    pub(crate) id: Option<Value>,
    #[serde(default)]
    pub(crate) method: Option<String>,
    #[serde(default)]
    pub(crate) params: Option<Value>,
}

#[derive(Debug, Clone)]
pub(crate) struct JsonRpcError {
    pub(crate) code: i32,
    pub(crate) message: String,
}

impl JsonRpcError {
    pub(crate) fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
        }
    }

    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: message.into(),
        }
    }
}

pub(crate) fn success_response(id: &Value, result: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

pub(crate) fn error_response(id: &Value, code: i32, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message.into(),
        },
    })
}

/// Wraps plain text as an MCP `content` array element — the shape MCP expects
/// for successful `tools/call` results.
pub(crate) fn text_content(text: impl Into<String>) -> Value {
    json!({
        "content": [{ "type": "text", "text": text.into() }],
    })
}

/// Same shape but with `isError: true` — surfaces tool failures to the agent
/// without turning the whole JSON-RPC call into an error.
pub(crate) fn tool_error_content(text: impl Into<String>) -> Value {
    json!({
        "content": [{ "type": "text", "text": text.into() }],
        "isError": true,
    })
}

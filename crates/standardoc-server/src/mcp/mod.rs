//! MCP server on stdio — JSON-RPC 2.0, line-delimited.
//!
//! No external MCP framework: we own the protocol wire format. The full MCP
//! spec is a JSON-RPC 2.0 variant with a fixed set of methods — simple enough
//! that re-implementing it here eliminates the bus-factor risk of depending
//! on an early-stage third-party crate.
//!
//! Supported surface (v1):
//! - `initialize` / `notifications/initialized`
//! - `tools/list` / `tools/call`
//! - `resources/list` / `resources/read`
//! - `ping`
//!
//! Everything else returns `-32601 Method not found`, which is the correct
//! MCP response and lets clients degrade gracefully.

mod protocol;
mod resources;
mod tools;
mod tools_index;
mod tools_satellites;

use crate::state::{ServerState, SharedStdout};
use protocol::{error_response, success_response, JsonRpcError, RawMessage};
use serde_json::{json, Value};
use std::io::{BufRead, Stdout, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

pub(crate) fn run(workspace: &Path) -> Result<(), String> {
    // Stdout shared between stdio thread (responses) and worker
    // (push notifications). A Mutex prevents message interleaving mid-write.
    // d'une ligne JSON-RPC.
    let shared_out: SharedStdout = Arc::new(Mutex::new(std::io::stdout()));

    let state =
        ServerState::boot(workspace, &shared_out).map_err(|err| format!("boot failed: {err}"))?;
    let stdin = std::io::stdin();

    let mut initialized = false;
    for line in stdin.lock().lines() {
        let line = line.map_err(|err| format!("stdin read error: {err}"))?;
        if line.trim().is_empty() {
            continue;
        }
        let msg: RawMessage = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(err) => {
                emit(
                    &shared_out,
                    &error_response(&Value::Null, -32700, format!("parse error: {err}")),
                );
                continue;
            }
        };

        if let Some(response) = dispatch(&state, &mut initialized, msg) {
            emit(&shared_out, &response);
        }
    }
    Ok(())
}

fn emit(out: &Arc<Mutex<Stdout>>, value: &Value) {
    if let Ok(line) = serde_json::to_string(value) {
        if let Ok(mut w) = out.lock() {
            let _ = writeln!(w, "{line}");
            let _ = w.flush();
        }
    }
}

fn dispatch(state: &ServerState, initialized: &mut bool, msg: RawMessage) -> Option<Value> {
    let method = msg.method.as_deref().unwrap_or("");
    let params = msg.params.unwrap_or(Value::Null);
    let id = msg.id.unwrap_or(Value::Null);
    // Notifications have no id and MUST NOT get a response.
    let is_notification = matches!(id, Value::Null);

    match method {
        "initialize" => Some(success_response(&id, &initialize_result())),
        "notifications/initialized" => {
            *initialized = true;
            None
        }
        "ping" => Some(success_response(&id, &json!({}))),

        "tools/list" => Some(success_response(&id, &json!({ "tools": tools_index::tools() }))),
        "tools/call" => {
            let response = tools::call(state, &params);
            if is_notification {
                None
            } else {
                Some(envelope(&id, response))
            }
        }

        "resources/list" => Some(success_response(
            &id,
            &json!({ "resources": resources::list() }),
        )),
        "resources/read" => {
            let response = resources::read(state, &params);
            if is_notification {
                None
            } else {
                Some(envelope(&id, response))
            }
        }

        other => {
            if is_notification {
                None
            } else {
                Some(error_response(
                    &id,
                    -32601,
                    format!("method not found: {other}"),
                ))
            }
        }
    }
}

fn envelope(id: &Value, result: Result<Value, JsonRpcError>) -> Value {
    match result {
        Ok(value) => success_response(id, &value),
        Err(err) => error_response(id, err.code, err.message),
    }
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": "2025-06-18",
        "serverInfo": {
            "name": "standardoc",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "capabilities": {
            "tools": { "listChanged": false },
            "resources": { "listChanged": false, "subscribe": false },
        }
    })
}

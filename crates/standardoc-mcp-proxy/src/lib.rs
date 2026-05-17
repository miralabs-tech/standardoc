//! Long-lived HTTP proxy between an MCP client (Claude Code, Copilot Chat,
//! Cursor, ...) and the standardoc daemon. The MCP client connects to a
//! stable proxy URL that never goes down; the proxy file-watches
//! `<workspace>/.standardoc/mcp.endpoint` to track the daemon's actual
//! HTTP address and forwards every request there. Daemon restarts /
//! rebuilds / migrations become invisible to the client.
//!
//! The proxy intentionally has zero coupling to the rest of the
//! standardoc workspace: no SQLite, no AST, no rmcp dependency. It's a
//! ~200-line axum + reqwest forwarder kept tiny so the binary is fast to
//! build, small on disk, and never needs to be rebuilt when the daemon's
//! internals evolve.

mod proxy;

pub use proxy::{ProxyConfig, ProxyError, run};

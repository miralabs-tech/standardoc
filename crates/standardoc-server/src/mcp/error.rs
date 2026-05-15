use rmcp::ErrorData;
use standardoc_core::SessionsError;

use crate::ServerError;

/// Thin newtype around `SessionsError` so MCP tool functions can use a single
/// `?` propagation path without having to import the underlying enum at the
/// call site. The wrapper preserves the chained `Display`.
#[derive(Debug)]
pub(crate) struct SessionsErr(pub SessionsError);

impl From<SessionsError> for SessionsErr {
    fn from(e: SessionsError) -> Self {
        Self(e)
    }
}

impl std::fmt::Display for SessionsErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

// Used as a `map_err(f)` callback (4 call sites), which requires
// `FnOnce(E) -> F` with `E` taken by value. Borrowing would force the
// callers to either re-introduce a closure or use `.map_err(|e| f(&e))`,
// both noisier than the lint suggests.
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn sessions_err_to_rmcp(err: SessionsErr) -> ErrorData {
    ErrorData::internal_error(err.to_string(), None)
}

/// Wire `ServerError` into the MCP layer. All variants collapse to an MCP
/// `internal_error` day-1: storage / cold-start / watcher / io failures are
/// infrastructure issues from the client's standpoint, and the chained
/// `Display` carries the detail. Per-variant codes can be introduced later
/// without breaking callers (additive — same shape as
/// `lsp::error::From<ServerError>`).
pub(crate) fn server_error_to_rmcp(err: &ServerError) -> ErrorData {
    ErrorData::internal_error(err.to_string(), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::ErrorCode;

    #[test]
    fn server_error_maps_to_internal_error_with_chained_display() {
        let server_err = ServerError::Io(std::io::Error::other("disk full"));
        let data = server_error_to_rmcp(&server_err);
        assert_eq!(data.code, ErrorCode::INTERNAL_ERROR);
        assert!(
            data.message.contains("io: disk full"),
            "message must surface the chained Display, got `{}`",
            data.message
        );
    }
}

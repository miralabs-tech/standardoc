use rmcp::ErrorData;

use crate::ServerError;

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

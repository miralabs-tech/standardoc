use tower_lsp_server::jsonrpc::Error as JsonRpcError;

use crate::ServerError;

/// Wire `ServerError` into the LSP JSON-RPC layer. All variants collapse to
/// `InternalError` (-32603) day-1: storage / cold-start / watcher / io
/// failures are infrastructure issues from the client's standpoint, and
/// the chained `Display` carries the detail. Per-variant codes can be
/// introduced later without breaking callers (additive).
impl From<ServerError> for JsonRpcError {
    fn from(err: ServerError) -> Self {
        let mut out = Self::internal_error();
        out.message = format!("{err}").into();
        out
    }
}

#[cfg(test)]
mod tests {
    use tower_lsp_server::jsonrpc::ErrorCode;

    use super::*;

    #[test]
    fn server_error_maps_to_internal_error_with_chained_display() {
        let server_err = ServerError::Io(std::io::Error::other("disk full"));
        let json: JsonRpcError = server_err.into();
        assert_eq!(json.code, ErrorCode::InternalError);
        assert!(
            json.message.contains("io: disk full"),
            "message must surface the chained Display, got `{}`",
            json.message
        );
    }
}

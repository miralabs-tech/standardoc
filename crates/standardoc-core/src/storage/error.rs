use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("connection pool error: {0}")]
    Pool(#[from] r2d2::Error),

    #[error("schema migration from v{from} to v{to} failed")]
    MigrationFailed {
        from: u32,
        to: u32,
        #[source]
        source: rusqlite::Error,
    },

    #[error(
        "database schema version v{db} is newer than supported v{supported} — \
         upgrade the binary or rescan from scratch"
    )]
    SchemaVersionTooNew { db: u32, supported: u32 },

    #[error("database lock held by another process: {path}")]
    LockHeld { path: PathBuf },

    #[error(
        "read-only open requested but no index database found at {path} — \
         start a writer (LSP daemon, `standardoc index`, ...) on this workspace first"
    )]
    ReadOnlyMissingDatabase { path: PathBuf },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("check constraint violated: {detail}")]
    CheckConstraintViolated { detail: String },

    #[error("invalid schema metadata: {key} = {value}")]
    InvalidSchemaMetadata { key: String, value: String },

    #[error("invalid stored data: {detail}")]
    InvalidStoredData { detail: String },

    #[error("FTS5 integrity-check reported corruption: {detail}")]
    FtsCorrupted { detail: String },

    #[error(
        "symbols_fts row count {fts} differs from symbols count {symbols}; \
         FTS rebuild required"
    )]
    FtsCountMismatch { symbols: i64, fts: i64 },

    #[error("rescan in progress; the connection pool is being rebuilt — retry the operation")]
    RescanInProgress,
}

/// Maps a `rusqlite::Error` to a typed `StorageError`, routing
/// `SQLITE_CONSTRAINT_CHECK` extended codes to `CheckConstraintViolated`
/// instead of opaque `Sqlite(_)`. Other variants pass through unchanged.
pub(crate) fn map_constraint(err: rusqlite::Error) -> StorageError {
    if let rusqlite::Error::SqliteFailure(ref ffi_err, ref msg) = err
        && ffi_err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_CHECK
    {
        let detail = msg.clone().unwrap_or_else(|| String::from("unknown CHECK constraint"));
        return StorageError::CheckConstraintViolated { detail };
    }
    StorageError::Sqlite(err)
}

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum RagError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("rag store handle is poisoned")]
    Poisoned,
    #[error(
        "rag schema version v{db} is newer than supported v{supported} — \
         upgrade the binary"
    )]
    SchemaVersionTooNew { db: u32, supported: u32 },
    #[error("invalid schema metadata: {key} = {value}")]
    InvalidSchemaMetadata { key: String, value: String },
    #[error("invalid stored data: {detail}")]
    InvalidStoredData { detail: String },
    #[error("invalid rag uri: {uri}")]
    InvalidUri { uri: String },
    #[error("chunker error: {detail}")]
    Chunker { detail: String },
    #[error("embedder error: {detail}")]
    Embedder { detail: String },
    #[error("model file not found at {path}")]
    ModelNotFound { path: PathBuf },
    #[error(
        "model dimension mismatch: chunk used model with dim={chunk_dim}, store expects dim={store_dim}"
    )]
    DimensionMismatch { chunk_dim: u32, store_dim: u32 },
}

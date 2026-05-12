//! Schema for `.standardoc/rag.db`. Versioned independently from the main
//! index. Phase A ships **v1** (chunks + embeddings + links). Future phases
//! migrate forward.

use rusqlite::Connection;

use crate::error::RagError;
use crate::types::EmbedModel;

pub const SUPPORTED_SCHEMA_VERSION: u32 = 1;

/// Default cap for the cascade chunker. Sections under this token count
/// stay as a single chunk ; longer ones cascade (H3 → paragraphs → sliding
/// window of `CHUNKER_MAX_TOKENS` / `CHUNKER_SLIDING_OVERLAP`).
pub const CHUNKER_MAX_TOKENS_DEFAULT: u32 = 512;
pub const CHUNKER_SLIDING_OVERLAP_DEFAULT: u32 = 64;

const DDL_V1: &str = r"
CREATE TABLE IF NOT EXISTS schema_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS chunks (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    source_path     TEXT    NOT NULL,
    chunk_idx       INTEGER NOT NULL,
    text            TEXT    NOT NULL,
    text_hash       TEXT    NOT NULL,
    section_header  TEXT,
    byte_start      INTEGER NOT NULL,
    byte_end        INTEGER NOT NULL,
    created_at      INTEGER NOT NULL,
    UNIQUE(source_path, chunk_idx)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_chunks_source ON chunks(source_path);
CREATE INDEX IF NOT EXISTS idx_chunks_text_hash ON chunks(text_hash);

CREATE TABLE IF NOT EXISTS chunk_embeddings (
    chunk_id INTEGER PRIMARY KEY REFERENCES chunks(id) ON DELETE CASCADE,
    model_id TEXT    NOT NULL,
    dim      INTEGER NOT NULL,
    vector   BLOB    NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS chunk_symbol_links (
    chunk_id      INTEGER NOT NULL REFERENCES chunks(id) ON DELETE CASCADE,
    fqdn          TEXT    NOT NULL,
    confidence    REAL    NOT NULL,
    source        TEXT    NOT NULL,
    def_site_path TEXT,
    PRIMARY KEY (chunk_id, fqdn)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_links_fqdn ON chunk_symbol_links(fqdn);
CREATE INDEX IF NOT EXISTS idx_links_fqdn_conf
    ON chunk_symbol_links(fqdn, confidence DESC);
";

/// Bootstraps the schema and seeds `schema_meta` defaults. Idempotent.
pub fn ensure_schema(conn: &Connection, model: &EmbedModel) -> Result<(), RagError> {
    conn.execute_batch(DDL_V1)?;
    seed_schema_meta(conn, model)?;
    let on_disk = read_schema_version(conn)?;
    if on_disk > SUPPORTED_SCHEMA_VERSION {
        return Err(RagError::SchemaVersionTooNew {
            db: on_disk,
            supported: SUPPORTED_SCHEMA_VERSION,
        });
    }
    Ok(())
}

fn seed_schema_meta(conn: &Connection, model: &EmbedModel) -> Result<(), RagError> {
    upsert_meta(conn, "schema_version", &SUPPORTED_SCHEMA_VERSION.to_string())?;
    upsert_meta(conn, "embed_model_id", &model.id)?;
    upsert_meta(conn, "embed_dim", &model.dim.to_string())?;
    upsert_meta_if_absent(
        conn,
        "chunker_max_tokens",
        &CHUNKER_MAX_TOKENS_DEFAULT.to_string(),
    )?;
    upsert_meta_if_absent(
        conn,
        "chunker_sliding_overlap",
        &CHUNKER_SLIDING_OVERLAP_DEFAULT.to_string(),
    )?;
    Ok(())
}

fn upsert_meta(conn: &Connection, key: &str, value: &str) -> Result<(), RagError> {
    conn.execute(
        "INSERT INTO schema_meta(key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![key, value],
    )?;
    Ok(())
}

fn upsert_meta_if_absent(conn: &Connection, key: &str, value: &str) -> Result<(), RagError> {
    conn.execute(
        "INSERT INTO schema_meta(key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO NOTHING",
        rusqlite::params![key, value],
    )?;
    Ok(())
}

fn read_schema_version(conn: &Connection) -> Result<u32, RagError> {
    let value: String = conn.query_row(
        "SELECT value FROM schema_meta WHERE key = 'schema_version'",
        [],
        |r| r.get(0),
    )?;
    value
        .parse::<u32>()
        .map_err(|_| RagError::InvalidSchemaMetadata {
            key: "schema_version".to_string(),
            value,
        })
}

pub fn read_meta(conn: &Connection, key: &str) -> Result<Option<String>, RagError> {
    use rusqlite::OptionalExtension;
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM schema_meta WHERE key = ?1",
            [key],
            |r| r.get(0),
        )
        .optional()?;
    Ok(value)
}

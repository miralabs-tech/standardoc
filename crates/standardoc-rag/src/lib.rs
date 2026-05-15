//! Standardoc RAG layer.
//!
//! Sidecar to `standardoc-core`. The graph stays the source of truth; this
//! crate enriches symbols with references to **prose chunks** (extended
//! documentation, ADRs, design notes) that live outside the AST.
//!
//! Architectural invariants (locked design):
//!
//! 1. **Graph = source of truth.** Chunks are *referenced* by symbols, never
//!    inlined in `get_context` payloads. The consumer fetches a chunk
//!    explicitly via the `fetch_chunks` MCP tool.
//! 2. **Lazy fetch.** `get_context(fqdn)` returns lightweight `ChunkRef`
//!    envelopes `{uri, confidence}`. Content is materialised on demand.
//! 3. **Scope = prose only.** Code bodies are served by the existing
//!    `get_body(fqdn)` path. This crate is for `README`, `docs/**/*.md`,
//!    ADRs, RFCs, etc.
//! 4. **Sidecar storage.** All RAG state lives in
//!    `<workspace>/.standardoc/rag.db`, separate from `index.db` and from
//!    `.standardoc-sessions/sessions.db`. Wiping it does not affect the
//!    code graph or session memos.
//! 5. **Local-only embeddings.** No API calls. Candle (pure Rust) +
//!    BGE-small-en-v1.5 (384-dim). Model downloaded on first cold start
//!    that touches prose, cached under `~/.cache/standardoc/models/`.
//!
//! The public surface (Phase A): types + store skeleton + chunker / embedder
//! / linker traits with `todo!("Phase B")` bodies. Phase B wires real
//! candle inference, the sqlite-vec virtual table, and the auto-FQDN
//! linker.

pub mod chunker;
pub mod embedder;
pub mod error;
pub mod linker;
pub mod markdown;
pub mod schema;
pub mod score;
pub mod store;
pub mod types;

pub use error::RagError;
pub use store::RagStore;
pub use types::{
    Chunk, ChunkId, ChunkRef, ChunkSymbolLink, EmbedModel, LinkSource, RAG_URI_SCHEME,
};

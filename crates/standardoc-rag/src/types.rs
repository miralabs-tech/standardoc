use std::fmt;

use crate::error::RagError;

/// Stable URI prefix for chunk references. Single-store day-1 ; future
/// multi-store namespacing would parse `rag://<store>:<id>`. Today
/// `rag://<id>` only.
pub const RAG_URI_SCHEME: &str = "rag";

/// Opaque integer id assigned by the rag store. 1:1 with `chunks.id`
/// (`INTEGER PRIMARY KEY AUTOINCREMENT`). Wrapped to keep accidental
/// conflation with `RawSymbol`-side ids type-checked.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct ChunkId(pub i64);

impl ChunkId {
    #[inline]
    pub const fn raw(self) -> i64 {
        self.0
    }

    /// Renders the canonical URI form `rag://<id>`.
    pub fn to_uri(self) -> String {
        format!("{RAG_URI_SCHEME}://{}", self.0)
    }

    /// Parses a URI of the form `rag://<id>`. Future multi-store form
    /// `rag://<store>:<id>` will be accepted by tolerating a `:`-namespaced
    /// id with the default store implicit ; this Phase A parser is strict.
    pub fn from_uri(uri: &str) -> Result<Self, RagError> {
        let prefix = format!("{RAG_URI_SCHEME}://");
        let rest = uri
            .strip_prefix(&prefix)
            .ok_or_else(|| RagError::InvalidUri {
                uri: uri.to_string(),
            })?;
        let id: i64 = rest.parse().map_err(|_| RagError::InvalidUri {
            uri: uri.to_string(),
        })?;
        Ok(Self(id))
    }
}

impl fmt::Display for ChunkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A single prose chunk persisted in `chunks`. The full materialised view ;
/// for query-time `get_context` payloads we return `ChunkRef` envelopes
/// instead (lightweight, no `text`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Chunk {
    pub id: ChunkId,
    /// Workspace-relative path of the source `.md` file the chunk came from.
    pub source_path: String,
    /// 0-based ordinal of this chunk within the source file (stable across
    /// re-embeds when the chunker output is deterministic).
    pub chunk_idx: u32,
    /// Raw chunk text (post-chunker, pre-embedding).
    pub text: String,
    /// BLAKE3 hex of `text`. Skip-rebuild key for the watcher / cold start.
    pub text_hash: String,
    /// Nearest enclosing H2 / H3 title if any (display affordance).
    pub section_header: Option<String>,
    /// Byte offsets in the source file (used for click-through and
    /// inclusion in MCP responses).
    pub byte_start: u32,
    pub byte_end: u32,
    /// Unix seconds.
    pub created_at: i64,
}

/// Lightweight reference shipped in `get_context` responses. The consumer
/// fetches the actual chunk text via `fetch_chunks([uri, ...])` (the MCP
/// tool added at lock time, tools 13 → 14).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ChunkRef {
    /// Canonical `rag://<id>` URI.
    pub uri: String,
    /// Final blended confidence in `[0.0, 1.0]`. Without a user query this
    /// is `link_confidence × def_site_boost` capped at 1.0 (see
    /// `score::compute_final_score`). With a query, this is the blended
    /// `(pre × 0.5 + query_score × 0.5)`.
    pub confidence: f32,
    /// Source path of the underlying chunk — helpful for the AI to decide
    /// whether to fetch (e.g. ignore third-party README).
    pub source_path: String,
    /// Nearest H2 / H3 if known. Display-only.
    pub section_header: Option<String>,
}

/// One linking row in `chunk_symbol_links`. A given (chunk, fqdn) pair is
/// unique ; if multiple signals match, the link source with the **highest**
/// confidence wins (frontmatter > auto_fqdn_exact > auto_name_substring).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ChunkSymbolLink {
    pub chunk_id: ChunkId,
    pub fqdn: String,
    /// Pre-computed `link_confidence × def_site_boost` (capped 1.0).
    /// See `score::compute_link_confidence` + `score::apply_def_site_boost`.
    pub confidence: f32,
    pub source: LinkSource,
    /// Workspace-relative path of the symbol's def-site, captured at link
    /// time for the `def_site_boost` signal. `None` if unknown.
    pub def_site_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkSource {
    /// Author declared the link explicitly in the file's frontmatter
    /// (`--- symbols: [a::b, c::d] ---`). Base confidence `1.0`.
    Frontmatter,
    /// Chunk text contains the full FQDN literally (`auth::login`). Base
    /// confidence `0.7`.
    AutoFqdnExact,
    /// Chunk text contains a long identifier matching the symbol's short
    /// name only. More ambiguous. Base confidence `0.4`.
    AutoNameSubstring,
}

impl LinkSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Frontmatter => "frontmatter",
            Self::AutoFqdnExact => "auto_fqdn_exact",
            Self::AutoNameSubstring => "auto_name_substring",
        }
    }

    pub fn from_sql(s: &str) -> Result<Self, RagError> {
        match s {
            "frontmatter" => Ok(Self::Frontmatter),
            "auto_fqdn_exact" => Ok(Self::AutoFqdnExact),
            "auto_name_substring" => Ok(Self::AutoNameSubstring),
            other => Err(RagError::InvalidStoredData {
                detail: format!("unknown link source: {other:?}"),
            }),
        }
    }

    /// Base confidence for this link source, before any def-site boost.
    pub const fn base_confidence(self) -> f32 {
        match self {
            Self::Frontmatter => 1.0,
            Self::AutoFqdnExact => 0.7,
            Self::AutoNameSubstring => 0.4,
        }
    }
}

/// Identifier of the embedding model the store was initialised with. Mixing
/// models in the same `chunk_embeddings` table is an error (`DimensionMismatch`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EmbedModel {
    /// Short id, e.g. `"bge-small-en-v1.5"`. Stored in `chunk_embeddings.model_id`
    /// and in `schema_meta.embed_model_id` at store init.
    pub id: String,
    /// Vector dimension produced by this model. `384` for BGE-small.
    pub dim: u32,
}

impl EmbedModel {
    /// Default local model locked for v1.0.0-beta.2.
    pub fn bge_small_en_v1_5() -> Self {
        Self {
            id: "bge-small-en-v1.5".to_string(),
            dim: 384,
        }
    }
}

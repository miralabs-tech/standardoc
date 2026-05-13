//! `RagStore` — workspace-scoped handle to `.standardoc/rag.db`. Mirrors
//! the `SessionsHandle` pattern from `standardoc-core::sessions` :
//! single-connection wrapped in a `Mutex` for serialised writes.
//!
//! Vector retrieval uses **brute-force cosine** over a `BLOB` column
//! (Rust-side scan, one normalised dot product per candidate). For the
//! `<100k` chunk regime targeted by v1.0.0-beta.2 this is sub-second and
//! avoids the platform-specific .so/.dll dance that `sqlite-vec` would
//! impose. Swapping in a vector ANN index is a perf-only follow-up that
//! does not change this API surface.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, ToSql};

use crate::chunker::ChunkPiece;
use crate::error::RagError;
use crate::schema::{ensure_schema, read_meta};
use crate::score::{blend_with_query, cosine_similarity};
use crate::types::{Chunk, ChunkId, ChunkRef, ChunkSymbolLink, EmbedModel};

const RAG_DIR: &str = ".standardoc";
const RAG_DB: &str = "rag.db";

pub struct RagStore {
    conn: Mutex<Connection>,
    db_path: PathBuf,
    model: EmbedModel,
}

impl RagStore {
    /// Opens (creating if absent) `<workspace>/.standardoc/rag.db`. Applies
    /// schema migrations and seeds `schema_meta` with the embedding model
    /// identifier — opening a store previously initialised with a
    /// different model is allowed (the value is overwritten), but mixing
    /// rows from two models in `chunk_embeddings` surfaces as
    /// `RagError::DimensionMismatch` on insert.
    pub fn open(workspace_root: &Path, model: EmbedModel) -> Result<Self, RagError> {
        let dir = workspace_root.join(RAG_DIR);
        std::fs::create_dir_all(&dir)?;
        let db_path = dir.join(RAG_DB);
        let conn = Connection::open(&db_path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; \
             PRAGMA synchronous = NORMAL;",
        )?;
        ensure_schema(&conn, &model)?;
        Ok(Self {
            conn: Mutex::new(conn),
            db_path,
            model,
        })
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub const fn model(&self) -> &EmbedModel {
        &self.model
    }

    /// Returns the stored value of a `schema_meta` key. Useful for the
    /// chunker (token cap) and the watcher (debounce).
    pub fn read_meta(&self, key: &str) -> Result<Option<String>, RagError> {
        let guard = self.conn.lock().map_err(|_| RagError::Poisoned)?;
        read_meta(&guard, key)
    }

    /// Upserts a `schema_meta` key/value. Counterpart to [`read_meta`].
    /// Backs the workspace fqdn hash storage used by the relink-all
    /// early-exit path.
    #[allow(clippy::significant_drop_tightening)]
    pub fn write_meta(&self, key: &str, value: &str) -> Result<(), RagError> {
        let guard = self.conn.lock().map_err(|_| RagError::Poisoned)?;
        guard.execute(
            "INSERT INTO schema_meta(key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }

    /// Atomically replaces every chunk + embedding tied to `source_path`.
    /// Mirrors `apply_documents` in `standardoc-core::pipeline` :
    /// delete-then-insert in one transaction. `pieces` and `vectors`
    /// must have the same length. `text_hash` is computed BLAKE3 on the
    /// piece text inside this call.
    #[allow(clippy::significant_drop_tightening)]
    pub fn replace_chunks_for_source(
        &self,
        source_path: &str,
        pieces: &[ChunkPiece],
        vectors: &[Vec<f32>],
    ) -> Result<Vec<ChunkId>, RagError> {
        if pieces.len() != vectors.len() {
            return Err(RagError::InvalidStoredData {
                detail: format!(
                    "pieces.len()={} != vectors.len()={}",
                    pieces.len(),
                    vectors.len()
                ),
            });
        }
        let expected_dim = usize::try_from(self.model.dim).unwrap_or(0);
        for v in vectors {
            if v.len() != expected_dim {
                return Err(RagError::DimensionMismatch {
                    chunk_dim: u32::try_from(v.len()).unwrap_or(u32::MAX),
                    store_dim: self.model.dim,
                });
            }
        }
        let now = current_unix_seconds();
        let mut guard = self.conn.lock().map_err(|_| RagError::Poisoned)?;
        let tx = guard.transaction()?;
        tx.execute("DELETE FROM chunks WHERE source_path = ?1", [source_path])?;
        let mut ids = Vec::with_capacity(pieces.len());
        for (idx, (piece, vector)) in pieces.iter().zip(vectors.iter()).enumerate() {
            let text_hash = blake3::hash(piece.text.as_bytes()).to_hex().to_string();
            let chunk_idx = u32::try_from(idx).map_err(|_| RagError::InvalidStoredData {
                detail: "chunk index overflow".into(),
            })?;
            tx.execute(
                "INSERT INTO chunks (source_path, chunk_idx, text, text_hash, \
                 section_header, byte_start, byte_end, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    source_path,
                    chunk_idx,
                    piece.text,
                    text_hash,
                    piece.section_header,
                    piece.byte_start,
                    piece.byte_end,
                    now,
                ],
            )?;
            let id = tx.last_insert_rowid();
            let bytes = vec_to_bytes(vector);
            tx.execute(
                "INSERT INTO chunk_embeddings (chunk_id, model_id, dim, vector) \
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![id, self.model.id, self.model.dim, bytes],
            )?;
            ids.push(ChunkId(id));
        }
        tx.commit()?;
        Ok(ids)
    }

    /// Deletes every chunk whose `source_path` is NOT in `live_sources`.
    /// `live_sources` empty → wipes the whole `chunks` table. Returns
    /// the number of rows deleted.
    #[allow(clippy::significant_drop_tightening)]
    pub fn purge_orphan_sources(&self, live_sources: &[String]) -> Result<usize, RagError> {
        let guard = self.conn.lock().map_err(|_| RagError::Poisoned)?;
        if live_sources.is_empty() {
            let count = guard.execute("DELETE FROM chunks", [])?;
            return Ok(count);
        }
        let placeholders = vec!["?"; live_sources.len()].join(",");
        let sql = format!("DELETE FROM chunks WHERE source_path NOT IN ({placeholders})");
        let params: Vec<&dyn ToSql> = live_sources.iter().map(|s| s as &dyn ToSql).collect();
        let count = guard.execute(&sql, &params[..])?;
        Ok(count)
    }

    /// Replaces every `chunk_symbol_links` row tied to `chunk_id` with
    /// the provided set. Idempotent re-runs of the linker pass.
    #[allow(clippy::significant_drop_tightening)]
    pub fn replace_links_for_chunk(
        &self,
        chunk_id: ChunkId,
        links: &[ChunkSymbolLink],
    ) -> Result<(), RagError> {
        let mut guard = self.conn.lock().map_err(|_| RagError::Poisoned)?;
        let tx = guard.transaction()?;
        tx.execute(
            "DELETE FROM chunk_symbol_links WHERE chunk_id = ?1",
            [chunk_id.raw()],
        )?;
        for link in links {
            tx.execute(
                "INSERT INTO chunk_symbol_links \
                 (chunk_id, fqdn, confidence, source, def_site_path) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    chunk_id.raw(),
                    link.fqdn,
                    link.confidence,
                    link.source.as_str(),
                    link.def_site_path,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Returns the `ChunkRef` envelopes linked to `fqdn`, ordered by
    /// pre-computed `confidence DESC`. Used by `get_context(fqdn)`.
    #[allow(clippy::significant_drop_tightening)]
    pub fn refs_for_symbol(&self, fqdn: &str, limit: u32) -> Result<Vec<ChunkRef>, RagError> {
        let guard = self.conn.lock().map_err(|_| RagError::Poisoned)?;
        let mut stmt = guard.prepare(
            "SELECT c.id, c.source_path, c.section_header, l.confidence \
             FROM chunk_symbol_links l \
             JOIN chunks c ON c.id = l.chunk_id \
             WHERE l.fqdn = ?1 \
             ORDER BY l.confidence DESC, c.id ASC \
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![fqdn, limit], row_to_chunk_ref)?;
        let out = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(out)
    }

    /// Re-ranks pre-computed refs against a user query embedding.
    /// Implementation : brute-force cosine over every linked chunk. The
    /// returned `confidence` is `0.5 × pre + 0.5 × cos`.
    #[allow(clippy::significant_drop_tightening)]
    pub fn refs_for_symbol_with_query(
        &self,
        fqdn: &str,
        query_vector: &[f32],
        limit: u32,
    ) -> Result<Vec<ChunkRef>, RagError> {
        let expected_dim = usize::try_from(self.model.dim).unwrap_or(0);
        if query_vector.len() != expected_dim {
            return Err(RagError::DimensionMismatch {
                chunk_dim: u32::try_from(query_vector.len()).unwrap_or(u32::MAX),
                store_dim: self.model.dim,
            });
        }
        let guard = self.conn.lock().map_err(|_| RagError::Poisoned)?;
        let mut stmt = guard.prepare(
            "SELECT c.id, c.source_path, c.section_header, l.confidence, e.vector \
             FROM chunk_symbol_links l \
             JOIN chunks c ON c.id = l.chunk_id \
             JOIN chunk_embeddings e ON e.chunk_id = c.id \
             WHERE l.fqdn = ?1",
        )?;
        let rows = stmt.query_map([fqdn], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, f32>(3)?,
                r.get::<_, Vec<u8>>(4)?,
            ))
        })?;

        let mut scored: Vec<ChunkRef> = Vec::new();
        for row in rows {
            let (id, source_path, section_header, pre_conf, vec_bytes) = row?;
            let vector = bytes_to_vec(&vec_bytes)?;
            let cos = cosine_similarity(&vector, query_vector).max(0.0);
            let blended = blend_with_query(pre_conf, cos);
            scored.push(ChunkRef {
                uri: ChunkId(id).to_uri(),
                confidence: blended,
                source_path,
                section_header,
            });
        }
        scored.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.uri.cmp(&b.uri))
        });
        let take = usize::try_from(limit).unwrap_or(usize::MAX);
        scored.truncate(take);
        Ok(scored)
    }

    /// Resolves a list of `rag://<id>` URIs to full `Chunk` rows. Backs
    /// the `fetch_chunks` MCP tool. Unknown ids are silently skipped
    /// (the caller may diff inputs vs outputs to detect them). Returned
    /// chunks are ordered by id ASC.
    #[allow(clippy::significant_drop_tightening)]
    pub fn fetch_by_uris(&self, uris: &[String]) -> Result<Vec<Chunk>, RagError> {
        if uris.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<i64> = uris
            .iter()
            .filter_map(|u| ChunkId::from_uri(u).ok().map(ChunkId::raw))
            .collect();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let guard = self.conn.lock().map_err(|_| RagError::Poisoned)?;
        let placeholders = vec!["?"; ids.len()].join(",");
        let sql = format!(
            "SELECT id, source_path, chunk_idx, text, text_hash, section_header, \
             byte_start, byte_end, created_at \
             FROM chunks WHERE id IN ({placeholders}) ORDER BY id ASC"
        );
        let params: Vec<&dyn ToSql> = ids.iter().map(|i| i as &dyn ToSql).collect();
        let mut stmt = guard.prepare(&sql)?;
        let rows = stmt.query_map(&params[..], row_to_chunk)?;
        let out = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(out)
    }

    /// Returns the BLAKE3 hex of every chunk currently stored for
    /// `source_path`, in `chunk_idx` order. Hash-skip key for the
    /// watcher : compare against the chunker output for a touched file ;
    /// if the sequences match, skip the re-embed entirely.
    #[allow(clippy::significant_drop_tightening)]
    pub fn hashes_for_source(&self, source_path: &str) -> Result<Vec<String>, RagError> {
        let guard = self.conn.lock().map_err(|_| RagError::Poisoned)?;
        let mut stmt = guard.prepare(
            "SELECT text_hash FROM chunks WHERE source_path = ?1 ORDER BY chunk_idx ASC",
        )?;
        let rows = stmt.query_map([source_path], |r| r.get::<_, String>(0))?;
        let out = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(out)
    }

    /// Returns `(chunk_id, text)` for every chunk currently stored for
    /// `source_path`, in `chunk_idx` ASC order. Backs the graph-change
    /// re-link pass : the chunker / embedder are NOT re-run, only the
    /// `chunk_symbol_links` rows are rewritten against the current
    /// workspace fqdn set.
    #[allow(clippy::significant_drop_tightening)]
    pub fn chunks_with_text_for_source(
        &self,
        source_path: &str,
    ) -> Result<Vec<(ChunkId, String)>, RagError> {
        let guard = self.conn.lock().map_err(|_| RagError::Poisoned)?;
        let mut stmt = guard.prepare(
            "SELECT id, text FROM chunks WHERE source_path = ?1 ORDER BY chunk_idx ASC",
        )?;
        let rows = stmt.query_map([source_path], |r| {
            Ok((ChunkId(r.get::<_, i64>(0)?), r.get::<_, String>(1)?))
        })?;
        let out = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(out)
    }

    /// Distinct `source_path` values currently present in the `chunks`
    /// table. Backs `relink_all` discovery — we only relink sources
    /// already chunked (no need to re-run prose-side filesystem
    /// discovery, which is `run_rag_cold_start`'s job).
    #[allow(clippy::significant_drop_tightening)]
    pub fn distinct_source_paths(&self) -> Result<Vec<String>, RagError> {
        let guard = self.conn.lock().map_err(|_| RagError::Poisoned)?;
        let mut stmt = guard.prepare(
            "SELECT DISTINCT source_path FROM chunks ORDER BY source_path ASC",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let out = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(out)
    }
}

fn row_to_chunk_ref(r: &rusqlite::Row<'_>) -> rusqlite::Result<ChunkRef> {
    let id: i64 = r.get(0)?;
    let source_path: String = r.get(1)?;
    let section_header: Option<String> = r.get(2)?;
    let confidence: f32 = r.get(3)?;
    Ok(ChunkRef {
        uri: ChunkId(id).to_uri(),
        source_path,
        section_header,
        confidence,
    })
}

fn row_to_chunk(r: &rusqlite::Row<'_>) -> rusqlite::Result<Chunk> {
    Ok(Chunk {
        id: ChunkId(r.get(0)?),
        source_path: r.get(1)?,
        chunk_idx: r.get(2)?,
        text: r.get(3)?,
        text_hash: r.get(4)?,
        section_header: r.get(5)?,
        byte_start: r.get(6)?,
        byte_end: r.get(7)?,
        created_at: r.get(8)?,
    })
}

fn vec_to_bytes(v: &[f32]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(v.len() * 4);
    for x in v {
        buf.extend_from_slice(&x.to_le_bytes());
    }
    buf
}

fn bytes_to_vec(b: &[u8]) -> Result<Vec<f32>, RagError> {
    if !b.len().is_multiple_of(4) {
        return Err(RagError::InvalidStoredData {
            detail: format!("vector blob length {} not divisible by 4", b.len()),
        });
    }
    let mut out = Vec::with_capacity(b.len() / 4);
    for chunk in b.chunks_exact(4) {
        out.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    Ok(out)
}

fn current_unix_seconds() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunker::ChunkPiece;
    use crate::types::LinkSource;
    use tempfile::TempDir;

    fn fresh_store() -> (TempDir, RagStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = RagStore::open(dir.path(), EmbedModel::bge_small_en_v1_5()).unwrap();
        (dir, store)
    }

    fn dummy_vector(seed: u8) -> Vec<f32> {
        let mut v = vec![0.0f32; 384];
        v[0] = f32::from(seed) / 255.0;
        v[1] = 1.0 - v[0];
        // L2-normalise so the brute-force cosine math behaves predictably.
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in &mut v {
                *x /= norm;
            }
        }
        v
    }

    fn piece(text: &str) -> ChunkPiece {
        ChunkPiece {
            text: text.to_string(),
            section_header: None,
            byte_start: 0,
            byte_end: u32::try_from(text.len()).unwrap(),
        }
    }

    #[test]
    fn open_creates_db_under_standardoc_dir() {
        let (dir, store) = fresh_store();
        let expected = dir.path().join(RAG_DIR).join(RAG_DB);
        assert_eq!(store.db_path(), expected);
        assert!(expected.exists());
    }

    #[test]
    fn open_is_idempotent_across_reopens() {
        let dir = tempfile::tempdir().unwrap();
        {
            let _ = RagStore::open(dir.path(), EmbedModel::bge_small_en_v1_5()).unwrap();
        }
        let store = RagStore::open(dir.path(), EmbedModel::bge_small_en_v1_5()).unwrap();
        let version = store.read_meta("schema_version").unwrap();
        assert_eq!(version.as_deref(), Some("1"));
    }

    #[test]
    fn open_seeds_default_model_metadata() {
        let (_dir, store) = fresh_store();
        assert_eq!(
            store.read_meta("embed_model_id").unwrap().as_deref(),
            Some("bge-small-en-v1.5"),
        );
        assert_eq!(
            store.read_meta("embed_dim").unwrap().as_deref(),
            Some("384")
        );
    }

    #[test]
    fn open_seeds_chunker_defaults_once_and_preserves_them() {
        let dir = tempfile::tempdir().unwrap();
        let _ = RagStore::open(dir.path(), EmbedModel::bge_small_en_v1_5()).unwrap();
        let store = RagStore::open(dir.path(), EmbedModel::bge_small_en_v1_5()).unwrap();
        assert_eq!(
            store.read_meta("chunker_max_tokens").unwrap().as_deref(),
            Some("512"),
        );
        assert_eq!(
            store
                .read_meta("chunker_sliding_overlap")
                .unwrap()
                .as_deref(),
            Some("64"),
        );
    }

    #[test]
    fn replace_chunks_round_trips_pieces() {
        let (_dir, store) = fresh_store();
        let pieces = vec![piece("alpha"), piece("bravo")];
        let vectors = vec![dummy_vector(1), dummy_vector(2)];
        let ids = store
            .replace_chunks_for_source("docs/a.md", &pieces, &vectors)
            .unwrap();
        assert_eq!(ids.len(), 2);
        let hashes = store.hashes_for_source("docs/a.md").unwrap();
        assert_eq!(hashes.len(), 2);
        for h in &hashes {
            assert_eq!(h.len(), 64, "BLAKE3 hex is 64 chars");
        }
    }

    #[test]
    fn replace_chunks_second_call_replaces_existing_chunks() {
        let (_dir, store) = fresh_store();
        let v1 = vec![dummy_vector(1)];
        let v2 = vec![dummy_vector(2)];
        store
            .replace_chunks_for_source("docs/a.md", &[piece("v1 text")], &v1)
            .unwrap();
        let first_hashes = store.hashes_for_source("docs/a.md").unwrap();
        store
            .replace_chunks_for_source("docs/a.md", &[piece("v2 different text")], &v2)
            .unwrap();
        let second_hashes = store.hashes_for_source("docs/a.md").unwrap();
        assert_eq!(first_hashes.len(), 1);
        assert_eq!(second_hashes.len(), 1);
        assert_ne!(first_hashes[0], second_hashes[0]);
    }

    #[test]
    fn replace_chunks_dimension_mismatch_errors_out() {
        let (_dir, store) = fresh_store();
        let bad = vec![vec![0.0f32; 100]];
        let res = store.replace_chunks_for_source("docs/a.md", &[piece("x")], &bad);
        assert!(matches!(res, Err(RagError::DimensionMismatch { .. })));
    }

    #[test]
    fn replace_chunks_pieces_vectors_length_mismatch_errors_out() {
        let (_dir, store) = fresh_store();
        let res = store.replace_chunks_for_source("docs/a.md", &[piece("x")], &[]);
        assert!(matches!(res, Err(RagError::InvalidStoredData { .. })));
    }

    #[test]
    fn purge_orphan_sources_keeps_listed_paths() {
        let (_dir, store) = fresh_store();
        store
            .replace_chunks_for_source("a.md", &[piece("a")], &[dummy_vector(1)])
            .unwrap();
        store
            .replace_chunks_for_source("b.md", &[piece("b")], &[dummy_vector(2)])
            .unwrap();
        store
            .replace_chunks_for_source("c.md", &[piece("c")], &[dummy_vector(3)])
            .unwrap();
        let purged = store
            .purge_orphan_sources(&["a.md".to_string(), "c.md".to_string()])
            .unwrap();
        assert_eq!(purged, 1, "only b.md must be deleted");
        assert!(store.hashes_for_source("b.md").unwrap().is_empty());
        assert_eq!(store.hashes_for_source("a.md").unwrap().len(), 1);
        assert_eq!(store.hashes_for_source("c.md").unwrap().len(), 1);
    }

    #[test]
    fn purge_orphan_sources_empty_live_list_wipes_chunks_table() {
        let (_dir, store) = fresh_store();
        store
            .replace_chunks_for_source("a.md", &[piece("a")], &[dummy_vector(1)])
            .unwrap();
        let purged = store.purge_orphan_sources(&[]).unwrap();
        assert_eq!(purged, 1);
        assert!(store.hashes_for_source("a.md").unwrap().is_empty());
    }

    #[test]
    fn replace_links_and_refs_for_symbol_round_trip() {
        let (_dir, store) = fresh_store();
        let ids = store
            .replace_chunks_for_source(
                "docs/auth.md",
                &[piece("auth::login is documented")],
                &[dummy_vector(1)],
            )
            .unwrap();
        let chunk_id = ids[0];
        let link = ChunkSymbolLink {
            chunk_id,
            fqdn: "auth::login".to_string(),
            confidence: 0.85,
            source: LinkSource::Frontmatter,
            def_site_path: Some("src/auth/login.rs".to_string()),
        };
        store.replace_links_for_chunk(chunk_id, &[link]).unwrap();
        let refs = store.refs_for_symbol("auth::login", 10).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].uri, chunk_id.to_uri());
        assert_eq!(refs[0].source_path, "docs/auth.md");
        assert!((refs[0].confidence - 0.85).abs() < 1e-6);
    }

    #[test]
    fn refs_for_symbol_orders_by_confidence_desc() {
        let (_dir, store) = fresh_store();
        let ids = store
            .replace_chunks_for_source(
                "docs/x.md",
                &[piece("one"), piece("two"), piece("three")],
                &[dummy_vector(1), dummy_vector(2), dummy_vector(3)],
            )
            .unwrap();
        for (i, &id) in ids.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let conf = (i as f32).mul_add(0.2, 0.3);
            let link = ChunkSymbolLink {
                chunk_id: id,
                fqdn: "x::y".into(),
                confidence: conf,
                source: LinkSource::AutoFqdnExact,
                def_site_path: None,
            };
            store.replace_links_for_chunk(id, &[link]).unwrap();
        }
        let refs = store.refs_for_symbol("x::y", 10).unwrap();
        assert_eq!(refs.len(), 3);
        for w in refs.windows(2) {
            assert!(w[0].confidence >= w[1].confidence);
        }
    }

    #[test]
    fn refs_for_symbol_with_query_reranks_by_cosine() {
        let (_dir, store) = fresh_store();
        // Two chunks linked at equal pre-computed confidence ; query
        // vector aligned with the second one. Re-rank must surface
        // chunk 2 first.
        let target = dummy_vector(99);
        let other = dummy_vector(7);
        let ids = store
            .replace_chunks_for_source(
                "docs/x.md",
                &[piece("noise"), piece("aligned")],
                &[other, target.clone()],
            )
            .unwrap();
        for &id in &ids {
            let link = ChunkSymbolLink {
                chunk_id: id,
                fqdn: "x::y".into(),
                confidence: 0.5,
                source: LinkSource::AutoFqdnExact,
                def_site_path: None,
            };
            store.replace_links_for_chunk(id, &[link]).unwrap();
        }
        let refs = store
            .refs_for_symbol_with_query("x::y", &target, 10)
            .unwrap();
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].uri, ids[1].to_uri());
    }

    #[test]
    fn refs_for_symbol_with_query_dimension_mismatch_errors() {
        let (_dir, store) = fresh_store();
        let bad_query = vec![0.0f32; 100];
        let res = store.refs_for_symbol_with_query("x::y", &bad_query, 10);
        assert!(matches!(res, Err(RagError::DimensionMismatch { .. })));
    }

    #[test]
    fn fetch_by_uris_returns_chunks_in_id_order() {
        let (_dir, store) = fresh_store();
        let ids = store
            .replace_chunks_for_source(
                "docs/x.md",
                &[piece("alpha"), piece("bravo")],
                &[dummy_vector(1), dummy_vector(2)],
            )
            .unwrap();
        let uris = vec![ids[1].to_uri(), ids[0].to_uri()];
        let chunks = store.fetch_by_uris(&uris).unwrap();
        assert_eq!(chunks.len(), 2);
        // ORDER BY id ASC regardless of input order.
        assert_eq!(chunks[0].id, ids[0]);
        assert_eq!(chunks[1].id, ids[1]);
        assert_eq!(chunks[0].text, "alpha");
        assert_eq!(chunks[1].text, "bravo");
    }

    #[test]
    fn fetch_by_uris_skips_invalid_and_unknown() {
        let (_dir, store) = fresh_store();
        let ids = store
            .replace_chunks_for_source("x.md", &[piece("a")], &[dummy_vector(1)])
            .unwrap();
        let uris = vec![
            "not-a-uri".to_string(),
            "rag://9999".to_string(),
            ids[0].to_uri(),
        ];
        let chunks = store.fetch_by_uris(&uris).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].id, ids[0]);
    }

    #[test]
    fn cascade_delete_removes_embeddings_and_links() {
        let (_dir, store) = fresh_store();
        let ids = store
            .replace_chunks_for_source("x.md", &[piece("a")], &[dummy_vector(1)])
            .unwrap();
        let chunk_id = ids[0];
        store
            .replace_links_for_chunk(
                chunk_id,
                &[ChunkSymbolLink {
                    chunk_id,
                    fqdn: "x::y".into(),
                    confidence: 0.9,
                    source: LinkSource::Frontmatter,
                    def_site_path: None,
                }],
            )
            .unwrap();
        // Deleting the chunk row should cascade to embeddings + links.
        store.purge_orphan_sources(&[]).unwrap();
        let refs = store.refs_for_symbol("x::y", 10).unwrap();
        assert!(refs.is_empty(), "links must cascade-delete with the chunk");
    }

    #[test]
    fn vec_to_bytes_roundtrips() {
        let v = vec![1.0f32, -2.5, 0.0, f32::MIN_POSITIVE];
        let b = vec_to_bytes(&v);
        let back = bytes_to_vec(&b).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn bytes_to_vec_rejects_malformed_length() {
        let res = bytes_to_vec(&[0, 1, 2]);
        assert!(matches!(res, Err(RagError::InvalidStoredData { .. })));
    }

    #[test]
    fn write_meta_upserts_idempotently_and_read_meta_observes_value() {
        let (_dir, store) = fresh_store();
        assert!(store.read_meta("custom_key").unwrap().is_none());
        store.write_meta("custom_key", "v1").unwrap();
        assert_eq!(store.read_meta("custom_key").unwrap().as_deref(), Some("v1"));
        store.write_meta("custom_key", "v2").unwrap();
        assert_eq!(store.read_meta("custom_key").unwrap().as_deref(), Some("v2"));
    }

    #[test]
    fn chunks_with_text_for_source_returns_ordered_pairs() {
        let (_dir, store) = fresh_store();
        let pieces = vec![
            ChunkPiece {
                text: "first chunk".into(),
                section_header: None,
                byte_start: 0,
                byte_end: 11,
            },
            ChunkPiece {
                text: "second chunk".into(),
                section_header: None,
                byte_start: 11,
                byte_end: 23,
            },
        ];
        let vectors = vec![dummy_vector(1), dummy_vector(2)];
        let ids = store
            .replace_chunks_for_source("README.md", &pieces, &vectors)
            .unwrap();
        let pairs = store.chunks_with_text_for_source("README.md").unwrap();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0], (ids[0], "first chunk".to_string()));
        assert_eq!(pairs[1], (ids[1], "second chunk".to_string()));
    }

    #[test]
    fn chunks_with_text_for_source_returns_empty_for_unknown_source() {
        let (_dir, store) = fresh_store();
        let pairs = store
            .chunks_with_text_for_source("does/not/exist.md")
            .unwrap();
        assert!(pairs.is_empty());
    }

    #[test]
    fn distinct_source_paths_lists_every_stored_source_sorted() {
        let (_dir, store) = fresh_store();
        let piece = ChunkPiece {
            text: "x".into(),
            section_header: None,
            byte_start: 0,
            byte_end: 1,
        };
        let vec = dummy_vector(1);
        let vecs = std::slice::from_ref(&vec);
        let pieces = std::slice::from_ref(&piece);
        store.replace_chunks_for_source("zeta.md", pieces, vecs).unwrap();
        store.replace_chunks_for_source("alpha.md", pieces, vecs).unwrap();
        store.replace_chunks_for_source("mu.md", pieces, vecs).unwrap();
        let paths = store.distinct_source_paths().unwrap();
        assert_eq!(paths, vec!["alpha.md", "mu.md", "zeta.md"]);
    }

    #[test]
    fn distinct_source_paths_empty_for_fresh_store() {
        let (_dir, store) = fresh_store();
        assert!(store.distinct_source_paths().unwrap().is_empty());
    }
}

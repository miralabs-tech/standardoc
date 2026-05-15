//! `RagPipeline` — chunk → embed → store → link orchestration.
//!
//! Single entry per source file (`run_for_source`) and a sweep over the
//! whole workspace (`run_cold_start`). The pipeline honours the
//! BLAKE3-hash skip key — if the chunker's deterministic output for a
//! file produces the same hash sequence already in `rag.db`, we skip
//! the (expensive) embed pass entirely.

use std::path::Path;
use std::sync::Arc;

use standardoc_rag::chunker::{Chunker, MarkdownCascadeChunker};
use standardoc_rag::embedder::Embedder;
use standardoc_rag::error::RagError;
use standardoc_rag::linker::{DefaultLinker, LinkInput, Linker, extract_frontmatter_block};
use standardoc_rag::store::RagStore;
use standardoc_rag::types::ChunkId;

use standardoc_rag::linker::SymbolLookup;

use crate::pipeline::ScanFilters;
use crate::rag::discovery::discover_prose_sources;
use crate::rag::lookup::CoreSymbolLookup;
use crate::storage::handle::IndexHandle;

/// `schema_meta` key used to cache the BLAKE3 hex of the sorted
/// workspace fqdn set on the last successful relink. Lets `relink_all`
/// no-op when the AST graph hasn't changed since the previous sweep.
const META_WORKSPACE_FQDNS_HASH: &str = "workspace_fqdns_hash";

#[derive(Debug, thiserror::Error)]
pub enum RagPipelineError {
    #[error("rag store error: {0}")]
    Rag(#[from] RagError),
    #[error("io error reading {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
}

pub struct RagPipeline {
    store: Arc<RagStore>,
    embedder: Arc<dyn Embedder>,
    chunker: Arc<dyn Chunker>,
    linker: Arc<dyn Linker>,
}

impl RagPipeline {
    /// Builds a pipeline with the locked default components :
    /// `MarkdownCascadeChunker` + `DefaultLinker` + the embedder
    /// supplied by the caller (real candle or `MockEmbedder` in tests).
    pub fn with_defaults(store: Arc<RagStore>, embedder: Arc<dyn Embedder>) -> Self {
        Self {
            store,
            embedder,
            chunker: Arc::new(MarkdownCascadeChunker::default()),
            linker: Arc::new(DefaultLinker::new()),
        }
    }

    /// Plug-in constructor for advanced wiring (custom chunker /
    /// linker — e.g. an `@chunk` in-source extractor down the line).
    pub fn new(
        store: Arc<RagStore>,
        embedder: Arc<dyn Embedder>,
        chunker: Arc<dyn Chunker>,
        linker: Arc<dyn Linker>,
    ) -> Self {
        Self {
            store,
            embedder,
            chunker,
            linker,
        }
    }

    pub fn store(&self) -> &RagStore {
        &self.store
    }

    /// Cheap clone of the store handle for sharing across daemons.
    pub fn store_arc(&self) -> Arc<RagStore> {
        Arc::clone(&self.store)
    }

    /// Cheap clone of the embedder handle for sharing across daemons.
    /// `StandardocMcp::with_embedder` consumes this to wire the optional
    /// `get_context(fqdn, query?)` re-rank path.
    pub fn embedder_arc(&self) -> Arc<dyn Embedder> {
        Arc::clone(&self.embedder)
    }

    /// Indexes a single prose source file end-to-end. Re-runs are
    /// idempotent : `replace_chunks_for_source` deletes the existing
    /// rows before re-inserting, and the BLAKE3 skip-check
    /// short-circuits unchanged files.
    ///
    /// `source_path` is workspace-relative, forward-slash separated.
    /// `handle` provides the symbol graph needed by the linker.
    pub fn run_for_source(
        &self,
        workspace_root: &Path,
        source_path: &str,
        handle: &IndexHandle,
    ) -> Result<Vec<ChunkId>, RagPipelineError> {
        let abs_path = workspace_root.join(source_path);
        let source_text =
            std::fs::read_to_string(&abs_path).map_err(|source| RagPipelineError::Io {
                path: source_path.to_string(),
                source,
            })?;
        let pieces = self.chunker.chunk(&source_text)?;

        if pieces.is_empty() {
            self.store
                .replace_chunks_for_source(source_path, &[], &[])?;
            return Ok(Vec::new());
        }

        let new_hashes: Vec<String> = pieces
            .iter()
            .map(|p| blake3::hash(p.text.as_bytes()).to_hex().to_string())
            .collect();
        let stored_hashes = self.store.hashes_for_source(source_path)?;
        if hashes_match(&new_hashes, &stored_hashes) {
            return Ok(Vec::new());
        }

        let texts: Vec<&str> = pieces.iter().map(|p| p.text.as_str()).collect();
        let vectors = self.embedder.embed_batch(&texts)?;
        let ids = self
            .store
            .replace_chunks_for_source(source_path, &pieces, &vectors)?;

        let frontmatter_raw = extract_frontmatter_block(&source_text);
        let chunk_refs: Vec<(ChunkId, &str)> =
            ids.iter().copied().zip(texts.iter().copied()).collect();
        let input = LinkInput {
            source_path,
            frontmatter_raw,
            chunks: &chunk_refs,
        };
        let lookup = CoreSymbolLookup::new(handle);
        let all_links = self.linker.link(&input, &lookup)?;

        for chunk_id in &ids {
            let bucket: Vec<_> = all_links
                .iter()
                .filter(|l| l.chunk_id == *chunk_id)
                .cloned()
                .collect();
            self.store.replace_links_for_chunk(*chunk_id, &bucket)?;
        }

        Ok(ids)
    }

    /// Full-workspace sweep — discovery + per-source indexing + purge
    /// of orphaned `source_path`s. Returns the list of paths actually
    /// processed (skipped by hash-match files are excluded from the
    /// return, but `purge_orphan_sources` runs over the full discovery
    /// set so it does not double-delete them).
    pub fn run_cold_start(
        &self,
        workspace_root: &Path,
        filters: &ScanFilters,
        handle: &IndexHandle,
    ) -> Result<Vec<String>, RagPipelineError> {
        let sources = discover_prose_sources(workspace_root, filters);
        let mut processed = Vec::new();
        for source in &sources {
            let ids = self.run_for_source(workspace_root, source, handle)?;
            if !ids.is_empty() {
                processed.push(source.clone());
            }
        }
        self.store.purge_orphan_sources(&sources)?;
        Ok(processed)
    }
}

fn hashes_match(a: &[String], b: &[String]) -> bool {
    a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x == y)
}

impl RagPipeline {
    /// Recomputes `chunk_symbol_links` for `source_path` against the
    /// current workspace fqdn set. Preserves existing chunk ids,
    /// embeddings, and prose text — the chunker and embedder are NOT
    /// invoked. Returns the number of chunks whose link set was
    /// rewritten.
    ///
    /// Frontmatter is re-read from disk to honour the explicit author
    /// directive signal. If the source file is missing (deleted .md
    /// that the cleanup pass hasn't caught yet), frontmatter is treated
    /// as absent and the linker still produces auto-name / auto-fqdn
    /// links from the stored chunk text.
    pub fn relink_for_source(
        &self,
        workspace_root: &Path,
        source_path: &str,
        handle: &IndexHandle,
    ) -> Result<usize, RagPipelineError> {
        let chunks = self.store.chunks_with_text_for_source(source_path)?;
        if chunks.is_empty() {
            return Ok(0);
        }
        let abs_path = workspace_root.join(source_path);
        let source_text = std::fs::read_to_string(&abs_path).ok();
        let frontmatter_raw = source_text.as_deref().and_then(extract_frontmatter_block);
        let chunk_refs: Vec<(ChunkId, &str)> =
            chunks.iter().map(|(id, t)| (*id, t.as_str())).collect();
        let input = LinkInput {
            source_path,
            frontmatter_raw,
            chunks: &chunk_refs,
        };
        let lookup = CoreSymbolLookup::new(handle);
        let all_links = self.linker.link(&input, &lookup)?;
        for (chunk_id, _) in &chunks {
            let bucket: Vec<_> = all_links
                .iter()
                .filter(|l| l.chunk_id == *chunk_id)
                .cloned()
                .collect();
            self.store.replace_links_for_chunk(*chunk_id, &bucket)?;
        }
        Ok(chunks.len())
    }

    /// Runs [`Self::relink_for_source`] for every prose source already
    /// stored in `rag.db`. Early-exits at near-zero cost when the
    /// workspace fqdn set is unchanged since the last successful sweep
    /// (BLAKE3 over the sorted fqdn list, stored as
    /// `schema_meta.workspace_fqdns_hash`).
    ///
    /// Returns the number of chunks whose link set was rewritten — `0`
    /// when the early-exit fires. The hash is updated on every
    /// successful pass (including the first one observed, even if no
    /// chunks exist yet).
    pub fn relink_all(
        &self,
        workspace_root: &Path,
        handle: &IndexHandle,
    ) -> Result<usize, RagPipelineError> {
        let new_hash = compute_workspace_fqdns_hash(handle)?;
        if let Some(stored) = self.store.read_meta(META_WORKSPACE_FQDNS_HASH)?
            && stored == new_hash
        {
            return Ok(0);
        }
        let sources = self.store.distinct_source_paths()?;
        let mut total = 0usize;
        for source in &sources {
            total += self.relink_for_source(workspace_root, source, handle)?;
        }
        self.store
            .write_meta(META_WORKSPACE_FQDNS_HASH, &new_hash)?;
        Ok(total)
    }
}

fn compute_workspace_fqdns_hash(handle: &IndexHandle) -> Result<String, RagPipelineError> {
    let lookup = CoreSymbolLookup::new(handle);
    let mut fqdns = lookup.workspace_fqdns()?;
    fqdns.sort();
    let mut hasher = blake3::Hasher::new();
    for fqdn in &fqdns {
        hasher.update(fqdn.as_bytes());
        hasher.update(b"\n");
    }
    Ok(hasher.finalize().to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use standardoc_rag::embedder::MockEmbedder;
    use standardoc_rag::types::EmbedModel;
    use tempfile::TempDir;

    fn fresh_pipeline_with_handle() -> (TempDir, RagPipeline, IndexHandle) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(RagStore::open(dir.path(), EmbedModel::bge_small_en_v1_5()).unwrap());
        let embedder: Arc<dyn Embedder> = Arc::new(MockEmbedder::new());
        let pipeline = RagPipeline::with_defaults(store, embedder);
        let handle = IndexHandle::open(dir.path()).unwrap();
        (dir, pipeline, handle)
    }

    #[test]
    fn run_for_source_indexes_a_simple_markdown_file() {
        let (dir, pipeline, handle) = fresh_pipeline_with_handle();
        std::fs::write(dir.path().join("README.md"), "# Title\n\nbody prose\n").unwrap();
        let ids = pipeline
            .run_for_source(dir.path(), "README.md", &handle)
            .unwrap();
        assert!(!ids.is_empty());
        // Re-run should be a no-op via hash-skip.
        let again = pipeline
            .run_for_source(dir.path(), "README.md", &handle)
            .unwrap();
        assert!(again.is_empty(), "hash-skip must short-circuit re-runs");
    }

    #[test]
    fn run_for_source_empty_file_clears_chunks() {
        let (dir, pipeline, handle) = fresh_pipeline_with_handle();
        std::fs::write(dir.path().join("README.md"), "first version\n").unwrap();
        pipeline
            .run_for_source(dir.path(), "README.md", &handle)
            .unwrap();
        std::fs::write(dir.path().join("README.md"), "").unwrap();
        let ids = pipeline
            .run_for_source(dir.path(), "README.md", &handle)
            .unwrap();
        assert!(ids.is_empty());
        let hashes = pipeline.store().hashes_for_source("README.md").unwrap();
        assert!(hashes.is_empty());
    }

    #[test]
    fn relink_all_seeds_workspace_fqdns_hash_on_empty_index() {
        let (dir, pipeline, handle) = fresh_pipeline_with_handle();
        let before = pipeline
            .store()
            .read_meta(META_WORKSPACE_FQDNS_HASH)
            .unwrap();
        assert!(before.is_none(), "hash must be absent before first sweep");
        let touched = pipeline.relink_all(dir.path(), &handle).unwrap();
        assert_eq!(touched, 0, "empty store + empty graph touches nothing");
        let after = pipeline
            .store()
            .read_meta(META_WORKSPACE_FQDNS_HASH)
            .unwrap();
        assert!(after.is_some(), "hash must be set after a successful sweep");
    }

    #[test]
    fn relink_all_early_exits_when_workspace_fqdns_hash_matches() {
        let (dir, pipeline, handle) = fresh_pipeline_with_handle();
        std::fs::write(dir.path().join("README.md"), "# r\n\nbody prose\n").unwrap();
        pipeline
            .run_for_source(dir.path(), "README.md", &handle)
            .unwrap();
        // Seed the hash.
        pipeline.relink_all(dir.path(), &handle).unwrap();
        let hash_after_first = pipeline
            .store()
            .read_meta(META_WORKSPACE_FQDNS_HASH)
            .unwrap()
            .unwrap();
        // Second sweep should early-exit, returning 0 untouched chunks.
        let touched = pipeline.relink_all(dir.path(), &handle).unwrap();
        assert_eq!(touched, 0, "early-exit must report zero relinked chunks");
        let hash_after_second = pipeline
            .store()
            .read_meta(META_WORKSPACE_FQDNS_HASH)
            .unwrap()
            .unwrap();
        assert_eq!(
            hash_after_first, hash_after_second,
            "hash must not be rewritten on a no-op pass",
        );
    }

    #[test]
    fn relink_for_source_returns_zero_for_unknown_source() {
        let (dir, pipeline, handle) = fresh_pipeline_with_handle();
        let touched = pipeline
            .relink_for_source(dir.path(), "does/not/exist.md", &handle)
            .unwrap();
        assert_eq!(touched, 0);
    }

    #[test]
    fn relink_for_source_preserves_chunk_ids_and_applies_frontmatter_links() {
        let (dir, pipeline, handle) = fresh_pipeline_with_handle();
        let body = "---\nsymbols: [myproj::Foo]\n---\n\n# Title\n\nbody prose mentioning Foo.\n";
        std::fs::write(dir.path().join("README.md"), body).unwrap();

        let ids_before = pipeline
            .run_for_source(dir.path(), "README.md", &handle)
            .unwrap();
        assert!(!ids_before.is_empty());

        let hashes_before = pipeline.store().hashes_for_source("README.md").unwrap();
        let touched = pipeline
            .relink_for_source(dir.path(), "README.md", &handle)
            .unwrap();
        assert_eq!(touched, ids_before.len());

        let hashes_after = pipeline.store().hashes_for_source("README.md").unwrap();
        assert_eq!(
            hashes_before, hashes_after,
            "relink must NOT touch chunk text — embeddings and hashes are preserved",
        );

        // The frontmatter directive should produce a link on the chunk
        // that carries it (the first one).
        let refs = pipeline.store().refs_for_symbol("myproj::Foo", 10).unwrap();
        assert!(
            !refs.is_empty(),
            "frontmatter directive must materialise as a chunk_symbol_link",
        );
    }

    #[test]
    fn cold_start_processes_all_convention_paths_and_purges_orphans() {
        let (dir, pipeline, handle) = fresh_pipeline_with_handle();
        std::fs::write(dir.path().join("README.md"), "# r\n\nrows.\n").unwrap();
        std::fs::create_dir_all(dir.path().join("docs")).unwrap();
        std::fs::write(dir.path().join("docs/a.md"), "# a\n\nrows.\n").unwrap();
        let filters = ScanFilters::from_stack(crate::pipeline::GitignoreStack::build(dir.path()));
        let processed = pipeline
            .run_cold_start(dir.path(), &filters, &handle)
            .unwrap();
        assert!(processed.contains(&"README.md".to_string()));
        assert!(processed.contains(&"docs/a.md".to_string()));

        // Add a third file, re-run : only the new one shows up as
        // "processed" (the first two are hash-skipped).
        std::fs::write(dir.path().join("docs/b.md"), "# b\n\nrows.\n").unwrap();
        let processed = pipeline
            .run_cold_start(dir.path(), &filters, &handle)
            .unwrap();
        assert_eq!(processed, vec!["docs/b.md".to_string()]);

        // Delete the third file, re-run : it must be purged from the store.
        std::fs::remove_file(dir.path().join("docs/b.md")).unwrap();
        pipeline
            .run_cold_start(dir.path(), &filters, &handle)
            .unwrap();
        let stored = pipeline.store().hashes_for_source("docs/b.md").unwrap();
        assert!(stored.is_empty(), "orphan must be purged");
    }
}

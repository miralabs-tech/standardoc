//! Markdown chunker — cascade adaptative locked at design time :
//!
//! 1. If the file fits in `max_tokens` → 1 chunk for the whole file.
//! 2. Else split on `## H2` boundaries. Each section ≤ `max_tokens` → 1 chunk.
//! 3. A section longer than `max_tokens` splits on `### H3`.
//! 4. A sub-section longer than `max_tokens` splits on blank-line
//!    paragraphs ; consecutive paragraphs are greedily packed until the
//!    pack would exceed `max_tokens`.
//! 5. A paragraph longer than `max_tokens` (rare, pathological prose)
//!    falls back to a sliding window of `max_tokens` / `overlap` tokens.
//!
//! The chunker is **deterministic** — same input → same chunks, same
//! `chunk_idx`, same `text_hash` — so the watcher's hash-skip logic can
//! short-circuit re-embeds when only unrelated bytes moved in the file.
//!
//! Token counting is approximate (whitespace-split) ; swapping in the BGE
//! tokenizer for boundary accuracy is a follow-up.

use crate::error::RagError;
use crate::markdown::{Section, sliding_token_windows, split_by_heading, split_paragraphs};

/// Intermediate type produced by the chunker, pre-persistence. The store
/// hashes `text` to BLAKE3 hex and assigns `chunk_idx` from the order in
/// the returned `Vec`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkPiece {
    pub text: String,
    pub section_header: Option<String>,
    pub byte_start: u32,
    pub byte_end: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct ChunkerConfig {
    pub max_tokens: u32,
    pub sliding_overlap: u32,
}

impl Default for ChunkerConfig {
    fn default() -> Self {
        Self {
            max_tokens: crate::schema::CHUNKER_MAX_TOKENS_DEFAULT,
            sliding_overlap: crate::schema::CHUNKER_SLIDING_OVERLAP_DEFAULT,
        }
    }
}

/// Trait kept narrow on purpose so a future `@chunk` in-source extractor or
/// a JSDoc-attached chunker can plug in without renaming the persistence
/// path.
pub trait Chunker: Send + Sync {
    /// Returns chunks in deterministic file order. `source_text` is the raw
    /// file bytes (typically `.md`) ; the chunker does not read frontmatter
    /// (the linker does — see `linker::extract_frontmatter_symbols`).
    fn chunk(&self, source_text: &str) -> Result<Vec<ChunkPiece>, RagError>;
}

/// Cascade markdown chunker, the only impl shipped day-1.
pub struct MarkdownCascadeChunker {
    pub cfg: ChunkerConfig,
}

impl MarkdownCascadeChunker {
    pub const fn new(cfg: ChunkerConfig) -> Self {
        Self { cfg }
    }
}

impl Default for MarkdownCascadeChunker {
    fn default() -> Self {
        Self::new(ChunkerConfig::default())
    }
}

impl Chunker for MarkdownCascadeChunker {
    fn chunk(&self, source_text: &str) -> Result<Vec<ChunkPiece>, RagError> {
        if source_text.is_empty() {
            return Ok(Vec::new());
        }
        if approx_token_count(source_text) <= self.cfg.max_tokens {
            return Ok(vec![ChunkPiece {
                text: source_text.to_string(),
                section_header: None,
                byte_start: 0,
                byte_end: u32_from(source_text.len()),
            }]);
        }
        let mut out = Vec::new();
        for section in split_by_heading(source_text, 2) {
            self.chunk_section(source_text, &section, None, &mut out);
        }
        Ok(out)
    }
}

impl MarkdownCascadeChunker {
    /// Handles a single H2 section : emit as one chunk if it fits ;
    /// otherwise cascade to H3 sub-sections.
    fn chunk_section(
        &self,
        source: &str,
        section: &Section<'_>,
        parent_header: Option<&str>,
        out: &mut Vec<ChunkPiece>,
    ) {
        let header = section.header.or(parent_header);
        if approx_token_count(section.body) <= self.cfg.max_tokens {
            out.push(piece_from_section(section, header));
            return;
        }
        let sub_sections = split_by_heading(section.body, 3);
        if sub_sections.len() > 1 || sub_sections.iter().any(|s| s.header.is_some()) {
            for sub in &sub_sections {
                let abs = Section {
                    header: sub.header,
                    body: sub.body,
                    byte_start: section.byte_start + sub.byte_start,
                    byte_end: section.byte_start + sub.byte_end,
                };
                self.chunk_subsection(source, &abs, header, out);
            }
            return;
        }
        self.chunk_paragraphs(source, section, header, out);
    }

    /// Handles a single H3 sub-section : emit if it fits, else paragraphs.
    fn chunk_subsection(
        &self,
        source: &str,
        section: &Section<'_>,
        parent_header: Option<&str>,
        out: &mut Vec<ChunkPiece>,
    ) {
        let header = section.header.or(parent_header);
        if approx_token_count(section.body) <= self.cfg.max_tokens {
            out.push(piece_from_section(section, header));
            return;
        }
        self.chunk_paragraphs(source, section, header, out);
    }

    /// Pack blank-line-delimited paragraphs greedily until the pack would
    /// exceed `max_tokens`. A single oversized paragraph falls back to
    /// sliding token windows.
    fn chunk_paragraphs(
        &self,
        source: &str,
        section: &Section<'_>,
        header: Option<&str>,
        out: &mut Vec<ChunkPiece>,
    ) {
        let paragraphs = split_paragraphs(section.body, section.byte_start);
        if paragraphs.is_empty() {
            // No prose, just headings whitespace — drop.
            return;
        }

        let mut current_start: Option<usize> = None;
        let mut current_end = 0usize;
        let mut current_tokens = 0u32;

        for &(p_start, p_end) in &paragraphs {
            let p_text = &source[p_start..p_end];
            let p_tokens = approx_token_count(p_text);

            if p_tokens > self.cfg.max_tokens {
                if let Some(start) = current_start.take() {
                    out.push(piece_from_range(source, start, current_end, header));
                    current_tokens = 0;
                }
                for (w_start, w_end) in sliding_token_windows(
                    p_text,
                    p_start,
                    self.cfg.max_tokens,
                    self.cfg.sliding_overlap,
                ) {
                    out.push(piece_from_range(source, w_start, w_end, header));
                }
                continue;
            }

            if current_tokens + p_tokens > self.cfg.max_tokens && current_start.is_some() {
                let start = current_start.take().unwrap_or(p_start);
                out.push(piece_from_range(source, start, current_end, header));
                current_tokens = 0;
            }
            if current_start.is_none() {
                current_start = Some(p_start);
            }
            current_end = p_end;
            current_tokens += p_tokens;
        }

        if let Some(start) = current_start {
            out.push(piece_from_range(source, start, current_end, header));
        }
    }
}

fn piece_from_section(section: &Section<'_>, header: Option<&str>) -> ChunkPiece {
    ChunkPiece {
        text: section.body.to_string(),
        section_header: header.map(str::to_string),
        byte_start: u32_from(section.byte_start),
        byte_end: u32_from(section.byte_end),
    }
}

fn piece_from_range(source: &str, start: usize, end: usize, header: Option<&str>) -> ChunkPiece {
    ChunkPiece {
        text: source[start..end].to_string(),
        section_header: header.map(str::to_string),
        byte_start: u32_from(start),
        byte_end: u32_from(end),
    }
}

fn u32_from(v: usize) -> u32 {
    u32::try_from(v).unwrap_or(u32::MAX)
}

/// Approximate token count — whitespace-split, no BPE. Good enough for
/// boundary decisions ; a future tokenizer-aware pass swaps this for
/// accuracy near the `max_tokens` threshold.
pub fn approx_token_count(s: &str) -> u32 {
    u32::try_from(s.split_whitespace().count()).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_chunker() -> MarkdownCascadeChunker {
        MarkdownCascadeChunker::new(ChunkerConfig {
            max_tokens: 8,
            sliding_overlap: 2,
        })
    }

    #[test]
    fn approx_token_count_basic() {
        assert_eq!(approx_token_count(""), 0);
        assert_eq!(approx_token_count("hello"), 1);
        assert_eq!(approx_token_count("hello   world\nfoo\tbar"), 4);
    }

    #[test]
    fn default_config_matches_schema_defaults() {
        let cfg = ChunkerConfig::default();
        assert_eq!(cfg.max_tokens, 512);
        assert_eq!(cfg.sliding_overlap, 64);
    }

    #[test]
    fn empty_source_yields_no_chunks() {
        let chunks = tiny_chunker().chunk("").unwrap();
        assert!(chunks.is_empty());
    }

    #[test]
    fn small_file_emits_single_chunk() {
        let src = "one two three four";
        let chunks = MarkdownCascadeChunker::default().chunk(src).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, src);
        assert_eq!(chunks[0].section_header, None);
        assert_eq!(chunks[0].byte_start, 0);
        assert_eq!(chunks[0].byte_end, u32::try_from(src.len()).unwrap());
    }

    #[test]
    fn two_h2_sections_yield_two_chunks() {
        let src = "## Alpha\nshort body\n## Beta\nother body\n";
        // Each section is short enough as a whole.
        let chunker = MarkdownCascadeChunker::new(ChunkerConfig {
            max_tokens: 64,
            sliding_overlap: 8,
        });
        let chunks = chunker.chunk(src).unwrap();
        // Whole file fits in 64 tokens → stage 1 emits one chunk only.
        assert_eq!(chunks.len(), 1);
        // Force stage 2 by lowering the cap below the whole-file token count.
        let chunker = MarkdownCascadeChunker::new(ChunkerConfig {
            max_tokens: 4,
            sliding_overlap: 1,
        });
        let chunks = chunker.chunk(src).unwrap();
        assert!(chunks.len() >= 2);
        let headers: Vec<_> = chunks
            .iter()
            .filter_map(|c| c.section_header.clone())
            .collect();
        assert!(headers.iter().any(|h| h == "Alpha"));
        assert!(headers.iter().any(|h| h == "Beta"));
    }

    #[test]
    fn long_section_falls_through_to_h3() {
        // Each H3 sub-section under the cap, parent section over it.
        let src = "## Outer\n### Sub-A\na b c d\n### Sub-B\ne f g h\n";
        let chunker = tiny_chunker();
        let chunks = chunker.chunk(src).unwrap();
        let headers: Vec<_> = chunks
            .iter()
            .filter_map(|c| c.section_header.clone())
            .collect();
        assert!(headers.iter().any(|h| h == "Sub-A"));
        assert!(headers.iter().any(|h| h == "Sub-B"));
    }

    #[test]
    fn long_paragraph_triggers_sliding_window() {
        let long = "alpha bravo charlie delta echo foxtrot golf hotel india juliet";
        let src = format!("## H\n{long}\n");
        let chunker = MarkdownCascadeChunker::new(ChunkerConfig {
            max_tokens: 4,
            sliding_overlap: 1,
        });
        let chunks = chunker.chunk(&src).unwrap();
        // 10 tokens, max=4, step=3 → windows at 0..4, 3..7, 6..10.
        assert!(chunks.len() >= 3);
        for c in &chunks {
            assert_eq!(c.section_header.as_deref(), Some("H"));
        }
    }

    #[test]
    fn paragraph_pack_groups_short_paragraphs() {
        // Each paragraph is 1 token. Cap = 3 → pack groups of 3.
        let src = "## H\n\nalpha\n\nbravo\n\ncharlie\n\ndelta\n\necho\n";
        let chunker = MarkdownCascadeChunker::new(ChunkerConfig {
            max_tokens: 3,
            sliding_overlap: 0,
        });
        let chunks = chunker.chunk(src).unwrap();
        // After "## H\n" + 5 paragraphs of 1 token each.
        // Header line + paragraph pack — splits when reaching cap.
        assert!(chunks.len() >= 2);
        for c in &chunks {
            assert_eq!(c.section_header.as_deref(), Some("H"));
        }
    }

    #[test]
    fn byte_offsets_are_contiguous_and_cover_emitted_text() {
        let src = "## A\nalpha bravo charlie\n## B\ndelta echo foxtrot\n";
        let chunks = tiny_chunker().chunk(src).unwrap();
        for c in &chunks {
            let slice = &src[c.byte_start as usize..c.byte_end as usize];
            assert_eq!(slice, c.text);
        }
    }

    #[test]
    fn determinism_same_input_same_output() {
        let src = "## A\nalpha bravo charlie\n## B\ndelta echo foxtrot\n";
        let a = tiny_chunker().chunk(src).unwrap();
        let b = tiny_chunker().chunk(src).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn preamble_then_h2_produces_distinct_chunks() {
        let src = "preamble paragraph here\n## Alpha\nbody one\n## Beta\nbody two\n";
        let chunks = tiny_chunker().chunk(src).unwrap();
        // Preamble has no header.
        assert!(
            chunks
                .iter()
                .any(|c| c.section_header.is_none() && c.text.contains("preamble"))
        );
        assert!(
            chunks
                .iter()
                .any(|c| c.section_header.as_deref() == Some("Alpha"))
        );
        assert!(
            chunks
                .iter()
                .any(|c| c.section_header.as_deref() == Some("Beta"))
        );
    }
}

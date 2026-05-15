//! Pure scoring functions for chunk relevance. No I/O, no DB — just maths.
//!
//! Three signals compose into the `confidence` value stored in
//! `chunk_symbol_links` and returned in `ChunkRef.confidence` :
//!
//! - `link_confidence` (base) : how the link was created — frontmatter
//!   (`1.0`), auto-fqdn-exact (`0.7`), auto-name-substring (`0.4`).
//!   See `LinkSource::base_confidence`.
//! - `def_site_boost` (multiplier) : `×1.5` if the chunk's source file
//!   lives in the same directory tree as the symbol's def-site,
//!   `×1.0` otherwise. Reflects the user's intuition "the doc co-located
//!   with the definition is more authoritative than the doc that just
//!   mentions the name from afar".
//! - `query_score` (optional) : cosine similarity ∈ `[0, 1]` between a
//!   user query embedding and the chunk embedding, computed at query
//!   time when the caller passes `query` to `get_context`.
//!
//! Composition (locked) :
//!
//! - No query : `final = min(1.0, link × boost)`
//! - With query : `final = 0.5 × min(1.0, link × boost) + 0.5 × query_score`

use crate::types::LinkSource;

pub const DEF_SITE_BOOST: f32 = 1.5;
pub const NO_BOOST: f32 = 1.0;
pub const FINAL_CONFIDENCE_CAP: f32 = 1.0;
pub const QUERY_BLEND_PRE: f32 = 0.5;
pub const QUERY_BLEND_QUERY: f32 = 0.5;

/// Base link confidence (no def-site boost, no query). Pure shorthand
/// over `LinkSource::base_confidence`.
#[inline]
pub const fn compute_link_confidence(source: LinkSource) -> f32 {
    source.base_confidence()
}

/// Decides whether the def-site boost applies given a chunk's source path
/// and the symbol's def-site path (both workspace-relative). Day-1 rule :
/// the boost applies when the chunk's parent dir is a prefix of the
/// def-site's parent dir, **or** vice versa. Mirrors how authors typically
/// co-locate `docs/auth/login.md` with `src/auth/login.rs`.
pub fn applies_def_site_boost(chunk_path: &str, def_site_path: Option<&str>) -> bool {
    let Some(def) = def_site_path else {
        return false;
    };
    let Some(chunk_dir) = parent_dir(chunk_path) else {
        return false;
    };
    let Some(def_dir) = parent_dir(def) else {
        return false;
    };
    // Tolerate the common `docs/` mirror : `docs/auth/...` boosts on
    // `src/auth/...` when the tail directories match. Done by comparing
    // the suffix after the first segment.
    if chunk_dir == def_dir {
        return true;
    }
    let chunk_tail = strip_first_segment(chunk_dir);
    let def_tail = strip_first_segment(def_dir);
    !chunk_tail.is_empty() && chunk_tail == def_tail
}

fn parent_dir(path: &str) -> Option<&str> {
    let trimmed = path.trim_end_matches('/');
    trimmed.rsplit_once('/').map(|(parent, _)| parent)
}

fn strip_first_segment(path: &str) -> &str {
    path.split_once('/').map_or("", |(_, rest)| rest)
}

/// Combines the base link confidence with the def-site multiplier,
/// capping at 1.0. Stored as `chunk_symbol_links.confidence`.
#[inline]
pub fn apply_def_site_boost(link_confidence: f32, applies: bool) -> f32 {
    let raw = link_confidence * if applies { DEF_SITE_BOOST } else { NO_BOOST };
    raw.min(FINAL_CONFIDENCE_CAP)
}

/// Blends the pre-computed (link × boost) confidence with a query-time
/// cosine similarity. Returns a value in `[0, 1]`. Used only when the
/// caller passes a `query` to `get_context`.
#[inline]
pub fn blend_with_query(precomputed: f32, query_score: f32) -> f32 {
    let pre = precomputed.clamp(0.0, FINAL_CONFIDENCE_CAP);
    let q = query_score.clamp(0.0, FINAL_CONFIDENCE_CAP);
    QUERY_BLEND_PRE.mul_add(pre, QUERY_BLEND_QUERY * q)
}

/// Cosine similarity between two equal-length vectors. Returns `0.0` when
/// either vector has zero norm. Both vectors are expected to be already
/// L2-normalised (the embedder normalises), in which case this reduces
/// to a dot product — but the implementation tolerates non-normalised
/// inputs for safety.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    let denom = (na.sqrt()) * (nb.sqrt());
    if denom == 0.0 { 0.0 } else { dot / denom }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_link_confidence_matches_source_table() {
        assert!((compute_link_confidence(LinkSource::Frontmatter) - 1.0).abs() < 1e-6);
        assert!((compute_link_confidence(LinkSource::AutoFqdnExact) - 0.7).abs() < 1e-6);
        assert!((compute_link_confidence(LinkSource::AutoNameSubstring) - 0.4).abs() < 1e-6);
    }

    #[test]
    fn apply_def_site_boost_caps_at_one() {
        assert!((apply_def_site_boost(1.0, true) - 1.0).abs() < 1e-6);
        assert!((apply_def_site_boost(0.7, true) - 1.0).abs() < 1e-6); // 0.7 × 1.5 = 1.05 → 1.0
        assert!((apply_def_site_boost(0.4, true) - 0.6).abs() < 1e-6);
        assert!((apply_def_site_boost(0.4, false) - 0.4).abs() < 1e-6);
    }

    #[test]
    fn blend_with_query_is_a_50_50_mix() {
        assert!((blend_with_query(1.0, 0.0) - 0.5).abs() < 1e-6);
        assert!((blend_with_query(0.0, 1.0) - 0.5).abs() < 1e-6);
        assert!((blend_with_query(0.8, 0.4) - 0.6).abs() < 1e-6);
    }

    #[test]
    fn applies_def_site_boost_same_directory() {
        assert!(applies_def_site_boost(
            "docs/auth/login.md",
            Some("docs/auth/notes.md"),
        ));
    }

    #[test]
    fn applies_def_site_boost_mirrored_docs_to_src() {
        assert!(applies_def_site_boost(
            "docs/auth/login.md",
            Some("src/auth/login.rs"),
        ));
    }

    #[test]
    fn applies_def_site_boost_unrelated_paths() {
        assert!(!applies_def_site_boost(
            "docs/billing/invoice.md",
            Some("src/auth/login.rs"),
        ));
    }

    #[test]
    fn applies_def_site_boost_handles_missing_def_site() {
        assert!(!applies_def_site_boost("docs/auth/login.md", None));
    }

    #[test]
    fn cosine_similarity_orthogonal_vectors() {
        let a = [1.0f32, 0.0];
        let b = [0.0f32, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_identical_vectors() {
        let a = [0.6f32, 0.8];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_mismatched_dimensions_returns_zero() {
        let a = [1.0f32, 0.0];
        let b = [1.0f32, 0.0, 0.0];
        assert!(cosine_similarity(&a, &b).abs() < f32::EPSILON);
    }
}

//! Markdown parsing helpers used by `MarkdownCascadeChunker`.
//!
//! Three primitives, all byte-offset-preserving so the chunker can record
//! exact `byte_start` / `byte_end` of every chunk it emits :
//!
//! - [`split_by_heading`] — splits the source on `## H2` (or `### H3`)
//!   boundaries, returning a `Vec<Section<'_>>`. Preamble text before the
//!   first heading shows up as a `Section { header: None, .. }`.
//! - [`split_paragraphs`] — splits a piece of text on blank-line
//!   boundaries, returning byte ranges relative to the original source.
//! - [`sliding_token_windows`] — last-resort fallback for paragraphs that
//!   exceed the token cap, sliding by `(max - overlap)` tokens.
//!
//! Token counting is whitespace-split (`approx_token_count` in
//! `chunker.rs`) — Phase B's first pass intentionally stays
//! tokenizer-free. Swapping in the BGE tokenizer for boundary accuracy
//! is a follow-up.

/// One heading-delimited region of a markdown source.
///
/// `byte_start` / `byte_end` index the original source (NOT the section's
/// own buffer), so chunks emitted from a section already carry workspace
/// coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section<'a> {
    /// Text of the heading line (sans the leading `## ` / `### `, sans
    /// trailing newline). `None` for the preamble before the first
    /// heading.
    pub header: Option<&'a str>,
    /// The section body including the heading line itself when present.
    pub body: &'a str,
    pub byte_start: usize,
    pub byte_end: usize,
}

/// Splits `source` on lines starting with exactly `level` `#` chars
/// followed by a space. `level=2` → H2, `level=3` → H3. Lines like `####`
/// (level 4) are NOT recognised as boundaries — they belong to the
/// surrounding section's body.
///
/// If `source` contains no matching heading, returns a single section
/// covering the whole document with `header = None`.
pub fn split_by_heading(source: &str, level: usize) -> Vec<Section<'_>> {
    if source.is_empty() {
        return Vec::new();
    }
    let prefix_owned = format!("{} ", "#".repeat(level));
    let prefix = prefix_owned.as_str();
    let mut sections = Vec::new();
    let mut current_start = 0usize;
    let mut current_header: Option<&str> = None;
    let mut byte_pos = 0usize;

    for line in source.split_inclusive('\n') {
        if line_starts_with_heading(line, prefix) && !line_starts_with_heading(line, &format!("{prefix}#")) {
            if byte_pos > current_start || current_header.is_some() {
                push_if_meaningful(
                    &mut sections,
                    source,
                    current_header,
                    current_start,
                    byte_pos,
                );
            }
            current_start = byte_pos;
            current_header = Some(extract_header_text(line, prefix));
        }
        byte_pos += line.len();
    }

    if byte_pos > current_start {
        push_if_meaningful(
            &mut sections,
            source,
            current_header,
            current_start,
            byte_pos,
        );
    }

    if sections.is_empty() {
        sections.push(Section {
            header: None,
            body: source,
            byte_start: 0,
            byte_end: source.len(),
        });
    }

    sections
}

fn line_starts_with_heading(line: &str, prefix: &str) -> bool {
    line.starts_with(prefix)
}

fn extract_header_text<'a>(line: &'a str, prefix: &str) -> &'a str {
    let after = &line[prefix.len()..];
    after.trim_end_matches('\n').trim_end_matches('\r')
}

fn push_if_meaningful<'a>(
    sections: &mut Vec<Section<'a>>,
    source: &'a str,
    header: Option<&'a str>,
    start: usize,
    end: usize,
) {
    let slice = &source[start..end];
    if slice.trim().is_empty() && header.is_none() {
        return;
    }
    sections.push(Section {
        header,
        body: slice,
        byte_start: start,
        byte_end: end,
    });
}

/// Splits `text` on blank-line boundaries. Returns byte ranges in the
/// **original** source by adding `offset` to every position.
///
/// Consecutive blank lines collapse into a single separator. Leading /
/// trailing blank lines are ignored.
pub fn split_paragraphs(text: &str, offset: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut byte_pos = 0usize;
    let mut current_start: Option<usize> = None;

    for line in text.split_inclusive('\n') {
        let is_blank = line.trim().is_empty();
        if is_blank {
            if let Some(start) = current_start.take() {
                out.push((offset + start, offset + byte_pos));
            }
        } else if current_start.is_none() {
            current_start = Some(byte_pos);
        }
        byte_pos += line.len();
    }
    if let Some(start) = current_start {
        out.push((offset + start, offset + byte_pos));
    }
    out
}

/// Sliding token windows over `text`. Walks the whitespace-tokenised view
/// in steps of `(max - overlap)` and emits `(byte_start, byte_end)`
/// ranges anchored at the source's coordinate system (via `offset`).
///
/// Edge cases :
/// - `text` empty → empty vec.
/// - Fewer tokens than `max` → one window covering everything.
/// - `overlap >= max` → step clamps to 1 (still progresses).
pub fn sliding_token_windows(
    text: &str,
    offset: usize,
    max: u32,
    overlap: u32,
) -> Vec<(usize, usize)> {
    let positions = token_positions(text);
    if positions.is_empty() {
        return Vec::new();
    }
    let max = usize::try_from(max).unwrap_or(usize::MAX).max(1);
    let overlap = usize::try_from(overlap).unwrap_or(usize::MAX);
    let step = max.saturating_sub(overlap).max(1);
    let n = positions.len();

    let mut out = Vec::new();
    let mut i = 0usize;
    loop {
        let end = (i + max).min(n);
        let start_byte = positions[i].0;
        let end_byte = positions[end - 1].1;
        out.push((offset + start_byte, offset + end_byte));
        if end == n {
            break;
        }
        i += step;
        if i >= n {
            break;
        }
    }
    out
}

fn token_positions(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        out.push((start, i));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_by_heading_no_heading_yields_single_section() {
        let src = "plain prose\nwithout any heading\n";
        let s = split_by_heading(src, 2);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].header, None);
        assert_eq!(s[0].byte_start, 0);
        assert_eq!(s[0].byte_end, src.len());
    }

    #[test]
    fn split_by_heading_two_h2_sections() {
        let src = "## First\nbody one\n## Second\nbody two\n";
        let s = split_by_heading(src, 2);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].header, Some("First"));
        assert_eq!(s[1].header, Some("Second"));
        assert_eq!(s[0].byte_start, 0);
        assert_eq!(s[1].byte_start, s[0].byte_end);
        assert_eq!(s[1].byte_end, src.len());
    }

    #[test]
    fn split_by_heading_preamble_before_first_h2() {
        let src = "preamble line\n## First\nbody\n";
        let s = split_by_heading(src, 2);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].header, None);
        assert_eq!(s[0].body, "preamble line\n");
        assert_eq!(s[1].header, Some("First"));
    }

    #[test]
    fn split_by_heading_does_not_match_h3_for_level_2() {
        let src = "## H2\nbody\n### H3 nested\nmore\n## H2-bis\nx\n";
        let s = split_by_heading(src, 2);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].header, Some("H2"));
        assert!(s[0].body.contains("### H3 nested"));
        assert_eq!(s[1].header, Some("H2-bis"));
    }

    #[test]
    fn split_by_heading_h3_picks_up_subsections() {
        let src = "## H2\npre\n### Sub\nbody\n### Sub2\nmore\n";
        let s = split_by_heading(src, 3);
        // preamble + 2 H3 sections
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].header, None);
        assert_eq!(s[1].header, Some("Sub"));
        assert_eq!(s[2].header, Some("Sub2"));
    }

    #[test]
    fn split_paragraphs_empty_text_returns_empty() {
        assert!(split_paragraphs("", 0).is_empty());
    }

    #[test]
    fn split_paragraphs_three_paragraphs() {
        let src = "para 1 line a\npara 1 line b\n\npara 2\n\n\npara 3\n";
        let paras = split_paragraphs(src, 0);
        assert_eq!(paras.len(), 3);
        for &(a, b) in &paras {
            assert!(!src[a..b].trim().is_empty());
        }
    }

    #[test]
    fn split_paragraphs_offset_is_applied() {
        let src = "x\n\ny\n";
        let paras = split_paragraphs(src, 100);
        assert!(paras.iter().all(|&(a, _)| a >= 100));
    }

    #[test]
    fn sliding_token_windows_short_text_yields_one_window() {
        let text = "one two three";
        let w = sliding_token_windows(text, 0, 10, 2);
        assert_eq!(w.len(), 1);
        assert_eq!(&text[w[0].0..w[0].1], "one two three");
    }

    #[test]
    fn sliding_token_windows_overlap_step() {
        let text = "a b c d e f g h i j";
        let w = sliding_token_windows(text, 0, 4, 1);
        // step = 3 ; tokens 0..4, 3..7, 6..10
        assert_eq!(w.len(), 3);
    }

    #[test]
    fn sliding_token_windows_empty_text() {
        assert!(sliding_token_windows("   \n\t  ", 0, 4, 1).is_empty());
    }
}

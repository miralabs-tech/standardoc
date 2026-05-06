//! Location / span utilities used by every provider when stamping the
//! file-spanning `Module` symbol or converting raw byte offsets back to
//! `Site { line, col }` coordinates.

use standardoc_ir::SymbolLocation;

/// Builds a `SymbolLocation` covering the entire file. Lines are
/// 1-indexed, columns 0-indexed (cohérent avec swc + syn).
pub(crate) fn file_span(path: &str, content: &str) -> SymbolLocation {
    let (end_line, end_col) = content_extent(content);
    SymbolLocation {
        file: path.into(),
        start_line: 1,
        end_line,
        start_col: 0,
        end_col,
    }
}

/// Returns `(line_count, last_line_byte_len)` for `content`. Empty input
/// returns `(1, 0)` — every file has at least line 1, even when empty,
/// matching the convention of the AST parsers used by every provider.
pub(crate) fn content_extent(content: &str) -> (u32, u32) {
    if content.is_empty() {
        return (1, 0);
    }
    let line_count = u32::try_from(content.lines().count()).unwrap_or(u32::MAX);
    let last_col = content
        .lines()
        .last()
        .map_or(0, |l| u32::try_from(l.len()).unwrap_or(u32::MAX));
    (line_count, last_col)
}

/// Converts an absolute byte offset within `content` to a `(line, col)`
/// pair. Lines are 1-indexed, columns 0-indexed. Used by the SFC
/// orchestrator to materialise `Site { line, col }` from the byte
/// offsets emitted by the template parsers.
///
/// Out-of-range offsets are clamped to the end of the input — callers
/// shouldn't pass them but the function is safe regardless.
pub(crate) fn byte_offset_to_line_col(content: &str, offset: usize) -> (u32, u32) {
    let bytes = content.as_bytes();
    let end = offset.min(bytes.len());
    let mut line = 1u32;
    let mut col = 0u32;
    for &b in &bytes[..end] {
        if b == b'\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_content_extent_is_one_line_zero_col() {
        assert_eq!(content_extent(""), (1, 0));
    }

    #[test]
    fn single_line_content_extent() {
        assert_eq!(content_extent("hello"), (1, 5));
    }

    #[test]
    fn multi_line_content_extent_counts_lines_and_last_col() {
        assert_eq!(content_extent("hello\nworld"), (2, 5));
    }

    #[test]
    fn file_span_starts_at_line_one_col_zero() {
        let loc = file_span("src/lib.rs", "fn main() {}");
        assert_eq!(loc.file, "src/lib.rs");
        assert_eq!(loc.start_line, 1);
        assert_eq!(loc.start_col, 0);
        assert_eq!(loc.end_line, 1);
        assert_eq!(loc.end_col, 12);
    }

    #[test]
    fn byte_offset_to_line_col_first_line() {
        assert_eq!(byte_offset_to_line_col("hello\nworld", 0), (1, 0));
        assert_eq!(byte_offset_to_line_col("hello\nworld", 3), (1, 3));
    }

    #[test]
    fn byte_offset_to_line_col_after_newline() {
        assert_eq!(byte_offset_to_line_col("hello\nworld", 6), (2, 0));
        assert_eq!(byte_offset_to_line_col("hello\nworld", 8), (2, 2));
    }

    #[test]
    fn byte_offset_to_line_col_clamps_overflow_to_end() {
        assert_eq!(byte_offset_to_line_col("ab", 100), (1, 2));
    }
}

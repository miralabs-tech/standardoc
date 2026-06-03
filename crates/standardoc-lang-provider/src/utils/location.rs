//! Location / span utilities used by every provider when stamping the
//! file-spanning `Module` symbol or converting raw byte offsets back to
//! `Site { line, col }` coordinates.

use standardoc_ir::SymbolLocation;

/// Builds a `SymbolLocation` covering the entire file. Lines are
/// 1-indexed, columns 0-indexed (consistent with swc + syn).
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

/// Returns `(line_count, last_line_utf16_len)` for `content`. Empty input
/// returns `(1, 0)` — every file has at least line 1, even when empty,
/// matching the convention of the AST parsers used by every provider.
///
/// The column is in UTF-16 code units (not bytes) so the file-spanning
/// `Module` symbol agrees with the per-symbol locations every provider
/// now stamps — see [`utf16_len`].
pub(crate) fn content_extent(content: &str) -> (u32, u32) {
    if content.is_empty() {
        return (1, 0);
    }
    let line_count = u32::try_from(content.lines().count()).unwrap_or(u32::MAX);
    let last_col = content.lines().last().map_or(0, utf16_len);
    (line_count, last_col)
}

/// UTF-16 code-unit length of `s`. This is the unit the LSP `Position.character`
/// and `vscode.Position.character` fields use (the default `positionEncoding`
/// is `utf-16`), so every provider stamps `SymbolLocation` columns in these
/// units and the navigation consumers can forward them unchanged.
pub(crate) fn utf16_len(s: &str) -> u32 {
    u32::try_from(s.encode_utf16().count()).unwrap_or(u32::MAX)
}

/// UTF-16 column of the position at `byte_offset` within `line`. `byte_offset`
/// is snapped down to the nearest char boundary `<= line.len()`, so a stray
/// offset can never panic on a multi-byte slice.
pub(crate) fn utf16_col(line: &str, byte_offset: usize) -> u32 {
    let mut end = byte_offset.min(line.len());
    while end > 0 && !line.is_char_boundary(end) {
        end -= 1;
    }
    utf16_len(&line[..end])
}

/// Converts an absolute `byte_offset` within `content` to a `(line, col)` pair
/// where `line` is 1-indexed and `col` is the 0-indexed UTF-16 column on that
/// line. Used by providers whose parser hands back absolute byte offsets
/// (e.g. full_moon's `Position::bytes`). `byte_offset` is snapped down to a
/// char boundary, so out-of-range / mid-codepoint offsets are safe.
pub(crate) fn line_and_utf16_col(content: &str, byte_offset: usize) -> (u32, u32) {
    let mut end = byte_offset.min(content.len());
    while end > 0 && !content.is_char_boundary(end) {
        end -= 1;
    }
    let before = &content[..end];
    let line =
        u32::try_from(before.bytes().filter(|&b| b == b'\n').count() + 1).unwrap_or(u32::MAX);
    let line_start = before.rfind('\n').map_or(0, |nl| nl + 1);
    (line, utf16_len(&content[line_start..end]))
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
    fn utf16_len_counts_code_units_not_bytes_or_chars() {
        assert_eq!(utf16_len("abc"), 3); // ASCII: byte == char == utf16
        assert_eq!(utf16_len("é"), 1); // 1 char, 2 bytes, 1 utf16 unit
        assert_eq!(utf16_len("中"), 1); // BMP CJK: 1 char, 3 bytes, 1 utf16 unit
        assert_eq!(utf16_len("😀"), 2); // astral: 1 char, 4 bytes, 2 utf16 units
        assert_eq!(utf16_len("a😀b"), 4);
    }

    #[test]
    fn utf16_col_measures_prefix_up_to_byte_offset() {
        // `é` is 2 bytes; the symbol after it starts at byte 2 → utf16 col 1.
        assert_eq!(utf16_col("éx", 2), 1);
        // Astral emoji is 4 bytes / 2 utf16 units; byte 4 → col 2.
        assert_eq!(utf16_col("😀x", 4), 2);
        // A leading tab is a single utf16 unit (unlike display columns).
        assert_eq!(utf16_col("\tfoo", 1), 1);
    }

    #[test]
    fn utf16_col_snaps_mid_codepoint_offset_down() {
        // Byte 1 lands inside `é`; snap down to byte 0 → col 0 (no panic).
        assert_eq!(utf16_col("éx", 1), 0);
        assert_eq!(utf16_col("abc", 100), 3);
    }

    #[test]
    fn line_and_utf16_col_from_absolute_byte_offset() {
        let src = "local x = 1\nfoo()";
        // `foo` starts at absolute byte 12 → line 2, col 0.
        assert_eq!(line_and_utf16_col(src, 12), (2, 0));
        // A multibyte char before the offset counts as utf16, not bytes.
        let src2 = "-- é\nbar"; // `bar` at absolute byte 6 (é = 2 bytes)
        assert_eq!(line_and_utf16_col(src2, 6), (2, 0));
        // Mid-line offset on line 2.
        assert_eq!(line_and_utf16_col("ab\ncdef", 5), (2, 2));
    }

    #[test]
    fn content_extent_last_col_is_utf16_not_bytes() {
        // Last line `é` is 2 bytes but 1 utf16 unit.
        assert_eq!(content_extent("x\né"), (2, 1));
    }
}

//! Body slice extraction + on-the-wire compaction.
//!
//! Extracted from `query.rs` (Phase 3.2+ structure split). The public
//! surface — `BodyOptions`, `BodySlice`, `body_for_fqdn` — is re-exported
//! from `query` so the call path stays `standardoc_core::query::*`. The
//! helpers (noise stripping, indent compaction, inline-comment strip,
//! blank-line collapse) live here too; `count_leading_noise_lines` is
//! `pub(super)` so `query::body_helper_tests` can keep poking at it.

use serde::{Deserialize, Serialize};

use crate::storage::error::StorageError;
use crate::storage::handle::IndexHandle;

/// Aggregated result of [`body_for_fqdn`]: the raw source slice covering a
/// symbol's declared `start_line..=end_line` plus enough metadata for the
/// caller to know what was returned and whether a `max_lines` cap or any of
/// the noise-stripping options kicked in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BodySlice {
    pub fqdn: String,
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub body: String,
    pub truncated: bool,
    pub total_body_lines: u32,
    /// Number of leading "noise" lines (doc comments, attributes, blank
    /// lines between them) dropped by `BodyOptions::strip_attrs`. Zero
    /// when stripping is disabled or the body had no leading noise.
    #[serde(default)]
    pub stripped_lines: u32,
    /// `true` when `BodyOptions::signature_only` truncated the body at
    /// the first line containing `{`. Independent of `truncated` (which
    /// only reflects the `max_lines` cap).
    #[serde(default)]
    pub signature_only: bool,
    /// Number of leading-whitespace bytes shared by every non-blank line
    /// of the returned slice that were stripped to dedent the body. Zero
    /// when the body had no common indent (or only one non-blank line at
    /// column 0). Pure compaction signal — the original column positions
    /// can be recovered by re-reading the file at `start_line`.
    #[serde(default)]
    pub dedented_prefix_len: u32,
    /// What one indent level in the returned `body` looks like. `"\t"`
    /// when leading 4-space (or 2-space) runs were converted to tabs OR
    /// the source already used tabs. Empty when the body has no indented
    /// line, or when the residual indent is too irregular to canonicalize
    /// (mixed tabs+spaces, non-power-of-2 widths) — in that case the
    /// body is returned verbatim post-dedent.
    #[serde(default)]
    pub indent_unit: String,
}

/// Knobs controlling the slice returned by [`body_for_fqdn`]. Defaults give
/// the legacy "verbatim slice" behavior.
#[derive(Debug, Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct BodyOptions {
    /// Cap on the number of returned lines. When the slice exceeds this
    /// count, the response is truncated and `BodySlice.truncated = true`.
    pub max_lines: Option<u32>,
    /// Drop leading doc comments (`///`, `//!`, `//`, `/* … */`, `/** … */`)
    /// AND attribute lines (`#[…]`, `#![…]` — including their multi-line
    /// continuations) AND any blank lines interleaved between them.
    /// Stops at the first line that is neither comment, attribute, nor
    /// blank. Massive shrink for handlers buried under verbose
    /// `#[tool(description = "…")]` blocks.
    pub strip_attrs: bool,
    /// Truncate the body just after the first line containing `{` (the
    /// opening brace of the function block). For Rust / TS / JS / C-like
    /// targets this returns the full multi-line signature without the
    /// implementation. Combine with `strip_attrs` to get the cleanest
    /// signature view. No-op for languages without `{` (Python, Lua —
    /// punted to a future per-language handler).
    pub signature_only: bool,
    /// Strip C-style inline comments from the returned body: `// …\n`
    /// line comments and `/* … */` block comments. Operates after
    /// `strip_attrs` / `signature_only` / `max_lines`, so the leading-
    /// noise paragraph drop and the dedent stay independent knobs.
    /// String-literal safe for `"…"` double-quoted strings (with `\`
    /// escapes), Rust raw strings (`r"…"`, `r#"…"#`, …) and TS
    /// template literals (`` `…` ``). Single-quoted strings are
    /// passed through verbatim — this is correct for Rust char
    /// literals + lifetimes but means TS `'string with // inside'`
    /// would still have the `//` consumed (rare; flag it if it bites).
    /// Lines fully consumed by a stripped comment become blank rather
    /// than disappearing, so line-number correspondence is preserved.
    pub strip_inline_comments: bool,
    /// Collapse runs of consecutive blank lines (lines containing only
    /// whitespace) down to a single blank line. Off by default so
    /// `get_body` preserves line-number alignment with the source;
    /// `get_code` flips it on so the cleaned output doesn't carry
    /// vestigial blanks where inline-comment-only lines used to sit.
    pub collapse_blank_lines: bool,
}

/// Returns the raw source text of the symbol at `fqdn`, sliced from the file
/// on disk between its `start_line` and `end_line`. Returns `None` when no
/// symbol matches the FQDN. See [`BodyOptions`] for the ways the slice can
/// be trimmed.
///
/// File I/O is anchored at `IndexHandle::workspace_root()`; the indexed
/// `location.file` is assumed workspace-relative (the IR contract).
pub fn body_for_fqdn(
    handle: &IndexHandle,
    fqdn: &str,
    opts: &BodyOptions,
) -> Result<Option<BodySlice>, StorageError> {
    let Some(symbol) = super::symbol_by_fqdn(handle, fqdn)? else {
        return Ok(None);
    };
    let workspace_root = handle.workspace_root();
    let file_abs = workspace_root.join(&symbol.location.file);
    let content = std::fs::read_to_string(&file_abs)?;
    let all_lines: Vec<&str> = content.lines().collect();

    let start_zero = symbol.location.start_line.saturating_sub(1) as usize;
    let end_inclusive = (symbol.location.end_line as usize).min(all_lines.len());
    if start_zero >= end_inclusive {
        return Ok(Some(BodySlice {
            fqdn: symbol.fqdn.clone(),
            file: symbol.location.file.clone(),
            start_line: symbol.location.start_line,
            end_line: symbol.location.end_line,
            body: String::new(),
            truncated: false,
            total_body_lines: 0,
            stripped_lines: 0,
            signature_only: false,
            dedented_prefix_len: 0,
            indent_unit: String::new(),
        }));
    }
    let raw_slice: &[&str] = &all_lines[start_zero..end_inclusive];
    let total = u32::try_from(raw_slice.len()).unwrap_or(u32::MAX);

    let stripped_count = if opts.strip_attrs {
        count_leading_noise_lines(raw_slice)
    } else {
        0
    };
    let after_strip: &[&str] = &raw_slice[stripped_count..];

    let (after_signature, signature_truncated) = if opts.signature_only {
        match after_strip.iter().position(|l| l.contains('{')) {
            Some(i) => (&after_strip[..=i], true),
            None => (after_strip, false),
        }
    } else {
        (after_strip, false)
    };

    let (taken, truncated) = match opts.max_lines {
        Some(cap) if (cap as usize) < after_signature.len() => {
            (&after_signature[..cap as usize], true)
        }
        _ => (after_signature, false),
    };
    let compact = compact_body_indent(taken);
    let stripped = if opts.strip_inline_comments {
        strip_inline_comments_in_body(&compact.body)
    } else {
        compact.body
    };
    let final_body = if opts.collapse_blank_lines {
        collapse_blank_lines_in_body(&stripped)
    } else {
        stripped
    };
    Ok(Some(BodySlice {
        fqdn: symbol.fqdn.clone(),
        file: symbol.location.file.clone(),
        start_line: symbol.location.start_line,
        end_line: symbol.location.end_line,
        body: final_body,
        truncated,
        total_body_lines: total,
        stripped_lines: u32::try_from(stripped_count).unwrap_or(u32::MAX),
        signature_only: signature_truncated,
        dedented_prefix_len: compact.dedented_prefix_len,
        indent_unit: compact.indent_unit,
    }))
}

/// Collapse runs of 2+ consecutive blank lines (whitespace-only) down
/// to a single blank line. Used by [`BodyOptions::collapse_blank_lines`]
/// to clean up the vestigial gaps left by `strip_inline_comments_in_body`
/// when an inline-comment-only line is stripped (the stripper preserves
/// the newline for line-number alignment; `get_code` doesn't care about
/// alignment so it asks for the collapse).
fn collapse_blank_lines_in_body(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut prev_blank = false;
    let mut first = true;
    for line in body.split('\n') {
        let is_blank = line.chars().all(char::is_whitespace);
        if is_blank && prev_blank {
            continue;
        }
        if !first {
            out.push('\n');
        }
        out.push_str(line);
        prev_blank = is_blank;
        first = false;
    }
    out
}

/// Strip C-style inline comments from `body` while leaving `"…"` string
/// literals, Rust raw strings (`r"…"`, `r#"…"#`), and TS template
/// literals (`` `…` ``) untouched.
///
/// `//` strips to end of line. `/* … */` strips through the closing
/// `*/`, preserving newlines so line-number alignment is intact when
/// the caller pairs the body with diagnostics. Single-quoted spans
/// (`'…'`) are walked as plain code — correct for Rust lifetimes /
/// char literals; a TS `'string'` with `//` inside is a documented
/// edge case (the comment characters get stripped).
///
/// Walk is byte-level but only ASCII tokens (`/ * " ` r # \ \n`) drive
/// state transitions — every non-token byte is included via
/// `out.push_str(&body[copy_from..i])` slice copies, so multi-byte
/// UTF8 sequences stay intact.
pub(super) fn strip_inline_comments_in_body(body: &str) -> String {
    enum St {
        Code,
        LineComment,
        BlockComment(u32),
        DqString,
        Template,
        RawString(usize),
    }
    let bytes = body.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut state = St::Code;
    let mut copy_from = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        match state {
            St::Code => {
                // Rust raw string opener: `r"` or `r#"` / `r##"` …
                if b == b'r' {
                    let mut j = i + 1;
                    let mut hashes = 0usize;
                    while j < bytes.len() && bytes[j] == b'#' {
                        hashes += 1;
                        j += 1;
                    }
                    if j < bytes.len() && bytes[j] == b'"' {
                        state = St::RawString(hashes);
                        i = j + 1;
                        continue;
                    }
                }
                if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                    out.push_str(&body[copy_from..i]);
                    state = St::LineComment;
                    i += 2;
                    continue;
                }
                if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                    out.push_str(&body[copy_from..i]);
                    state = St::BlockComment(1);
                    i += 2;
                    continue;
                }
                if b == b'"' {
                    state = St::DqString;
                } else if b == b'`' {
                    state = St::Template;
                }
                i += 1;
            }
            St::LineComment => {
                if b == b'\n' {
                    copy_from = i;
                    state = St::Code;
                }
                i += 1;
            }
            St::BlockComment(depth) => {
                if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                    state = St::BlockComment(depth + 1);
                    i += 2;
                    continue;
                }
                if b == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                    if depth == 1 {
                        state = St::Code;
                        copy_from = i + 2;
                    } else {
                        state = St::BlockComment(depth - 1);
                    }
                    i += 2;
                    continue;
                }
                if b == b'\n' {
                    out.push('\n');
                }
                i += 1;
            }
            St::DqString => {
                if b == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if b == b'"' {
                    state = St::Code;
                }
                i += 1;
            }
            St::Template => {
                if b == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if b == b'`' {
                    state = St::Code;
                }
                i += 1;
            }
            St::RawString(hashes) => {
                if b == b'"' {
                    let end = i + 1 + hashes;
                    if end <= bytes.len() && bytes[i + 1..end].iter().all(|&c| c == b'#') {
                        state = St::Code;
                        i = end;
                        continue;
                    }
                }
                i += 1;
            }
        }
    }
    if copy_from < bytes.len() {
        out.push_str(&body[copy_from..]);
    }
    out
}

/// Counts how many leading lines of `slice` look like noise:
/// `///` / `//!` / `//` doc comments, `/* … */` block comments, `#[…]` /
/// `#![…]` attributes (with multi-line continuations balanced via paren
/// depth), and blank lines interleaved between them. Stops at the first
/// non-noise line. Pure function — easy to test in isolation.
pub(super) fn count_leading_noise_lines(slice: &[&str]) -> usize {
    let mut i = 0usize;
    let mut paren_depth: i32 = 0;
    let mut in_block_comment = false;
    while i < slice.len() {
        let raw = slice[i];
        let line = raw.trim_start();
        if in_block_comment {
            if line.contains("*/") {
                in_block_comment = false;
            }
            i += 1;
            continue;
        }
        if paren_depth > 0 {
            paren_depth += paren_depth_delta(raw);
            i += 1;
            continue;
        }
        if line.is_empty() {
            i += 1;
            continue;
        }
        if line.starts_with("///") || line.starts_with("//!") || line.starts_with("//") {
            i += 1;
            continue;
        }
        if line.starts_with("/**") || line.starts_with("/*") {
            if !line.contains("*/") {
                in_block_comment = true;
            }
            i += 1;
            continue;
        }
        if line.starts_with("#[") || line.starts_with("#![") {
            paren_depth += paren_depth_delta(raw);
            i += 1;
            continue;
        }
        // `*` continuation of a `/* …` block comment we missed by starting
        // mid-block (shouldn't happen with well-formed slices, but cheap
        // to handle).
        if line.starts_with('*') && !line.starts_with("*/") {
            i += 1;
            continue;
        }
        break;
    }
    i
}

#[inline]
fn paren_depth_delta(line: &str) -> i32 {
    let mut d: i32 = 0;
    for c in line.chars() {
        match c {
            '(' | '[' => d += 1,
            ')' | ']' => d -= 1,
            _ => {}
        }
    }
    d
}

/// Output of [`compact_body_indent`]: a body string ready to serialize
/// plus enough metadata for the caller (and downstream tools) to know
/// what was done.
pub(super) struct CompactedBody {
    pub(super) body: String,
    pub(super) dedented_prefix_len: u32,
    pub(super) indent_unit: String,
}

/// Compacts the indentation of a body slice for over-the-wire transport.
///
/// Two passes:
///   1. **Dedent common prefix.** Find the longest leading-whitespace
///      sequence shared by every non-blank line and strip it. A method
///      body indented at 8 spaces inside an impl block becomes flush-left
///      — multi-KB savings on long bodies.
///   2. **Tab-convert residual leading runs.** If every remaining
///      non-blank line is indented with a uniform-width space run (every
///      width a multiple of 4, or every width a multiple of 2), each
///      such run is converted to `\t`. Sources that already use tabs
///      pass through unchanged.
///
/// Mixed or irregular indent (tabs + spaces in the same line, or
/// non-power-of-2 widths) skips pass 2 and returns the dedented body
/// verbatim with `indent_unit = ""`. The line *content* is never altered
/// beyond leading whitespace — `taken.join("\n")` semantics are preserved
/// when no compaction is applicable.
pub(super) fn compact_body_indent(lines: &[&str]) -> CompactedBody {
    if lines.is_empty() {
        return CompactedBody {
            body: String::new(),
            dedented_prefix_len: 0,
            indent_unit: String::new(),
        };
    }

    let common = longest_common_leading_ws(lines);
    let prefix_len = common.len();

    let stripped: Vec<String> = lines
        .iter()
        .map(|l| {
            if l.starts_with(common) {
                l[prefix_len..].to_string()
            } else {
                String::new()
            }
        })
        .collect();

    let unit = detect_indent_unit(&stripped);
    let (final_lines, indent_unit) = match unit {
        Some(width) if width > 0 => {
            let converted: Vec<String> = stripped
                .iter()
                .map(|l| convert_leading_spaces_to_tabs(l, width))
                .collect();
            (converted, "\t".to_string())
        }
        Some(_) => (stripped, String::new()),
        None => (stripped, "\t".to_string()),
    };

    CompactedBody {
        body: final_lines.join("\n"),
        dedented_prefix_len: u32::try_from(prefix_len).unwrap_or(u32::MAX),
        indent_unit,
    }
}

fn leading_ws(s: &str) -> &str {
    let end = s.bytes().take_while(|b| matches!(*b, b' ' | b'\t')).count();
    &s[..end]
}

fn longest_common_leading_ws<'a>(lines: &[&'a str]) -> &'a str {
    let mut iter = lines.iter().filter(|l| !l.trim().is_empty());
    let Some(first) = iter.next() else {
        return "";
    };
    let mut prefix = leading_ws(first);
    for line in iter {
        let lw = leading_ws(line);
        let n = prefix
            .bytes()
            .zip(lw.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        prefix = &prefix[..n];
        if prefix.is_empty() {
            break;
        }
    }
    prefix
}

/// Returns:
/// - `None` when leading whitespace is already entirely tabs — no
///   conversion is necessary; the output uses tabs natively.
/// - `Some(width)` with `width > 0` when every non-blank line's leading
///   whitespace is a multiple of `width` spaces (try 4 first, fall back
///   to 2) — conversion is applicable.
/// - `Some(0)` when the residual is irregular (mixed tabs+spaces on the
///   same line, or non-multiple widths) — leave the body verbatim and
///   report `indent_unit = ""`.
fn detect_indent_unit(lines: &[String]) -> Option<usize> {
    let mut has_tab_only = false;
    let mut space_widths: Vec<usize> = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let lw = leading_ws(line);
        if lw.is_empty() {
            continue;
        }
        let has_tab = lw.bytes().any(|b| b == b'\t');
        let has_space = lw.bytes().any(|b| b == b' ');
        if has_tab && has_space {
            return Some(0);
        }
        if has_tab {
            has_tab_only = true;
        } else {
            space_widths.push(lw.len());
        }
    }
    if has_tab_only && space_widths.is_empty() {
        return None;
    }
    if has_tab_only {
        return Some(0);
    }
    if space_widths.is_empty() {
        return Some(0);
    }
    if space_widths.iter().all(|n| *n % 4 == 0) {
        return Some(4);
    }
    if space_widths.iter().all(|n| *n % 2 == 0) {
        return Some(2);
    }
    Some(0)
}

fn convert_leading_spaces_to_tabs(line: &str, width: usize) -> String {
    let bytes = line.as_bytes();
    let mut tabs = 0;
    let mut i = 0;
    while i + width <= bytes.len() && bytes[i..i + width].iter().all(|b| *b == b' ') {
        tabs += 1;
        i += width;
    }
    if tabs == 0 {
        return line.to_string();
    }
    let mut out = String::with_capacity(tabs + bytes.len() - i);
    for _ in 0..tabs {
        out.push('\t');
    }
    out.push_str(&line[i..]);
    out
}

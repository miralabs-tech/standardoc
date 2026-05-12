//! Single-File-Component (SFC) parser shared by Vue and Svelte providers.
//!
//! Lock 41 §1 Q11 chose a custom zero-dep state machine (~50-100 LOC) rather
//! than pulling in a dedicated SFC crate. This module exposes the lossless
//! split of an SFC source into its top-level `<script>` and `<template>`
//! blocks, preserving byte spans so callers can pad whitespace prefixes
//! (`pad_until_byte_offset`) before delegating to swc — keeping all
//! emitted IR positions aligned with the original `.vue` / `.svelte` file.
//!
//! Day-1 scope (lock 41 §1 Q11):
//! - Multi `<script>` blocks (Vue's `<script>` + `<script setup>` pair).
//! - Single `<template>` block per file.
//! - `lang` attribute capture (quoted or unquoted).
//! - Naive HTML-comment skipping (`<!-- ... -->`).
//! - CDATA, DOCTYPE, processing instructions: ignored.

/// One top-level block extracted from an SFC source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SfcBlock {
    /// Tag name as authored (`script`, `template`, `style`, ...). Lowercase.
    pub tag: String,
    /// `lang` attribute value when present (`ts`, `tsx`, `js`, `jsx`, ...).
    pub lang: Option<String>,
    /// Byte offset of the block's content start (just past the opening
    /// tag's `>`) in the original source. Used by callers to compute the
    /// padding prefix that keeps swc spans aligned with the SFC file.
    pub content_start: usize,
    /// Byte offset of the block's content end (just before the closing
    /// `</tag>`).
    pub content_end: usize,
    /// Other attributes captured verbatim (key → value-or-empty). Day-1 the
    /// only consumer is `lang` lookup, but we keep the bag open for Vue 3
    /// `<script setup>` flagging or future framework attrs.
    pub attributes: Vec<(String, Option<String>)>,
}

/// Result of running the SFC parser over a `.vue` / `.svelte` source.
#[derive(Debug, Default)]
pub(crate) struct SfcDocument {
    /// All `<script ...>...</script>` blocks in source order. Vue 3 SFCs
    /// commonly carry both `<script>` and `<script setup>` — both land here.
    pub scripts: Vec<SfcBlock>,
    /// The single `<template>...</template>` block when present.
    pub template: Option<SfcBlock>,
    /// All `<style ...>...</style>` blocks. Day-1 not consumed by any
    /// provider but captured so future CSS-aware providers can plug in
    /// without re-parsing the SFC.
    pub styles: Vec<SfcBlock>,
}

impl SfcBlock {
    /// True when the `attributes` bag carries a bare attribute named `setup`
    /// (Vue 3 `<script setup>` form). Currently only consumed by the
    /// scaffold tests — kept on the public surface because phase B's
    /// multi-script merge order will use it (lock 41 §1 Q2: `<script
    /// setup>` always concatenates after a plain `<script>`).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn is_script_setup(&self) -> bool {
        self.tag == "script"
            && self
                .attributes
                .iter()
                .any(|(k, _)| k == "setup")
    }
}

/// Splits an SFC source into top-level blocks.
///
/// Returns an empty `SfcDocument` if no recognised block is found. Never
/// fails — broken SFCs degrade gracefully (caller treats unparsed regions
/// as inert text).
pub(crate) fn extract_blocks(source: &str) -> SfcDocument {
    let bytes = source.as_bytes();
    let mut doc = SfcDocument::default();
    let mut i = 0;
    while i < bytes.len() {
        if starts_with(bytes, i, b"<!--") {
            i = skip_comment(bytes, i + 4);
            continue;
        }
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        // `<` followed by an alphabetic char is a candidate opening tag.
        let after_lt = i + 1;
        if after_lt >= bytes.len() || !bytes[after_lt].is_ascii_alphabetic() {
            i += 1;
            continue;
        }
        let name_end = read_tag_name(bytes, after_lt);
        let tag_name = std::str::from_utf8(&bytes[after_lt..name_end])
            .unwrap_or("")
            .to_ascii_lowercase();
        if !is_recognised_block_tag(&tag_name) {
            i += 1;
            continue;
        }
        let (attrs, close_pos, self_closing) = parse_attributes(bytes, name_end);
        if self_closing {
            // `<script ... />` — empty block, nothing to extract.
            i = close_pos + 2;
            continue;
        }
        let content_start = close_pos + 1;
        let needle = format!("</{tag_name}");
        let Some(content_end) = find_close_tag(bytes, content_start, needle.as_bytes()) else {
            // Unmatched opening — treat the rest as inert and stop walking
            // this branch to avoid pathological re-scans.
            i = content_start;
            continue;
        };
        let lang = attrs
            .iter()
            .find(|(k, _)| k == "lang")
            .and_then(|(_, v)| v.clone());
        let block = SfcBlock {
            tag: tag_name.clone(),
            lang,
            content_start,
            content_end,
            attributes: attrs,
        };
        match tag_name.as_str() {
            "script" => doc.scripts.push(block),
            "template" if doc.template.is_none() => doc.template = Some(block),
            "style" => doc.styles.push(block),
            _ => {}
        }
        // Skip past the closing tag's `>`.
        i = match find_after(bytes, content_end, b'>') {
            Some(after) => after,
            None => bytes.len(),
        };
    }
    doc
}

/// Pads `out` with newlines and spaces until its byte length matches
/// `target`, copying line breaks from the matching prefix of `source` so
/// row counts stay aligned. Used to produce a script payload whose swc
/// spans line up with the SFC file's coordinate system without
/// post-processing.
///
/// Byte-wise filler (one space per non-newline byte, including UTF-8
/// continuation bytes) — swc spans are byte-based, so byte alignment is
/// what matters for `Site { line, col }` accuracy. Column counts within
/// a line containing multi-byte UTF-8 chars in the prefix may drift
/// slightly but day-1 we don't index `<template>` script-substitutes
/// for symbol locations, only for refs.
pub(crate) fn pad_until_byte_offset(out: &mut String, target: usize, source: &str) {
    let current = out.len();
    if current >= target {
        return;
    }
    let bytes = source.as_bytes();
    let end = target.min(bytes.len());
    for &b in &bytes[current..end] {
        if b == b'\n' {
            out.push('\n');
        } else {
            out.push(' ');
        }
    }
}

// --- helpers (some pub(crate) for reuse by template/{vue,svelte}.rs) -----

fn is_recognised_block_tag(tag: &str) -> bool {
    matches!(tag, "script" | "template" | "style")
}

pub(crate) fn starts_with(bytes: &[u8], i: usize, needle: &[u8]) -> bool {
    bytes.len() >= i + needle.len() && &bytes[i..i + needle.len()] == needle
}

pub(crate) fn skip_comment(bytes: &[u8], from: usize) -> usize {
    let mut i = from;
    while i + 2 < bytes.len() {
        if bytes[i] == b'-' && bytes[i + 1] == b'-' && bytes[i + 2] == b'>' {
            return i + 3;
        }
        i += 1;
    }
    bytes.len()
}

pub(crate) fn read_tag_name(bytes: &[u8], from: usize) -> usize {
    let mut i = from;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' {
            i += 1;
        } else {
            break;
        }
    }
    i
}

/// Reads attributes from `bytes[from..]` until the matching `>` (or `/>`).
/// Returns `(attrs, close_pos, self_closing)` where `close_pos` is the
/// index of `>` (or the leading `/` for `/>`), and `self_closing` is true
/// when the form was `... />`.
fn parse_attributes(
    bytes: &[u8],
    from: usize,
) -> (Vec<(String, Option<String>)>, usize, bool) {
    let mut attrs: Vec<(String, Option<String>)> = Vec::new();
    let mut i = from;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b == b'>' {
            return (attrs, i, false);
        }
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'>' {
            return (attrs, i, true);
        }
        // Read attribute name.
        let name_start = i;
        while i < bytes.len() {
            let b = bytes[i];
            if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b':' || b == b'@' {
                i += 1;
            } else {
                break;
            }
        }
        if i == name_start {
            // Couldn't make progress — guard against infinite loop on
            // malformed input by skipping the offending byte.
            i += 1;
            continue;
        }
        let name = std::str::from_utf8(&bytes[name_start..i])
            .unwrap_or("")
            .to_string();
        // Optional `= value` pair.
        let value = read_attribute_value(bytes, &mut i);
        attrs.push((name, value));
    }
    (attrs, bytes.len(), false)
}

/// On entry, `*i` points to the byte after the attribute name. Skips
/// optional whitespace, then if the next byte is `=`, consumes the value
/// (quoted or bare). On return `*i` points past the value (or unchanged
/// when no `=` was present).
fn read_attribute_value(bytes: &[u8], i: &mut usize) -> Option<String> {
    let saved = *i;
    while *i < bytes.len() && bytes[*i].is_ascii_whitespace() {
        *i += 1;
    }
    if *i >= bytes.len() || bytes[*i] != b'=' {
        *i = saved;
        return None;
    }
    *i += 1;
    while *i < bytes.len() && bytes[*i].is_ascii_whitespace() {
        *i += 1;
    }
    if *i >= bytes.len() {
        return None;
    }
    let quote = bytes[*i];
    if quote == b'"' || quote == b'\'' {
        *i += 1;
        let start = *i;
        while *i < bytes.len() && bytes[*i] != quote {
            *i += 1;
        }
        let value = std::str::from_utf8(&bytes[start..*i])
            .unwrap_or("")
            .to_string();
        if *i < bytes.len() {
            *i += 1; // skip closing quote
        }
        Some(value)
    } else {
        // Unquoted value — runs until whitespace, `>` or `/`.
        let start = *i;
        while *i < bytes.len() {
            let b = bytes[*i];
            if b.is_ascii_whitespace() || b == b'>' || b == b'/' {
                break;
            }
            *i += 1;
        }
        Some(
            std::str::from_utf8(&bytes[start..*i])
                .unwrap_or("")
                .to_string(),
        )
    }
}

/// Returns the byte index of the first occurrence of `needle` in
/// `bytes[from..]` followed by a non-name byte (so `</script` doesn't
/// match `</scripts`). Case-insensitive ASCII match — we lowercase the
/// tag on the opening side so an opening `<SCRIPT>` and closing
/// `</SCRIPT>` both round-trip correctly.
fn find_close_tag(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    let mut i = from;
    while i + needle.len() <= bytes.len() {
        if eq_ignore_ascii_case(&bytes[i..i + needle.len()], needle) {
            let next = bytes.get(i + needle.len()).copied().unwrap_or(b' ');
            if !next.is_ascii_alphanumeric() && next != b'-' && next != b'_' {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

fn eq_ignore_ascii_case(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .all(|(x, y)| x.eq_ignore_ascii_case(y))
}

pub(crate) fn find_after(bytes: &[u8], from: usize, target: u8) -> Option<usize> {
    bytes[from..]
        .iter()
        .position(|&b| b == target)
        .map(|rel| from + rel + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract(src: &str) -> SfcDocument {
        extract_blocks(src)
    }

    #[test]
    fn empty_source_yields_empty_document() {
        let doc = extract("");
        assert!(doc.scripts.is_empty());
        assert!(doc.template.is_none());
        assert!(doc.styles.is_empty());
    }

    #[test]
    fn plain_html_without_recognised_tags_yields_empty() {
        let doc = extract("<div>hello</div><span/>");
        assert!(doc.scripts.is_empty());
        assert!(doc.template.is_none());
    }

    #[test]
    fn single_script_block_is_captured() {
        let src = "<script>let x = 1;</script>";
        let doc = extract(src);
        assert_eq!(doc.scripts.len(), 1);
        let b = &doc.scripts[0];
        assert_eq!(b.tag, "script");
        assert_eq!(&src[b.content_start..b.content_end], "let x = 1;");
    }

    #[test]
    fn template_block_is_captured() {
        let src = "<template><h1>Hi</h1></template>";
        let doc = extract(src);
        let t = doc.template.as_ref().unwrap();
        assert_eq!(t.tag, "template");
        assert_eq!(&src[t.content_start..t.content_end], "<h1>Hi</h1>");
    }

    #[test]
    fn style_block_is_captured() {
        let src = "<style>body { color: red; }</style>";
        let doc = extract(src);
        assert_eq!(doc.styles.len(), 1);
    }

    #[test]
    fn script_with_lang_attribute_quoted() {
        let doc = extract(r#"<script lang="ts">const a = 1;</script>"#);
        assert_eq!(doc.scripts[0].lang.as_deref(), Some("ts"));
    }

    #[test]
    fn script_with_lang_attribute_single_quoted() {
        let doc = extract(r"<script lang='tsx'>const a = 1;</script>");
        assert_eq!(doc.scripts[0].lang.as_deref(), Some("tsx"));
    }

    #[test]
    fn script_with_lang_attribute_unquoted() {
        let doc = extract("<script lang=ts>const a = 1;</script>");
        assert_eq!(doc.scripts[0].lang.as_deref(), Some("ts"));
    }

    #[test]
    fn script_setup_is_detected() {
        let doc = extract("<script setup>const a = 1;</script>");
        assert!(doc.scripts[0].is_script_setup());
    }

    #[test]
    fn multi_script_blocks_both_captured_in_order() {
        let src = "<script>const a = 1;</script>\n<script setup>const b = 2;</script>";
        let doc = extract(src);
        assert_eq!(doc.scripts.len(), 2);
        assert!(!doc.scripts[0].is_script_setup());
        assert!(doc.scripts[1].is_script_setup());
    }

    #[test]
    fn html_comment_is_skipped() {
        let src = "<!-- <script>NOT REAL</script> --><script>real;</script>";
        let doc = extract(src);
        assert_eq!(doc.scripts.len(), 1);
        assert_eq!(&src[doc.scripts[0].content_start..doc.scripts[0].content_end], "real;");
    }

    #[test]
    fn unclosed_block_does_not_panic() {
        let doc = extract("<script>oh no");
        assert!(doc.scripts.is_empty());
    }

    #[test]
    fn self_closing_block_is_dropped() {
        let doc = extract("<script src=\"x.js\" />");
        assert!(doc.scripts.is_empty());
    }

    #[test]
    fn first_template_wins_when_duplicated() {
        let src = "<template>first</template><template>second</template>";
        let doc = extract(src);
        let t = doc.template.as_ref().unwrap();
        assert_eq!(&src[t.content_start..t.content_end], "first");
    }

    #[test]
    fn script_attributes_capture_extra_keys() {
        let doc = extract(r#"<script lang="ts" setup foo="bar">x;</script>"#);
        let attrs = &doc.scripts[0].attributes;
        let lang = attrs.iter().find(|(k, _)| k == "lang");
        let setup = attrs.iter().find(|(k, _)| k == "setup");
        let foo = attrs.iter().find(|(k, _)| k == "foo");
        assert!(lang.is_some());
        assert!(setup.is_some());
        assert_eq!(foo.unwrap().1.as_deref(), Some("bar"));
    }

    #[test]
    fn template_then_script_then_style_all_captured() {
        let src = "<template><h1/></template><script>x;</script><style>a{}</style>";
        let doc = extract(src);
        assert!(doc.template.is_some());
        assert_eq!(doc.scripts.len(), 1);
        assert_eq!(doc.styles.len(), 1);
    }

    #[test]
    fn close_tag_lookup_is_strict_about_word_boundary() {
        // `</scriptish>` shouldn't end a `<script>` block.
        let src = "<script>const x = '</scriptish>';</script>";
        let doc = extract(src);
        assert_eq!(doc.scripts.len(), 1);
        let body = &src[doc.scripts[0].content_start..doc.scripts[0].content_end];
        assert!(body.contains("</scriptish>"));
    }

    #[test]
    fn style_with_lang_scss() {
        let doc = extract("<style lang=\"scss\">.x{}</style>");
        assert_eq!(doc.styles[0].lang.as_deref(), Some("scss"));
    }

    #[test]
    fn unrecognised_top_level_tag_is_ignored() {
        let doc = extract("<unknown>x</unknown><script>y;</script>");
        assert_eq!(doc.scripts.len(), 1);
    }

    #[test]
    fn pad_until_byte_offset_pads_with_spaces_and_keeps_newlines() {
        let mut out = String::new();
        let source = "abc\ndef\n<script>X</script>";
        // Pad until offset of 'X' which is just past `<script>` (8 bytes
        // into source at position of '<' = 8). Wait the actual layout:
        //   "abc\ndef\n<script>X..."
        //   0123 4 567 8 ...
        //   "abc" 0..2, '\n' 3, "def" 4..6, '\n' 7, '<script>' 8..15, 'X' 16
        pad_until_byte_offset(&mut out, 16, source);
        assert_eq!(out.len(), 16);
        // Lines preserved.
        assert_eq!(out.matches('\n').count(), 2);
    }

    #[test]
    fn pad_no_op_when_already_at_target() {
        let mut out = String::from("hello");
        pad_until_byte_offset(&mut out, 3, "world");
        assert_eq!(out, "hello");
    }

    #[test]
    fn pad_handles_target_beyond_source_length() {
        let mut out = String::new();
        pad_until_byte_offset(&mut out, 100, "ab");
        assert_eq!(out, "  ");
    }

    #[test]
    fn nested_template_uses_outer_close() {
        // We don't track nesting — naive find_close_tag picks the FIRST
        // `</template>`. A user nesting templates triggers the
        // documented-acceptable-edge-case (lock 41 §1 Q11).
        let src = "<template><template>inner</template>outer</template>";
        let doc = extract(src);
        let t = doc.template.as_ref().unwrap();
        // First close wins.
        assert_eq!(&src[t.content_start..t.content_end], "<template>inner");
    }

    #[test]
    fn block_byte_spans_round_trip_through_source_slice() {
        let src = "<template>\n  <h1>{{ msg }}</h1>\n</template>\n<script>const msg = 'hi';</script>";
        let doc = extract(src);
        let template_body = &src[doc.template.as_ref().unwrap().content_start
            ..doc.template.as_ref().unwrap().content_end];
        assert!(template_body.contains("{{ msg }}"));
        let script_body = &src[doc.scripts[0].content_start..doc.scripts[0].content_end];
        assert_eq!(script_body, "const msg = 'hi';");
    }

    #[test]
    fn template_attribute_with_directive_form_is_recorded() {
        let doc = extract(r#"<template><div v-if="x">y</div></template>"#);
        // Attribute capture is at the BLOCK level only — directives inside
        // the block content live in the body (parsed by template/vue.rs).
        // Just assert the block boundary is sane.
        let t = doc.template.as_ref().unwrap();
        assert_eq!(&t.tag, "template");
    }

    #[test]
    fn style_scoped_attribute_captured() {
        let doc = extract("<style scoped>.x{}</style>");
        let attrs = &doc.styles[0].attributes;
        assert!(attrs.iter().any(|(k, _)| k == "scoped"));
    }

    #[test]
    fn mixed_with_doctype_prefix_skips_doctype_text() {
        // We don't parse DOCTYPE; <! is recognised only as comment opener.
        // A DOCTYPE line acts as inert text — the script after still parses.
        let src = "<!DOCTYPE html><script>x;</script>";
        let doc = extract(src);
        assert_eq!(doc.scripts.len(), 1);
    }

    #[test]
    fn empty_script_block_yields_zero_length_body() {
        let src = "<script></script>";
        let doc = extract(src);
        let b = &doc.scripts[0];
        assert_eq!(b.content_start, b.content_end);
    }

    #[test]
    fn script_block_with_uppercase_tag_is_captured_lowercased() {
        let src = "<SCRIPT>x;</SCRIPT>";
        let doc = extract(src);
        assert_eq!(doc.scripts.len(), 1);
        assert_eq!(doc.scripts[0].tag, "script");
    }
}

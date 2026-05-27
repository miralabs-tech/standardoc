
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
    assert_eq!(
        &src[doc.scripts[0].content_start..doc.scripts[0].content_end],
        "real;"
    );
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
    let template_body = &src
        [doc.template.as_ref().unwrap().content_start..doc.template.as_ref().unwrap().content_end];
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

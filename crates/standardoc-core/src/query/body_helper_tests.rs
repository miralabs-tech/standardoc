use super::body::count_leading_noise_lines;

#[test]
fn count_leading_noise_lines_returns_zero_when_first_line_is_code() {
    let lines = vec!["pub fn foo() {", "    do_thing();", "}"];
    assert_eq!(count_leading_noise_lines(&lines), 0);
}

#[test]
fn count_leading_noise_lines_strips_doc_comments() {
    let lines = vec![
        "/// This is the doc.",
        "/// Second doc line.",
        "pub fn foo() {",
    ];
    assert_eq!(count_leading_noise_lines(&lines), 2);
}

#[test]
fn count_leading_noise_lines_strips_simple_attribute() {
    let lines = vec!["#[inline]", "pub fn foo() {"];
    assert_eq!(count_leading_noise_lines(&lines), 1);
}

#[test]
fn count_leading_noise_lines_strips_multi_line_attribute_via_paren_depth() {
    let lines = vec![
        "#[tool(",
        "    description = \"long\"",
        ")]",
        "async fn handler() {",
    ];
    assert_eq!(count_leading_noise_lines(&lines), 3);
}

#[test]
fn count_leading_noise_lines_strips_doc_then_attr_then_blank() {
    let lines = vec!["/// A function.", "#[allow(dead_code)]", "", "fn f() {"];
    assert_eq!(count_leading_noise_lines(&lines), 3);
}

#[test]
fn count_leading_noise_lines_strips_block_comment_spanning_lines() {
    let lines = vec!["/*", " * Multi-line block.", " */", "fn f() {"];
    assert_eq!(count_leading_noise_lines(&lines), 3);
}

#[test]
fn count_leading_noise_lines_handles_indented_attributes() {
    let lines = vec!["    /// indented doc", "    #[inline]", "    fn inner() {"];
    assert_eq!(count_leading_noise_lines(&lines), 2);
}

#[test]
fn count_leading_noise_lines_stops_at_first_non_noise_line() {
    let lines = vec![
        "/// doc",
        "fn first() {",
        "/// doc on inner",
        "fn nested() {",
    ];
    // Only the first /// is leading noise; everything after the `fn first()`
    // is body and must be preserved.
    assert_eq!(count_leading_noise_lines(&lines), 1);
}

use super::body::compact_body_indent;

#[test]
fn compact_body_indent_dedents_common_4_space_prefix_and_converts_to_tabs() {
    // A method body indented 4 spaces inside an impl block.
    let lines = vec!["    fn foo(&self) -> u32 {", "        self.x + 1", "    }"];
    let out = compact_body_indent(&lines);
    assert_eq!(out.dedented_prefix_len, 4);
    assert_eq!(out.indent_unit, "\t");
    assert_eq!(out.body, "fn foo(&self) -> u32 {\n\tself.x + 1\n}");
}

#[test]
fn compact_body_indent_dedents_8_space_then_tab_compacts_residual() {
    let lines = vec![
        "        fn deep() {",
        "            inner();",
        "                more();",
        "        }",
    ];
    let out = compact_body_indent(&lines);
    assert_eq!(out.dedented_prefix_len, 8);
    assert_eq!(out.indent_unit, "\t");
    assert_eq!(out.body, "fn deep() {\n\tinner();\n\t\tmore();\n}");
}

#[test]
fn compact_body_indent_preserves_tab_source_verbatim() {
    let lines = vec!["fn foo() {", "\tinner();", "\t\tnested();", "}"];
    let out = compact_body_indent(&lines);
    assert_eq!(out.dedented_prefix_len, 0);
    assert_eq!(out.indent_unit, "\t");
    assert_eq!(out.body, "fn foo() {\n\tinner();\n\t\tnested();\n}");
}

#[test]
fn compact_body_indent_converts_2_space_indent_to_tabs() {
    // TypeScript-style 2-space indent.
    let lines = vec![
        "export function foo() {",
        "  if (x) {",
        "    bar();",
        "  }",
        "}",
    ];
    let out = compact_body_indent(&lines);
    assert_eq!(out.dedented_prefix_len, 0);
    assert_eq!(out.indent_unit, "\t");
    assert_eq!(
        out.body,
        "export function foo() {\n\tif (x) {\n\t\tbar();\n\t}\n}"
    );
}

#[test]
fn compact_body_indent_blank_lines_do_not_break_dedent() {
    let lines = vec!["    fn foo() {", "", "        let x = 1;", "    }"];
    let out = compact_body_indent(&lines);
    assert_eq!(out.dedented_prefix_len, 4);
    assert_eq!(out.indent_unit, "\t");
    // Blank line stays empty (its leading-ws was shorter than common prefix).
    assert_eq!(out.body, "fn foo() {\n\n\tlet x = 1;\n}");
}

#[test]
fn compact_body_indent_skips_conversion_on_mixed_indent() {
    // One line uses tabs, another uses 3 spaces — non-uniform residual.
    let lines = vec!["fn foo() {", "\tinner();", "   weird();", "}"];
    let out = compact_body_indent(&lines);
    assert_eq!(out.dedented_prefix_len, 0);
    // Tabs present but spaces are non-multiple-of-2 / 4 → unit empty,
    // body returned verbatim post-(no-op) dedent.
    assert_eq!(out.indent_unit, "");
    assert_eq!(out.body, "fn foo() {\n\tinner();\n   weird();\n}");
}

#[test]
fn compact_body_indent_empty_input_returns_empty() {
    let lines: Vec<&str> = vec![];
    let out = compact_body_indent(&lines);
    assert_eq!(out.dedented_prefix_len, 0);
    assert_eq!(out.indent_unit, "");
    assert_eq!(out.body, "");
}

#[test]
fn compact_body_indent_single_line_no_indent_no_op() {
    let lines = vec!["fn foo() {}"];
    let out = compact_body_indent(&lines);
    assert_eq!(out.dedented_prefix_len, 0);
    assert_eq!(out.indent_unit, "");
    assert_eq!(out.body, "fn foo() {}");
}

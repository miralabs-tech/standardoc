//! Generic comment-span scanner — Path A (line-based, no string-literal
//! protection).
//!
//! Walks a file's content using the provider's declared `CommentStyles` and
//! returns every comment block as a `CommentSpan` ready to feed into the
//! annotation parser. Used by the satellite-annotation pass (`@doc-extend
//! K`) which must discover free-floating comments — those not attached to
//! any AST symbol.
//!
//! Limitation: a comment marker inside a string literal (`let s = "// foo"`)
//! is treated as a real comment opener. Marginal in practice. Per-provider
//! AST-aware overrides (Path B) are tracked as future work.

use crate::model::{CommentDelimiters, CommentStyle, CommentStyles};

/// One comment block, with delimiters and per-line decoration stripped,
/// ready for `annotation::parse`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentSpan {
    /// 1-based line of the opener in the source file.
    pub line_start: u32,
    /// 1-based line of the closer (or the last grouped line for single-line runs).
    pub line_end: u32,
    /// 1-based column of the opener.
    pub column: u32,
    pub style: CommentStyle,
    /// Body text after stripping the comment delimiters and per-line
    /// decoration (leading `*` for jsdoc, leading single-space padding).
    pub body: String,
}

/// Scan `content` for every comment span, contiguous single-line runs
/// merged into one span. Multi-line styles tried before single-line so a
/// `/**` is not consumed as `/*` followed by code.
pub fn scan(content: &str, styles: &CommentStyles) -> Vec<CommentSpan> {
    let lines: Vec<&str> = content.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if let Some(adv) = try_multi(
            &lines,
            i,
            styles.doc_multi.as_ref(),
            CommentStyle::DocMulti,
        ) {
            out.push(adv.span);
            i = adv.next;
            continue;
        }
        if let Some(adv) = try_multi(&lines, i, styles.multi.as_ref(), CommentStyle::MultiLine) {
            out.push(adv.span);
            i = adv.next;
            continue;
        }
        if let Some(adv) = try_single_run(&lines, i, &styles.doc_single, CommentStyle::DocSingle) {
            out.push(adv.span);
            i = adv.next;
            continue;
        }
        if let Some(adv) = try_single_run(&lines, i, &styles.single, CommentStyle::SingleLine) {
            out.push(adv.span);
            i = adv.next;
            continue;
        }
        i += 1;
    }
    out
}

struct Advance {
    span: CommentSpan,
    next: usize,
}

fn try_multi(
    lines: &[&str],
    i: usize,
    delims: Option<&CommentDelimiters>,
    style: CommentStyle,
) -> Option<Advance> {
    let delims = delims?;
    if delims.start.is_empty() || delims.end.is_empty() {
        return None;
    }
    let line = lines[i];
    let trimmed = line.trim_start();
    if !trimmed.starts_with(delims.start.as_str()) {
        return None;
    }
    let column = u32::try_from(line.len() - trimmed.len() + 1).unwrap_or(1);
    let after_start = &trimmed[delims.start.len()..];

    if let Some(close_idx) = after_start.find(delims.end.as_str()) {
        let body = strip_multi_body(&after_start[..close_idx]);
        return Some(Advance {
            span: CommentSpan {
                line_start: line_no(i),
                line_end: line_no(i),
                column,
                style,
                body,
            },
            next: i + 1,
        });
    }

    let mut collected = String::new();
    collected.push_str(after_start);
    let mut end = i;
    let mut j = i + 1;
    while j < lines.len() {
        let l = lines[j];
        if let Some(close_idx) = l.find(delims.end.as_str()) {
            collected.push('\n');
            collected.push_str(&l[..close_idx]);
            end = j;
            j += 1;
            return Some(Advance {
                span: CommentSpan {
                    line_start: line_no(i),
                    line_end: line_no(end),
                    column,
                    style,
                    body: strip_multi_body(&collected),
                },
                next: j,
            });
        }
        collected.push('\n');
        collected.push_str(l);
        end = j;
        j += 1;
    }
    // Unterminated — surface what we have. Stays robust on malformed input.
    Some(Advance {
        span: CommentSpan {
            line_start: line_no(i),
            line_end: line_no(end),
            column,
            style,
            body: strip_multi_body(&collected),
        },
        next: j,
    })
}

fn try_single_run(
    lines: &[&str],
    i: usize,
    markers: &[String],
    style: CommentStyle,
) -> Option<Advance> {
    if markers.is_empty() {
        return None;
    }
    let line = lines[i];
    let trimmed = line.trim_start();
    let marker = pick_longest_prefix(trimmed, markers)?;
    let column = u32::try_from(line.len() - trimmed.len() + 1).unwrap_or(1);

    let mut bodies = vec![strip_single_body(trimmed, &marker)];
    let mut end = i;
    let mut j = i + 1;
    while j < lines.len() {
        let t = lines[j].trim_start();
        let Some(next_marker) = pick_longest_prefix(t, markers) else {
            break;
        };
        if next_marker != marker {
            break;
        }
        bodies.push(strip_single_body(t, &marker));
        end = j;
        j += 1;
    }
    Some(Advance {
        span: CommentSpan {
            line_start: line_no(i),
            line_end: line_no(end),
            column,
            style,
            body: bodies.join("\n"),
        },
        next: j,
    })
}

fn pick_longest_prefix(trimmed: &str, markers: &[String]) -> Option<String> {
    markers
        .iter()
        .filter(|m| trimmed.starts_with(m.as_str()))
        .max_by_key(|m| m.len())
        .cloned()
}

fn strip_single_body(trimmed: &str, marker: &str) -> String {
    let after = &trimmed[marker.len()..];
    after.strip_prefix(' ').unwrap_or(after).to_owned()
}

fn strip_multi_body(raw: &str) -> String {
    let stripped: Vec<String> = raw
        .lines()
        .map(|l| {
            let t = l.trim_start();
            let s = t.strip_prefix('*').unwrap_or(t);
            s.strip_prefix(' ').unwrap_or(s).to_owned()
        })
        .collect();
    stripped.join("\n").trim().to_owned()
}

fn line_no(i: usize) -> u32 {
    u32::try_from(i + 1).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::CommentDelimiters;

    fn rust_styles() -> CommentStyles {
        CommentStyles {
            single: vec!["//".to_owned()],
            multi: Some(CommentDelimiters {
                start: "/*".to_owned(),
                end: "*/".to_owned(),
            }),
            doc_single: vec!["///".to_owned(), "//!".to_owned()],
            doc_multi: Some(CommentDelimiters {
                start: "/**".to_owned(),
                end: "*/".to_owned(),
            }),
        }
    }

    fn python_styles() -> CommentStyles {
        CommentStyles {
            single: vec!["#".to_owned()],
            multi: None,
            doc_single: vec![],
            doc_multi: None,
        }
    }

    #[test]
    fn single_line_run_groups_consecutive_lines() {
        let src = "// first\n// second\n// third\nfn foo() {}\n";
        let spans = scan(src, &rust_styles());
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].style, CommentStyle::SingleLine);
        assert_eq!(spans[0].line_start, 1);
        assert_eq!(spans[0].line_end, 3);
        assert_eq!(spans[0].column, 1);
        assert_eq!(spans[0].body, "first\nsecond\nthird");
    }

    #[test]
    fn doc_single_separates_from_plain_single() {
        let src = "/// doc above\n// plain below\nfn foo() {}\n";
        let spans = scan(src, &rust_styles());
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].style, CommentStyle::DocSingle);
        assert_eq!(spans[0].body, "doc above");
        assert_eq!(spans[1].style, CommentStyle::SingleLine);
        assert_eq!(spans[1].body, "plain below");
    }

    #[test]
    fn doc_inner_distinct_from_doc_outer() {
        let src = "//! inner one\n//! inner two\n/// outer one\nfn foo() {}\n";
        let spans = scan(src, &rust_styles());
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].body, "inner one\ninner two");
        assert_eq!(spans[1].body, "outer one");
    }

    #[test]
    fn multi_line_block_one_span() {
        let src = "/* hello\n   world */\nfn foo() {}\n";
        let spans = scan(src, &rust_styles());
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].style, CommentStyle::MultiLine);
        assert_eq!(spans[0].line_start, 1);
        assert_eq!(spans[0].line_end, 2);
        assert_eq!(spans[0].body, "hello\nworld");
    }

    #[test]
    fn jsdoc_strips_leading_star() {
        let src = "/**\n * first\n * second\n */\nfn foo() {}\n";
        let spans = scan(src, &rust_styles());
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].style, CommentStyle::DocMulti);
        assert_eq!(spans[0].body, "first\nsecond");
    }

    #[test]
    fn single_line_block_short_form() {
        let src = "/* one-shot */\nfn foo() {}\n";
        let spans = scan(src, &rust_styles());
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].body, "one-shot");
        assert_eq!(spans[0].line_start, 1);
        assert_eq!(spans[0].line_end, 1);
    }

    #[test]
    fn empty_line_breaks_run() {
        let src = "// a\n\n// b\nfn foo() {}\n";
        let spans = scan(src, &rust_styles());
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].body, "a");
        assert_eq!(spans[1].body, "b");
        assert_eq!(spans[1].line_start, 3);
    }

    #[test]
    fn python_hash_comments() {
        let src = "# first\n# second\ndef foo():\n    pass\n";
        let spans = scan(src, &python_styles());
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].style, CommentStyle::SingleLine);
        assert_eq!(spans[0].body, "first\nsecond");
    }

    #[test]
    fn indented_comment_records_column() {
        let src = "    // indented\nfn foo() {}\n";
        let spans = scan(src, &rust_styles());
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].column, 5);
        assert_eq!(spans[0].body, "indented");
    }

    #[test]
    fn no_match_returns_empty() {
        let src = "fn foo() {}\nlet x = 1;\n";
        let spans = scan(src, &rust_styles());
        assert!(spans.is_empty());
    }

    #[test]
    fn unterminated_multi_recovers() {
        let src = "/* never closed\nstill text\n";
        let spans = scan(src, &rust_styles());
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].style, CommentStyle::MultiLine);
        assert_eq!(spans[0].body, "never closed\nstill text");
    }

    #[test]
    fn satellite_directive_round_trip() {
        // Exercise the full intent: a free-floating block carrying a
        // satellite directive should land in the body untouched.
        let src = "// @doc-extend validator.rules.std002 extractor\n// @description Emitted by the extractor on malformed @tag.\n";
        let spans = scan(src, &rust_styles());
        assert_eq!(spans.len(), 1);
        assert_eq!(
            spans[0].body,
            "@doc-extend validator.rules.std002 extractor\n@description Emitted by the extractor on malformed @tag."
        );
    }
}

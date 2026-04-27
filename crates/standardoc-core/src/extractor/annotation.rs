//! Parses `@tag` annotations out of a raw (already-cleaned) comment string.
//!
//! Language providers strip comment prefixes (`///`, `*`, `#`, ...) before
//! handing the comment to the extractor — this parser assumes plain text.

use crate::model::{TagFields, TagName};
use std::collections::BTreeMap;

/// Canonical tags with multi-line bodies: content keeps running until the
/// next `@tag` or the end of the comment. All other tags are single-line.
/// `schema` carries JSON (often with embedded whitespace) and so must be
/// preserved as a single blob rather than whitespace-split into fields.
const MULTILINE_TAGS: &[&str] = &["example", "description", "schema"];

/// Result of parsing an annotation block.
///
/// `tags` is a flat map of tag name → occurrences, each occurrence a
/// positional field vector. Field conventions per tag:
/// - `@<doc_tag> key [label]` → `[key, label?]` (label can contain spaces)
/// - `@param name type description` → `[name, type, description]`
/// - `@returns type description` → `[type, description]`
/// - `@example` / `@description` (multi-line) → `[body]`
/// - anything else → whitespace-split
///
/// `warnings` collects the **non-blocking** syntax violations detected
/// during the parse — `@doc` without key, `@param` without name, etc.
/// The caller (extractor) promotes them to STD002 diagnostics attached
/// to the block. We surface them instead of swallowing them so the
/// validator can show them to the user (see rule STD002).
#[derive(Debug, Default, Clone)]
pub struct Annotations {
    pub tags: BTreeMap<TagName, Vec<TagFields>>,
    pub warnings: Vec<AnnotationWarning>,
}

/// A `@tag` syntax violation detected during the parse.
///
/// The position is approximate — we keep the line offset within the
/// comment (0-based) so the extractor can compute the absolute line in
/// the source file.
#[derive(Debug, Clone)]
pub struct AnnotationWarning {
    pub message: String,
    /// 0-based line offset within the comment block that triggered the
    /// warning.
    pub line_offset: u32,
}

impl Annotations {
    pub fn has(&self, name: &str) -> bool {
        self.tags.contains_key(name)
    }

    pub fn first(&self, name: &str) -> Option<&TagFields> {
        self.tags.get(name).and_then(|v| v.first())
    }

    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
    }
}

pub fn parse(content: &str, doc_tag: &str) -> Annotations {
    let mut annotations = Annotations::default();
    let lines: Vec<&str> = content.lines().collect();

    let first_tag_idx = lines
        .iter()
        .position(|l| match_tag_line(l.trim_start()).is_some());

    let has_explicit_description = lines
        .iter()
        .any(|l| match_tag_line(l.trim_start()).is_some_and(|(tag, _)| tag == "description"));

    if !has_explicit_description {
        let prose_end = first_tag_idx.unwrap_or(lines.len());
        let prose = lines[..prose_end]
            .iter()
            .map(|l| l.trim())
            .collect::<Vec<_>>()
            .join("\n");
        let prose = prose.trim().to_owned();
        if !prose.is_empty() {
            annotations
                .tags
                .entry("description".to_owned())
                .or_default()
                .push(vec![prose]);
        }
    }

    let mut i = first_tag_idx.unwrap_or(lines.len());
    while i < lines.len() {
        let line = lines[i].trim_start();
        let Some((tag, rest)) = match_tag_line(line) else {
            i += 1;
            continue;
        };

        let line_offset = u32::try_from(i).unwrap_or(u32::MAX);
        if is_multiline(&tag) {
            let mut body = rest.to_owned();
            let mut j = i + 1;
            while j < lines.len() {
                let next = lines[j].trim_start();
                if match_tag_line(next).is_some() {
                    break;
                }
                body.push('\n');
                body.push_str(lines[j]);
                j += 1;
            }
            push_fields(&mut annotations, &tag, body.trim(), doc_tag, line_offset);
            i = j;
        } else {
            push_fields(&mut annotations, &tag, rest.trim(), doc_tag, line_offset);
            i += 1;
        }
    }

    annotations
}

fn match_tag_line(line: &str) -> Option<(String, &str)> {
    let bytes = line.as_bytes();
    if bytes.first() != Some(&b'@') {
        return None;
    }
    let rest = &line[1..];
    // Tag names accept alphanumeric / `_` / `.` plus `-` (e.g. `@doc-extend`,
    // `@args-schema`). The leading char must be alphanumeric or `_` so a
    // stray `@-foo` doesn't masquerade as a tag.
    let first = rest.chars().next()?;
    if !(first.is_ascii_alphanumeric() || first == '_') {
        return None;
    }
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-'))
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    let tag = rest[..end].to_owned();
    let remainder = rest[end..].trim_start();
    Some((tag, remainder))
}

fn is_multiline(tag: &str) -> bool {
    MULTILINE_TAGS.contains(&tag)
}

fn push_fields(
    annotations: &mut Annotations,
    tag: &str,
    content: &str,
    doc_tag: &str,
    line_offset: u32,
) {
    let fields = if tag == doc_tag {
        let f = parse_doc_fields(content);
        // `@doc` with no key (just the tag, nothing after) — the user
        // forgot the essential argument. We let it through (the extractor
        // will infer the key from the signature) but we flag it.
        if f.is_empty() {
            annotations.warnings.push(AnnotationWarning {
                message: format!("`@{tag}` has no key — expected `@{tag} <key> [label]`"),
                line_offset,
            });
        }
        f
    } else {
        match tag {
            "param" => {
                let f = parse_param(content);
                if f.is_empty() || f[0].is_empty() {
                    annotations.warnings.push(AnnotationWarning {
                        message:
                            "`@param` has no name — expected `@param <name> [type] [description]`"
                                .to_owned(),
                        line_offset,
                    });
                }
                f
            }
            "returns" | "return" => {
                let f = parse_returns(content);
                if f.is_empty() {
                    annotations.warnings.push(AnnotationWarning {
                        message: format!(
                            "`@{tag}` has no type — expected `@{tag} <type> [description]`"
                        ),
                        line_offset,
                    });
                }
                f
            }
            t if is_multiline(t) => {
                if content.is_empty() {
                    Vec::new()
                } else {
                    vec![content.to_owned()]
                }
            }
            _ => content.split_whitespace().map(ToOwned::to_owned).collect(),
        }
    };

    annotations
        .tags
        .entry(tag.to_owned())
        .or_default()
        .push(fields);
}

fn parse_doc_fields(content: &str) -> Vec<String> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    trimmed.find(char::is_whitespace).map_or_else(
        || vec![trimmed.to_owned()],
        |space| {
            let key = trimmed[..space].to_owned();
            let label = trimmed[space..].trim().to_owned();
            if label.is_empty() {
                vec![key]
            } else {
                vec![key, label]
            }
        },
    )
}

fn parse_param(content: &str) -> Vec<String> {
    let mut parts = content.splitn(3, char::is_whitespace);
    let raw_name = parts.next().unwrap_or("").trim();
    // LuaCATS / EmmyLua convention: `@param x? string` marks the parameter
    // optional via a trailing `?` on the name. We strip it to normalize
    // the match against the AST signature (which has no `?`). We lose
    // the "optional" info for now — TODO: store is_optional in a dedicated
    // field if the user wants to surface it in the DSL.
    let name = raw_name.trim_end_matches('?');
    let ty = parts.next().unwrap_or("").trim();
    let desc = parts.next().unwrap_or("").trim();
    let mut out = Vec::new();
    if !name.is_empty() {
        out.push(name.to_owned());
    }
    if !ty.is_empty() {
        out.push(ty.to_owned());
    }
    if !desc.is_empty() {
        out.push(desc.to_owned());
    }
    out
}

fn parse_returns(content: &str) -> Vec<String> {
    let mut parts = content.splitn(2, char::is_whitespace);
    let ty = parts.next().unwrap_or("").trim();
    let desc = parts.next().unwrap_or("").trim();
    let mut out = Vec::new();
    if !ty.is_empty() {
        out.push(ty.to_owned());
    }
    if !desc.is_empty() {
        out.push(desc.to_owned());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_doc_with_key_and_label() {
        let anno = parse("@doc my_key My Nice Label", "doc");
        let fields = anno.first("doc").unwrap();
        assert_eq!(
            fields,
            &vec!["my_key".to_owned(), "My Nice Label".to_owned()]
        );
    }

    #[test]
    fn parses_doc_with_key_only() {
        let anno = parse("@doc my_key", "doc");
        let fields = anno.first("doc").unwrap();
        assert_eq!(fields, &vec!["my_key".to_owned()]);
    }

    #[test]
    fn parses_param_with_three_fields() {
        let anno = parse("@param a number First arg", "doc");
        let fields = anno.first("param").unwrap();
        assert_eq!(
            fields,
            &vec!["a".to_owned(), "number".to_owned(), "First arg".to_owned()]
        );
    }

    #[test]
    fn parses_multiple_params() {
        let anno = parse("@param a number First\n@param b string Second", "doc");
        let params = &anno.tags["param"];
        assert_eq!(params.len(), 2);
        assert_eq!(params[0][0], "a");
        assert_eq!(params[1][0], "b");
    }

    #[test]
    fn description_is_multiline() {
        let src = "@description First line\nSecond line\nthird line\n@param a number X";
        let anno = parse(src, "doc");
        let desc = anno.first("description").unwrap();
        assert_eq!(desc.len(), 1);
        assert!(desc[0].contains("First line"));
        assert!(desc[0].contains("third line"));
    }

    #[test]
    fn custom_doc_tag_name_is_respected() {
        let anno = parse("@standardoc my_key Label", "standardoc");
        assert!(anno.has("standardoc"));
        assert!(!anno.has("doc"));
    }

    #[test]
    fn prose_before_doc_plus_doc_tag_yields_two_entries() {
        // Prose before @doc becomes implicit description.
        let anno = parse("Just some prose.\n@doc k l\nMore prose.", "doc");
        assert_eq!(anno.tags.len(), 2);
        assert!(anno.has("doc"));
        assert!(anno.has("description"));
        assert_eq!(anno.first("description").unwrap()[0], "Just some prose.");
    }

    #[test]
    fn returns_has_two_fields() {
        let anno = parse("@returns number the sum", "doc");
        let fields = anno.first("returns").unwrap();
        assert_eq!(fields, &vec!["number".to_owned(), "the sum".to_owned()]);
    }

    #[test]
    fn unknown_tag_splits_on_whitespace() {
        let anno = parse("@since 2.0.0", "doc");
        let fields = anno.first("since").unwrap();
        assert_eq!(fields, &vec!["2.0.0".to_owned()]);
    }

    #[test]
    fn prose_before_first_tag_becomes_implicit_description() {
        let anno = parse("Adds two integers.\n@doc math.add\n@param a i32 x", "doc");
        let desc = anno.first("description").unwrap();
        assert_eq!(desc, &vec!["Adds two integers.".to_owned()]);
    }

    #[test]
    fn prose_only_no_tags_becomes_implicit_description() {
        let anno = parse("This is the whole docstring.\nSecond line.", "doc");
        let desc = anno.first("description").unwrap();
        assert!(desc[0].contains("This is the whole docstring."));
        assert!(desc[0].contains("Second line."));
    }

    #[test]
    fn explicit_description_wins_over_prose() {
        let anno = parse(
            "Implicit prose here.\n@description Explicit description.\n@doc k",
            "doc",
        );
        let desc = anno.tags.get("description").unwrap();
        assert_eq!(desc.len(), 1);
        assert_eq!(desc[0][0], "Explicit description.");
    }

    // -------- STD002 warnings --------

    #[test]
    fn doc_without_key_emits_warning() {
        let anno = parse("@doc", "doc");
        assert_eq!(anno.warnings.len(), 1);
        assert!(anno.warnings[0].message.contains("no key"));
    }

    #[test]
    fn param_without_name_emits_warning() {
        let anno = parse("@doc x\n@param ", "doc");
        let warns: Vec<&AnnotationWarning> = anno
            .warnings
            .iter()
            .filter(|w| w.message.contains("@param"))
            .collect();
        assert_eq!(warns.len(), 1);
    }

    #[test]
    fn returns_without_type_emits_warning() {
        let anno = parse("@doc x\n@returns ", "doc");
        let warns: Vec<&AnnotationWarning> = anno
            .warnings
            .iter()
            .filter(|w| w.message.contains("@returns"))
            .collect();
        assert_eq!(warns.len(), 1);
    }

    #[test]
    fn well_formed_annotation_has_no_warnings() {
        let anno = parse(
            "@doc math.add\n@param a i32 first arg\n@returns i32 sum",
            "doc",
        );
        assert!(anno.warnings.is_empty());
    }

    #[test]
    fn luacats_optional_param_strips_question_mark() {
        // `@param x? string` is the LuaCATS convention for optional params.
        // We strip `?` from the name to match AST signature.
        let anno = parse("@doc foo\n@param x? string optional thing", "doc");
        let p = anno.first("param").unwrap();
        assert_eq!(p[0], "x");
        assert_eq!(p[1], "string");
        assert_eq!(p[2], "optional thing");
    }

    // -------- Hyphenated tag names (satellite directives) --------

    #[test]
    fn hyphenated_tag_doc_extend_parses_as_single_tag() {
        let anno = parse("@doc-extend validator.rules.std002 schema", "doc");
        let fields = anno.first("doc-extend").unwrap();
        assert_eq!(
            fields,
            &vec!["validator.rules.std002".to_owned(), "schema".to_owned()]
        );
        assert!(!anno.has("doc"));
    }

    #[test]
    fn doc_extend_two_args_co_exists_with_other_tags() {
        let anno = parse(
            "@doc-extend mcp.tools.get_doc schema\n@description Fetch a single block.",
            "doc",
        );
        assert_eq!(
            anno.first("doc-extend").unwrap(),
            &vec!["mcp.tools.get_doc".to_owned(), "schema".to_owned()]
        );
        assert_eq!(
            anno.first("description").unwrap(),
            &vec!["Fetch a single block.".to_owned()]
        );
    }

    #[test]
    fn hyphenated_args_schema_field_keeps_hyphen() {
        // The original use case: deport heavy `@args-schema` JSON to a
        // satellite. Just make sure the tag name is preserved as-is.
        let anno = parse("@args-schema {\"type\":\"object\"}", "doc");
        let fields = anno.first("args-schema").unwrap();
        assert_eq!(fields, &vec!["{\"type\":\"object\"}".to_owned()]);
    }

    #[test]
    fn leading_hyphen_or_dot_does_not_form_a_tag() {
        // `@-foo` and `@.foo` shouldn't be picked up as tags — protects
        // against misparses on stray markers.
        let anno = parse("@-foo bar\n@.foo baz", "doc");
        assert!(anno.tags.is_empty() || !anno.has("-foo"));
        assert!(!anno.has(".foo"));
    }
}

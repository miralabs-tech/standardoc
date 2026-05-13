//! Vue `<template>` parser. Lock 41 §1 Q4 supports day-1:
//!
//! - `@event="expr"` / `v-on:event="expr"`     → [`TemplateAttribute::Event`]
//! - `:prop="expr"` / `v-bind:prop="expr"`      → [`TemplateAttribute::Bind`]
//! - `v-if`/`v-else-if`/`v-show`/`v-model="expr"` → [`TemplateAttribute::Directive`]
//! - `v-for="item in collection"` (collection only — `item` is a local def)
//! - `{{ expr }}` interpolation                → [`TemplateAttribute::Interpolation`]
//! - `<MyComp />` (uppercase tag) → [`TemplateAttribute::ComponentRef`]
//!
//! NOT day-1: scoped slots, `<slot>` indirection, CSS `:deep()`,
//! two-way `v-model` lvalue inference, fragment shorthand `<>`.

use super::{TemplateAttribute, TemplateRef, TemplateRefSink, extract_identifiers_from_expression};
use crate::sfc::{find_after, read_tag_name, skip_comment, starts_with};
use crate::utils::find_top_level_keyword;

/// Walks a Vue `<template>` source slice and pushes one [`TemplateRef`]
/// per identifier reference encountered.
///
/// `template_src` is the raw text *between* the opening `<template>` and
/// the closing `</template>` (exclusive). `base_offset` is the byte
/// position of `template_src[0]` within the original SFC source so each
/// emitted [`TemplateRef::byte_offset`] is absolute.
///
/// The function never panics on malformed templates — unknown directive
/// names, unmatched braces and stray quotes are silently skipped to
/// keep the indexer running over WIP edits.
pub(crate) fn parse(template_src: &str, base_offset: usize, sink: &mut dyn TemplateRefSink) {
    let bytes = template_src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if starts_with(bytes, i, b"<!--") {
            i = skip_comment(bytes, i + 4);
            continue;
        }
        if bytes[i] == b'<' {
            let after = i + 1;
            if after < bytes.len() && bytes[after] == b'/' {
                // Closing tag — skip to `>`.
                i = find_after(bytes, after, b'>').unwrap_or(bytes.len());
                continue;
            }
            if after < bytes.len() && bytes[after].is_ascii_alphabetic() {
                let name_end = read_tag_name(bytes, after);
                let tag_name = std::str::from_utf8(&bytes[after..name_end]).unwrap_or("");
                if is_component_ref_tag(tag_name) {
                    sink.push(TemplateRef {
                        name: tag_name.to_string(),
                        attribute: TemplateAttribute::ComponentRef,
                        byte_offset: base_offset + after,
                    });
                }
                i = walk_attributes(bytes, name_end, base_offset, sink);
                continue;
            }
            i += 1;
            continue;
        }
        if i + 1 < bytes.len() && bytes[i] == b'{' && bytes[i + 1] == b'{' {
            let start = i + 2;
            if let Some(end) = find_double_brace_close(bytes, start) {
                if let Ok(expr) = std::str::from_utf8(&bytes[start..end]) {
                    extract_identifiers_from_expression(
                        expr,
                        base_offset + start,
                        TemplateAttribute::Interpolation,
                        sink,
                    );
                }
                i = end + 2;
                continue;
            }
        }
        i += 1;
    }
}

/// Strips a `v-for` loop-variable binding (`item` in `item in list` /
/// `(item, index) in list` / `item of list`) and returns the iterable
/// expression text together with the locals to remove from the
/// per-binding identifier walker.
///
/// On unrecognised shapes returns `(value.to_string(), vec![])` —
/// graceful degradation: the caller forwards the whole expression
/// to the identifier extractor and accepts the iteration var as a
/// potentially-Unresolved reference (cohérent feedback_scope_graph_not_lsp).
pub(crate) fn split_v_for(value: &str) -> (String, Vec<String>) {
    // Match either ` in ` or ` of ` (Vue 3 supports both).
    let trimmed = value.trim();
    let split_at =
        find_top_level_keyword(trimmed, " in ").or_else(|| find_top_level_keyword(trimmed, " of "));
    let Some((kw_pos, kw_len)) = split_at else {
        return (trimmed.to_string(), Vec::new());
    };
    let lhs = trimmed[..kw_pos].trim();
    let rhs = trimmed[kw_pos + kw_len..].trim();
    let locals = parse_loop_locals(lhs);
    (rhs.to_string(), locals)
}

// --- helpers --------------------------------------------------------------

fn is_component_ref_tag(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

/// Walks attributes from `from` until the closing `>` (or `/>`) emitting
/// refs for each Vue-recognised binding form. Returns the position
/// just past the closing `>`.
fn walk_attributes(
    bytes: &[u8],
    from: usize,
    base_offset: usize,
    sink: &mut dyn TemplateRefSink,
) -> usize {
    let mut i = from;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b == b'>' {
            return i + 1;
        }
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'>' {
            return i + 2;
        }
        let name_start = i;
        while i < bytes.len() {
            let b = bytes[i];
            if b.is_ascii_whitespace() || b == b'=' || b == b'>' || b == b'/' {
                break;
            }
            i += 1;
        }
        if i == name_start {
            i += 1;
            continue;
        }
        let attr_name = std::str::from_utf8(&bytes[name_start..i])
            .unwrap_or("")
            .to_string();
        let value_span = read_attr_value(bytes, &mut i);
        if let Some((value, value_start)) = value_span
            && let Some(attribute) = classify_attribute(&attr_name)
        {
            emit_attribute_refs(
                &attr_name,
                &value,
                value_start,
                base_offset,
                attribute,
                sink,
            );
        }
    }
    bytes.len()
}

/// On entry, `*i` points to the byte after the attribute name. Returns
/// `Some((value, byte_offset_of_value_start))` when the attribute had a
/// `=` followed by a quoted value; `None` for bare attributes (`disabled`)
/// or unquoted values (we don't extract from those — Vue almost always
/// quotes binding values and unquoted shapes are typically static
/// strings like `id=foo`).
fn read_attr_value(bytes: &[u8], i: &mut usize) -> Option<(String, usize)> {
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
    if quote != b'"' && quote != b'\'' {
        // Unquoted value — skip without emitting.
        while *i < bytes.len() {
            let b = bytes[*i];
            if b.is_ascii_whitespace() || b == b'>' || b == b'/' {
                break;
            }
            *i += 1;
        }
        return None;
    }
    *i += 1;
    let start = *i;
    while *i < bytes.len() && bytes[*i] != quote {
        *i += 1;
    }
    let end = *i;
    if *i < bytes.len() {
        *i += 1;
    }
    let value = std::str::from_utf8(&bytes[start..end]).ok()?.to_string();
    Some((value, start))
}

fn classify_attribute(name: &str) -> Option<TemplateAttribute> {
    if name.starts_with('@') || name.starts_with("v-on:") || name.starts_with("v-on") {
        return Some(TemplateAttribute::Event);
    }
    if name.starts_with(':') || name.starts_with("v-bind:") || name == "v-bind" {
        return Some(TemplateAttribute::Bind);
    }
    if name == "v-if"
        || name == "v-else-if"
        || name == "v-show"
        || name == "v-model"
        || name == "v-for"
        || name.starts_with("v-model:")
    {
        return Some(TemplateAttribute::Directive);
    }
    None
}

fn emit_attribute_refs(
    name: &str,
    value: &str,
    value_start: usize,
    base_offset: usize,
    attribute: TemplateAttribute,
    sink: &mut dyn TemplateRefSink,
) {
    if name == "v-for" {
        let (iterable, _locals) = split_v_for(value);
        // Re-locate the iterable inside the original quoted slice so byte
        // offsets stay accurate — we look for the iterable text starting
        // at the position where the original value contained ` in ` or
        // ` of `. Cheap & robust vs tracking offsets through the splitter.
        let inner_offset = value.find(iterable.as_str()).map_or(0, |pos| pos);
        extract_identifiers_from_expression(
            &iterable,
            base_offset + value_start + inner_offset,
            attribute,
            sink,
        );
        return;
    }
    extract_identifiers_from_expression(value, base_offset + value_start, attribute, sink);
}

fn parse_loop_locals(lhs: &str) -> Vec<String> {
    // Strip optional outer parens `(item, index)`.
    let inner = lhs.trim_start_matches('(').trim_end_matches(')');
    inner
        .split(',')
        .filter_map(|p| {
            let name = p.trim();
            if name.is_empty() {
                None
            } else {
                Some(name.to_string())
            }
        })
        .collect()
}

fn find_double_brace_close(bytes: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 1 < bytes.len() {
        if bytes[i] == b'}' && bytes[i + 1] == b'}' {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(src: &str) -> Vec<TemplateRef> {
        let mut sink: Vec<TemplateRef> = Vec::new();
        parse(src, 0, &mut sink);
        sink
    }

    fn names(refs: &[TemplateRef]) -> Vec<&str> {
        refs.iter().map(|r| r.name.as_str()).collect()
    }

    fn names_with_attr(refs: &[TemplateRef], attr: TemplateAttribute) -> Vec<&str> {
        refs.iter()
            .filter(|r| r.attribute == attr)
            .map(|r| r.name.as_str())
            .collect()
    }

    #[test]
    fn empty_template_yields_no_refs() {
        assert!(collect("").is_empty());
    }

    #[test]
    fn plain_text_yields_no_refs() {
        assert!(collect("<div>hello world</div>").is_empty());
    }

    #[test]
    fn interpolation_emits_inner_idents() {
        let refs = collect("<div>{{ msg }}</div>");
        assert_eq!(names(&refs), vec!["msg"]);
        assert_eq!(refs[0].attribute, TemplateAttribute::Interpolation);
    }

    #[test]
    fn interpolation_with_member_access() {
        let refs = collect("<p>{{ user.name }}</p>");
        let interp = names_with_attr(&refs, TemplateAttribute::Interpolation);
        assert_eq!(interp, vec!["user"]);
    }

    #[test]
    fn at_event_handler_is_emitted_as_event() {
        let refs = collect(r#"<button @click="handleClick">x</button>"#);
        assert_eq!(
            names_with_attr(&refs, TemplateAttribute::Event),
            vec!["handleClick"]
        );
    }

    #[test]
    fn v_on_event_handler_is_emitted_as_event() {
        let refs = collect(r#"<button v-on:click="handleClick">x</button>"#);
        assert_eq!(
            names_with_attr(&refs, TemplateAttribute::Event),
            vec!["handleClick"]
        );
    }

    #[test]
    fn colon_bind_is_emitted_as_bind() {
        let refs = collect(r#"<input :value="model" />"#);
        assert_eq!(
            names_with_attr(&refs, TemplateAttribute::Bind),
            vec!["model"]
        );
    }

    #[test]
    fn v_bind_long_form_is_emitted_as_bind() {
        let refs = collect(r#"<input v-bind:value="model" />"#);
        assert_eq!(
            names_with_attr(&refs, TemplateAttribute::Bind),
            vec!["model"]
        );
    }

    #[test]
    fn v_if_is_emitted_as_directive() {
        let refs = collect(r#"<div v-if="isVisible">x</div>"#);
        assert_eq!(
            names_with_attr(&refs, TemplateAttribute::Directive),
            vec!["isVisible"]
        );
    }

    #[test]
    fn v_show_is_emitted_as_directive() {
        let refs = collect(r#"<div v-show="visible">x</div>"#);
        assert_eq!(
            names_with_attr(&refs, TemplateAttribute::Directive),
            vec!["visible"]
        );
    }

    #[test]
    fn v_model_is_emitted_as_directive() {
        let refs = collect(r#"<input v-model="text" />"#);
        assert_eq!(
            names_with_attr(&refs, TemplateAttribute::Directive),
            vec!["text"]
        );
    }

    #[test]
    fn v_for_extracts_iterable_only() {
        let refs = collect(r#"<li v-for="item in items">{{ item.name }}</li>"#);
        let directive = names_with_attr(&refs, TemplateAttribute::Directive);
        assert!(directive.contains(&"items"));
        // `item` from the body interpolation lands as Interpolation (not
        // Directive), and we don't strip it day-1 — accepted noise.
        let interp = names_with_attr(&refs, TemplateAttribute::Interpolation);
        assert!(interp.contains(&"item"));
    }

    #[test]
    fn v_for_with_paren_index() {
        let refs = collect(r#"<li v-for="(item, i) in items">x</li>"#);
        let directive = names_with_attr(&refs, TemplateAttribute::Directive);
        assert_eq!(directive, vec!["items"]);
    }

    #[test]
    fn v_for_of_form() {
        let refs = collect(r#"<li v-for="item of items">x</li>"#);
        assert_eq!(
            names_with_attr(&refs, TemplateAttribute::Directive),
            vec!["items"]
        );
    }

    #[test]
    fn split_v_for_simple_in_form() {
        let (iter, locals) = split_v_for("item in items");
        assert_eq!(iter, "items");
        assert_eq!(locals, vec!["item"]);
    }

    #[test]
    fn split_v_for_paren_in_form() {
        let (iter, locals) = split_v_for("(item, idx) in items");
        assert_eq!(iter, "items");
        assert_eq!(locals, vec!["item", "idx"]);
    }

    #[test]
    fn split_v_for_no_keyword_returns_value_unchanged() {
        let (iter, locals) = split_v_for("hello");
        assert_eq!(iter, "hello");
        assert!(locals.is_empty());
    }

    #[test]
    fn component_ref_pascal_case_emitted() {
        let refs = collect("<MyComp />");
        assert_eq!(
            names_with_attr(&refs, TemplateAttribute::ComponentRef),
            vec!["MyComp"]
        );
    }

    #[test]
    fn lowercase_html_tag_is_not_a_component_ref() {
        let refs = collect("<div></div>");
        assert!(refs.is_empty());
    }

    #[test]
    fn nested_components_both_captured() {
        let refs = collect("<UserCard><UserBadge /></UserCard>");
        let comps = names_with_attr(&refs, TemplateAttribute::ComponentRef);
        assert!(comps.contains(&"UserCard"));
        assert!(comps.contains(&"UserBadge"));
    }

    #[test]
    fn comment_is_skipped() {
        let refs = collect("<!-- {{ ghost }} --><p>{{ real }}</p>");
        let interp = names_with_attr(&refs, TemplateAttribute::Interpolation);
        assert_eq!(interp, vec!["real"]);
    }

    #[test]
    fn unmatched_double_brace_is_skipped() {
        let refs = collect("<p>{{ ghost</p>");
        assert!(refs.is_empty());
    }

    #[test]
    fn multiple_attrs_each_emit_their_attribute_kind() {
        let src = r#"<button @click="onClick" :disabled="loading" v-if="show">x</button>"#;
        let refs = collect(src);
        let by_attr: std::collections::HashMap<&str, TemplateAttribute> = refs
            .iter()
            .map(|r| (r.name.as_str(), r.attribute))
            .collect();
        assert_eq!(by_attr["onClick"], TemplateAttribute::Event);
        assert_eq!(by_attr["loading"], TemplateAttribute::Bind);
        assert_eq!(by_attr["show"], TemplateAttribute::Directive);
    }

    #[test]
    fn unquoted_attribute_value_is_not_extracted() {
        // Unquoted values are typically static IDs/classes — we skip
        // expression extraction to avoid false positives.
        let refs = collect("<div id=foo class=bar>x</div>");
        assert!(refs.is_empty());
    }

    #[test]
    fn bare_attribute_without_value_is_skipped() {
        let refs = collect("<button disabled>x</button>");
        assert!(refs.is_empty());
    }

    #[test]
    fn byte_offset_points_into_original_template() {
        let src = "<p>{{ msg }}</p>";
        // 'msg' starts at offset 6 ("<p>{{ ".len() == 6).
        let refs = collect(src);
        assert_eq!(refs[0].byte_offset, 6);
    }

    #[test]
    fn base_offset_is_added() {
        let mut sink: Vec<TemplateRef> = Vec::new();
        parse("<p>{{ msg }}</p>", 1000, &mut sink);
        assert_eq!(sink[0].byte_offset, 1006);
    }

    #[test]
    fn closing_tag_does_not_emit_component_ref() {
        // `</MyComp>` — closing tags are skipped wholesale.
        let refs = collect("<MyComp></MyComp>");
        let comps = names_with_attr(&refs, TemplateAttribute::ComponentRef);
        assert_eq!(comps, vec!["MyComp"]);
    }

    #[test]
    fn directive_value_with_member_access_emits_root() {
        let refs = collect(r#"<div v-if="user.isActive">x</div>"#);
        assert_eq!(
            names_with_attr(&refs, TemplateAttribute::Directive),
            vec!["user"]
        );
    }

    #[test]
    fn event_handler_with_inline_call() {
        let refs = collect(r#"<button @click="handle(payload)">x</button>"#);
        let events = names_with_attr(&refs, TemplateAttribute::Event);
        assert!(events.contains(&"handle"));
        assert!(events.contains(&"payload"));
    }

    #[test]
    fn v_else_if_is_directive() {
        let refs = collect(r#"<div v-if="a">x</div><div v-else-if="b">y</div>"#);
        let dirs = names_with_attr(&refs, TemplateAttribute::Directive);
        assert!(dirs.contains(&"a"));
        assert!(dirs.contains(&"b"));
    }
}

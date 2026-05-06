//! Svelte template parser. Lock 41 §1 Q4 supports day-1:
//!
//! - `on:event={expr}` / `on:event|modifiers={expr}` (Svelte 4) and
//!   `onevent={expr}` (Svelte 5) → [`TemplateAttribute::Event`]
//! - `bind:prop={expr}`                              → [`TemplateAttribute::Bind`]
//! - `{#if expr}` / `{:else if expr}` /
//!   `{#each expr as item, i (key)}` / `{#await expr}` → [`TemplateAttribute::Directive`]
//! - `{expr}` interpolation                          → [`TemplateAttribute::Interpolation`]
//! - `{@const x = expr}` / `{@html expr}` /
//!   `{@render snippet(args)}` (Svelte 5)            → [`TemplateAttribute::Interpolation`]
//! - `<MyComp />` (uppercase tag)                    → [`TemplateAttribute::ComponentRef`]
//!
//! NOT day-1: `$store` auto-subscribe semantics (the `$store` token is
//! emitted as a plain identifier reference), `<svelte:fragment>` slot
//! indirection, label statement `$:` reactive deps, snippet definition
//! capture as symbols (the `{#snippet name(args)}` form will get a
//! follow-up track post-beta.2).

use super::{TemplateAttribute, TemplateRef, TemplateRefSink, extract_identifiers_from_expression};
use crate::sfc::{find_after, read_tag_name, skip_comment, starts_with};
use crate::utils::find_top_level_keyword;

/// Walks a Svelte template source slice and pushes one [`TemplateRef`]
/// per identifier reference encountered.
///
/// Same contract as [`crate::template::vue::parse`] — `base_offset` is
/// the absolute byte position of `template_src[0]` within the original
/// SFC source, so every emitted [`TemplateRef::byte_offset`] stays in
/// the SFC's coordinate system without post-processing.
///
/// Svelte (unlike Vue) doesn't wrap its template in a dedicated
/// `<template>` block — the SFC parser passes the whole "outside the
/// `<script>` and `<style>` blocks" region through here. The walker is
/// expected to ignore plain text and only recognise the
/// `{...}` / `{#...}` / `{@...}` Svelte syntax forms plus tag
/// attributes.
pub(crate) fn parse(
    template_src: &str,
    base_offset: usize,
    sink: &mut dyn TemplateRefSink,
) {
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
        if bytes[i] == b'{' {
            // Svelte uses single-brace blocks. Skip closing block markers
            // (`{/each}`, `{:else}` without expression) and `{#snippet`
            // definitions (no expression-side refs day-1, snippet name is
            // a local def). Other shapes feed extract_identifiers.
            let after = i + 1;
            let next = bytes.get(after).copied();
            match next {
                Some(b'/') => {
                    if let Some(end) = find_brace_close(bytes, after) {
                        i = end + 1;
                    } else {
                        i = bytes.len();
                    }
                    continue;
                }
                Some(b'#' | b':') => {
                    i = consume_block_marker(bytes, after, base_offset, sink);
                    continue;
                }
                Some(b'@') => {
                    i = consume_at_block(bytes, after, base_offset, sink);
                    continue;
                }
                _ => {}
            }
            // Plain `{expr}` interpolation.
            if let Some(end) = find_brace_close(bytes, after) {
                if let Ok(expr) = std::str::from_utf8(&bytes[after..end]) {
                    extract_identifiers_from_expression(
                        expr,
                        base_offset + after,
                        TemplateAttribute::Interpolation,
                        sink,
                    );
                }
                i = end + 1;
                continue;
            }
        }
        i += 1;
    }
}

/// Strips an `{#each}` loop-variable binding from `each_clause`
/// (`expr as item, index (key)`) and returns the iterable expression
/// text plus the locals that the per-binding identifier walker must
/// skip when emitting refs from the iterable's inner uses.
pub(crate) fn split_each_clause(each_clause: &str) -> (String, Vec<String>) {
    let trimmed = each_clause.trim();
    // First strip an optional `(key)` suffix.
    let (head, _key) = strip_trailing_paren(trimmed);
    // Then split on top-level ` as `.
    let head = head.trim();
    let Some((kw_pos, kw_len)) = find_top_level_keyword(head, " as ") else {
        return (head.to_string(), Vec::new());
    };
    let iterable = head[..kw_pos].trim().to_string();
    let locals_part = head[kw_pos + kw_len..].trim();
    let locals = locals_part
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split(',')
        .filter_map(|p| {
            let n = p.trim();
            if n.is_empty() { None } else { Some(n.to_string()) }
        })
        .collect();
    (iterable, locals)
}

// --- helpers --------------------------------------------------------------

fn is_component_ref_tag(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|c| c.is_ascii_uppercase())
}

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
        if let Some((value, value_start, was_brace)) = value_span {
            let attribute = classify_attribute(&attr_name, was_brace);
            if let (Some(attr), true) = (attribute, was_brace) {
                extract_identifiers_from_expression(
                    &value,
                    base_offset + value_start,
                    attr,
                    sink,
                );
            }
        }
    }
    bytes.len()
}

/// Returns `Some((value, start_offset, was_brace_form))`.
/// `was_brace_form == true` when the value was `{expr}` (Svelte
/// expression value), `false` for quoted literals or bare values.
fn read_attr_value(bytes: &[u8], i: &mut usize) -> Option<(String, usize, bool)> {
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
    let first = bytes[*i];
    if first == b'{' {
        *i += 1;
        let start = *i;
        if let Some(end) = find_brace_close(bytes, start) {
            let value = std::str::from_utf8(&bytes[start..end]).ok()?.to_string();
            *i = end + 1;
            return Some((value, start, true));
        }
        return None;
    }
    if first == b'"' || first == b'\'' {
        let quote = first;
        *i += 1;
        let start = *i;
        while *i < bytes.len() && bytes[*i] != quote {
            *i += 1;
        }
        let value = std::str::from_utf8(&bytes[start..*i]).ok()?.to_string();
        if *i < bytes.len() {
            *i += 1;
        }
        return Some((value, start, false));
    }
    // Unquoted bare value.
    let start = *i;
    while *i < bytes.len() {
        let b = bytes[*i];
        if b.is_ascii_whitespace() || b == b'>' || b == b'/' {
            break;
        }
        *i += 1;
    }
    let value = std::str::from_utf8(&bytes[start..*i]).ok()?.to_string();
    Some((value, start, false))
}

fn classify_attribute(name: &str, was_brace_value: bool) -> Option<TemplateAttribute> {
    if name.starts_with("on:") {
        return Some(TemplateAttribute::Event);
    }
    if name.starts_with("bind:") {
        return Some(TemplateAttribute::Bind);
    }
    // Svelte 5 `onclick={...}` / `oninput={...}` etc. Heuristic: starts
    // with `on` + lowercase letter + total length > 2, AND the value is a
    // brace expression. Static attrs like `online="true"` keep their bare
    // text classification (no extraction).
    if was_brace_value
        && name.len() > 2
        && name.starts_with("on")
        && name.as_bytes().get(2).is_some_and(u8::is_ascii_lowercase)
    {
        return Some(TemplateAttribute::Event);
    }
    if was_brace_value {
        return Some(TemplateAttribute::Bind);
    }
    None
}

/// Handles `{#if expr}`, `{#each ... as ...}`, `{#await expr}`,
/// `{:else if expr}`, `{#snippet name(...)}`. Returns the position
/// just past the closing `}` of the block marker.
fn consume_block_marker(
    bytes: &[u8],
    after_brace: usize,
    base_offset: usize,
    sink: &mut dyn TemplateRefSink,
) -> usize {
    let Some(end) = find_brace_close(bytes, after_brace) else {
        return bytes.len();
    };
    let inner = std::str::from_utf8(&bytes[after_brace..end]).unwrap_or("");
    // inner starts with `#` or `:` followed by a keyword.
    let after_marker = &inner[1..];
    // Identify the keyword.
    let kw_end = after_marker
        .find(|c: char| c.is_ascii_whitespace())
        .unwrap_or(after_marker.len());
    let keyword = &after_marker[..kw_end];
    let rest = after_marker[kw_end..].trim_start();
    let rest_offset = after_brace + 1 + kw_end + count_leading_ws(&after_marker[kw_end..]);
    match keyword {
        "if" if !rest.is_empty() => {
            extract_identifiers_from_expression(
                rest,
                base_offset + rest_offset,
                TemplateAttribute::Directive,
                sink,
            );
        }
        "else" => {
            // Svelte writes `{:else if cond}` (NOT `{:elseif cond}`). The
            // `else` keyword on its own takes no expression — strip the
            // optional `if ` prefix from `rest` before handing the
            // remainder to swc, otherwise the parser sees `if cond` and
            // chokes on the keyword.
            if let Some(after_if) = rest.strip_prefix("if ") {
                let after_if = after_if.trim_start();
                let inner_offset = rest.len() - after_if.len();
                extract_identifiers_from_expression(
                    after_if,
                    base_offset + rest_offset + inner_offset,
                    TemplateAttribute::Directive,
                    sink,
                );
            }
        }
        "await" => {
            // `await expr` or `await expr then result` or `await expr catch err`.
            let expr = strip_then_or_catch(rest);
            extract_identifiers_from_expression(
                expr,
                base_offset + rest_offset,
                TemplateAttribute::Directive,
                sink,
            );
        }
        "each" => {
            let (iterable, _locals) = split_each_clause(rest);
            // Re-locate iterable inside `rest` for the byte offset.
            let inner_offset = rest.find(iterable.as_str()).unwrap_or(0);
            extract_identifiers_from_expression(
                &iterable,
                base_offset + rest_offset + inner_offset,
                TemplateAttribute::Directive,
                sink,
            );
        }
        "key" => {
            extract_identifiers_from_expression(
                rest,
                base_offset + rest_offset,
                TemplateAttribute::Directive,
                sink,
            );
        }
        // {#snippet name(args)} — name + arg names are local defs day-1.
        // {:else} bare — no expr.
        // {:then result}, {:catch err} — the rest is a local def.
        _ => {}
    }
    end + 1
}

/// Handles `{@const x = expr}` / `{@html expr}` / `{@render fn(args)}` —
/// emits Interpolation refs for the right-hand expression.
fn consume_at_block(
    bytes: &[u8],
    after_brace: usize,
    base_offset: usize,
    sink: &mut dyn TemplateRefSink,
) -> usize {
    let Some(end) = find_brace_close(bytes, after_brace) else {
        return bytes.len();
    };
    let inner = std::str::from_utf8(&bytes[after_brace..end]).unwrap_or("");
    // inner starts with `@` then keyword + space + payload.
    let after_at = &inner[1..];
    let kw_end = after_at
        .find(|c: char| c.is_ascii_whitespace())
        .unwrap_or(after_at.len());
    let keyword = &after_at[..kw_end];
    let rest = after_at[kw_end..].trim_start();
    let rest_offset = after_brace + 1 + kw_end + count_leading_ws(&after_at[kw_end..]);
    match keyword {
        "html" | "render" => {
            extract_identifiers_from_expression(
                rest,
                base_offset + rest_offset,
                TemplateAttribute::Interpolation,
                sink,
            );
        }
        "const" => {
            // `{@const x = expr}` — drop the `name = ` prefix, extract expr.
            if let Some(eq_pos) = rest.find('=') {
                let expr = rest[eq_pos + 1..].trim_start();
                let expr_offset = rest_offset + eq_pos + 1 + count_leading_ws(&rest[eq_pos + 1..]);
                extract_identifiers_from_expression(
                    expr,
                    base_offset + expr_offset,
                    TemplateAttribute::Interpolation,
                    sink,
                );
            }
        }
        _ => {}
    }
    end + 1
}

/// Finds the closing `}` of a Svelte block, balancing nested `{...}`
/// expressions. Returns the index of the matching `}`.
fn find_brace_close(bytes: &[u8], from: usize) -> Option<usize> {
    let mut depth = 1i32;
    let mut in_quote: Option<u8> = None;
    let mut i = from;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = in_quote {
            if b == q {
                in_quote = None;
            }
            i += 1;
            continue;
        }
        match b {
            b'\'' | b'"' | b'`' => in_quote = Some(b),
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn strip_trailing_paren(s: &str) -> (&str, Option<&str>) {
    let trimmed = s.trim_end();
    if let Some(stripped) = trimmed.strip_suffix(')') {
        // Walk back to matching `(`.
        let mut depth = 1i32;
        let bytes = stripped.as_bytes();
        let mut i = bytes.len();
        while i > 0 {
            i -= 1;
            match bytes[i] {
                b')' => depth += 1,
                b'(' => {
                    depth -= 1;
                    if depth == 0 {
                        let head = &stripped[..i];
                        let key = &stripped[i + 1..];
                        return (head, Some(key));
                    }
                }
                _ => {}
            }
        }
    }
    (s, None)
}

fn strip_then_or_catch(s: &str) -> &str {
    let s = s.trim();
    if let Some(pos) = find_top_level_keyword(s, " then ") {
        return s[..pos.0].trim();
    }
    if let Some(pos) = find_top_level_keyword(s, " catch ") {
        return s[..pos.0].trim();
    }
    s
}

fn count_leading_ws(s: &str) -> usize {
    s.bytes().take_while(u8::is_ascii_whitespace).count()
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
        assert!(collect("<div>hi</div>").is_empty());
    }

    #[test]
    fn single_brace_interpolation() {
        let refs = collect("<p>{msg}</p>");
        assert_eq!(names_with_attr(&refs, TemplateAttribute::Interpolation), vec!["msg"]);
    }

    #[test]
    fn member_access_emits_root_only() {
        let refs = collect("<p>{user.name}</p>");
        assert_eq!(names_with_attr(&refs, TemplateAttribute::Interpolation), vec!["user"]);
    }

    #[test]
    fn on_event_handler_svelte4() {
        let refs = collect(r"<button on:click={handleClick}>x</button>");
        assert_eq!(names_with_attr(&refs, TemplateAttribute::Event), vec!["handleClick"]);
    }

    #[test]
    fn on_event_with_modifiers() {
        let refs = collect(r"<button on:click|preventDefault={handle}>x</button>");
        assert_eq!(names_with_attr(&refs, TemplateAttribute::Event), vec!["handle"]);
    }

    #[test]
    fn onevent_svelte5() {
        let refs = collect(r"<button onclick={handle}>x</button>");
        assert_eq!(names_with_attr(&refs, TemplateAttribute::Event), vec!["handle"]);
    }

    #[test]
    fn bind_prop() {
        let refs = collect(r"<input bind:value={text} />");
        assert_eq!(names_with_attr(&refs, TemplateAttribute::Bind), vec!["text"]);
    }

    #[test]
    fn plain_brace_attribute_is_bind() {
        let refs = collect(r#"<img src={url} alt="static" />"#);
        // `src={url}` → Bind. `alt="static"` → no extraction.
        assert_eq!(names_with_attr(&refs, TemplateAttribute::Bind), vec!["url"]);
    }

    #[test]
    fn if_block_directive() {
        let refs = collect("{#if visible}<p>x</p>{/if}");
        assert_eq!(names_with_attr(&refs, TemplateAttribute::Directive), vec!["visible"]);
    }

    #[test]
    fn else_if_block_directive() {
        let refs = collect("{#if a}1{:else if b}2{/if}");
        let dirs = names_with_attr(&refs, TemplateAttribute::Directive);
        assert!(dirs.contains(&"a"));
        assert!(dirs.contains(&"b"));
    }

    #[test]
    fn each_block_extracts_iterable_only() {
        let refs = collect("{#each items as item}{item.name}{/each}");
        let dirs = names_with_attr(&refs, TemplateAttribute::Directive);
        assert!(dirs.contains(&"items"));
    }

    #[test]
    fn each_block_with_index_and_key() {
        let refs = collect("{#each items as item, i (item.id)}{item.name}{/each}");
        let dirs = names_with_attr(&refs, TemplateAttribute::Directive);
        assert!(dirs.contains(&"items"));
    }

    #[test]
    fn await_block_extracts_promise_only() {
        let refs = collect("{#await load() then result}{result}{/await}");
        let dirs = names_with_attr(&refs, TemplateAttribute::Directive);
        assert!(dirs.contains(&"load"));
    }

    #[test]
    fn at_html_extracts_inner_idents() {
        let refs = collect("{@html rendered}");
        assert_eq!(names_with_attr(&refs, TemplateAttribute::Interpolation), vec!["rendered"]);
    }

    #[test]
    fn at_render_extracts_callee_and_args() {
        let refs = collect("{@render row(user)}");
        let interp = names_with_attr(&refs, TemplateAttribute::Interpolation);
        assert!(interp.contains(&"row"));
        assert!(interp.contains(&"user"));
    }

    #[test]
    fn at_const_extracts_rhs_only() {
        let refs = collect("{@const total = price * qty}");
        let interp = names_with_attr(&refs, TemplateAttribute::Interpolation);
        // `total` is the LHS local — not extracted.
        assert!(!interp.contains(&"total"));
        assert!(interp.contains(&"price"));
        assert!(interp.contains(&"qty"));
    }

    #[test]
    fn close_block_marker_is_skipped() {
        let refs = collect("{/each}");
        assert!(refs.is_empty());
    }

    #[test]
    #[allow(clippy::literal_string_with_formatting_args)]
    fn else_bare_is_skipped() {
        let refs = collect("{:else}");
        assert!(refs.is_empty());
    }

    #[test]
    fn component_ref_pascal_case() {
        let refs = collect("<MyComp />");
        assert_eq!(
            names_with_attr(&refs, TemplateAttribute::ComponentRef),
            vec!["MyComp"]
        );
    }

    #[test]
    fn lowercase_html_tag_not_a_component() {
        assert!(collect("<div></div>").is_empty());
    }

    #[test]
    fn split_each_clause_simple() {
        let (iter, locals) = split_each_clause("items as item");
        assert_eq!(iter, "items");
        assert_eq!(locals, vec!["item"]);
    }

    #[test]
    fn split_each_clause_with_index() {
        let (iter, locals) = split_each_clause("items as item, idx");
        assert_eq!(iter, "items");
        assert_eq!(locals, vec!["item", "idx"]);
    }

    #[test]
    fn split_each_clause_with_key() {
        let (iter, locals) = split_each_clause("items as item (item.id)");
        assert_eq!(iter, "items");
        assert_eq!(locals, vec!["item"]);
    }

    #[test]
    fn quoted_attribute_value_is_skipped() {
        let refs = collect(r#"<button class="btn">x</button>"#);
        assert!(refs.is_empty());
    }

    #[test]
    fn unmatched_brace_does_not_panic() {
        let refs = collect("<p>{ghost</p>");
        // Parser drops unmatched expression silently.
        assert!(names(&refs).is_empty());
    }

    #[test]
    fn comment_skipped() {
        let refs = collect("<!-- {hidden} -->{shown}");
        assert_eq!(
            names_with_attr(&refs, TemplateAttribute::Interpolation),
            vec!["shown"]
        );
    }

    #[test]
    fn nested_braces_in_interpolation_are_balanced() {
        // `{ {a, b} }` — the outer is Svelte interpolation of the
        // expression `{a, b}` (an object literal).
        let refs = collect("<p>{ {a, b} }</p>");
        let interp = names_with_attr(&refs, TemplateAttribute::Interpolation);
        assert!(interp.contains(&"a"));
        assert!(interp.contains(&"b"));
    }

    #[test]
    fn byte_offset_added_to_base() {
        let mut sink: Vec<TemplateRef> = Vec::new();
        parse("<p>{msg}</p>", 1000, &mut sink);
        // 'msg' starts at offset 4 (after "<p>{").
        assert_eq!(sink[0].byte_offset, 1004);
    }

    #[test]
    fn multiple_attrs_each_kind() {
        let refs = collect(r"<button on:click={onClick} disabled={loading}>x</button>");
        let by_attr: std::collections::HashMap<&str, TemplateAttribute> =
            refs.iter().map(|r| (r.name.as_str(), r.attribute)).collect();
        assert_eq!(by_attr["onClick"], TemplateAttribute::Event);
        assert_eq!(by_attr["loading"], TemplateAttribute::Bind);
    }

    #[test]
    fn svelte_dollar_store_emitted_as_plain_ident() {
        // `$store` auto-subscribe — emitted as `$store` ident (lock 41
        // §1 Q6). The parser doesn't treat the `$` specially.
        let refs = collect("<p>{$store}</p>");
        let names: Vec<&str> = refs.iter().map(|r| r.name.as_str()).collect();
        // swc parses `$store` as a plain Ident.
        assert!(names.contains(&"$store"));
    }

    #[test]
    fn snippet_definition_does_not_emit_name_as_ref() {
        // `{#snippet row(user)}` — `row` and `user` are local defs.
        let refs = collect("{#snippet row(user)}body{/snippet}");
        let dirs = names_with_attr(&refs, TemplateAttribute::Directive);
        assert!(dirs.is_empty());
    }
}

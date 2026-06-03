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
    assert_eq!(
        names_with_attr(&refs, TemplateAttribute::Interpolation),
        vec!["msg"]
    );
}

#[test]
fn member_access_emits_root_only() {
    let refs = collect("<p>{user.name}</p>");
    assert_eq!(
        names_with_attr(&refs, TemplateAttribute::Interpolation),
        vec!["user"]
    );
}

#[test]
fn on_event_handler_svelte4() {
    let refs = collect(r"<button on:click={handleClick}>x</button>");
    assert_eq!(
        names_with_attr(&refs, TemplateAttribute::Event),
        vec!["handleClick"]
    );
}

#[test]
fn on_event_with_modifiers() {
    let refs = collect(r"<button on:click|preventDefault={handle}>x</button>");
    assert_eq!(
        names_with_attr(&refs, TemplateAttribute::Event),
        vec!["handle"]
    );
}

#[test]
fn onevent_svelte5() {
    let refs = collect(r"<button onclick={handle}>x</button>");
    assert_eq!(
        names_with_attr(&refs, TemplateAttribute::Event),
        vec!["handle"]
    );
}

#[test]
fn bind_prop() {
    let refs = collect(r"<input bind:value={text} />");
    assert_eq!(
        names_with_attr(&refs, TemplateAttribute::Bind),
        vec!["text"]
    );
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
    assert_eq!(
        names_with_attr(&refs, TemplateAttribute::Directive),
        vec!["visible"]
    );
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
    assert_eq!(
        names_with_attr(&refs, TemplateAttribute::Interpolation),
        vec!["rendered"]
    );
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
    let by_attr: std::collections::HashMap<&str, TemplateAttribute> = refs
        .iter()
        .map(|r| (r.name.as_str(), r.attribute))
        .collect();
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

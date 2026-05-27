
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

//! End-to-end tests for the DSL: build realistic `DocBlock`s, render templates
//! against them, and check the rendered output byte-for-byte.

use standardoc_core::config::{Config, TagSchema};
use standardoc_core::dsl::render_string;
use standardoc_core::model::{
    BlockOrigin, CommentStyle, DocBlock, DocKey, DocMeta, ParamInfo, SymbolInfo, SymbolKind,
    Visibility,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn sample_block() -> DocBlock {
    let mut tags: BTreeMap<String, Vec<Vec<String>>> = BTreeMap::new();
    tags.insert(
        "description".to_owned(),
        vec![vec!["Adds two integers together.".to_owned()]],
    );
    tags.insert(
        "param".to_owned(),
        vec![
            vec!["a".to_owned(), "i32".to_owned(), "first operand".to_owned()],
            vec![
                "b".to_owned(),
                "i32".to_owned(),
                "second operand".to_owned(),
            ],
        ],
    );
    tags.insert(
        "returns".to_owned(),
        vec![vec!["i32".to_owned(), "the sum".to_owned()]],
    );
    tags.insert(
        "example".to_owned(),
        vec![vec!["let r = add(1, 2);".to_owned()]],
    );

    DocBlock {
        key: DocKey::new("math.add"),
        label: "add".to_owned(),
        origin: BlockOrigin::Hybrid,
        tags,
        symbol: Some(SymbolInfo {
            kind: SymbolKind::Function,
            visibility: Visibility::Public,
            signature: "pub fn add(a: i32, b: i32) -> i32".to_owned(),
            params: vec![
                ParamInfo {
                    name: "a".to_owned(),
                    type_repr: Some("i32".to_owned()),
                    default: None,
                    is_optional: false,
                    is_variadic: false,
                },
                ParamInfo {
                    name: "b".to_owned(),
                    type_repr: Some("i32".to_owned()),
                    default: None,
                    is_optional: false,
                    is_variadic: false,
                },
            ],
            returns: None,
            generics: vec![],
            decorators: vec![],
            is_async: false,
            is_deprecated: false,
            ..SymbolInfo::default()
        }),
        meta: DocMeta {
            path: PathBuf::from("src/math.rs"),
            line_start: 10,
            line_end: 12,
            column: 1,
            file_ext: "rs".to_owned(),
            comment_style: CommentStyle::DocSingle,
            last_indexed: 1_700_000_000,
            source_mtime: 1_699_000_000,
        },
        body_hash: 42,
        diagnostics: vec![],
        virtual_tags: BTreeMap::new(),
        virtual_confidence: None,
        virtual_sources: Vec::new(),
    }
}

fn blocks() -> BTreeMap<String, DocBlock> {
    let mut m = BTreeMap::new();
    m.insert("math.add".to_owned(), sample_block());
    m
}

fn no_custom_schemas() -> BTreeMap<String, TagSchema> {
    Config::default().tags
}

#[test]
fn renders_block_field() {
    let src = "Label: {{ @doc.math.add:label }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "Label: add");
}

#[test]
fn renders_tag_shortcut_single_field() {
    let src = "{{ @doc.math.add:description }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "Adds two integers together.");
}

#[test]
fn renders_tag_field_via_dot_shortcut() {
    // Inline backticks are passthrough — directives outside backticks evaluate.
    let src = "Returns {{ @doc.math.add:returns.type }} — {{ @doc.math.add:returns.description }}.";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "Returns i32 — the sum.");
}

#[test]
fn renders_tag_field_via_explicit_index() {
    let src = "First param: {{ @doc.math.add:param[0].name }} ({{ @doc.math.add:param[0].type }})";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "First param: a (i32)");
}

#[test]
fn renders_each_loop() {
    let src = "{{ each p in @doc.math.add:param }}- **{{ p.name }}** ({{ p.type }}): {{ p.description }}\n{{ /each }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(
        out,
        "- **a** (i32): first operand\n- **b** (i32): second operand\n"
    );
}

#[test]
fn renders_if_has_true_branch() {
    let src = "{{ if @doc.math.add:has(example) }}has example{{ else }}no example{{ /if }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "has example");
}

#[test]
fn renders_if_has_false_branch() {
    let src = "{{ if @doc.math.add:has(deprecated) }}deprecated{{ else }}stable{{ /if }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "stable");
}

#[test]
fn renders_count_comparison() {
    let src = "{{ if @doc.math.add:count(param) > 1 }}multi{{ else }}single{{ /if }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "multi");
}

#[test]
fn renders_meta_subpath() {
    let src = "Defined in {{ @doc.math.add:meta.path }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "Defined in src/math.rs");
}

#[test]
fn renders_symbol_subpath() {
    // Bare directive — outside any fence — evaluates to the signature.
    let src = "{{ @doc.math.add:symbol.signature }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "pub fn add(a: i32, b: i32) -> i32");
}

#[test]
fn realistic_end_to_end_markdown() {
    // Realistic C-shape: live data lives outside fences/inline-backticks.
    // Fences are reserved for code samples that should render verbatim.
    // For directives that need to inject data formatted as a code block,
    // use a `dsl`-tagged fence (see `dsl_fence_evaluates_directives`).
    let src = r"# {{ @doc.math.add:label }}

{{ @doc.math.add:description }}

**Signature**: {{ @doc.math.add:symbol.signature }}

## Parameters

{{ each p in @doc.math.add:param }}
- **{{ p.name }}** ({{ p.type }}): {{ p.description }}
{{ /each }}

**Returns** ({{ @doc.math.add:returns.type }}): {{ @doc.math.add:returns.description }}
{{ if @doc.math.add:has(example) }}

## Example

{{ @doc.math.add:first(example) }}
{{ /if }}
";

    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert!(out.contains("# add"));
    assert!(out.contains("Adds two integers together."));
    assert!(out.contains("pub fn add(a: i32, b: i32) -> i32"));
    assert!(out.contains("- **a** (i32): first operand"));
    assert!(out.contains("- **b** (i32): second operand"));
    assert!(out.contains("**Returns** (i32): the sum"));
    assert!(out.contains("let r = add(1, 2);"));
}

// ---- Fence semantics (option C: lang-aware) ----

#[test]
fn dsl_fence_evaluates_directives() {
    // Info-string `dsl` opts into evaluation inside the fence.
    let src = "```dsl\n{{ @doc.math.add:symbol.signature }}\n```";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "```dsl\npub fn add(a: i32, b: i32) -> i32\n```");
}

#[test]
fn dsl_fence_info_string_is_case_insensitive() {
    let src = "```DSL\n{{ @doc.math.add:label }}\n```";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "```DSL\nadd\n```");
}

#[test]
fn rust_fence_passes_through_directives() {
    // Default: any non-`dsl` info-string is passthrough.
    let src = "```rust\n{{ @doc.math.add:symbol.signature }}\n```";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, src);
}

#[test]
fn bare_fence_passes_through_directives() {
    let src = "```\n{{ @doc.math.add:symbol.signature }}\n```";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, src);
}

#[test]
fn inline_backticks_pass_through_directives() {
    let src = "Type: `{{ @doc.math.add:returns.type }}`";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "Type: `{{ @doc.math.add:returns.type }}`");
}

#[test]
fn unknown_key_errors() {
    let src = "{{ @doc.unknown:label }}";
    let err = render_string(src, &blocks(), &no_custom_schemas()).unwrap_err();
    assert!(err.to_string().contains("unknown block key"));
}

#[test]
fn block_directives_consume_own_line() {
    // `each` and `end` on their own lines should disappear without leaving
    // empty lines behind, whether at the top, middle, or end of the template.
    let src = "Before\n{{ each p in @doc.math.add:param }}\n- {{ p.name }}\n{{ /each }}\nAfter";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "Before\n- a\n- b\nAfter");
}

#[test]
fn block_directive_inline_does_not_strip_neighbours() {
    // When the directive shares its line with other content, we must NOT trim.
    let src = "x {{ each p in @doc.math.add:param }}[{{ p.name }}]{{ /each }} y";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "x [a][b] y");
}

#[test]
fn parse_error_surfaces() {
    let src = "{{ @doc.math.add:has(example }}";
    let err = render_string(src, &blocks(), &no_custom_schemas()).unwrap_err();
    assert!(err.to_string().contains("expected ')'") || err.to_string().contains("')'"));
}

// ---- Cardinality + truthy semantics (DSL v2) ----

#[test]
fn multi_tag_bare_access_errors() {
    // `param` est Multi → `:param` standalone est ambigu.
    let src = "{{ @doc.math.add:param }}";
    let err = render_string(src, &blocks(), &no_custom_schemas()).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("ambiguous"), "got: {msg}");
    assert!(msg.contains("param"));
    assert!(msg.contains("first(param)") || msg.contains("each"));
}

#[test]
fn multi_tag_field_shortcut_errors() {
    // `param.name` est ambigu sur Multi — il faut `param[N].name` ou `each`.
    let src = "{{ @doc.math.add:param.name }}";
    let err = render_string(src, &blocks(), &no_custom_schemas()).unwrap_err();
    assert!(err.to_string().contains("ambiguous"));
}

#[test]
fn single_tag_field_shortcut_works() {
    // `returns.type` est Single → shortcut OK.
    let src = "{{ @doc.math.add:returns.type }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "i32");
}

#[test]
fn single_tag_bare_access_works() {
    // `description` est Single → `:description` standalone OK.
    let src = "{{ @doc.math.add:description }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert!(out.contains("Adds two integers"));
}

#[test]
fn explicit_tag_index_still_works_on_multi() {
    // User disambiguates with [N] -> no error.
    let src = "{{ @doc.math.add:param[0].name }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "a");
}

#[test]
fn truthy_missing_tag_returns_false_not_error() {
    // `if @doc.x:tag` when tag does not exist -> false, no error.
    let src = "{{ if @doc.math.add:deprecated }}DEP{{ else }}OK{{ /if }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "OK");
}

#[test]
fn truthy_missing_field_returns_false_not_error() {
    // `if @doc.x:returns.type` on a block without `@returns` -> false.
    // (math.add has `@returns`, so this should return true here.)
    let src = "{{ if @doc.math.add:returns.type }}HAS_TYPE{{ /if }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "HAS_TYPE");
}

#[test]
fn truthy_swallows_unknown_meta_field() {
    // Un champ meta inexistant en condition → false, pas d'erreur.
    let src = "{{ if @doc.math.add:meta.bogus }}WAT{{ else }}safe{{ /if }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "safe");
}

#[test]
fn ambiguous_access_is_not_swallowed_in_truthy() {
    // Safety: `if @doc.x:param` must ERROR (not false) because it is a
    // template bug, not a legitimate absence.
    let src = "{{ if @doc.math.add:param }}YES{{ /if }}";
    let err = render_string(src, &blocks(), &no_custom_schemas()).unwrap_err();
    assert!(err.to_string().contains("ambiguous"));
}

// ---- Closing tags (DSL v2) ----

#[test]
fn each_must_close_with_slash_each_not_slash_if() {
    let src = "{{ each p in @doc.math.add:param }}{{ p.name }}{{ /if }}";
    let err = render_string(src, &blocks(), &no_custom_schemas()).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("/each"), "got: {msg}");
}

#[test]
fn unterminated_each_reports_missing_slash_each() {
    let src = "{{ each p in @doc.math.add:param }}{{ p.name }}";
    let err = render_string(src, &blocks(), &no_custom_schemas()).unwrap_err();
    assert!(err.to_string().contains("/each"));
}

#[test]
fn old_end_keyword_is_rejected() {
    // Old `{{ end }}` should no longer parse — otherwise we'd have a silent regression.
    let src = "{{ each p in @doc.math.add:param }}{{ p.name }}{{ end }}";
    assert!(render_string(src, &blocks(), &no_custom_schemas()).is_err());
}

// ---- first() / last() chainables ----

#[test]
fn first_with_field_returns_first_param_field() {
    let src = "{{ @doc.math.add:first(param).name }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "a");
}

#[test]
fn last_with_field_returns_last_param_field() {
    let src = "{{ @doc.math.add:last(param).name }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "b");
}

#[test]
fn first_without_field_still_works() {
    let src = "{{ @doc.math.add:first(param) }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert!(out.contains('a'));
}

#[test]
fn has_with_field_is_a_type_error() {
    // has() returns a scalar — chaining .field is invalid.
    let src = "{{ @doc.math.add:has(param).name }}";
    let err = render_string(src, &blocks(), &no_custom_schemas()).unwrap_err();
    assert!(err.to_string().contains("scalaire") || err.to_string().contains("scalar"));
}

#[test]
fn count_with_field_is_a_type_error() {
    let src = "{{ @doc.math.add:count(param).name }}";
    assert!(render_string(src, &blocks(), &no_custom_schemas()).is_err());
}

#[test]
fn first_field_on_missing_tag_in_truthy_is_false() {
    let src = "{{ if @doc.math.add:first(deprecated).reason }}DEP{{ else }}OK{{ /if }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "OK");
}

// ---- else if ----

#[test]
fn else_if_picks_second_branch() {
    // First truthy -> first branch (count(param) > 5 = false; > 1 = true)
    let src = "{{ if @doc.math.add:count(param) > 5 }}A{{ else if @doc.math.add:count(param) > 1 }}B{{ else }}C{{ /if }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "B");
}

#[test]
fn else_if_falls_through_to_else() {
    let src = "{{ if @doc.math.add:count(param) > 5 }}A{{ else if @doc.math.add:count(param) > 100 }}B{{ else }}C{{ /if }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "C");
}

#[test]
fn multiple_else_if_chain() {
    let src = "{{ if @doc.math.add:count(param) == 0 }}zero{{ else if @doc.math.add:count(param) == 1 }}one{{ else if @doc.math.add:count(param) == 2 }}two{{ else }}many{{ /if }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "two");
}

#[test]
fn else_if_without_else_is_valid() {
    let src = "{{ if @doc.math.add:count(param) > 100 }}A{{ else if @doc.math.add:count(param) > 1 }}B{{ /if }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "B");
}

// ---- Default projection ----

#[test]
fn bare_ref_with_symbol_and_description_combines_both() {
    let src = "{{ @doc.math.add }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    // Signature then description, separated by blank line.
    assert_eq!(
        out,
        "pub fn add(a: i32, b: i32) -> i32\n\nAdds two integers together."
    );
}

#[test]
fn bare_ref_with_symbol_only_returns_signature() {
    let mut block = sample_block();
    block.tags.remove("description");
    let mut bs = BTreeMap::new();
    bs.insert("math.add".to_owned(), block);
    let out = render_string("{{ @doc.math.add }}", &bs, &no_custom_schemas()).unwrap();
    assert_eq!(out, "pub fn add(a: i32, b: i32) -> i32");
}

#[test]
fn bare_ref_with_description_only_returns_description() {
    let mut block = sample_block();
    block.symbol = None;
    let mut bs = BTreeMap::new();
    bs.insert("math.add".to_owned(), block);
    let out = render_string("{{ @doc.math.add }}", &bs, &no_custom_schemas()).unwrap();
    assert_eq!(out, "Adds two integers together.");
}

#[test]
fn bare_ref_with_neither_falls_back_to_label() {
    let mut block = sample_block();
    block.symbol = None;
    block.tags.remove("description");
    let mut bs = BTreeMap::new();
    bs.insert("math.add".to_owned(), block);
    let out = render_string("{{ @doc.math.add }}", &bs, &no_custom_schemas()).unwrap();
    assert_eq!(out, "add");
}

#[test]
fn label_access_still_returns_just_label() {
    // Sanity: `:label` is explicit and remains label regardless of default projection.
    let src = "{{ @doc.math.add:label }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "add");
}

// ---- Block iteration : @docs.module(K) / @docs.all ----

fn module_blocks() -> BTreeMap<String, DocBlock> {
    // Three blocks: two in `api.users.*`, one in another module to verify
    // prefix filtering.
    let mut create = sample_block();
    create.key = DocKey::new("api.users.create");
    create.label.clear();
    create.label.push_str("create");
    let mut delete = sample_block();
    delete.key = DocKey::new("api.users.delete");
    delete.label.clear();
    delete.label.push_str("delete");
    let mut other = sample_block();
    other.key = DocKey::new("api.posts.publish");
    other.label.clear();
    other.label.push_str("publish");
    let mut m = BTreeMap::new();
    m.insert("api.users.create".to_owned(), create);
    m.insert("api.users.delete".to_owned(), delete);
    m.insert("api.posts.publish".to_owned(), other);
    m
}

#[test]
fn each_blocks_module_filters_by_prefix() {
    let src = "{{ each f in @docs.module(api.users) }}- {{ f.label }}\n{{ /each }}";
    let out = render_string(src, &module_blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "- create\n- delete\n");
}

#[test]
fn each_blocks_all_iterates_everything() {
    let src = "{{ each f in @docs.all }}{{ f.label }};{{ /each }}";
    let out = render_string(src, &module_blocks(), &no_custom_schemas()).unwrap();
    // BTreeMap -> lexical key order: api.posts.publish, api.users.create, api.users.delete
    assert_eq!(out, "publish;create;delete;");
}

#[test]
fn block_alias_default_projection() {
    // {{ f }} bare → render_default_projection(block).
    let src = "{{ each f in @docs.module(api.users) }}{{ f }}\n---\n{{ /each }}";
    let out = render_string(src, &module_blocks(), &no_custom_schemas()).unwrap();
    // Each block has same symbol+description (because sample_block() is
    // shared). So signature appears twice.
    assert!(out.contains("pub fn add(a: i32, b: i32) -> i32"));
    assert!(out.contains("Adds two integers together."));
    assert_eq!(out.matches("---").count(), 2);
}

#[test]
fn block_alias_field_access() {
    let src = "{{ each f in @docs.module(api.users) }}{{ f.meta.path }};{{ /each }}";
    let out = render_string(src, &module_blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "src/math.rs;src/math.rs;");
}

#[test]
fn block_alias_tag_shortcut() {
    // f.returns.type -> resolved through TagShortcut on Single.
    let src = "{{ each f in @docs.module(api.users) }}{{ f.returns.type }}|{{ /each }}";
    let out = render_string(src, &module_blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "i32|i32|");
}

#[test]
fn block_alias_multi_tag_errors() {
    // f.param est ambigu → erreur claire.
    let src = "{{ each f in @docs.module(api.users) }}{{ f.param }}{{ /each }}";
    let err = render_string(src, &module_blocks(), &no_custom_schemas()).unwrap_err();
    assert!(err.to_string().contains("ambiguous"));
}

#[test]
fn module_query_does_not_match_unrelated_keys() {
    // `api.users` ne matche pas `api.posts.publish`.
    let src = "{{ each f in @docs.module(api.users) }}{{ f.label }}\n{{ /each }}";
    let out = render_string(src, &module_blocks(), &no_custom_schemas()).unwrap();
    assert!(!out.contains("publish"));
}

#[test]
fn module_query_does_not_match_partial_segment() {
    // `api.user` (without s) must NOT match `api.users.*` — strict separator.
    let src = "{{ each f in @docs.module(api.user) }}MATCH{{ /each }}";
    let out = render_string(src, &module_blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "");
}

#[test]
fn unknown_docs_query_kind_errors() {
    let src = "{{ each f in @docs.bogus }}x{{ /each }}";
    let err = render_string(src, &module_blocks(), &no_custom_schemas()).unwrap_err();
    assert!(err.to_string().contains("@docs"));
}

#[test]
fn nested_each_blocks_then_each_tag() {
    // Real pattern: iterate over blocks, then over their params.
    let src = "{{ each f in @docs.module(api.users) }}### {{ f.label }}\n{{ each p in @doc.api.users.create:param }}- {{ p.name }}\n{{ /each }}{{ /each }}";
    let out = render_string(src, &module_blocks(), &no_custom_schemas()).unwrap();
    // Each block sees both params (same fixture).
    assert!(out.contains("### create"));
    assert!(out.contains("### delete"));
    assert_eq!(out.matches("- a\n").count(), 2);
}

// ---- meta / symbol whitelist ----

#[test]
fn meta_path_uses_forward_slashes() {
    let src = "{{ @doc.math.add:meta.path }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "src/math.rs");
    assert!(!out.contains('\\'));
}

#[test]
fn meta_line_start_returns_number_as_string() {
    let src = "{{ @doc.math.add:meta.lineStart }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "10");
}

#[test]
fn meta_internal_fields_are_rejected() {
    // `lastIndexed` and `sourceMtime` must NOT be accessible —
    // these are internal pipeline fields.
    for f in ["lastIndexed", "sourceMtime", "snake_case_thing"] {
        let src = format!("{{{{ @doc.math.add:meta.{f} }}}}");
        let res = render_string(&src, &blocks(), &no_custom_schemas());
        assert!(res.is_err(), "meta.{f} should be rejected");
    }
}

#[test]
fn symbol_kind_returns_kebab_case_string() {
    let src = "{{ @doc.math.add:symbol.kind }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "function");
}

#[test]
fn symbol_visibility_returns_string() {
    let src = "{{ @doc.math.add:symbol.visibility }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "public");
}

#[test]
fn symbol_is_async_returns_bool_string() {
    let src = "{{ @doc.math.add:symbol.isAsync }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "false");
}

#[test]
fn symbol_internal_fields_are_rejected() {
    // `params` and `returns` must be accessed via `@param`/`@returns` tags.
    for f in ["params", "returns", "references"] {
        let src = format!("{{{{ @doc.math.add:symbol.{f} }}}}");
        let res = render_string(&src, &blocks(), &no_custom_schemas());
        assert!(res.is_err(), "symbol.{f} should be rejected");
    }
}

#[test]
fn symbol_signature_works() {
    let src = "{{ @doc.math.add:symbol.signature }}";
    let out = render_string(src, &blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "pub fn add(a: i32, b: i32) -> i32");
}

// ---- Block-alias functions + alias in conditions (DSL v2.1) ----

#[test]
fn block_alias_has_function() {
    let src = "{{ each f in @docs.module(api.users) }}{{ f.has(example) }};{{ /each }}";
    let out = render_string(src, &module_blocks(), &no_custom_schemas()).unwrap();
    // sample_block() has an `@example` tag → "true" twice.
    assert_eq!(out, "true;true;");
}

#[test]
fn block_alias_count_function() {
    let src = "{{ each f in @docs.module(api.users) }}{{ f.count(param) }};{{ /each }}";
    let out = render_string(src, &module_blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "2;2;");
}

#[test]
fn block_alias_first_with_field() {
    let src = "{{ each f in @docs.module(api.users) }}{{ f.first(param).name }};{{ /each }}";
    let out = render_string(src, &module_blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "a;a;");
}

#[test]
fn block_alias_last_with_field() {
    let src = "{{ each f in @docs.module(api.users) }}{{ f.last(param).type }};{{ /each }}";
    let out = render_string(src, &module_blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "i32;i32;");
}

#[test]
fn if_alias_truthy_on_single_tag() {
    let src = "{{ each f in @docs.module(api.users) }}{{ if f.description }}D{{ else }}_{{ /if }}{{ /each }}";
    let out = render_string(src, &module_blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "DD");
}

#[test]
fn if_alias_has_function() {
    let src = "{{ each f in @docs.module(api.users) }}{{ if f.has(example) }}E{{ /if }}{{ /each }}";
    let out = render_string(src, &module_blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "EE");
}

#[test]
fn if_alias_count_comparison() {
    let src =
        "{{ each f in @docs.module(api.users) }}{{ if f.count(param) > 1 }}M{{ /if }}{{ /each }}";
    let out = render_string(src, &module_blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "MM");
}

#[test]
fn if_alias_truthy_on_missing_tag_is_false() {
    // f.deprecated n'existe pas → false (pas d'erreur).
    let src = "{{ each f in @docs.module(api.users) }}{{ if f.deprecated }}DEP{{ else }}OK{{ /if }};{{ /each }}";
    let out = render_string(src, &module_blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "OK;OK;");
}

#[test]
fn alias_func_chained_field_in_truthy_works() {
    // f.first(deprecated).reason absent → false.
    let src = "{{ each f in @docs.module(api.users) }}{{ if f.first(deprecated).reason }}DEP{{ else }}OK{{ /if }};{{ /each }}";
    let out = render_string(src, &module_blocks(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "OK;OK;");
}

// -------- Satellite annotation queries (`::` separator) --------

fn anchor_with_satellites() -> BTreeMap<String, DocBlock> {
    // Anchor at `tools.get_doc`, two satellites, plus a dot-child to
    // distinguish dotted vs `::` boundaries.
    let mut anchor = sample_block();
    anchor.key = DocKey::new("tools.get_doc");
    anchor.label = "get_doc".into();

    let mut sat_schema = sample_block();
    sat_schema.key = DocKey::new("tools.get_doc::schema");
    sat_schema.label = "schema".into();

    let mut sat_examples = sample_block();
    sat_examples.key = DocKey::new("tools.get_doc::examples");
    sat_examples.label = "examples".into();

    let mut child = sample_block();
    child.key = DocKey::new("tools.get_doc.helper");
    child.label = "helper".into();

    let mut other = sample_block();
    other.key = DocKey::new("tools.list_docs");
    other.label = "list_docs".into();

    let mut m = BTreeMap::new();
    m.insert(anchor.key.as_str().to_owned(), anchor);
    m.insert(sat_schema.key.as_str().to_owned(), sat_schema);
    m.insert(sat_examples.key.as_str().to_owned(), sat_examples);
    m.insert(child.key.as_str().to_owned(), child);
    m.insert(other.key.as_str().to_owned(), other);
    m
}

#[test]
fn ref_with_double_colon_resolves_satellite() {
    let src = "Schema doc: {{ @doc.tools.get_doc::schema:label }}";
    let out = render_string(src, &anchor_with_satellites(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "Schema doc: schema");
}

#[test]
fn ref_to_anchor_unaffected_by_satellites() {
    // Sanity: the anchor key still resolves cleanly when satellites exist.
    let src = "{{ @doc.tools.get_doc:label }}";
    let out = render_string(src, &anchor_with_satellites(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "get_doc");
}

#[test]
fn module_query_includes_anchor_dot_children_and_satellites() {
    // module(tools.get_doc) must capture the anchor itself, the dot-child
    // `.helper`, and both `::` satellites — but NOT the sibling `tools.list_docs`.
    let src = "{{ each f in @docs.module(tools.get_doc) }}{{ f.label }};{{ /each }}";
    let out = render_string(src, &anchor_with_satellites(), &no_custom_schemas()).unwrap();
    // BTreeMap order: tools.get_doc, tools.get_doc.helper, tools.get_doc::examples, tools.get_doc::schema
    assert_eq!(out, "get_doc;helper;examples;schema;");
}

#[test]
fn satellites_query_excludes_anchor_and_dot_children() {
    let src = "{{ each f in @docs.satellites(tools.get_doc) }}{{ f.label }};{{ /each }}";
    let out = render_string(src, &anchor_with_satellites(), &no_custom_schemas()).unwrap();
    // BTreeMap order: ::examples before ::schema
    assert_eq!(out, "examples;schema;");
}

#[test]
fn satellites_query_does_not_match_partial_segment() {
    // `tools.get` (without `_doc`) must not match `tools.get_doc::*`.
    let src = "{{ each f in @docs.satellites(tools.get) }}MATCH{{ /each }}";
    let out = render_string(src, &anchor_with_satellites(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "");
}

#[test]
fn satellites_query_empty_when_no_satellites() {
    // `tools.list_docs` has no satellites in the fixture.
    let src = "{{ each f in @docs.satellites(tools.list_docs) }}{{ f.label }};{{ /each }}";
    let out = render_string(src, &anchor_with_satellites(), &no_custom_schemas()).unwrap();
    assert_eq!(out, "");
}

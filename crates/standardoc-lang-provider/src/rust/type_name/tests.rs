use super::*;

#[test]
fn path_type_returns_last_segment() {
    let ty: Type = syn::parse_str("Vec").unwrap();
    assert_eq!(parametric_type(&ty).as_deref(), Some("Vec"));
}

#[test]
fn strips_qualified_path_to_last_segment_preserving_generics() {
    let ty: Type = syn::parse_str("std::collections::HashMap<K, V>").unwrap();
    assert_eq!(parametric_type(&ty).as_deref(), Some("HashMap<K, V>"));
}

#[test]
fn strips_reference_chain() {
    let ty: Type = syn::parse_str("&&mut Foo").unwrap();
    assert_eq!(parametric_type(&ty).as_deref(), Some("Foo"));
}

#[test]
fn tuple_returns_none() {
    let ty: Type = syn::parse_str("(u8, u16)").unwrap();
    assert_eq!(parametric_type(&ty), None);
}

#[test]
fn slice_returns_none() {
    let ty: Type = syn::parse_str("[u8]").unwrap();
    assert_eq!(parametric_type(&ty), None);
}

#[test]
fn impl_trait_returns_none() {
    let ty: Type = syn::parse_str("impl Fn(u8) -> u8").unwrap();
    assert_eq!(parametric_type(&ty), None);
}

#[test]
fn never_returns_none() {
    let ty: Type = syn::parse_str("!").unwrap();
    assert_eq!(parametric_type(&ty), None);
}

// --- Bug E-3 ext P-E3.2: parametric_type / nominal_of / generic_args /
// substitute_template

#[test]
fn parametric_type_preserves_single_arg() {
    let ty: Type = syn::parse_str("Vec<Foo>").unwrap();
    assert_eq!(parametric_type(&ty).as_deref(), Some("Vec<Foo>"));
}

#[test]
fn parametric_type_preserves_multi_arg_and_strips_refs() {
    let ty: Type = syn::parse_str("&HashMap<String, Vec<u8>>").unwrap();
    assert_eq!(
        parametric_type(&ty).as_deref(),
        Some("HashMap<String, Vec<u8>>")
    );
}

#[test]
fn parametric_type_lifetime_collapses_to_underscore() {
    let ty: Type = syn::parse_str("Cow<'a, str>").unwrap();
    assert_eq!(parametric_type(&ty).as_deref(), Some("Cow<_, str>"));
}

#[test]
fn nominal_of_strips_generics() {
    assert_eq!(nominal_of("Vec<Foo>"), "Vec");
    assert_eq!(nominal_of("HashMap<String, Vec<u8>>"), "HashMap");
    assert_eq!(nominal_of("Bare"), "Bare");
}

#[test]
fn generic_args_splits_at_depth_zero() {
    assert_eq!(generic_args("Vec<Foo>"), vec!["Foo"]);
    assert_eq!(
        generic_args("HashMap<String, Vec<u8>>"),
        vec!["String", "Vec<u8>"]
    );
    assert_eq!(generic_args("Bare"), Vec::<&str>::new());
}

#[test]
fn substitute_template_t_for_vec() {
    assert_eq!(
        substitute_template("Iterator<T>", "Vec", &["Foo"]),
        "Iterator<Foo>"
    );
}

#[test]
fn substitute_template_e_for_result() {
    assert_eq!(
        substitute_template("E", "Result", &["Foo", "ApiErr"]),
        "ApiErr"
    );
    assert_eq!(
        substitute_template("Result<_, E>", "Result", &["Foo", "ApiErr"]),
        "Result<_, ApiErr>"
    );
}

#[test]
fn substitute_template_k_v_for_hashmap() {
    assert_eq!(
        substitute_template("(&K, &mut V)", "HashMap", &["String", "User"]),
        "(&String, &mut User)"
    );
}

#[test]
fn substitute_template_leaves_unknown_tokens() {
    // `usize` is not a parametric placeholder — substitution must
    // preserve it verbatim.
    assert_eq!(
        substitute_template("Option<usize>", "Iterator", &["Foo"]),
        "Option<usize>"
    );
}

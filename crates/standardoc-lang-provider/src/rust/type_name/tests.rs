use super::*;

#[test]
fn path_type_returns_last_segment() {
    let ty: Type = syn::parse_str("Vec").unwrap();
    assert_eq!(nominal_type(&ty).as_deref(), Some("Vec"));
}

#[test]
fn drops_generic_args() {
    let ty: Type = syn::parse_str("HashMap<String, Vec<u8>>").unwrap();
    assert_eq!(nominal_type(&ty).as_deref(), Some("HashMap"));
}

#[test]
fn strips_qualified_path_to_last_segment() {
    let ty: Type = syn::parse_str("std::collections::HashMap<K, V>").unwrap();
    assert_eq!(nominal_type(&ty).as_deref(), Some("HashMap"));
}

#[test]
fn strips_reference_chain() {
    let ty: Type = syn::parse_str("&&mut Foo").unwrap();
    assert_eq!(nominal_type(&ty).as_deref(), Some("Foo"));
}

#[test]
fn tuple_returns_none() {
    let ty: Type = syn::parse_str("(u8, u16)").unwrap();
    assert_eq!(nominal_type(&ty), None);
}

#[test]
fn slice_returns_none() {
    let ty: Type = syn::parse_str("[u8]").unwrap();
    assert_eq!(nominal_type(&ty), None);
}

#[test]
fn impl_trait_returns_none() {
    let ty: Type = syn::parse_str("impl Fn(u8) -> u8").unwrap();
    assert_eq!(nominal_type(&ty), None);
}

#[test]
fn never_returns_none() {
    let ty: Type = syn::parse_str("!").unwrap();
    assert_eq!(nominal_type(&ty), None);
}

use super::*;

fn ty(src: &str) -> Type {
    syn::parse_str(src).expect("parse Type")
}

#[test]
fn record_and_lookup_named_field() {
    let mut t = StructFieldTable::default();
    t.record("crate::Foo", "name", &ty("String"));
    assert_eq!(t.lookup("crate::Foo", "name"), Some("String"));
}

#[test]
fn lookup_unknown_struct_returns_none() {
    let t = StructFieldTable::default();
    assert_eq!(t.lookup("crate::Foo", "name"), None);
}

#[test]
fn lookup_unknown_field_returns_none() {
    let mut t = StructFieldTable::default();
    t.record("crate::Foo", "name", &ty("String"));
    assert_eq!(t.lookup("crate::Foo", "missing"), None);
}

#[test]
fn record_drops_generics_via_nominal_type() {
    let mut t = StructFieldTable::default();
    t.record("crate::Cache", "map", &ty("HashMap<String, Vec<u8>>"));
    assert_eq!(t.lookup("crate::Cache", "map"), Some("HashMap"));
}

#[test]
fn record_strips_reference_chain() {
    let mut t = StructFieldTable::default();
    t.record("crate::Ref", "inner", &ty("&'a mut Foo"));
    assert_eq!(t.lookup("crate::Ref", "inner"), Some("Foo"));
}

#[test]
fn record_skips_tuple_type() {
    let mut t = StructFieldTable::default();
    t.record("crate::Pair", "xy", &ty("(u8, u16)"));
    assert_eq!(t.lookup("crate::Pair", "xy"), None);
}

#[test]
fn record_skips_closure_type() {
    let mut t = StructFieldTable::default();
    t.record("crate::Hook", "cb", &ty("Box<dyn Fn(u8) -> u8>"));
    // Box collapses to Box — the outer is nominal.
    assert_eq!(t.lookup("crate::Hook", "cb"), Some("Box"));
}

#[test]
fn two_structs_isolated() {
    let mut t = StructFieldTable::default();
    t.record("crate::A", "x", &ty("Foo"));
    t.record("crate::B", "x", &ty("Bar"));
    assert_eq!(t.lookup("crate::A", "x"), Some("Foo"));
    assert_eq!(t.lookup("crate::B", "x"), Some("Bar"));
}

#[test]
fn record_overwrites_on_repeat() {
    let mut t = StructFieldTable::default();
    t.record("crate::Foo", "x", &ty("u8"));
    assert_eq!(t.lookup("crate::Foo", "x"), Some("u8"));
    t.record("crate::Foo", "x", &ty("String"));
    assert_eq!(t.lookup("crate::Foo", "x"), Some("String"));
}

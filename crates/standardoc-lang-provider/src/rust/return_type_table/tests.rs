use super::*;

fn ty(src: &str) -> Type {
    syn::parse_str(src).expect("parse Type")
}

#[test]
fn record_and_lookup_free_fn() {
    let mut t = ReturnTypeTable::default();
    t.record("crate::get_user", &ty("User"));
    assert_eq!(t.lookup("crate::get_user"), Some("User"));
}

#[test]
fn record_and_lookup_impl_method() {
    // Bug E-3 ext P-E3.2.1: parametric storage preserves generics so
    // workspace chains can propagate the inner type into closures.
    let mut t = ReturnTypeTable::default();
    t.record("crate::Repo::find_by_id", &ty("Option<User>"));
    assert_eq!(t.lookup("crate::Repo::find_by_id"), Some("Option<User>"));
}

#[test]
fn lookup_unknown_fqdn_returns_none() {
    let t = ReturnTypeTable::default();
    assert_eq!(t.lookup("crate::missing"), None);
}

#[test]
fn record_preserves_generics_parametric() {
    // Bug E-3 ext P-E3.2.1: keep the full parametric form for chains
    // that need `T` substituted (e.g. `cache_get().iter().map(|x|...)`).
    let mut t = ReturnTypeTable::default();
    t.record("crate::cache_get", &ty("HashMap<String, Vec<u8>>"));
    assert_eq!(
        t.lookup("crate::cache_get"),
        Some("HashMap<String, Vec<u8>>")
    );
}

#[test]
fn record_strips_reference_chain() {
    let mut t = ReturnTypeTable::default();
    t.record("crate::peek_ref", &ty("&'a mut Foo"));
    assert_eq!(t.lookup("crate::peek_ref"), Some("Foo"));
}

#[test]
fn record_skips_tuple_return() {
    let mut t = ReturnTypeTable::default();
    t.record("crate::pair", &ty("(u8, u16)"));
    assert_eq!(t.lookup("crate::pair"), None);
}

#[test]
fn record_skips_unit_return() {
    let mut t = ReturnTypeTable::default();
    t.record("crate::side_effect", &ty("()"));
    assert_eq!(t.lookup("crate::side_effect"), None);
}

#[test]
fn record_overwrites_on_repeat() {
    let mut t = ReturnTypeTable::default();
    t.record("crate::factory", &ty("u8"));
    assert_eq!(t.lookup("crate::factory"), Some("u8"));
    t.record("crate::factory", &ty("String"));
    assert_eq!(t.lookup("crate::factory"), Some("String"));
}

#[test]
fn free_fn_and_method_share_namespace() {
    let mut t = ReturnTypeTable::default();
    t.record("crate::foo", &ty("Foo"));
    t.record("crate::Bar::baz", &ty("Baz"));
    assert_eq!(t.lookup("crate::foo"), Some("Foo"));
    assert_eq!(t.lookup("crate::Bar::baz"), Some("Baz"));
}

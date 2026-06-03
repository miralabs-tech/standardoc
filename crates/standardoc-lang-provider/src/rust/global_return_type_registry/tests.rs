use super::GlobalReturnTypeRegistry;
use syn::parse_quote;

#[test]
fn record_nominal_return_type_is_indexed_parametrically() {
    let mut reg = GlobalReturnTypeRegistry::default();
    let ty: syn::Type = parse_quote!(Option<User>);
    reg.record("crate_a::foo::get_user", &ty);
    assert_eq!(reg.lookup("crate_a::foo::get_user"), Some("Option<User>"));
}

#[test]
fn record_non_nominal_return_type_is_skipped() {
    let mut reg = GlobalReturnTypeRegistry::default();
    // Tuple, closure, slice — all non-nominal under `parametric_type`.
    let tuple: syn::Type = parse_quote!((u32, String));
    let slice: syn::Type = parse_quote!([u8]);
    reg.record("crate_a::tuple_fn", &tuple);
    reg.record("crate_a::slice_fn", &slice);
    assert_eq!(reg.lookup("crate_a::tuple_fn"), None);
    assert_eq!(reg.lookup("crate_a::slice_fn"), None);
    assert_eq!(reg.len(), 0);
}

#[test]
fn lookup_miss_returns_none_for_unrecorded_fqdn() {
    let reg = GlobalReturnTypeRegistry::default();
    assert_eq!(reg.lookup("never::recorded"), None);
}

#[test]
fn record_same_fqdn_twice_overwrites_with_last_value() {
    let mut reg = GlobalReturnTypeRegistry::default();
    let first: syn::Type = parse_quote!(Option<A>);
    let second: syn::Type = parse_quote!(Result<B, E>);
    reg.record("crate_a::foo", &first);
    reg.record("crate_a::foo", &second);
    assert_eq!(reg.lookup("crate_a::foo"), Some("Result<B, E>"));
    assert_eq!(reg.len(), 1);
}

#[test]
fn record_reference_return_strips_through_to_nominal() {
    // `parametric_type` peels references — `&User` records as "User".
    // Matches `ReturnTypeTable`'s shape for symmetry with the per-file
    // path so the lookup chain stays uniform.
    let mut reg = GlobalReturnTypeRegistry::default();
    let ty: syn::Type = parse_quote!(&User);
    reg.record("crate_a::borrow_user", &ty);
    assert_eq!(reg.lookup("crate_a::borrow_user"), Some("User"));
}

#[test]
fn distinct_fqdns_do_not_collide() {
    let mut reg = GlobalReturnTypeRegistry::default();
    let a: syn::Type = parse_quote!(A);
    let b: syn::Type = parse_quote!(B);
    reg.record("crate_x::module_a::foo", &a);
    reg.record("crate_x::module_b::foo", &b);
    assert_eq!(reg.lookup("crate_x::module_a::foo"), Some("A"));
    assert_eq!(reg.lookup("crate_x::module_b::foo"), Some("B"));
}

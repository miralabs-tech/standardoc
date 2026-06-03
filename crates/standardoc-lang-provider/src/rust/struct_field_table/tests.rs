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
fn record_preserves_generics_parametric() {
    // Bug E-3 ext P-E3.2.1: parametric form lets closure-arg
    // substitution resolve `T` for `cache.map.iter().map(|(k, v)|...)`.
    let mut t = StructFieldTable::default();
    t.record("crate::Cache", "map", &ty("HashMap<String, Vec<u8>>"));
    assert_eq!(
        t.lookup("crate::Cache", "map"),
        Some("HashMap<String, Vec<u8>>")
    );
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
    // Bug E-3 ext P-E3.2.1: parametric form keeps Box's args slot but
    // the inner `dyn Fn(u8) -> u8` is non-nominal so collapses to `_`.
    assert_eq!(t.lookup("crate::Hook", "cb"), Some("Box<_>"));
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

#[test]
fn lookup_via_nominal_short_name_resolves_when_unique() {
    // Bug E-3 ext P-E3.2.2: a bare nominal short name lookup resolves
    // via the nominal→FQDN side-index when only one struct owns the
    // nominal.
    let mut t = StructFieldTable::default();
    t.record("standardoc-ir::symbol::RawSymbol", "name", &ty("String"));
    assert_eq!(t.lookup("RawSymbol", "name"), Some("String"));
    // FQDN path still works (covers `self.field` chains).
    assert_eq!(
        t.lookup("standardoc-ir::symbol::RawSymbol", "name"),
        Some("String")
    );
}

#[test]
fn lookup_via_nominal_short_name_falls_through_when_ambiguous() {
    // Two structs share the nominal `Foo` — the nominal lookup must
    // refuse to guess.
    let mut t = StructFieldTable::default();
    t.record("crate::a::Foo", "x", &ty("u8"));
    t.record("crate::b::Foo", "x", &ty("String"));
    assert_eq!(t.lookup("Foo", "x"), None);
    // FQDN lookups still hit the individual definitions.
    assert_eq!(t.lookup("crate::a::Foo", "x"), Some("u8"));
    assert_eq!(t.lookup("crate::b::Foo", "x"), Some("String"));
}

#[test]
fn nominal_lookup_after_repeat_record_to_same_fqdn_stays_unambiguous() {
    let mut t = StructFieldTable::default();
    t.record("crate::Foo", "x", &ty("u8"));
    t.record("crate::Foo", "y", &ty("String"));
    assert_eq!(t.lookup("Foo", "x"), Some("u8"));
    assert_eq!(t.lookup("Foo", "y"), Some("String"));
}

// --- Bug field-as-CALL V2: presence-only tracking via record_presence ---

#[test]
fn record_presence_tracks_field_name_without_type() {
    let mut t = StructFieldTable::default();
    t.record_presence("crate::S", "handler");
    assert!(t.has_field("crate::S", "handler"));
    assert!(!t.has_field("crate::S", "missing"));
}

#[test]
fn has_field_resolves_nominal_short_via_side_index() {
    let mut t = StructFieldTable::default();
    t.record_presence("crate::module::S", "cb");
    assert!(t.has_field("S", "cb"));
    assert!(t.has_field("crate::module::S", "cb"));
}

#[test]
fn has_field_returns_false_when_presence_nominal_is_ambiguous() {
    // Two distinct FQDNs colliding on the same nominal short name —
    // the side-index records `None` (ambiguous) and `has_field` on
    // the bare nominal falls through.
    let mut t = StructFieldTable::default();
    t.record_presence("crate::a::S", "x");
    t.record_presence("crate::b::S", "y");
    assert!(t.has_field("crate::a::S", "x"));
    assert!(t.has_field("crate::b::S", "y"));
    // bare "S" is ambiguous now
    assert!(!t.has_field("S", "x"));
    assert!(!t.has_field("S", "y"));
}

#[test]
fn has_field_independent_of_record_typed_lookup() {
    // V2 invariant: `record_presence` populates the presence table
    // even when `record` would skip the type (non-nominal `fn()`).
    // The typed `lookup` stays `None`, the presence `has_field`
    // returns `true`.
    let mut t = StructFieldTable::default();
    // No `record` call — only presence (simulating extract_items
    // calling `record_presence` for a `fn()` field where `record`
    // bailed out via `parametric_type`).
    t.record_presence("crate::S", "bare_ptr");
    assert_eq!(t.lookup("crate::S", "bare_ptr"), None);
    assert!(t.has_field("crate::S", "bare_ptr"));
}

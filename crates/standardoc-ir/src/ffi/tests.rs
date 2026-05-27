
use super::*;

#[test]
fn ffi_abi_slug_round_trip_known_variants() {
    for abi in [
        FfiAbi::C,
        FfiAbi::System,
        FfiAbi::Stdcall,
        FfiAbi::Lua,
        FfiAbi::Jni,
        FfiAbi::PythonCApi,
    ] {
        let slug = abi.as_slug().to_string();
        let back = FfiAbi::from_slug(&slug);
        assert_eq!(abi, back, "slug {slug} must round-trip");
    }
}

#[test]
fn ffi_abi_unknown_slug_lands_in_other() {
    let abi = FfiAbi::from_slug("syscall");
    assert_eq!(abi, FfiAbi::Other("syscall".into()));
    assert_eq!(abi.as_slug(), "syscall");
}

#[test]
fn ffi_direction_slug_round_trip() {
    for d in [FfiDirection::Export, FfiDirection::Import] {
        assert_eq!(FfiDirection::from_slug(d.as_slug()).unwrap(), d);
    }
    assert!(FfiDirection::from_slug("sideways").is_none());
}

#[test]
fn raw_ffi_binding_round_trips_through_json() {
    let b = RawFfiBinding {
        symbol_fqdn: "lurlang::runtime::vm::lur_vm_init".into(),
        abi: FfiAbi::C,
        direction: FfiDirection::Export,
        abi_name: "lur_vm_init".into(),
        convention: None,
    };
    let json = serde_json::to_string(&b).unwrap();
    let back: RawFfiBinding = serde_json::from_str(&json).unwrap();
    assert_eq!(b, back);
}

#[test]
fn raw_ffi_binding_serialises_with_convention_only_when_set() {
    let b = RawFfiBinding {
        symbol_fqdn: "x::luaopen_mymod".into(),
        abi: FfiAbi::Lua,
        direction: FfiDirection::Export,
        abi_name: "luaopen_mymod".into(),
        convention: Some("lua-module".into()),
    };
    let json = serde_json::to_string(&b).unwrap();
    assert!(json.contains("\"convention\":\"lua-module\""));
}

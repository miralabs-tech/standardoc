use super::*;

#[test]
fn project_kind_roundtrip_via_as_str_for_known_variants() {
    for k in [
        ProjectKind::Rust,
        ProjectKind::Node,
        ProjectKind::Bun,
        ProjectKind::Deno,
        ProjectKind::Python,
        ProjectKind::Lua,
        ProjectKind::C,
        ProjectKind::Cpp,
        ProjectKind::Unknown,
    ] {
        let s = k.as_str().into_owned();
        assert_eq!(ProjectKind::from_str(&s), Some(k.clone()));
    }
}

#[test]
fn project_kind_custom_roundtrips_via_custom_prefix() {
    let k = ProjectKind::Custom("wgsl".into());
    let s = k.as_str().into_owned();
    assert_eq!(s, "custom:wgsl");
    assert_eq!(ProjectKind::from_str(&s), Some(k));
}

#[test]
fn project_kind_cpp_accepts_cplusplus_alias() {
    assert_eq!(ProjectKind::from_str("c++"), Some(ProjectKind::Cpp));
    assert_eq!(ProjectKind::from_str("cpp"), Some(ProjectKind::Cpp));
}

#[test]
fn project_kind_serde_uses_external_tagging() {
    // Sanity: the JSON shape stays stable for any MCP / LSP
    // consumer that wires on top.
    let k = ProjectKind::Custom("wgsl".into());
    let json = serde_json::to_string(&k).unwrap();
    let back: ProjectKind = serde_json::from_str(&json).unwrap();
    assert_eq!(back, k);
}

#[test]
fn project_kind_from_str_rejects_unknown_garbage() {
    assert!(ProjectKind::from_str("scala").is_none());
    assert!(ProjectKind::from_str("").is_none());
}

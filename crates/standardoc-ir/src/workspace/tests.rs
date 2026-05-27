use super::*;

#[test]
fn link_direction_roundtrip_via_i64() {
    for d in [
        LinkDirection::In,
        LinkDirection::Out,
        LinkDirection::Bidirectional,
    ] {
        let n = d.as_i64();
        let back = LinkDirection::from_i64(n).expect("known value");
        assert_eq!(back, d);
    }
    assert!(LinkDirection::from_i64(3).is_none());
    assert!(LinkDirection::from_i64(-1).is_none());
}

#[test]
fn linked_workspace_status_roundtrip_via_str() {
    for s in [
        LinkedWorkspaceStatus::Active,
        LinkedWorkspaceStatus::Paused,
        LinkedWorkspaceStatus::Archived,
    ] {
        let txt = s.as_str();
        let back = LinkedWorkspaceStatus::from_str(txt).expect("known value");
        assert_eq!(back, s);
    }
    assert!(LinkedWorkspaceStatus::from_str("bogus").is_none());
}

#[test]
fn link_direction_serde_uses_snake_case() {
    let json = serde_json::to_string(&LinkDirection::Bidirectional).unwrap();
    assert_eq!(json, "\"bidirectional\"");
    let back: LinkDirection = serde_json::from_str(&json).unwrap();
    assert_eq!(back, LinkDirection::Bidirectional);
}

#[test]
fn workspace_kind_roundtrip_via_as_str_for_known_variants() {
    for k in [
        WorkspaceKind::Cargo,
        WorkspaceKind::Npm,
        WorkspaceKind::Pnpm,
        WorkspaceKind::Yarn,
        WorkspaceKind::Bun,
        WorkspaceKind::Deno,
        WorkspaceKind::Go,
        WorkspaceKind::Lerna,
        WorkspaceKind::Nx,
        WorkspaceKind::Turborepo,
        WorkspaceKind::Mira,
    ] {
        let s = k.as_str().into_owned();
        assert_eq!(WorkspaceKind::from_str(&s), k.clone());
        assert!(k.is_builtin());
    }
}

#[test]
fn workspace_kind_custom_roundtrips_via_custom_prefix() {
    let k = WorkspaceKind::Custom("bazel".into());
    let s = k.as_str().into_owned();
    assert_eq!(s, "custom:bazel");
    assert_eq!(WorkspaceKind::from_str(&s), k);
    assert!(!k.is_builtin());
}

#[test]
fn workspace_kind_unknown_slug_becomes_custom() {
    // Total function — unknown slugs are absorbed as Custom rather
    // than rejected. Mirrors `WorkspaceKindId::from_slug`.
    let k = WorkspaceKind::from_str("scala-sbt");
    assert_eq!(k, WorkspaceKind::Custom("scala-sbt".into()));
}

#[test]
fn workspace_kind_legacy_single_slug_becomes_custom() {
    // The `Single` variant was dropped in the 3e-3 revert to align with
    // `standarbuild-detect 0.3` (which has no `Single`). Legacy DBs
    // persisted with `"single"` round-trip as `Custom("single")` —
    // `delete_workspace_kind` purges the row at the next cold-start
    // when no workspace manifest is detected.
    let k = WorkspaceKind::from_str("single");
    assert_eq!(k, WorkspaceKind::Custom("single".into()));
}

#[test]
fn workspace_kind_serde_uses_external_tagging() {
    let k = WorkspaceKind::Custom("bazel".into());
    let json = serde_json::to_string(&k).unwrap();
    let back: WorkspaceKind = serde_json::from_str(&json).unwrap();
    assert_eq!(back, k);
}

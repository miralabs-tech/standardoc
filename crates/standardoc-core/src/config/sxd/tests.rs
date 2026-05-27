use super::*;

const FULL_SAMPLE: &str = r#"
version "0.1.0"

ignore {
  patterns ```
.git/
node_modules/
target/
```
}

projects {
  exclude ["crates-standardoc-graph-viz-pkg"]
}

group "standardoc" {
  label "Standardoc"
  members ["standardoc-core" "standardoc-ir"]
}

group "lurlang" {
  label "Lurlang"
  members ["lur-syntax" "lur-sema"]
}
"#;

#[test]
fn parses_full_sample() {
    let cfg = parse_sxd_source(FULL_SAMPLE).expect("parse full sample");
    assert_eq!(cfg.version.as_deref(), Some("0.1.0"));
    assert!(cfg.ignore.is_some());
    assert!(cfg.projects.is_some());
    assert_eq!(cfg.groups.len(), 2);
}

#[test]
fn ignore_patterns_preserved_verbatim() {
    let cfg = parse_sxd_source(FULL_SAMPLE).unwrap();
    let p = cfg.ignore.unwrap().patterns;
    assert!(p.contains(".git/"));
    assert!(p.contains("node_modules/"));
    assert!(p.contains("target/"));
}

#[test]
fn projects_exclude_collected() {
    let cfg = parse_sxd_source(FULL_SAMPLE).unwrap();
    let p = cfg.projects.unwrap();
    assert_eq!(
        p.exclude,
        vec!["crates-standardoc-graph-viz-pkg".to_string()]
    );
    assert!(p.include.is_empty());
}

#[test]
fn group_slug_label_members_extracted() {
    let cfg = parse_sxd_source(FULL_SAMPLE).unwrap();
    let g = cfg.groups.iter().find(|g| g.slug == "standardoc").unwrap();
    assert_eq!(g.label.as_deref(), Some("Standardoc"));
    assert_eq!(g.members, vec!["standardoc-core", "standardoc-ir"]);
}

#[test]
fn empty_sxd_yields_default_config() {
    let cfg = parse_sxd_source("").unwrap();
    assert_eq!(cfg.version, None);
    assert!(cfg.ignore.is_none());
    assert!(cfg.projects.is_none());
    assert!(cfg.groups.is_empty());
}

#[test]
fn unknown_block_kind_rejected() {
    let err = parse_sxd_source("unknown { foo \"bar\" }").expect_err("must reject");
    let msg = format!("{err}");
    assert!(msg.contains("unknown top-level block"));
}

#[test]
fn unknown_top_level_assign_rejected() {
    let err = parse_sxd_source("foo \"bar\"").expect_err("must reject");
    let msg = format!("{err}");
    assert!(msg.contains("unknown top-level assign"));
}

#[test]
fn group_without_label_rejected() {
    let err = parse_sxd_source("group { label \"X\" members [] }").expect_err("must reject");
    let msg = format!("{err}");
    assert!(msg.contains("requires a string label"));
}

#[test]
fn projects_with_unknown_field_rejected() {
    let err = parse_sxd_source("projects { foo [] }").expect_err("must reject");
    let msg = format!("{err}");
    assert!(msg.contains("unknown field `foo`"));
}

#[test]
fn group_with_unknown_field_rejected() {
    let err = parse_sxd_source(r#"group "g" { foo "bar" }"#).expect_err("must reject");
    let msg = format!("{err}");
    assert!(msg.contains("unknown field `foo`"));
}

#[test]
fn interpolation_rejected_in_plain_strings() {
    // standardoc.sxd v0.1 doesn't support ${env.X} — only standarbuild .sxb does.
    let src = r"version `${env.VERSION}`";
    let err = parse_sxd_source(src).expect_err("must reject interpolation");
    let msg = format!("{err}");
    assert!(
        msg.contains("interpolation"),
        "expected interpolation rejection, got: {msg}"
    );
}

#[test]
fn load_workspace_config_returns_none_when_absent() {
    let dir = tempfile::tempdir().unwrap();
    let result = load_workspace_config(dir.path()).expect("load ok");
    assert!(result.is_none());
}

#[test]
fn load_workspace_config_returns_some_when_present() {
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(SXD_CONFIG_FILENAME), r#"version "0.1.0""#).unwrap();
    let cfg = load_workspace_config(dir.path())
        .expect("load ok")
        .expect("config present");
    assert_eq!(cfg.version.as_deref(), Some("0.1.0"));
}

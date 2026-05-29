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

project "standardoc" {
  label "Standardoc"
  paths ["crates" "ext/vscode"]
}

project "lur-workspace" {
  label "Lurlang"
  path ".projects/lur-workspace"
}

group "platform" {
  label "Platform"
  members ["standardoc" "lur-workspace"]
}
"#;

#[test]
fn parses_full_sample() {
    let cfg = parse_sxd_source(FULL_SAMPLE).expect("parse full sample");
    assert_eq!(cfg.version.as_deref(), Some("0.1.0"));
    assert!(cfg.ignore.is_some());
    assert_eq!(cfg.projects.len(), 2);
    assert_eq!(cfg.groups.len(), 1);
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
fn project_paths_multi_collected() {
    let cfg = parse_sxd_source(FULL_SAMPLE).unwrap();
    let std = cfg
        .projects
        .iter()
        .find(|p| p.slug == "standardoc")
        .unwrap();
    assert_eq!(std.label.as_deref(), Some("Standardoc"));
    assert_eq!(std.paths, vec!["crates", "ext/vscode"]);
}

#[test]
fn project_path_single_expanded_to_paths() {
    let cfg = parse_sxd_source(FULL_SAMPLE).unwrap();
    let lur = cfg
        .projects
        .iter()
        .find(|p| p.slug == "lur-workspace")
        .unwrap();
    assert_eq!(lur.paths, vec![".projects/lur-workspace"]);
}

#[test]
fn group_slug_label_members_extracted() {
    let cfg = parse_sxd_source(FULL_SAMPLE).unwrap();
    let g = cfg.groups.iter().find(|g| g.slug == "platform").unwrap();
    assert_eq!(g.label.as_deref(), Some("Platform"));
    assert_eq!(g.members, vec!["standardoc", "lur-workspace"]);
}

#[test]
fn empty_sxd_yields_default_config() {
    let cfg = parse_sxd_source("").unwrap();
    assert_eq!(cfg.version, None);
    assert!(cfg.ignore.is_none());
    assert!(cfg.projects.is_empty());
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
fn project_without_slug_rejected() {
    let err = parse_sxd_source(r#"project { path "foo" }"#).expect_err("must reject");
    let msg = format!("{err}");
    assert!(msg.contains("requires a string slug"));
}

#[test]
fn project_without_paths_rejected() {
    let err = parse_sxd_source(r#"project "x" { label "X" }"#).expect_err("must reject");
    let msg = format!("{err}");
    assert!(msg.contains("must declare at least one"));
}

#[test]
fn project_path_and_paths_conflict_rejected() {
    let err = parse_sxd_source(r#"project "x" { path "a" paths ["b"] }"#).expect_err("must reject");
    let msg = format!("{err}");
    assert!(msg.contains("declares both `path` and `paths`"));
}

#[test]
fn project_with_unknown_field_rejected() {
    let err = parse_sxd_source(r#"project "x" { foo "bar" }"#).expect_err("must reject");
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

#[test]
fn ensure_sxd_seed_writes_template_when_neither_file_exists() {
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    ensure_sxd_seed_at(dir.path()).unwrap();

    let body = fs::read_to_string(dir.path().join(SXD_CONFIG_FILENAME)).unwrap();
    assert!(body.contains("version \"0.1.0\""));
    assert!(body.contains("ignore {"));
    assert!(body.contains(".git/"));
    assert!(body.contains("target/"));
    // Default template stays project-less so mechanical detection remains active.
    assert!(!body.contains("project \""));
}

#[test]
fn ensure_sxd_seed_preserves_existing_sxd() {
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let existing = "version \"0.1.0\"\n# user-authored\n";
    fs::write(dir.path().join(SXD_CONFIG_FILENAME), existing).unwrap();

    ensure_sxd_seed_at(dir.path()).unwrap();

    let body = fs::read_to_string(dir.path().join(SXD_CONFIG_FILENAME)).unwrap();
    assert_eq!(body, existing, "user .sxd must not be overwritten");
}

#[test]
fn ensure_sxd_seed_migrates_stdignore_to_ignore_block() {
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let legacy = "# my excludes\nfoo/\nbar/\n";
    fs::write(dir.path().join(".stdignore"), legacy).unwrap();

    ensure_sxd_seed_at(dir.path()).unwrap();

    let sxd = fs::read_to_string(dir.path().join(SXD_CONFIG_FILENAME)).unwrap();
    assert!(sxd.contains("Auto-migrated from .stdignore"));
    assert!(sxd.contains("foo/"));
    assert!(sxd.contains("bar/"));
    // Migrated .sxd must parse cleanly.
    let cfg = parse_sxd_source(&sxd).expect("migrated .sxd parses");
    let patterns = cfg.ignore.unwrap().patterns;
    assert!(patterns.contains("foo/"));
    assert!(patterns.contains("bar/"));

    // Legacy file moved to .stdignore.bak.
    assert!(!dir.path().join(".stdignore").exists());
    assert!(dir.path().join(".stdignore.bak").exists());
    let bak = fs::read_to_string(dir.path().join(".stdignore.bak")).unwrap();
    assert_eq!(
        bak, legacy,
        "backup preserves the original content verbatim"
    );
}

#[test]
fn ensure_sxd_seed_idempotent_when_sxd_present_with_stdignore_alongside() {
    // User may keep .stdignore around (for nested-cascade tooling). When .sxd
    // already exists, the seeder must NOT migrate / move / touch anything.
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(SXD_CONFIG_FILENAME), "version \"0.1.0\"\n").unwrap();
    fs::write(dir.path().join(".stdignore"), "leftover/\n").unwrap();

    ensure_sxd_seed_at(dir.path()).unwrap();

    assert!(
        dir.path().join(".stdignore").exists(),
        ".stdignore stays put"
    );
    assert!(
        !dir.path().join(".stdignore.bak").exists(),
        "no migration triggered when .sxd already exists",
    );
}

#[test]
fn mcp_block_parses_port() {
    let cfg = parse_sxd_source("mcp { port 7700 }").unwrap();
    assert_eq!(cfg.mcp.unwrap().port, Some(7700));
}

#[test]
fn viz_block_parses_port() {
    let cfg = parse_sxd_source("viz { port 3001 }").unwrap();
    assert_eq!(cfg.viz.unwrap().port, Some(3001));
}

#[test]
fn mcp_block_with_no_fields_yields_default() {
    let cfg = parse_sxd_source("mcp { }").unwrap();
    assert!(cfg.mcp.is_some());
    assert_eq!(cfg.mcp.unwrap().port, None);
}

#[test]
fn duplicate_mcp_block_rejected() {
    let err = parse_sxd_source("mcp { port 7700 }\nmcp { port 7701 }").expect_err("must reject");
    assert!(format!("{err}").contains("more than once"));
}

#[test]
fn port_zero_rejected_as_out_of_range() {
    let err = parse_sxd_source("mcp { port 0 }").expect_err("must reject port 0");
    assert!(format!("{err}").contains("out of TCP port range"));
}

#[test]
fn port_above_65535_rejected_as_out_of_range() {
    let err = parse_sxd_source("mcp { port 70000 }").expect_err("must reject port > 65535");
    assert!(format!("{err}").contains("out of TCP port range"));
}

#[test]
fn port_with_string_value_rejected() {
    let err = parse_sxd_source(r#"mcp { port "7700" }"#).expect_err("must reject string");
    assert!(format!("{err}").contains("expected an integer port"));
}

#[test]
fn mcp_with_unknown_field_rejected() {
    let err = parse_sxd_source("mcp { foo 7700 }").expect_err("must reject unknown field");
    assert!(format!("{err}").contains("unknown field `foo`"));
}

#[test]
fn proxy_block_rejected_as_unknown_top_level_block() {
    // The `proxy` block was removed from the .sxd schema — the proxy
    // is a per-machine singleton, configured via VSCode settings.
    // Existing configs with a `proxy { ... }` block now fail loudly so
    // the user knows to migrate.
    let err = parse_sxd_source(r#"proxy { bind "127.0.0.1" port 7700 }"#)
        .expect_err("proxy block must be rejected");
    assert!(format!("{err}").contains("unknown top-level block `proxy`"));
}

#[test]
fn parses_real_standardoc_workspace_template() {
    // Regression check : the template authored at the workspace root
    // (see standardoc.sxd) must parse cleanly with the production schema.
    let src = include_str!("../../../../../standardoc.sxd");
    let cfg = parse_sxd_source(src).expect("parse real .sxd");
    assert_eq!(cfg.version.as_deref(), Some("0.1.0"));
    assert!(cfg.ignore.is_some(), "ignore block present");
    let slugs: Vec<&str> = cfg.projects.iter().map(|p| p.slug.as_str()).collect();
    assert!(slugs.contains(&"standardoc"));
    assert!(slugs.contains(&"lur-workspace"));
    assert!(slugs.contains(&"matchigo-lua"));
    assert!(slugs.contains(&"standarx-dsl"));
}

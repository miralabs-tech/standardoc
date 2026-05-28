//! Smoke test : parse a real-world `.sxb` file (the standardoc workspace's
//! standarbuild config) to verify the DSL round-trips intact through the
//! extracted crate.

const SXB_SAMPLE: &str = r#"
# Auto-generated from {WorkspaceFolder} by `standarbuild init`.
version "0.1.0"

project "ext-vscode" {
  path "./ext/vscode"
  name "ext-vscode"
  type "bun"

  metadata {
    env_keys ["VSCE_PAT" "OVSX_PAT"]
  }

  task "install" { cmd "bun install" depends_on [task.version] }
  task "build" { cmd "bun run build" }
}
"#;

#[test]
fn parses_standarbuild_sample() {
    let file = standarx_dsl::parse(SXB_SAMPLE).expect("parse sxb sample");
    assert!(!file.stmts.is_empty(), "expected at least one stmt");
    // version "0.1.0" + project { ... } => 2 top-level stmts
    assert_eq!(file.stmts.len(), 2);
}

#[test]
fn parses_proposed_sxd_template() {
    // Proposed standardoc.sxd v0.1 template — verifies the DSL supports
    // the schema this ADR locks in.
    // Multi-line raw strings use triple-backticks (markdown-style).
    let src = r#"
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
  members [
    "standardoc-core"
    "standardoc-ir"
  ]
}
"#;
    let file = standarx_dsl::parse(src).expect("parse sxd template");
    // version + ignore + projects + group  = 4 stmts
    assert_eq!(file.stmts.len(), 4);
}

#[test]
fn empty_input_parses() {
    let file = standarx_dsl::parse("").expect("empty parse");
    assert!(file.stmts.is_empty());
}

#[test]
fn parse_error_carries_span() {
    // `{` without preceding ident is a parse error.
    let err = standarx_dsl::parse("{ broken }").expect_err("expected parse error");
    assert!(err.is_error());
}

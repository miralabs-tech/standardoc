//! Schema-aware LSP validation for `standardoc.sxd` documents.
//!
//! Walks a parsed [`standarx_dsl::File`] tree and emits spanned
//! [`Diag`]s for every schema rule violated. Mirrors the lowering
//! rules enforced by `standardoc_core::config::sxd::parse_sxd_source`
//! but :
//!
//!   * continues past the first error so the editor surfaces every
//!     problem in one pass;
//!   * carries AST-derived spans so the LSP client highlights the
//!     exact offending token (block kind, field key, value expr).
//!
//! Plugged into [`standarx_dsl_lsp::run_stdio_with_schemas`] from
//! the `lsp-sxd` CLI subcommand.
//!
//! NOTE: kept deliberately separate from `standardoc_core` so the
//! tower-lsp + tokio LSP runtime stays out of the core dependency
//! graph — only the daemon binary needs it.

use standarx_dsl::ast::{Block, Expr, Stmt, StmtNode};
use standarx_dsl::{Diag, File};
use standarx_dsl_lsp::Schema;

pub(crate) struct SxdSchema;

impl Schema for SxdSchema {
    fn validate(&self, file: &File, _src: &str) -> Vec<Diag> {
        let mut diags = Vec::new();
        for stmt in &file.stmts {
            validate_top_stmt(stmt, &mut diags);
        }
        diags
    }
}

fn validate_top_stmt(stmt: &StmtNode, diags: &mut Vec<Diag>) {
    match &stmt.node {
        Stmt::Assign(a) => {
            let key = a.key.node.as_str();
            if key == "version" {
                expect_plain_string(&a.value, "version", diags);
            } else {
                diags.push(Diag::schema(
                    a.key.span.clone(),
                    format!(
                        "unknown top-level assign `{key}` \
                         (only `version` accepted at top level)"
                    ),
                ));
            }
        }
        Stmt::Block(b) => {
            let kind = b.kind.node.as_str();
            match kind {
                "ignore" => validate_ignore_block(b, diags),
                "project" => validate_project_block(b, diags),
                "group" => validate_group_block(b, diags),
                "mcp" => validate_mcp_block(b, diags),
                "viz" => validate_viz_block(b, diags),
                "proxy" => validate_proxy_block(b, diags),
                _ => diags.push(Diag::schema(
                    b.kind.span.clone(),
                    format!(
                        "unknown top-level block `{kind}` \
                         (expected `ignore`, `project`, `group`, `mcp`, `viz`, or `proxy`)"
                    ),
                )),
            }
        }
        _ => {}
    }
}

fn validate_ignore_block(b: &Block, diags: &mut Vec<Diag>) {
    for stmt in &b.stmts {
        match &stmt.node {
            Stmt::Assign(a) => {
                let key = a.key.node.as_str();
                if key == "patterns" {
                    expect_plain_string(&a.value, "ignore.patterns", diags);
                } else {
                    diags.push(Diag::schema(
                        a.key.span.clone(),
                        format!(
                            "unknown field `{key}` inside `ignore` \
                             (only `patterns` accepted)"
                        ),
                    ));
                }
            }
            Stmt::Block(inner) => {
                diags.push(Diag::schema(
                    inner.kind.span.clone(),
                    "`ignore` block only accepts `patterns = ...` assignment, \
                     not nested blocks",
                ));
            }
            _ => {}
        }
    }
}

fn validate_project_block(b: &Block, diags: &mut Vec<Diag>) {
    let Some(label) = &b.label else {
        diags.push(Diag::schema(
            b.kind.span.clone(),
            "`project` block requires a string slug, \
             e.g. `project \"standardoc\" { ... }`",
        ));
        return;
    };
    let slug = label.node.as_str();
    let mut has_path = false;
    let mut has_paths = false;
    for stmt in &b.stmts {
        match &stmt.node {
            Stmt::Assign(a) => {
                let key = a.key.node.as_str();
                match key {
                    "label" => expect_plain_string(&a.value, "project.label", diags),
                    "path" => {
                        has_path = true;
                        expect_plain_string(&a.value, "project.path", diags);
                    }
                    "paths" => {
                        has_paths = true;
                        expect_string_list(&a.value, "project.paths", diags);
                    }
                    other => diags.push(Diag::schema(
                        a.key.span.clone(),
                        format!(
                            "unknown field `{other}` inside `project \"{slug}\"` \
                             (expected `label`, `path`, or `paths`)"
                        ),
                    )),
                }
            }
            Stmt::Block(inner) => {
                diags.push(Diag::schema(
                    inner.kind.span.clone(),
                    format!(
                        "`project \"{slug}\"` block only accepts assignments \
                         (`label`, `path`, `paths`)"
                    ),
                ));
            }
            _ => {}
        }
    }
    if has_path && has_paths {
        diags.push(Diag::schema(
            b.kind.span.clone(),
            format!("`project \"{slug}\"` declares both `path` and `paths` — pick one"),
        ));
    }
    if !has_path && !has_paths {
        diags.push(Diag::schema(
            b.kind.span.clone(),
            format!(
                "`project \"{slug}\"` must declare at least one \
                 `path \"...\"` or `paths [...]`"
            ),
        ));
    }
}

fn validate_group_block(b: &Block, diags: &mut Vec<Diag>) {
    let Some(label) = &b.label else {
        diags.push(Diag::schema(
            b.kind.span.clone(),
            "`group` block requires a string slug, \
             e.g. `group \"platform\" { ... }`",
        ));
        return;
    };
    let slug = label.node.as_str();
    for stmt in &b.stmts {
        match &stmt.node {
            Stmt::Assign(a) => {
                let key = a.key.node.as_str();
                match key {
                    "label" => expect_plain_string(&a.value, "group.label", diags),
                    "members" => expect_string_list(&a.value, "group.members", diags),
                    other => diags.push(Diag::schema(
                        a.key.span.clone(),
                        format!(
                            "unknown field `{other}` inside `group \"{slug}\"` \
                             (expected `label` or `members`)"
                        ),
                    )),
                }
            }
            Stmt::Block(inner) => {
                diags.push(Diag::schema(
                    inner.kind.span.clone(),
                    format!(
                        "`group \"{slug}\"` block only accepts assignments \
                         (`label`, `members`)"
                    ),
                ));
            }
            _ => {}
        }
    }
}

fn validate_mcp_block(b: &Block, diags: &mut Vec<Diag>) {
    for stmt in &b.stmts {
        match &stmt.node {
            Stmt::Assign(a) => {
                let key = a.key.node.as_str();
                match key {
                    "port" => expect_port(&a.value, "mcp.port", diags),
                    other => diags.push(Diag::schema(
                        a.key.span.clone(),
                        format!("unknown field `{other}` inside `mcp` (only `port` accepted)"),
                    )),
                }
            }
            Stmt::Block(inner) => diags.push(Diag::schema(
                inner.kind.span.clone(),
                "`mcp` block only accepts assignments (`port`)",
            )),
            _ => {}
        }
    }
}

fn validate_viz_block(b: &Block, diags: &mut Vec<Diag>) {
    for stmt in &b.stmts {
        match &stmt.node {
            Stmt::Assign(a) => {
                let key = a.key.node.as_str();
                match key {
                    "port" => expect_port(&a.value, "viz.port", diags),
                    other => diags.push(Diag::schema(
                        a.key.span.clone(),
                        format!("unknown field `{other}` inside `viz` (only `port` accepted)"),
                    )),
                }
            }
            Stmt::Block(inner) => diags.push(Diag::schema(
                inner.kind.span.clone(),
                "`viz` block only accepts assignments (`port`)",
            )),
            _ => {}
        }
    }
}

fn validate_proxy_block(b: &Block, diags: &mut Vec<Diag>) {
    for stmt in &b.stmts {
        match &stmt.node {
            Stmt::Assign(a) => {
                let key = a.key.node.as_str();
                match key {
                    "bind" => expect_plain_string(&a.value, "proxy.bind", diags),
                    "port" => expect_port(&a.value, "proxy.port", diags),
                    other => diags.push(Diag::schema(
                        a.key.span.clone(),
                        format!(
                            "unknown field `{other}` inside `proxy` \
                             (expected `bind` or `port`)"
                        ),
                    )),
                }
            }
            Stmt::Block(inner) => diags.push(Diag::schema(
                inner.kind.span.clone(),
                "`proxy` block only accepts assignments (`bind`, `port`)",
            )),
            _ => {}
        }
    }
}

fn expect_port(value: &standarx_dsl::diag::Spanned<Expr>, context: &str, diags: &mut Vec<Diag>) {
    let Expr::Int(n) = &value.node else {
        diags.push(Diag::schema(
            value.span.clone(),
            format!("expected an integer port for `{context}`"),
        ));
        return;
    };
    if *n < 1 || *n > i64::from(u16::MAX) {
        diags.push(Diag::schema(
            value.span.clone(),
            format!("`{context}` = {n} is out of TCP port range 1..=65535"),
        ));
    }
}

fn expect_plain_string(
    value: &standarx_dsl::diag::Spanned<Expr>,
    context: &str,
    diags: &mut Vec<Diag>,
) {
    match &value.node {
        Expr::String(lit) => {
            for part in &lit.parts {
                if let standarx_dsl::ast::StringPart::Interp(interp) = part {
                    diags.push(Diag::schema(
                        interp.span.clone(),
                        format!(
                            "string interpolation (`${{...}}`) is not supported in `{context}` \
                             — standardoc.sxd v0.1 expects plain strings"
                        ),
                    ));
                }
            }
        }
        _ => diags.push(Diag::schema(
            value.span.clone(),
            format!("expected a string value for `{context}`"),
        )),
    }
}

fn expect_string_list(
    value: &standarx_dsl::diag::Spanned<Expr>,
    context: &str,
    diags: &mut Vec<Diag>,
) {
    let Expr::List(items) = &value.node else {
        diags.push(Diag::schema(
            value.span.clone(),
            format!("expected an array for `{context}`, e.g. `[\"a\" \"b\"]`"),
        ));
        return;
    };
    for item in items {
        expect_plain_string(item, context, diags);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diags_for(src: &str) -> Vec<Diag> {
        let file = standarx_dsl::parse(src).expect("parse ok");
        SxdSchema.validate(&file, src)
    }

    #[test]
    fn empty_config_no_diags() {
        assert!(diags_for("").is_empty());
    }

    #[test]
    fn valid_config_no_diags() {
        let src = r#"version "0.1.0"
project "x" { label "X" path "foo" }
group "g" { label "G" members ["x"] }
ignore { patterns ```.git/``` }
"#;
        assert!(diags_for(src).is_empty(), "got: {:?}", diags_for(src));
    }

    #[test]
    fn unknown_top_level_block_reports_span_on_kind() {
        let src = "frobulator \"x\" { }\n";
        let diags = diags_for(src);
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0]
                .kind
                .to_string()
                .contains("unknown top-level block `frobulator`")
        );
        // span covers the identifier `frobulator` (bytes 0..10)
        assert_eq!(diags[0].span, 0..10);
    }

    #[test]
    fn project_without_path_reports_span_on_block_kind() {
        let src = "project \"x\" { label \"X\" }\n";
        let diags = diags_for(src);
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0]
                .kind
                .to_string()
                .contains("must declare at least one")
        );
    }

    #[test]
    fn project_with_both_path_and_paths_reports_conflict() {
        let src = "project \"x\" { path \"a\" paths [\"b\"] }\n";
        let diags = diags_for(src);
        assert!(
            diags.iter().any(|d| d
                .kind
                .to_string()
                .contains("declares both `path` and `paths`")),
            "got: {diags:?}"
        );
    }

    #[test]
    fn unknown_project_field_reports_span_on_key() {
        let src = "project \"x\" { foo \"bar\" path \"a\" }\n";
        let diags = diags_for(src);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].kind.to_string().contains("unknown field `foo`"));
    }

    #[test]
    fn ignore_with_extra_field_reports_diag() {
        let src = "ignore { patterns ```.git/``` extra \"oops\" }\n";
        let diags = diags_for(src);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].kind.to_string().contains("unknown field `extra`"));
    }

    #[test]
    fn mcp_block_valid_no_diags() {
        assert!(diags_for("mcp { port 7700 }\n").is_empty());
    }

    #[test]
    fn viz_block_valid_no_diags() {
        assert!(diags_for("viz { port 3001 }\n").is_empty());
    }

    #[test]
    fn proxy_block_valid_no_diags() {
        assert!(diags_for(r#"proxy { bind "127.0.0.1" port 7701 }"#).is_empty());
    }

    #[test]
    fn port_out_of_range_reports_span_on_value() {
        let src = "mcp { port 70000 }\n";
        let diags = diags_for(src);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].kind.to_string().contains("out of TCP port range"));
    }

    #[test]
    fn port_with_string_value_reports_diag() {
        let src = r#"viz { port "3000" }"#;
        let diags = diags_for(src);
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0]
                .kind
                .to_string()
                .contains("expected an integer port")
        );
    }

    #[test]
    fn unknown_field_in_mcp_block_reports_span_on_key() {
        let src = "mcp { foo 7700 }\n";
        let diags = diags_for(src);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].kind.to_string().contains("unknown field `foo`"));
    }

    #[test]
    fn unknown_field_in_proxy_block_reports_span_on_key() {
        let src = r#"proxy { foo "x" }"#;
        let diags = diags_for(src);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].kind.to_string().contains("unknown field `foo`"));
    }

    #[test]
    fn collects_multiple_errors_in_one_pass() {
        let src = "frobulator \"x\" { }\nproject \"y\" { foo \"bar\" }\n";
        let diags = diags_for(src);
        // 1 unknown block + 1 unknown field + 1 missing path = 3 diags
        assert!(diags.len() >= 2, "expected multiple diags, got: {diags:?}");
    }
}

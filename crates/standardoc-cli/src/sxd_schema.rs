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

use standarx_dsl::ast::{Block, Expr, Stmt, StmtNode, StringPart};
use standarx_dsl::diag::Span;
use standarx_dsl::{Diag, File};
use standarx_dsl_lsp::Schema;
use standarx_dsl_lsp::conversion::span_to_range;
use standarx_dsl_lsp::schema::Hover;
use tower_lsp::lsp_types::{HoverContents, MarkupContent, MarkupKind};

/// Schema version this build of standardoc accepts in `.sxd` files.
/// Surfaced verbatim by the `version` hover so the editor can warn
/// when a workspace pins a different version than the binary.
pub(crate) const SUPPORTED_SXD_VERSION: &str = "0.1.0";

pub(crate) struct SxdSchema;

impl Schema for SxdSchema {
    fn validate(&self, file: &File, _src: &str) -> Vec<Diag> {
        let mut diags = Vec::new();
        for stmt in &file.stmts {
            validate_top_stmt(stmt, &mut diags);
        }
        diags
    }

    fn hover(&self, file: &File, src: &str, offset: usize) -> Option<Hover> {
        for stmt in &file.stmts {
            if let Some(h) = hover_in_top_stmt(stmt, src, offset) {
                return Some(h);
            }
        }
        None
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
                _ => diags.push(Diag::schema(
                    b.kind.span.clone(),
                    format!(
                        "unknown top-level block `{kind}` \
                         (expected `ignore`, `project`, `group`, `mcp`, or `viz`)"
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
                if let StringPart::Interp(interp) = part {
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

// ---------- hover ----------

fn hover_in_top_stmt(stmt: &StmtNode, src: &str, offset: usize) -> Option<Hover> {
    match &stmt.node {
        Stmt::Assign(a) => {
            if !span_contains(&a.key.span, offset) {
                return None;
            }
            if a.key.node.as_str() == "version" {
                let declared = string_value(&a.value.node).unwrap_or("<unset>");
                Some(markdown_hover(version_doc(declared), src, &a.key.span))
            } else {
                // Unknown top-level key — diagnostics already flag it,
                // hover stays silent to avoid drowning the squiggle.
                None
            }
        }
        Stmt::Block(b) => {
            if span_contains(&b.kind.span, offset) {
                return Some(markdown_hover(
                    block_kind_doc(b.kind.node.as_str()),
                    src,
                    &b.kind.span,
                ));
            }
            for inner in &b.stmts {
                if let Some(h) = hover_in_inner_stmt(inner, b.kind.node.as_str(), src, offset) {
                    return Some(h);
                }
            }
            None
        }
        _ => None,
    }
}

fn hover_in_inner_stmt(
    stmt: &StmtNode,
    parent_kind: &str,
    src: &str,
    offset: usize,
) -> Option<Hover> {
    let Stmt::Assign(a) = &stmt.node else {
        return None;
    };
    if !span_contains(&a.key.span, offset) {
        return None;
    }
    let doc = field_key_doc(parent_kind, a.key.node.as_str())?;
    Some(markdown_hover(doc, src, &a.key.span))
}

const fn span_contains(span: &Span, offset: usize) -> bool {
    // Inclusive on both ends — LSP can land the cursor right at the
    // token boundary (e.g. after typing the last char) and the user
    // still expects hover to fire on that token.
    offset >= span.start && offset <= span.end
}

fn string_value(expr: &Expr) -> Option<&str> {
    let Expr::String(lit) = expr else {
        return None;
    };
    lit.parts.iter().find_map(|p| match p {
        StringPart::Lit(s) => Some(s.as_str()),
        // `StringPart` is `#[non_exhaustive]` (currently only `Lit`
        // and `Interp`) — extra variants degrade to "no plain value"
        // rather than crashing on a hover.
        _ => None,
    })
}

fn markdown_hover(value: String, src: &str, span: &Span) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: Some(span_to_range(src, span)),
    }
}

fn version_doc(declared: &str) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    s.push_str("**`version`** — schema version this `.sxd` file targets.\n\n");
    let _ = writeln!(s, "- Supported by this build : `{SUPPORTED_SXD_VERSION}`");
    let _ = writeln!(s, "- Declared in this file : `{declared}`");
    if declared != SUPPORTED_SXD_VERSION && declared != "<unset>" {
        s.push_str(
            "\n_Version mismatch — the parser may accept it for forward-compat \
             but some fields can be ignored._\n",
        );
    }
    s
}

fn block_kind_doc(kind: &str) -> String {
    match kind {
        "ignore" => "**`ignore`** — path patterns excluded from indexing.\n\n\
                     Single field : `patterns` (triple-backtick block of \
                     gitignore-syntax lines, one per line)."
            .to_string(),
        "project" => "**`project \"<slug>\"`** — explicit project root.\n\n\
                      When at least one `project` block is declared, mechanical \
                      detection (cargo workspaces, npm packages, lua rockspecs) is \
                      short-circuited and only these projects are indexed.\n\n\
                      Fields : `label`, `path` *or* `paths`."
            .to_string(),
        "group" => "**`group \"<slug>\"`** — logical grouping of projects.\n\n\
                    Used for cross-workspace views and roll-up displays in the viz.\n\n\
                    Fields : `label`, `members`."
            .to_string(),
        "mcp" => "**`mcp`** — local MCP HTTP transport.\n\n\
                  Field : `port` (default `7700`).\n\n\
                  Read by the daemon when binding the MCP endpoint ; \
                  overridable on the CLI."
            .to_string(),
        "viz" => "**`viz`** — graph viz playground.\n\n\
                  Field : `port` (default `3000`).\n\n\
                  Read by the `Standardoc: Open Graph Viz` command to know which \
                  port to probe / open."
            .to_string(),
        other => format!("**`{other}`** — unknown block kind."),
    }
}

fn field_key_doc(parent_kind: &str, key: &str) -> Option<String> {
    let doc = match (parent_kind, key) {
        ("ignore", "patterns") => {
            "**`patterns`** — triple-backtick block of gitignore-syntax lines, \
             one per line. Blank lines and `#` comments are accepted."
        }
        ("project", "label") => {
            "**`label \"...\"`** — display name shown in trees, viz, and CLI \
             listings. Optional ; defaults to the slug."
        }
        ("project", "path") => {
            "**`path \"...\"`** — single workspace-relative directory to index for \
             this project. Mutually exclusive with `paths`."
        }
        ("project", "paths") => {
            "**`paths [ \"...\" ... ]`** — list of workspace-relative directories \
             to index for this project. Mutually exclusive with `path`."
        }
        ("group", "label") => {
            "**`label \"...\"`** — display name for this group. Optional ; defaults \
             to the slug."
        }
        ("group", "members") => {
            "**`members [ \"<project-slug>\" ... ]`** — project slugs that belong \
             to this group. Each slug must match a `project` block declared \
             elsewhere in this file."
        }
        ("mcp" | "viz", "port") => "**`port <u16>`** — TCP port (1..=65535).",
        _ => return None,
    };
    Some(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diags_for(src: &str) -> Vec<Diag> {
        let file = standarx_dsl::parse(src).expect("parse ok");
        SxdSchema.validate(&file, src)
    }

    fn hover_at(src: &str, needle: &str) -> Option<String> {
        let file = standarx_dsl::parse(src).expect("parse ok");
        let pos = src.find(needle).expect("needle present");
        let offset = pos + needle.len() / 2;
        let h = SxdSchema.hover(&file, src, offset)?;
        match h.contents {
            HoverContents::Markup(m) => Some(m.value),
            _ => None,
        }
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
    fn proxy_block_rejected_as_unknown_top_level() {
        // `proxy { ... }` was removed from the schema in Bug F cleanup —
        // it's a per-machine concern, not a workspace concern. Existing
        // configs are rejected loudly so the user knows to migrate.
        let src = r#"proxy { bind "127.0.0.1" port 7701 }"#;
        let diags = diags_for(src);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(
            diags[0]
                .kind
                .to_string()
                .contains("unknown top-level block `proxy`")
        );
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
    fn collects_multiple_errors_in_one_pass() {
        let src = "frobulator \"x\" { }\nproject \"y\" { foo \"bar\" }\n";
        let diags = diags_for(src);
        // 1 unknown block + 1 unknown field + 1 missing path = 3 diags
        assert!(diags.len() >= 2, "expected multiple diags, got: {diags:?}");
    }

    // ---------- hover ----------

    #[test]
    fn hover_on_version_key_shows_supported_and_declared() {
        let src = "version \"0.1.0\"\n";
        let md = hover_at(src, "version").expect("hover");
        assert!(md.contains("Supported by this build"), "got: {md}");
        assert!(md.contains("0.1.0"), "got: {md}");
        assert!(!md.contains("Version mismatch"), "got: {md}");
    }

    #[test]
    fn hover_on_version_key_flags_mismatch() {
        let src = "version \"0.9.9\"\n";
        let md = hover_at(src, "version").expect("hover");
        assert!(md.contains("Version mismatch"), "got: {md}");
    }

    #[test]
    fn hover_on_block_kind_returns_doc() {
        let src = "project \"x\" { path \"foo\" }\n";
        let md = hover_at(src, "project").expect("hover");
        assert!(md.contains("explicit project root"), "got: {md}");
    }

    #[test]
    fn hover_on_each_block_kind_has_distinct_doc() {
        for (src, marker) in [
            ("ignore { patterns ```a``` }\n", "ignore"),
            ("group \"g\" { members [\"x\"] }\n", "group"),
            ("mcp { port 7700 }\n", "mcp"),
            ("viz { port 3000 }\n", "viz"),
        ] {
            let md = hover_at(src, marker).unwrap_or_else(|| panic!("no hover for `{marker}`"));
            assert!(
                md.to_lowercase().contains(marker),
                "expected `{marker}` doc to mention itself, got: {md}"
            );
        }
    }

    #[test]
    fn hover_on_field_key_scoped_by_parent_block() {
        let src = "project \"x\" { path \"foo\" }\n";
        let md = hover_at(src, "path").expect("hover");
        assert!(md.contains("single workspace-relative"), "got: {md}");
        assert!(md.contains("Mutually exclusive"), "got: {md}");
    }

    #[test]
    fn hover_on_port_inside_mcp_block() {
        let src = "mcp { port 7700 }\n";
        let md = hover_at(src, "port").expect("hover");
        assert!(md.contains("TCP port"), "got: {md}");
    }

    #[test]
    fn hover_outside_known_tokens_returns_none() {
        let src = "project \"x\" { path \"foo\" }\n";
        // cursor inside the value "foo" — TS hover handles this, LSP stays silent.
        let h = {
            let file = standarx_dsl::parse(src).unwrap();
            let pos = src.find("foo").unwrap();
            SxdSchema.hover(&file, src, pos + 1)
        };
        assert!(h.is_none(), "value hover should be TS-side, not LSP");
    }

    #[test]
    fn hover_on_unknown_field_key_returns_none() {
        let src = "project \"x\" { frobulate \"y\" path \"a\" }\n";
        let h = {
            let file = standarx_dsl::parse(src).unwrap();
            let pos = src.find("frobulate").unwrap();
            SxdSchema.hover(&file, src, pos)
        };
        assert!(h.is_none(), "unknown key has no doc");
    }

    #[test]
    fn hover_carries_range_spanning_the_key_token() {
        let src = "version \"0.1.0\"\n";
        let file = standarx_dsl::parse(src).unwrap();
        let pos = src.find("version").unwrap();
        let h = SxdSchema.hover(&file, src, pos).expect("hover");
        let range = h.range.expect("range present");
        assert_eq!(range.start.line, 0);
        assert_eq!(range.start.character, 0);
        assert_eq!(range.end.character, u32::try_from("version".len()).unwrap());
    }
}

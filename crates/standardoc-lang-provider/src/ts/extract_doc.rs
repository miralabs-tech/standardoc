use swc_core::common::BytePos;
use swc_core::common::comments::{Comment, CommentKind, Comments};

/// Extract a JSDoc description from the leading block comments before the
/// given span. Convention: a JSDoc block is a block comment whose text starts
/// with `*` (i.e. the source is `/** ... */`). When multiple JSDoc blocks
/// precede the span, only the closest one wins.
///
/// We strip the leading `*` from the first line and the ` * ` / `*` prefix
/// from continuation lines, then trim the result. Returns `None` if no JSDoc
/// block is attached to `pos`.
///
/// Generic over `C: Comments` because `Comments::with_leading` requires
/// `Self: Sized`, so it cannot be invoked through `&dyn Comments`. Callers
/// pass `&SingleThreadedComments` (the concrete impl built at parse time).
pub(crate) fn extract_at<C: Comments>(comments: &C, pos: BytePos) -> Option<String> {
    let mut out: Option<String> = None;
    comments.with_leading(pos, |list: &[Comment]| {
        if let Some(jsdoc) = list.iter().rev().find(|c| is_jsdoc_block(c)) {
            out = Some(strip_jsdoc(&jsdoc.text));
        }
    });
    out.filter(|s| !s.is_empty())
}

fn is_jsdoc_block(comment: &Comment) -> bool {
    comment.kind == CommentKind::Block && comment.text.starts_with('*')
}

fn strip_jsdoc(text: &str) -> String {
    let inner = text.strip_prefix('*').unwrap_or(text);
    let mut lines: Vec<String> = Vec::new();
    for raw in inner.lines() {
        lines.push(strip_line_prefix(raw));
    }
    while lines.first().is_some_and(String::is_empty) {
        lines.remove(0);
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n")
}

fn strip_line_prefix(line: &str) -> String {
    let trimmed = line.trim_start();
    let after_star = trimmed.strip_prefix('*').unwrap_or(trimmed);
    after_star.strip_prefix(' ').unwrap_or(after_star).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use swc_core::common::FileName;
    use swc_core::common::comments::SingleThreadedComments;
    use swc_core::common::sync::Lrc;
    use swc_core::common::SourceMap;
    use swc_core::ecma::ast::EsVersion;
    use swc_core::ecma::parser::{Parser, StringInput, Syntax, TsSyntax, lexer::Lexer};

    fn parse_with_comments(src: &str) -> (SingleThreadedComments, Vec<BytePos>) {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(
            Lrc::new(FileName::Custom("test.ts".into())),
            src.to_string(),
        );
        let comments = SingleThreadedComments::default();
        let lexer = Lexer::new(
            Syntax::Typescript(TsSyntax {
                tsx: false,
                decorators: false,
                dts: false,
                no_early_errors: true,
                disallow_ambiguous_jsx_like: false,
            }),
            EsVersion::EsNext,
            StringInput::from(&*fm),
            Some(&comments),
        );
        let mut parser = Parser::new_from(lexer);
        let module = parser.parse_module().expect("parse ok");
        let lo_positions: Vec<BytePos> =
            module.body.iter().map(|item| match item {
                swc_core::ecma::ast::ModuleItem::ModuleDecl(decl) => match decl {
                    swc_core::ecma::ast::ModuleDecl::ExportDecl(d) => d.span.lo,
                    swc_core::ecma::ast::ModuleDecl::ExportDefaultDecl(d) => d.span.lo,
                    swc_core::ecma::ast::ModuleDecl::Import(d) => d.span.lo,
                    swc_core::ecma::ast::ModuleDecl::ExportAll(d) => d.span.lo,
                    swc_core::ecma::ast::ModuleDecl::ExportDefaultExpr(d) => d.span.lo,
                    swc_core::ecma::ast::ModuleDecl::ExportNamed(d) => d.span.lo,
                    swc_core::ecma::ast::ModuleDecl::TsExportAssignment(d) => d.span.lo,
                    swc_core::ecma::ast::ModuleDecl::TsImportEquals(d) => d.span.lo,
                    swc_core::ecma::ast::ModuleDecl::TsNamespaceExport(d) => d.span.lo,
                },
                swc_core::ecma::ast::ModuleItem::Stmt(stmt) => match stmt {
                    swc_core::ecma::ast::Stmt::Decl(d) => match d {
                        swc_core::ecma::ast::Decl::Fn(f) => f.function.span.lo,
                        swc_core::ecma::ast::Decl::Class(c) => c.class.span.lo,
                        swc_core::ecma::ast::Decl::Var(v) => v.span.lo,
                        swc_core::ecma::ast::Decl::TsInterface(i) => i.span.lo,
                        swc_core::ecma::ast::Decl::TsTypeAlias(t) => t.span.lo,
                        swc_core::ecma::ast::Decl::TsEnum(e) => e.span.lo,
                        swc_core::ecma::ast::Decl::TsModule(m) => m.span.lo,
                        swc_core::ecma::ast::Decl::Using(u) => u.span.lo,
                    },
                    other => panic!("unsupported stmt: {other:?}"),
                },
            }).collect();
        (comments, lo_positions)
    }

    #[test]
    fn jsdoc_block_extracted_with_strip_markers() {
        let src = "/**\n * Creates a new user.\n */\nexport function makeUser() {}\n";
        let (comments, los) = parse_with_comments(src);
        let pos = los[0];
        let out = extract_at(&comments, pos);
        assert_eq!(out.as_deref(), Some("Creates a new user."));
    }

    #[test]
    fn jsdoc_multiline_joins_with_newline() {
        let src = "/**\n * First line.\n * Second line.\n */\nexport function f() {}\n";
        let (comments, los) = parse_with_comments(src);
        let out = extract_at(&comments, los[0]).unwrap();
        assert_eq!(out, "First line.\nSecond line.");
    }

    #[test]
    fn non_jsdoc_block_is_ignored() {
        let src = "/* Regular comment */\nexport function foo() {}\n";
        let (comments, los) = parse_with_comments(src);
        assert!(extract_at(&comments, los[0]).is_none());
    }

    #[test]
    fn line_comments_are_ignored() {
        let src = "// Just a regular comment.\nexport function foo() {}\n";
        let (comments, los) = parse_with_comments(src);
        assert!(extract_at(&comments, los[0]).is_none());
    }

    #[test]
    fn no_leading_comments_returns_none() {
        let src = "export function foo() {}\n";
        let (comments, los) = parse_with_comments(src);
        assert!(extract_at(&comments, los[0]).is_none());
    }

    #[test]
    fn at_tags_kept_inline_in_description() {
        let src = "/**\n * Computes sum.\n * @param a first\n */\nexport function add() {}\n";
        let (comments, los) = parse_with_comments(src);
        let out = extract_at(&comments, los[0]).unwrap();
        assert_eq!(out, "Computes sum.\n@param a first");
    }

    #[test]
    fn closest_jsdoc_block_wins_when_two_present() {
        let src =
            "/**\n * First doc.\n */\n/**\n * Second doc.\n */\nexport function foo() {}\n";
        let (comments, los) = parse_with_comments(src);
        let out = extract_at(&comments, los[0]).unwrap();
        assert_eq!(out, "Second doc.");
    }
}

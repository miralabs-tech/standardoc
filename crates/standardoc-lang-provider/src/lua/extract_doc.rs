use full_moon::tokenizer::{Token, TokenReference, TokenType};
use standardoc_ir::RawDocument;

/// Capture the doc block immediately preceding a symbol declaration.
///
/// Walks the `leading_trivia` of `token` (the first significant token of
/// the symbol's AST node) in reverse, collecting contiguous comment lines
/// (`---`, `--`, `--[[ ... ]]`, EmmyLua/LuaCATS `---@...`). A blank line
/// (whitespace trivia containing 2+ consecutive newlines) terminates the
/// block so an unrelated header comment far above the symbol does not
/// leak in.
///
/// Returns `Some(RawDocument { symbol_fqdn, description })` when a block
/// was present, `None` otherwise. `description` keeps tags as raw text
/// (`@param x ...`, `@return ...`) — structured tag parsing comes via the
/// Annotation parser track of the beta.2 sequence.
pub(crate) fn capture_doc_for_symbol(
    symbol_fqdn: &str,
    token: &TokenReference,
) -> Option<RawDocument> {
    let description = collect_block_for(token.leading_trivia())?;
    Some(RawDocument {
        symbol_fqdn: symbol_fqdn.to_string(),
        description,
    })
}

/// Capture the file-level doc — the leading comment block of the first
/// significant token in the file. Same algorithm as
/// [`capture_doc_for_symbol`]; called by the extract pipeline with the
/// first stmt's leading trivia.
pub(crate) fn capture_module_doc(
    file_module_fqdn: &str,
    first_token: &TokenReference,
) -> Option<RawDocument> {
    capture_doc_for_symbol(file_module_fqdn, first_token)
}

fn collect_block_for<'a>(trivia: impl Iterator<Item = &'a Token>) -> Option<String> {
    // Walk forward, splitting comments into blocks separated by blank
    // lines. A blank line = a (possibly multi-token) whitespace gap that
    // contains 2+ newlines TOTAL between two comments. The block closest
    // to the symbol (i.e. the last one) is the doc.
    let mut blocks: Vec<Vec<&Token>> = vec![Vec::new()];
    let mut newlines_since_last_comment: usize = 0;

    for tok in trivia {
        match tok.token_type() {
            TokenType::SingleLineComment { .. } | TokenType::MultiLineComment { .. } => {
                if newlines_since_last_comment >= 2 && !blocks.last().is_some_and(Vec::is_empty) {
                    blocks.push(Vec::new());
                }
                if let Some(last) = blocks.last_mut() {
                    last.push(tok);
                }
                newlines_since_last_comment = 0;
            }
            TokenType::Whitespace { characters } => {
                let s: &str = characters;
                newlines_since_last_comment += s.chars().filter(|c| *c == '\n').count();
            }
            _ => {
                // Other trivia (shebang) — treat as a hard break to avoid
                // pulling in unrelated material above it.
                if !blocks.last().is_some_and(Vec::is_empty) {
                    blocks.push(Vec::new());
                }
                newlines_since_last_comment = 0;
            }
        }
    }

    let last_block = blocks.into_iter().next_back().unwrap_or_default();
    if last_block.is_empty() {
        return None;
    }

    let mut lines: Vec<String> = Vec::with_capacity(last_block.len());
    for tok in last_block {
        lines.push(strip_comment_markers(tok.token_type()));
    }
    let joined = lines.join("\n");
    let trimmed = joined.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn strip_comment_markers(ty: &TokenType) -> String {
    match ty {
        TokenType::SingleLineComment { comment } => {
            // `---foo` / `--foo` — full_moon's `comment` already excludes
            // the leading `--`, so a LuaDoc `---foo` arrives as `-foo`.
            // Strip a single optional leading `-` and one optional space.
            let s: &str = comment;
            let s = s.strip_prefix('-').unwrap_or(s);
            let s = s.strip_prefix(' ').unwrap_or(s);
            s.to_string()
        }
        TokenType::MultiLineComment { comment, .. } => {
            // `--[[ ... ]]` — `comment` excludes the brackets. Trim
            // surrounding whitespace and a single optional `!` shebang-
            // style marker.
            let s: &str = comment;
            let s = s.trim();
            let s = s.strip_prefix('!').unwrap_or(s);
            s.trim().to_string()
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first_stmt_token(content: &str) -> TokenReference {
        let ast = full_moon::parse(content).expect("parse");
        let first = ast.nodes().stmts().next().expect("at least one stmt");
        // Reach into the first stmt's first token. We use Display-equivalent
        // reconstruction via Node::tokens — but for tests we cheat by using
        // a known shape: LocalAssignment has local_token().
        match first {
            full_moon::ast::Stmt::LocalAssignment(la) => la.local_token().clone(),
            full_moon::ast::Stmt::LocalFunction(lf) => lf.local_token().clone(),
            full_moon::ast::Stmt::FunctionDeclaration(fd) => fd.function_token().clone(),
            other => panic!(
                "test fixture must use LocalAssignment / LocalFunction / FunctionDeclaration, got {other:?}"
            ),
        }
    }

    #[test]
    fn captures_single_luadoc_line() {
        let src = "--- the answer\nlocal x = 42\n";
        let token = first_stmt_token(src);
        let doc = capture_doc_for_symbol("pkg::file::x", &token).expect("doc");
        assert_eq!(doc.description, "the answer");
        assert_eq!(doc.symbol_fqdn, "pkg::file::x");
    }

    #[test]
    fn captures_contiguous_luadoc_block() {
        let src = "--- line one\n--- line two\nlocal x = 42\n";
        let token = first_stmt_token(src);
        let doc = capture_doc_for_symbol("pkg::file::x", &token).expect("doc");
        assert_eq!(doc.description, "line one\nline two");
    }

    #[test]
    fn captures_emmylua_tags_as_raw_text() {
        let src = "--- compute the sum\n---@param a number\n---@return number\nlocal function sum(a, b) return a + b end\n";
        let token = first_stmt_token(src);
        let doc = capture_doc_for_symbol("pkg::file::sum", &token).expect("doc");
        assert!(doc.description.contains("compute the sum"));
        assert!(doc.description.contains("@param a number"));
        assert!(doc.description.contains("@return number"));
    }

    #[test]
    fn captures_multiline_block_comment() {
        let src = "--[[ multi\nline ]]\nlocal x = 1\n";
        let token = first_stmt_token(src);
        let doc = capture_doc_for_symbol("pkg::file::x", &token).expect("doc");
        assert_eq!(doc.description, "multi\nline");
    }

    #[test]
    fn blank_line_breaks_block_and_drops_unrelated_header() {
        let src = "--- file header\n\n--- attached doc\nlocal x = 1\n";
        let token = first_stmt_token(src);
        let doc = capture_doc_for_symbol("pkg::file::x", &token).expect("doc");
        assert_eq!(doc.description, "attached doc");
    }

    #[test]
    fn no_comments_returns_none() {
        let src = "local x = 1\n";
        let token = first_stmt_token(src);
        assert!(capture_doc_for_symbol("pkg::file::x", &token).is_none());
    }

    #[test]
    fn empty_comment_returns_none() {
        let src = "--\nlocal x = 1\n";
        let token = first_stmt_token(src);
        assert!(capture_doc_for_symbol("pkg::file::x", &token).is_none());
    }
}

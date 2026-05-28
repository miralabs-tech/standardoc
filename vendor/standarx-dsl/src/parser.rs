use crate::ast::*;
use crate::diag::{Diag, Span, Spanned};
use crate::lexer::{token_kind_name, LexItem, Token};

pub fn parse_tokens(items: Vec<LexItem>, eof: Span) -> Result<File, Diag> {
    let mut p = Parser { items, pos: 0, eof };
    p.parse_file()
}

struct Parser {
    items: Vec<LexItem>,
    pos: usize,
    eof: Span,
}

impl Parser {
    fn peek_token(&self) -> Option<&Spanned<Token>> {
        for it in &self.items[self.pos..] {
            match it {
                LexItem::Token(t) => return Some(t),
                LexItem::Trivia { .. } => continue,
            }
        }
        None
    }

    fn bump_token(&mut self) -> Option<Spanned<Token>> {
        while self.pos < self.items.len() {
            match &self.items[self.pos] {
                LexItem::Trivia { .. } => self.pos += 1,
                LexItem::Token(_) => break,
            }
        }
        if self.pos < self.items.len() {
            let LexItem::Token(t) = self.items[self.pos].clone() else {
                unreachable!()
            };
            self.pos += 1;
            Some(t)
        } else {
            None
        }
    }

    fn take_leading_trivia(&mut self) -> Vec<(Trivia, bool)> {
        let mut out = Vec::new();
        while self.pos < self.items.len() {
            match &self.items[self.pos] {
                LexItem::Trivia {
                    trivia,
                    attached_to_prev,
                } => {
                    out.push((trivia.clone(), *attached_to_prev));
                    self.pos += 1;
                }
                LexItem::Token(_) => break,
            }
        }
        out
    }

    #[allow(dead_code)]
    fn current_span(&self) -> Span {
        self.peek_token()
            .map(|t| t.span.clone())
            .unwrap_or_else(|| self.eof.clone())
    }

    fn expect_kind(&mut self, want: &Token, label: &str) -> Result<Spanned<Token>, Diag> {
        match self.peek_token() {
            Some(t) if std::mem::discriminant(&t.node) == std::mem::discriminant(want) => {
                Ok(self.bump_token().unwrap())
            }
            Some(t) => Err(Diag::parse(
                t.span.clone(),
                format!("expected {}, found {}", label, token_kind_name(&t.node)),
            )),
            None => Err(Diag::parse(
                self.eof.clone(),
                format!("expected {}, found end of input", label),
            )),
        }
    }

    fn parse_file(&mut self) -> Result<File, Diag> {
        let mut stmts: Vec<StmtNode> = Vec::new();
        loop {
            let mut raw = self.take_leading_trivia();
            if self.peek_token().is_none() {
                attach_prev_trailing(&mut stmts, &mut raw);
                let trailing_trivia: Vec<Trivia> = raw.into_iter().map(|(t, _)| t).collect();
                return Ok(File {
                    stmts,
                    trailing_trivia,
                });
            }
            if !stmts.is_empty() {
                attach_prev_trailing(&mut stmts, &mut raw);
            }
            let leading: Vec<Trivia> = raw.into_iter().map(|(t, _)| t).collect();
            let (stmt, span) = self.parse_stmt_body()?;
            stmts.push(StmtNode {
                leading,
                trailing: None,
                node: stmt,
                span,
            });
        }
    }

    fn parse_stmt_body(&mut self) -> Result<(Stmt, Span), Diag> {
        let head = self.peek_token().cloned().ok_or_else(|| {
            Diag::parse(self.eof.clone(), "expected statement, found end of input")
        })?;
        let Token::Ident(name) = head.node else {
            return Err(Diag::parse(
                head.span,
                format!(
                    "expected identifier at start of statement, found {}",
                    token_kind_name(&self.peek_token().unwrap().node)
                ),
            ));
        };
        let head_span = head.span.clone();
        self.bump_token();
        let key = Spanned::new(Ident(name), head_span.clone());

        match self.peek_token().map(|t| &t.node) {
            Some(Token::LBrace) => self.parse_block_body(key, None),

            Some(Token::String(_)) => {
                // Lookahead: a string followed by `{` is a block label; otherwise
                // it's a string value (assignment).
                let str_tok = self.bump_token().unwrap();
                let str_span = str_tok.span.clone();
                let lit = match str_tok.node {
                    Token::String(s) => s,
                    _ => unreachable!(),
                };

                if matches!(self.peek_token().map(|t| &t.node), Some(Token::LBrace)) {
                    // Label
                    if lit.multiline {
                        return Err(Diag::parse(
                            str_span,
                            "block label cannot be a multi-line string",
                        ));
                    }
                    if lit.parts.iter().any(|p| matches!(p, StringPart::Interp(_))) {
                        return Err(Diag::parse(
                            str_span,
                            "block label cannot contain interpolation",
                        ));
                    }
                    let label_text = match lit.parts.into_iter().next() {
                        Some(StringPart::Lit(s)) => s,
                        None => String::new(),
                        _ => unreachable!(),
                    };
                    self.parse_block_body(key, Some(Spanned::new(label_text, str_span)))
                } else {
                    // Value
                    let value = Spanned::new(Expr::String(lit), str_span.clone());
                    let span = head_span.start..str_span.end;
                    Ok((Stmt::Assign(Assign { key, value }), span))
                }
            }

            Some(_) => {
                let value = self.parse_expr()?;
                let span = head_span.start..value.span.end;
                Ok((Stmt::Assign(Assign { key, value }), span))
            }

            None => Err(Diag::parse(
                self.eof.clone(),
                "expected value or '{' after identifier, found end of input",
            )),
        }
    }

    fn parse_block_body(
        &mut self,
        kind: Spanned<Ident>,
        label: Option<Spanned<String>>,
    ) -> Result<(Stmt, Span), Diag> {
        let lbrace = self.expect_kind(&Token::LBrace, "'{'")?;
        let start = kind.span.start;
        let mut stmts: Vec<StmtNode> = Vec::new();
        loop {
            let mut raw = self.take_leading_trivia();
            match self.peek_token().map(|t| &t.node) {
                Some(Token::RBrace) => {
                    attach_prev_trailing(&mut stmts, &mut raw);
                    let trailing_trivia: Vec<Trivia> = raw.into_iter().map(|(t, _)| t).collect();
                    let rbrace = self.expect_kind(&Token::RBrace, "'}'")?;
                    let span = start..rbrace.span.end;
                    return Ok((
                        Stmt::Block(Block {
                            kind,
                            label,
                            stmts,
                            trailing_trivia,
                        }),
                        span,
                    ));
                }
                None => {
                    return Err(Diag::parse(
                        lbrace.span,
                        "unterminated block body, expected '}'",
                    ));
                }
                _ => {
                    if !stmts.is_empty() {
                        attach_prev_trailing(&mut stmts, &mut raw);
                    }
                    let leading: Vec<Trivia> = raw.into_iter().map(|(t, _)| t).collect();
                    let (stmt, span) = self.parse_stmt_body()?;
                    stmts.push(StmtNode {
                        leading,
                        trailing: None,
                        node: stmt,
                        span,
                    });
                    // Optional comma between block stmts (tolerated for users
                    // coming from JSON-ish habits and for blocks that look like
                    // inline maps: `{ a 1, b 2 }`).
                    if matches!(self.peek_token().map(|t| &t.node), Some(Token::Comma)) {
                        self.bump_token();
                    }
                }
            }
        }
    }

    fn parse_expr(&mut self) -> Result<Spanned<Expr>, Diag> {
        let head = self.peek_token().cloned().ok_or_else(|| {
            Diag::parse(self.eof.clone(), "expected expression, found end of input")
        })?;
        match head.node {
            Token::Int(v) => {
                self.bump_token();
                Ok(Spanned::new(Expr::Int(v), head.span))
            }
            Token::Float(v) => {
                self.bump_token();
                Ok(Spanned::new(Expr::Float(v), head.span))
            }
            Token::True => {
                self.bump_token();
                Ok(Spanned::new(Expr::Bool(true), head.span))
            }
            Token::False => {
                self.bump_token();
                Ok(Spanned::new(Expr::Bool(false), head.span))
            }
            Token::Null => {
                self.bump_token();
                Ok(Spanned::new(Expr::Null, head.span))
            }
            Token::String(s) => {
                self.bump_token();
                Ok(Spanned::new(Expr::String(s), head.span))
            }
            Token::Ident(_) => self.parse_ref_expr(),
            Token::LBracket => self.parse_list_expr(),
            Token::LBrace => self.parse_map_expr(),
            other => Err(Diag::parse(
                head.span,
                format!("expected expression, found {}", token_kind_name(&other)),
            )),
        }
    }

    fn parse_ref_expr(&mut self) -> Result<Spanned<Expr>, Diag> {
        let head = self.bump_token().expect("caller checked ident");
        let Token::Ident(name) = head.node else {
            unreachable!()
        };
        let head_span = head.span.clone();
        let mut path = vec![Spanned::new(Ident(name), head_span.clone())];
        let mut end = head_span.end;
        while matches!(self.peek_token().map(|t| &t.node), Some(Token::Dot)) {
            self.bump_token();
            let seg = self.bump_token().ok_or_else(|| {
                Diag::parse(
                    self.eof.clone(),
                    "expected identifier or quoted segment after '.'",
                )
            })?;
            let seg_span = seg.span.clone();
            let seg_name = match seg.node {
                Token::Ident(s) => s,
                Token::String(lit) => {
                    if lit.multiline {
                        return Err(Diag::parse(
                            seg_span,
                            "ref segment cannot be a multi-line string".to_string(),
                        ));
                    }
                    if lit.parts.iter().any(|p| matches!(p, StringPart::Interp(_))) {
                        return Err(Diag::parse(
                            seg_span,
                            "ref segment cannot contain interpolation".to_string(),
                        ));
                    }
                    match lit.parts.into_iter().next() {
                        Some(StringPart::Lit(s)) => s,
                        None => String::new(),
                        _ => unreachable!(),
                    }
                }
                other => {
                    return Err(Diag::parse(
                        seg_span,
                        format!(
                            "expected identifier or quoted segment after '.', found {}",
                            token_kind_name(&other)
                        ),
                    ))
                }
            };
            end = seg_span.end;
            path.push(Spanned::new(Ident(seg_name), seg_span));
        }
        Ok(Spanned::new(Expr::Ref(Ref { path }), head_span.start..end))
    }

    fn parse_list_expr(&mut self) -> Result<Spanned<Expr>, Diag> {
        let lb = self.expect_kind(&Token::LBracket, "'['")?;
        let start = lb.span.start;
        let mut items = Vec::new();
        loop {
            match self.peek_token().map(|t| &t.node) {
                Some(Token::RBracket) => break,
                None => {
                    return Err(Diag::parse(
                        self.eof.clone(),
                        "unterminated list, expected ']'",
                    ))
                }
                _ => {}
            }
            items.push(self.parse_expr()?);
            if matches!(self.peek_token().map(|t| &t.node), Some(Token::Comma)) {
                self.bump_token();
            }
        }
        let rb = self.expect_kind(&Token::RBracket, "']'")?;
        Ok(Spanned::new(Expr::List(items), start..rb.span.end))
    }

    fn parse_map_expr(&mut self) -> Result<Spanned<Expr>, Diag> {
        let lb = self.expect_kind(&Token::LBrace, "'{'")?;
        let start = lb.span.start;
        let mut entries: Vec<MapEntry> = Vec::new();
        loop {
            match self.peek_token().map(|t| &t.node) {
                Some(Token::RBrace) => break,
                None => {
                    return Err(Diag::parse(
                        self.eof.clone(),
                        "unterminated map, expected '}'",
                    ))
                }
                _ => {}
            }
            let key_tok = self.bump_token().expect("peeked above");
            let key_span = key_tok.span.clone();
            let key_name = match key_tok.node {
                Token::Ident(s) => s,
                other => {
                    return Err(Diag::parse(
                        key_span,
                        format!(
                            "expected identifier as map key, found {}",
                            token_kind_name(&other)
                        ),
                    ))
                }
            };
            let key = Spanned::new(Ident(key_name), key_span);
            let value = self.parse_expr()?;
            entries.push(MapEntry { key, value });
            if matches!(self.peek_token().map(|t| &t.node), Some(Token::Comma)) {
                self.bump_token();
            }
        }
        let rb = self.expect_kind(&Token::RBrace, "'}'")?;
        Ok(Spanned::new(Expr::Map(entries), start..rb.span.end))
    }
}

fn attach_prev_trailing(stmts: &mut [StmtNode], raw: &mut Vec<(Trivia, bool)>) {
    let Some(last) = stmts.last_mut() else { return };
    if let Some((t, true)) = raw.first() {
        if matches!(t.kind, TriviaKind::LineComment(_)) {
            last.trailing = Some(t.clone());
            raw.remove(0);
        }
    }
}

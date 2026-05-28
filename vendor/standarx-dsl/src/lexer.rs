use crate::ast::{Ident, InterpExpr, Ref, StringLit, StringPart, Trivia, TriviaKind};
use crate::diag::{Diag, Spanned};

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Ident(String),
    Int(i64),
    Float(f64),
    True,
    False,
    Null,
    String(StringLit),
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Dot,
}

#[derive(Debug, Clone)]
pub enum LexItem {
    Token(Spanned<Token>),
    Trivia {
        trivia: Trivia,
        attached_to_prev: bool,
    },
}

pub fn tokenize(src: &str) -> Result<Vec<LexItem>, Diag> {
    let mut lx = Lexer::new(src);
    let mut out = Vec::new();
    loop {
        lx.consume_trivia(&mut out)?;
        if lx.eof() {
            break;
        }
        let tok = lx.next_token()?;
        out.push(LexItem::Token(tok));
    }
    Ok(out)
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src: src.as_bytes(),
            pos: 0,
        }
    }

    fn eof(&self) -> bool {
        self.pos >= self.src.len()
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn peek_at(&self, off: usize) -> Option<u8> {
        self.src.get(self.pos + off).copied()
    }

    fn starts_with(&self, s: &[u8]) -> bool {
        self.src.get(self.pos..self.pos + s.len()) == Some(s)
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.pos += 1;
        Some(b)
    }

    fn consume_trivia(&mut self, out: &mut Vec<LexItem>) -> Result<(), Diag> {
        let mut crossed_newline = false;
        loop {
            match self.peek() {
                Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n') => {
                    let mut newlines: usize = 0;
                    let ws_start = self.pos;
                    while let Some(b) = self.peek() {
                        match b {
                            b'\n' => {
                                newlines += 1;
                                self.pos += 1;
                            }
                            b' ' | b'\t' | b'\r' => self.pos += 1,
                            _ => break,
                        }
                    }
                    if newlines > 0 {
                        crossed_newline = true;
                    }
                    if newlines >= 2 {
                        out.push(LexItem::Trivia {
                            trivia: Trivia {
                                kind: TriviaKind::BlankLine,
                                span: ws_start..self.pos,
                            },
                            attached_to_prev: false,
                        });
                    }
                }
                Some(b'#') => {
                    let start = self.pos;
                    self.pos += 1;
                    let body_start = self.pos;
                    while let Some(b) = self.peek() {
                        if b == b'\n' {
                            break;
                        }
                        self.pos += 1;
                    }
                    let text = std::str::from_utf8(&self.src[body_start..self.pos])
                        .unwrap_or("")
                        .to_owned();
                    let attached_to_prev = !crossed_newline;
                    out.push(LexItem::Trivia {
                        trivia: Trivia {
                            kind: TriviaKind::LineComment(text),
                            span: start..self.pos,
                        },
                        attached_to_prev,
                    });
                    crossed_newline = true;
                }
                Some(b'/') if self.peek_at(1) == Some(b'/') => {
                    return Err(Diag::parse(
                        self.pos..self.pos + 2,
                        "'//' comments are not supported, use '#' instead",
                    ));
                }
                Some(b'/') if self.peek_at(1) == Some(b'*') => {
                    return Err(Diag::parse(
                        self.pos..self.pos + 2,
                        "'/* ... */' block comments are not supported, use '#' line comments",
                    ));
                }
                _ => return Ok(()),
            }
        }
    }

    fn next_token(&mut self) -> Result<Spanned<Token>, Diag> {
        let start = self.pos;
        let b = self.peek().expect("caller checked eof");
        match b {
            b'{' => {
                self.pos += 1;
                Ok(Spanned::new(Token::LBrace, start..self.pos))
            }
            b'}' => {
                self.pos += 1;
                Ok(Spanned::new(Token::RBrace, start..self.pos))
            }
            b'[' => {
                self.pos += 1;
                Ok(Spanned::new(Token::LBracket, start..self.pos))
            }
            b']' => {
                self.pos += 1;
                Ok(Spanned::new(Token::RBracket, start..self.pos))
            }
            b',' => {
                self.pos += 1;
                Ok(Spanned::new(Token::Comma, start..self.pos))
            }
            b'.' => {
                self.pos += 1;
                Ok(Spanned::new(Token::Dot, start..self.pos))
            }
            b'=' => Err(Diag::parse(
                start..start + 1,
                "'=' is not used in the standar DSL — write `key value` instead of `key = value`",
            )),
            b'"' => self.lex_plain_string_token(start),
            b'`' => self.lex_template_token(start),
            b'-' => self.lex_number(start),
            b'0'..=b'9' => self.lex_number(start),
            b if is_ident_start(b) => Ok(self.lex_ident(start)),
            other => Err(Diag::parse(
                start..start + 1,
                format!("unexpected character {:?}", other as char),
            )),
        }
    }

    fn lex_ident(&mut self, start: usize) -> Spanned<Token> {
        while let Some(b) = self.peek() {
            if is_ident_continue(b) {
                self.pos += 1;
            } else {
                break;
            }
        }
        let text = std::str::from_utf8(&self.src[start..self.pos])
            .expect("ident bytes are ASCII")
            .to_owned();
        let tok = match text.as_str() {
            "true" => Token::True,
            "false" => Token::False,
            "null" => Token::Null,
            _ => Token::Ident(text),
        };
        Spanned::new(tok, start..self.pos)
    }

    fn lex_number(&mut self, start: usize) -> Result<Spanned<Token>, Diag> {
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        let int_start = self.pos;
        while let Some(b) = self.peek() {
            if b.is_ascii_digit() {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == int_start {
            return Err(Diag::parse(start..self.pos, "expected digit after '-'"));
        }
        let mut is_float = false;
        if self.peek() == Some(b'.') {
            if let Some(next) = self.peek_at(1) {
                if next.is_ascii_digit() {
                    is_float = true;
                    self.pos += 1;
                    while let Some(b) = self.peek() {
                        if b.is_ascii_digit() {
                            self.pos += 1;
                        } else {
                            break;
                        }
                    }
                }
            }
        }
        let text = std::str::from_utf8(&self.src[start..self.pos]).expect("number bytes are ASCII");
        let span = start..self.pos;
        if is_float {
            let v: f64 = text
                .parse()
                .map_err(|_| Diag::parse(span.clone(), "invalid float"))?;
            Ok(Spanned::new(Token::Float(v), span))
        } else {
            let v: i64 = text
                .parse()
                .map_err(|_| Diag::parse(span.clone(), "invalid int"))?;
            Ok(Spanned::new(Token::Int(v), span))
        }
    }

    /// `"..."` — plain string, no interpolation, single-line.
    /// `${...}` inside is literal; `\$` is no longer needed and not recognised.
    fn lex_plain_string_token(&mut self, start: usize) -> Result<Spanned<Token>, Diag> {
        if self.starts_with(b"\"\"\"") {
            return Err(Diag::parse(
                start..start + 3,
                "triple-quote multi-line strings are not supported, use ``` ... ``` for multi-line templates",
            ));
        }
        debug_assert_eq!(self.peek(), Some(b'"'));
        self.pos += 1;
        let mut buf = String::new();
        loop {
            match self.peek() {
                None => return Err(Diag::parse(start..self.pos, "unterminated string literal")),
                Some(b'"') => {
                    self.pos += 1;
                    let parts = if buf.is_empty() {
                        Vec::new()
                    } else {
                        vec![StringPart::Lit(buf)]
                    };
                    let lit = StringLit {
                        parts,
                        multiline: false,
                        template: false,
                    };
                    return Ok(Spanned::new(Token::String(lit), start..self.pos));
                }
                Some(b'\n') => {
                    return Err(Diag::parse(
                        self.pos..self.pos + 1,
                        "unterminated string literal (newline in basic string — use ``` ... ``` for multi-line)",
                    ));
                }
                Some(b'\\') => {
                    self.read_plain_escape(&mut buf)?;
                }
                Some(_) => {
                    let b = self.bump().unwrap();
                    push_byte(&mut buf, b);
                }
            }
        }
    }

    /// `` `...` `` (single-line) or `` ```...``` `` (multi-line) — template
    /// string with `${...}` interpolation.
    fn lex_template_token(&mut self, start: usize) -> Result<Spanned<Token>, Diag> {
        if self.starts_with(b"```") {
            self.lex_multiline_template(start)
        } else {
            self.lex_inline_template(start)
        }
    }

    fn lex_inline_template(&mut self, start: usize) -> Result<Spanned<Token>, Diag> {
        debug_assert_eq!(self.peek(), Some(b'`'));
        self.pos += 1;
        let mut parts: Vec<StringPart> = Vec::new();
        let mut buf = String::new();
        loop {
            match self.peek() {
                None => {
                    return Err(Diag::parse(
                        start..self.pos,
                        "unterminated template literal",
                    ))
                }
                Some(b'`') => {
                    self.pos += 1;
                    if !buf.is_empty() {
                        parts.push(StringPart::Lit(buf));
                    }
                    let lit = StringLit {
                        parts,
                        multiline: false,
                        template: true,
                    };
                    return Ok(Spanned::new(Token::String(lit), start..self.pos));
                }
                Some(b'\n') => {
                    return Err(Diag::parse(
                        self.pos..self.pos + 1,
                        "unterminated template (newline in inline template — use ``` ... ``` for multi-line)",
                    ));
                }
                Some(b'\\') => {
                    self.read_template_escape(&mut buf)?;
                }
                Some(b'$') if self.peek_at(1) == Some(b'{') => {
                    if !buf.is_empty() {
                        parts.push(StringPart::Lit(std::mem::take(&mut buf)));
                    }
                    let interp = self.lex_interp()?;
                    parts.push(StringPart::Interp(interp));
                }
                Some(_) => {
                    let b = self.bump().unwrap();
                    push_byte(&mut buf, b);
                }
            }
        }
    }

    fn lex_multiline_template(&mut self, start: usize) -> Result<Spanned<Token>, Diag> {
        debug_assert!(self.starts_with(b"```"));
        self.pos += 3;
        let mut parts: Vec<StringPart> = Vec::new();
        let mut buf = String::new();
        loop {
            if self.starts_with(b"```") {
                self.pos += 3;
                if !buf.is_empty() {
                    parts.push(StringPart::Lit(buf));
                }
                let lit = StringLit {
                    parts,
                    multiline: true,
                    template: true,
                };
                return Ok(Spanned::new(Token::String(lit), start..self.pos));
            }
            match self.peek() {
                None => {
                    return Err(Diag::parse(
                        start..self.pos,
                        "unterminated multi-line template",
                    ))
                }
                Some(b'\\') => self.read_template_escape(&mut buf)?,
                Some(b'$') if self.peek_at(1) == Some(b'{') => {
                    if !buf.is_empty() {
                        parts.push(StringPart::Lit(std::mem::take(&mut buf)));
                    }
                    let interp = self.lex_interp()?;
                    parts.push(StringPart::Interp(interp));
                }
                Some(_) => {
                    let b = self.bump().unwrap();
                    push_byte(&mut buf, b);
                }
            }
        }
    }

    /// Escapes for plain `"..."` — `\"`, `\\`, `\n`, `\t`, `\r`, `\u{...}`.
    fn read_plain_escape(&mut self, buf: &mut String) -> Result<(), Diag> {
        let start = self.pos;
        debug_assert_eq!(self.peek(), Some(b'\\'));
        self.pos += 1;
        let e = self
            .bump()
            .ok_or_else(|| Diag::parse(start..self.pos, "dangling backslash"))?;
        match e {
            b'"' => buf.push('"'),
            b'\\' => buf.push('\\'),
            b'n' => buf.push('\n'),
            b't' => buf.push('\t'),
            b'r' => buf.push('\r'),
            b'u' => self.read_unicode_escape(buf, start)?,
            other => {
                return Err(Diag::parse(
                    start..self.pos,
                    format!("unknown escape '\\{}'", other as char),
                ));
            }
        }
        Ok(())
    }

    /// Escapes for templates `` `...` `` — adds `\`` (backtick) and `\$`.
    fn read_template_escape(&mut self, buf: &mut String) -> Result<(), Diag> {
        let start = self.pos;
        debug_assert_eq!(self.peek(), Some(b'\\'));
        self.pos += 1;
        let e = self
            .bump()
            .ok_or_else(|| Diag::parse(start..self.pos, "dangling backslash"))?;
        match e {
            b'`' => buf.push('`'),
            b'\\' => buf.push('\\'),
            b'$' => buf.push('$'),
            b'n' => buf.push('\n'),
            b't' => buf.push('\t'),
            b'r' => buf.push('\r'),
            b'u' => self.read_unicode_escape(buf, start)?,
            other => {
                return Err(Diag::parse(
                    start..self.pos,
                    format!("unknown escape '\\{}'", other as char),
                ));
            }
        }
        Ok(())
    }

    fn read_unicode_escape(&mut self, buf: &mut String, start: usize) -> Result<(), Diag> {
        if self.bump() != Some(b'{') {
            return Err(Diag::parse(start..self.pos, "expected '{' after \\u"));
        }
        let hex_start = self.pos;
        while let Some(b) = self.peek() {
            if b.is_ascii_hexdigit() {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == hex_start || (self.pos - hex_start) > 6 {
            return Err(Diag::parse(start..self.pos, "invalid \\u{...} hex digits"));
        }
        let hex = std::str::from_utf8(&self.src[hex_start..self.pos]).expect("hex bytes are ASCII");
        let code = u32::from_str_radix(hex, 16)
            .map_err(|_| Diag::parse(start..self.pos, "invalid hex"))?;
        let ch = char::from_u32(code)
            .ok_or_else(|| Diag::parse(start..self.pos, "invalid unicode codepoint"))?;
        if self.bump() != Some(b'}') {
            return Err(Diag::parse(
                start..self.pos,
                "expected '}' to close \\u{...}",
            ));
        }
        buf.push(ch);
        Ok(())
    }

    fn lex_interp(&mut self) -> Result<Spanned<InterpExpr>, Diag> {
        let start = self.pos;
        debug_assert_eq!(self.peek(), Some(b'$'));
        self.pos += 2;
        self.skip_inline_ws();
        let body_start = self.pos;
        let body = self.lex_interp_body(body_start)?;
        self.skip_inline_ws();
        if self.peek() != Some(b'}') {
            return Err(Diag::parse(
                start..self.pos,
                "expected '}' to close interpolation",
            ));
        }
        self.pos += 1;
        Ok(Spanned::new(body, start..self.pos))
    }

    fn skip_inline_ws(&mut self) {
        while matches!(self.peek(), Some(b' ') | Some(b'\t')) {
            self.pos += 1;
        }
    }

    fn lex_interp_body(&mut self, start: usize) -> Result<InterpExpr, Diag> {
        match self.peek() {
            Some(b'"') => {
                let saved = self.pos;
                let tok = self.lex_plain_string_token(saved)?;
                let lit = match tok.node {
                    Token::String(s) => s,
                    _ => unreachable!(),
                };
                let s = match lit.parts.as_slice() {
                    [] => String::new(),
                    [StringPart::Lit(s)] => s.clone(),
                    _ => {
                        return Err(Diag::parse(
                            start..self.pos,
                            "nested interpolation inside ${...} is not allowed",
                        ))
                    }
                };
                Ok(InterpExpr::String(s))
            }
            Some(b'-') | Some(b'0'..=b'9') => {
                let tok = self.lex_number(start)?;
                match tok.node {
                    Token::Int(v) => Ok(InterpExpr::Int(v)),
                    Token::Float(v) => Ok(InterpExpr::Float(v)),
                    _ => unreachable!("lex_number returns Int or Float"),
                }
            }
            Some(b) if is_ident_start(b) => {
                let mut path = Vec::new();
                let head = self.lex_ident(start);
                let head_span = head.span.clone();
                let head_name = match head.node {
                    Token::Ident(s) => s,
                    Token::True => return Ok(InterpExpr::Bool(true)),
                    Token::False => return Ok(InterpExpr::Bool(false)),
                    Token::Null => return Ok(InterpExpr::Null),
                    _ => unreachable!(),
                };
                path.push(Spanned::new(Ident(head_name), head_span));
                loop {
                    self.skip_inline_ws();
                    if self.peek() != Some(b'.') {
                        break;
                    }
                    self.pos += 1;
                    self.skip_inline_ws();
                    let seg_start = self.pos;
                    let Some(b) = self.peek() else {
                        return Err(Diag::parse(
                            seg_start..self.pos,
                            "expected identifier or quoted segment after '.'",
                        ));
                    };
                    if b == b'"' {
                        let tok = self.lex_plain_string_token(seg_start)?;
                        let lit = match tok.node {
                            Token::String(s) => s,
                            _ => unreachable!(),
                        };
                        let seg_span = seg_start..self.pos;
                        if lit.parts.iter().any(|p| matches!(p, StringPart::Interp(_))) {
                            return Err(Diag::parse(
                                seg_span,
                                "ref segment cannot contain interpolation".to_string(),
                            ));
                        }
                        let text = match lit.parts.into_iter().next() {
                            Some(StringPart::Lit(s)) => s,
                            None => String::new(),
                            _ => unreachable!(),
                        };
                        path.push(Spanned::new(Ident(text), seg_span));
                        continue;
                    }
                    if !is_ident_start(b) {
                        return Err(Diag::parse(
                            seg_start..self.pos + 1,
                            "expected identifier or quoted segment after '.'",
                        ));
                    }
                    let seg = self.lex_ident(seg_start);
                    let seg_span = seg.span.clone();
                    let seg_name = match seg.node {
                        Token::Ident(s) => s,
                        Token::True | Token::False | Token::Null => {
                            return Err(Diag::parse(
                                seg_span,
                                "reserved word cannot appear in reference path",
                            ));
                        }
                        _ => unreachable!(),
                    };
                    path.push(Spanned::new(Ident(seg_name), seg_span));
                }
                Ok(InterpExpr::Ref(Ref { path }))
            }
            _ => Err(Diag::parse(
                start..self.pos + 1,
                "expected ref, number, bool, null, or string inside ${...}",
            )),
        }
    }
}

fn is_ident_start(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphabetic()
}

fn is_ident_continue(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphanumeric()
}

fn push_byte(buf: &mut String, b: u8) {
    if b.is_ascii() {
        buf.push(b as char);
    } else {
        let bytes = unsafe { buf.as_mut_vec() };
        bytes.push(b);
    }
}

pub fn token_kind_name(t: &Token) -> &'static str {
    match t {
        Token::Ident(_) => "identifier",
        Token::Int(_) => "int",
        Token::Float(_) => "float",
        Token::True => "true",
        Token::False => "false",
        Token::Null => "null",
        Token::String(_) => "string",
        Token::LBrace => "'{'",
        Token::RBrace => "'}'",
        Token::LBracket => "'['",
        Token::RBracket => "']'",
        Token::Comma => "','",
        Token::Dot => "'.'",
    }
}

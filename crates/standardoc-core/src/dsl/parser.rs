//! Recursive-descent parser for the Standardoc DSL.
//!
//! Grammar summary (see `standardoc-core::dsl` module docs for the full spec):
//!
//! ```text
//! template    = *( text / expr )
//! expr        = "{{" ws ( keyword / reference / alias_ref ) ws "}}"
//! keyword     = "each" <alias> "in" <ref>
//!             / "if" <condition>
//!             / "else"    ; closes a then-body and opens else-body
//!             / "/each"   ; closes an `each` block
//!             / "/if"     ; closes an `if` block (with or without else)
//! reference   = "@doc." KEY [ ":" access ]
//! alias_ref   = IDENT ( "." IDENT )*    ; only valid inside an `each` body
//! access      = "label" / "key" / "origin"
//!             / "meta" ( "." IDENT )*
//!             / "symbol" ( "." IDENT )*
//!             / TAG [ "[" INT "]" ] [ "." FIELD ]
//!             / FUNC "(" IDENT ")"       ; has|count|first|last
//! condition   = reference [ compare_op literal ]
//! ```
//!
//! Closing tags are **typed** (`/each`, `/if`) rather than a generic `end`.
//! Trade-off: slightly more typing, but zero ambiguity in nested blocks, and
//! "expected /each, found /if" points directly to the bug. LLMs also produce
//! more reliable templates with explicit closing tags.
//!
//! Key rule: `.` is only part of the KEY path; everything after `:` is either
//! a reserved field (`label`/`key`/`origin`/`meta.*`/`symbol.*`) or a tag name.
//! This keeps parsing deterministic without needing the index.

use crate::dsl::ast::{
    Access, AliasAccess, AliasRef, BlockQuery, CompareOp, CondTarget, Condition, EachSource,
    FuncName, Literal, Node, Reference, Template,
};
use crate::model::DocKey;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("empty expression")]
    Empty,

    #[error("invalid reference: {0}")]
    InvalidRef(String),

    #[error("unknown function '{0}' — expected has / count / first / last")]
    UnknownFunc(String),

    #[error("expected '{expected}', found '{found}'")]
    Expected { expected: String, found: String },

    #[error("unexpected trailing input: '{0}'")]
    Trailing(String),

    #[error("unterminated block: expected '/{0}'")]
    MissingClosing(&'static str),

    #[error("mismatched closing tag: expected '/{expected}', found '/{found}'")]
    MismatchedClosing {
        expected: &'static str,
        found: String,
    },

    #[error("unterminated string literal")]
    UnterminatedString,

    #[error("{0}")]
    Other(String),
}

pub fn parse(input: &str) -> Result<Template, ParseError> {
    let mut raws = tokenize(input);
    apply_block_whitespace_trim(&mut raws);
    let mut cursor = 0;
    let nodes = parse_nodes(&raws, &mut cursor, None)?;
    if cursor != raws.len() {
        // Orphan terminator (`/each`, `/if`, `else`) at top level.
        let stray = match &raws[cursor] {
            Raw::Expr(s) => s.trim().to_owned(),
            Raw::Text(_) => String::from("<text>"),
        };
        return Err(ParseError::Other(format!(
            "unexpected closing tag at top level: '{stray}'"
        )));
    }
    Ok(Template { nodes })
}

// -------- Tokenization --------

#[derive(Debug, Clone)]
enum Raw {
    Text(String),
    Expr(String),
}

fn tokenize(input: &str) -> Vec<Raw> {
    let mut out = Vec::new();
    let mut buf = String::new();
    // CommonMark fenced code-block awareness: by default, any `{{ … }}` inside
    // a ``` (or ~~~) block is treated as a documentation example and passed
    // through as text — otherwise dogfooding the DSL's own README (which shows
    // template snippets in fences) would explode trying to resolve them.
    //
    // Opt-in evaluation: a fence whose info-string is `dsl` (case-insensitive)
    // is tokenized like regular text — the directives inside ARE evaluated.
    // This lets templates inject live data formatted as a code block.
    //
    // We track the opening marker so the closer must match in length and char
    // (CommonMark rule 119): ```` opens with `code` body until ```` closes.
    let mut fence: Option<Fence> = None;

    for line in input.split_inclusive('\n') {
        if let Some(f) = &fence {
            // Inside a fenced block. Detect whether this line closes the fence
            // (same marker, alone on the line modulo leading whitespace and
            // trailing newline). The fence-line itself is always text — DSL
            // never runs on the marker line.
            let stripped = line.trim_start_matches([' ', '\t']);
            let stripped_no_nl = stripped.trim_end_matches('\n').trim_end();
            let closes = stripped_no_nl == f.closer.as_str();
            if closes {
                buf.push_str(line);
                fence = None;
            } else if f.eval {
                tokenize_line(line, &mut out, &mut buf);
            } else {
                buf.push_str(line);
            }
            continue;
        }

        // Not in fence — but maybe this line opens one.
        let stripped = line.trim_start_matches([' ', '\t']);
        if stripped.starts_with("```") || stripped.starts_with("~~~") {
            let first = stripped.chars().next().expect("starts_with checked above");
            let marker_len = stripped.chars().take_while(|&c| c == first).count();
            if marker_len >= 3 {
                let info_string = stripped[marker_len..].trim();
                fence = Some(Fence {
                    closer: std::iter::repeat_n(first, marker_len).collect(),
                    eval: info_string.eq_ignore_ascii_case("dsl"),
                });
                buf.push_str(line);
                continue;
            }
        }

        // Regular line — scan for `{{ … }}` expressions.
        tokenize_line(line, &mut out, &mut buf);
    }

    if !buf.is_empty() {
        out.push(Raw::Text(buf));
    }
    out
}

struct Fence {
    closer: String,
    eval: bool,
}

/// Scan a single line for `{{ … }}` directives. Pre-condition: the line is
/// outside any fenced code block. Multi-line expressions are not supported
/// (an unterminated `{{` is left as literal text). Inline backtick code
/// spans (`` `…` ``) are also passed through verbatim — without this,
/// any README that documents the DSL syntax inline (e.g.
/// `` `{{ each X in @doc.KEY:tag }}` ``) would fail to render against a
/// real index.
fn tokenize_line(line: &str, out: &mut Vec<Raw>, buf: &mut String) {
    let mut chars = line.chars().peekable();
    // `in_code` flips on every backtick. Single-backtick inline code only —
    // doubled-backtick spans (`` `` … `` ``) are rare enough in standardoc-
    // facing docs to ignore for now.
    let mut in_code = false;
    while let Some(&c) = chars.peek() {
        if c == '`' {
            in_code = !in_code;
            buf.push(chars.next().unwrap());
            continue;
        }
        if !in_code && c == '{' {
            let mut probe = chars.clone();
            probe.next();
            if probe.peek() == Some(&'{') {
                chars.next();
                chars.next();
                if !buf.is_empty() {
                    out.push(Raw::Text(std::mem::take(buf)));
                }
                let mut expr = String::new();
                let mut terminated = false;
                while let Some(ch) = chars.next() {
                    if ch == '}' && chars.peek() == Some(&'}') {
                        chars.next();
                        terminated = true;
                        break;
                    }
                    expr.push(ch);
                }
                if terminated {
                    out.push(Raw::Expr(expr));
                } else {
                    buf.push_str("{{");
                    buf.push_str(&expr);
                }
                continue;
            }
        }
        buf.push(chars.next().unwrap());
    }
}

// -------- Whitespace control around block directives --------
//
// Rule: when a block directive (`each X`, `if X`, `else`, `end`) is alone on
// its line — i.e. only whitespace precedes it on the line and only whitespace
// follows it before the next newline — the directive consumes the entire line.
// References like `{{ @doc.foo:label }}` are NEVER stripped; only control-flow
// keywords. This mirrors Jinja2's `trim_blocks + lstrip_blocks` defaults and
// keeps rendered markdown free of phantom blank lines.

fn apply_block_whitespace_trim(raws: &mut [Raw]) {
    let directives: Vec<bool> = raws
        .iter()
        .map(|r| matches!(r, Raw::Expr(s) if is_block_directive(s)))
        .collect();
    let eligible: Vec<bool> = (0..raws.len())
        .map(|i| directives[i] && has_line_prefix(raws, i) && has_line_suffix(raws, i))
        .collect();

    for i in 0..raws.len() {
        if !eligible[i] {
            continue;
        }
        if i > 0 {
            if let Raw::Text(t) = &mut raws[i - 1] {
                match t.rfind('\n') {
                    Some(pos) => t.truncate(pos + 1),
                    None => t.clear(),
                }
            }
        }
        if let Some(Raw::Text(t)) = raws.get_mut(i + 1) {
            let cut = t
                .bytes()
                .position(|b| b == b'\n')
                .map_or(t.len(), |p| p + 1);
            t.replace_range(..cut, "");
        }
    }
}

fn is_block_directive(expr: &str) -> bool {
    let trimmed = expr.trim();
    trimmed == "/each"
        || trimmed == "/if"
        || trimmed == "else"
        || trimmed == "else if"
        || trimmed.starts_with("else if ")
        || trimmed.starts_with("each ")
        || trimmed == "each"
        || trimmed.starts_with("if ")
        || trimmed == "if"
}

fn has_line_prefix(raws: &[Raw], i: usize) -> bool {
    if i == 0 {
        return true;
    }
    match &raws[i - 1] {
        Raw::Text(t) => t.rfind('\n').map_or_else(
            || t.chars().all(char::is_whitespace),
            |pos| t[pos + 1..].chars().all(char::is_whitespace),
        ),
        Raw::Expr(_) => false,
    }
}

fn has_line_suffix(raws: &[Raw], i: usize) -> bool {
    match raws.get(i + 1) {
        None => true,
        Some(Raw::Text(t)) => t.find('\n').map_or_else(
            || t.chars().all(|c| c == ' ' || c == '\t'),
            |pos| t[..pos].chars().all(|c| c == ' ' || c == '\t'),
        ),
        Some(Raw::Expr(_)) => false,
    }
}

// -------- Block-level parsing --------

/// Which delimiter closes/separates the current block? Parser calls
/// `parse_nodes(.., Some(BlockKind))` to know what it expects as output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Each,
    If,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Terminator {
    /// `{{ /each }}`
    EndEach,
    /// `{{ /if }}`
    EndIf,
    /// `{{ else }}` — only valid inside an `if`.
    Else,
    /// `{{ else if <cond> }}` — flat `if/else` chain without visual nesting.
    /// Condition is re-parsed from expression when attaching branch.
    ElseIf,
}

fn classify_terminator(expr: &str) -> Option<Terminator> {
    let trimmed = expr.trim();
    if trimmed == "/each" {
        return Some(Terminator::EndEach);
    }
    if trimmed == "/if" {
        return Some(Terminator::EndIf);
    }
    if trimmed == "else" {
        return Some(Terminator::Else);
    }
    // `else if X` — space after `else` is required to distinguish from a
    // possible `elseif` alias (reserved for later).
    if trimmed == "else if" || trimmed.starts_with("else if ") {
        return Some(Terminator::ElseIf);
    }
    None
}

fn parse_nodes(
    raws: &[Raw],
    cursor: &mut usize,
    enclosing: Option<BlockKind>,
) -> Result<Vec<Node>, ParseError> {
    let mut nodes = Vec::new();
    while *cursor < raws.len() {
        if enclosing.is_some() {
            if let Raw::Expr(s) = &raws[*cursor] {
                if classify_terminator(s).is_some() {
                    return Ok(nodes);
                }
            }
        }
        let raw = &raws[*cursor];
        *cursor += 1;
        match raw {
            Raw::Text(s) => {
                if !s.is_empty() {
                    nodes.push(Node::Text(s.clone()));
                }
            }
            Raw::Expr(expr_src) => {
                let parsed = parse_expression(expr_src)?;
                match parsed {
                    ParsedExpr::Each { alias, collection } => {
                        let body = parse_nodes(raws, cursor, Some(BlockKind::Each))?;
                        expect_close(raws, cursor, BlockKind::Each)?;
                        nodes.push(Node::Each {
                            alias,
                            collection,
                            body,
                        });
                    }
                    ParsedExpr::If { condition } => {
                        nodes.push(parse_if_chain(raws, cursor, condition)?);
                    }
                    ParsedExpr::Node(n) => nodes.push(n),
                }
            }
        }
    }
    if let Some(kind) = enclosing {
        return Err(ParseError::MissingClosing(close_keyword(kind)));
    }
    Ok(nodes)
}

const fn close_keyword(kind: BlockKind) -> &'static str {
    match kind {
        BlockKind::Each => "each",
        BlockKind::If => "if",
    }
}

/// Parse an `if … [else if …]* [else …] /if` chain in flat form, then fold
/// right to produce a potentially nested `Node::If`.
///
/// Flat-then-fold approach is easier to reason about and debug than direct
/// recursion: collection treats branches at same level, fold reconstructs the
/// nested structure in one pass.
fn parse_if_chain(
    raws: &[Raw],
    cursor: &mut usize,
    first_condition: Condition,
) -> Result<Node, ParseError> {
    let mut branches: Vec<(Condition, Vec<Node>)> = Vec::new();
    let mut else_body: Option<Vec<Node>> = None;

    let then_body = parse_nodes(raws, cursor, Some(BlockKind::If))?;
    branches.push((first_condition, then_body));

    loop {
        match peek_terminator(raws, *cursor) {
            Some(Terminator::ElseIf) => {
                let cond = consume_else_if_condition(raws, cursor)?;
                let body = parse_nodes(raws, cursor, Some(BlockKind::If))?;
                branches.push((cond, body));
            }
            Some(Terminator::Else) => {
                *cursor += 1;
                let body = parse_nodes(raws, cursor, Some(BlockKind::If))?;
                else_body = Some(body);
                break;
            }
            _ => break,
        }
    }
    expect_close(raws, cursor, BlockKind::If)?;

    // Right fold: the last `if/else if` wraps `else`, and each previous
    // branch wraps the next as its else_body.
    let mut node_else: Option<Vec<Node>> = else_body;
    while let Some((cond, body)) = branches.pop() {
        let new_node = Node::If {
            condition: cond,
            then_body: body,
            else_body: node_else.take(),
        };
        node_else = Some(vec![new_node]);
    }
    // node_else contient maintenant exactement un Node::If — l'extraire.
    Ok(node_else
        .expect("at least one branch")
        .pop()
        .expect("single node"))
}

/// Consume `{{ else if <cond> }}` and return parsed `Condition`.
fn consume_else_if_condition(raws: &[Raw], cursor: &mut usize) -> Result<Condition, ParseError> {
    let Some(Raw::Expr(s)) = raws.get(*cursor) else {
        return Err(ParseError::Other("expected 'else if' expression".into()));
    };
    let trimmed = s.trim();
    let rest = trimmed
        .strip_prefix("else if")
        .ok_or_else(|| ParseError::Other(format!("expected 'else if', got '{trimmed}'")))?
        .trim_start();
    let mut lex = Lex::new(rest);
    let cond = parse_condition(&mut lex)?;
    lex.assert_end()?;
    *cursor += 1;
    Ok(cond)
}

fn peek_terminator(raws: &[Raw], cursor: usize) -> Option<Terminator> {
    match raws.get(cursor) {
        Some(Raw::Expr(s)) => classify_terminator(s),
        _ => None,
    }
}

fn expect_close(raws: &[Raw], cursor: &mut usize, kind: BlockKind) -> Result<(), ParseError> {
    let expected = match kind {
        BlockKind::Each => Terminator::EndEach,
        BlockKind::If => Terminator::EndIf,
    };
    let expected_kw = close_keyword(kind);
    match raws.get(*cursor) {
        Some(Raw::Expr(s)) => match classify_terminator(s) {
            Some(t) if t == expected => {
                *cursor += 1;
                Ok(())
            }
            Some(_) => Err(ParseError::MismatchedClosing {
                expected: expected_kw,
                found: s.trim().trim_start_matches('/').to_owned(),
            }),
            None => Err(ParseError::Expected {
                expected: format!("/{expected_kw}"),
                found: s.trim().to_owned(),
            }),
        },
        _ => Err(ParseError::MissingClosing(expected_kw)),
    }
}

// -------- Expression-level parsing --------

#[derive(Debug)]
enum ParsedExpr {
    Each {
        alias: String,
        collection: EachSource,
    },
    If {
        condition: Condition,
    },
    Node(Node),
}

fn parse_expression(src: &str) -> Result<ParsedExpr, ParseError> {
    let trimmed = src.trim();
    if trimmed.is_empty() {
        return Err(ParseError::Empty);
    }
    let mut lex = Lex::new(trimmed);

    if lex.eat_keyword("each") {
        let alias = lex
            .take_ident()
            .ok_or_else(|| ParseError::Other("expected alias after 'each'".into()))?;
        if !lex.eat_keyword("in") {
            return Err(ParseError::Expected {
                expected: "in".into(),
                found: lex.rest().trim().to_owned(),
            });
        }
        let collection = parse_each_source(&mut lex)?;
        lex.assert_end()?;
        return Ok(ParsedExpr::Each { alias, collection });
    }

    if lex.eat_keyword("if") {
        let condition = parse_condition(&mut lex)?;
        lex.assert_end()?;
        return Ok(ParsedExpr::If { condition });
    }

    if lex.peek() == Some('@') {
        let reference = parse_reference(&mut lex)?;
        lex.assert_end()?;
        return Ok(ParsedExpr::Node(Node::Reference(reference)));
    }

    if let Some(alias) = lex.take_ident() {
        let aref = parse_alias_tail(alias, &mut lex)?;
        lex.assert_end()?;
        return Ok(ParsedExpr::Node(Node::Alias(aref)));
    }

    Err(ParseError::Other(format!(
        "cannot parse expression: '{trimmed}'"
    )))
}

/// Parse source of an `each`:
/// - `@doc.K[:tag]` → `EachSource::Tag(reference)`
/// - `@docs.module(K)` → `EachSource::Blocks(Module(K))`
/// - `@docs.all` → `EachSource::Blocks(All)`
fn parse_each_source(lex: &mut Lex<'_>) -> Result<EachSource, ParseError> {
    lex.skip_ws();
    if lex.starts_with("@docs.") {
        return parse_block_query(lex).map(EachSource::Blocks);
    }
    let reference = parse_reference(lex)?;
    Ok(EachSource::Tag(reference))
}

fn parse_block_query(lex: &mut Lex<'_>) -> Result<BlockQuery, ParseError> {
    if !lex.eat("@docs.") {
        return Err(ParseError::InvalidRef(format!(
            "expected '@docs.' at '{}'",
            lex.rest()
        )));
    }
    let kind = lex
        .take_ident()
        .ok_or_else(|| ParseError::InvalidRef("expected query kind after '@docs.'".into()))?;
    match kind.as_str() {
        "all" => Ok(BlockQuery::All),
        "module" => {
            let key = parse_paren_key(lex, "module")?;
            Ok(BlockQuery::Module(DocKey::new(key)))
        }
        "satellites" => {
            let key = parse_paren_key(lex, "satellites")?;
            Ok(BlockQuery::Satellites(DocKey::new(key)))
        }
        other => Err(ParseError::InvalidRef(format!(
            "unknown @docs query '{other}' — expected 'module(K)', 'satellites(K)', or 'all'"
        ))),
    }
}

fn parse_paren_key(lex: &mut Lex<'_>, kind: &'static str) -> Result<String, ParseError> {
    if lex.peek() != Some('(') {
        return Err(ParseError::Expected {
            expected: "(".into(),
            found: lex.rest().trim().to_owned(),
        });
    }
    lex.advance();
    lex.skip_ws();
    let key = lex.take_dotted_ident().ok_or_else(|| {
        ParseError::InvalidRef(format!("expected key inside '{kind}(...)'"))
    })?;
    lex.skip_ws();
    if lex.peek() != Some(')') {
        return Err(ParseError::Expected {
            expected: ")".into(),
            found: lex.rest().trim().to_owned(),
        });
    }
    lex.advance();
    Ok(key)
}

fn parse_reference(lex: &mut Lex<'_>) -> Result<Reference, ParseError> {
    lex.skip_ws();
    if !lex.eat("@doc.") {
        return Err(ParseError::InvalidRef(format!(
            "expected '@doc.' at '{}'",
            lex.rest()
        )));
    }
    let key = lex
        .take_dotted_ident()
        .ok_or_else(|| ParseError::InvalidRef("empty key after '@doc.'".into()))?;
    let access = if lex.peek() == Some(':') {
        lex.advance();
        parse_access(lex)?
    } else {
        Access::Bare
    };
    Ok(Reference {
        key: DocKey::new(key),
        access,
    })
}

fn parse_access(lex: &mut Lex<'_>) -> Result<Access, ParseError> {
    let first = lex
        .take_ident()
        .ok_or_else(|| ParseError::Other("expected identifier after ':'".into()))?;

    if lex.peek() == Some('(') {
        lex.advance();
        let arg = lex
            .take_ident()
            .ok_or_else(|| ParseError::Other("expected tag name in function call".into()))?;
        lex.skip_ws();
        if lex.peek() != Some(')') {
            return Err(ParseError::Expected {
                expected: ")".into(),
                found: lex.rest().trim().to_owned(),
            });
        }
        lex.advance();
        let name = func_from_str(&first)?;
        // Optional chaining: `first(param).name`, `last(see).target`.
        let field = if lex.peek() == Some('.') {
            lex.advance();
            Some(
                lex.take_ident()
                    .ok_or_else(|| ParseError::Other("expected field name after '.'".into()))?,
            )
        } else {
            None
        };
        return Ok(Access::Func {
            name,
            tag: arg,
            field,
        });
    }

    let is_block_field = matches!(first.as_str(), "label" | "key" | "origin");
    let is_nested_field = matches!(first.as_str(), "meta" | "symbol");

    if lex.peek() == Some('[') {
        lex.advance();
        let idx_raw = lex
            .take_int()
            .ok_or_else(|| ParseError::Other("expected index".into()))?;
        let index =
            usize::try_from(idx_raw).map_err(|_| ParseError::Other("negative tag index".into()))?;
        lex.skip_ws();
        if lex.peek() != Some(']') {
            return Err(ParseError::Expected {
                expected: "]".into(),
                found: lex.rest().trim().to_owned(),
            });
        }
        lex.advance();
        if lex.peek() == Some('.') {
            lex.advance();
            let field = lex
                .take_ident()
                .ok_or_else(|| ParseError::Other("expected field name after '.'".into()))?;
            return Ok(Access::TagField {
                tag: first,
                index,
                field,
            });
        }
        return Ok(Access::TagIndex { tag: first, index });
    }

    if lex.peek() == Some('.') {
        let mut path = vec![first.clone()];
        while lex.peek() == Some('.') {
            lex.advance();
            let seg = lex
                .take_ident()
                .ok_or_else(|| ParseError::Other("expected segment after '.'".into()))?;
            path.push(seg);
        }
        if is_nested_field {
            return Ok(Access::Field(path));
        }
        if path.len() == 2 {
            // `@doc.foo:returns.description` — implicit shortcut (first occ).
            // Valid only when tag is `cardinality: Single`. Check is done in
            // evaluator since parser has no access to schema.
            return Ok(Access::TagShortcut {
                tag: path[0].clone(),
                field: path[1].clone(),
            });
        }
        return Err(ParseError::InvalidRef(format!(
            "multi-level dotted access '{}' requires an explicit index (use '{}[0].{}')",
            path.join("."),
            first,
            path[1..].join(".")
        )));
    }

    if is_block_field || is_nested_field {
        return Ok(Access::Field(vec![first]));
    }
    Ok(Access::Tag(first))
}

fn func_from_str(name: &str) -> Result<FuncName, ParseError> {
    match name {
        "has" => Ok(FuncName::Has),
        "count" => Ok(FuncName::Count),
        "first" => Ok(FuncName::First),
        "last" => Ok(FuncName::Last),
        other => Err(ParseError::UnknownFunc(other.to_owned())),
    }
}

/// Parse alias "tail" after identifier. Supports:
/// - `<alias>` (queue vide)               → `AliasAccess::Bare`
/// - `<alias>.X.Y`                         → `AliasAccess::Path`
/// - `<alias>.FUNC(arg)[.field]`           → `AliasAccess::Func`
///
/// Distinction between Path vs Func is made by `(` after the last identifier
/// — it turns the whole path into a function call (previous path must then be
/// a single segment: the function name).
fn parse_alias_tail(alias: String, lex: &mut Lex<'_>) -> Result<AliasRef, ParseError> {
    let mut segments: Vec<String> = Vec::new();
    while lex.peek() == Some('.') {
        lex.advance();
        let f = lex
            .take_ident()
            .ok_or_else(|| ParseError::Other("expected field name after '.' in alias".into()))?;
        // Detect call: `f.has(example)`. Current segment becomes function
        // name; any previous segment is invalid (no function chaining on
        // block alias).
        if lex.peek() == Some('(') {
            if !segments.is_empty() {
                return Err(ParseError::Other(format!(
                    "function call '{f}(...)' must be the first segment of an alias path, not after '{}'",
                    segments.join(".")
                )));
            }
            lex.advance();
            let arg = lex.take_ident().ok_or_else(|| {
                ParseError::Other("expected tag name in alias function call".into())
            })?;
            lex.skip_ws();
            if lex.peek() != Some(')') {
                return Err(ParseError::Expected {
                    expected: ")".into(),
                    found: lex.rest().trim().to_owned(),
                });
            }
            lex.advance();
            let func_name = func_from_str(&f)?;
            // Optional `.field` chaining (first/last only).
            let field = if lex.peek() == Some('.') {
                lex.advance();
                Some(lex.take_ident().ok_or_else(|| {
                    ParseError::Other("expected field name after '.' on alias func".into())
                })?)
            } else {
                None
            };
            return Ok(AliasRef {
                alias,
                access: AliasAccess::Func {
                    name: func_name,
                    tag: arg,
                    field,
                },
            });
        }
        segments.push(f);
    }
    let access = if segments.is_empty() {
        AliasAccess::Bare
    } else {
        AliasAccess::Path(segments)
    };
    Ok(AliasRef { alias, access })
}

fn parse_condition(lex: &mut Lex<'_>) -> Result<Condition, ParseError> {
    let left = parse_cond_target(lex)?;
    lex.skip_ws();
    let op = if lex.eat(">=") {
        Some(CompareOp::Gte)
    } else if lex.eat("<=") {
        Some(CompareOp::Lte)
    } else if lex.eat("==") {
        Some(CompareOp::Eq)
    } else if lex.eat("!=") {
        Some(CompareOp::Ne)
    } else if lex.eat(">") {
        Some(CompareOp::Gt)
    } else if lex.eat("<") {
        Some(CompareOp::Lt)
    } else {
        None
    };
    if let Some(op) = op {
        lex.skip_ws();
        let right = parse_literal(lex)?;
        return Ok(Condition::Compare { left, op, right });
    }
    Ok(Condition::Truthy(left))
}

/// `if` condition target: either `@doc.K[:access]` or dotted alias
/// (usable inside `each`).
fn parse_cond_target(lex: &mut Lex<'_>) -> Result<CondTarget, ParseError> {
    lex.skip_ws();
    if lex.peek() == Some('@') {
        return Ok(CondTarget::Ref(parse_reference(lex)?));
    }
    let alias = lex
        .take_ident()
        .ok_or_else(|| ParseError::Other("expected '@doc.…' or alias in condition".into()))?;
    Ok(CondTarget::Alias(parse_alias_tail(alias, lex)?))
}

fn parse_literal(lex: &mut Lex<'_>) -> Result<Literal, ParseError> {
    lex.skip_ws();
    if let Some(n) = lex.try_int() {
        return Ok(Literal::Int(n));
    }
    let quote = lex.peek();
    if quote == Some('"') || quote == Some('\'') {
        let q = quote.unwrap();
        lex.advance();
        let start = lex.pos;
        while let Some(c) = lex.peek() {
            if c == q {
                break;
            }
            lex.advance();
        }
        if lex.peek() != Some(q) {
            return Err(ParseError::UnterminatedString);
        }
        let s = lex.src[start..lex.pos].to_owned();
        lex.advance();
        return Ok(Literal::String(s));
    }
    if lex.eat_keyword("true") {
        return Ok(Literal::Bool(true));
    }
    if lex.eat_keyword("false") {
        return Ok(Literal::Bool(false));
    }
    Err(ParseError::Other(format!(
        "expected literal, found '{}'",
        lex.rest().trim()
    )))
}

// -------- Mini lexer --------

struct Lex<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Lex<'a> {
    const fn new(src: &'a str) -> Self {
        Self { src, pos: 0 }
    }

    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn eat(&mut self, prefix: &str) -> bool {
        if self.src[self.pos..].starts_with(prefix) {
            self.pos += prefix.len();
            true
        } else {
            false
        }
    }

    /// Like `eat`, but without consuming — used for dispatch (`@doc.` vs
    /// `@docs.`).
    fn starts_with(&self, prefix: &str) -> bool {
        self.src[self.pos..].starts_with(prefix)
    }

    fn eat_keyword(&mut self, kw: &str) -> bool {
        self.skip_ws();
        if let Some(after) = self.src[self.pos..].strip_prefix(kw) {
            let boundary = after.chars().next().is_none_or(|c| !is_ident_char(c));
            if boundary {
                self.pos += kw.len();
                return true;
            }
        }
        false
    }

    fn take_ident(&mut self) -> Option<String> {
        self.skip_ws();
        let start = self.pos;
        while let Some(c) = self.peek() {
            if is_ident_char(c) {
                self.advance();
            } else {
                break;
            }
        }
        if self.pos > start {
            Some(self.src[start..self.pos].to_owned())
        } else {
            None
        }
    }

    fn take_dotted_ident(&mut self) -> Option<String> {
        self.skip_ws();
        let start = self.pos;
        while let Some(c) = self.peek() {
            if is_ident_char(c) || c == '.' {
                self.advance();
            } else if c == ':' {
                // Accept `::` as the satellite separator inside a key.
                // A single `:` is the access marker (`@doc.K:label`) and
                // ends the key path — leave it for the caller to consume.
                let mut probe = self.src[self.pos..].chars();
                probe.next();
                if probe.next() == Some(':') {
                    self.advance();
                    self.advance();
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        if self.pos > start {
            Some(self.src[start..self.pos].to_owned())
        } else {
            None
        }
    }

    fn take_int(&mut self) -> Option<i64> {
        self.try_int()
    }

    fn try_int(&mut self) -> Option<i64> {
        self.skip_ws();
        let start = self.pos;
        if self.peek() == Some('-') {
            self.advance();
        }
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                self.advance();
            } else {
                break;
            }
        }
        let slice = &self.src[start..self.pos];
        if slice.is_empty() || slice == "-" {
            self.pos = start;
            return None;
        }
        slice.parse().ok()
    }

    fn rest(&self) -> &str {
        &self.src[self.pos..]
    }

    fn at_end(&mut self) -> bool {
        self.skip_ws();
        self.pos >= self.src.len()
    }

    fn assert_end(&mut self) -> Result<(), ParseError> {
        if self.at_end() {
            Ok(())
        } else {
            Err(ParseError::Trailing(self.rest().trim().to_owned()))
        }
    }
}

const fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

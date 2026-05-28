use crate::diag::{Span, Spanned};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct File {
    pub stmts: Vec<StmtNode>,
    pub trailing_trivia: Vec<Trivia>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StmtNode {
    pub leading: Vec<Trivia>,
    pub trailing: Option<Trivia>,
    pub node: Stmt,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", content = "text", rename_all = "snake_case")]
pub enum TriviaKind {
    LineComment(String),
    BlockComment(String),
    BlankLine,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Trivia {
    pub kind: TriviaKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Stmt {
    Assign(Assign),
    Block(Block),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Assign {
    pub key: Spanned<Ident>,
    pub value: Spanned<Expr>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Block {
    pub kind: Spanned<Ident>,
    pub label: Option<Spanned<String>>,
    pub stmts: Vec<StmtNode>,
    pub trailing_trivia: Vec<Trivia>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Ident(pub String);

impl Ident {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Expr {
    Int(i64),
    Float(f64),
    Bool(bool),
    Null,
    String(StringLit),
    Ref(Ref),
    List(Vec<Spanned<Expr>>),
    Map(Vec<MapEntry>),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Ref {
    pub path: Vec<Spanned<Ident>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MapEntry {
    pub key: Spanned<Ident>,
    pub value: Spanned<Expr>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StringLit {
    pub parts: Vec<StringPart>,
    pub multiline: bool,
    /// true for `` `...` `` and `` ```...``` `` (interpolation allowed),
    /// false for plain `"..."` (literal, no `${}` substitution).
    pub template: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum StringPart {
    Lit(String),
    Interp(Spanned<InterpExpr>),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum InterpExpr {
    Ref(Ref),
    Int(i64),
    Float(f64),
    Bool(bool),
    Null,
    String(String),
}

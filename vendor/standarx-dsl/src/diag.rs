//! Diagnostic primitives: spans, severities, error type.
//!
//! Vendored from `standarbuild`'s `diag` module (the renderer is left to
//! downstream consumers — see `standarbuild::diag::render` for a sample).

use std::ops::Range;

pub type Span = Range<usize>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spanned<T> {
    pub node: T,
    pub span: Span,
}

impl<T> Spanned<T> {
    pub fn new(node: T, span: Span) -> Self {
        Self { node, span }
    }

    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> Spanned<U> {
        Spanned {
            node: f(self.node),
            span: self.span,
        }
    }

    pub fn as_ref(&self) -> Spanned<&T> {
        Spanned {
            node: &self.node,
            span: self.span.clone(),
        }
    }
}

impl<T: serde::Serialize> serde::Serialize for Spanned<T> {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        self.node.serialize(ser)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum DiagKind {
    #[error("parse error: {0}")]
    Parse(String),
    #[error("schema error: {0}")]
    Schema(String),
}

#[derive(Debug, Clone)]
pub struct Diag {
    pub kind: DiagKind,
    pub span: Span,
    pub severity: Severity,
}

impl Diag {
    pub fn parse(span: Span, msg: impl Into<String>) -> Self {
        Self {
            kind: DiagKind::Parse(msg.into()),
            span,
            severity: Severity::Error,
        }
    }

    pub fn schema(span: Span, msg: impl Into<String>) -> Self {
        Self {
            kind: DiagKind::Schema(msg.into()),
            span,
            severity: Severity::Error,
        }
    }

    pub fn schema_warn(span: Span, msg: impl Into<String>) -> Self {
        Self {
            kind: DiagKind::Schema(msg.into()),
            span,
            severity: Severity::Warning,
        }
    }

    pub fn is_error(&self) -> bool {
        matches!(self.severity, Severity::Error)
    }
}

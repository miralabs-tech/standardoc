use crate::model::SourceRange;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Standardoc diagnostic code — stable across releases, prefixed `STD`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DiagnosticCode(pub String);

impl DiagnosticCode {
    pub fn new(code: impl Into<String>) -> Self {
        Self(code.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub severity: Severity,
    pub message: String,
    pub path: PathBuf,
    pub range: SourceRange,
    #[serde(default)]
    pub related: Vec<RelatedInformation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedInformation {
    pub message: String,
    pub path: PathBuf,
    pub range: SourceRange,
}

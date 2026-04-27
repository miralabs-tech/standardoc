use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CommentStyle {
    SingleLine,
    MultiLine,
    DocSingle,
    DocMulti,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CommentDelimiters {
    pub start: String,
    pub end: String,
}

/// Per-language comment syntax configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentStyles {
    #[serde(default)]
    pub single: Vec<String>,
    #[serde(default)]
    pub multi: Option<CommentDelimiters>,
    #[serde(default)]
    pub doc_single: Vec<String>,
    #[serde(default)]
    pub doc_multi: Option<CommentDelimiters>,
}

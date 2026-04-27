use crate::model::CommentStyle;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Metadata automatically attached to every doc block.
///
/// Timestamps are Unix seconds to keep JSON output simple and portable.
/// Convert from `SystemTime` at the ingestion boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocMeta {
    pub path: PathBuf,
    pub line_start: u32,
    pub line_end: u32,
    pub column: u32,
    pub file_ext: String,
    pub comment_style: CommentStyle,
    /// When standardoc last indexed this block (unix seconds).
    pub last_indexed: i64,
    /// mtime of the source file at index time (unix seconds).
    pub source_mtime: i64,
}

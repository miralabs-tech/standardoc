use serde::{Deserialize, Serialize};

/// 1-indexed inclusive source range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceRange {
    pub line_start: u32,
    pub line_end: u32,
    pub column_start: u32,
    pub column_end: u32,
}

impl SourceRange {
    pub const fn single_line(line: u32, column_start: u32, column_end: u32) -> Self {
        Self {
            line_start: line,
            line_end: line,
            column_start,
            column_end,
        }
    }
}

use serde::{Deserialize, Serialize};

pub type TagName = String;

/// Parsed fields of a single tag occurrence.
///
/// Example: `@param a number First arg` → `["a", "number", "First arg"]`.
pub type TagFields = Vec<String>;

/// Flat representation of a tag occurrence with its source line — useful for
/// clients that want a list view rather than the grouped `BTreeMap` in `DocBlock`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagOccurrence {
    pub name: TagName,
    pub fields: TagFields,
    pub line: u32,
}

use serde::{Deserialize, Serialize};
use std::fmt;

/// Stable identifier of a documentable block.
///
/// Derived from a symbol's fully qualified name (FQN) when discovery is
/// AST-driven, or set explicitly via `@doc <key>` in annotation mode.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DocKey(String);

impl DocKey {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl From<&str> for DocKey {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for DocKey {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl fmt::Display for DocKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

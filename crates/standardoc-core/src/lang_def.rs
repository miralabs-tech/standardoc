//! JSON definitions for language providers loaded at runtime.
//!
//! The user drops `.standardoc/languages/<id>.json` files at the workspace
//! root. Standardoc loads them at boot and instantiates a matching provider
//! alongside the built-in providers.
//!
//! Use cases:
//! - **Language fork**: `extends = "lua"` + query overrides to capture
//!   patterns specific to a dialect (`Teal`, `MoonScript`, `Fennel`, …)
//! - **Full override**: replace a built-in's query with your own (useful
//!   when you want to be more permissive/strict than the default)
//!
//! We do not load native plugins (.dll/.so) or WASM grammars at this level
//! — runtime grammar loading is planned for a future release.
//!
//! ## Format
//!
//! ```json
//! {
//!   "id": "myx",
//!   "extensions": [".myx"],
//!   "commentStyles": {
//!     "single": ["--"],
//!     "docSingle": ["---"]
//!   },
//!   "backend": {
//!     "kind": "treeSitterFork",
//!     "base": "lua",
//!     "extraPatterns": "(function_call ...) @symbol @kind.fn"
//!   }
//! }
//! ```

use crate::model::CommentStyles;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Subdirectory (relative to the workspace) where the loader looks for
/// configs.
pub const LANGUAGES_DIR: &str = ".standardoc/languages";

/// Definition of a dynamically-loaded provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageDef {
    /// Stable identifier (`"teal"`, `"moon"`, …). If a built-in shares
    /// the same id, the dynamic provider **replaces** it to avoid double
    /// scanning.
    pub id: String,
    /// Extensions handled by this provider (`[".lua", ".tl"]`).
    pub extensions: Vec<String>,
    /// Comment styles for the scanner — determines what becomes a
    /// doc-comment vs a regular comment.
    #[serde(default)]
    pub comment_styles: CommentStyles,
    pub backend: LanguageBackend,
}

/// Which extraction engine to use for this language.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LanguageBackend {
    /// Reuses a tree-sitter grammar already compiled into the binary
    /// (`base = "lua"` today), with a custom query or extra patterns.
    #[serde(rename_all = "camelCase")]
    TreeSitterFork {
        /// Identifier of the base tree-sitter provider to extend.
        base: String,
        /// If present, **fully replaces** the default query.
        #[serde(default)]
        query: Option<String>,
        /// If present (and `query` is absent), **appends** these patterns
        /// to the default query. Convenient to add a pattern without
        /// copying the entire existing query.
        #[serde(default)]
        extra_patterns: Option<String>,
    },
    /// Pure-regex provider — fallback for exotic languages without an
    /// available grammar. No AST understanding, just a line-by-line scan.
    /// Moderate precision, but lets you cover any language or text format
    /// with a bit of configuration.
    #[serde(rename_all = "camelCase")]
    Regex {
        /// List of patterns applied to each line of the file. Multiple
        /// patterns may match the same line (e.g. one captures fns, another
        /// captures classes).
        patterns: Vec<RegexPatternDef>,
    },
}

/// A regex pattern for the `Regex` backend. The regex must have at least a
/// named `name` capture; `params` and `signature` captures are optional.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegexPatternDef {
    /// Which `SymbolKind` to assign to matches: `"function"`, `"class"`,
    /// `"struct"`, `"enum"`, `"trait"`, `"module"`, etc. Case-insensitive.
    /// Unknown → defaults to `Function`.
    pub kind: String,
    /// Regex to compile. Must use `(?P<name>...)` at minimum.
    pub regex: String,
}

/// Error loading / parsing a dynamic language definition.
#[derive(Debug, thiserror::Error)]
pub enum LanguageDefError {
    #[error("failed to read {path}: {source}")]
    Read {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse {path}: {source}")]
    Parse {
        path: std::path::PathBuf,
        source: serde_json::Error,
    },
}

/// Loads every `.standardoc/languages/*.json` at the workspace root.
///
/// Best-effort: a malformed file is logged to stderr, we keep going with
/// the others. Returns the list of loaded defs (potentially empty).
///
/// Not an error if the `.standardoc/languages/` directory doesn't exist —
/// that's the most common case (workspace without custom languages).
#[must_use]
pub fn load_workspace_languages(workspace: &Path) -> Vec<LanguageDef> {
    let dir = workspace.join(LANGUAGES_DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match load_one(&path) {
            Ok(def) => out.push(def),
            Err(err) => eprintln!("standardoc: skipping invalid language def: {err}"),
        }
    }
    out
}

fn load_one(path: &Path) -> Result<LanguageDef, LanguageDefError> {
    let content = std::fs::read_to_string(path).map_err(|err| LanguageDefError::Read {
        path: path.to_path_buf(),
        source: err,
    })?;
    serde_json::from_str(&content).map_err(|err| LanguageDefError::Parse {
        path: path.to_path_buf(),
        source: err,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_tree_sitter_fork_def() {
        let json = r#"{
            "id": "teal",
            "extensions": [".tl"],
            "backend": {
                "kind": "treeSitterFork",
                "base": "lua",
                "extraPatterns": "; nothing"
            }
        }"#;
        let def: LanguageDef = serde_json::from_str(json).unwrap();
        assert_eq!(def.id, "teal");
        assert_eq!(def.extensions, vec![".tl"]);
        match def.backend {
            LanguageBackend::TreeSitterFork {
                base,
                extra_patterns,
                query,
            } => {
                assert_eq!(base, "lua");
                assert_eq!(extra_patterns.as_deref(), Some("; nothing"));
                assert!(query.is_none());
            }
            LanguageBackend::Regex { .. } => panic!("expected TreeSitterFork"),
        }
    }

    #[test]
    fn parses_with_full_query_override() {
        let json = r#"{
            "id": "myx",
            "extensions": [".myx"],
            "backend": {
                "kind": "treeSitterFork",
                "base": "lua",
                "query": "(comment) @ignore"
            }
        }"#;
        let def: LanguageDef = serde_json::from_str(json).unwrap();
        match def.backend {
            LanguageBackend::TreeSitterFork {
                query,
                extra_patterns,
                ..
            } => {
                assert_eq!(query.as_deref(), Some("(comment) @ignore"));
                assert!(extra_patterns.is_none());
            }
            LanguageBackend::Regex { .. } => panic!("expected TreeSitterFork"),
        }
    }

    #[test]
    fn loads_from_workspace_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(LANGUAGES_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("teal.json"),
            r#"{"id":"teal","extensions":[".tl"],"backend":{"kind":"treeSitterFork","base":"lua"}}"#,
        )
        .unwrap();
        // Non-json files must be ignored.
        std::fs::write(dir.join("readme.md"), "not json").unwrap();

        let defs = load_workspace_languages(tmp.path());
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].id, "teal");
    }

    #[test]
    fn missing_directory_returns_empty_vec() {
        let tmp = tempfile::tempdir().unwrap();
        let defs = load_workspace_languages(tmp.path());
        assert!(defs.is_empty());
    }

    #[test]
    fn invalid_json_is_skipped_silently() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(LANGUAGES_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bad.json"), "{ this is not json").unwrap();
        std::fs::write(
            dir.join("good.json"),
            r#"{"id":"good","extensions":[".g"],"backend":{"kind":"treeSitterFork","base":"lua"}}"#,
        )
        .unwrap();

        let defs = load_workspace_languages(tmp.path());
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].id, "good");
    }
}

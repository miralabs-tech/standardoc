//! Generic regex-driven provider — ultimate fallback for languages without
//! a tree-sitter grammar or a native AST.
//!
//! The user declares patterns in their `.standardoc/languages/` JSON and
//! standardoc runs a line-by-line scan. No AST precision: we can't tell
//! a comment apart from a literal that contains `function`. But for many
//! exotic languages it's enough to give agents a navigable index.
//!
//! ## JSON format
//!
//! ```json
//! {
//!   "id": "myexotic",
//!   "extensions": [".myx"],
//!   "commentStyles": {
//!     "single": ["#"],
//!     "docSingle": ["##"]
//!   },
//!   "backend": {
//!     "kind": "regex",
//!     "patterns": [
//!       {
//!         "kind": "function",
//!         "regex": "^\\s*function\\s+(?P<name>[a-zA-Z_][a-zA-Z0-9_]*)\\s*\\((?P<params>[^)]*)\\)"
//!       }
//!     ]
//!   }
//! }
//! ```
//!
//! ## Expected regex captures
//!
//! - `name` (required) — symbol name
//! - `params` (optional) — comma-separated parameter list
//! - `signature` (optional) — used as-is when present; otherwise we
//!   synthesize `<kind> <name>(<params>)`
//!
//! Matches without a `name` capture are silently skipped.

use crate::lang::{DiscoveredSymbol, LanguageProvider, ParseError};
use crate::lang_def::{LanguageBackend, LanguageDef};
use crate::model::{
    CommentStyles, ParamInfo, References, SourceRange, SymbolInfo, SymbolKind, Visibility,
};
use regex::Regex;
use std::path::Path;

/// Fallback provider: runs a list of compiled regexes against each line
/// and emits one `DiscoveredSymbol` per matching capture.
pub struct RegexProvider {
    /// `&'static` to match the `LanguageProvider::id` trait signature. We
    /// leak the string once at boot — negligible cost for a server's
    /// process lifetime.
    id: &'static str,
    extensions: &'static [&'static str],
    comment_styles: CommentStyles,
    patterns: Vec<CompiledPattern>,
    /// Doc-comment prefixes (e.g. `["///", "##"]`) used to collect the
    /// comment block that precedes a symbol.
    doc_comment_prefixes: Vec<String>,
}

// Manual Debug impl: we omit `patterns` (compiled regex isn't
// Debug-friendly) and `comment_styles` / `doc_comment_prefixes` (verbose
// and rarely useful in debug). We expose just id, extensions, and the
// pattern count.
#[allow(clippy::missing_fields_in_debug)]
impl std::fmt::Debug for RegexProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegexProvider")
            .field("id", &self.id)
            .field("extensions", &self.extensions)
            .field("patterns_count", &self.patterns.len())
            .finish()
    }
}

struct CompiledPattern {
    kind: SymbolKind,
    regex: Regex,
}

/// Error building a `RegexProvider` from a `LanguageDef`.
#[derive(Debug, thiserror::Error)]
pub enum RegexProviderError {
    #[error("backend is not 'regex' for language '{0}'")]
    WrongBackend(String),
    #[error("invalid regex for language '{id}' pattern #{index}: {source}")]
    InvalidRegex {
        id: String,
        index: usize,
        #[source]
        source: regex::Error,
    },
    #[error("regex for '{id}' pattern #{index} has no `name` capture group")]
    MissingNameCapture { id: String, index: usize },
}

impl RegexProvider {
    /// Builds a regex provider from a JSON definition. Pre-compiles every
    /// pattern and checks each one has at least a `name` capture.
    pub fn from_lang_def(def: &LanguageDef) -> Result<Self, RegexProviderError> {
        let LanguageBackend::Regex { patterns } = &def.backend else {
            return Err(RegexProviderError::WrongBackend(def.id.clone()));
        };

        let mut compiled: Vec<CompiledPattern> = Vec::with_capacity(patterns.len());
        for (i, p) in patterns.iter().enumerate() {
            let regex = Regex::new(&p.regex).map_err(|err| RegexProviderError::InvalidRegex {
                id: def.id.clone(),
                index: i,
                source: err,
            })?;
            // Sanity check: the `name` capture is required so we can
            // extract a symbol. Without it the pattern is structurally
            // invalid.
            if !regex.capture_names().any(|n| n == Some("name")) {
                return Err(RegexProviderError::MissingNameCapture {
                    id: def.id.clone(),
                    index: i,
                });
            }
            compiled.push(CompiledPattern {
                kind: parse_kind(&p.kind),
                regex,
            });
        }

        let id_static: &'static str = Box::leak(def.id.clone().into_boxed_str());
        let extensions_owned: Vec<&'static str> = def
            .extensions
            .iter()
            .map(|s| Box::leak(s.clone().into_boxed_str()) as &'static str)
            .collect();
        let extensions_slice: &'static [&'static str] = extensions_owned.leak();

        let doc_comment_prefixes = if def.comment_styles.doc_single.is_empty() {
            // Reasonable fallback: if the user only declared regular
            // comments, we use those to collect doc-comments. Otherwise
            // we'd be silently blind to leading documentation.
            def.comment_styles.single.clone()
        } else {
            def.comment_styles.doc_single.clone()
        };

        Ok(Self {
            id: id_static,
            extensions: extensions_slice,
            comment_styles: def.comment_styles.clone(),
            patterns: compiled,
            doc_comment_prefixes,
        })
    }
}

impl LanguageProvider for RegexProvider {
    fn id(&self) -> &'static str {
        self.id
    }

    fn extensions(&self) -> &[&'static str] {
        self.extensions
    }

    fn comment_styles(&self) -> &CommentStyles {
        &self.comment_styles
    }

    fn discover_symbols(
        &self,
        content: &str,
        _path: &Path,
    ) -> Result<Vec<DiscoveredSymbol>, ParseError> {
        let lines: Vec<&str> = content.lines().collect();
        let mut out = Vec::new();
        for (line_idx, line) in lines.iter().enumerate() {
            for pattern in &self.patterns {
                let Some(caps) = pattern.regex.captures(line) else {
                    continue;
                };
                let Some(name_cap) = caps.name("name") else {
                    continue;
                };
                let name = name_cap.as_str().to_owned();
                let params = caps
                    .name("params")
                    .map(|m| parse_params(m.as_str()))
                    .unwrap_or_default();
                let signature = caps.name("signature").map_or_else(
                    || synthesize_signature(pattern.kind, &name, &params),
                    |m| m.as_str().to_owned(),
                );
                let visibility = if name.starts_with('_') {
                    Visibility::Private
                } else {
                    Visibility::Public
                };
                let leading_comment =
                    collect_leading_comment(&lines, line_idx, &self.doc_comment_prefixes);
                let line_u32 = u32::try_from(line_idx + 1).unwrap_or(u32::MAX);
                let col = line.find(name.as_str()).unwrap_or(0);
                let col_u32 = u32::try_from(col + 1).unwrap_or(1);

                out.push(DiscoveredSymbol {
                    fqn: vec![name],
                    symbol: SymbolInfo {
                        kind: pattern.kind,
                        visibility,
                        signature,
                        params,
                        returns: None,
                        generics: vec![],
                        decorators: vec![],
                        is_async: false,
                        is_deprecated: false,
                        references: References::default(),
                    },
                    source_range: SourceRange {
                        line_start: line_u32,
                        line_end: line_u32,
                        column_start: col_u32,
                        column_end: col_u32,
                    },
                    leading_comment,
                    leading_comment_line_start: None,
                });
            }
        }
        Ok(out)
    }
}

/// String → `SymbolKind` mapping. Case-insensitive, accepts the common
/// short names (`function`, `method`, `class`, `module`, …). Unknown →
/// `Function` (sensible fallback).
fn parse_kind(kind: &str) -> SymbolKind {
    match kind.to_ascii_lowercase().as_str() {
        "method" => SymbolKind::Method,
        "class" => SymbolKind::Class,
        "struct" => SymbolKind::Struct,
        "enum" => SymbolKind::Enum,
        "trait" | "interface" => SymbolKind::Interface,
        "type" | "typealias" | "alias" => SymbolKind::TypeAlias,
        "const" | "constant" => SymbolKind::Const,
        "static" => SymbolKind::Static,
        "module" | "namespace" => SymbolKind::Module,
        "macro" => SymbolKind::Macro,
        "field" => SymbolKind::Field,
        "variant" => SymbolKind::Variant,
        _ => SymbolKind::Function,
    }
}

/// Splits the `params` capture on `,`, trims each element, and builds
/// the `ParamInfo`s. No typing — we're in regex mode, not AST mode.
fn parse_params(raw: &str) -> Vec<ParamInfo> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|name| ParamInfo {
            name: name.to_owned(),
            type_repr: None,
            default: None,
            is_optional: false,
            is_variadic: name == "...",
        })
        .collect()
}

fn synthesize_signature(kind: SymbolKind, name: &str, params: &[ParamInfo]) -> String {
    let kw = match kind {
        SymbolKind::Function | SymbolKind::Method => "function",
        SymbolKind::Class => "class",
        SymbolKind::Struct => "struct",
        SymbolKind::Enum => "enum",
        SymbolKind::Trait | SymbolKind::Interface => "interface",
        SymbolKind::TypeAlias => "type",
        SymbolKind::Const => "const",
        SymbolKind::Static => "static",
        SymbolKind::Module => "module",
        SymbolKind::Macro => "macro",
        SymbolKind::Field => "field",
        SymbolKind::Variant => "variant",
        SymbolKind::Other => "symbol",
    };
    if matches!(kind, SymbolKind::Function | SymbolKind::Method) {
        let p = params
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        format!("{kw} {name}({p})")
    } else {
        format!("{kw} {name}")
    }
}

/// Walks the lines above `symbol_row` and collects contiguous comments
/// matching one of the declared doc prefixes. Returns `None` if there's
/// nothing to collect.
fn collect_leading_comment(
    lines: &[&str],
    symbol_row: usize,
    doc_prefixes: &[String],
) -> Option<String> {
    if symbol_row == 0 || doc_prefixes.is_empty() {
        return None;
    }
    let mut collected: Vec<String> = Vec::new();
    let mut i = symbol_row;
    while i > 0 {
        i -= 1;
        let line = lines.get(i)?;
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            break;
        }
        let matched = doc_prefixes
            .iter()
            .find_map(|p| trimmed.strip_prefix(p.as_str()));
        match matched {
            Some(rest) => collected.push(rest.trim_start().to_owned()),
            None => break,
        }
    }
    if collected.is_empty() {
        None
    } else {
        collected.reverse();
        Some(collected.join("\n").trim().to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang_def::{LanguageBackend, LanguageDef, RegexPatternDef};
    use crate::model::CommentStyles;

    fn def_with_patterns(patterns: Vec<RegexPatternDef>) -> LanguageDef {
        LanguageDef {
            id: "test".to_owned(),
            extensions: vec![".x".to_owned()],
            comment_styles: CommentStyles {
                single: vec!["#".to_owned()],
                doc_single: vec!["##".to_owned()],
                ..CommentStyles::default()
            },
            backend: LanguageBackend::Regex { patterns },
        }
    }

    #[test]
    fn discovers_simple_function() {
        let def = def_with_patterns(vec![RegexPatternDef {
            kind: "function".to_owned(),
            regex: r"^\s*fn\s+(?P<name>\w+)\s*\((?P<params>[^)]*)\)".to_owned(),
        }]);
        let provider = RegexProvider::from_lang_def(&def).unwrap();
        let symbols = provider
            .discover_symbols("fn add(a, b)\n", Path::new("test.x"))
            .unwrap();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].fqn, vec!["add"]);
        assert_eq!(symbols[0].symbol.kind, SymbolKind::Function);
        assert_eq!(symbols[0].symbol.params.len(), 2);
        assert_eq!(symbols[0].symbol.params[0].name, "a");
    }

    #[test]
    fn underscore_prefix_is_private() {
        let def = def_with_patterns(vec![RegexPatternDef {
            kind: "function".to_owned(),
            regex: r"^\s*fn\s+(?P<name>\w+)".to_owned(),
        }]);
        let provider = RegexProvider::from_lang_def(&def).unwrap();
        let symbols = provider
            .discover_symbols("fn _hidden\n", Path::new("test.x"))
            .unwrap();
        assert_eq!(symbols[0].symbol.visibility, Visibility::Private);
    }

    #[test]
    fn collects_leading_doc_comments() {
        let def = def_with_patterns(vec![RegexPatternDef {
            kind: "function".to_owned(),
            regex: r"^\s*fn\s+(?P<name>\w+)".to_owned(),
        }]);
        let provider = RegexProvider::from_lang_def(&def).unwrap();
        let src = "## first line\n## second line\nfn greet\n";
        let symbols = provider.discover_symbols(src, Path::new("test.x")).unwrap();
        let comment = symbols[0].leading_comment.as_deref().unwrap_or("");
        assert!(comment.contains("first line"));
        assert!(comment.contains("second line"));
    }

    #[test]
    fn missing_name_capture_errors() {
        let def = def_with_patterns(vec![RegexPatternDef {
            kind: "function".to_owned(),
            regex: r"^fn\s+\w+".to_owned(),
        }]);
        let err = RegexProvider::from_lang_def(&def).unwrap_err();
        assert!(matches!(err, RegexProviderError::MissingNameCapture { .. }));
    }

    #[test]
    fn invalid_regex_errors() {
        let def = def_with_patterns(vec![RegexPatternDef {
            kind: "function".to_owned(),
            regex: r"^fn\s+(?P<name>".to_owned(),
        }]);
        let err = RegexProvider::from_lang_def(&def).unwrap_err();
        assert!(matches!(err, RegexProviderError::InvalidRegex { .. }));
    }

    #[test]
    fn signature_capture_overrides_synthesis() {
        let def = def_with_patterns(vec![RegexPatternDef {
            kind: "function".to_owned(),
            regex: r"^(?P<signature>fn\s+(?P<name>\w+)\s*\([^)]*\))".to_owned(),
        }]);
        let provider = RegexProvider::from_lang_def(&def).unwrap();
        let symbols = provider
            .discover_symbols("fn add(a, b)\n", Path::new("test.x"))
            .unwrap();
        assert_eq!(symbols[0].symbol.signature, "fn add(a, b)");
    }

    #[test]
    fn multiple_patterns_match_independently() {
        let def = def_with_patterns(vec![
            RegexPatternDef {
                kind: "function".to_owned(),
                regex: r"^fn\s+(?P<name>\w+)".to_owned(),
            },
            RegexPatternDef {
                kind: "class".to_owned(),
                regex: r"^class\s+(?P<name>\w+)".to_owned(),
            },
        ]);
        let provider = RegexProvider::from_lang_def(&def).unwrap();
        let src = "fn foo\nclass Bar\nfn baz\n";
        let symbols = provider.discover_symbols(src, Path::new("test.x")).unwrap();
        assert_eq!(symbols.len(), 3);
        let kinds: Vec<SymbolKind> = symbols.iter().map(|s| s.symbol.kind).collect();
        assert_eq!(
            kinds,
            vec![
                SymbolKind::Function,
                SymbolKind::Class,
                SymbolKind::Function
            ]
        );
    }
}

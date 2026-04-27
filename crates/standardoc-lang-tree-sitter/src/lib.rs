//! Generic tree-sitter language provider — fallback for exotic languages.
//!
//! The idea: most "second-tier" languages (Lua, Sh, GLSL, Solidity,
//! Nim, Zig, ...) already have high-quality tree-sitter grammars. Instead of
//! writing one crate per language, we host a single provider here that takes
//! a `LangConfig` (grammar + extensions + comment styles + tree-sitter query)
//! and produces `DiscoveredSymbol`s. Cost of adding a language: add
//! `pub fn xxx() -> Self` plus the query.
//!
//! We start with **Lua** as the reference grammar: it is the most requested
//! exotic language (Neovim plugins, gamedev, embedded scripting), and the
//! grammar is very stable.
//!
//! ## Explicit limits
//!
//! - **No typed cross-ref.** Lua, sh, glsl and friends are dynamically typed;
//!   we extract function/class names and arguments but not `ParamType` /
//!   `ReturnType` edges. `find_usages` by direct call can still be done via
//!   a "calls" query, but that's out of scope for v1.
//! - **Source range = header line only.** We do not descend to end of body —
//!   not critical for UX (editor jump is still accurate).
//! - **Visibility** follows language convention: `local` -> `Private` in
//!   Lua, otherwise `Public`.

#![forbid(unsafe_code)]

use standardoc_core::lang::{DiscoveredSymbol, LanguageProvider, ParseError};
use standardoc_core::lang_def::{LanguageBackend, LanguageDef};
use standardoc_core::model::{
    CommentStyles, ParamInfo, References, SourceRange, SymbolInfo, SymbolKind, Visibility,
};
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, StreamingIterator};

/// Function pointer type for obtaining a tree-sitter `Language` instance.
/// Exposed so callers don't need a direct `tree_sitter` dependency.
pub type LanguageFn = fn() -> Language;

/// A comment node extracted from a source file via tree-sitter.
#[derive(Debug, Clone)]
pub struct CommentEntry {
    /// 0-based line number (matches tree-sitter's `start_position().row`).
    pub line: usize,
    /// Raw comment text as it appears in the source (trimmed of surrounding whitespace).
    pub text: String,
}

/// Extract every comment node from `content` using the given tree-sitter grammar.
///
/// Uses the standard `(comment) @comment` query — valid for all grammars that
/// model comments as `comment` nodes (Lua, Teal, Rust, TS, Python, …).
/// Returns an empty vec on parse failure rather than surfacing an error, so the
/// tool remains useful even for files with minor syntax issues.
pub fn extract_comments(language_fn: LanguageFn, content: &str) -> Vec<CommentEntry> {
    let lang = language_fn();
    let mut parser = Parser::new();
    if parser.set_language(&lang).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(content, None) else {
        return Vec::new();
    };
    let Ok(query) = Query::new(&lang, "(comment) @comment") else {
        return Vec::new();
    };
    let bytes = content.as_bytes();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), bytes);
    let mut entries = Vec::new();
    while let Some(m) = matches.next() {
        for cap in m.captures {
            let text = cap.node.utf8_text(bytes).unwrap_or("").trim().to_owned();
            entries.push(CommentEntry {
                line: cap.node.start_position().row,
                text,
            });
        }
    }
    entries
}

/// Configuration of a tree-sitter-driven language. Owned strings to
/// support **both** built-in providers and configs loaded at runtime
/// from `.standardoc/languages/*.json`. Allocation cost at boot is
/// negligible (one allocation per provider).
struct LangConfig {
    /// `&'static` because the `LanguageProvider::id` trait requires that
    /// lifetime. For runtime configs we leak the `String` at boot.
    id: &'static str,
    /// Same as `id`: leaked to match the trait signature.
    extensions: &'static [&'static str],
    comment_styles: CommentStyles,
    /// Compiled grammar. Retrieved from grammar crate function
    /// (e.g. `tree_sitter_lua::LANGUAGE.into()`). Stored in `OnceLock`
    /// to share the same `Language` across instances.
    language: fn() -> Language,
    /// Tree-sitter query used to extract symbols. Must use these captures:
    ///   - `@symbol`   : root symbol node (`function_declaration`, ...)
    ///   - `@name`     : identifier node (single or dotted/method)
    ///   - `@kind.fn`  | `@kind.method` | `@kind.class` | `@kind.module` :
    ///     indicates [`SymbolKind`] to assign (at least one per match)
    ///   - `@params`   : parameter list (optional)
    ///   - `@private`  : if present, force `Visibility::Private`
    symbols_query: String,
}

/// @doc lang.providers.tree_sitter Lua
/// @description Generic tree-sitter language provider — drives Lua out of the box, plus any language declared via a `.standardoc/languages/*.json` config (Teal, MoonScript, CfxLua, Bash, …).
/// @crate standardoc-lang-tree-sitter
/// @backend tree-sitter
pub struct TreeSitterProvider {
    config: LangConfig,
    /// `Query` compiled on first use. Cost: ~ms.
    query: OnceLock<Query>,
    language: OnceLock<Language>,
}

impl std::fmt::Debug for TreeSitterProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Do not dump compiled `Query` (heavy, not useful for debugging).
        f.debug_struct("TreeSitterProvider")
            .field("id", &self.config.id)
            .field("extensions", &self.config.extensions)
            .field("query_compiled", &self.query.get().is_some())
            .field("language_compiled", &self.language.get().is_some())
            .finish()
    }
}

/// Provider construction error from a runtime `LanguageDef`.
#[derive(Debug, thiserror::Error)]
pub enum FromDefError {
    #[error("backend is not 'treeSitterFork' for language '{0}'")]
    WrongBackend(String),
    #[error("unknown base grammar '{0}' — built-in bases: lua")]
    UnknownBase(String),
    #[error("invalid tree-sitter query for language '{id}': {source}")]
    InvalidQuery {
        id: String,
        #[source]
        source: tree_sitter::QueryError,
    },
}

impl TreeSitterProvider {
    /// Provider configured for Lua 5.x via `tree-sitter-lua` grammar.
    #[must_use]
    pub fn lua() -> Self {
        Self::new_with(
            "lua".to_owned(),
            vec![".lua".to_owned()],
            CommentStyles {
                single: vec!["--".to_owned()],
                multi: Some(standardoc_core::model::CommentDelimiters {
                    start: "--[[".to_owned(),
                    end: "]]".to_owned(),
                }),
                doc_single: vec!["---".to_owned()],
                doc_multi: None,
            },
            || tree_sitter_lua::LANGUAGE.into(),
            LUA_SYMBOLS_QUERY.to_owned(),
        )
    }

    /// Builds a provider from a runtime definition
    /// (`.standardoc/languages/*.json`). Resolves the `base` grammar
    /// against the built-in grammar table, applies the query overrides,
    /// and pre-compiles the query to validate that it's syntactically
    /// OK **before** registering the provider.
    pub fn from_lang_def(def: &LanguageDef) -> Result<Self, FromDefError> {
        let LanguageBackend::TreeSitterFork {
            base,
            query,
            extra_patterns,
        } = &def.backend
        else {
            return Err(FromDefError::WrongBackend(def.id.clone()));
        };
        let language_fn =
            base_grammar(base).ok_or_else(|| FromDefError::UnknownBase(base.clone()))?;
        let base_query = base_default_query(base).unwrap_or("");
        let final_query = match (query, extra_patterns) {
            (Some(q), _) => q.clone(),
            (None, Some(extra)) => {
                format!("{base_query}\n\n; -- extras from runtime config --\n{extra}")
            }
            (None, None) => base_query.to_owned(),
        };

        // Pre-compile the query to catch syntax errors at boot rather
        // than on first use (where it would panic).
        let lang_instance = language_fn();
        Query::new(&lang_instance, &final_query).map_err(|err| FromDefError::InvalidQuery {
            id: def.id.clone(),
            source: err,
        })?;

        Ok(Self::new_with(
            def.id.clone(),
            def.extensions.clone(),
            def.comment_styles.clone(),
            language_fn,
            final_query,
        ))
    }

    fn new_with(
        id: String,
        extensions: Vec<String>,
        comment_styles: CommentStyles,
        language: fn() -> Language,
        symbols_query: String,
    ) -> Self {
        // Leak the strings to match the `'static` lifetime required by
        // the `LanguageProvider` trait. Cost: a few dozen bytes per
        // provider, allocated at boot, never freed — irrelevant for a
        // long-lived server process (the leak is bounded by the number
        // of language configs in `.standardoc/languages/`).
        let id_static: &'static str = Box::leak(id.into_boxed_str());
        let extensions_static: Vec<&'static str> = extensions
            .into_iter()
            .map(|s| Box::leak(s.into_boxed_str()) as &'static str)
            .collect();
        let extensions_slice: &'static [&'static str] = extensions_static.leak();
        Self {
            config: LangConfig {
                id: id_static,
                extensions: extensions_slice,
                comment_styles,
                language,
                symbols_query,
            },
            query: OnceLock::new(),
            language: OnceLock::new(),
        }
    }

    /// Returns the raw language function pointer for this provider.
    /// Useful for callers that need the grammar without going through the full provider lifecycle.
    pub fn language_fn(&self) -> LanguageFn {
        self.config.language
    }

    fn language(&self) -> &Language {
        self.language.get_or_init(|| (self.config.language)())
    }

    fn query(&self) -> &Query {
        self.query.get_or_init(|| {
            Query::new(self.language(), &self.config.symbols_query)
                .expect("query was validated at construction time")
        })
    }
}

/// Table of tree-sitter grammars compiled in binary. Used by `from_lang_def`
/// to resolve `base = "lua"` -> grammar function.
/// To add a new base: register it here and add the crate in `Cargo.toml`.
fn base_grammar(id: &str) -> Option<fn() -> Language> {
    match id {
        "lua" => Some(|| tree_sitter_lua::LANGUAGE.into()),
        _ => None,
    }
}

/// Default query for a base grammar — used by forks that pass
/// `extraPatterns` without full override via `query`.
const fn base_default_query(id: &str) -> Option<&'static str> {
    match id.as_bytes() {
        b"lua" => Some(LUA_SYMBOLS_QUERY),
        _ => None,
    }
}

impl LanguageProvider for TreeSitterProvider {
    fn id(&self) -> &'static str {
        self.config.id
    }

    fn extensions(&self) -> &[&'static str] {
        self.config.extensions
    }

    fn comment_styles(&self) -> &CommentStyles {
        &self.config.comment_styles
    }

    fn discover_symbols(
        &self,
        content: &str,
        path: &Path,
    ) -> Result<Vec<DiscoveredSymbol>, ParseError> {
        let mut parser = Parser::new();
        parser
            .set_language(self.language())
            .map_err(|err| ParseError::Other(format!("set_language failed: {err}")))?;
        let tree = parser
            .parse(content, None)
            .ok_or_else(|| ParseError::Syntax {
                path: path.display().to_string(),
                message: "tree-sitter returned no parse tree".to_owned(),
            })?;

        // If root has syntax errors, surface that instead of silently
        // producing a partial index.
        if tree.root_node().has_error() {
            return Err(ParseError::Syntax {
                path: path.display().to_string(),
                message: "syntax errors detected by tree-sitter".to_owned(),
            });
        }

        let query = self.query();
        let bytes = content.as_bytes();
        let aggregates = collect_aggregates(query, &tree, bytes);
        let lines: Vec<&str> = content.lines().collect();
        Ok(aggregates
            .into_iter()
            .filter_map(|agg| aggregate_to_symbol(&agg, bytes, &lines))
            .collect())
    }
}

/// Captures grouped by `@symbol` node. Multiple patterns can match the same
/// node (e.g. top-level fn also matching "local function"). We merge so
/// `@private` and `@kind.method` survive regardless of pattern evaluation
/// order.
struct Aggregate<'a> {
    symbol: Node<'a>,
    name: Option<Node<'a>>,
    params: Option<Node<'a>>,
    kind: SymbolKind,
    is_private: bool,
}

fn collect_aggregates<'tree>(
    query: &Query,
    tree: &'tree tree_sitter::Tree,
    bytes: &[u8],
) -> Vec<Aggregate<'tree>> {
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), bytes);
    let mut by_symbol: HashMap<usize, Aggregate<'tree>> = HashMap::new();

    while let Some(m) = matches.next() {
        let mut symbol_node: Option<Node> = None;
        let mut name_node: Option<Node> = None;
        let mut params_node: Option<Node> = None;
        let mut local_kind: Option<SymbolKind> = None;
        let mut is_private = false;

        for cap in m.captures {
            match query.capture_names()[cap.index as usize] {
                "symbol" => symbol_node = Some(cap.node),
                "name" => name_node = Some(cap.node),
                "params" => params_node = Some(cap.node),
                "kind.fn" => local_kind = Some(SymbolKind::Function),
                "kind.method" => local_kind = Some(SymbolKind::Method),
                "kind.class" => local_kind = Some(SymbolKind::Class),
                "kind.module" => local_kind = Some(SymbolKind::Module),
                "private" => is_private = true,
                _ => {}
            }
        }
        let Some(sym) = symbol_node else { continue };
        let entry = by_symbol.entry(sym.id()).or_insert(Aggregate {
            symbol: sym,
            name: None,
            params: None,
            kind: SymbolKind::Function,
            is_private: false,
        });
        if entry.name.is_none() {
            entry.name = name_node;
        }
        if entry.params.is_none() {
            entry.params = params_node;
        }
        if let Some(k) = local_kind {
            // Method/Class take precedence over Function; otherwise keep the
            // first specialization encountered.
            if matches!(k, SymbolKind::Method | SymbolKind::Class)
                || entry.kind == SymbolKind::Function
            {
                entry.kind = k;
            }
        }
        if is_private {
            entry.is_private = true;
        }
    }

    let mut aggregates: Vec<Aggregate<'tree>> = by_symbol.into_values().collect();
    aggregates.sort_by_key(|a| a.symbol.start_byte());
    aggregates
}

fn aggregate_to_symbol(
    agg: &Aggregate<'_>,
    bytes: &[u8],
    lines: &[&str],
) -> Option<DiscoveredSymbol> {
    let name_text = node_text(agg.name?, bytes).to_owned();
    let fqn = fqn_from_name(&name_text);
    let params = agg
        .params
        .map(|n| extract_params(n, bytes))
        .unwrap_or_default();
    let visibility = if agg.is_private || name_text.starts_with('_') {
        Visibility::Private
    } else {
        Visibility::Public
    };
    let signature = format_signature(&name_text, &params);
    let leading_comment = collect_leading_comment(lines, agg.symbol.start_position().row);
    let source_range = node_to_source_range(agg.symbol);

    Some(DiscoveredSymbol {
        fqn,
        symbol: SymbolInfo {
            kind: agg.kind,
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
        source_range,
        leading_comment,
        leading_comment_line_start: None,
    })
}

fn node_text<'a>(node: Node<'_>, bytes: &'a [u8]) -> &'a str {
    std::str::from_utf8(&bytes[node.byte_range()]).unwrap_or("")
}

/// For Lua names like `foo`, `tbl.bar` or `tbl:method`, produce segmented
/// FQN (`["foo"]`, `["tbl","bar"]`, `["tbl","method"]`).
fn fqn_from_name(name: &str) -> Vec<String> {
    name.split([':', '.'])
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Extract parameter names from a Lua `parameters` node. Types do not exist
/// in Lua, so `type_repr` stays `None`.
fn extract_params(params: Node<'_>, bytes: &[u8]) -> Vec<ParamInfo> {
    let mut out = Vec::new();
    let mut walker = params.walk();
    for child in params.children(&mut walker) {
        if child.kind() == "identifier" {
            out.push(ParamInfo {
                name: node_text(child, bytes).to_owned(),
                type_repr: None,
                default: None,
                is_optional: false,
                is_variadic: false,
            });
        } else if child.kind() == "vararg_expression" {
            out.push(ParamInfo {
                name: "...".to_owned(),
                type_repr: None,
                default: None,
                is_optional: false,
                is_variadic: true,
            });
        }
    }
    out
}

fn format_signature(name: &str, params: &[ParamInfo]) -> String {
    let params_str = params
        .iter()
        .map(|p| p.name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    format!("function {name}({params_str})")
}

/// Walk lines above symbol to recover contiguous comment block. Conventions:
/// - a `---` or `--` line joins the block
/// - a `]]` line (end of `--[[ ... ]]`) starts collection of a multiline
///   block, walking upward until `--[[`
/// - an empty line or code breaks the chain
///
/// Returns concatenated text (without `--`/`---` prefixes), or `None` if
/// nothing was found.
fn collect_leading_comment(lines: &[&str], symbol_row: usize) -> Option<String> {
    if symbol_row == 0 {
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
        if let Some(rest) = trimmed.strip_prefix("---") {
            collected.push(rest.trim_start().to_owned());
        } else if let Some(rest) = trimmed.strip_prefix("--") {
            // `--[[` opens a multiline block, but encountered while walking
            // upward we are missing its end, so skip it.
            // By contrast, plain `--` is a line comment.
            if rest.starts_with("[[") {
                break;
            }
            collected.push(rest.trim_start().to_owned());
        } else if trimmed.starts_with("]]") {
            // End of a multiline block. Collect up to `--[[`.
            let mut block: Vec<String> = Vec::new();
            // Current line may be just `]]` or include text before it.
            let before_close = trimmed.trim_end_matches(']').trim_end();
            if !before_close.is_empty() {
                block.push(before_close.to_owned());
            }
            while i > 0 {
                i -= 1;
                let line = lines.get(i)?;
                let trimmed = line.trim_start();
                if let Some(rest) = trimmed.strip_prefix("--[[") {
                    let cleaned = rest.trim_start();
                    if !cleaned.is_empty() {
                        block.push(cleaned.to_owned());
                    }
                    block.reverse();
                    collected.push(block.join("\n"));
                    break;
                }
                block.push((*line).to_owned());
            }
        } else {
            break;
        }
    }
    if collected.is_empty() {
        None
    } else {
        collected.reverse();
        Some(collected.join("\n").trim().to_owned())
    }
}

fn node_to_source_range(node: Node<'_>) -> SourceRange {
    let start = node.start_position();
    let end = node.end_position();
    SourceRange {
        line_start: u32::try_from(start.row + 1).unwrap_or(1),
        line_end: u32::try_from(end.row + 1).unwrap_or(1),
        column_start: u32::try_from(start.column + 1).unwrap_or(1),
        column_end: u32::try_from(end.column + 1).unwrap_or(1),
    }
}

// -------- Lua query --------
//
// Targeted patterns:
//   1. `function foo(a, b) end`                — top-level fn
//   2. `function tbl.foo(a, b) end`            — table fn (kind.fn)
//   3. `function tbl:foo(self, a, b) end`      — method
//   4. `local function foo(a, b) end`          — private fn (`@private`)
//   5. `local foo = function(a, b) end`        — private fn-as-var
//   6. `tbl.foo = function(a, b) end`          — table assignment to fn
//   7. `local M = {}` / `M = {}`               — module pattern (kind.module)
//
// Capture names are consumed by the `discover_symbols` walker.
// Patterns are deliberately split (no nested alternation) to keep queries
// readable and avoid predicate pitfalls.

const LUA_SYMBOLS_QUERY: &str = r#"
; 1. Top-level: function foo(...) end
(function_declaration
  name: (identifier) @name
  parameters: (parameters) @params
  (#match? @name "^[^.:]+$")) @symbol
@kind.fn

; 2. Dotted: function tbl.foo(...) end
(function_declaration
  name: (dot_index_expression) @name
  parameters: (parameters) @params) @symbol
@kind.fn

; 3. Method: function tbl:foo(...) end
(function_declaration
  name: (method_index_expression) @name
  parameters: (parameters) @params) @symbol
@kind.method

; 4. local function foo(...) end
(function_declaration
  "local" @private
  name: (identifier) @name
  parameters: (parameters) @params) @symbol
@kind.fn

; 5. local foo = function(...) end
(variable_declaration
  (assignment_statement
    (variable_list
      name: (identifier) @name)
    (expression_list
      value: (function_definition
        parameters: (parameters) @params)))) @symbol
@kind.fn
@private

; 6. tbl.foo = function(...) end
(assignment_statement
  (variable_list
    name: (dot_index_expression) @name)
  (expression_list
    value: (function_definition
      parameters: (parameters) @params))) @symbol
@kind.fn

; 7a. local M = {} — module pattern (private = local)
(variable_declaration
  (assignment_statement
    (variable_list
      name: (identifier) @name)
    (expression_list
      value: (table_constructor)))) @symbol
@kind.module
@private

; 7b. M = {} — global module / class table
(assignment_statement
  (variable_list
    name: (identifier) @name)
  (expression_list
    value: (table_constructor))) @symbol
@kind.module
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn discover(src: &str) -> Vec<DiscoveredSymbol> {
        let p = TreeSitterProvider::lua();
        p.discover_symbols(src, Path::new("test.lua")).unwrap()
    }

    #[test]
    fn discovers_top_level_function() {
        let src = "function hello(name)\n  return name\nend\n";
        let symbols = discover(src);
        assert!(symbols.iter().any(|s| s.fqn == vec!["hello"]));
        let hello = symbols.iter().find(|s| s.fqn == vec!["hello"]).unwrap();
        assert_eq!(hello.symbol.kind, SymbolKind::Function);
        assert_eq!(hello.symbol.visibility, Visibility::Public);
        assert_eq!(hello.symbol.params.len(), 1);
        assert_eq!(hello.symbol.params[0].name, "name");
    }

    #[test]
    fn local_function_is_private() {
        let src = "local function helper()\nend\n";
        let symbols = discover(src);
        let h = symbols.iter().find(|s| s.fqn == vec!["helper"]).unwrap();
        assert_eq!(h.symbol.visibility, Visibility::Private);
    }

    #[test]
    fn method_definition() {
        let src = "function Player:attack(target)\nend\n";
        let symbols = discover(src);
        let m = symbols
            .iter()
            .find(|s| s.fqn == vec!["Player", "attack"])
            .unwrap();
        assert_eq!(m.symbol.kind, SymbolKind::Method);
        assert_eq!(m.symbol.params.len(), 1);
        assert_eq!(m.symbol.params[0].name, "target");
    }

    #[test]
    fn dotted_function() {
        let src = "function utils.dump(x)\nend\n";
        let symbols = discover(src);
        let f = symbols
            .iter()
            .find(|s| s.fqn == vec!["utils", "dump"])
            .unwrap();
        assert_eq!(f.symbol.kind, SymbolKind::Function);
    }

    #[test]
    fn collects_leading_line_comments() {
        let src =
            "--- Greets the user.\n--- Returns nothing.\nfunction greet(name)\n  print(name)\nend\n";
        let symbols = discover(src);
        let g = symbols.iter().find(|s| s.fqn == vec!["greet"]).unwrap();
        let comment = g.leading_comment.as_deref().unwrap_or("");
        assert!(comment.contains("Greets the user."));
        assert!(comment.contains("Returns nothing."));
    }

    #[test]
    fn variadic_param_extracted() {
        let src = "function printf(fmt, ...)\nend\n";
        let symbols = discover(src);
        let f = symbols.iter().find(|s| s.fqn == vec!["printf"]).unwrap();
        assert!(f.symbol.params.iter().any(|p| p.is_variadic));
    }

    #[test]
    fn underscore_prefixed_name_is_private() {
        let src = "function _private_helper()\nend\n";
        let symbols = discover(src);
        let h = symbols
            .iter()
            .find(|s| s.fqn == vec!["_private_helper"])
            .unwrap();
        assert_eq!(h.symbol.visibility, Visibility::Private);
    }

    #[test]
    fn signature_format() {
        let src = "function add(a, b)\nend\n";
        let symbols = discover(src);
        let add = symbols.iter().find(|s| s.fqn == vec!["add"]).unwrap();
        assert_eq!(add.symbol.signature, "function add(a, b)");
    }

    #[test]
    fn syntax_error_returns_err() {
        let src = "function broken(\nend\n";
        let p = TreeSitterProvider::lua();
        let res = p.discover_symbols(src, Path::new("bad.lua"));
        assert!(res.is_err());
    }

    #[test]
    fn provider_metadata() {
        let p = TreeSitterProvider::lua();
        assert_eq!(p.id(), "lua");
        assert!(p.extensions().contains(&".lua"));
    }

    // -------- Module pattern --------

    #[test]
    fn local_table_assignment_is_module() {
        let src = "local M = {}\nfunction M.greet(name)\nend\nreturn M\n";
        let symbols = discover(src);
        let m = symbols.iter().find(|s| s.fqn == vec!["M"]).unwrap();
        assert_eq!(m.symbol.kind, SymbolKind::Module);
        // `local` -> private.
        assert_eq!(m.symbol.visibility, Visibility::Private);
    }

    #[test]
    fn global_table_assignment_is_module() {
        let src = "Player = {}\nfunction Player:attack(target)\nend\n";
        let symbols = discover(src);
        let p = symbols.iter().find(|s| s.fqn == vec!["Player"]).unwrap();
        assert_eq!(p.symbol.kind, SymbolKind::Module);
        assert_eq!(p.symbol.visibility, Visibility::Public);
    }

    #[test]
    fn module_and_methods_both_indexed() {
        let src = "local M = {}\nfunction M.foo() end\nfunction M.bar() end\nreturn M\n";
        let symbols = discover(src);
        // 1 module + 2 functions
        assert!(symbols.iter().any(|s| s.fqn == vec!["M"]));
        assert!(symbols.iter().any(|s| s.fqn == vec!["M", "foo"]));
        assert!(symbols.iter().any(|s| s.fqn == vec!["M", "bar"]));
    }

    // -------- Dynamic providers (`.standardoc/languages/*.json`) --------

    #[test]
    fn fork_inherits_base_query_and_runs() {
        // Fork = no override, just a new id + ext that reuses the Lua
        // query as-is. The provider must work just like Lua.
        let def = LanguageDef {
            id: "teal".to_owned(),
            extensions: vec![".tl".to_owned()],
            comment_styles: CommentStyles::default(),
            backend: LanguageBackend::TreeSitterFork {
                base: "lua".to_owned(),
                query: None,
                extra_patterns: None,
            },
        };
        let provider = TreeSitterProvider::from_lang_def(&def).expect("valid def");
        assert_eq!(provider.id(), "teal");
        let symbols = provider
            .discover_symbols("function hello(x) end\n", Path::new("test.tl"))
            .unwrap();
        assert!(symbols.iter().any(|s| s.fqn == vec!["hello"]));
    }

    #[test]
    fn fork_with_extra_patterns_compiles() {
        // Verifies we can append a pattern to the Lua query without
        // breaking it. Pattern: we capture `(comment)` as `@ignore` —
        // not a useful query, but valid tree-sitter for the test.
        let def = LanguageDef {
            id: "luax".to_owned(),
            extensions: vec![".luax".to_owned()],
            comment_styles: CommentStyles::default(),
            backend: LanguageBackend::TreeSitterFork {
                base: "lua".to_owned(),
                query: None,
                extra_patterns: Some("(comment) @ignore".to_owned()),
            },
        };
        let provider = TreeSitterProvider::from_lang_def(&def).expect("query must compile");
        // Base Lua functions still work.
        let symbols = provider
            .discover_symbols("function hello() end\n", Path::new("test.luax"))
            .unwrap();
        assert!(symbols.iter().any(|s| s.fqn == vec!["hello"]));
    }

    #[test]
    fn unknown_base_grammar_errors() {
        let def = LanguageDef {
            id: "gibberish".to_owned(),
            extensions: vec![".x".to_owned()],
            comment_styles: CommentStyles::default(),
            backend: LanguageBackend::TreeSitterFork {
                base: "klingon".to_owned(),
                query: None,
                extra_patterns: None,
            },
        };
        let err = TreeSitterProvider::from_lang_def(&def).unwrap_err();
        assert!(matches!(err, FromDefError::UnknownBase(_)));
    }

    #[test]
    fn invalid_query_errors_at_construction() {
        // Syntactically broken `extra_patterns` → immediate error, no
        // panic on first use.
        let def = LanguageDef {
            id: "broken".to_owned(),
            extensions: vec![".x".to_owned()],
            comment_styles: CommentStyles::default(),
            backend: LanguageBackend::TreeSitterFork {
                base: "lua".to_owned(),
                query: None,
                extra_patterns: Some("((( this is not valid".to_owned()),
            },
        };
        let err = TreeSitterProvider::from_lang_def(&def).unwrap_err();
        assert!(matches!(err, FromDefError::InvalidQuery { .. }));
    }
}

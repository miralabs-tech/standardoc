use crate::model::CommentStyles;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Standard config filename searched at workspace root.
pub const CONFIG_FILE: &str = ".standardoc.json";

/// Top-level standardoc configuration loaded from `.standardoc.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    /// Annotation tag name (default: `"doc"`).
    pub doc_tag: String,
    /// Tag whose mere presence in a comment excludes block from index.
    /// Default: `"hide"` -> user writes `@hide` above a symbol to remove it
    /// from public docs. Customizable for projects wanting their own
    /// convention (`@internal`, etc.).
    pub hide_tag: String,
    /// Schema version of the config file.
    pub version: u32,
    /// Per-language comment patterns. Keys are file extensions or language ids.
    pub languages: BTreeMap<String, CommentStyles>,
    pub transform: TransformConfig,
    /// Custom tag schemas for validation.
    pub tags: BTreeMap<String, TagSchema>,
    /// Per-rule severity overrides. Keys are diagnostic codes (e.g. `"STD001"`).
    pub rules: BTreeMap<String, RuleOverride>,
    pub ai: AiConfig,
    pub mcp: McpConfig,
    pub discovery: DiscoveryConfig,
    pub watch: WatchConfig,
    pub source: SourceConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            doc_tag: "doc".to_owned(),
            hide_tag: "hide".to_owned(),
            version: 2,
            languages: BTreeMap::new(),
            transform: TransformConfig::default(),
            tags: BTreeMap::new(),
            rules: BTreeMap::new(),
            ai: AiConfig::default(),
            mcp: McpConfig::default(),
            discovery: DiscoveryConfig::default(),
            watch: WatchConfig::default(),
            source: SourceConfig::default(),
        }
    }
}

impl Config {
    /// Search `.standardoc.json` at workspace root and deserialize it.
    /// Returns:
    /// - `Ok(config)` if file exists and parses
    /// - `Ok(Config::default())` if file does not exist (common case)
    /// - `Err(...)` if file exists but is invalid (visible error instead of
    ///   silent fallback)
    pub fn load_from_workspace(workspace: &Path) -> Result<Self, ConfigError> {
        let path = workspace.join(CONFIG_FILE);
        match std::fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content)
                .map_err(|err| ConfigError::Parse { path, source: err }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(ConfigError::Read { path, source: err }),
        }
    }

    /// Best-effort variant: log error to stderr then fall back to defaults.
    /// Used by CLI/MCP surfaces where serving index is preferred over crashing
    /// on malformed file.
    #[must_use]
    pub fn load_from_workspace_or_default(workspace: &Path) -> Self {
        match Self::load_from_workspace(workspace) {
            Ok(c) => c,
            Err(err) => {
                eprintln!("standardoc: {err} — falling back to default config");
                Self::default()
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config at {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse config at {path}: {source}")]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
}

/// Filesystem watcher and auto-pause settings.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WatchConfig {
    /// If `false`, no watcher starts at server boot. Pipeline remains usable
    /// via manual `rescan`.
    pub enabled: bool,
    /// Debounce window used by `notify-debouncer-full`.
    pub debounce_ms: u64,
    /// When auto-pause heuristic is active, watcher freezes after this number
    /// of parse errors on same file within `auto_pause_window_ms`. Set `0`
    /// to disable auto-pause.
    pub auto_pause_parse_errors: u32,
    /// Sliding window (milliseconds) for parse error counting.
    pub auto_pause_window_ms: u64,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            debounce_ms: 100,
            auto_pause_parse_errors: 3,
            auto_pause_window_ms: 10_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TransformConfig {
    pub entry: Option<PathBuf>,
    pub output: Option<PathBuf>,
    /// Ordered list of transform passes, e.g. `["dsl", "ai-enrich"]`.
    pub passes: Vec<String>,
}

impl Default for TransformConfig {
    fn default() -> Self {
        Self {
            entry: None,
            output: None,
            passes: vec!["dsl".to_owned()],
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TagSchema {
    pub fields: Vec<String>,
    pub required: Vec<String>,
    /// How many occurrences of this tag can legitimately appear on the same block.
    ///
    /// `Single` (default): DSL allows shortcut `:tag.field` because there is
    /// no ambiguity. `description`, `returns`, `since`, etc.
    /// `Multi`: `:tag.field` shortcut becomes an explicit error — use
    /// `:tag[0].field`, `:first(tag).field`, or `each`.
    /// `param`, `example`, `see`, etc.
    #[serde(default)]
    pub cardinality: TagCardinality,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TagCardinality {
    /// Single expected occurrence. Shortcut `:tag.field` reads that single one.
    #[default]
    Single,
    /// Multiple occurrences allowed. `:tag.field` shortcut is forbidden
    /// (ambiguous) — user must choose `[n]`, `first()`, `last()`, or `each`.
    Multi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleOverride {
    Off,
    Hint,
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AiConfig {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub write_policy: WritePolicy,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WritePolicy {
    #[default]
    DiffPreview,
    Direct,
    ReadOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct McpConfig {
    /// Allowed tool names. `["*"]` means expose every tool.
    pub exposed_tools: Vec<String>,
    pub rate_limit: Option<RateLimit>,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            exposed_tools: vec!["*".to_owned()],
            rate_limit: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimit {
    pub per_minute: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DiscoveryConfig {
    pub mode: DiscoveryMode,
    pub include: SymbolInclusion,
    /// A `@doc` annotation on a private symbol forces it to be indexed anyway.
    pub include_private_with_doc: bool,
    /// Languages where AST discovery is enabled. Others fall back to annotation-only.
    pub ast_languages: Vec<String>,
    pub key_strategy: KeyStrategy,
    /// List of key patterns to exclude from index, in addition to source-level
    /// `@hide` tag. Format:
    ///
    /// - `"matchigo.bench.foo"` — exact key match
    /// - `"matchigo.bench.*"` — strict descendants (`matchigo.bench.` prefix)
    /// - `"matchigo.p.P*"` — string prefix (matches `PAny`, `PArray`, ...)
    ///
    /// Combine to hide a full module *and* its parent node:
    /// `["matchigo.bench", "matchigo.bench.*"]`.
    ///
    /// Side-by-side with `@hide`:
    /// - `@hide` = source itself is marked -> portable, lives with code
    /// - `exclude` = external rule -> useful when documenting code you cannot
    ///   or do not want to modify, or for web admin mode
    pub exclude: Vec<String>,
    /// Gitignore-style patterns applied during scanner walk. Different from
    /// `exclude` (which filters by `DocKey` after scan): this prevents opening
    /// the file at all.
    ///
    /// Exemples :
    /// - `"node_modules/"` — skip this directory anywhere in tree
    /// - `"**/*.generated.ts"` — skip a file pattern
    /// - `"!src/keep.ts"` — re-include a file excluded by a parent pattern
    ///
    /// Automatically combined with repo `.gitignore` files and custom
    /// `.stdocignore`. Team `.gitignore` rules are already respected by
    /// default — only add Standardoc-specific patterns here.
    pub exclude_files: Vec<String>,
    /// If `false`, scanner ignores `.gitignore` and only uses
    /// `.stdocignore` + `exclude_files`. Default `true`.
    pub respect_gitignore: bool,
    /// How aggressively to synthesize virtual `@doc` annotations from AST +
    /// heuristics on symbols that lack real annotations. See
    /// [`VirtualAnnotationsLevel`] for tier semantics. Default `Medium` —
    /// covers the public surface with high-confidence templates and
    /// param/return narratives.
    #[serde(default)]
    pub virtual_annotations: VirtualAnnotationsLevel,
}

/// Aggressiveness of the virtual annotation pass.
///
/// - `Off`: skip entirely. Useful in CI where you want only real `@doc`.
/// - `Low`: only public symbols, only highest-confidence signature templates
///   (`new`, `is_*`, `len`, `is_empty`, trait impls).
/// - `Medium` (default): `Low` + verb-prefix conventions + param-name
///   hints + return-type narrative.
/// - `High`: `Medium` + crate/internal visibility + module-path
///   categorization.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VirtualAnnotationsLevel {
    Off,
    Low,
    #[default]
    Medium,
    High,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            mode: DiscoveryMode::default(),
            include: SymbolInclusion::default(),
            include_private_with_doc: true,
            ast_languages: Vec::new(),
            key_strategy: KeyStrategy::default(),
            exclude: Vec::new(),
            exclude_files: Vec::new(),
            respect_gitignore: true,
            virtual_annotations: VirtualAnnotationsLevel::default(),
        }
    }
}

/// Returns `true` if `key` matches one exclusion-list pattern.
/// See [`DiscoveryConfig::exclude`] for pattern syntax.
#[must_use]
pub fn key_matches_any_exclude(key: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|p| key_matches_exclude_pattern(key, p))
}

fn key_matches_exclude_pattern(key: &str, pattern: &str) -> bool {
    pattern
        .strip_suffix('*')
        .map_or_else(|| key == pattern, |prefix| key.starts_with(prefix))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiscoveryMode {
    Ast,
    Annotation,
    #[default]
    Hybrid,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SymbolInclusion {
    #[default]
    Public,
    All,
    AnnotatedOnly,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KeyStrategy {
    #[default]
    Fqn,
    NameOnly,
    PathBased,
}

/// How footer "Source: src/match.ts:32" should open file.
///
/// Three targets:
/// - **VSCode**: opens `vscode://file/<absolute-path>:<line>` — works locally
///   in daemon mode docs.
/// - **GitHub**: opens `https://github.com/<repo>/blob/<branch>/<path>#L<line>`
///   — natural target for static export deployed on CDN.
/// - **SourceView**: collapsible panel embedded in reference page,
///   syntect-highlighted, targeted line, no external dependency.
///
/// `mode: "auto"` (default) resolves at server boot: daemon -> vscode,
/// static export -> github (if configured) otherwise source-view.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SourceConfig {
    pub mode: SourceMode,
    pub github: Option<GithubSource>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceMode {
    #[default]
    Auto,
    Vscode,
    Github,
    SourceView,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubSource {
    /// Format `owner/repo`, e.g. `wesleycormier/matchigo`.
    pub repo: String,
    /// Default `"main"` if missing.
    #[serde(default = "default_github_branch")]
    pub branch: String,
}

fn default_github_branch() -> String {
    "main".to_owned()
}

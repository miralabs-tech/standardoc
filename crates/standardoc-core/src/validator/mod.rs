//! Validation rules → `Vec<Diagnostic>`.
//!
//! We aim for a tight set of rules **useful, low-noise, easy to explain**
//! rather than an exhaustive linter. Each rule can be overridden (off,
//! hint→error, etc.) via `Config::rules` (key = STD code).
//!
//! Source of truth: each `STDxxx` `RuleSpec` const declares the code +
//! default severity in one place; rule fns reference the const. The
//! `@doc validator.rules.stdXXX` annotation lives on the rule fn — or on
//! the const itself for STD002 / STD013, which are emitted by the
//! extractor and have no rule fn here.
//!
//! Not yet shipped (needs extra plumbing or too noisy by default):
//! - STD010: key drift (requires a keys.lock)
//! - STD015: malformed satellite (`@doc-extend` with fewer than two args).
//!   Requires propagating extraction-time diagnostics through
//!   `PipelineReport` and the 4 `validate()` call sites — deferred.

use crate::config::{Config, RuleOverride};
use crate::dsl::parser as dsl_parser;
use crate::model::{
    BlockOrigin, Diagnostic, DiagnosticCode, DocBlock, ParamInfo, Severity, SourceRange, Visibility,
};
use crate::pages::DocPage;
use crate::pipeline::KeyCollision;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Per-rule spec: stable code + default severity. The config may override
/// the severity at runtime via `Config::rules` (`Off` fully disables).
pub struct RuleSpec {
    pub code: &'static str,
    pub severity: Severity,
}

pub const STD001: RuleSpec = RuleSpec {
    code: "STD001",
    severity: Severity::Error,
};

/// @doc validator.rules.std002
/// @code STD002
/// @severity Warning
/// @description Malformed `@tag` (e.g. `@doc` with no key, `@param` with no name). Emitted by the extractor while parsing annotations.
pub const STD002: RuleSpec = RuleSpec {
    code: "STD002",
    severity: Severity::Warning,
};

pub const STD003: RuleSpec = RuleSpec {
    code: "STD003",
    severity: Severity::Warning,
};

pub const STD004: RuleSpec = RuleSpec {
    code: "STD004",
    severity: Severity::Warning,
};

pub const STD005: RuleSpec = RuleSpec {
    code: "STD005",
    severity: Severity::Info,
};

pub const STD006: RuleSpec = RuleSpec {
    code: "STD006",
    severity: Severity::Hint,
};

pub const STD007: RuleSpec = RuleSpec {
    code: "STD007",
    severity: Severity::Error,
};

pub const STD008: RuleSpec = RuleSpec {
    code: "STD008",
    severity: Severity::Warning,
};

pub const STD012: RuleSpec = RuleSpec {
    code: "STD012",
    severity: Severity::Warning,
};

/// @doc validator.rules.std013
/// @code STD013
/// @severity Hint
/// @description Explicit `@doc K` is redundant when `K` matches the FQN-inferred key. Emitted by the extractor.
pub const STD013: RuleSpec = RuleSpec {
    code: "STD013",
    severity: Severity::Hint,
};

pub const STD014: RuleSpec = RuleSpec {
    code: "STD014",
    severity: Severity::Warning,
};

/// Runs every rule against the current index.
///
/// `collisions` come from the pipeline: we pass them explicitly rather
/// than recomputing them here. If you don't have any (e.g. tests), pass
/// `&[]`.
///
/// `pages` is the set of narrative `.md` pages in the workspace (from
/// `state.index().pages`). Empty or `BTreeMap::new()` disables STD004 /
/// STD007 — useful for tests that focus on per-block rules.
pub fn validate(
    blocks: &BTreeMap<String, DocBlock>,
    collisions: &[KeyCollision],
    pages: &BTreeMap<String, DocPage>,
    config: &Config,
) -> Vec<Diagnostic> {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();

    rule_std001_dup_keys(collisions, &mut diagnostics);

    for block in blocks.values() {
        // STD002: collect the diagnostics the extractor already attached
        // to the block during the `@tag` parse (`@doc` no key, `@param`
        // no name, etc.). No logic here — the STDxxx code is already set.
        diagnostics.extend(block.diagnostics.iter().cloned());

        rule_std003_param_missing_description(block, &mut diagnostics);
        rule_std005_block_without_description(block, &mut diagnostics);
        rule_std006_public_inferred_undocumented(block, &mut diagnostics);
        rule_std008_param_name_not_in_signature(block, &mut diagnostics);
        rule_std012_param_type_mismatch(block, &mut diagnostics);
        rule_std014_orphan_satellite(block, blocks, &mut diagnostics);
    }

    // Per-page rules: every pass walks the pages only once to avoid
    // tokenizing the markdown N times. Graceful fallback when `pages`
    // is empty (cf. per-block tests) — the loop doesn't run.
    if !pages.is_empty() {
        let known_keys = build_known_key_set(blocks);
        for page in pages.values() {
            rule_std004_unknown_key_ref(page, &known_keys, &mut diagnostics);
            rule_std007_invalid_dsl_syntax(page, &mut diagnostics);
        }
    }

    apply_overrides(&mut diagnostics, &config.rules);
    diagnostics.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then(a.range.line_start.cmp(&b.range.line_start))
            .then(a.code.as_str().cmp(b.code.as_str()))
    });
    diagnostics
}

/// Precomputes the set of recognized keys (FQN + short name of each key)
/// for O(1) lookup during DSL reference validation. We accept the short
/// name because the DSL allows it for brevity.
fn build_known_key_set(blocks: &BTreeMap<String, DocBlock>) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    for key in blocks.keys() {
        set.insert(key.clone());
        if let Some((_, short)) = key.rsplit_once('.') {
            set.insert(short.to_owned());
        }
    }
    set
}

fn apply_overrides(
    diagnostics: &mut Vec<Diagnostic>,
    rule_overrides: &BTreeMap<String, RuleOverride>,
) {
    diagnostics.retain_mut(|d| {
        let Some(rule_override) = rule_overrides.get(d.code.as_str()) else {
            return true;
        };
        match rule_override {
            RuleOverride::Off => false,
            RuleOverride::Hint => {
                d.severity = Severity::Hint;
                true
            }
            RuleOverride::Info => {
                d.severity = Severity::Info;
                true
            }
            RuleOverride::Warning => {
                d.severity = Severity::Warning;
                true
            }
            RuleOverride::Error => {
                d.severity = Severity::Error;
                true
            }
        }
    });
}

fn make_diagnostic(spec: &RuleSpec, message: String, block: &DocBlock) -> Diagnostic {
    Diagnostic {
        code: DiagnosticCode::new(spec.code),
        severity: spec.severity,
        message,
        path: block.meta.path.clone(),
        range: SourceRange {
            line_start: block.meta.line_start,
            line_end: block.meta.line_end,
            column_start: block.meta.column,
            column_end: block.meta.column,
        },
        related: Vec::new(),
    }
}

// -------- Rules --------

/// @doc validator.rules.std001
/// @code STD001
/// @severity Error
/// @description Duplicate DocKey: two annotated items declared the same key.
fn rule_std001_dup_keys(collisions: &[KeyCollision], out: &mut Vec<Diagnostic>) {
    for collision in collisions {
        let dropped_locations: String = collision
            .dropped
            .iter()
            .map(|p| format!("{}:{}", p.path.display(), p.line))
            .collect::<Vec<_>>()
            .join(", ");
        out.push(Diagnostic {
            code: DiagnosticCode::new(STD001.code),
            severity: STD001.severity,
            message: format!(
                "duplicate DocKey '{}' — kept {}:{}, dropped: {dropped_locations}",
                collision.key,
                collision.kept.path.display(),
                collision.kept.line,
            ),
            path: collision.kept.path.clone(),
            range: SourceRange {
                line_start: collision.kept.line,
                line_end: collision.kept.line,
                column_start: 1,
                column_end: 1,
            },
            related: Vec::new(),
        });
    }
}

/// @doc validator.rules.std003
/// @code STD003
/// @severity Warning
/// @description `@param` is missing a description (the 3rd schema field).
fn rule_std003_param_missing_description(block: &DocBlock, out: &mut Vec<Diagnostic>) {
    let Some(params) = block.tags.get("param") else {
        return;
    };
    for occurrence in params {
        // Schema: [name, type, description]. Require all 3 non-empty fields.
        let name = occurrence.first().map_or("", String::as_str);
        let has_description = occurrence.get(2).is_some_and(|s| !s.trim().is_empty());
        if !name.is_empty() && !has_description {
            out.push(make_diagnostic(
                &STD003,
                format!("@param '{name}' is missing a description"),
                block,
            ));
        }
    }
}

/// @doc validator.rules.std005
/// @code STD005
/// @severity Info
/// @description Block has no description: no `@description`, and no leading prose was promoted to one.
fn rule_std005_block_without_description(block: &DocBlock, out: &mut Vec<Diagnostic>) {
    // A description can come from an explicit tag OR be implicit (prose
    // before first `@tag`, converted to automatic `@description` by
    // extractor). So we only check whether map contains an entry.
    let has_description = block
        .tags
        .get("description")
        .and_then(|v| v.first())
        .and_then(|fields| fields.first())
        .is_some_and(|s| !s.trim().is_empty());
    if !has_description {
        out.push(make_diagnostic(
            &STD005,
            format!("block '{}' has no description", block.key.as_str()),
            block,
        ));
    }
}

/// @doc validator.rules.std006
/// @code STD006
/// @severity Hint
/// @description Public (or `pub(crate)`) symbol has no `@doc` annotation. Inferred-only blocks trigger this.
fn rule_std006_public_inferred_undocumented(block: &DocBlock, out: &mut Vec<Diagnostic>) {
    let Some(symbol) = &block.symbol else {
        return;
    };
    if !matches!(symbol.visibility, Visibility::Public | Visibility::Crate) {
        return;
    }
    if block.origin != BlockOrigin::Inferred {
        return;
    }
    // Skip if block has at least one description (free prose in a regular
    // doc-comment without explicit `@doc`).
    let has_any_doc = !block.tags.is_empty();
    if has_any_doc {
        return;
    }
    // If the virtual annotation pass produced a description, surface it in the
    // hint message so editors / agents have something actionable to show
    // without an extra MCP roundtrip. The user can accept it via the future
    // `standardoc materialize` command (or just copy the text).
    let suggestion = block
        .virtual_tags
        .get("description")
        .and_then(|entries| entries.first())
        .and_then(|fields| fields.first())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let message = match suggestion {
        Some(text) => format!(
            "public symbol '{}' has no @doc annotation (suggested: \"{text}\")",
            block.key.as_str()
        ),
        None => format!(
            "public symbol '{}' has no @doc annotation",
            block.key.as_str()
        ),
    };
    out.push(make_diagnostic(&STD006, message, block));
}

/// @doc validator.rules.std008
/// @code STD008
/// @severity Warning
/// @description `@param` names a parameter that does not exist in the signature.
fn rule_std008_param_name_not_in_signature(block: &DocBlock, out: &mut Vec<Diagnostic>) {
    let Some(symbol) = &block.symbol else {
        return;
    };
    let Some(params_tag) = block.tags.get("param") else {
        return;
    };
    let sig_names: Vec<&str> = symbol.params.iter().map(|p| p.name.as_str()).collect();
    for occurrence in params_tag {
        let Some(name) = occurrence.first() else {
            continue;
        };
        if name.is_empty() || sig_names.contains(&name.as_str()) {
            continue;
        }
        out.push(make_diagnostic(
            &STD008,
            format!(
                "@param '{name}' does not match any parameter in the signature \
                 (expected one of: {})",
                sig_names.join(", ")
            ),
            block,
        ));
    }
}

/// @doc validator.rules.std012
/// @code STD012
/// @severity Warning
/// @description `@param` documented type does not match the signature type (tolerant: short-name inclusion accepted).
fn rule_std012_param_type_mismatch(block: &DocBlock, out: &mut Vec<Diagnostic>) {
    let Some(symbol) = &block.symbol else {
        return;
    };
    let Some(params_tag) = block.tags.get("param") else {
        return;
    };
    let by_name: BTreeMap<&str, &ParamInfo> =
        symbol.params.iter().map(|p| (p.name.as_str(), p)).collect();
    for occurrence in params_tag {
        let name = occurrence.first().map_or("", String::as_str);
        let documented_type = occurrence.get(1).map_or("", String::as_str);
        if name.is_empty() || documented_type.is_empty() {
            continue;
        }
        let Some(param) = by_name.get(name) else {
            continue; // Already covered by STD008.
        };
        let Some(actual_type) = &param.type_repr else {
            continue;
        };
        if !types_compatible(documented_type, actual_type) {
            out.push(make_diagnostic(
                &STD012,
                format!(
                    "@param '{name}' documented as `{documented_type}` but signature says `{actual_type}`"
                ),
                block,
            ));
        }
    }
}

/// @doc validator.rules.std014
/// @code STD014
/// @severity Warning
/// @description Satellite (`K::NAME`) references a missing anchor `K`. Define `@doc K` somewhere or fix the `@doc-extend` target.
fn rule_std014_orphan_satellite(
    block: &DocBlock,
    blocks: &BTreeMap<String, DocBlock>,
    out: &mut Vec<Diagnostic>,
) {
    let key = block.key.as_str();
    let Some(sep_idx) = key.find("::") else {
        return;
    };
    // Satellite keys are built via `format!("{anchor}::{ext}")` after
    // whitespace-tokenizing the `@doc-extend` directive — no spaces ever
    // surround the separator. Inferred Rust trait-impl labels, on the other
    // hand, always render the path syntax as ` :: ` with padding (e.g.
    // `<RegexProvider as std :: fmt :: Debug>`). Reject space-padded
    // separators so STD014 does not flag those as orphan satellites.
    let before_padded = key[..sep_idx].ends_with(' ');
    let after_padded = key[sep_idx + 2..].starts_with(' ');
    if before_padded || after_padded {
        return;
    }
    let anchor_key = &key[..sep_idx];
    if blocks.contains_key(anchor_key) {
        return;
    }
    out.push(make_diagnostic(
        &STD014,
        format!(
            "satellite '{key}' references missing anchor '{anchor_key}' — define `@doc {anchor_key}` somewhere or fix the `@doc-extend` target"
        ),
        block,
    ));
}

/// Tolerant comparison between documented type and AST type. Notation
/// conventions differ (`i32` vs `&i32`, `String` vs `&str`...) — accept
/// literal equality OR short-name inclusion.
fn types_compatible(documented: &str, actual: &str) -> bool {
    let d = documented.trim();
    let a = actual.trim();
    if d == a {
        return true;
    }
    // Accept e.g. documented `Foo` and actual `&Foo` / `Box<Foo>` / `Foo<T>`.
    a.contains(d)
}

/// A DSL reference spotted in a page: `@doc.KEY` or `@docs.module(KEY)`.
/// The position is in (line, column) of the `raw_body` — note that we
/// don't yet have the frontmatter offset, so positions are relative to
/// the start of the body, not the full file.
struct DslRef<'a> {
    key: &'a str,
    line: u32,
    column: u32,
}

/// Walks a page's `raw_body` and collects every `@doc.KEY` /
/// `@docs.module(KEY)`. Textual lexer, enough for ref validation — no
/// need for the full DSL parser (that's STD007's job for syntax).
fn collect_dsl_key_refs(content: &str) -> Vec<DslRef<'_>> {
    let mut refs = Vec::new();
    for (line_idx, line) in content.lines().enumerate() {
        let line_u32 = u32::try_from(line_idx + 1).unwrap_or(u32::MAX);
        // Pattern `@doc.KEY` (direct reference).
        let mut search_from = 0;
        while let Some(rel) = line[search_from..].find("@doc.") {
            let absolute = search_from + rel;
            let key_start = absolute + "@doc.".len();
            let after = &line[key_start..];
            let end = after
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '_')
                .unwrap_or(after.len());
            if end > 0 {
                let key = &after[..end];
                let column = u32::try_from(key_start + 1).unwrap_or(u32::MAX);
                refs.push(DslRef {
                    key,
                    line: line_u32,
                    column,
                });
            }
            search_from = key_start + end.max(1);
        }
        // Pattern `@docs.module(KEY)` (iterating over a module's blocks).
        let mut search_from = 0;
        while let Some(rel) = line[search_from..].find("@docs.module(") {
            let absolute = search_from + rel;
            let key_start = absolute + "@docs.module(".len();
            let after = &line[key_start..];
            let Some(end) = after.find(')') else {
                search_from = key_start;
                continue;
            };
            let raw = after[..end].trim();
            if !raw.is_empty() {
                let column = u32::try_from(key_start + 1).unwrap_or(u32::MAX);
                refs.push(DslRef {
                    key: raw,
                    line: line_u32,
                    column,
                });
            }
            search_from = key_start + end + 1;
        }
    }
    refs
}

/// @doc validator.rules.std004
/// @code STD004
/// @severity Warning
/// @description Page DSL reference (`@doc.KEY` / `@docs.module(KEY)`) targets a key not present in the index.
fn rule_std004_unknown_key_ref(
    page: &DocPage,
    known_keys: &BTreeSet<String>,
    out: &mut Vec<Diagnostic>,
) {
    for r in collect_dsl_key_refs(&page.raw_body) {
        if known_keys.contains(r.key) {
            continue;
        }
        // Tolerate suffix-match: `@doc.foo.bar.baz` can match if there's
        // a key ending in `.foo.bar.baz` even without an exact match.
        // Avoids false positives when the user writes a partial FQN (very
        // common).
        let dotted_suffix = format!(".{}", r.key);
        let matches_suffix = known_keys
            .iter()
            .any(|k| k.ends_with(&dotted_suffix) || k == r.key);
        if matches_suffix {
            continue;
        }
        out.push(make_page_diagnostic(
            &STD004,
            format!(
                "DSL reference '@doc.{}' targets a DocKey that doesn't exist in the index",
                r.key
            ),
            &page.path,
            r.line,
            r.column,
        ));
    }
}

/// @doc validator.rules.std007
/// @code STD007
/// @severity Error
/// @description Page contains a `{{ ... }}` expression that fails to parse as DSL.
fn rule_std007_invalid_dsl_syntax(page: &DocPage, out: &mut Vec<Diagnostic>) {
    // Quick skip: if the page contains no `{{`, there's no DSL to
    // validate. Avoids invoking the parser on hundreds of markdown files
    // without any dynamic expression.
    if !page.raw_body.contains("{{") {
        return;
    }
    let Err(err) = dsl_parser::parse(&page.raw_body) else {
        return;
    };
    out.push(make_page_diagnostic(
        &STD007,
        format!("invalid DSL syntax: {err}"),
        &page.path,
        1,
        1,
    ));
}

/// Builds a Diagnostic for a narrative page (vs a source block). We
/// don't have a `DocBlock` here so we keep it minimal: path, line,
/// column. The range is point-like (line/column same position in
/// start/end).
fn make_page_diagnostic(
    spec: &RuleSpec,
    message: String,
    path: &Path,
    line: u32,
    column: u32,
) -> Diagnostic {
    Diagnostic {
        code: DiagnosticCode::new(spec.code),
        severity: spec.severity,
        message,
        path: path.to_path_buf(),
        range: SourceRange {
            line_start: line,
            line_end: line,
            column_start: column,
            column_end: column,
        },
        related: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CommentStyle, DocKey, DocMeta, References, SymbolInfo, SymbolKind};
    use crate::pipeline::PathLine;
    use std::path::PathBuf;

    fn block_with(
        key: &str,
        tags: BTreeMap<String, Vec<Vec<String>>>,
        origin: BlockOrigin,
        visibility: Visibility,
        params: Vec<ParamInfo>,
    ) -> DocBlock {
        DocBlock {
            key: DocKey::new(key),
            label: key.to_owned(),
            origin,
            tags,
            symbol: Some(SymbolInfo {
                kind: SymbolKind::Function,
                visibility,
                signature: format!("fn {key}"),
                params,
                returns: None,
                generics: vec![],
                decorators: vec![],
                is_async: false,
                is_deprecated: false,
                references: References::default(),
            }),
            meta: DocMeta {
                path: PathBuf::from("src/lib.rs"),
                line_start: 1,
                line_end: 1,
                column: 1,
                file_ext: "rs".to_owned(),
                comment_style: CommentStyle::DocSingle,
                last_indexed: 0,
                source_mtime: 0,
            },
            body_hash: 0,
            diagnostics: vec![],
            virtual_tags: BTreeMap::new(),
            virtual_confidence: None,
            virtual_sources: Vec::new(),
        }
    }

    fn empty_blocks() -> BTreeMap<String, DocBlock> {
        BTreeMap::new()
    }

    fn empty_pages() -> BTreeMap<String, DocPage> {
        BTreeMap::new()
    }

    #[test]
    fn std001_emits_one_diagnostic_per_collision() {
        let collision = KeyCollision {
            key: "math.add".to_owned(),
            kept: PathLine {
                path: PathBuf::from("a.rs"),
                line: 10,
            },
            dropped: vec![PathLine {
                path: PathBuf::from("b.rs"),
                line: 20,
            }],
        };
        let diagnostics = validate(
            &empty_blocks(),
            &[collision],
            &empty_pages(),
            &Config::default(),
        );
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code.as_str(), "STD001");
        assert_eq!(diagnostics[0].severity, Severity::Error);
        assert!(diagnostics[0]
            .message
            .contains("duplicate DocKey 'math.add'"));
        assert!(diagnostics[0].message.contains("dropped: b.rs:20"));
    }

    #[test]
    fn std003_param_missing_description() {
        let mut tags = BTreeMap::new();
        tags.insert(
            "param".to_owned(),
            vec![
                vec!["a".to_owned(), "i32".to_owned(), "first".to_owned()],
                vec!["b".to_owned(), "i32".to_owned()], // missing description
            ],
        );
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "f".to_owned(),
            block_with(
                "f",
                tags,
                BlockOrigin::Annotated,
                Visibility::Public,
                vec![],
            ),
        );
        let diags = validate(&blocks, &[], &empty_pages(), &Config::default());
        let std003: Vec<&Diagnostic> = diags
            .iter()
            .filter(|d| d.code.as_str() == "STD003")
            .collect();
        assert_eq!(std003.len(), 1);
        assert!(std003[0].message.contains("'b'"));
    }

    #[test]
    fn std005_no_description() {
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "f".to_owned(),
            block_with(
                "f",
                BTreeMap::new(),
                BlockOrigin::Annotated,
                Visibility::Public,
                vec![],
            ),
        );
        let diags = validate(&blocks, &[], &empty_pages(), &Config::default());
        assert!(diags.iter().any(|d| d.code.as_str() == "STD005"));
    }

    #[test]
    fn std005_does_not_fire_when_description_present() {
        let mut tags = BTreeMap::new();
        tags.insert(
            "description".to_owned(),
            vec![vec!["does the thing".to_owned()]],
        );
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "f".to_owned(),
            block_with(
                "f",
                tags,
                BlockOrigin::Annotated,
                Visibility::Public,
                vec![],
            ),
        );
        let diags = validate(&blocks, &[], &empty_pages(), &Config::default());
        assert!(!diags.iter().any(|d| d.code.as_str() == "STD005"));
    }

    #[test]
    fn std006_public_inferred_without_doc() {
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "f".to_owned(),
            block_with(
                "f",
                BTreeMap::new(),
                BlockOrigin::Inferred,
                Visibility::Public,
                vec![],
            ),
        );
        let diags = validate(&blocks, &[], &empty_pages(), &Config::default());
        assert!(diags.iter().any(|d| d.code.as_str() == "STD006"));
    }

    #[test]
    fn std006_skips_private_symbols() {
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "f".to_owned(),
            block_with(
                "f",
                BTreeMap::new(),
                BlockOrigin::Inferred,
                Visibility::Private,
                vec![],
            ),
        );
        let diags = validate(&blocks, &[], &empty_pages(), &Config::default());
        assert!(!diags.iter().any(|d| d.code.as_str() == "STD006"));
    }

    #[test]
    fn std008_param_name_not_in_signature() {
        let mut tags = BTreeMap::new();
        tags.insert(
            "param".to_owned(),
            vec![vec!["typo".to_owned(), "i32".to_owned(), "?".to_owned()]],
        );
        let params = vec![ParamInfo {
            name: "actual".to_owned(),
            type_repr: Some("i32".to_owned()),
            default: None,
            is_optional: false,
            is_variadic: false,
        }];
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "f".to_owned(),
            block_with(
                "f",
                tags,
                BlockOrigin::Annotated,
                Visibility::Public,
                params,
            ),
        );
        let diags = validate(&blocks, &[], &empty_pages(), &Config::default());
        assert!(diags.iter().any(|d| d.code.as_str() == "STD008"));
    }

    #[test]
    fn std012_param_type_mismatch() {
        let mut tags = BTreeMap::new();
        tags.insert(
            "param".to_owned(),
            vec![vec!["x".to_owned(), "u64".to_owned(), "?".to_owned()]],
        );
        let params = vec![ParamInfo {
            name: "x".to_owned(),
            type_repr: Some("i32".to_owned()),
            default: None,
            is_optional: false,
            is_variadic: false,
        }];
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "f".to_owned(),
            block_with(
                "f",
                tags,
                BlockOrigin::Annotated,
                Visibility::Public,
                params,
            ),
        );
        let diags = validate(&blocks, &[], &empty_pages(), &Config::default());
        let std012: Vec<&Diagnostic> = diags
            .iter()
            .filter(|d| d.code.as_str() == "STD012")
            .collect();
        assert_eq!(std012.len(), 1);
        assert!(std012[0].message.contains("u64"));
        assert!(std012[0].message.contains("i32"));
    }

    #[test]
    fn std012_tolerates_reference_wrapping() {
        // Documented as `Foo`, signature uses `&Foo` — should be considered compatible.
        let mut tags = BTreeMap::new();
        tags.insert(
            "param".to_owned(),
            vec![vec!["x".to_owned(), "Foo".to_owned(), "?".to_owned()]],
        );
        let params = vec![ParamInfo {
            name: "x".to_owned(),
            type_repr: Some("&Foo".to_owned()),
            default: None,
            is_optional: false,
            is_variadic: false,
        }];
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "f".to_owned(),
            block_with(
                "f",
                tags,
                BlockOrigin::Annotated,
                Visibility::Public,
                params,
            ),
        );
        let diags = validate(&blocks, &[], &empty_pages(), &Config::default());
        assert!(!diags.iter().any(|d| d.code.as_str() == "STD012"));
    }

    #[test]
    fn rule_overrides_disable_codes() {
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "f".to_owned(),
            block_with(
                "f",
                BTreeMap::new(),
                BlockOrigin::Inferred,
                Visibility::Public,
                vec![],
            ),
        );
        let mut config = Config::default();
        config.rules.insert("STD006".to_owned(), RuleOverride::Off);
        let diags = validate(&blocks, &[], &empty_pages(), &config);
        assert!(!diags.iter().any(|d| d.code.as_str() == "STD006"));
    }

    #[test]
    fn rule_overrides_change_severity() {
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "f".to_owned(),
            block_with(
                "f",
                BTreeMap::new(),
                BlockOrigin::Inferred,
                Visibility::Public,
                vec![],
            ),
        );
        let mut config = Config::default();
        config
            .rules
            .insert("STD006".to_owned(), RuleOverride::Error);
        let diags = validate(&blocks, &[], &empty_pages(), &config);
        let std006 = diags.iter().find(|d| d.code.as_str() == "STD006").unwrap();
        assert_eq!(std006.severity, Severity::Error);
    }

    fn page_with(slug: &str, body: &str) -> DocPage {
        use crate::pages::PageKind;
        DocPage {
            slug: slug.to_owned(),
            path: PathBuf::from(format!(".standardoc/pages/{slug}.md")),
            title: slug.to_owned(),
            order: None,
            section: vec![],
            frontmatter: BTreeMap::new(),
            raw_body: body.to_owned(),
            kind: PageKind::Md,
        }
    }

    #[test]
    fn std004_flags_unknown_doc_key_reference() {
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "math.add".to_owned(),
            block_with(
                "math.add",
                BTreeMap::new(),
                BlockOrigin::Annotated,
                Visibility::Public,
                vec![],
            ),
        );
        let mut pages = BTreeMap::new();
        pages.insert(
            "intro".to_owned(),
            page_with("intro", "See {{ @doc.math.subtract }} for details."),
        );
        let diags = validate(&blocks, &[], &pages, &Config::default());
        let std004 = diags.iter().find(|d| d.code.as_str() == "STD004").unwrap();
        assert!(std004.message.contains("math.subtract"));
        assert_eq!(std004.severity, Severity::Warning);
    }

    #[test]
    fn std004_accepts_short_name_match() {
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "math.add".to_owned(),
            block_with(
                "math.add",
                BTreeMap::new(),
                BlockOrigin::Annotated,
                Visibility::Public,
                vec![],
            ),
        );
        let mut pages = BTreeMap::new();
        pages.insert(
            "intro".to_owned(),
            page_with(
                "intro",
                "See {{ @doc.add }} — short name resolves to math.add.",
            ),
        );
        let diags = validate(&blocks, &[], &pages, &Config::default());
        assert!(diags.iter().all(|d| d.code.as_str() != "STD004"));
    }

    #[test]
    fn std004_flags_unknown_module_in_docs_iteration() {
        let blocks = BTreeMap::new();
        let mut pages = BTreeMap::new();
        pages.insert(
            "intro".to_owned(),
            page_with(
                "intro",
                "{{ each block in @docs.module(missing.module) }}{{ /each }}",
            ),
        );
        let diags = validate(&blocks, &[], &pages, &Config::default());
        let std004 = diags.iter().find(|d| d.code.as_str() == "STD004").unwrap();
        assert!(std004.message.contains("missing.module"));
    }

    #[test]
    fn std004_silent_on_plain_markdown_without_dsl() {
        let blocks = BTreeMap::new();
        let mut pages = BTreeMap::new();
        pages.insert(
            "intro".to_owned(),
            page_with(
                "intro",
                "# Hello\n\nJust regular markdown without any DSL refs.",
            ),
        );
        let diags = validate(&blocks, &[], &pages, &Config::default());
        assert!(diags.iter().all(|d| d.code.as_str() != "STD004"));
    }

    #[test]
    fn std007_flags_invalid_dsl_syntax() {
        let blocks = BTreeMap::new();
        let mut pages = BTreeMap::new();
        pages.insert(
            "broken".to_owned(),
            page_with(
                "broken",
                "{{ each block in @docs.all }}\noops no closing tag",
            ),
        );
        let diags = validate(&blocks, &[], &pages, &Config::default());
        let std007 = diags.iter().find(|d| d.code.as_str() == "STD007").unwrap();
        assert_eq!(std007.severity, Severity::Error);
    }

    #[test]
    fn std007_silent_on_plain_markdown() {
        let blocks = BTreeMap::new();
        let mut pages = BTreeMap::new();
        pages.insert(
            "plain".to_owned(),
            page_with("plain", "# A page\n\nNo DSL here at all."),
        );
        let diags = validate(&blocks, &[], &pages, &Config::default());
        assert!(diags.iter().all(|d| d.code.as_str() != "STD007"));
    }

    #[test]
    fn std002_diagnostics_attached_by_extractor_are_surfaced() {
        // Simulates what the extractor does when it encounters a
        // malformed `@doc`: it attaches the STD002 diagnostic directly
        // to the block, and the validator surfaces it without extra
        // logic.
        use crate::model::SourceRange;
        let mut block = block_with(
            "f",
            BTreeMap::new(),
            BlockOrigin::Annotated,
            Visibility::Public,
            vec![],
        );
        block.diagnostics.push(Diagnostic {
            code: DiagnosticCode::new(STD002.code),
            severity: STD002.severity,
            message: "`@doc` has no key".to_owned(),
            path: PathBuf::from("src/lib.rs"),
            range: SourceRange {
                line_start: 1,
                line_end: 1,
                column_start: 1,
                column_end: 1,
            },
            related: Vec::new(),
        });
        let mut blocks = BTreeMap::new();
        blocks.insert("f".to_owned(), block);
        let diags = validate(&blocks, &[], &empty_pages(), &Config::default());
        let std002 = diags.iter().find(|d| d.code.as_str() == "STD002").unwrap();
        assert!(std002.message.contains("no key"));
    }

    // -------- STD014 orphan satellite --------

    #[test]
    fn std014_fires_when_anchor_missing() {
        // Satellite at `tools.get_doc::schema` with no anchor `tools.get_doc`.
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "tools.get_doc::schema".to_owned(),
            block_with(
                "tools.get_doc::schema",
                BTreeMap::new(),
                BlockOrigin::Annotated,
                Visibility::Public,
                vec![],
            ),
        );
        let diags = validate(&blocks, &[], &empty_pages(), &Config::default());
        let std014 = diags.iter().find(|d| d.code.as_str() == "STD014").unwrap();
        assert!(std014.message.contains("tools.get_doc::schema"));
        assert!(std014.message.contains("tools.get_doc"));
        assert_eq!(std014.severity, Severity::Warning);
    }

    #[test]
    fn std014_silent_when_anchor_present() {
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "tools.get_doc".to_owned(),
            block_with(
                "tools.get_doc",
                BTreeMap::new(),
                BlockOrigin::Annotated,
                Visibility::Public,
                vec![],
            ),
        );
        blocks.insert(
            "tools.get_doc::schema".to_owned(),
            block_with(
                "tools.get_doc::schema",
                BTreeMap::new(),
                BlockOrigin::Annotated,
                Visibility::Public,
                vec![],
            ),
        );
        let diags = validate(&blocks, &[], &empty_pages(), &Config::default());
        assert!(diags.iter().all(|d| d.code.as_str() != "STD014"));
    }

    #[test]
    fn std014_does_not_fire_on_anchors_without_double_colon() {
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "math.add".to_owned(),
            block_with(
                "math.add",
                BTreeMap::new(),
                BlockOrigin::Annotated,
                Visibility::Public,
                vec![],
            ),
        );
        let diags = validate(&blocks, &[], &empty_pages(), &Config::default());
        assert!(diags.iter().all(|d| d.code.as_str() != "STD014"));
    }
}

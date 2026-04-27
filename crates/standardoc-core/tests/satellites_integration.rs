//! End-to-end pipeline tests for satellite annotations (`@doc-extend`).
//!
//! Uses an in-memory pipeline driven by a minimal `StubProvider` so the
//! tests don't need a real language crate. The stub recognizes the marker
//! `STUB-SYMBOL: <fqn>` in source and treats any `///` block immediately
//! above as `leading_comment`. This is enough to exercise both the anchor
//! path (symbol-attached `@doc`) and the satellite path
//! (free-floating `@doc-extend`).

use standardoc_core::config::Config;
use standardoc_core::lang::{DiscoveredSymbol, LanguageProvider, ParseError};
use standardoc_core::model::{
    CommentDelimiters, CommentStyles, SourceRange, SymbolInfo, SymbolKind, Visibility,
};
use standardoc_core::pipeline::scan_and_extract_in_memory;
use standardoc_core::scanner::Registry;
use standardoc_core::validator::validate;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Default, Debug, Clone, Copy)]
struct StubProvider;

impl LanguageProvider for StubProvider {
    fn id(&self) -> &'static str {
        "stub"
    }

    fn extensions(&self) -> &[&'static str] {
        &[".stub"]
    }

    fn comment_styles(&self) -> &CommentStyles {
        static STYLES: OnceLock<CommentStyles> = OnceLock::new();
        STYLES.get_or_init(|| CommentStyles {
            single: vec!["//".to_owned()],
            multi: Some(CommentDelimiters {
                start: "/*".to_owned(),
                end: "*/".to_owned(),
            }),
            doc_single: vec!["///".to_owned(), "//!".to_owned()],
            doc_multi: Some(CommentDelimiters {
                start: "/**".to_owned(),
                end: "*/".to_owned(),
            }),
        })
    }

    fn discover_symbols(
        &self,
        content: &str,
        _path: &Path,
    ) -> Result<Vec<DiscoveredSymbol>, ParseError> {
        let lines: Vec<&str> = content.lines().collect();
        let mut out = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let Some(name) = line.trim().strip_prefix("STUB-SYMBOL: ") else {
                continue;
            };
            let mut comment_lines: Vec<String> = Vec::new();
            for prev in lines[..i].iter().rev() {
                let trimmed = prev.trim_start();
                if let Some(body) = trimmed.strip_prefix("///") {
                    comment_lines.push(body.strip_prefix(' ').unwrap_or(body).to_owned());
                } else {
                    break;
                }
            }
            comment_lines.reverse();
            let leading = (!comment_lines.is_empty()).then(|| comment_lines.join("\n"));
            out.push(DiscoveredSymbol {
                fqn: vec![name.to_owned()],
                symbol: SymbolInfo {
                    kind: SymbolKind::Function,
                    visibility: Visibility::Public,
                    signature: format!("fn {name}()"),
                    ..SymbolInfo::default()
                },
                source_range: SourceRange::single_line(u32::try_from(i + 1).unwrap_or(1), 1, 20),
                leading_comment: leading,
                leading_comment_line_start: None,
            });
        }
        Ok(out)
    }
}

fn registry() -> Registry {
    Registry::builder().with(StubProvider).build()
}

fn workspace() -> PathBuf {
    PathBuf::from("/workspace")
}

fn file(name: &str, content: &str) -> (PathBuf, String) {
    (
        PathBuf::from(format!("/workspace/{name}")),
        content.to_owned(),
    )
}

#[test]
fn satellite_in_separate_file_attaches_to_anchor() {
    // anchor.stub declares the anchor; satellite.stub adds a side block.
    let files = vec![
        file(
            "anchor.stub",
            "/// @doc tools.get_doc\n/// @description Fetch one block.\nSTUB-SYMBOL: tools.get_doc\n",
        ),
        file(
            "satellite.stub",
            "// @doc-extend tools.get_doc schema\n// @args-schema {\"type\":\"object\"}\n",
        ),
    ];
    let report = scan_and_extract_in_memory(files, &workspace(), &registry(), &Config::default());
    assert!(report.errors.is_empty(), "scan errors: {:?}", report.errors);

    let anchor = report
        .blocks
        .get("tools.get_doc")
        .expect("anchor block missing");
    assert_eq!(
        anchor.tags.get("description").unwrap()[0][0],
        "Fetch one block."
    );

    let satellite = report
        .blocks
        .get("tools.get_doc::schema")
        .expect("satellite block missing");
    assert_eq!(satellite.label, "schema");
    assert!(satellite.symbol.is_none());
    assert_eq!(
        satellite.tags.get("args-schema").unwrap()[0][0],
        "{\"type\":\"object\"}"
    );
    assert!(!satellite.tags.contains_key("doc-extend"));
}

#[test]
fn validator_passes_when_anchor_present_for_satellite() {
    let files = vec![
        file(
            "anchor.stub",
            "/// @doc tools.get_doc\n/// @description Fetch one block.\nSTUB-SYMBOL: tools.get_doc\n",
        ),
        file(
            "satellite.stub",
            "// @doc-extend tools.get_doc schema\n",
        ),
    ];
    let report = scan_and_extract_in_memory(files, &workspace(), &registry(), &Config::default());
    let diags = validate(
        &report.blocks,
        &report.collisions,
        &report.pages,
        &Config::default(),
    );
    assert!(
        diags.iter().all(|d| d.code.as_str() != "STD014"),
        "unexpected STD014: {diags:?}"
    );
}

#[test]
fn validator_emits_std014_when_satellite_anchor_missing() {
    // No anchor file — satellite is orphaned.
    let files = vec![file(
        "satellite.stub",
        "// @doc-extend tools.get_doc schema\n",
    )];
    let report = scan_and_extract_in_memory(files, &workspace(), &registry(), &Config::default());
    let diags = validate(
        &report.blocks,
        &report.collisions,
        &report.pages,
        &Config::default(),
    );
    let std014 = diags
        .iter()
        .find(|d| d.code.as_str() == "STD014")
        .expect("expected STD014");
    assert!(std014.message.contains("tools.get_doc::schema"));
    assert!(std014.message.contains("tools.get_doc"));
}

#[test]
fn malformed_satellite_is_silently_dropped_for_now() {
    // `@doc-extend` with only one argument (no extended-key segment) → no
    // satellite block emitted. STD015 is deferred until extraction-time
    // diagnostics flow through.
    let files = vec![file(
        "satellite.stub",
        "// @doc-extend tools.get_doc\n// @description forgot the second arg\n",
    )];
    let report = scan_and_extract_in_memory(files, &workspace(), &registry(), &Config::default());
    assert!(
        !report.blocks.keys().any(|k| k.contains("::")),
        "no satellite should be created without the extended-key arg"
    );
}

#[test]
fn multiple_satellites_each_get_their_own_block() {
    let files = vec![
        file(
            "anchor.stub",
            "/// @doc tools.get_doc\nSTUB-SYMBOL: tools.get_doc\n",
        ),
        file(
            "schema.stub",
            "// @doc-extend tools.get_doc schema\n// @description Schema satellite.\n",
        ),
        file(
            "examples.stub",
            "// @doc-extend tools.get_doc examples\n// @description Examples satellite.\n",
        ),
    ];
    let report = scan_and_extract_in_memory(files, &workspace(), &registry(), &Config::default());
    assert!(report.blocks.contains_key("tools.get_doc::schema"));
    assert!(report.blocks.contains_key("tools.get_doc::examples"));
    assert_eq!(
        report.blocks["tools.get_doc::schema"]
            .tags
            .get("description")
            .unwrap()[0][0],
        "Schema satellite."
    );
    assert_eq!(
        report.blocks["tools.get_doc::examples"]
            .tags
            .get("description")
            .unwrap()[0][0],
        "Examples satellite."
    );
}

#[test]
fn free_floating_anchor_creates_block_without_symbol() {
    // A free-floating `// @doc K` (no STUB-SYMBOL after) becomes an anchor
    // block with `symbol=None`, just like satellites do.
    let files = vec![file(
        "anchor.stub",
        "// @doc tools.shared\n// @description Anchor declared without a backing symbol.\n",
    )];
    let report = scan_and_extract_in_memory(files, &workspace(), &registry(), &Config::default());
    assert!(report.errors.is_empty(), "scan errors: {:?}", report.errors);
    let block = report
        .blocks
        .get("tools.shared")
        .expect("free-floating anchor block missing");
    assert_eq!(block.label, "shared");
    assert!(block.symbol.is_none());
    assert!(!block.tags.contains_key("doc"));
    assert_eq!(
        block.tags.get("description").unwrap()[0][0],
        "Anchor declared without a backing symbol."
    );
}

#[test]
fn free_floating_anchor_does_not_collide_with_symbol_attached_anchor() {
    // The same `///` block leading a symbol should NOT also be re-emitted as
    // a free-floating anchor — that would yield a phantom STD001 for every
    // annotated symbol. The pipeline filters spans whose end-line is right
    // before a discovered symbol.
    let files = vec![file(
        "anchor.stub",
        "/// @doc tools.attached\n/// @description Attached anchor.\nSTUB-SYMBOL: tools.attached\n",
    )];
    let report = scan_and_extract_in_memory(files, &workspace(), &registry(), &Config::default());
    assert!(
        report.collisions.is_empty(),
        "no collision expected, got {:?}",
        report.collisions
    );
    let block = &report.blocks["tools.attached"];
    assert!(
        block.symbol.is_some(),
        "symbol-attached anchor must keep its symbol info"
    );
}

#[test]
fn free_floating_anchor_resolves_satellite_in_separate_file() {
    // The motivating use case: an MCP tool whose handler is shared
    // (no 1:1 attachment possible) declares its anchor free-floating, and
    // a satellite extends it from the same or a different file. The
    // satellite should resolve cleanly with no STD014.
    let files = vec![
        file(
            "anchor.stub",
            "// @doc mcp.tools.emit_llms_txt\n// @description Generate llms.txt.\n",
        ),
        file(
            "satellite.stub",
            "// @doc-extend mcp.tools.emit_llms_txt args\n// @schema {\"type\":\"object\"}\n",
        ),
    ];
    let report = scan_and_extract_in_memory(files, &workspace(), &registry(), &Config::default());
    let diags = validate(
        &report.blocks,
        &report.collisions,
        &report.pages,
        &Config::default(),
    );
    assert!(
        diags.iter().all(|d| d.code.as_str() != "STD014"),
        "free-floating anchor should keep satellite happy, got STD014: {diags:?}"
    );
    assert!(report.blocks.contains_key("mcp.tools.emit_llms_txt"));
    assert!(report.blocks.contains_key("mcp.tools.emit_llms_txt::args"));
}

#[test]
fn std014_ignores_rust_path_syntax_in_inferred_keys() {
    // Inferred Rust trait-impl symbols produce keys like
    // `<RegexProvider as std :: fmt :: Debug>.fmt` where the ` :: ` is the
    // path syntax, not a satellite separator. STD014 must NOT fire on them.
    use standardoc_core::model::{
        BlockOrigin, CommentStyle, DocBlock, DocKey, DocMeta, SymbolInfo, SymbolKind, Visibility,
    };
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    let key = "lang_regex.<RegexProvider as std :: fmt :: Debug>.fmt";
    let mut blocks = BTreeMap::new();
    blocks.insert(
        key.to_owned(),
        DocBlock {
            key: DocKey::new(key),
            label: "fmt".to_owned(),
            origin: BlockOrigin::Inferred,
            tags: BTreeMap::new(),
            symbol: Some(SymbolInfo {
                kind: SymbolKind::Method,
                visibility: Visibility::Public,
                signature: "fn fmt(&self, f: &mut Formatter) -> Result".to_owned(),
                ..SymbolInfo::default()
            }),
            meta: DocMeta {
                path: PathBuf::from("lang_regex.rs"),
                line_start: 1,
                line_end: 1,
                column: 1,
                file_ext: "rs".to_owned(),
                comment_style: CommentStyle::DocSingle,
                last_indexed: 0,
                source_mtime: 0,
            },
            body_hash: 0,
            diagnostics: Vec::new(),
            virtual_tags: BTreeMap::new(),
            virtual_confidence: None,
            virtual_sources: Vec::new(),
        },
    );

    let pages = BTreeMap::new();
    let collisions = Vec::new();
    let diags = validate(&blocks, &collisions, &pages, &Config::default());
    assert!(
        diags.iter().all(|d| d.code.as_str() != "STD014"),
        "Rust path syntax should not trigger STD014: {diags:?}"
    );
}

#[test]
fn duplicate_satellite_key_triggers_collision() {
    // Two satellite files declaring the same K::NAME — STD001 collision.
    let files = vec![
        file(
            "anchor.stub",
            "/// @doc tools.get_doc\nSTUB-SYMBOL: tools.get_doc\n",
        ),
        file("a.stub", "// @doc-extend tools.get_doc schema\n"),
        file("b.stub", "// @doc-extend tools.get_doc schema\n"),
    ];
    let report = scan_and_extract_in_memory(files, &workspace(), &registry(), &Config::default());
    let collision = report
        .collisions
        .iter()
        .find(|c| c.key == "tools.get_doc::schema")
        .expect("expected collision on duplicate K::NAME");
    assert_eq!(collision.dropped.len(), 1);
}

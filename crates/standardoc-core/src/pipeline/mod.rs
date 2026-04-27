//! High-level orchestration: scan a workspace, run extraction, collect blocks.
//!
//! Both `standardoc` (CLI) and `standardoc-server` (daemon) call into this
//! module so their results stay byte-identical.

use crate::config::Config;
use crate::extractor::{
    extract_block, extract_free_floating_anchors, extract_satellite_blocks, ExtractContext,
};
use crate::model::{CommentStyle, DocBlock};
use crate::pages::{scan_pages, DocPage};
use crate::scanner::{
    derive_module_prefix, scan_in_memory, scan_workspace, FileScan, Registry, ScanError,
    ScanOptions,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Result of a full scan+extract pass.
#[derive(Debug, Default)]
pub struct PipelineReport {
    pub blocks: BTreeMap<String, DocBlock>,
    /// Narrative pages loaded from `.standardoc/pages/`. User-curated source
    /// of truth, parallel to `blocks` extracted from code.
    pub pages: BTreeMap<String, DocPage>,
    pub errors: Vec<ScanError>,
    /// Key collisions detected during extraction. Each entry records the
    /// winning block and the file/line of every discarded duplicate.
    /// Corresponds to future diagnostic `STD001` — we surface it here so the
    /// server and CLI can report it without needing the full validator yet.
    pub collisions: Vec<KeyCollision>,
    pub workspace_root: PathBuf,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct KeyCollision {
    pub key: String,
    pub kept: PathLine,
    pub dropped: Vec<PathLine>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PathLine {
    pub path: PathBuf,
    pub line: u32,
}

/// Scans `root` with the given provider `registry`, extracts `DocBlock`s
/// against `config`, and returns them keyed by string `DocKey`.
///
/// Errors surfaced on individual files are collected into `PipelineReport::errors`
/// rather than aborting, matching the behavior of `scan_workspace`. Key
/// collisions are NOT discarded silently — see `PipelineReport::collisions`.
pub fn scan_and_extract(
    root: &Path,
    registry: &Registry,
    config: &Config,
) -> Result<PipelineReport, std::io::Error> {
    let root = root.canonicalize()?;
    let scan = scan_workspace(&root, registry, &scan_options_from(config));
    let pages = scan_pages(&root);
    let (blocks, collisions) = extract_blocks_from_scan(scan.files, &root, config, |path| {
        file_mtime_unix_seconds(path)
    });

    Ok(PipelineReport {
        blocks,
        pages,
        errors: scan.errors,
        collisions,
        workspace_root: root,
    })
}

/// In-memory variant of [`scan_and_extract`] for hosts without direct
/// filesystem access — primarily the WASM build linked into the VSCode
/// extension, where the host enumerates files via VS Code APIs and feeds
/// the bytes in.
///
/// `workspace_root` is taken as-is (no canonicalization) and used only to
/// normalize `DocMeta.path` of emitted blocks. `pages` stay empty: narrative
/// pages live under `.standardoc/pages/` on disk and there is no
/// equivalent in-memory ingestion path yet — the caller can attach pages
/// post-hoc to the returned `PipelineReport` if needed.
#[must_use]
pub fn scan_and_extract_in_memory(
    files: Vec<(PathBuf, String)>,
    workspace_root: &Path,
    registry: &Registry,
    config: &Config,
) -> PipelineReport {
    let scan = scan_in_memory(files, registry);
    let (blocks, collisions) = extract_blocks_from_scan(scan.files, workspace_root, config, |_| 0);

    PipelineReport {
        blocks,
        pages: BTreeMap::new(),
        errors: scan.errors,
        collisions,
        workspace_root: workspace_root.to_path_buf(),
    }
}

/// Shared post-scan logic: walk discovered symbols, run extraction +
/// virtual-annotation synthesis, collect blocks and key collisions. Both
/// `scan_and_extract` and `scan_and_extract_in_memory` funnel here so the
/// extraction semantics stay byte-identical between disk and in-memory
/// hosts. `mtime_for` lets the disk path attach real mtimes while the
/// in-memory path passes `0` (no concept of mtime there).
fn extract_blocks_from_scan(
    files: Vec<FileScan>,
    workspace_root: &Path,
    config: &Config,
    mtime_for: impl Fn(&Path) -> i64,
) -> (BTreeMap<String, DocBlock>, Vec<KeyCollision>) {
    let now = unix_seconds_now();
    let mut blocks: BTreeMap<String, DocBlock> = BTreeMap::new();
    let mut locations: BTreeMap<String, Vec<PathLine>> = BTreeMap::new();

    for file in files {
        let source_mtime = mtime_for(&file.path);
        let ext = file
            .path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_owned();
        let ctx = ExtractContext {
            workspace_root,
            file_path: file.path.clone(),
            file_ext: ext,
            comment_style: CommentStyle::DocSingle,
            source_mtime,
            last_indexed: now,
        };
        let symbol_starts: std::collections::BTreeSet<u32> = file
            .symbols
            .iter()
            .map(|s| s.source_range.line_start)
            .collect();
        // Ranges of doc-comments that a language provider already folded
        // into a symbol's `leading_comment` even though the comment is not
        // contiguous with the symbol (e.g. Rust `///` separated from
        // `pub struct Foo` by `#[derive(...)]` attributes). We exclude any
        // span falling in `[leading_start, symbol_start)` from free-floating
        // anchor extraction — re-emitting it would produce STD001 duplicates.
        let consumed_ranges: Vec<(u32, u32)> = file
            .symbols
            .iter()
            .filter_map(|s| {
                s.leading_comment_line_start
                    .map(|start| (start, s.source_range.line_start))
            })
            .collect();
        for sym in file.symbols {
            if let Some(mut block) = extract_block(sym, &ctx, config) {
                crate::virtual_annotation::synthesize(
                    &mut block,
                    config.discovery.virtual_annotations,
                );
                let key = block.key.as_str().to_owned();
                let origin = PathLine {
                    path: block.meta.path.clone(),
                    line: block.meta.line_start,
                };
                locations.entry(key.clone()).or_default().push(origin);
                blocks.insert(key, block);
            }
        }
        for satellite in extract_satellite_blocks(&file.comment_spans, &ctx, config) {
            let key = satellite.key.as_str().to_owned();
            let origin = PathLine {
                path: satellite.meta.path.clone(),
                line: satellite.meta.line_start,
            };
            locations.entry(key.clone()).or_default().push(origin);
            blocks.insert(key, satellite);
        }
        // Free-floating `@doc K` anchors: spans not attached to a discovered
        // symbol. A span is "attached" if (a) the next line after it begins a
        // symbol, or (b) it falls inside a `consumed_ranges` window where a
        // provider already folded it into the symbol's leading_comment across
        // intervening attribute/blank lines. Both paths would otherwise re-emit
        // the span as a duplicate anchor here.
        let free_spans: Vec<crate::extractor::comment_scan::CommentSpan> = file
            .comment_spans
            .iter()
            .filter(|span| {
                if symbol_starts.contains(&(span.line_end + 1)) {
                    return false;
                }
                !consumed_ranges
                    .iter()
                    .any(|(start, end)| span.line_start >= *start && span.line_end < *end)
            })
            .cloned()
            .collect();
        for anchor in extract_free_floating_anchors(&free_spans, &ctx, config) {
            let key = anchor.key.as_str().to_owned();
            let origin = PathLine {
                path: anchor.meta.path.clone(),
                line: anchor.meta.line_start,
            };
            locations.entry(key.clone()).or_default().push(origin);
            blocks.insert(key, anchor);
        }
    }

    let collisions: Vec<KeyCollision> = locations
        .into_iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|(key, mut v)| {
            let kept = v.pop().expect("non-empty by filter");
            KeyCollision {
                key,
                kept,
                dropped: v,
            }
        })
        .collect();

    (blocks, collisions)
}

/// Build `ScanOptions` from workspace `Config` — glue between user-facing
/// JSON format and scanner internal type.
fn scan_options_from(config: &Config) -> ScanOptions {
    ScanOptions {
        respect_gitignore: config.discovery.respect_gitignore,
        exclude_files: config.discovery.exclude_files.clone(),
        ..ScanOptions::default()
    }
}

/// Scan result for **a single file**. Used by watcher worker to perform
/// incremental rescans without touching other index entries.
pub enum FileScanOutcome {
    /// File was parsed successfully, with extracted blocks (can be empty).
    Ok(Vec<DocBlock>),
    /// File failed to parse — possibly mid-edit. Caller should **keep old
    /// blocks** for this path in index instead of creating a temporary hole.
    ParseError(String),
    /// I/O error (file removed between watch and read, permission denied, etc.)
    /// Same caller handling as `ParseError`.
    IoError(std::io::Error),
    /// No provider accepts this extension — caller can ignore.
    NoProvider,
}

/// Rescan **a single file** and extract its blocks. Applies the same module
/// prefix injection as `scan_workspace`.
///
/// Returned blocks keep `meta.path` relative to `workspace_root`, same as full
/// pipeline — caller can merge into global index without path conversion.
///
/// This function is designed for FS watcher events: fast (single file), and
/// resilient to transient parse errors (returns explicit variant, no panic).
pub fn scan_and_extract_file(
    file_path: &Path,
    workspace_root: &Path,
    registry: &Registry,
    config: &Config,
) -> FileScanOutcome {
    let Some(provider) = registry.resolve(file_path) else {
        return FileScanOutcome::NoProvider;
    };

    let content = match std::fs::read_to_string(file_path) {
        Ok(c) => c,
        Err(err) => return FileScanOutcome::IoError(err),
    };

    let mut symbols = match provider.discover_symbols(&content, file_path) {
        Ok(s) => s,
        Err(err) => return FileScanOutcome::ParseError(err.to_string()),
    };

    // Same module-prefix handling as `scan_workspace::scan_file`.
    let prefix = derive_module_prefix(file_path);
    if !prefix.is_empty() {
        for sym in &mut symbols {
            let mut full = prefix.clone();
            full.append(&mut sym.fqn);
            sym.fqn = full;
        }
    }

    let comment_spans = crate::extractor::comment_scan::scan(&content, provider.comment_styles());

    let now = unix_seconds_now();
    let source_mtime = file_mtime_unix_seconds(file_path);
    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_owned();

    let ctx = ExtractContext {
        workspace_root,
        file_path: file_path.to_path_buf(),
        file_ext: ext,
        comment_style: CommentStyle::DocSingle,
        source_mtime,
        last_indexed: now,
    };

    let symbol_starts: std::collections::BTreeSet<u32> =
        symbols.iter().map(|s| s.source_range.line_start).collect();
    let mut blocks = Vec::new();
    for sym in symbols {
        if let Some(mut block) = extract_block(sym, &ctx, config) {
            crate::virtual_annotation::synthesize(&mut block, config.discovery.virtual_annotations);
            blocks.push(block);
        }
    }
    blocks.extend(extract_satellite_blocks(&comment_spans, &ctx, config));
    let free_spans: Vec<crate::extractor::comment_scan::CommentSpan> = comment_spans
        .iter()
        .filter(|span| !symbol_starts.contains(&(span.line_end + 1)))
        .cloned()
        .collect();
    blocks.extend(extract_free_floating_anchors(&free_spans, &ctx, config));
    FileScanOutcome::Ok(blocks)
}

fn unix_seconds_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

fn file_mtime_unix_seconds(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

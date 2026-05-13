//! Best-effort token-savings telemetry hooked into each read-path tool.
//!
//! The baseline is grounded in the response graph: every workspace-relative
//! source file referenced by the response counts once. Sum of those file
//! sizes = the bytes an AI would have consumed by reading the relevant
//! sources raw (the honest floor — ignores transitive navigation, no
//! arbitrary multiplier).
//!
//! Logging is fire-and-forget: errors are swallowed, sqlite latency stays
//! off the response path. The `sessions.db` write goes through
//! `SessionsHandle::open` on each call — cheap (~1ms WAL) and avoids
//! parking a connection in the MCP handler struct.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use standardoc_core::SessionsHandle;
use standardoc_core::query::{BodySlice, SymbolContextWithNeighbors};
use standardoc_ir::RawSymbol;

/// Sums the file sizes of distinct workspace-relative paths in `files`.
/// Missing files silently contribute 0 — the index may reference a path
/// that has been deleted between scan time and query time. Empty strings
/// are skipped.
pub(crate) fn sum_distinct_file_sizes<I>(workspace_root: &Path, files: I) -> u64
where
    I: IntoIterator<Item = String>,
{
    let mut seen = HashSet::new();
    let mut total: u64 = 0;
    for rel in files {
        if rel.is_empty() {
            continue;
        }
        if seen.insert(rel.clone()) {
            let abs = workspace_root.join(&rel);
            if let Ok(meta) = std::fs::metadata(&abs) {
                total = total.saturating_add(meta.len());
            }
        }
    }
    total
}

/// Collects the source files referenced by a `get_context` response:
/// the queried symbol plus every resolved neighbor across all six groups.
/// `depth = 1` responses leave `resolved_symbol = None` — only the
/// queried symbol's file shows up, which is the honest floor for that
/// shape (the AI got FQDNs, not bodies).
pub(crate) fn files_from_context(ctx: &SymbolContextWithNeighbors) -> Vec<String> {
    let mut files: Vec<String> = Vec::with_capacity(8);
    files.push(ctx.context.symbol.location.file.clone());
    let groups = [
        ctx.callers.as_slice(),
        ctx.callees.as_slice(),
        ctx.imports.as_slice(),
        ctx.imported_by.as_slice(),
        ctx.dependents.as_slice(),
        ctx.tests.as_slice(),
    ];
    for group in groups {
        for n in group {
            if let Some(sym) = &n.resolved_symbol {
                files.push(sym.location.file.clone());
            }
        }
    }
    files
}

/// Source files for a flat list of symbols (`find_symbol`, `list_symbols`,
/// `find_symbols_by_pattern`).
pub(crate) fn files_from_symbols(syms: &[RawSymbol]) -> Vec<String> {
    syms.iter().map(|s| s.location.file.clone()).collect()
}

/// Source files for similarity results `(symbol, score)`.
pub(crate) fn files_from_similar(rows: &[(RawSymbol, f32)]) -> Vec<String> {
    rows.iter().map(|(s, _)| s.location.file.clone()).collect()
}

/// Source file for a single `get_body` slice.
pub(crate) fn files_from_body(body: &BodySlice) -> Vec<String> {
    vec![body.file.clone()]
}

/// Fire-and-forget usage logging. Spawns onto the tokio runtime so the
/// caller's `await` returns immediately; the spawned task opens a fresh
/// `SessionsHandle` and inserts a row. Errors are dropped silently —
/// telemetry must never block or fail a tool call.
pub(crate) fn log_usage_fire_and_forget(
    workspace_root: PathBuf,
    tool_name: &'static str,
    fqdn: Option<String>,
    bytes_out: u64,
    baseline_bytes: u64,
) {
    tokio::task::spawn(async move {
        let _ = tokio::task::spawn_blocking(move || {
            if let Ok(h) = SessionsHandle::open(&workspace_root) {
                let bytes_out_i64 = i64::try_from(bytes_out).unwrap_or(i64::MAX);
                let baseline_i64 = i64::try_from(baseline_bytes).unwrap_or(i64::MAX);
                let _ = h.log_usage(tool_name, fqdn.as_deref(), bytes_out_i64, baseline_i64);
            }
        })
        .await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fixture_workspace_with_files(files: &[(&str, &str)]) -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (rel, content) in files {
            let abs = dir.path().join(rel);
            if let Some(parent) = abs.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(abs, content).unwrap();
        }
        dir
    }

    #[test]
    fn sum_distinct_file_sizes_dedupes() {
        let dir = fixture_workspace_with_files(&[("src/a.rs", "aaa"), ("src/b.rs", "bbbbb")]);
        let sum = sum_distinct_file_sizes(
            dir.path(),
            ["src/a.rs", "src/a.rs", "src/b.rs"]
                .into_iter()
                .map(String::from),
        );
        assert_eq!(sum, 3 + 5);
    }

    #[test]
    fn sum_distinct_file_sizes_skips_missing() {
        let dir = fixture_workspace_with_files(&[("src/a.rs", "aaa")]);
        let sum = sum_distinct_file_sizes(
            dir.path(),
            ["src/a.rs", "src/ghost.rs"].into_iter().map(String::from),
        );
        assert_eq!(sum, 3);
    }

    #[test]
    fn sum_distinct_file_sizes_skips_empty_paths() {
        let dir = fixture_workspace_with_files(&[("src/a.rs", "abc")]);
        let sum = sum_distinct_file_sizes(
            dir.path(),
            ["", "src/a.rs", ""].into_iter().map(String::from),
        );
        assert_eq!(sum, 3);
    }

    #[test]
    fn sum_distinct_file_sizes_empty_input() {
        let dir = tempfile::tempdir().unwrap();
        let sum = sum_distinct_file_sizes(dir.path(), std::iter::empty::<String>());
        assert_eq!(sum, 0);
    }
}

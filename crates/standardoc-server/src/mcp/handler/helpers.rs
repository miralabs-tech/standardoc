//! Stateless helper functions for the MCP handler: param parsing,
//! similarity-search enrichment, FQDN normalisation, response framing.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use rmcp::ErrorData;
use rmcp::model::{CallToolResult, Content};
use serde::Serialize;
use standardoc_core::{
    IndexHandle, WatcherHandle,
    query::{self, SymbolFilter},
};
use standardoc_ir::{IndexingMode, Kind, LinkDirection, SourceOrigin, Visibility};

use crate::mcp::error::server_error_to_rmcp;

use super::{
    DID_YOU_MEAN_LIMIT, DID_YOU_MEAN_THRESHOLD, FIND_SIMILAR_DEFAULT_THRESHOLD,
    FIND_SYMBOL_DEFAULT_LIMIT, FIND_SYMBOL_MAX_LIMIT,
};

/// Drop empty / whitespace-only strings to `None` so an MCP caller can
/// pass `from_fqdn: ""` without smuggling a vacuous filter into the SQL.
#[allow(clippy::needless_pass_by_value)]
pub(super) fn non_empty(s: String) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub(super) const fn source_origin_label(origin: SourceOrigin) -> &'static str {
    match origin {
        SourceOrigin::Workspace => "workspace",
        SourceOrigin::CargoRegistry => "cargo_registry",
        SourceOrigin::NodeModulesDts => "node_modules_dts",
        SourceOrigin::ManualExternal => "manual_external",
    }
}

pub(super) fn parse_filter(
    kind: Option<&str>,
    visibility: Option<&str>,
    module: Option<String>,
    include_external: Option<bool>,
    workspace_id: Option<String>,
) -> Result<SymbolFilter, ErrorData> {
    let kind = kind.map(parse_kind).transpose()?;
    let visibility = visibility.map(parse_visibility).transpose()?;
    let include_external = include_external.unwrap_or(true);
    Ok(SymbolFilter {
        kind,
        visibility,
        module,
        include_external,
        workspace_id,
    })
}

pub(super) fn parse_link_direction(s: &str) -> Result<LinkDirection, ErrorData> {
    match s {
        "in" => Ok(LinkDirection::In),
        "out" => Ok(LinkDirection::Out),
        "bidirectional" => Ok(LinkDirection::Bidirectional),
        other => Err(ErrorData::invalid_params(
            format!("unknown direction `{other}` — expected one of: in, out, bidirectional"),
            None,
        )),
    }
}

pub(super) fn parse_indexing_mode(s: Option<&str>) -> Result<IndexingMode, ErrorData> {
    match s {
        None => Ok(IndexingMode::default()),
        Some("blob_import") => Ok(IndexingMode::BlobImport),
        Some("extract") => Ok(IndexingMode::Extract),
        Some(other) => Err(ErrorData::invalid_params(
            format!("unknown indexing_mode `{other}` — expected one of: blob_import, extract"),
            None,
        )),
    }
}

pub(super) const fn link_direction_label(d: LinkDirection) -> &'static str {
    match d {
        LinkDirection::In => "in",
        LinkDirection::Out => "out",
        LinkDirection::Bidirectional => "bidirectional",
    }
}

/// Does this direction trigger the live watcher to observe the peer
/// root? `Out` means the peer reads us — we have nothing to watch on
/// their side, so the watcher stays silent. `In` and `Bidirectional`
/// both require watching the peer's source.
pub(super) const fn watches_peer(d: LinkDirection) -> bool {
    matches!(d, LinkDirection::In | LinkDirection::Bidirectional)
}

/// L3d-3 helper: hand a freshly-linked peer to the live watcher. Lives
/// outside the handler impl so the locking pattern is visible at the
/// call site. Best-effort: any failure (slot empty, debouncer dropped,
/// notify error) is logged and swallowed — the catalog write already
/// succeeded and the next cold_start will reconcile.
#[allow(clippy::needless_pass_by_value)]
pub(super) fn register_peer_with_watcher(
    slot: &Arc<Mutex<Option<WatcherHandle>>>,
    workspace_id: String,
    root: &Path,
) {
    let mut guard = match slot.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let Some(w) = guard.as_mut() else {
        // Watcher not booted yet (readonly mode, pre-cold-start, or
        // already shut down). Cold_start will pick up the peer from
        // workspace_catalog on the next boot.
        return;
    };
    if let Err(e) = w.add_peer(workspace_id.clone(), root) {
        eprintln!(
            "standardoc mcp: watcher add_peer failed for {workspace_id} ({}): {e}",
            root.display()
        );
    }
}

/// L3d-3 helper: drop a peer from the live watcher registry. Idempotent.
pub(super) fn unregister_peer_from_watcher(
    slot: &Arc<Mutex<Option<WatcherHandle>>>,
    workspace_id: &str,
) {
    let mut guard = match slot.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let Some(w) = guard.as_mut() else {
        return;
    };
    if let Err(e) = w.remove_peer(workspace_id) {
        eprintln!("standardoc mcp: watcher remove_peer failed for {workspace_id}: {e}");
    }
}

pub(super) fn parse_kind(s: &str) -> Result<Kind, ErrorData> {
    match s {
        "callable" => Ok(Kind::Callable),
        "type" => Ok(Kind::Type),
        "value" => Ok(Kind::Value),
        "module" => Ok(Kind::Module),
        "macro" => Ok(Kind::Macro),
        other => Err(ErrorData::invalid_params(
            format!(
                "unknown kind `{other}` — expected one of: callable, type, value, module, macro"
            ),
            None,
        )),
    }
}

pub(super) fn parse_threshold(raw: Option<f32>) -> Result<f32, ErrorData> {
    let value = raw.unwrap_or(FIND_SIMILAR_DEFAULT_THRESHOLD);
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(ErrorData::invalid_params(
            format!("threshold must be a finite value in [0.0, 1.0], got `{value}`"),
            None,
        ));
    }
    Ok(value)
}

pub(super) fn parse_visibility(s: &str) -> Result<Visibility, ErrorData> {
    match s {
        "public" => Ok(Visibility::Public),
        "private" => Ok(Visibility::Private),
        "crate" => Ok(Visibility::Crate),
        "protected" => Ok(Visibility::Protected),
        other => Err(ErrorData::invalid_params(
            format!(
                "unknown visibility `{other}` — expected one of: public, private, crate, protected"
            ),
            None,
        )),
    }
}

/// Runs the strsim-backed similarity search to populate the
/// `did_you_mean` field surfaced by `find_symbol` /
/// `find_symbols_by_pattern` when the primary query returns zero hits.
/// Returns a slim JSON array `[{fqdn, name, kind, score}, ...]` capped
/// at `DID_YOU_MEAN_LIMIT` and floored at `DID_YOU_MEAN_THRESHOLD`.
pub(super) async fn compute_did_you_mean(
    handle: IndexHandle,
    text: String,
    filter: SymbolFilter,
) -> Result<Vec<serde_json::Value>, ErrorData> {
    let pairs = tokio::task::spawn_blocking(move || {
        query::find_similar(
            &handle,
            &text,
            DID_YOU_MEAN_THRESHOLD,
            &filter,
            DID_YOU_MEAN_LIMIT,
        )
    })
    .await
    .map_err(|e| ErrorData::internal_error(format!("spawn_blocking: {e}"), None))?
    .map_err(|e| server_error_to_rmcp(&e.into()))?;
    Ok(pairs
        .into_iter()
        .map(|(sym, score)| {
            serde_json::json!({
                "fqdn": sym.fqdn,
                "name": sym.name,
                "kind": serde_json::to_value(sym.kind).unwrap_or(serde_json::Value::Null),
                "score": score,
            })
        })
        .collect())
}

/// Strips SQLite GLOB wildcards (`*`, `?`, `[`, `]`) from a pattern to
/// extract a "core name" usable for similarity scoring. Backs the
/// `did_you_mean` enrichment on empty `find_symbols_by_pattern`
/// results — e.g. `*to_token_string*` → `to_token_string`, then strsim
/// surfaces `to_token_stream`.
pub(super) fn glob_core_text(pattern: &str) -> String {
    pattern
        .chars()
        .filter(|c| !matches!(c, '*' | '?' | '[' | ']'))
        .collect::<String>()
        .trim()
        .to_string()
}

/// Defense-in-depth normalization for FQDN inputs reaching exact-match
/// query paths (`get_body`, `get_context`, module filters, …).
///
/// LLM consumers trained on Python / JS / TS naturally emit OOP-style
/// dotted names (`Type.method`) even though Standardoc stores every
/// FQDN with `::` regardless of source language. Without this
/// normalization, `get_body("StandardocMcp.find_symbol")` would miss
/// the symbol stored as `…::StandardocMcp::find_symbol` and surface a
/// "no symbol found" message that looks like a real absence.
///
/// `.` never appears inside a valid FQDN segment in any supported
/// language (Rust / TS / Lua identifiers can't contain a dot), so the
/// replacement is lossless and idempotent on `::`-form inputs.
pub(super) fn normalize_fqdn(raw: &str) -> String {
    raw.replace('.', "::")
}

/// Project `fqdn` to its `relative_to`-anchored form. FQDNs sharing the
/// prefix become `::<rest>`; the prefix itself collapses to the empty
/// string; FQDNs that don't share the prefix are returned verbatim. An
/// empty `relative_to` short-circuits to the input. Used by the
/// `find_symbol_fqdns` / `list_symbol_fqdns` projections to compress
/// scoped listings.
pub(super) fn relative_fqdn(fqdn: &str, relative_to: &str) -> String {
    if relative_to.is_empty() {
        return fqdn.to_string();
    }
    if fqdn == relative_to {
        return String::new();
    }
    if let Some(rest) = fqdn.strip_prefix(relative_to)
        && let Some(rest) = rest.strip_prefix("::")
    {
        return format!("::{rest}");
    }
    fqdn.to_string()
}

pub(super) fn success_json<T: Serialize>(value: &T) -> CallToolResult {
    match serde_json::to_string_pretty(value) {
        Ok(json) => CallToolResult::success(vec![Content::text(json)]),
        Err(e) => CallToolResult::error(vec![Content::text(format!(
            "failed to serialize tool result: {e}"
        ))]),
    }
}

pub(super) fn clamp_limit(raw: Option<u8>) -> u8 {
    raw.unwrap_or(FIND_SYMBOL_DEFAULT_LIMIT)
        .clamp(1, FIND_SYMBOL_MAX_LIMIT)
}

/// Wall-clock seconds since the Unix epoch. Cheap helper used by the
/// in-memory `recent_depth1` tracker — no need to drag a sessions
/// dependency in.
pub(super) fn current_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

pub(super) fn indexing_in_progress_message(progress: Option<(u64, u64)>) -> String {
    match progress {
        Some((done, total)) if total > 0 => format!(
            "Workspace indexing in progress ({done}/{total} files). Please retry in a few seconds."
        ),
        _ => "Workspace indexing in progress. Please retry in a few seconds.".to_string(),
    }
}

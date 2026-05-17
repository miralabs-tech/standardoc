//! Cross-workspace catalog ops and module-lookup queries — the query-layer
//! façade over `storage::workspace_catalog`, `storage::module_lookup`, and
//! `storage::cross_workspace`. MCP/LSP tools call into this, never into
//! the storage submodules directly.

use std::path::Path;

use standardoc_ir::{IndexingMode, LinkDirection, ModuleLookup, WorkspaceKind};
use strsim::jaro_winkler;

use crate::pipeline::peer_extract::{self, PeerExtractStats};
use crate::pipeline::{ColdStartError, LanguageProvider};
use crate::storage::cross_workspace::{CrossWorkspaceResolution, list_cross_workspace_providers};
use crate::storage::error::StorageError;
use crate::storage::handle::IndexHandle;
use crate::storage::module_lookup::{self, PRIMARY_WORKSPACE_ID};
use crate::storage::schema_meta;
use crate::storage::workspace_catalog::{self, LinkedWorkspace};

/// Strsim floor for path did-you-mean suggestions. Tuned to surface
/// likely typos (`projcts` → `projects`) without flooding with weakly
/// related names.
const PATH_DID_YOU_MEAN_THRESHOLD: f64 = 0.7;

/// Hard cap on suggestion count to keep tool responses compact.
const PATH_DID_YOU_MEAN_LIMIT: usize = 5;

/// Error variants returned by [`link_workspace`]. `PathNotFound` carries
/// a did-you-mean list computed from sibling directory entries so MCP /
/// LSP tools can surface them in a single round-trip.
#[derive(Debug)]
pub enum LinkWorkspaceError {
    PathNotFound {
        input: String,
        suggestions: Vec<String>,
    },
    Storage(StorageError),
}

impl From<StorageError> for LinkWorkspaceError {
    fn from(e: StorageError) -> Self {
        LinkWorkspaceError::Storage(e)
    }
}

impl std::fmt::Display for LinkWorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkWorkspaceError::PathNotFound { input, suggestions } => {
                if suggestions.is_empty() {
                    write!(f, "path not found: {input}")
                } else {
                    write!(
                        f,
                        "path not found: {input} (did you mean: {})",
                        suggestions.join(", ")
                    )
                }
            }
            LinkWorkspaceError::Storage(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for LinkWorkspaceError {}

/// Walk UP `input` until an existing ancestor directory is found, then
/// jaro-winkler match sibling directory names against the first missing
/// component (case-insensitive). Returns the top-N rebuilt paths whose
/// score meets [`PATH_DID_YOU_MEAN_THRESHOLD`].
///
/// Returns an empty vec when no existing ancestor is reachable or no
/// sibling clears the threshold.
pub fn path_did_you_mean(input: &Path) -> Vec<String> {
    let mut cursor = input;
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    let existing = loop {
        if cursor.is_dir() {
            break Some(cursor);
        }
        match (cursor.parent(), cursor.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name.to_os_string());
                cursor = parent;
            }
            _ => break None,
        }
    };
    let Some(existing) = existing else {
        return Vec::new();
    };
    let Some(needle_os) = missing.last() else {
        return Vec::new();
    };
    let needle = needle_os.to_string_lossy().to_lowercase();
    let Ok(entries) = std::fs::read_dir(existing) else {
        return Vec::new();
    };
    let mut scored: Vec<(String, f64)> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .map(|name| {
            let score = jaro_winkler(&needle, &name.to_lowercase());
            (name, score)
        })
        .filter(|(_, s)| *s >= PATH_DID_YOU_MEAN_THRESHOLD)
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored
        .into_iter()
        .take(PATH_DID_YOU_MEAN_LIMIT)
        .map(|(name, _)| {
            let mut rebuilt = existing.to_path_buf();
            rebuilt.push(name);
            for component in missing.iter().rev().skip(1) {
                rebuilt.push(component);
            }
            rebuilt.display().to_string()
        })
        .collect()
}

/// Register a linked workspace. Canonicalises `root_path` before storing;
/// on filesystem error returns [`LinkWorkspaceError::PathNotFound`] with
/// did-you-mean suggestions.
///
/// `indexing_mode` controls which extraction pipeline `cold_start`
/// (and future explicit refresh hooks) routes this peer through —
/// [`IndexingMode::BlobImport`] (Stage 3b-7-a, cheap blob copy of the
/// peer's pre-built DB) or [`IndexingMode::Extract`] (Stage 3b-7-b,
/// primary walks the peer's source files autonomously). Callers that
/// pre-date the choice can pass `IndexingMode::default()`
/// (= `BlobImport`).
pub fn link_workspace(
    handle: &IndexHandle,
    root_path: &str,
    direction: LinkDirection,
    indexing_mode: IndexingMode,
) -> Result<String, LinkWorkspaceError> {
    let raw = Path::new(root_path);
    let canonical = std::fs::canonicalize(raw).map_err(|_| LinkWorkspaceError::PathNotFound {
        input: root_path.to_string(),
        suggestions: path_did_you_mean(raw),
    })?;
    let canon_str = canonical.to_string_lossy();
    let pool = handle.pool().map_err(StorageError::from)?;
    let conn = pool.get().map_err(StorageError::from)?;
    Ok(workspace_catalog::register_linked_workspace(
        &conn,
        &canon_str,
        direction,
        indexing_mode,
    )?)
}

pub fn unlink_workspace(handle: &IndexHandle, workspace_id: &str) -> Result<(), StorageError> {
    let pool = handle.pool()?;
    let conn = pool.get()?;
    workspace_catalog::unregister_linked_workspace(&conn, workspace_id)
}

/// Stage 3b-7-b L3-bis: explicit re-extraction of a single linked peer.
/// Looks up the peer by `workspace_id`, then delegates to
/// [`peer_extract::extract_peer_workspace`] — same code path cold_start
/// uses, but scoped to one peer rather than the full sweep. Intended
/// as the user-facing escape hatch for the Q4 staleness gap: peer
/// source can drift between cold_starts, and the watcher (L3d) only
/// catches changes that occur while the daemon is up.
pub fn refresh_peer(
    handle: &IndexHandle,
    provider: &dyn LanguageProvider,
    workspace_id: &str,
) -> Result<PeerExtractStats, RefreshPeerError> {
    let peer = {
        let pool = handle.pool().map_err(StorageError::from)?;
        let conn = pool.get().map_err(StorageError::from)?;
        workspace_catalog::get_linked_workspace(&conn, workspace_id)?
            .ok_or_else(|| RefreshPeerError::NotFound(workspace_id.to_string()))?
    };
    Ok(peer_extract::extract_peer_workspace(
        handle, &peer, provider,
    )?)
}

/// Error variants for [`refresh_peer`]. `NotFound` is its own variant
/// (vs. being wrapped in StorageError) so MCP / LSP callers can map
/// it to an `invalid_params` response with the offending workspace_id.
#[derive(Debug, thiserror::Error)]
pub enum RefreshPeerError {
    #[error("workspace_id not found: {0}")]
    NotFound(String),
    #[error("storage: {0}")]
    Storage(#[from] StorageError),
    #[error("extract: {0}")]
    Extract(#[from] ColdStartError),
}

/// Outcome of [`set_link_direction`]. Carries both the previous and the
/// new direction so the caller (MCP handler) can decide whether the
/// transition crosses the watch boundary (`Out ↔ {In, Bidirectional}`)
/// and react accordingly. `root_path` is surfaced so the watcher's
/// `add_peer` call has the path it needs without re-querying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetLinkDirectionOutcome {
    pub workspace_id: String,
    pub root_path: String,
    pub previous_direction: LinkDirection,
    pub new_direction: LinkDirection,
}

/// Error variants for [`set_link_direction`]. Mirrors [`RefreshPeerError`].
#[derive(Debug, thiserror::Error)]
pub enum SetLinkDirectionError {
    #[error("workspace_id not found: {0}")]
    NotFound(String),
    #[error("storage: {0}")]
    Storage(#[from] StorageError),
}

/// Update the link direction of a registered peer. Returns the
/// previous direction in the [`SetLinkDirectionOutcome`] so the
/// caller can detect transitions that cross the watch boundary
/// (`Out ↔ {In, Bidirectional}`) and propagate them to the live
/// watcher. Idempotent at the catalog layer: setting the same
/// direction twice is a no-op write (the UPDATE matches but
/// changes nothing).
pub fn set_link_direction(
    handle: &IndexHandle,
    workspace_id: &str,
    new_direction: LinkDirection,
) -> Result<SetLinkDirectionOutcome, SetLinkDirectionError> {
    let pool = handle.pool().map_err(StorageError::from)?;
    let conn = pool.get().map_err(StorageError::from)?;
    let peer = workspace_catalog::get_linked_workspace(&conn, workspace_id)?
        .ok_or_else(|| SetLinkDirectionError::NotFound(workspace_id.to_string()))?;
    let previous_direction = peer.link_direction;
    workspace_catalog::set_link_direction(&conn, workspace_id, new_direction)?;
    Ok(SetLinkDirectionOutcome {
        workspace_id: workspace_id.to_string(),
        root_path: peer.root_path,
        previous_direction,
        new_direction,
    })
}

pub fn list_linked_workspaces(handle: &IndexHandle) -> Result<Vec<LinkedWorkspace>, StorageError> {
    let pool = handle.pool()?;
    let conn = pool.get()?;
    workspace_catalog::list_linked_workspaces(&conn)
}

/// Stage 3e-3 — fetch the primary workspace's persisted [`WorkspaceKind`].
/// Returns `Ok(None)` when discovery hasn't run yet (fresh DB, pre-3e-3
/// database, or first cold-start in progress) AND when discovery ran
/// but no workspace manifest was detected at the root (loose project
/// tree / single-crate layout). MCP / LSP consumers surface the `None`
/// case as a literal `null` field rather than guessing a sentinel.
pub fn read_primary_workspace_kind(
    handle: &IndexHandle,
) -> Result<Option<WorkspaceKind>, StorageError> {
    let pool = handle.pool()?;
    let conn = pool.get()?;
    schema_meta::read_workspace_kind(&conn)
}

/// Fetch the persisted `ModuleLookup` for `(workspace_id, module_fqdn)`.
/// `workspace_id` defaults to the [`PRIMARY_WORKSPACE_ID`] sentinel when
/// the caller omits it.
pub fn get_module_lookup(
    handle: &IndexHandle,
    workspace_id: Option<&str>,
    module_fqdn: &str,
) -> Result<Option<ModuleLookup>, StorageError> {
    let wid = workspace_id.unwrap_or(PRIMARY_WORKSPACE_ID);
    let pool = handle.pool()?;
    let conn = pool.get()?;
    module_lookup::get_module_lookup(&conn, wid, module_fqdn)
}

/// Persist (UPSERT) a `ModuleLookup` blob under `(workspace_id, module_fqdn)`.
/// `workspace_id` defaults to the [`PRIMARY_WORKSPACE_ID`] sentinel. This is
/// the write-side symmetric to [`get_module_lookup`]; the daemon's walk
/// pipeline uses it after the AOT pre-pass for each module, and Stage
/// 3b-6 tests use it to seed peer workspaces.
pub fn put_module_lookup(
    handle: &IndexHandle,
    workspace_id: Option<&str>,
    lookup: &ModuleLookup,
) -> Result<(), StorageError> {
    let wid = workspace_id.unwrap_or(PRIMARY_WORKSPACE_ID);
    let pool = handle.pool()?;
    let conn = pool.get()?;
    module_lookup::put_module_lookup(&conn, wid, lookup)
}

/// Enumerate every linked workspace that re-exports / declares
/// `(origin_module, origin_symbol)`. Returns an empty vec when no
/// linked workspace matches.
pub fn resolve_cross_workspace(
    handle: &IndexHandle,
    origin_module: &str,
    origin_symbol: &str,
) -> Result<Vec<CrossWorkspaceResolution>, StorageError> {
    let pool = handle.pool()?;
    let conn = pool.get()?;
    list_cross_workspace_providers(&conn, origin_module, origin_symbol)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn path_did_you_mean_returns_close_sibling_directory() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("projects")).unwrap();
        fs::create_dir(dir.path().join("docs")).unwrap();

        let typo = dir.path().join("projcts");
        let suggestions = path_did_you_mean(&typo);
        assert!(
            suggestions.iter().any(|s| s.ends_with("projects")),
            "expected `projects` in suggestions, got {suggestions:?}"
        );
    }

    #[test]
    fn path_did_you_mean_returns_empty_when_no_close_match() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("alpha")).unwrap();
        fs::create_dir(dir.path().join("beta")).unwrap();

        let nothing_close = dir.path().join("zzzzzz");
        let suggestions = path_did_you_mean(&nothing_close);
        assert!(
            suggestions.is_empty(),
            "expected no suggestions for far-removed name, got {suggestions:?}"
        );
    }

    #[test]
    fn path_did_you_mean_walks_up_to_first_existing_ancestor() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("workspace")).unwrap();
        fs::create_dir(dir.path().join("worskpace")).unwrap();

        // The user typed two missing components (workspce/foo/bar). We
        // expect suggestions to surface `workspace` and `worskpace` —
        // anchored at the existing tempdir, with the missing tail
        // re-appended after the corrected component.
        let bad = dir.path().join("workspce").join("foo");
        let suggestions = path_did_you_mean(&bad);
        assert!(
            suggestions.iter().any(|s| s.contains("workspace")),
            "expected `workspace` as a suggestion, got {suggestions:?}"
        );
    }

    #[test]
    fn path_did_you_mean_skips_files() {
        let dir = tempdir().unwrap();
        // A regular file with a name very close to the target — must be
        // excluded because we only suggest directories.
        fs::write(dir.path().join("projects"), "not a dir").unwrap();

        let typo = dir.path().join("projcts");
        let suggestions = path_did_you_mean(&typo);
        assert!(
            suggestions.is_empty(),
            "files must not appear in did-you-mean output, got {suggestions:?}"
        );
    }

    #[test]
    fn link_workspace_error_path_not_found_renders_did_you_mean() {
        let err = LinkWorkspaceError::PathNotFound {
            input: "/no/such/path".into(),
            suggestions: vec!["/no/such/path1".into(), "/no/such/path2".into()],
        };
        let rendered = format!("{err}");
        assert!(rendered.contains("did you mean"));
        assert!(rendered.contains("/no/such/path1"));
        assert!(rendered.contains("/no/such/path2"));
    }

    #[test]
    fn link_workspace_error_path_not_found_omits_section_when_no_suggestions() {
        let err = LinkWorkspaceError::PathNotFound {
            input: "/no/such/path".into(),
            suggestions: vec![],
        };
        let rendered = format!("{err}");
        assert!(!rendered.contains("did you mean"));
        assert!(rendered.contains("/no/such/path"));
    }

    // ────────────────────────────────────────────────────────────────
    // L3-bis-1: refresh_peer
    // ────────────────────────────────────────────────────────────────

    #[test]
    fn refresh_peer_returns_not_found_for_unknown_workspace_id() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        let provider = crate::pipeline::provider::mock::MockProvider::new();
        let err = refresh_peer(&handle, &provider, "no-such-uuid").unwrap_err();
        match err {
            RefreshPeerError::NotFound(id) => assert_eq!(id, "no-such-uuid"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    // ────────────────────────────────────────────────────────────────
    // post-3b-7-b: set_link_direction
    // ────────────────────────────────────────────────────────────────

    #[test]
    fn set_link_direction_returns_not_found_for_unknown_workspace_id() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        let err = set_link_direction(&handle, "no-such-uuid", LinkDirection::Out).unwrap_err();
        match err {
            SetLinkDirectionError::NotFound(id) => assert_eq!(id, "no-such-uuid"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn set_link_direction_round_trips_previous_and_new_direction() {
        // Link with direction=In, then flip to Out. Outcome must carry
        // both directions so the caller can detect the watch-boundary
        // crossing (In → Out means stop watching).
        let primary = tempdir().unwrap();
        let peer = tempdir().unwrap();
        let handle = IndexHandle::open(primary.path()).unwrap();
        let workspace_id = link_workspace(
            &handle,
            &peer.path().to_string_lossy(),
            LinkDirection::In,
            IndexingMode::default(),
        )
        .expect("link ok");

        let outcome = set_link_direction(&handle, &workspace_id, LinkDirection::Out).unwrap();
        assert_eq!(outcome.workspace_id, workspace_id);
        assert_eq!(outcome.previous_direction, LinkDirection::In);
        assert_eq!(outcome.new_direction, LinkDirection::Out);
        assert!(
            outcome.root_path.ends_with(
                &peer
                    .path()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            ) || outcome.root_path.contains(&*peer.path().to_string_lossy()),
            "root_path must surface canonical peer path, got {outcome:?}"
        );
    }

    #[test]
    fn set_link_direction_is_idempotent_for_same_direction() {
        // Setting the same direction twice must succeed (no error) and
        // report previous == new in the second call.
        let primary = tempdir().unwrap();
        let peer = tempdir().unwrap();
        let handle = IndexHandle::open(primary.path()).unwrap();
        let workspace_id = link_workspace(
            &handle,
            &peer.path().to_string_lossy(),
            LinkDirection::Bidirectional,
            IndexingMode::default(),
        )
        .expect("link ok");

        let outcome =
            set_link_direction(&handle, &workspace_id, LinkDirection::Bidirectional).unwrap();
        assert_eq!(outcome.previous_direction, LinkDirection::Bidirectional);
        assert_eq!(outcome.new_direction, LinkDirection::Bidirectional);
    }

    #[test]
    fn refresh_peer_round_trips_link_then_extracts_peer_source() {
        // Happy path: link a peer with direction=in + mode=extract, then
        // call refresh_peer with a stub provider that emits one symbol
        // per file. Check the returned stats AND that the peer row is
        // tagged with the peer's workspace_id (NOT 'primary').
        use crate::pipeline::provider::mock::{MockProvider, MockResponse};
        use standardoc_ir::{
            Blake3Hash, ExtractedFile, Kind, Language, LanguageKind, RawSymbol, SourceOrigin,
            SymbolLocation, Visibility,
        };

        let primary = tempdir().unwrap();
        let peer = tempdir().unwrap();
        fs::create_dir_all(peer.path().join("src")).unwrap();
        fs::write(
            peer.path().join("src").join("lib.rs"),
            "pub fn peer_only_marker() {}",
        )
        .unwrap();

        let handle = IndexHandle::open(primary.path()).unwrap();
        let workspace_id = link_workspace(
            &handle,
            &peer.path().to_string_lossy(),
            LinkDirection::In,
            IndexingMode::Extract,
        )
        .expect("link ok");

        let mock = MockProvider::new();
        let extracted = ExtractedFile {
            file: "src/lib.rs".into(),
            language: Language::Rust,
            source_origin: SourceOrigin::Workspace,
            is_external: false,
            content_hash: Blake3Hash::new([0xab; 32]),
            byte_size: 32,
            module_lookup: None,
            symbols: vec![RawSymbol {
                name: "peer_only_marker".into(),
                fqdn: "crate::peer_only_marker".into(),
                kind: Kind::Function,
                language_kind: LanguageKind::from("fn_item"),
                module: None,
                visibility: Visibility::Public,
                location: SymbolLocation {
                    file: "src/lib.rs".into(),
                    start_line: 1,
                    end_line: 1,
                    start_col: 0,
                    end_col: 0,
                },
                signature: None,
                body_hash: Some(Blake3Hash::new([0x01; 32])),
                attributes: vec![],
                flags: vec![],
            }],
            edges: vec![],
            call_sites: vec![],
            documents: vec![],
            ffi_bindings: vec![],
        };
        mock.set("src/lib.rs", MockResponse::Ok(extracted));

        let stats = refresh_peer(&handle, &mock, &workspace_id).expect("refresh_peer ok");
        assert_eq!(stats.workspace_id, workspace_id);
        assert_eq!(stats.status, peer_extract::PeerExtractStatus::Ok);
        assert_eq!(stats.files_extracted, 1);

        // Confirm the row landed under the peer's workspace_id, not 'primary'.
        let conn = handle.pool().unwrap().get().unwrap();
        let sym_workspace_id: String = conn
            .query_row(
                "SELECT workspace_id FROM symbols WHERE fqdn = 'crate::peer_only_marker'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sym_workspace_id, workspace_id);
    }
}

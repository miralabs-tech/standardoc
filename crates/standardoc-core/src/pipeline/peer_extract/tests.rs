use super::*;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

use crate::pipeline::provider::{ExtractContext, ExtractError, LanguageProvider};
use crate::storage::handle::IndexHandle;
use standardoc_ir::{
    BuiltinEntry, ExtractedFile, Kind, LanguageKind, LinkDirection, RawSymbol, SourceOrigin,
    SymbolLocation, Visibility,
};

/// Minimal LanguageProvider stub that emits one symbol per .rs file.
/// Avoids dragging the full `standardoc-lang-provider` crate into the
/// core's test deps (would be a workspace dep cycle).
struct StubProvider;

impl LanguageProvider for StubProvider {
    fn extract(
        &self,
        content: &str,
        path: &str,
        _ctx: &ExtractContext<'_>,
    ) -> Result<ExtractedFile, ExtractError> {
        // Synthesize one function symbol per file at line 1.
        let fqdn = format!("{path}::stub");
        let symbol = RawSymbol {
            decl_kind: None,
            implements_trait: None,
            receiver_type: None,
            entry_point: None,
            name: "stub".into(),
            fqdn,
            kind: Kind::Callable,
            language_kind: LanguageKind::from("fn"),
            module: None,
            visibility: Visibility::Public,
            location: SymbolLocation {
                file: path.into(),
                start_line: 1,
                end_line: 1,
                start_col: 0,
                end_col: 0,
            },
            signature: None,
            body_hash: Some(Blake3Hash::new(
                *blake3::hash(content.as_bytes()).as_bytes(),
            )),
            attributes: vec![],
            flags: vec![],
        };
        Ok(ExtractedFile {
            file: path.into(),
            language: standardoc_ir::Language::Rust,
            source_origin: SourceOrigin::Workspace,
            is_external: false,
            content_hash: Blake3Hash::new(*blake3::hash(content.as_bytes()).as_bytes()),
            byte_size: content.len() as u64,
            symbols: vec![symbol],
            edges: vec![],
            call_sites: vec![],
            documents: vec![],
            ffi_bindings: vec![],
            module_lookup: None,
        })
    }

    fn edge_builtins(&self) -> Vec<BuiltinEntry> {
        vec![]
    }
}

fn write_rs_file(dir: &Path, rel: &str, body: &str) -> PathBuf {
    let abs = dir.join(rel);
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&abs, body).unwrap();
    abs
}

fn linked_peer(workspace_id: &str, root: &Path) -> LinkedWorkspace {
    LinkedWorkspace {
        workspace_id: workspace_id.into(),
        root_path: root.to_string_lossy().into_owned(),
        link_direction: LinkDirection::In,
        linked_at_epoch_ms: 0,
        last_indexed_at_epoch_ms: None,
        status: LinkedWorkspaceStatus::Active,
        indexing_mode: standardoc_ir::IndexingMode::Extract,
    }
}

#[test]
fn peer_path_primary_round_trips_unchanged() {
    assert_eq!(peer_path(PRIMARY_WORKSPACE_ID, "src/lib.rs"), "src/lib.rs");
}

#[test]
fn peer_path_non_primary_carries_workspace_prefix() {
    assert_eq!(
        peer_path("peer-uuid-1", "src/lib.rs"),
        "ws:peer-uuid-1:src/lib.rs"
    );
}

#[test]
fn scope_extracted_paths_primary_is_no_op() {
    let mut ef = ExtractedFile {
        file: "src/lib.rs".into(),
        language: standardoc_ir::Language::Rust,
        source_origin: SourceOrigin::Workspace,
        is_external: false,
        content_hash: Blake3Hash::default(),
        byte_size: 0,
        symbols: vec![],
        edges: vec![],
        call_sites: vec![],
        documents: vec![],
        ffi_bindings: vec![],
        module_lookup: None,
    };
    scope_extracted_paths(&mut ef, PRIMARY_WORKSPACE_ID);
    assert_eq!(ef.file, "src/lib.rs", "primary must NOT be prefixed");
}

#[test]
fn scope_extracted_paths_peer_rewrites_every_embedded_path() {
    let mut ef = ExtractedFile {
        file: "src/lib.rs".into(),
        language: standardoc_ir::Language::Rust,
        source_origin: SourceOrigin::Workspace,
        is_external: false,
        content_hash: Blake3Hash::default(),
        byte_size: 0,
        module_lookup: None,
        symbols: vec![RawSymbol {
            decl_kind: None,
            implements_trait: None,
            receiver_type: None,
            entry_point: None,
            name: "f".into(),
            fqdn: "x::f".into(),
            kind: Kind::Callable,
            language_kind: LanguageKind::from("fn"),
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
            body_hash: None,
            attributes: vec![],
            flags: vec![],
        }],
        edges: vec![],
        call_sites: vec![],
        documents: vec![],
        ffi_bindings: vec![],
    };
    scope_extracted_paths(&mut ef, "peer-1");
    assert_eq!(ef.file, "ws:peer-1:src/lib.rs");
    assert_eq!(ef.symbols[0].location.file, "ws:peer-1:src/lib.rs");
}

#[test]
fn extract_peer_workspace_skipped_for_inactive_peer() {
    let primary_dir = tempfile::tempdir().unwrap();
    let peer_dir = tempfile::tempdir().unwrap();
    let handle = IndexHandle::open(primary_dir.path()).unwrap();
    let mut peer = linked_peer("peer-x", peer_dir.path());
    peer.status = LinkedWorkspaceStatus::Paused;
    let stats = extract_peer_workspace(&handle, &peer, &StubProvider).unwrap();
    assert_eq!(stats.status, PeerExtractStatus::SkippedInactive);
    assert_eq!(stats.files_extracted, 0);
}

#[test]
fn extract_peer_workspace_skipped_when_root_missing() {
    let primary_dir = tempfile::tempdir().unwrap();
    let handle = IndexHandle::open(primary_dir.path()).unwrap();
    let peer = linked_peer("peer-y", Path::new("/nonexistent/peer/path/xyz"));
    let stats = extract_peer_workspace(&handle, &peer, &StubProvider).unwrap();
    assert_eq!(stats.status, PeerExtractStatus::SkippedMissing);
}

#[test]
fn extract_peer_workspace_indexes_peer_files_under_peer_workspace_id() {
    let primary_dir = tempfile::tempdir().unwrap();
    let peer_dir: TempDir = tempfile::tempdir().unwrap();
    write_rs_file(peer_dir.path(), "src/lib.rs", "fn body() {}");
    write_rs_file(peer_dir.path(), "src/main.rs", "fn main() {}");

    let handle = IndexHandle::open(primary_dir.path()).unwrap();
    let peer = linked_peer("peer-zeta", peer_dir.path());
    let stats = extract_peer_workspace(&handle, &peer, &StubProvider).unwrap();

    assert_eq!(stats.status, PeerExtractStatus::Ok);
    assert_eq!(stats.files_extracted, 2);
    assert_eq!(stats.files_skipped_unchanged, 0);

    let conn = handle.pool().unwrap().get().unwrap();
    // Symbols land tagged with the peer's workspace_id.
    let peer_symbol_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM symbols WHERE workspace_id = ?1",
            ["peer-zeta"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(peer_symbol_count, 2);

    // Files land with scoped (prefixed) paths so they don't clash
    // with any future primary file at the same rel-path.
    let scoped_file_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path LIKE 'ws:peer-zeta:%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(scoped_file_count, 2);
}

#[test]
fn extract_peer_workspace_idempotent_skips_unchanged_files_on_second_run() {
    let primary_dir = tempfile::tempdir().unwrap();
    let peer_dir = tempfile::tempdir().unwrap();
    write_rs_file(peer_dir.path(), "src/lib.rs", "fn one() {}");
    let handle = IndexHandle::open(primary_dir.path()).unwrap();
    let peer = linked_peer("peer-idem", peer_dir.path());

    let first = extract_peer_workspace(&handle, &peer, &StubProvider).unwrap();
    assert_eq!(first.files_extracted, 1);
    assert_eq!(first.files_skipped_unchanged, 0);

    let second = extract_peer_workspace(&handle, &peer, &StubProvider).unwrap();
    assert_eq!(second.files_extracted, 0);
    assert_eq!(second.files_skipped_unchanged, 1);
}

#[test]
fn extract_peer_workspace_coexists_with_primary_same_rel_path() {
    // The scenario the whole scoping convention is built for:
    // primary AND peer both have `src/lib.rs`, both should index
    // without `files.path` PK collision.
    let primary_dir = tempfile::tempdir().unwrap();
    let peer_dir = tempfile::tempdir().unwrap();
    write_rs_file(primary_dir.path(), "src/lib.rs", "fn primary() {}");
    write_rs_file(peer_dir.path(), "src/lib.rs", "fn peer() {}");

    let handle = IndexHandle::open(primary_dir.path()).unwrap();
    // Seed primary's `src/lib.rs` row via the stub provider through
    // the standard apply_upsert_file path.
    {
        let primary_extracted = StubProvider
            .extract(
                "fn primary() {}",
                "src/lib.rs",
                &ExtractContext {
                    workspace_root: primary_dir.path(),
                    cross_workspace: None,
                },
            )
            .unwrap();
        let conn = handle.pool().unwrap().get().unwrap();
        apply_upsert_file(&conn, &primary_extracted, 0, PRIMARY_WORKSPACE_ID).unwrap();
    }

    let peer = linked_peer("peer-coex", peer_dir.path());
    let stats = extract_peer_workspace(&handle, &peer, &StubProvider).unwrap();
    assert_eq!(stats.status, PeerExtractStatus::Ok);
    assert_eq!(stats.files_extracted, 1);

    let conn = handle.pool().unwrap().get().unwrap();
    let total_files: i64 = conn
        .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total_files, 2, "primary + peer must coexist");
    // Primary row keeps unprefixed path.
    let primary_present: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path = ?1",
            ["src/lib.rs"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(primary_present, 1);
    // Peer row carries the workspace prefix.
    let peer_present: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path = ?1",
            ["ws:peer-coex:src/lib.rs"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(peer_present, 1);
}

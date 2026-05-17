use std::path::{Path, PathBuf};

use rayon::prelude::*;
use rusqlite::TransactionBehavior;
use walkdir::{DirEntry, WalkDir};

use standardoc_ir::IndexingMode;

use crate::pipeline::batch::apply_delete_file;
use crate::pipeline::filters::ScanFilters;
use crate::pipeline::paths::{has_supported_extension, to_workspace_relative};
use crate::pipeline::peer_extract;
use crate::pipeline::peer_import;
use crate::pipeline::projects::{discover_and_persist_projects, reconcile_files_project_id};
use crate::pipeline::provider::LanguageProvider;
use crate::pipeline::reindex::{Outcome, commit_outcomes, process_one};
use crate::pipeline::seed_builtins;
use crate::storage::error::StorageError;
use crate::storage::handle::IndexHandle;
use crate::storage::workspace_catalog::list_linked_workspaces;

pub use crate::pipeline::reindex::ColdStartError;

const SUB_BATCH_SIZE: usize = 100;

/// Runs the eager cold start: walks the workspace, extracts symbols for any
/// file whose `files.content_hash` does not match disk, and removes files
/// that disappeared since the last run.
///
/// Blocks the calling thread. The caller orchestrates threading (the server
/// crate post-beta.1 will spawn this on a tokio blocking thread). Must run
/// BEFORE `spawn_watcher` at boot — otherwise live FS events race the scan.
///
/// On crash mid-scan, `schema_meta.cold_start_progress` is left stale until
/// the next invocation, which resets it to `0/<total>` before walking. Skip
/// via `files.content_hash` makes the resume natural: already-ingested files
/// are detected and skipped without re-extraction.
///
/// Pause check at every chunk boundary returns `Ok(())` cleanly when
/// `handle.is_paused()` flips: progress stays stale, the next call resumes
/// via the `content_hash` skip path.
pub fn run(
    handle: &IndexHandle,
    provider: &dyn LanguageProvider,
    filters: &ScanFilters,
) -> Result<(), ColdStartError> {
    let workspace_root = handle.workspace_root().to_path_buf();

    // Stage 3d-2 — discover projects BEFORE walking files so the
    // post-walk reconcile step has everything it needs in one pass.
    // Detection errors are non-fatal: an empty `projects` table just
    // means `files.project_id` stays NULL and consumers degrade
    // gracefully (treat the workspace as a single anonymous project).
    discover_projects_quietly(handle, &workspace_root);

    // Stage 3e-1 — eagerly seed Edge-tier builtin symbols so the
    // synthetic FQDNs emitted by tier-aware resolvers
    // (`<builtin>::ts::Math`, `<builtin>::lua::print`, …) resolve
    // against a real `symbols.id` instead of staying as unresolved
    // canonicals. Best-effort: failures fall back to the pre-3e-1
    // behaviour (edges remain unresolved) without blocking cold start.
    let edge_builtins = provider.edge_builtins();
    seed_builtins::seed_quietly(handle, &edge_builtins);

    let candidates = collect_candidates(&workspace_root, filters)?;
    let total = u64_of(candidates.len());

    set_progress(handle, 0, total)?;

    let mut seen: Vec<String> = Vec::with_capacity(candidates.len());
    let mut done: u64 = 0;

    for chunk in candidates.chunks(SUB_BATCH_SIZE) {
        if handle.is_paused() {
            return Ok(());
        }
        let outcomes = process_chunk(chunk, &workspace_root, handle, provider)?;
        commit_outcomes(handle, &outcomes)?;

        for outcome in &outcomes {
            seen.push(outcome.rel().to_string());
        }

        done = done.saturating_add(u64_of(chunk.len()));
        set_progress(handle, done, total)?;
    }

    cleanup_unseen(handle, &seen)?;
    reconcile_projects_quietly(handle);
    // Stage 3b-7-b Layer 3c — per-peer dispatch. Runs AFTER primary
    // indexing is done so primary's data is always load-bearing; peer
    // ingestion (whether via 3b-7-a blob import or 3b-7-b source
    // extraction) enriches cross-workspace resolution as a bonus.
    // Best-effort like the discover / reconcile steps: failures don't
    // block cold start.
    process_peers_quietly(handle, provider);
    clear_progress(handle)?;
    Ok(())
}

/// Stage 3d-2 — best-effort discover + persist. Logged as a warning on
/// failure (typically a transient FS error during the walk); never
/// blocks cold start.
fn discover_projects_quietly(handle: &IndexHandle, workspace_root: &Path) {
    let pool = match handle.pool() {
        Ok(p) => p,
        Err(_) => return,
    };
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return,
    };
    let _ = discover_and_persist_projects(&conn, workspace_root);
}

/// Stage 3d-2 — best-effort reconcile of `files.project_id`. Same
/// graceful-degradation rationale as `discover_projects_quietly`.
fn reconcile_projects_quietly(handle: &IndexHandle) {
    let pool = match handle.pool() {
        Ok(p) => p,
        Err(_) => return,
    };
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return,
    };
    let _ = reconcile_files_project_id(&conn);
}

/// Stage 3b-7-b Layer 3c — best-effort per-peer dispatch.
///
/// Walks every linked workspace registered in `workspace_catalog` and
/// routes each peer through the pipeline its `indexing_mode` selects:
///
/// - [`IndexingMode::BlobImport`] (Stage 3b-7-a, default for legacy
///   rows) — copies the peer's pre-built `module_lookups` +
///   `workspace_imports` blobs into primary's DB. Cheap, assumes the
///   peer's DB is fresh + schema-compatible.
/// - [`IndexingMode::Extract`] (Stage 3b-7-b) — primary walks the
///   peer's source files via `peer_extract::extract_peer_workspace`
///   and indexes them autonomously under the peer's `workspace_id`.
///   Authoritative, no peer-side schema-version assumption.
///
/// Best-effort: a peer whose DB / source is missing or unreadable
/// gets logged as a no-op for that peer; other peers and the primary's
/// cold_start finish untouched.
fn process_peers_quietly(handle: &IndexHandle, provider: &dyn LanguageProvider) {
    let peers = {
        let Ok(pool) = handle.pool() else { return };
        let Ok(conn) = pool.get() else { return };
        match list_linked_workspaces(&conn) {
            Ok(p) => p,
            Err(_) => return,
        }
    };

    for peer in peers {
        match peer.indexing_mode {
            IndexingMode::BlobImport => {
                let Ok(pool) = handle.pool() else { continue };
                let Ok(mut conn) = pool.get() else { continue };
                let _ = peer_import::import_peer_workspace(&mut conn, &peer);
            }
            IndexingMode::Extract => {
                let _ = peer_extract::extract_peer_workspace(handle, &peer, provider);
            }
        }
    }
}

fn u64_of(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

pub(crate) fn collect_candidates(
    workspace_root: &Path,
    filters: &ScanFilters,
) -> Result<Vec<PathBuf>, ColdStartError> {
    let mut out = Vec::new();
    let walker = WalkDir::new(workspace_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !is_dir_excluded(entry, workspace_root, filters));

    for entry in walker {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        if !has_supported_extension(entry.path()) {
            continue;
        }
        let Some(rel) = to_workspace_relative(entry.path(), workspace_root) else {
            continue;
        };
        if filters.is_skipped(&rel) {
            continue;
        }
        out.push(entry.into_path());
    }
    Ok(out)
}

fn is_dir_excluded(entry: &DirEntry, workspace_root: &Path, filters: &ScanFilters) -> bool {
    if entry.depth() == 0 || !entry.file_type().is_dir() {
        return false;
    }
    let Some(rel) = to_workspace_relative(entry.path(), workspace_root) else {
        return false;
    };
    filters.is_skipped(&rel)
}

fn process_chunk(
    chunk: &[PathBuf],
    workspace_root: &Path,
    handle: &IndexHandle,
    provider: &dyn LanguageProvider,
) -> Result<Vec<Outcome>, ColdStartError> {
    let nested = chunk
        .par_iter()
        .map(|abs| process_one(abs, workspace_root, handle, provider))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(nested.into_iter().flatten().collect())
}

fn cleanup_unseen(handle: &IndexHandle, seen: &[String]) -> Result<(), ColdStartError> {
    let pool = handle.pool()?;
    let mut conn = pool.get().map_err(StorageError::from)?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(StorageError::from)?;
    tx.execute("DROP TABLE IF EXISTS seen_paths", [])
        .map_err(StorageError::from)?;
    tx.execute("CREATE TEMP TABLE seen_paths (path TEXT PRIMARY KEY)", [])
        .map_err(StorageError::from)?;
    {
        let mut stmt = tx
            .prepare("INSERT OR IGNORE INTO seen_paths (path) VALUES (?1)")
            .map_err(StorageError::from)?;
        for path in seen {
            stmt.execute([path]).map_err(StorageError::from)?;
        }
    }
    // Route every missing path through `apply_delete_file` so the per-symbol
    // reverse-promote step (`delete_symbol`) runs BEFORE the cascade FK
    // would set `edges.to_symbol_id = NULL`. A raw `DELETE FROM files`
    // here would let the cascade fire `ON DELETE SET NULL` on
    // `edges.to_symbol_id` while leaving `to_unresolved` untouched —
    // immediately violating the XOR CHECK constraint.
    //
    // `is_external = 0` filter (S3-G): external resolvers populate rows
    // whose path lives OUTSIDE the workspace tree (`~/.cargo/registry/...`,
    // `node_modules/...`). The workspace walk never sees those paths so a
    // naive cleanup would purge every cached external on every daemon
    // boot. The flag is set by external resolvers when they submit their
    // `ExtractedFile` and read here to skip cleanup.
    let missing: Vec<String> = {
        let mut stmt = tx
            .prepare(
                "SELECT path FROM files \
                 WHERE is_external = 0 \
                   AND path NOT IN (SELECT path FROM seen_paths)",
            )
            .map_err(StorageError::from)?;
        stmt.query_map([], |row| row.get::<_, String>(0))
            .map_err(StorageError::from)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(StorageError::from)?
    };
    for path in &missing {
        apply_delete_file(&tx, path)?;
    }
    let removed = missing.len();
    tx.commit().map_err(StorageError::from)?;
    conn.execute("DROP TABLE seen_paths", [])
        .map_err(StorageError::from)?;
    if removed > 0 {
        handle.bump_revision();
    }
    Ok(())
}

fn set_progress(handle: &IndexHandle, done: u64, total: u64) -> Result<(), ColdStartError> {
    write_progress_value(handle, &format!("{done}/{total}"))
}

fn clear_progress(handle: &IndexHandle) -> Result<(), ColdStartError> {
    write_progress_value(handle, "")
}

fn write_progress_value(handle: &IndexHandle, value: &str) -> Result<(), ColdStartError> {
    let pool = handle.pool()?;
    let conn = pool.get().map_err(StorageError::from)?;
    conn.execute(
        "UPDATE schema_meta SET value = ?1 WHERE key = 'cold_start_progress'",
        [value],
    )
    .map_err(StorageError::from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use standardoc_ir::{
        Blake3Hash, ExtractedFile, Kind, Language, LanguageKind, RawSymbol, SourceOrigin,
        SymbolLocation, Visibility,
    };
    use tempfile::tempdir;

    use super::*;
    use crate::pipeline::provider::mock::{MockProvider, MockResponse};

    fn sample_extracted(rel: &str, fqdn: &str) -> ExtractedFile {
        ExtractedFile {
            file: rel.into(),
            language: Language::Rust,
            source_origin: SourceOrigin::Workspace,
            is_external: false,
            content_hash: Blake3Hash::default(),
            byte_size: 100,
            symbols: vec![RawSymbol {
                name: fqdn.rsplit("::").next().unwrap_or(fqdn).into(),
                fqdn: fqdn.into(),
                kind: Kind::Function,
                language_kind: LanguageKind::from("fn_item"),
                module: None,
                visibility: Visibility::Public,
                location: SymbolLocation {
                    file: rel.into(),
                    start_line: 1,
                    end_line: 5,
                    start_col: 0,
                    end_col: 1,
                },
                signature: None,
                body_hash: Some(Blake3Hash::new([0x01; 32])),
                attributes: vec![],
                flags: vec![],
            }],
            edges: vec![],
            call_sites: vec![],
            documents: vec![],
        }
    }

    fn write_file(root: &Path, rel: &str, body: &[u8]) {
        let abs = root.join(rel);
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(abs, body).unwrap();
    }

    fn count_symbols(handle: &IndexHandle) -> i64 {
        let conn = handle.pool().unwrap().get().unwrap();
        conn.query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap()
    }

    fn count_files(handle: &IndexHandle) -> i64 {
        let conn = handle.pool().unwrap().get().unwrap();
        conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
            .unwrap()
    }

    fn read_progress(handle: &IndexHandle) -> String {
        let conn = handle.pool().unwrap().get().unwrap();
        conn.query_row(
            "SELECT value FROM schema_meta WHERE key = 'cold_start_progress'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn filters_for(handle: &IndexHandle) -> ScanFilters {
        ScanFilters::load(handle.workspace_root())
    }

    #[test]
    fn run_indexes_workspace_files_via_provider() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        write_file(handle.workspace_root(), "src/lib.rs", b"fn foo() {}");

        let mock = MockProvider::new();
        mock.set(
            "src/lib.rs",
            MockResponse::Ok(sample_extracted("src/lib.rs", "crate::foo")),
        );

        run(&handle, &mock, &filters_for(&handle)).unwrap();

        assert_eq!(count_symbols(&handle), 1);
        assert_eq!(read_progress(&handle), "");
        assert!(handle.revision() >= 1);
    }

    #[test]
    fn run_skips_files_with_matching_content_hash() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        let body = b"fn foo() {}";
        write_file(handle.workspace_root(), "src/lib.rs", body);

        let conn = handle.pool().unwrap().get().unwrap();
        let hash = Blake3Hash::new(*blake3::hash(body).as_bytes());
        let body_len = i64::try_from(body.len()).unwrap();
        conn.execute(
            "INSERT INTO files (path, content_hash, language, last_scanned, byte_size) \
             VALUES ('src/lib.rs', ?1, 'rust', 0, ?2)",
            rusqlite::params![hash.to_hex(), body_len],
        )
        .unwrap();
        drop(conn);

        let mock = MockProvider::new();
        run(&handle, &mock, &filters_for(&handle)).unwrap();

        assert_eq!(count_symbols(&handle), 0, "skipped path must not extract");
        assert_eq!(count_files(&handle), 1, "skipped path must remain in DB");
    }

    #[test]
    fn run_records_parse_errors_inline() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        write_file(handle.workspace_root(), "src/bad.rs", b"fn ???");

        let mock = MockProvider::new();
        mock.set(
            "src/bad.rs",
            MockResponse::ParseError("unexpected token".into()),
        );

        run(&handle, &mock, &filters_for(&handle)).unwrap();

        let conn = handle.pool().unwrap().get().unwrap();
        let last_error: Option<String> = conn
            .query_row(
                "SELECT last_scan_error FROM files WHERE path = 'src/bad.rs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(last_error.as_deref(), Some("unexpected token"));
        assert_eq!(count_symbols(&handle), 0);
    }

    #[test]
    fn run_skips_seeded_excluded_dirs() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        let root = handle.workspace_root().to_path_buf();
        write_file(&root, "target/debug/build.rs", b"fn target_noise() {}");
        write_file(&root, "node_modules/pkg/index.ts", b"export const x = 1;");
        write_file(&root, ".git/hooks/pre-commit.rs", b"fn hook() {}");

        let mock = MockProvider::new();
        run(&handle, &mock, &filters_for(&handle)).unwrap();

        assert_eq!(count_symbols(&handle), 0);
        assert_eq!(count_files(&handle), 0);
    }

    #[test]
    fn run_filters_unsupported_extensions() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        write_file(handle.workspace_root(), "notes.md", b"# hello");
        write_file(handle.workspace_root(), "script.py", b"print('hi')");

        let mock = MockProvider::new();
        run(&handle, &mock, &filters_for(&handle)).unwrap();

        assert_eq!(count_files(&handle), 0);
    }

    #[test]
    fn run_cleanup_removes_orphan_files() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();

        let conn = handle.pool().unwrap().get().unwrap();
        conn.execute(
            "INSERT INTO files (path, content_hash, language, last_scanned, byte_size) \
             VALUES ('src/gone.rs', 'aa', 'rust', 0, 0)",
            [],
        )
        .unwrap();
        drop(conn);

        let mock = MockProvider::new();
        run(&handle, &mock, &filters_for(&handle)).unwrap();

        assert_eq!(count_files(&handle), 0, "orphan file row must be deleted");
    }

    #[test]
    fn run_cleanup_reverse_promotes_inbound_resolved_edges_before_cascade_delete() {
        // Reproduces the bug where a raw `DELETE FROM files` cascade-set
        // `edges.to_symbol_id = NULL` while `to_unresolved` stayed NULL —
        // immediately violating the XOR CHECK constraint. The cleanup
        // must route through `apply_delete_file` so `delete_symbol`'s
        // reverse-promote step runs in the SAME transaction.
        use crate::pipeline::batch::apply_upsert_file;
        use crate::storage::module_lookup::PRIMARY_WORKSPACE_ID;
        use standardoc_ir::{EdgeConfidence, EdgeKind, RawEdge, ResolvedOrUnresolved};

        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();

        // Disk: only caller.rs survives — target.rs disappeared between
        // sessions. We compute the body's hash so cold_start matches
        // it against `files.content_hash` and emits `Outcome::Skip`,
        // landing caller.rs in `seen` without invoking the provider.
        let caller_body: &[u8] = b"fn caller(){}";
        let caller_hash = Blake3Hash::new(*blake3::hash(caller_body).as_bytes());
        write_file(handle.workspace_root(), "src/caller.rs", caller_body);

        // Seed both files in DB. caller.rs gets the disk-matching hash so
        // the cold_start hash check skips it; target.rs lives only in DB.
        let mut caller_extracted = ExtractedFile {
            file: "src/caller.rs".into(),
            language: Language::Rust,
            source_origin: SourceOrigin::Workspace,
            is_external: false,
            content_hash: caller_hash,
            byte_size: caller_body.len() as u64,
            symbols: vec![RawSymbol {
                name: "caller".into(),
                fqdn: "crate::caller".into(),
                kind: Kind::Function,
                language_kind: LanguageKind::from("fn_item"),
                module: None,
                visibility: Visibility::Public,
                location: SymbolLocation {
                    file: "src/caller.rs".into(),
                    start_line: 1,
                    end_line: 3,
                    start_col: 0,
                    end_col: 1,
                },
                signature: None,
                body_hash: Some(Blake3Hash::new([0x01; 32])),
                attributes: vec![],
                flags: vec![],
            }],
            edges: vec![RawEdge {
                from_fqdn: "crate::caller".into(),
                kind: EdgeKind::Calls,
                to: ResolvedOrUnresolved::Resolved {
                    fqdn: "crate::target".into(),
                },
                sites: vec![],
                attributes: vec![],
                confidence: EdgeConfidence::Extracted,
            }],
            call_sites: vec![],
            documents: vec![],
        };
        let target = sample_extracted("src/target.rs", "crate::target");
        {
            let conn = handle.pool().unwrap().get().unwrap();
            apply_upsert_file(&conn, &target, 0, PRIMARY_WORKSPACE_ID).unwrap();
            apply_upsert_file(&conn, &caller_extracted, 0, PRIMARY_WORKSPACE_ID).unwrap();
            // After insert, the resolved-on-insert promotion should have
            // linked the edge to target's symbol id — sanity-check before
            // the cleanup so we know the cascade has something to chew.
            let to_id: Option<i64> = conn
                .query_row("SELECT to_symbol_id FROM edges", [], |r| r.get(0))
                .unwrap();
            assert!(
                to_id.is_some(),
                "test setup: caller→target edge must be Resolved before cleanup"
            );
        }
        // Silence unused-mut warning for the now-immutable extracted form.
        caller_extracted.symbols.clear();

        let mock = MockProvider::new();
        run(&handle, &mock, &filters_for(&handle)).unwrap();

        // target.rs row gone, caller.rs row preserved.
        let conn = handle.pool().unwrap().get().unwrap();
        let remaining_files: Vec<String> = conn
            .prepare("SELECT path FROM files ORDER BY path")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(remaining_files, vec!["src/caller.rs"]);

        // Caller's outbound edge survived as Unresolved with the deleted
        // target's fqdn — the XOR invariant held throughout the cascade.
        let (to_id, to_unresolved): (Option<i64>, Option<String>) = conn
            .query_row("SELECT to_symbol_id, to_unresolved FROM edges", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(to_id, None);
        assert_eq!(to_unresolved.as_deref(), Some("crate::target"));
    }

    #[test]
    fn run_clears_progress_at_end() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        write_file(handle.workspace_root(), "src/lib.rs", b"fn foo() {}");

        let mock = MockProvider::new();
        mock.set(
            "src/lib.rs",
            MockResponse::Ok(sample_extracted("src/lib.rs", "crate::foo")),
        );
        run(&handle, &mock, &filters_for(&handle)).unwrap();

        assert_eq!(read_progress(&handle), "");
        assert_eq!(handle.cold_start_progress().unwrap(), None);
    }

    #[test]
    fn run_resume_skips_already_indexed_files() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        write_file(handle.workspace_root(), "src/lib.rs", b"fn foo() {}");

        let mock = MockProvider::new();
        mock.set(
            "src/lib.rs",
            MockResponse::Ok(sample_extracted("src/lib.rs", "crate::foo")),
        );
        run(&handle, &mock, &filters_for(&handle)).unwrap();
        let revision_after_first = handle.revision();
        assert_eq!(count_symbols(&handle), 1);

        run(&handle, &mock, &filters_for(&handle)).unwrap();

        assert_eq!(count_symbols(&handle), 1);
        assert!(
            handle.revision() == revision_after_first,
            "second run on unchanged content must not bump revision"
        );
    }

    #[test]
    fn run_empty_workspace_clears_progress() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();

        let mock = MockProvider::new();
        run(&handle, &mock, &filters_for(&handle)).unwrap();

        assert_eq!(read_progress(&handle), "");
    }

    #[test]
    fn run_indexes_multiple_files_and_keeps_them() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        write_file(handle.workspace_root(), "src/a.rs", b"fn a() {}");
        write_file(handle.workspace_root(), "src/b.rs", b"fn b() {}");
        write_file(handle.workspace_root(), "src/nested/c.rs", b"fn c() {}");

        let mock = MockProvider::new();
        mock.set(
            "src/a.rs",
            MockResponse::Ok(sample_extracted("src/a.rs", "crate::a")),
        );
        mock.set(
            "src/b.rs",
            MockResponse::Ok(sample_extracted("src/b.rs", "crate::b")),
        );
        mock.set(
            "src/nested/c.rs",
            MockResponse::Ok(sample_extracted("src/nested/c.rs", "crate::nested::c")),
        );

        run(&handle, &mock, &filters_for(&handle)).unwrap();

        assert_eq!(count_symbols(&handle), 3);
        assert_eq!(count_files(&handle), 3);
    }

    #[test]
    fn run_aborts_when_paused_before_first_chunk() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        write_file(handle.workspace_root(), "src/lib.rs", b"fn foo() {}");

        let mock = MockProvider::new();
        mock.set(
            "src/lib.rs",
            MockResponse::Ok(sample_extracted("src/lib.rs", "crate::foo")),
        );

        handle.pause();
        run(&handle, &mock, &filters_for(&handle)).unwrap();

        assert_eq!(count_symbols(&handle), 0, "paused run must not index");
        assert_eq!(count_files(&handle), 0);
    }

    #[test]
    fn run_resumes_after_pause_via_content_hash_skip() {
        let dir = tempdir().unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        write_file(handle.workspace_root(), "src/lib.rs", b"fn foo() {}");

        let mock = MockProvider::new();
        mock.set(
            "src/lib.rs",
            MockResponse::Ok(sample_extracted("src/lib.rs", "crate::foo")),
        );

        run(&handle, &mock, &filters_for(&handle)).unwrap();
        let revision_after_first = handle.revision();
        assert_eq!(count_symbols(&handle), 1);

        handle.pause();
        run(&handle, &mock, &filters_for(&handle)).unwrap();
        assert_eq!(handle.revision(), revision_after_first);

        handle.resume();
        run(&handle, &mock, &filters_for(&handle)).unwrap();
        assert_eq!(
            count_symbols(&handle),
            1,
            "resume must not double-index already-hashed files"
        );
    }

    #[test]
    fn run_excludes_paths_via_filters() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join(".stdignore"),
            "# custom\nsrc/excluded/\n.git/\ntarget/\nnode_modules/\n",
        )
        .unwrap();
        let handle = IndexHandle::open(dir.path()).unwrap();
        write_file(handle.workspace_root(), "src/keep.rs", b"fn keep() {}");
        write_file(
            handle.workspace_root(),
            "src/excluded/skip.rs",
            b"fn skip() {}",
        );

        let mock = MockProvider::new();
        mock.set(
            "src/keep.rs",
            MockResponse::Ok(sample_extracted("src/keep.rs", "crate::keep")),
        );
        mock.set(
            "src/excluded/skip.rs",
            MockResponse::Ok(sample_extracted(
                "src/excluded/skip.rs",
                "crate::excluded::skip",
            )),
        );

        run(&handle, &mock, &filters_for(&handle)).unwrap();

        let conn = handle.pool().unwrap().get().unwrap();
        let kept: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE path = 'src/keep.rs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let excluded: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE path = 'src/excluded/skip.rs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept, 1);
        assert_eq!(excluded, 0, "filtered subtree must not be indexed");
    }

    #[test]
    fn run_populates_projects_and_attaches_file_project_id() {
        let dir = tempdir().unwrap();
        // Workspace root is a Rust project; ext/vscode is a Bun
        // sub-project. We expect cold-start to pick both up and the
        // src/lib.rs file to land on the root project's id.
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"root\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("ext/vscode")).unwrap();
        fs::write(
            dir.path().join("ext/vscode/package.json"),
            "{\"name\":\"ext\"}",
        )
        .unwrap();
        fs::write(dir.path().join("ext/vscode/bun.lock"), "").unwrap();

        let handle = IndexHandle::open(dir.path()).unwrap();
        write_file(handle.workspace_root(), "src/lib.rs", b"fn main() {}");

        let mock = MockProvider::new();
        mock.set(
            "src/lib.rs",
            MockResponse::Ok(sample_extracted("src/lib.rs", "root::main")),
        );

        run(&handle, &mock, &filters_for(&handle)).unwrap();

        let conn = handle.pool().unwrap().get().unwrap();
        let project_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0))
            .unwrap();
        assert!(
            project_count >= 2,
            "expected at least root + ext/vscode projects, got {project_count}"
        );

        // The Rust file under src/ must be attached to the root project
        // (rel_path = '').
        let root_proj_id: i64 = conn
            .query_row(
                "SELECT project_id FROM projects WHERE rel_path = ''",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let file_pid: Option<i64> = conn
            .query_row(
                "SELECT project_id FROM files WHERE path = 'src/lib.rs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            file_pid,
            Some(root_proj_id),
            "src/lib.rs must be attached to the root project"
        );
    }
}

use std::path::{Path, PathBuf};

use rayon::prelude::*;
use rusqlite::TransactionBehavior;
use walkdir::{DirEntry, WalkDir};

use crate::pipeline::filters::ScanFilters;
use crate::pipeline::paths::{has_supported_extension, to_workspace_relative};
use crate::pipeline::provider::LanguageProvider;
use crate::pipeline::reindex::{Outcome, commit_outcomes, process_one};
use crate::storage::error::StorageError;
use crate::storage::handle::IndexHandle;

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
    clear_progress(handle)?;
    Ok(())
}

fn u64_of(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

fn collect_candidates(
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
    let removed = tx
        .execute(
            "DELETE FROM files WHERE path NOT IN (SELECT path FROM seen_paths)",
            [],
        )
        .map_err(StorageError::from)?;
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
            }],
            edges: vec![],
            call_sites: vec![],
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
}


use std::time::{Duration, Instant};

use standardoc_ir::{
    Blake3Hash, ExtractedFile, Kind, Language, LanguageKind, RawSymbol, SourceOrigin,
    SymbolLocation, Visibility,
};
use tempfile::tempdir;

use super::*;

fn read_meta(handle: &IndexHandle, key: &str) -> String {
    let conn = handle.pool().unwrap().get().unwrap();
    conn.query_row(
        "SELECT value FROM schema_meta WHERE key = ?1",
        [key],
        |row| row.get(0),
    )
    .unwrap()
}

fn sample_extracted(file: &str, fqdn: &str) -> ExtractedFile {
    ExtractedFile {
        file: file.into(),
        language: Language::Rust,
        source_origin: SourceOrigin::Workspace,
        is_external: false,
        content_hash: Blake3Hash::new([0xab; 32]),
        byte_size: 100,
        module_lookup: None,
        symbols: vec![RawSymbol {
            decl_kind: None,
            implements_trait: None,
            receiver_type: None,
            entry_point: None,
            name: fqdn.rsplit("::").next().unwrap_or(fqdn).into(),
            fqdn: fqdn.into(),
            kind: Kind::Callable,
            language_kind: LanguageKind::from("fn_item"),
            module: None,
            visibility: Visibility::Public,
            location: SymbolLocation {
                file: file.into(),
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
        ffi_bindings: vec![],
    }
}

fn wait_revision_at_least(handle: &IndexHandle, target: u64, timeout: Duration) {
    let start = Instant::now();
    while handle.revision() < target {
        assert!(
            start.elapsed() <= timeout,
            "revision did not reach {target} within {timeout:?} (was {})",
            handle.revision()
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn open_creates_standardoc_dir_and_db_files() {
    let dir = tempdir().unwrap();
    let _handle = IndexHandle::open(dir.path()).unwrap();
    assert!(dir.path().join(".standardoc/index.db").exists());
    assert!(dir.path().join(".standardoc/db.lock").exists());
}

#[test]
fn open_seeds_workspace_root() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    let stored = read_meta(&handle, "workspace_root");
    assert!(!stored.is_empty(), "workspace_root must be seeded");
}

#[test]
fn open_seeds_created_at_with_unix_epoch_ms() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    let stored = read_meta(&handle, "created_at");
    let parsed: u128 = stored.parse().expect("created_at must parse as u128");
    assert!(parsed > 0);
}

#[test]
fn second_open_on_same_workspace_fails_with_lock_held() {
    let dir = tempdir().unwrap();
    let _handle1 = IndexHandle::open(dir.path()).unwrap();
    let result = IndexHandle::open(dir.path());
    assert!(matches!(result, Err(StorageError::LockHeld { .. })));
}

#[test]
fn handle_clone_shares_pool() {
    let dir = tempdir().unwrap();
    let h1 = IndexHandle::open(dir.path()).unwrap();
    let h2 = h1.clone();
    let _c1 = h1.pool().unwrap().get().unwrap();
    let _c2 = h2.pool().unwrap().get().unwrap();
}

#[test]
fn reopen_after_drop_preserves_created_at() {
    let dir = tempdir().unwrap();
    let stored1 = {
        let handle = IndexHandle::open(dir.path()).unwrap();
        read_meta(&handle, "created_at")
    };
    std::thread::sleep(Duration::from_millis(2));
    let handle2 = IndexHandle::open(dir.path()).unwrap();
    let stored2 = read_meta(&handle2, "created_at");
    assert_eq!(stored1, stored2, "created_at must persist across reopens");
}

#[test]
fn open_returns_workspace_root_canonicalized() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    assert!(handle.workspace_root().is_absolute());
}

#[test]
fn rescan_from_scratch_on_fresh_db_succeeds() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    handle.rescan_from_scratch().unwrap();
    let _conn = handle.pool().unwrap().get().unwrap();
}

#[test]
fn rescan_from_scratch_wipes_existing_data() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        conn.execute(
            "INSERT INTO files (path, content_hash, language, last_scanned, byte_size) \
                 VALUES ('src/lib.rs', 'aa', 'rust', 0, 0)",
            [],
        )
        .unwrap();
    }
    handle.rescan_from_scratch().unwrap();
    let conn = handle.pool().unwrap().get().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0, "rescan must wipe existing data");
}

#[test]
fn rescan_from_scratch_reseeds_created_at_strictly_greater() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    let before: u128 = read_meta(&handle, "created_at").parse().unwrap();

    std::thread::sleep(Duration::from_millis(2));
    handle.rescan_from_scratch().unwrap();

    let after: u128 = read_meta(&handle, "created_at").parse().unwrap();
    assert!(
        after > before,
        "rescan must reseed created_at to a newer epoch (before={before}, after={after})"
    );
}

#[test]
fn try_submit_upsert_then_revision_bumps() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    assert_eq!(handle.revision(), 0);

    handle
        .try_submit(IngestCommand::UpsertFile {
            path: "src/main.rs".into(),
            extracted: sample_extracted("src/main.rs", "crate::foo"),
        })
        .unwrap();
    wait_revision_at_least(&handle, 1, Duration::from_secs(2));

    let conn = handle.pool().unwrap().get().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn submit_blocking_succeeds_on_open_handle() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    handle
        .submit_blocking(IngestCommand::UpsertFile {
            path: "src/main.rs".into(),
            extracted: sample_extracted("src/main.rs", "crate::bar"),
        })
        .unwrap();
    wait_revision_at_least(&handle, 1, Duration::from_secs(2));
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_async_reaches_writer_thread() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    handle
        .submit(IngestCommand::UpsertFile {
            path: "src/main.rs".into(),
            extracted: sample_extracted("src/main.rs", "crate::async_baz"),
        })
        .await
        .unwrap();
    wait_revision_at_least(&handle, 1, Duration::from_secs(2));

    let conn = handle.pool().unwrap().get().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn delete_file_via_submit_removes_data() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    handle
        .submit_blocking(IngestCommand::UpsertFile {
            path: "src/main.rs".into(),
            extracted: sample_extracted("src/main.rs", "crate::foo"),
        })
        .unwrap();
    wait_revision_at_least(&handle, 1, Duration::from_secs(2));

    handle
        .submit_blocking(IngestCommand::DeleteFile {
            path: "src/main.rs".into(),
        })
        .unwrap();
    wait_revision_at_least(&handle, 2, Duration::from_secs(2));

    let conn = handle.pool().unwrap().get().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn rescan_command_does_not_bump_revision_in_14a() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    handle
        .submit_blocking(IngestCommand::RescanFromScratch)
        .unwrap();
    handle
        .submit_blocking(IngestCommand::UpsertFile {
            path: "src/main.rs".into(),
            extracted: sample_extracted("src/main.rs", "crate::foo"),
        })
        .unwrap();
    wait_revision_at_least(&handle, 1, Duration::from_secs(2));
    assert_eq!(
        handle.revision(),
        1,
        "RescanFromScratch command is unimplemented in 14a (returns Err)"
    );
}

#[test]
fn cold_start_progress_returns_none_on_empty_meta() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    assert_eq!(handle.cold_start_progress().unwrap(), None);
}

#[test]
fn cold_start_progress_parses_done_over_total() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        conn.execute(
            "UPDATE schema_meta SET value = '42/100' WHERE key = 'cold_start_progress'",
            [],
        )
        .unwrap();
    }
    assert_eq!(handle.cold_start_progress().unwrap(), Some((42, 100)));
}

#[test]
fn cold_start_progress_returns_invalid_stored_data_on_garbage() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    {
        let conn = handle.pool().unwrap().get().unwrap();
        conn.execute(
            "UPDATE schema_meta SET value = 'lol' WHERE key = 'cold_start_progress'",
            [],
        )
        .unwrap();
    }
    let err = handle.cold_start_progress().unwrap_err();
    assert!(matches!(err, StorageError::InvalidStoredData { .. }));
}

#[test]
fn drop_last_handle_joins_writer_thread() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    handle
        .submit_blocking(IngestCommand::UpsertFile {
            path: "src/main.rs".into(),
            extracted: sample_extracted("src/main.rs", "crate::foo"),
        })
        .unwrap();
    wait_revision_at_least(&handle, 1, Duration::from_secs(2));
    drop(handle);
}

#[test]
fn open_seeds_stdignore_when_absent() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();

    let stdignore = handle.workspace_root().join(".stdignore");
    let body = std::fs::read_to_string(&stdignore).unwrap();
    assert!(body.contains(".git/"));
    assert!(body.contains("target/"));
    assert!(body.contains("node_modules/"));
    assert!(
        !body.contains(".standardoc/"),
        ".stdignore seed must not include .standardoc/ (lock 21 Q3)"
    );
}

#[test]
fn open_preserves_existing_stdignore() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(".stdignore");
    let custom = "# user authored\nfoo/\n!foo/keep.rs\n";
    std::fs::write(&path, custom).unwrap();

    let _handle = IndexHandle::open(dir.path()).unwrap();

    let body = std::fs::read_to_string(&path).unwrap();
    assert_eq!(body, custom);
}

#[test]
fn is_paused_default_false() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    assert!(!handle.is_paused());
}

#[test]
fn pause_resume_round_trips() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();

    handle.pause();
    assert!(handle.is_paused());

    handle.resume();
    assert!(!handle.is_paused());

    handle.pause();
    handle.pause();
    assert!(handle.is_paused(), "double pause is idempotent");
    handle.resume();
    assert!(!handle.is_paused());
}

fn seed_file(handle: &IndexHandle, path: &str) {
    let conn = handle.pool().unwrap().get().unwrap();
    conn.execute(
        "INSERT INTO files (path, content_hash, language, last_scanned, byte_size) \
             VALUES (?1, 'aa', 'rust', 0, 0)",
        [path],
    )
    .unwrap();
}

fn count_files(handle: &IndexHandle) -> i64 {
    let conn = handle.pool().unwrap().get().unwrap();
    conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
        .unwrap()
}

fn write_stdignore(root: &Path, body: &str) {
    std::fs::write(root.join(".stdignore"), body).unwrap();
}

#[test]
fn list_paths_matching_ignore_returns_only_matched() {
    let dir = tempdir().unwrap();
    write_stdignore(dir.path(), "target/\n");
    let handle = IndexHandle::open(dir.path()).unwrap();
    seed_file(&handle, "src/lib.rs");
    seed_file(&handle, "target/debug/build.rs");
    seed_file(&handle, "target/release/foo.rs");

    let filters = ScanFilters::load(handle.workspace_root());
    let mut matched = handle.list_paths_matching_ignore(&filters).unwrap();
    matched.sort();

    assert_eq!(
        matched,
        vec![
            "target/debug/build.rs".to_string(),
            "target/release/foo.rs".to_string(),
        ]
    );
}

#[test]
fn list_paths_matching_ignore_empty_when_no_match() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    seed_file(&handle, "src/lib.rs");

    let filters = ScanFilters::load(handle.workspace_root());
    assert!(
        handle
            .list_paths_matching_ignore(&filters)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn delete_paths_removes_files_and_bumps_revision() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    seed_file(&handle, "src/a.rs");
    seed_file(&handle, "src/b.rs");
    let revision_before = handle.revision();

    handle
        .delete_paths(&["src/a.rs".into(), "src/b.rs".into()])
        .unwrap();

    assert_eq!(count_files(&handle), 0);
    assert!(handle.revision() > revision_before);
}

#[test]
fn delete_paths_cascades_symbols() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    handle
        .submit_blocking(IngestCommand::UpsertFile {
            path: "src/main.rs".into(),
            extracted: sample_extracted("src/main.rs", "crate::foo"),
        })
        .unwrap();
    wait_revision_at_least(&handle, 1, Duration::from_secs(2));

    handle.delete_paths(&["src/main.rs".into()]).unwrap();

    let conn = handle.pool().unwrap().get().unwrap();
    let symbols: i64 = conn
        .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
        .unwrap();
    assert_eq!(symbols, 0, "FK cascade must drop symbols");
    assert_eq!(count_files(&handle), 0);
}

#[test]
fn delete_paths_no_op_on_empty_input() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    seed_file(&handle, "src/a.rs");
    let revision_before = handle.revision();

    handle.delete_paths(&[]).unwrap();

    assert_eq!(count_files(&handle), 1);
    assert_eq!(
        handle.revision(),
        revision_before,
        "empty input must not bump revision"
    );
}

#[test]
fn delete_paths_does_not_bump_when_nothing_matches() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    let revision_before = handle.revision();

    handle.delete_paths(&["src/missing.rs".into()]).unwrap();

    assert_eq!(handle.revision(), revision_before);
}

#[test]
fn list_all_file_paths_returns_all_rows() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    seed_file(&handle, "src/a.rs");
    seed_file(&handle, "src/b.rs");
    seed_file(&handle, "target/c.rs");

    let mut paths = handle.list_all_file_paths().unwrap();
    paths.sort();

    assert_eq!(
        paths,
        vec![
            "src/a.rs".to_string(),
            "src/b.rs".to_string(),
            "target/c.rs".to_string(),
        ]
    );
}

#[test]
fn open_readonly_returns_err_when_db_missing() {
    let dir = tempdir().unwrap();
    let result = IndexHandle::open_readonly(dir.path());
    assert!(matches!(
        result,
        Err(StorageError::ReadOnlyMissingDatabase { .. })
    ));
}

#[test]
fn open_readonly_coexists_with_writer_without_lock_collision() {
    let dir = tempdir().unwrap();
    let writer = IndexHandle::open(dir.path()).unwrap();
    assert!(!writer.is_readonly());

    let reader = IndexHandle::open_readonly(dir.path()).unwrap();
    assert!(reader.is_readonly());

    let _conn = reader.pool().unwrap().get().unwrap();
    drop(reader);
    drop(writer);
}

#[test]
fn open_readonly_reads_data_written_by_writer_then_dropped() {
    let dir = tempdir().unwrap();
    {
        let handle = IndexHandle::open(dir.path()).unwrap();
        handle
            .submit_blocking(IngestCommand::UpsertFile {
                path: "src/main.rs".into(),
                extracted: sample_extracted("src/main.rs", "crate::ro_target"),
            })
            .unwrap();
        wait_revision_at_least(&handle, 1, Duration::from_secs(2));
    }

    let reader = IndexHandle::open_readonly(dir.path()).unwrap();
    let conn = reader.pool().unwrap().get().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1, "secondary handle must see writer-committed data");
}

#[test]
fn secondary_handle_can_write_alongside_primary() {
    // v6+ semantic shift: "readonly" handles are R/W under WAL,
    // they just don't own the fs4 lock. Both primary + secondary
    // can submit; SQLite serialises and the persisted revision
    // counter keeps them in sync.
    let dir = tempdir().unwrap();
    let primary = IndexHandle::open(dir.path()).unwrap();
    primary
        .submit_blocking(IngestCommand::UpsertFile {
            path: "src/main.rs".into(),
            extracted: sample_extracted("src/main.rs", "crate::primary_sym"),
        })
        .unwrap();
    wait_revision_at_least(&primary, 1, Duration::from_secs(2));

    let secondary = IndexHandle::open_readonly(dir.path()).unwrap();
    secondary
        .submit_blocking(IngestCommand::UpsertFile {
            path: "src/other.rs".into(),
            extracted: sample_extracted("src/other.rs", "crate::secondary_sym"),
        })
        .expect("secondary handle must accept writes (v6+ WAL semantic)");
    wait_revision_at_least(&secondary, 2, Duration::from_secs(2));

    // Both handles observe the persisted revision after both writes.
    let conn = primary.pool().unwrap().get().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);
    assert_eq!(primary.revision(), secondary.revision());
}

#[test]
fn revision_persists_across_handle_drop_and_reopen() {
    let dir = tempdir().unwrap();
    {
        let handle = IndexHandle::open(dir.path()).unwrap();
        handle
            .submit_blocking(IngestCommand::UpsertFile {
                path: "src/main.rs".into(),
                extracted: sample_extracted("src/main.rs", "crate::persistent"),
            })
            .unwrap();
        wait_revision_at_least(&handle, 1, Duration::from_secs(2));
    }
    let reopened = IndexHandle::open(dir.path()).unwrap();
    assert!(
        reopened.revision() >= 1,
        "revision must survive process restart (v6+ SQL-persisted)"
    );
}

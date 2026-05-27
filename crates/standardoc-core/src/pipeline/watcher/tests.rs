
use std::time::{Duration, Instant};

use notify::event::{CreateKind, ModifyKind, RemoveKind};
use standardoc_ir::{
    Blake3Hash, ExtractedFile, Kind, Language, LanguageKind, RawSymbol, SourceOrigin,
    SymbolLocation, Visibility,
};
use tempfile::tempdir;

use super::*;
use crate::pipeline::provider::mock::{MockProvider, MockResponse};

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

fn set_debounce_ms(handle: &IndexHandle, ms: u64) {
    let conn = handle.pool().unwrap().get().unwrap();
    conn.execute(
        "UPDATE schema_meta SET value = ?1 WHERE key = 'watcher_debounce_ms'",
        [ms.to_string()],
    )
    .unwrap();
}

fn wait_revision_at_least(handle: &IndexHandle, target: u64, timeout: Duration) {
    let start = Instant::now();
    while handle.revision() < target {
        assert!(
            start.elapsed() <= timeout,
            "revision did not reach {target} within {timeout:?} (was {})",
            handle.revision()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn fresh_filters(handle: &IndexHandle) -> Arc<RwLock<ScanFilters>> {
    Arc::new(RwLock::new(ScanFilters::load(handle.workspace_root())))
}

#[test]
fn read_debounce_ms_returns_seeded_default_500() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    assert_eq!(read_debounce_ms(&handle).unwrap(), 500);
}

#[test]
fn read_debounce_ms_returns_invalid_stored_data_on_garbage() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    let conn = handle.pool().unwrap().get().unwrap();
    conn.execute(
        "UPDATE schema_meta SET value = 'lol' WHERE key = 'watcher_debounce_ms'",
        [],
    )
    .unwrap();
    drop(conn);
    let err = read_debounce_ms(&handle).unwrap_err();
    assert!(matches!(
        err,
        WatcherError::Storage(StorageError::InvalidStoredData { .. })
    ));
}

#[test]
fn is_dispatchable_filters_event_kinds() {
    assert!(is_dispatchable(&EventKind::Create(CreateKind::File)));
    assert!(is_dispatchable(&EventKind::Modify(ModifyKind::Any)));
    assert!(is_dispatchable(&EventKind::Remove(RemoveKind::File)));
    assert!(!is_dispatchable(&EventKind::Any));
    assert!(!is_dispatchable(&EventKind::Other));
}

#[test]
fn watcher_indexes_new_file_via_provider() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    set_debounce_ms(&handle, 50);

    let mock = Arc::new(MockProvider::new());
    mock.set(
        "src/lib.rs",
        MockResponse::Ok(sample_extracted("src/lib.rs", "crate::foo")),
    );
    let provider: Arc<dyn LanguageProvider> = mock;
    let filters = fresh_filters(&handle);

    // Pre-create src/ before spawning the watcher so inotify (Linux) watches
    // it from the start. Creating the directory after spawn races with
    // inotify adding the recursive watch for the new subdirectory.
    let src_dir = handle.workspace_root().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();

    let _watcher = spawn_watcher(handle.clone(), provider, filters).unwrap();

    std::fs::write(src_dir.join("lib.rs"), b"fn foo() {}").unwrap();

    wait_revision_at_least(&handle, 1, Duration::from_secs(15));

    let conn = handle.pool().unwrap().get().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn watcher_records_parse_error_through_command() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    set_debounce_ms(&handle, 50);

    let mock = Arc::new(MockProvider::new());
    mock.set(
        "src/bad.rs",
        MockResponse::ParseError("unexpected token".into()),
    );
    let provider: Arc<dyn LanguageProvider> = mock;
    let filters = fresh_filters(&handle);

    let src_dir = handle.workspace_root().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();

    let _watcher = spawn_watcher(handle.clone(), provider, filters).unwrap();

    std::fs::write(src_dir.join("bad.rs"), b"fn ???").unwrap();

    wait_revision_at_least(&handle, 1, Duration::from_secs(15));

    let conn = handle.pool().unwrap().get().unwrap();
    let last_error: Option<String> = conn
        .query_row(
            "SELECT last_scan_error FROM files WHERE path = 'src/bad.rs'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(last_error.as_deref(), Some("unexpected token"));
}

#[test]
fn watcher_deletes_file_on_remove_event() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    set_debounce_ms(&handle, 50);

    let mock = Arc::new(MockProvider::new());
    mock.set(
        "src/lib.rs",
        MockResponse::Ok(sample_extracted("src/lib.rs", "crate::foo")),
    );
    let provider: Arc<dyn LanguageProvider> = mock;
    let filters = fresh_filters(&handle);

    let src_dir = handle.workspace_root().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();

    let _watcher = spawn_watcher(handle.clone(), provider, filters).unwrap();

    let file_path = src_dir.join("lib.rs");
    std::fs::write(&file_path, b"fn foo() {}").unwrap();

    wait_revision_at_least(&handle, 1, Duration::from_secs(15));
    let revision_after_create = handle.revision();

    std::fs::remove_file(&file_path).unwrap();
    wait_revision_at_least(&handle, revision_after_create + 1, Duration::from_secs(15));

    let conn = handle.pool().unwrap().get().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path = 'src/lib.rs'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn watcher_skips_filtered_subtree() {
    let dir = tempdir().unwrap();
    std::fs::write(
        dir.path().join(".stdignore"),
        "target/\nvendored/\n.git/\nnode_modules/\n",
    )
    .unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    set_debounce_ms(&handle, 50);

    let mock = Arc::new(MockProvider::new());
    let provider: Arc<dyn LanguageProvider> = mock;
    let filters = fresh_filters(&handle);

    let _watcher = spawn_watcher(handle.clone(), provider, filters).unwrap();

    let vendored = handle.workspace_root().join("vendored");
    std::fs::create_dir_all(&vendored).unwrap();
    std::fs::write(vendored.join("noise.rs"), b"fn noise() {}").unwrap();

    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        handle.revision(),
        0,
        "events on filtered paths must never bump the revision"
    );
}

#[test]
fn watcher_ignores_unsupported_extensions() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    set_debounce_ms(&handle, 50);

    let mock = Arc::new(MockProvider::new());
    let provider: Arc<dyn LanguageProvider> = mock;
    let filters = fresh_filters(&handle);

    let _watcher = spawn_watcher(handle.clone(), provider, filters).unwrap();

    std::fs::write(handle.workspace_root().join("notes.txt"), b"hello").unwrap();

    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        handle.revision(),
        0,
        "unsupported extensions must never bump the revision"
    );
}

#[test]
fn watcher_skips_paused_events() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    set_debounce_ms(&handle, 50);

    let mock = Arc::new(MockProvider::new());
    mock.set(
        "src/lib.rs",
        MockResponse::Ok(sample_extracted("src/lib.rs", "crate::foo")),
    );
    let provider: Arc<dyn LanguageProvider> = mock;
    let filters = fresh_filters(&handle);

    let _watcher = spawn_watcher(handle.clone(), provider, filters).unwrap();

    handle.pause();
    let src_dir = handle.workspace_root().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::write(src_dir.join("lib.rs"), b"fn foo() {}").unwrap();

    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(
        handle.revision(),
        0,
        "paused watcher must not dispatch events"
    );
}

#[test]
fn watcher_warns_when_pattern_added_marks_existing_rows() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    set_debounce_ms(&handle, 50);

    // Seed an indexed file in `vendored/` while no such pattern exists.
    let conn = handle.pool().unwrap().get().unwrap();
    conn.execute(
        "INSERT INTO files (path, content_hash, language, last_scanned, byte_size) \
             VALUES ('vendored/keep.rs', 'aa', 'rust', 0, 0)",
        [],
    )
    .unwrap();
    drop(conn);

    let mock = Arc::new(MockProvider::new());
    let provider: Arc<dyn LanguageProvider> = mock;
    let filters = fresh_filters(&handle);
    let filters_clone = Arc::clone(&filters);

    let _watcher = spawn_watcher(handle.clone(), provider, filters).unwrap();

    // Append a pattern that now excludes `vendored/`.
    let stdignore_path = handle.workspace_root().join(".stdignore");
    let mut body = std::fs::read_to_string(&stdignore_path).unwrap();
    body.push_str("\nvendored/\n");
    std::fs::write(&stdignore_path, body).unwrap();

    // Wait for the swap to land.
    let start = Instant::now();
    loop {
        let guard = filters_clone.read().unwrap();
        if guard.is_skipped("vendored/keep.rs") {
            break;
        }
        drop(guard);
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "filters were never reloaded after .stdignore change"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    // Row must remain in DB until the user purges (lock 21 Q5 add path).
    let conn = handle.pool().unwrap().get().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path = 'vendored/keep.rs'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 1,
        "newly-excluded rows are not auto-purged (Q5 add path)"
    );
}

#[test]
fn watcher_dispatches_peer_event_with_workspace_id() {
    // L3d-2: a file change inside an `add_peer`'d root produces a
    // scoped `files.path` row + a symbol row tagged with the peer's
    // workspace_id, NOT the 'primary' default.
    let primary_dir = tempdir().unwrap();
    let peer_dir = tempdir().unwrap();
    let handle = IndexHandle::open(primary_dir.path()).unwrap();
    set_debounce_ms(&handle, 50);

    let mock = Arc::new(MockProvider::new());
    mock.set(
        "src/peer.rs",
        MockResponse::Ok(sample_extracted("src/peer.rs", "peer::foo")),
    );
    let provider: Arc<dyn LanguageProvider> = mock;
    let filters = fresh_filters(&handle);

    let peer_src = peer_dir.path().join("src");
    std::fs::create_dir_all(&peer_src).unwrap();

    let mut watcher = spawn_watcher(handle.clone(), provider, filters).unwrap();
    watcher
        .add_peer("peer-aaa".into(), peer_dir.path())
        .unwrap();

    std::fs::write(peer_src.join("peer.rs"), b"fn foo() {}").unwrap();
    wait_revision_at_least(&handle, 1, Duration::from_secs(15));

    let conn = handle.pool().unwrap().get().unwrap();
    let file_path: String = conn
        .query_row(
            "SELECT path FROM files WHERE path LIKE 'ws:peer-aaa:%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(file_path, "ws:peer-aaa:src/peer.rs");

    let sym_ws_id: String = conn
        .query_row(
            "SELECT workspace_id FROM symbols WHERE fqdn = 'peer::foo'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(sym_ws_id, "peer-aaa");
}

#[test]
fn watcher_remove_peer_stops_dispatching_subsequent_events() {
    // L3d-2: after `remove_peer`, a subsequent change in the unwatched
    // peer root must NOT bump the revision. Use a sleep-then-check
    // pattern rather than `wait_revision_at_least` to assert the
    // negative outcome.
    let primary_dir = tempdir().unwrap();
    let peer_dir = tempdir().unwrap();
    let handle = IndexHandle::open(primary_dir.path()).unwrap();
    set_debounce_ms(&handle, 50);

    let mock = Arc::new(MockProvider::new());
    mock.set(
        "src/peer.rs",
        MockResponse::Ok(sample_extracted("src/peer.rs", "peer::foo")),
    );
    let provider: Arc<dyn LanguageProvider> = mock;
    let filters = fresh_filters(&handle);

    let peer_src = peer_dir.path().join("src");
    std::fs::create_dir_all(&peer_src).unwrap();

    let mut watcher = spawn_watcher(handle.clone(), provider, filters).unwrap();
    watcher
        .add_peer("peer-bbb".into(), peer_dir.path())
        .unwrap();
    watcher.remove_peer("peer-bbb").unwrap();
    assert!(watcher.peers_snapshot().is_empty());

    std::fs::write(peer_src.join("peer.rs"), b"fn foo() {}").unwrap();

    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(
        handle.revision(),
        0,
        "removed peer's events must not bump the revision"
    );
}

#[test]
fn watcher_add_peer_is_idempotent() {
    // L3d-2: re-adding the same workspace_id must not double-register
    // and must not error. The peer snapshot stays at length 1.
    let primary_dir = tempdir().unwrap();
    let peer_dir = tempdir().unwrap();
    let handle = IndexHandle::open(primary_dir.path()).unwrap();
    set_debounce_ms(&handle, 50);

    let mock = Arc::new(MockProvider::new());
    let provider: Arc<dyn LanguageProvider> = mock;
    let filters = fresh_filters(&handle);

    let mut watcher = spawn_watcher(handle, provider, filters).unwrap();
    watcher
        .add_peer("peer-ccc".into(), peer_dir.path())
        .unwrap();
    watcher
        .add_peer("peer-ccc".into(), peer_dir.path())
        .unwrap();
    assert_eq!(watcher.peers_snapshot().len(), 1);
}

#[test]
fn watcher_routes_primary_and_peer_events_concurrently() {
    // L3d-2: with both a primary file change AND a peer file change
    // submitted, both result in their respective scoped rows. Guards
    // against routing regressions where every event collapses to
    // primary or peer.
    let primary_dir = tempdir().unwrap();
    let peer_dir = tempdir().unwrap();
    let handle = IndexHandle::open(primary_dir.path()).unwrap();
    set_debounce_ms(&handle, 50);

    let mock = Arc::new(MockProvider::new());
    mock.set(
        "src/lib.rs",
        MockResponse::Ok(sample_extracted("src/lib.rs", "crate::main")),
    );
    mock.set(
        "src/lib.rs",
        MockResponse::Ok(sample_extracted("src/lib.rs", "crate::main")),
    );
    let provider: Arc<dyn LanguageProvider> = mock;
    let filters = fresh_filters(&handle);

    let primary_src = primary_dir.path().join("src");
    let peer_src = peer_dir.path().join("src");
    std::fs::create_dir_all(&primary_src).unwrap();
    std::fs::create_dir_all(&peer_src).unwrap();

    let mut watcher = spawn_watcher(handle.clone(), provider, filters).unwrap();
    watcher
        .add_peer("peer-ddd".into(), peer_dir.path())
        .unwrap();

    std::fs::write(primary_src.join("lib.rs"), b"fn main() {}").unwrap();
    std::fs::write(peer_src.join("lib.rs"), b"fn main() {}").unwrap();

    wait_revision_at_least(&handle, 2, Duration::from_secs(15));

    let conn = handle.pool().unwrap().get().unwrap();
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 2, "primary + peer rows must coexist");

    let primary_present: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path = 'src/lib.rs'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(primary_present, 1);

    let peer_present: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path = 'ws:peer-ddd:src/lib.rs'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(peer_present, 1);
}

#[test]
fn watcher_reindexes_subtree_when_pattern_removed() {
    let dir = tempdir().unwrap();
    std::fs::write(
        dir.path().join(".stdignore"),
        "target/\nvendored/\n.git/\nnode_modules/\n",
    )
    .unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    set_debounce_ms(&handle, 50);

    // Drop a file in the (currently) excluded subtree.
    let vendored_dir = handle.workspace_root().join("vendored");
    std::fs::create_dir_all(&vendored_dir).unwrap();
    std::fs::write(vendored_dir.join("lib.rs"), b"fn vendored_fn() {}").unwrap();

    let mock = Arc::new(MockProvider::new());
    mock.set(
        "vendored/lib.rs",
        MockResponse::Ok(sample_extracted("vendored/lib.rs", "vendored::vendored_fn")),
    );
    let provider: Arc<dyn LanguageProvider> = mock;
    let filters = fresh_filters(&handle);

    let _watcher = spawn_watcher(handle.clone(), provider, filters).unwrap();

    // Confirm the file is NOT indexed yet.
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(handle.revision(), 0);

    // Remove the `vendored/` pattern.
    std::fs::write(
        handle.workspace_root().join(".stdignore"),
        "target/\n.git/\nnode_modules/\n",
    )
    .unwrap();

    wait_revision_at_least(&handle, 1, Duration::from_secs(15));

    let conn = handle.pool().unwrap().get().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path = 'vendored/lib.rs'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "newly-allowed file must be re-indexed");
}


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
        ffi_bindings: vec![],
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
        module_lookup: None,
        symbols: vec![RawSymbol {
            decl_kind: None,
            implements_trait: None,
            receiver_type: None,
            entry_point: None,
            name: "caller".into(),
            fqdn: "crate::caller".into(),
            kind: Kind::Callable,
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
        ffi_bindings: vec![],
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

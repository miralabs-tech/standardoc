
use super::*;

use rusqlite::params;
use standardoc_ir::SourceOrigin;
use tempfile::tempdir;

fn fresh_handle() -> (tempfile::TempDir, IndexHandle) {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    (dir, handle)
}

fn insert_external_symbol(handle: &IndexHandle, fqdn: &str, origin: SourceOrigin) {
    let conn = handle.pool().unwrap().get().unwrap();
    conn.execute(
        "INSERT INTO files (path, content_hash, language, last_scanned, byte_size) \
             VALUES (?1, 'aa', 'rust', 0, 0)",
        [fqdn],
    )
    .ok(); // file may already exist for another symbol
    conn.execute(
        "INSERT INTO symbols (fqdn, name, kind, language_kind, language, file_path, \
                                  start_line, end_line, start_col, end_col, \
                                  is_external, source_origin) \
             VALUES (?1, ?2, 'callable', 'fn', 'rust', ?1, 0, 0, 0, 0, 1, ?3)",
        params![fqdn, fqdn, source_origin_to_sql_text(origin)],
    )
    .unwrap();
}

fn count_externals(handle: &IndexHandle, origin: SourceOrigin) -> i64 {
    let conn = handle.pool().unwrap().get().unwrap();
    conn.query_row(
        "SELECT COUNT(*) FROM symbols WHERE is_external = 1 AND source_origin = ?1",
        [source_origin_to_sql_text(origin)],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn npm_lockfile_kind_round_trips_via_parse() {
    for kind in [
        NpmLockfileKind::PackageLockJson,
        NpmLockfileKind::PnpmLockYaml,
        NpmLockfileKind::YarnLock,
        NpmLockfileKind::YarnPnpCjs,
    ] {
        assert_eq!(NpmLockfileKind::parse(kind.as_str()), Some(kind));
    }
    assert_eq!(NpmLockfileKind::parse("bogus"), None);
}

#[test]
fn compute_lockfile_hashes_empty_workspace_returns_all_none() {
    let dir = tempdir().unwrap();
    let h = compute_lockfile_hashes(dir.path()).unwrap();
    assert_eq!(h, LockfileHashes::default());
}

#[test]
fn compute_lockfile_hashes_picks_pnpm_over_package_lock() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("package-lock.json"), "{}").unwrap();
    std::fs::write(dir.path().join("pnpm-lock.yaml"), "lockfileVersion: 6").unwrap();
    let h = compute_lockfile_hashes(dir.path()).unwrap();
    let (kind, _hash) = h.npm.expect("npm hash present");
    assert_eq!(kind, NpmLockfileKind::PnpmLockYaml);
}

#[test]
fn compute_lockfile_hashes_picks_yarn_lock_when_no_pnpm() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("package-lock.json"), "{}").unwrap();
    std::fs::write(dir.path().join("yarn.lock"), "# yarn classic").unwrap();
    let h = compute_lockfile_hashes(dir.path()).unwrap();
    let (kind, _hash) = h.npm.expect("npm hash present");
    assert_eq!(kind, NpmLockfileKind::YarnLock);
}

#[test]
fn compute_lockfile_hashes_picks_pnp_when_no_pnpm_or_yarn() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("package-lock.json"), "{}").unwrap();
    std::fs::write(dir.path().join(".pnp.cjs"), "// pnp").unwrap();
    let h = compute_lockfile_hashes(dir.path()).unwrap();
    let (kind, _hash) = h.npm.expect("npm hash present");
    assert_eq!(kind, NpmLockfileKind::YarnPnpCjs);
}

#[test]
fn compute_lockfile_hashes_falls_back_to_package_lock_only() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("package-lock.json"), "{}").unwrap();
    let h = compute_lockfile_hashes(dir.path()).unwrap();
    let (kind, _hash) = h.npm.expect("npm hash present");
    assert_eq!(kind, NpmLockfileKind::PackageLockJson);
}

#[test]
fn compute_lockfile_hashes_blake3_hex_stable_for_same_content() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.lock"), "# cargo lockfile").unwrap();
    let h1 = compute_lockfile_hashes(dir.path()).unwrap();
    let h2 = compute_lockfile_hashes(dir.path()).unwrap();
    assert_eq!(h1.cargo, h2.cargo);
    assert!(h1.cargo.as_deref().is_some_and(|h| h.len() == 64));
}

#[test]
fn read_stored_hashes_returns_all_none_on_fresh_db() {
    let (_dir, handle) = fresh_handle();
    let h = read_stored_hashes(&handle).unwrap();
    assert_eq!(h, LockfileHashes::default());
}

#[test]
fn write_then_read_stored_hashes_round_trips_cargo() {
    let (_dir, handle) = fresh_handle();
    let h = LockfileHashes {
        cargo: Some("abcdef".repeat(8).chars().take(64).collect()),
        ..LockfileHashes::default()
    };
    write_stored_hashes(&handle, &h).unwrap();
    let back = read_stored_hashes(&handle).unwrap();
    assert_eq!(back.cargo, h.cargo);
}

#[test]
fn write_then_read_stored_hashes_round_trips_npm_pair() {
    let (_dir, handle) = fresh_handle();
    let h = LockfileHashes {
        npm: Some((NpmLockfileKind::PnpmLockYaml, "deadbeef".into())),
        ..LockfileHashes::default()
    };
    write_stored_hashes(&handle, &h).unwrap();
    let back = read_stored_hashes(&handle).unwrap();
    assert_eq!(back.npm, h.npm);
}

#[test]
fn write_stored_hashes_overwrites_previous_blank() {
    let (_dir, handle) = fresh_handle();
    let h1 = LockfileHashes {
        cargo: Some("first".into()),
        ..LockfileHashes::default()
    };
    write_stored_hashes(&handle, &h1).unwrap();

    let h2 = LockfileHashes::default();
    write_stored_hashes(&handle, &h2).unwrap();
    let back = read_stored_hashes(&handle).unwrap();
    assert_eq!(back, LockfileHashes::default());
}

#[test]
fn purge_externals_by_origin_drops_only_matching_rows() {
    let (_dir, handle) = fresh_handle();
    insert_external_symbol(&handle, "serde::Deserialize", SourceOrigin::CargoRegistry);
    insert_external_symbol(&handle, "react::Component", SourceOrigin::NodeModulesDts);

    let purged = purge_externals_by_origin(&handle, SourceOrigin::CargoRegistry).unwrap();
    assert_eq!(purged, 1);
    assert_eq!(count_externals(&handle, SourceOrigin::CargoRegistry), 0);
    assert_eq!(count_externals(&handle, SourceOrigin::NodeModulesDts), 1);
}

#[test]
fn purge_externals_by_origin_bumps_revision_on_delete() {
    let (_dir, handle) = fresh_handle();
    insert_external_symbol(&handle, "serde::Deserialize", SourceOrigin::CargoRegistry);
    let before = handle.revision();
    purge_externals_by_origin(&handle, SourceOrigin::CargoRegistry).unwrap();
    assert!(
        handle.revision() > before,
        "purge with deletion must bump the workspace revision"
    );
}

#[test]
fn purge_externals_by_origin_does_not_bump_when_nothing_matches() {
    let (_dir, handle) = fresh_handle();
    let before = handle.revision();
    purge_externals_by_origin(&handle, SourceOrigin::CargoRegistry).unwrap();
    assert_eq!(handle.revision(), before);
}

#[test]
fn invalidate_changed_lockfiles_purges_cargo_when_hash_diverges() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    insert_external_symbol(&handle, "serde::Deserialize", SourceOrigin::CargoRegistry);

    // Seed a "previous" hash that won't match the live one.
    let stale = LockfileHashes {
        cargo: Some("00".repeat(32)),
        ..LockfileHashes::default()
    };
    write_stored_hashes(&handle, &stale).unwrap();

    std::fs::write(dir.path().join("Cargo.lock"), "# fresh content").unwrap();

    let purged = invalidate_changed_lockfiles(&handle, handle.workspace_root()).unwrap();
    let cargo_entry = purged
        .iter()
        .find(|(o, _)| *o == SourceOrigin::CargoRegistry)
        .expect("cargo origin must be reported");
    assert_eq!(
        cargo_entry.1, 1,
        "purged_count must mirror the actual row count deleted"
    );
    assert_eq!(count_externals(&handle, SourceOrigin::CargoRegistry), 0);

    // Baseline now matches the live state — second invocation must be no-op.
    let again = invalidate_changed_lockfiles(&handle, handle.workspace_root()).unwrap();
    assert!(again.is_empty());
}

#[test]
fn invalidate_changed_lockfiles_no_op_when_hashes_match() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    let purged = invalidate_changed_lockfiles(&handle, handle.workspace_root()).unwrap();
    assert!(
        purged.is_empty(),
        "empty workspace with no stored hashes must not purge anything"
    );
}

#[test]
fn handle_lockfile_change_returns_cargo_origin_for_cargo_lock() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    std::fs::write(handle.workspace_root().join("Cargo.lock"), "# v1").unwrap();
    let result = handle_lockfile_change(
        &handle,
        handle.workspace_root(),
        &handle.workspace_root().join("Cargo.lock"),
    )
    .unwrap();
    assert!(matches!(result, Some((SourceOrigin::CargoRegistry, _))));
}

#[test]
fn handle_lockfile_change_reports_purged_count() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    insert_external_symbol(&handle, "serde::Deserialize", SourceOrigin::CargoRegistry);
    insert_external_symbol(&handle, "serde::Serialize", SourceOrigin::CargoRegistry);
    std::fs::write(handle.workspace_root().join("Cargo.lock"), "# v1").unwrap();

    let result = handle_lockfile_change(
        &handle,
        handle.workspace_root(),
        &handle.workspace_root().join("Cargo.lock"),
    )
    .unwrap();
    match result {
        Some((SourceOrigin::CargoRegistry, count)) => assert_eq!(count, 2),
        other => panic!("expected Some((CargoRegistry, 2)), got {other:?}"),
    }
}

#[test]
fn handle_lockfile_change_maps_npm_lockfiles_to_node_modules_origin() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    for name in [
        "package-lock.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        ".pnp.cjs",
    ] {
        std::fs::write(handle.workspace_root().join(name), "x").unwrap();
        let result = handle_lockfile_change(
            &handle,
            handle.workspace_root(),
            &handle.workspace_root().join(name),
        )
        .unwrap();
        assert!(
            matches!(result, Some((SourceOrigin::NodeModulesDts, _))),
            "{name} must map to NodeModulesDts, got {result:?}"
        );
        std::fs::remove_file(handle.workspace_root().join(name)).unwrap();
    }
}

#[test]
fn handle_lockfile_change_returns_none_for_unrelated_path() {
    let dir = tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    let result = handle_lockfile_change(
        &handle,
        handle.workspace_root(),
        &handle.workspace_root().join("src/main.rs"),
    )
    .unwrap();
    assert_eq!(result, None);
}

#[test]
fn tracked_lockfile_paths_returns_five_known_lockfiles() {
    let paths = tracked_lockfile_paths(Path::new("/tmp/wks"));
    assert_eq!(paths.len(), 5);
    let names: Vec<&std::ffi::OsStr> = paths.iter().filter_map(|p| p.file_name()).collect();
    assert!(names.iter().any(|n| *n == "Cargo.lock"));
    assert!(names.iter().any(|n| *n == "package-lock.json"));
    assert!(names.iter().any(|n| *n == "pnpm-lock.yaml"));
    assert!(names.iter().any(|n| *n == "yarn.lock"));
    assert!(names.iter().any(|n| *n == ".pnp.cjs"));
}

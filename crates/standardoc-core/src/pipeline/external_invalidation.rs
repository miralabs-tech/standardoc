//! Lockfile-driven invalidation for externally-cached symbols.
//!
//! Cached externals (Cargo crate sources, npm `.d.ts`, luarocks rocks)
//! are pinned to the lockfile state that produced them. When the user
//! runs `cargo update`, switches `pnpm` ⇄ `npm`, or installs a new rock,
//! the cached symbols become stale. We do NOT re-walk eagerly — the
//! resolver is lazy on-demand by design — but we MUST purge the stale
//! cache so a subsequent `resolve_external(fqdn)` repopulates from the
//! new source.
//!
//! Two entry points:
//!
//! 1. **Cold start** — [`invalidate_changed_lockfiles`] compares the
//!    BLAKE3 hash of each lockfile against the values cached in
//!    `schema_meta.external_*_lockfile_hash`. Each mismatch triggers
//!    [`purge_externals_by_origin`] for the corresponding [`SourceOrigin`].
//!
//! 2. **Live watcher** — [`handle_lockfile_change`] is wired into
//!    `pipeline::watcher` and called whenever notify reports a change
//!    to one of the tracked manifests (`Cargo.lock`,
//!    `package-lock.json`, `pnpm-lock.yaml`, `yarn.lock`, `.pnp.cjs`).
//!    Same purge semantics, no extra book-keeping.
//!
//! Invalidation = purge only. Re-population is deferred to the next
//! `resolve_external` call. This keeps the watcher path cheap (a
//! `DELETE WHERE is_external = 1 AND source_origin = ?1` is < 1ms even
//! on large indexes) and avoids re-walking entire `node_modules/` trees
//! on every `pnpm install`.

use std::path::{Path, PathBuf};

use standardoc_ir::SourceOrigin;

use crate::storage::conv::source_origin_to_sql_text;
use crate::storage::error::StorageError;
use crate::storage::handle::IndexHandle;

/// Identifier for the npm-family lockfile actually consumed by the
/// workspace. Stored as a separate `schema_meta` row alongside the hash
/// so a kind switch (npm → pnpm, pnpm → yarn-PnP, ...) triggers a purge
/// even when the new lockfile happens to hash to the same value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NpmLockfileKind {
    PackageLockJson,
    PnpmLockYaml,
    YarnLock,
    YarnPnpCjs,
}

impl NpmLockfileKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PackageLockJson => "package-lock.json",
            Self::PnpmLockYaml => "pnpm-lock.yaml",
            Self::YarnLock => "yarn.lock",
            Self::YarnPnpCjs => ".pnp.cjs",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "package-lock.json" => Some(Self::PackageLockJson),
            "pnpm-lock.yaml" => Some(Self::PnpmLockYaml),
            "yarn.lock" => Some(Self::YarnLock),
            ".pnp.cjs" => Some(Self::YarnPnpCjs),
            _ => None,
        }
    }
}

/// Snapshot of the lockfile fingerprints currently observable on disk
/// at a given workspace root. `None` for any lockfile that does not
/// exist (e.g. a pure Rust workspace has `npm = None` / `luarocks = None`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LockfileHashes {
    pub cargo: Option<String>,
    pub npm: Option<(NpmLockfileKind, String)>,
    pub luarocks: Option<String>,
}

/// Reads each tracked lockfile from disk and BLAKE3-hashes its content.
/// Resolves the npm lockfile kind via a priority order:
/// `pnpm-lock.yaml` > `yarn.lock` > `.pnp.cjs` > `package-lock.json`
/// (pnpm preferred since it's the most precise; PnP picked over
/// `package-lock.json` because PnP overrides classic resolution).
///
/// Luarocks has no canonical lockfile on disk so `LockfileHashes::luarocks`
/// stays `None` here — the [`super::watcher`]-side and the
/// [`crate::externals::luarocks::LuarocksResolver`] are the ones who
/// hash the `luarocks list --porcelain` snapshot.
pub fn compute_lockfile_hashes(workspace_root: &Path) -> Result<LockfileHashes, StorageError> {
    let cargo = hash_file_if_exists(&workspace_root.join("Cargo.lock"))?;

    let pnpm_lock = workspace_root.join("pnpm-lock.yaml");
    let yarn_lock = workspace_root.join("yarn.lock");
    let pnp_cjs = workspace_root.join(".pnp.cjs");
    let package_lock = workspace_root.join("package-lock.json");

    let npm = if pnpm_lock.is_file() {
        hash_file_if_exists(&pnpm_lock)?.map(|h| (NpmLockfileKind::PnpmLockYaml, h))
    } else if yarn_lock.is_file() {
        hash_file_if_exists(&yarn_lock)?.map(|h| (NpmLockfileKind::YarnLock, h))
    } else if pnp_cjs.is_file() {
        hash_file_if_exists(&pnp_cjs)?.map(|h| (NpmLockfileKind::YarnPnpCjs, h))
    } else if package_lock.is_file() {
        hash_file_if_exists(&package_lock)?.map(|h| (NpmLockfileKind::PackageLockJson, h))
    } else {
        None
    };

    Ok(LockfileHashes {
        cargo,
        npm,
        luarocks: None,
    })
}

fn hash_file_if_exists(path: &Path) -> Result<Option<String>, StorageError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(blake3::hash(&bytes).to_hex().to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(StorageError::Io(e)),
    }
}

/// Reads the cached lockfile hashes stored in `schema_meta`. Blank
/// strings (the v4→v5 init sentinel) come back as `None`. Unknown npm
/// kind values are skipped silently — caller treats them as "no
/// previously-cached npm baseline".
pub fn read_stored_hashes(handle: &IndexHandle) -> Result<LockfileHashes, StorageError> {
    let pool = handle.pool()?;
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT key, value FROM schema_meta WHERE key IN (\
            'external_cargo_lockfile_hash', \
            'external_npm_lockfile_hash', \
            'external_npm_lockfile_kind', \
            'external_luarocks_hash')",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut cargo = None;
    let mut npm_hash = None;
    let mut npm_kind_raw = None;
    let mut luarocks = None;
    for (k, v) in rows {
        if v.is_empty() {
            continue;
        }
        match k.as_str() {
            "external_cargo_lockfile_hash" => cargo = Some(v),
            "external_npm_lockfile_hash" => npm_hash = Some(v),
            "external_npm_lockfile_kind" => npm_kind_raw = Some(v),
            "external_luarocks_hash" => luarocks = Some(v),
            _ => {}
        }
    }
    let npm = match (
        npm_hash,
        npm_kind_raw.as_deref().and_then(NpmLockfileKind::parse),
    ) {
        (Some(h), Some(kind)) => Some((kind, h)),
        _ => None,
    };
    Ok(LockfileHashes {
        cargo,
        npm,
        luarocks,
    })
}

/// Persists the live hashes into `schema_meta`. Called after a
/// successful purge so the next cold start observes the new baseline.
/// `None` values are written as blank strings (the v4→v5 sentinel).
pub fn write_stored_hashes(
    handle: &IndexHandle,
    hashes: &LockfileHashes,
) -> Result<(), StorageError> {
    let pool = handle.pool()?;
    let mut conn = pool.get()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    update_meta(
        &tx,
        "external_cargo_lockfile_hash",
        hashes.cargo.as_deref().unwrap_or(""),
    )?;
    let (npm_hash, npm_kind) = hashes
        .npm
        .as_ref()
        .map_or(("", ""), |(k, h)| (h.as_str(), k.as_str()));
    update_meta(&tx, "external_npm_lockfile_hash", npm_hash)?;
    update_meta(&tx, "external_npm_lockfile_kind", npm_kind)?;
    update_meta(
        &tx,
        "external_luarocks_hash",
        hashes.luarocks.as_deref().unwrap_or(""),
    )?;
    tx.commit()?;
    Ok(())
}

fn update_meta(tx: &rusqlite::Transaction<'_>, key: &str, value: &str) -> Result<(), StorageError> {
    tx.execute(
        "UPDATE schema_meta SET value = ?1 WHERE key = ?2",
        rusqlite::params![value, key],
    )?;
    Ok(())
}

/// Deletes every symbol row with `is_external = 1 AND source_origin = ?1`
/// in a single transaction. Edges + documents + enrichments cascade via
/// the schema's existing FK relationships. Returns the row count for
/// the WARN log.
///
/// Bumps the workspace revision when at least one row was deleted so
/// downstream `check_stale` callers observe the invalidation event.
pub fn purge_externals_by_origin(
    handle: &IndexHandle,
    origin: SourceOrigin,
) -> Result<usize, StorageError> {
    let pool = handle.pool()?;
    let mut conn = pool.get()?;
    let origin_text = source_origin_to_sql_text(origin);
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let purged = tx.execute(
        "DELETE FROM symbols WHERE is_external = 1 AND source_origin = ?1",
        [origin_text],
    )?;
    tx.commit()?;
    if purged > 0 {
        handle.bump_revision();
    }
    Ok(purged)
}

/// Cold-start entry: diff stored vs. live hashes, purge each mismatched
/// origin, then persist the new baseline. Returns the list of
/// `(origin, purged_count)` pairs so the CLI / watcher can emit
/// `STDOC_WARN` with the actual symbol count dropped from the index
/// (rather than a placeholder zero).
pub fn invalidate_changed_lockfiles(
    handle: &IndexHandle,
    workspace_root: &Path,
) -> Result<Vec<(SourceOrigin, usize)>, StorageError> {
    let stored = read_stored_hashes(handle)?;
    let live = compute_lockfile_hashes(workspace_root)?;
    let mut purged = Vec::new();

    if stored.cargo != live.cargo {
        let count = purge_externals_by_origin(handle, SourceOrigin::CargoRegistry)?;
        purged.push((SourceOrigin::CargoRegistry, count));
    }

    if stored.npm != live.npm {
        let count = purge_externals_by_origin(handle, SourceOrigin::NodeModulesDts)?;
        purged.push((SourceOrigin::NodeModulesDts, count));
    }

    if stored.luarocks != live.luarocks {
        let count = purge_externals_by_origin(handle, SourceOrigin::ManualExternal)?;
        purged.push((SourceOrigin::ManualExternal, count));
    }

    write_stored_hashes(handle, &live)?;
    Ok(purged)
}

/// Live-watcher entry: called from `pipeline::watcher` when notify
/// reports a path matching one of the tracked lockfiles. Same purge
/// semantics as the cold-start path but scoped to the single
/// [`SourceOrigin`] derived from the changed path.
///
/// Path → origin mapping:
///
/// - `Cargo.lock` → `SourceOrigin::CargoRegistry`.
/// - `package-lock.json` / `pnpm-lock.yaml` / `yarn.lock` / `.pnp.cjs`
///   → `SourceOrigin::NodeModulesDts`.
///
/// Returns `None` when the path is not a tracked lockfile (no-op).
pub fn handle_lockfile_change(
    handle: &IndexHandle,
    workspace_root: &Path,
    changed_path: &Path,
) -> Result<Option<(SourceOrigin, usize)>, StorageError> {
    let Some(origin) = classify_lockfile_path(workspace_root, changed_path) else {
        return Ok(None);
    };
    let count = purge_externals_by_origin(handle, origin)?;
    let live = compute_lockfile_hashes(workspace_root)?;
    write_stored_hashes(handle, &live)?;
    Ok(Some((origin, count)))
}

fn classify_lockfile_path(workspace_root: &Path, changed_path: &Path) -> Option<SourceOrigin> {
    let rel_name = changed_path
        .strip_prefix(workspace_root)
        .ok()
        .and_then(|rel| rel.file_name())
        .and_then(|os| os.to_str())?;
    match rel_name {
        "Cargo.lock" => Some(SourceOrigin::CargoRegistry),
        "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock" | ".pnp.cjs" => {
            Some(SourceOrigin::NodeModulesDts)
        }
        _ => None,
    }
}

/// Returns the list of absolute paths the watcher should subscribe to
/// in addition to the workspace tree. Computed from the workspace root
/// so the watcher does not need to know about the lockfile catalog.
#[must_use]
pub fn tracked_lockfile_paths(workspace_root: &Path) -> Vec<PathBuf> {
    vec![
        workspace_root.join("Cargo.lock"),
        workspace_root.join("package-lock.json"),
        workspace_root.join("pnpm-lock.yaml"),
        workspace_root.join("yarn.lock"),
        workspace_root.join(".pnp.cjs"),
    ]
}

#[cfg(test)]
mod tests {
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
             VALUES (?1, ?2, 'function', 'fn', 'rust', ?1, 0, 0, 0, 0, 1, ?3)",
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
}

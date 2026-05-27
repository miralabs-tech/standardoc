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
/// stays `None` here — the `watcher`-side and the
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
mod tests;

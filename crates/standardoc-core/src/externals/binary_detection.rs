//! Binary probing + workspace fingerprinting for the external resolvers.
//!
//! Two responsibilities:
//!
//! 1. **Probe** : detect whether the binary backing a resolver (`cargo`,
//!    `node`, `luarocks`) is actually invokable. Each resolver consults
//!    an env-var override BEFORE the default binary name on `PATH`, so
//!    the VSCode extension can inject a user-configured path via
//!    `ServerOptions.options.env` (settings `standardoc.binaryPaths.*`).
//!
//! 2. **Workspace fingerprint** : answer "should this resolver even be
//!    constructed?" — e.g. skip the LuarocksResolver entirely when the
//!    workspace has zero `.lua` files. Probing a missing binary is fine
//!    (returns `Missing`); spamming `luarocks --version` on every Rust
//!    project is wasteful.
//!
//! ## Walk-down manifest discovery
//!
//! When the user opens a workspace whose root is a PARENT of the real
//! Rust / npm project (a monorepo layout: `monorepo/api/Cargo.toml`,
//! `monorepo/web/package.json`, …), the manifest probes
//! [`find_cargo_lock`] / [`find_package_json`] walk DOWN up to
//! [`SCAN_MAX_DEPTH`] levels to locate the manifest. The resolver is
//! anchored at the directory containing the manifest, not at the user's
//! workspace root — so `cargo metadata`, `node_modules` walks, etc.
//! operate on the correct tree. The shallowest match wins.
//!
//! ## `.stdignore` consultation
//!
//! [`workspace_has_lua_files`] respects the workspace's `.stdignore` so
//! a user can opt out of luarocks scanning by ignoring `.lua` files
//! (e.g. vendored Lua test fixtures inside an otherwise non-Lua
//! workspace). The other probes use direct `is_file` checks against
//! well-known manifest names that are NOT typically ignored.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use walkdir::WalkDir;

use crate::pipeline::ScanFilters;

/// Env var read first when probing the `cargo` binary. Falls back to
/// looking up `cargo` on `PATH` when unset. Set by the VSCode extension
/// from `standardoc.binaryPaths.cargo`.
pub const ENV_CARGO_PATH: &str = "STANDARDOC_CARGO_PATH";

/// Env var read first when probing the `node` binary (for yarn PnP
/// resolution). Falls back to `node` on `PATH`.
pub const ENV_NODE_PATH: &str = "STANDARDOC_NODE_PATH";

/// Env var read first when probing the `luarocks` binary. Falls back to
/// `luarocks` on `PATH`.
pub const ENV_LUAROCKS_PATH: &str = "STANDARDOC_LUAROCKS_PATH";

/// Latest probe result for a binary. Cached on the resolver instance —
/// refreshed only when an actual call reports `MissingBinary`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BinaryAvailability {
    /// Binary was located and `<binary> --version` exited cleanly.
    Available {
        path: PathBuf,
        version: Option<String>,
    },
    /// Binary was probed but failed to invoke. The resolver returns
    /// `MissingBinary` from any `resolve` call.
    Missing { detail: String },
    /// The workspace has no triggers for this resolver — e.g. no
    /// `.pnp.cjs` ⇒ `node` is `NotApplicable` regardless of PATH.
    /// Resolvers in this state are skipped at boot WARN time.
    NotApplicable,
}

/// Bounded walk depth used by manifest discovery + lua scanning.
/// Eight nested directories is generous enough to catch typical
/// monorepo layouts (`monorepo/<lang>/<crate>/Cargo.toml`,
/// `apps/<name>/package.json`, vendored Lua scripts in `vendor/`)
/// without descending into deep `node_modules`/`target` trees on pure
/// Rust+TS projects.
const SCAN_MAX_DEPTH: usize = 8;

/// Directory names that the bounded scanner refuses to descend into.
/// Keeps the probe cheap on workspaces that ship large vendored trees.
const SCAN_SKIP_DIRS: &[&str] = &[".git", "target", "node_modules", ".standardoc"];

/// Probes the given binary by running `<binary> --version`. Honours the
/// env override before falling back to the default name on PATH.
/// Returns `Missing` with a diagnostic detail when the spawn or the
/// exit status indicates a problem.
#[must_use]
pub fn probe_binary(default_name: &str, env_override: &str) -> BinaryAvailability {
    let path =
        std::env::var(env_override).map_or_else(|_| PathBuf::from(default_name), PathBuf::from);
    probe_path(&path)
}

/// Inner subprocess probe used by [`probe_binary`] AND by callers that
/// already know the absolute path (e.g. unit tests that inject a known
/// binary without going through the env-var indirection).
#[must_use]
pub(crate) fn probe_path(path: &Path) -> BinaryAvailability {
    let output = Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let version = String::from_utf8_lossy(&out.stdout)
                .lines()
                .next()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            BinaryAvailability::Available {
                path: path.to_path_buf(),
                version,
            }
        }
        Ok(out) => BinaryAvailability::Missing {
            detail: format!(
                "`{} --version` exited with status {:?}",
                path.display(),
                out.status.code()
            ),
        },
        Err(e) => BinaryAvailability::Missing {
            detail: format!("could not invoke `{}`: {e}", path.display()),
        },
    }
}

/// Returns the directory containing a `Cargo.lock` reachable from
/// `root`. Checks `root` itself first (typical case), then walks DOWN
/// up to [`SCAN_MAX_DEPTH`] levels to discover monorepo subprojects —
/// the shallowest match wins. Returns `None` when no `Cargo.lock`
/// is found anywhere within the bounded tree.
#[must_use]
pub fn find_cargo_lock(root: &Path) -> Option<PathBuf> {
    find_manifest_dir(root, "Cargo.lock")
}

/// Returns the directory containing a `package.json` reachable from
/// `root`. Same semantics as [`find_cargo_lock`] — root-first, then
/// bounded walk-down.
#[must_use]
pub fn find_package_json(root: &Path) -> Option<PathBuf> {
    find_manifest_dir(root, "package.json")
}

fn find_manifest_dir(root: &Path, filename: &str) -> Option<PathBuf> {
    // Fast path: most workspaces have the manifest at the root.
    if root.join(filename).is_file() {
        return Some(root.to_path_buf());
    }
    let mut candidates: Vec<(usize, PathBuf)> = WalkDir::new(root)
        .max_depth(SCAN_MAX_DEPTH)
        .into_iter()
        .filter_entry(|e| {
            !e.file_name()
                .to_str()
                .is_some_and(|n| SCAN_SKIP_DIRS.contains(&n))
        })
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && entry.file_name().to_str() == Some(filename))
        .filter_map(|entry| {
            entry
                .path()
                .parent()
                .map(|p| (entry.depth(), p.to_path_buf()))
        })
        .collect();
    candidates.sort_by_key(|(depth, _)| *depth);
    candidates.into_iter().next().map(|(_, p)| p)
}

/// `true` when the given directory holds a yarn-PnP manifest at its
/// top level. Triggers the `node`-subprocess probe in [`probe_binary`].
/// The NpmResolver is already anchored at the directory returned by
/// [`find_package_json`], so this direct-root check is correct without
/// further walking.
#[must_use]
pub fn workspace_has_pnp_cjs(root: &Path) -> bool {
    root.join(".pnp.cjs").is_file()
}

/// `true` when at least one `.lua` file exists anywhere under the
/// workspace root AFTER consulting the workspace's `.stdignore`. A
/// `.lua` file that matches an ignore pattern is treated as absent for
/// the purpose of LuarocksResolver registration. Bounded walk depth
/// (see [`SCAN_MAX_DEPTH`]) avoids descending into deep vendored trees
/// on pure Rust+TS projects.
#[must_use]
pub fn workspace_has_lua_files(root: &Path) -> bool {
    let filters = ScanFilters::load(root);
    WalkDir::new(root)
        .max_depth(SCAN_MAX_DEPTH)
        .into_iter()
        .filter_entry(|e| {
            !e.file_name()
                .to_str()
                .is_some_and(|n| SCAN_SKIP_DIRS.contains(&n))
        })
        .filter_map(Result::ok)
        .any(|entry| {
            if !entry.file_type().is_file() {
                return false;
            }
            if entry.path().extension().is_none_or(|ext| ext != "lua") {
                return false;
            }
            let Ok(rel) = entry.path().strip_prefix(root) else {
                return false;
            };
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            !filters.is_skipped(&rel_str)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::tempdir;

    #[test]
    fn probe_binary_marks_missing_when_default_name_does_not_resolve() {
        let result = probe_binary(
            "standardoc-nonexistent-binary-name-xyz",
            "STANDARDOC_PROBE_TEST_NONE",
        );
        match result {
            BinaryAvailability::Missing { detail } => {
                assert!(
                    detail.contains("standardoc-nonexistent-binary-name-xyz"),
                    "detail must name the binary, got `{detail}`"
                );
            }
            other => panic!("expected Missing, got {other:?}"),
        }
    }

    #[test]
    fn probe_path_marks_available_for_known_good_binary() {
        // `cargo` is guaranteed present in any environment running this
        // test suite. probe_path bypasses the env-var indirection so the
        // test does not need to mutate process env (workspace bans
        // unsafe std::env::set_var under unsafe_code = "forbid").
        match probe_path(Path::new("cargo")) {
            BinaryAvailability::Available { path, .. } => {
                assert_eq!(path, PathBuf::from("cargo"));
            }
            other => panic!("cargo must probe Available, got {other:?}"),
        }
    }

    #[test]
    fn probe_path_marks_missing_for_unknown_binary() {
        match probe_path(Path::new("standardoc-no-such-binary-test")) {
            BinaryAvailability::Missing { detail } => {
                assert!(
                    detail.contains("standardoc-no-such-binary-test"),
                    "detail must name the binary, got `{detail}`"
                );
            }
            other => panic!("expected Missing, got {other:?}"),
        }
    }

    #[test]
    fn find_cargo_lock_returns_root_when_present_at_top() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.lock"), "").unwrap();
        assert_eq!(find_cargo_lock(dir.path()), Some(dir.path().to_path_buf()));
    }

    #[test]
    fn find_cargo_lock_returns_none_on_empty_workspace() {
        let dir = tempdir().unwrap();
        assert_eq!(find_cargo_lock(dir.path()), None);
    }

    #[test]
    fn find_cargo_lock_walks_down_into_monorepo_subproject() {
        // Mimics `stdoc/standardoc/Cargo.lock` when the user opened
        // VSCode on `stdoc/` (parent). find_cargo_lock walks down and
        // returns `stdoc/standardoc/` as the anchor for cargo.
        let dir = tempdir().unwrap();
        let sub = dir.path().join("standardoc");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("Cargo.lock"), "").unwrap();
        assert_eq!(find_cargo_lock(dir.path()), Some(sub));
    }

    #[test]
    fn find_cargo_lock_prefers_shallowest_match() {
        let dir = tempdir().unwrap();
        let shallow = dir.path().join("api");
        let deep = dir.path().join("vendor").join("nested").join("crate");
        std::fs::create_dir_all(&shallow).unwrap();
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(shallow.join("Cargo.lock"), "").unwrap();
        std::fs::write(deep.join("Cargo.lock"), "").unwrap();
        assert_eq!(find_cargo_lock(dir.path()), Some(shallow));
    }

    #[test]
    fn find_cargo_lock_skips_target_dir() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("target").join("package");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("Cargo.lock"), "").unwrap();
        assert_eq!(
            find_cargo_lock(dir.path()),
            None,
            "Cargo.lock under target/ must be ignored (build artifact)"
        );
    }

    #[test]
    fn find_package_json_returns_root_when_present() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        assert_eq!(
            find_package_json(dir.path()),
            Some(dir.path().to_path_buf())
        );
    }

    #[test]
    fn find_package_json_walks_down_into_subdir() {
        let dir = tempdir().unwrap();
        let sub = dir.path().join("apps").join("web");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("package.json"), "{}").unwrap();
        assert_eq!(find_package_json(dir.path()), Some(sub));
    }

    #[test]
    fn find_package_json_skips_node_modules_dir() {
        let dir = tempdir().unwrap();
        let inner = dir.path().join("node_modules").join("react");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(inner.join("package.json"), "{}").unwrap();
        assert_eq!(
            find_package_json(dir.path()),
            None,
            "package.json under node_modules/ must be ignored (dependency, not workspace)"
        );
    }

    #[test]
    fn workspace_has_pnp_cjs_true_when_file_present() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".pnp.cjs"), "// pnp").unwrap();
        assert!(workspace_has_pnp_cjs(dir.path()));
    }

    #[test]
    fn workspace_has_lua_files_true_when_top_level_lua_present() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("main.lua"), "").unwrap();
        assert!(workspace_has_lua_files(dir.path()));
    }

    #[test]
    fn workspace_has_lua_files_true_when_nested_lua_present() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("scripts/sub")).unwrap();
        std::fs::write(dir.path().join("scripts/sub/init.lua"), "").unwrap();
        assert!(workspace_has_lua_files(dir.path()));
    }

    #[test]
    fn workspace_has_lua_files_false_when_lua_only_inside_skipped_dirs() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/pkg")).unwrap();
        std::fs::write(dir.path().join("node_modules/pkg/inner.lua"), "").unwrap();
        std::fs::create_dir_all(dir.path().join("target/debug")).unwrap();
        std::fs::write(dir.path().join("target/debug/leftover.lua"), "").unwrap();
        assert!(
            !workspace_has_lua_files(dir.path()),
            ".lua under skip-listed dirs must not trigger the LuarocksResolver gate"
        );
    }

    #[test]
    fn workspace_has_lua_files_false_on_pure_rust_workspace() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();
        assert!(!workspace_has_lua_files(dir.path()));
    }

    #[test]
    fn workspace_has_lua_files_respects_stdignore_pattern() {
        // .lua exists but is matched by .stdignore → resolver should
        // NOT register. Lets a Rust-only user with vendored Lua test
        // fixtures opt out of luarocks without deleting the files.
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("vendor")).unwrap();
        std::fs::write(dir.path().join("vendor/sample.lua"), "").unwrap();
        std::fs::write(dir.path().join(".stdignore"), "vendor/\n").unwrap();
        assert!(
            !workspace_has_lua_files(dir.path()),
            ".lua matched by .stdignore must not trigger the LuarocksResolver gate"
        );
    }

    #[test]
    fn workspace_has_lua_files_finds_non_ignored_when_some_are_ignored() {
        // Two .lua files: one under vendor/ (ignored), one at root (kept).
        // The probe must still return true on the kept one.
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("vendor")).unwrap();
        std::fs::write(dir.path().join("vendor/skip.lua"), "").unwrap();
        std::fs::write(dir.path().join("plugin.lua"), "").unwrap();
        std::fs::write(dir.path().join(".stdignore"), "vendor/\n").unwrap();
        assert!(workspace_has_lua_files(dir.path()));
    }
}

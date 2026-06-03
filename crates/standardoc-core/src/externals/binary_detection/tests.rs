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

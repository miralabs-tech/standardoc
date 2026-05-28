use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;

fn write_sample_workspace(root: &Path) {
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/lib.rs"),
        "pub fn alpha() {}\npub fn beta() { alpha(); }\n",
    )
    .unwrap();
}

fn standardoc() -> Command {
    Command::cargo_bin("standardoc").expect("standardoc binary must build")
}

#[test]
fn index_succeeds_on_sample_workspace() {
    let dir = tempfile::tempdir().unwrap();
    write_sample_workspace(dir.path());

    standardoc().arg("index").arg(dir.path()).assert().success();
}

#[test]
fn query_fqdn_returns_symbol_after_index() {
    let dir = tempfile::tempdir().unwrap();
    write_sample_workspace(dir.path());

    standardoc().arg("index").arg(dir.path()).assert().success();

    standardoc()
        .arg("query")
        .arg(dir.path())
        .args(["--fqdn", "sample::alpha"])
        .assert()
        .success()
        .stdout(predicate::str::contains("sample::alpha"))
        .stdout(predicate::str::contains("Callable"));
}

#[test]
fn query_name_lists_matches() {
    let dir = tempfile::tempdir().unwrap();
    write_sample_workspace(dir.path());

    standardoc().arg("index").arg(dir.path()).assert().success();

    standardoc()
        .arg("query")
        .arg(dir.path())
        .args(["--name", "alpha"])
        .assert()
        .success()
        .stdout(predicate::str::contains("sample::alpha"));
}

#[test]
fn query_text_uses_fts() {
    let dir = tempfile::tempdir().unwrap();
    write_sample_workspace(dir.path());

    standardoc().arg("index").arg(dir.path()).assert().success();

    standardoc()
        .arg("query")
        .arg(dir.path())
        .args(["--text", "alpha"])
        .assert()
        .success()
        .stdout(predicate::str::contains("sample::alpha"));
}

#[test]
fn query_edges_from_lists_resolved_callee() {
    let dir = tempfile::tempdir().unwrap();
    write_sample_workspace(dir.path());

    standardoc().arg("index").arg(dir.path()).assert().success();

    standardoc()
        .arg("query")
        .arg(dir.path())
        .args(["--fqdn", "sample::beta", "--edges-from"])
        .assert()
        .success()
        .stdout(predicate::str::contains("sample::beta"))
        .stdout(predicate::str::contains("sample::alpha"))
        .stdout(predicate::str::contains("Calls"));
}

#[test]
fn query_without_selector_fails() {
    let dir = tempfile::tempdir().unwrap();
    write_sample_workspace(dir.path());

    standardoc().arg("query").arg(dir.path()).assert().failure();
}

#[test]
fn query_fqdn_and_name_conflict() {
    let dir = tempfile::tempdir().unwrap();
    write_sample_workspace(dir.path());

    standardoc()
        .arg("query")
        .arg(dir.path())
        .args(["--fqdn", "sample::alpha", "--name", "alpha"])
        .assert()
        .failure();
}

#[test]
fn query_unknown_fqdn_prints_friendly_message() {
    let dir = tempfile::tempdir().unwrap();
    write_sample_workspace(dir.path());

    standardoc().arg("index").arg(dir.path()).assert().success();

    standardoc()
        .arg("query")
        .arg(dir.path())
        .args(["--fqdn", "sample::ghost"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no symbol found"));
}

#[test]
fn rescan_succeeds_after_index() {
    let dir = tempfile::tempdir().unwrap();
    write_sample_workspace(dir.path());

    standardoc().arg("index").arg(dir.path()).assert().success();
    standardoc()
        .arg("rescan")
        .arg(dir.path())
        .assert()
        .success();
}

#[test]
fn purge_excluded_no_op_when_nothing_matches() {
    let dir = tempfile::tempdir().unwrap();
    write_sample_workspace(dir.path());

    standardoc().arg("index").arg(dir.path()).assert().success();

    standardoc()
        .arg("purge-excluded")
        .arg(dir.path())
        .arg("--yes")
        .assert()
        .success()
        .stdout(predicate::str::contains("(nothing to purge)"));
}

#[test]
fn purge_excluded_yes_flag_purges_matching_rows() {
    let dir = tempfile::tempdir().unwrap();
    write_sample_workspace(dir.path());
    fs::create_dir_all(dir.path().join("vendored")).unwrap();
    fs::write(
        dir.path().join("vendored/lib.rs"),
        "pub fn vendored_fn() {}\n",
    )
    .unwrap();

    // Index first (vendored/ has no exclusion yet → indexed).
    standardoc().arg("index").arg(dir.path()).assert().success();

    // Now exclude vendored/ via the ignore block in standardoc.sxd
    // (the .sxd is auto-seeded by `index`'s IndexHandle::open).
    extend_sxd_ignore(dir.path(), "vendored/");

    standardoc()
        .arg("purge-excluded")
        .arg(dir.path())
        .arg("--yes")
        .assert()
        .success()
        .stdout(predicate::str::contains("vendored/lib.rs"))
        .stdout(predicate::str::contains("purged 1 path"));
}

/// Append a single line to the `ignore { patterns ```...``` }` block of
/// `standardoc.sxd`. Used by tests that need to extend exclusions
/// post-seed without re-authoring the whole file.
fn extend_sxd_ignore(root: &Path, line: &str) {
    let sxd_path = root.join("standardoc.sxd");
    let body = fs::read_to_string(&sxd_path).expect("seed sxd present");
    // Closing ``` fence — insert above it.
    let needle = "```\n}";
    let injected = format!("{line}\n```\n}}");
    let new = body.replacen(needle, &injected, 1);
    assert_ne!(
        body, new,
        "sxd template must contain the ignore close fence"
    );
    fs::write(&sxd_path, new).unwrap();
}

fn write_mixed_workspace(root: &Path) {
    fs::create_dir_all(root.join("src-tauri/src")).unwrap();
    fs::write(
        root.join("src-tauri/Cargo.toml"),
        "[package]\nname = \"sample-api\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::write(
        root.join("src-tauri/src/lib.rs"),
        "pub fn alpha() {}\npub fn beta() { alpha(); }\n",
    )
    .unwrap();

    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("package.json"),
        "{\"name\":\"@app/web\",\"version\":\"0.1.0\"}",
    )
    .unwrap();
    fs::write(
        root.join("src/index.ts"),
        "import { login } from \"./auth\";\nexport function main() { login(); }\n",
    )
    .unwrap();
    fs::write(
        root.join("src/auth.ts"),
        "export function login(): string { return \"ok\"; }\n",
    )
    .unwrap();
}

#[test]
fn cli_index_then_query_on_mixed_workspace() {
    let dir = tempfile::tempdir().unwrap();
    write_mixed_workspace(dir.path());

    standardoc().arg("index").arg(dir.path()).assert().success();

    standardoc()
        .arg("query")
        .arg(dir.path())
        .args(["--fqdn", "@app/web::src::main"])
        .assert()
        .success()
        .stdout(predicate::str::contains("@app/web::src::main"))
        .stdout(predicate::str::contains("Callable"));

    standardoc()
        .arg("query")
        .arg(dir.path())
        .args(["--fqdn", "sample-api::beta"])
        .assert()
        .success()
        .stdout(predicate::str::contains("sample-api::beta"));
}

#[test]
fn cli_query_text_finds_ts_symbol_via_fts() {
    let dir = tempfile::tempdir().unwrap();
    write_mixed_workspace(dir.path());

    standardoc().arg("index").arg(dir.path()).assert().success();

    standardoc()
        .arg("query")
        .arg(dir.path())
        .args(["--text", "login"])
        .assert()
        .success()
        .stdout(predicate::str::contains("@app/web::src::auth::login"));
}

#[test]
fn purge_excluded_requires_yes_in_non_interactive_shell() {
    let dir = tempfile::tempdir().unwrap();
    write_sample_workspace(dir.path());
    fs::create_dir_all(dir.path().join("vendored")).unwrap();
    fs::write(
        dir.path().join("vendored/lib.rs"),
        "pub fn vendored_fn() {}\n",
    )
    .unwrap();

    standardoc().arg("index").arg(dir.path()).assert().success();

    extend_sxd_ignore(dir.path(), "vendored/");

    standardoc()
        .arg("purge-excluded")
        .arg(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("non-interactive"));
}

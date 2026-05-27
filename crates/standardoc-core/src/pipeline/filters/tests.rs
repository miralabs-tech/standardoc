
use std::fs;

use tempfile::tempdir;

use super::*;

fn write(root: &Path, rel: &str, body: &str) {
    let abs = root.join(rel);
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(abs, body).unwrap();
}

#[test]
fn is_ignored_returns_false_when_no_layer() {
    let dir = tempdir().unwrap();
    let stack = GitignoreStack::build(dir.path());
    assert!(!stack.is_ignored("src/lib.rs"));
    assert!(!stack.is_ignored("anything"));
}

#[test]
fn gitignore_stack_root_only() {
    let dir = tempdir().unwrap();
    write(dir.path(), ".stdignore", "target/\nbuild/\n");

    let stack = GitignoreStack::build(dir.path());

    assert!(stack.is_ignored("target/debug/foo.rs"));
    assert!(stack.is_ignored("build/output.js"));
    assert!(!stack.is_ignored("src/lib.rs"));
}

#[test]
fn gitignore_stack_nested_layers_extend_root() {
    let dir = tempdir().unwrap();
    write(dir.path(), ".stdignore", "target/\n");
    write(dir.path(), "crates/.stdignore", "vendor/\n");

    let stack = GitignoreStack::build(dir.path());

    assert!(stack.is_ignored("target/debug.rs"));
    assert!(stack.is_ignored("crates/vendor/lib.rs"));
    assert!(!stack.is_ignored("crates/src/lib.rs"));
}

#[test]
fn gitignore_stack_negation_override() {
    let dir = tempdir().unwrap();
    write(dir.path(), ".stdignore", "build/\n");
    write(dir.path(), "crates/.stdignore", "!build/\n");

    let stack = GitignoreStack::build(dir.path());

    assert!(stack.is_ignored("build/output.js"));
    assert!(!stack.is_ignored("crates/build/output.js"));
}

#[test]
fn is_ignored_matches_glob_patterns() {
    let dir = tempdir().unwrap();
    write(dir.path(), ".stdignore", "*.lock\n**/generated/**\n");

    let stack = GitignoreStack::build(dir.path());

    assert!(stack.is_ignored("Cargo.lock"));
    assert!(stack.is_ignored("crates/foo/generated/api.ts"));
    assert!(!stack.is_ignored("Cargo.toml"));
}

#[test]
fn build_skips_descent_into_excluded_subtrees() {
    let dir = tempdir().unwrap();
    write(dir.path(), ".stdignore", "target/\n");
    write(dir.path(), "target/.stdignore", "!debug/\n");

    let stack = GitignoreStack::build(dir.path());

    assert!(
        stack.is_ignored("target/debug/foo.rs"),
        "nested .stdignore inside an excluded subtree must not be discovered"
    );
}

#[test]
fn ensure_stdignore_seed_writes_when_absent() {
    let dir = tempdir().unwrap();
    ensure_stdignore_seed_at(dir.path()).unwrap();

    let path = dir.path().join(STDIGNORE_FILENAME);
    let body = fs::read_to_string(&path).unwrap();
    assert!(body.contains(".git/"));
    assert!(body.contains("target/"));
    assert!(body.contains("node_modules/"));
    assert!(body.contains("dist/"));
    assert!(body.contains("build/"));
    assert!(body.contains(".old/"));
    assert!(body.contains("*-old/"));
    assert!(body.contains("test-export/"));
    assert!(
        !body.contains(".standardoc/"),
        "seed must not include .standardoc/ (user decision lock 21 Q3)"
    );
}

#[test]
fn ensure_stdignore_seed_preserves_existing_file() {
    let dir = tempdir().unwrap();
    let existing = "# my own exclusions\nfoo/\n";
    write(dir.path(), STDIGNORE_FILENAME, existing);

    ensure_stdignore_seed_at(dir.path()).unwrap();

    let body = fs::read_to_string(dir.path().join(STDIGNORE_FILENAME)).unwrap();
    assert_eq!(body, existing);
}

#[test]
fn ensure_stdignore_seed_preserves_empty_file() {
    let dir = tempdir().unwrap();
    write(dir.path(), STDIGNORE_FILENAME, "");

    ensure_stdignore_seed_at(dir.path()).unwrap();

    let body = fs::read_to_string(dir.path().join(STDIGNORE_FILENAME)).unwrap();
    assert!(
        body.is_empty(),
        "an existing empty .stdignore must stay empty"
    );
}

#[test]
fn scan_filters_load_constructs_from_workspace_root() {
    let dir = tempdir().unwrap();
    write(dir.path(), ".stdignore", "target/\n");

    let filters = ScanFilters::load(dir.path());

    assert!(filters.is_skipped("target/debug.rs"));
    assert!(!filters.is_skipped("src/lib.rs"));
}

#[test]
fn is_ignored_handles_root_path_safely() {
    let dir = tempdir().unwrap();
    let stack = GitignoreStack::build(dir.path());
    assert!(!stack.is_ignored(""));
}

#[test]
fn preview_pattern_matches_returns_paths_under_target_directory() {
    let dir = tempdir().unwrap();
    write(dir.path(), "src/lib.rs", "");
    write(dir.path(), "target/debug/foo.rs", "");
    write(dir.path(), "target/release/bar.rs", "");
    let preview = preview_pattern_matches(dir.path(), "target/", 20).unwrap();
    assert_eq!(preview.pattern, "target/");
    assert!(
        preview
            .matches
            .iter()
            .any(|p| p == "target" || p == "target/debug" || p == "target/debug/foo.rs")
    );
    // Three entries under target/ (target dir + debug + release + 2 files)
    // — count is whatever the walker enumerates, all of which match.
    assert!(preview.total_count >= 3);
    assert!(!preview.matches.iter().any(|p| p == "src/lib.rs"));
    assert!(!preview.walk_truncated);
}

#[test]
fn preview_pattern_matches_returns_empty_for_blank_or_comment() {
    let dir = tempdir().unwrap();
    write(dir.path(), "src/lib.rs", "");
    for blank in &["", "   ", "# comment line", "  # indented comment"] {
        let preview = preview_pattern_matches(dir.path(), blank, 20).unwrap();
        assert!(preview.matches.is_empty());
        assert_eq!(preview.total_count, 0);
    }
}

#[test]
fn preview_pattern_matches_truncates_at_limit_while_counting_total() {
    let dir = tempdir().unwrap();
    for i in 0..10 {
        write(dir.path(), &format!("logs/file{i}.log"), "");
    }
    let preview = preview_pattern_matches(dir.path(), "*.log", 3).unwrap();
    assert_eq!(preview.matches.len(), 3);
    assert_eq!(preview.total_count, 10);
    assert!(preview.truncated);
}

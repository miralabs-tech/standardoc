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
fn sxd_root_patterns_prune_descent_into_excluded_dirs() {
    // FI4 regression: when the root ignore comes from sxd patterns (no
    // physical root `.stdignore`), the nested-layer walk must still be
    // pruned by those patterns. Otherwise it descends into `target/` and
    // loads `target/.stdignore`, which — against git semantics — would
    // re-include a path the root pattern excluded.
    let dir = tempdir().unwrap();
    write(dir.path(), "target/.stdignore", "!important.rs\n");
    write(dir.path(), "target/important.rs", "");

    let stack = GitignoreStack::build_with_root_patterns(dir.path(), "target/\n");

    assert!(
        stack.is_ignored("target/important.rs"),
        "a file under an sxd-excluded dir must stay ignored even if a nested \
         .stdignore tries to re-include it (descent walk must be pruned)"
    );
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

// Bug E-3 follow-up P2 — legacy `ensure_stdignore_seed_at` tests are
// gone ; the equivalent behaviour for `standardoc.sxd` lives in
// `config::sxd::tests`. See `ensure_sxd_seed_*` test family there.

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

// --- Bug E-3 follow-up : standardoc.sxd back-compat tests ---

#[test]
fn build_with_root_patterns_overrides_stdignore() {
    // .sxd ignore patterns take precedence over .stdignore at root.
    let dir = tempdir().unwrap();
    write(dir.path(), ".stdignore", "build/\n");
    let stack = GitignoreStack::build_with_root_patterns(dir.path(), "target/\n");

    assert!(stack.is_ignored("target/debug.rs"));
    // .stdignore root layer is shadowed by the .sxd patterns.
    assert!(!stack.is_ignored("build/out.js"));
}

#[test]
fn build_with_root_patterns_skips_blank_and_comment_lines() {
    let dir = tempdir().unwrap();
    let patterns = "\n# leading comment\ntarget/\n\n# trailing\nbuild/\n";
    let stack = GitignoreStack::build_with_root_patterns(dir.path(), patterns);
    assert!(stack.is_ignored("target/foo"));
    assert!(stack.is_ignored("build/bar"));
}

#[test]
fn scan_filters_load_reads_sxd_when_present() {
    let dir = tempdir().unwrap();
    write(dir.path(), ".stdignore", "build/\n");
    write(
        dir.path(),
        "standardoc.sxd",
        "version \"0.1.0\"\nignore { patterns ```\ntarget/\n``` }\n",
    );
    let filters = ScanFilters::load(dir.path());
    assert!(filters.is_skipped("target/x"));
    // .sxd's ignore block replaces the root .stdignore patterns.
    assert!(!filters.is_skipped("build/y"));
}

#[test]
fn scan_filters_load_falls_back_to_stdignore_when_sxd_absent() {
    let dir = tempdir().unwrap();
    write(dir.path(), ".stdignore", "target/\n");
    let filters = ScanFilters::load(dir.path());
    assert!(filters.is_skipped("target/x"));
}

#[test]
fn scan_filters_load_falls_back_to_stdignore_when_sxd_has_no_ignore_block() {
    let dir = tempdir().unwrap();
    write(dir.path(), ".stdignore", "target/\n");
    write(dir.path(), "standardoc.sxd", "version \"0.1.0\"\n");
    let filters = ScanFilters::load(dir.path());
    assert!(filters.is_skipped("target/x"));
}

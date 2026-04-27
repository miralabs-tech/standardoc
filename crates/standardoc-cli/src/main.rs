//! Standardoc CLI entry point.
//!
//! Commands implemented:
//! - `scan <path>` — print canonical `DocBlock`s as JSON
//! - `transform <path> <template.md>` — render a markdown template against a scan
//! - `emit <format> <path>` — emit `llms`/`llms-full`/`skill` documentation
//! - `validate <path>` — run validator rules, exit 1 on errors
//! - `materialize <path>` — write virtual annotations into source as `///` comments

use standardoc_core::config::Config;
use standardoc_core::dsl::render_string;
use standardoc_core::emit::{emit_llms_full, emit_llms_txt, emit_skill_md, EmitOptions};
use standardoc_core::materialize::{apply_to_disk, plan, ConfidenceFilter, MaterializePlan};
use standardoc_core::model::{DocBlock, Severity};
use standardoc_core::pipeline::{scan_and_extract, PipelineReport};
use standardoc_core::scanner::Registry;
use standardoc_core::validator::validate;
use standardoc_lang_python::PythonProvider;
use standardoc_lang_rust::RustProvider;
use standardoc_lang_tree_sitter::TreeSitterProvider;
use standardoc_lang_ts::TsProvider;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("scan") => run_scan(&args[1..]),
        Some("transform") => run_transform(&args[1..]),
        Some("emit") => run_emit(&args[1..]),
        Some("validate") => run_validate(&args[1..]),
        Some("materialize") => run_materialize(&args[1..]),
        Some("--help" | "-h") | None => {
            print_help();
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown command: {other}");
            print_help();
            ExitCode::from(2)
        }
    }
}

/// @doc cli.commands.help --help
/// @category meta
/// @since 0.1
/// @usage standardoc --help
/// @description
/// Print the command list with brief usage. Always exits `0`.
fn print_help() {
    println!(
        "standardoc — scaffold\n\n\
         USAGE:\n  \
         standardoc scan <path>\n      \
             Scan a directory and print canonical DocBlocks as JSON.\n  \
         standardoc transform <path> <template.md>\n      \
             Scan <path>, then render <template.md> with the DSL.\n  \
         standardoc emit <format> <path> [--name <project>] [--tagline <line>] [--link-base <url>]\n      \
             Scan <path>, then emit one of: llms, llms-full, skill.\n      \
             Outputs to stdout. Redirect to a file with `>`.\n  \
         standardoc validate <path>\n      \
             Run validator rules. Exit 1 if any error-level diagnostic.\n  \
         standardoc materialize <path> [--apply] [--confidence low|medium|high]\n      \
             Write virtual annotations into source as `///` doc-comments.\n      \
             Defaults to a dry-run; pass --apply to actually edit files.\n      \
             Default --confidence is `medium` (skip low-tier suggestions).\n  \
         standardoc --help\n      \
             Show this help."
    );
}

fn build_registry(workspace: &Path) -> Registry {
    use standardoc_core::lang_def::{load_workspace_languages, LanguageBackend};
    use standardoc_core::lang_regex::RegexProvider;
    let mut builder = Registry::builder()
        .with(RustProvider)
        .with(TsProvider)
        .with(PythonProvider)
        .with(TreeSitterProvider::lua());
    for def in load_workspace_languages(workspace) {
        builder = match &def.backend {
            LanguageBackend::TreeSitterFork { .. } => match TreeSitterProvider::from_lang_def(&def)
            {
                Ok(p) => {
                    eprintln!("standardoc: loaded tree-sitter fork '{}'", def.id);
                    builder.with(p)
                }
                Err(err) => {
                    eprintln!("standardoc: skipping '{}': {err}", def.id);
                    builder
                }
            },
            LanguageBackend::Regex { .. } => match RegexProvider::from_lang_def(&def) {
                Ok(p) => {
                    eprintln!("standardoc: loaded regex provider '{}'", def.id);
                    builder.with(p)
                }
                Err(err) => {
                    eprintln!("standardoc: skipping '{}': {err}", def.id);
                    builder
                }
            },
        };
    }
    builder.build()
}

/// @doc cli.commands.scan scan
/// @category index
/// @since 0.1
/// @usage `standardoc scan <path>`
/// @description
/// Walk `<path>` and emit canonical [`DocBlock`](../crates/standardoc-core/src/model/)
/// entries as JSON, one block per record.
///
/// Useful for : piping into `jq`, building external tooling, debugging discovery,
/// snapshot diffs in CI.
///
/// **Exit codes** :
/// - `0` — success
/// - `1` — pipeline error (unreadable path, parse failure)
/// - `2` — missing required argument
///
/// **Example** :
/// ```sh
/// standardoc scan ./my-project | jq '.[] | {key, kind: .symbol.kind}'
/// ```
fn run_scan(args: &[String]) -> ExitCode {
    let Some(root) = args.first() else {
        eprintln!("usage: standardoc scan <path>");
        return ExitCode::from(2);
    };
    let report = match run_pipeline(Path::new(root)) {
        Ok(r) => r,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
    };

    let ordered: Vec<DocBlock> = report.blocks.into_values().collect();
    match serde_json::to_string_pretty(&ordered) {
        Ok(json) => println!("{json}"),
        Err(err) => {
            eprintln!("failed to serialize blocks: {err}");
            return ExitCode::from(1);
        }
    }
    eprintln!(
        "scanned {} block(s), {} error(s), {} key collision(s)",
        ordered.len(),
        report.errors.len(),
        report.collisions.len()
    );
    for err in &report.errors {
        eprintln!("error: {err}");
    }
    for collision in &report.collisions {
        eprintln!(
            "collision: key '{}' kept {}:{} — dropped {}",
            collision.key,
            collision.kept.path.display(),
            collision.kept.line,
            collision
                .dropped
                .iter()
                .map(|p| format!("{}:{}", p.path.display(), p.line))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if report.errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// @doc cli.commands.transform transform
/// @category render
/// @since 0.1
/// @usage `standardoc transform <path> <template.md>`
/// @description
/// Scan `<path>`, then render `<template.md>` against the resulting index. The
/// template uses the standardoc DSL (`{{ @doc.KEY:tag }}`,
/// `{{ each x in @docs.module(...) }}`, `{{ if ... }}`, …). Result printed to stdout.
///
/// **Exit codes** :
/// - `0` — render OK
/// - `1` — pipeline or render error
/// - `2` — missing argument
///
/// **Example** :
/// ```sh
/// standardoc transform ./my-project ./docs-src/api.md > ./public/api.md
/// ```
fn run_transform(args: &[String]) -> ExitCode {
    let (Some(root), Some(template_path)) = (args.first(), args.get(1)) else {
        eprintln!("usage: standardoc transform <path> <template.md>");
        return ExitCode::from(2);
    };
    let template_src = match std::fs::read_to_string(template_path) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("cannot read template '{template_path}': {err}");
            return ExitCode::from(1);
        }
    };
    let (report, config) = match run_pipeline_with_config(Path::new(root)) {
        Ok(r) => r,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
    };
    match render_string(&template_src, &report.blocks, &config.tags) {
        Ok(rendered) => {
            print!("{rendered}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("render error: {err}");
            ExitCode::from(1)
        }
    }
}

/// @doc cli.commands.emit emit
/// @category emit
/// @since 0.1
/// @usage `standardoc emit <format> <path> [--name <project>] [--tagline <line>] [--link-base <url>]`
/// @description
/// Generate one of three agent-oriented documentation standards from a workspace scan.
///
/// **Formats** :
/// - `llms` (alias `llms.txt`) — [Jeremy Howard's `llms.txt`](https://llmstxt.org/) summary index
/// - `llms-full` (alias `llms-full.txt`) — `llms-full.txt` long-form variant
/// - `skill` (alias `skill.md`) — Claude Code [`SKILL.md`](https://docs.anthropic.com/en/docs/claude-code/skills) format
///
/// **Options** :
/// - `--name <project>` — overrides the auto-detected project name (default : the workspace root directory name)
/// - `--tagline <line>` — short description embedded in the output header
/// - `--link-base <url>` — base URL prefix for source links (e.g. `https://github.com/owner/repo/blob/main`)
///
/// Output goes to stdout. Redirect with `>` to write a file.
///
/// **Example** :
/// ```sh
/// standardoc emit llms ./my-project \
///   --name "My Project" \
///   --tagline "REST API for X" \
///   --link-base "https://github.com/owner/repo/blob/main" \
///   > llms.txt
/// ```
fn run_emit(args: &[String]) -> ExitCode {
    let (Some(format), Some(root)) = (args.first(), args.get(1)) else {
        eprintln!("usage: standardoc emit <llms|llms-full|skill> <path> [--name N] [--tagline T] [--link-base U]");
        return ExitCode::from(2);
    };

    let mut opts = EmitOptions::default();
    let mut iter = args[2..].iter();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--name" => opts.project_name = iter.next().cloned(),
            "--tagline" => opts.tagline = iter.next().cloned(),
            "--link-base" => opts.link_base = iter.next().cloned(),
            other => {
                eprintln!("unknown emit flag: {other}");
                return ExitCode::from(2);
            }
        }
    }

    let report = match run_pipeline(Path::new(root)) {
        Ok(r) => r,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
    };

    // If no explicit project name is provided, derive it from root directory
    // name — convenient for dogfooding (`standardoc/` -> "standardoc").
    if opts.project_name.is_none() {
        opts.project_name = report
            .workspace_root
            .file_name()
            .and_then(|n| n.to_str())
            .map(ToOwned::to_owned);
    }

    let output = match format.as_str() {
        "llms" | "llms.txt" => emit_llms_txt(&report.blocks, &opts),
        "llms-full" | "llms-full.txt" => emit_llms_full(&report.blocks, &opts),
        "skill" | "skill.md" => emit_skill_md(&report.blocks, &opts),
        other => {
            eprintln!("unknown format '{other}' (expected: llms, llms-full, skill)");
            return ExitCode::from(2);
        }
    };
    print!("{output}");
    ExitCode::SUCCESS
}

/// @doc cli.commands.validate validate
/// @category validate
/// @since 0.1
/// @usage `standardoc validate <path>`
/// @description
/// Run the full validator suite over a workspace, print one diagnostic per line in the
/// format `<severity> [STD###] <path>:<line>: <message>`. A summary count is printed
/// to stderr.
///
/// **Severities** : `error`, `warning`, `info`, `hint` — see the
/// [validator rules table in README.md](../README.md#validator) for the full list.
///
/// **Exit codes** :
/// - `0` — no error-severity diagnostic found (warnings/info/hints don't fail)
/// - `1` — at least one `error` diagnostic
/// - `2` — missing argument
///
/// **Example** :
/// ```sh
/// standardoc validate ./my-project
/// # error [STD001] src/lib.rs:42: duplicate DocKey "foo.bar"
/// # warning [STD006] src/lib.rs:10: public symbol with no @doc annotation
/// # 1 error(s), 1 warning(s), 0 info, 0 hint(s)
/// ```
///
/// CI integration : run `standardoc validate .` as a step; non-zero exit blocks the merge.
fn run_validate(args: &[String]) -> ExitCode {
    let Some(root) = args.first() else {
        eprintln!("usage: standardoc validate <path>");
        return ExitCode::from(2);
    };
    let (report, config) = match run_pipeline_with_config(Path::new(root)) {
        Ok(r) => r,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
    };
    let diagnostics = validate(&report.blocks, &report.collisions, &report.pages, &config);

    let mut error_count = 0_usize;
    let mut warning_count = 0_usize;
    let mut info_count = 0_usize;
    let mut hint_count = 0_usize;
    for d in &diagnostics {
        match d.severity {
            Severity::Error => error_count += 1,
            Severity::Warning => warning_count += 1,
            Severity::Info => info_count += 1,
            Severity::Hint => hint_count += 1,
        }
        let prefix = match d.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
            Severity::Hint => "hint",
        };
        println!(
            "{prefix} [{code}] {path}:{line}: {message}",
            code = d.code.as_str(),
            path = d.path.display(),
            line = d.range.line_start,
            message = d.message,
        );
    }

    eprintln!(
        "\n{error_count} error(s), {warning_count} warning(s), {info_count} info, {hint_count} hint(s)"
    );
    if error_count > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// @doc cli.commands.materialize materialize
/// @category migrate
/// @since 0.1
/// @usage `standardoc materialize <path> [--apply] [--confidence low|medium|high]`
/// @description
/// Promote virtual annotations (synthesized by the virtual-annotation pass on
/// `Inferred` blocks) into real source-level `///` doc-comments. Defaults to a dry-run
/// that prints exactly what would be inserted, file-by-file ; pass `--apply` to actually
/// edit the source.
///
/// **Options** :
/// - `--apply` — perform the edits. Without this flag, only a dry-run report is printed.
/// - `--confidence <tier>` — minimum confidence required for a virtual annotation to be
///   eligible. `low` (everything), `medium` (default), `high` (only the most confident
///   templates : constructors, trait impls, predicates, etc.).
///
/// The output respects the language's preferred doc-comment syntax (`///` for Rust,
/// `---` for Lua, `/** … */` for TS/JS) and preserves the indentation of the symbol it
/// documents. Python is intentionally unsupported in this MVP — docstrings live inside
/// the function body, which needs different placement logic.
///
/// **Exit codes** :
/// - `0` — dry-run printed, or `--apply` succeeded
/// - `1` — pipeline error or write failure
/// - `2` — bad argument
///
/// **Example** :
/// ```sh
/// # Preview what would be added on the public API
/// standardoc materialize ./my-project --confidence high
///
/// # Actually write
/// standardoc materialize ./my-project --confidence high --apply
/// ```
fn run_materialize(args: &[String]) -> ExitCode {
    let Some(root) = args.first() else {
        eprintln!("usage: standardoc materialize <path> [--apply] [--confidence low|medium|high]");
        return ExitCode::from(2);
    };
    let mut apply = false;
    let mut filter = ConfidenceFilter::AtLeastMedium;
    let mut iter = args[1..].iter();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--apply" => apply = true,
            "--confidence" => match iter.next().map(String::as_str) {
                Some("low") => filter = ConfidenceFilter::AtLeastLow,
                Some("medium") => filter = ConfidenceFilter::AtLeastMedium,
                Some("high") => filter = ConfidenceFilter::AtLeastHigh,
                other => {
                    eprintln!(
                        "--confidence requires low|medium|high (got {})",
                        other.unwrap_or("nothing")
                    );
                    return ExitCode::from(2);
                }
            },
            other => {
                eprintln!("unknown materialize flag: {other}");
                return ExitCode::from(2);
            }
        }
    }

    let report = match run_pipeline(Path::new(root)) {
        Ok(r) => r,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
    };

    let mp: MaterializePlan = plan(&report.blocks, filter);
    if mp.edits.is_empty() {
        eprintln!(
            "standardoc materialize: no virtual annotations to write at this confidence tier."
        );
        return ExitCode::SUCCESS;
    }

    let total_edits: usize = mp.edits.values().map(Vec::len).sum();
    let mode = if apply { "applying" } else { "dry-run" };
    eprintln!(
        "standardoc materialize ({mode}): {total_edits} block(s) across {} file(s)",
        mp.edits.len()
    );

    for (path, edits) in &mp.edits {
        eprintln!("\n  {}", path.display());
        // Print in ascending line order for readability (the plan stores
        // descending so writes don't shift later targets).
        let mut display = edits.clone();
        display.sort_by_key(|e| e.line_start);
        for edit in display {
            eprintln!(
                "    L{} ({} — confidence={:?})",
                edit.line_start, edit.key, edit.confidence
            );
            for line in &edit.comment_lines {
                eprintln!("        {line}");
            }
        }
    }

    if !apply {
        eprintln!("\nDry-run only. Re-run with --apply to write the changes.");
        return ExitCode::SUCCESS;
    }

    match apply_to_disk(&mp, &report.workspace_root) {
        Ok((files, edits)) => {
            eprintln!("\nMaterialized {edits} annotation(s) into {files} file(s).");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("\nmaterialize failed: {err}");
            ExitCode::from(1)
        }
    }
}

fn run_pipeline(root: &Path) -> Result<PipelineReport, String> {
    let (report, _config) = run_pipeline_with_config(root)?;
    Ok(report)
}

/// Variant exposing resolved config — useful when caller needs it later
/// (DSL schemas, validator rules) to stay consistent with scan settings.
fn run_pipeline_with_config(root: &Path) -> Result<(PipelineReport, Config), String> {
    let registry = build_registry(root);
    let config = Config::load_from_workspace_or_default(root);
    let report = scan_and_extract(root, &registry, &config)
        .map_err(|err| format!("cannot resolve path '{}': {err}", root.display()))?;
    Ok((report, config))
}

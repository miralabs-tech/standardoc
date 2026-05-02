#![allow(clippy::result_large_err)]

use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use clap::{ArgGroup, Args, Parser, Subcommand};
use standardoc_core::{IndexHandle, ScanFilters, cold_start, spawn_watcher};
use standardoc_ir::{RawEdge, RawSymbol, ResolvedOrUnresolved};
use standardoc_lang_provider::WorkspaceProvider;
use standardoc_server::ServerError;
use standardoc_server::query;

#[derive(Parser)]
#[command(
    name = "standardoc",
    version,
    about = "Standardoc — workspace-wide symbol graph"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run cold start once on the workspace, then exit.
    Index { path: PathBuf },

    /// Run cold start, then watch the workspace live until Ctrl+C.
    Watch { path: PathBuf },

    /// Look up a symbol or run a search query (read-only, no watcher).
    Query(QueryArgs),

    /// Drop the existing index and re-build from scratch.
    Rescan { path: PathBuf },

    /// Remove indexed paths that now match the workspace's `.stdignore`.
    PurgeExcluded {
        path: PathBuf,

        /// Skip the interactive confirmation prompt.
        #[arg(long)]
        yes: bool,
    },

    /// Run the LSP daemon over stdio (workspace `<path>` is the index root).
    Lsp {
        path: PathBuf,

        /// Accepted for vscode-languageclient compatibility; stdio is the only
        /// transport supported and is the default — this flag is ignored.
        #[arg(long, hide = true)]
        stdio: bool,
    },

    /// Run the MCP daemon over stdio (workspace `<path>` is the index root).
    Mcp {
        path: PathBuf,

        /// Open the index in read-only mode: do not acquire the workspace
        /// lock, do not run cold start, do not spawn the watcher. Polls for
        /// `.standardoc/index.db` for up to 60 s while a primary writer
        /// (LSP daemon, `standardoc watch`, ...) initializes the workspace.
        #[arg(long)]
        readonly: bool,
    },
}

#[derive(Args)]
#[command(group(
    ArgGroup::new("selector").required(true).multiple(false).args(["fqdn", "name", "file", "text"])
))]
struct QueryArgs {
    /// Workspace root.
    path: PathBuf,

    /// Fully-qualified name lookup. Combine with --edges-from / --edges-to to
    /// switch from symbol details to edge listings.
    #[arg(long)]
    fqdn: Option<String>,

    /// Match symbols whose `name` equals the given identifier.
    #[arg(long)]
    name: Option<String>,

    /// List symbols defined in a workspace-relative file path.
    #[arg(long)]
    file: Option<String>,

    /// Full-text search across symbol names + fqdns (FTS5).
    #[arg(long)]
    text: Option<String>,

    /// List outbound edges (callees, imports, ...) from --fqdn.
    #[arg(long, requires = "fqdn")]
    edges_from: bool,

    /// List inbound edges (callers, references, ...) into --fqdn.
    #[arg(long, requires = "fqdn", conflicts_with = "edges_from")]
    edges_to: bool,

    /// Cap the number of results for --name / --text.
    #[arg(short = 'l', long, default_value_t = 50)]
    limit: usize,
}

fn main() -> ExitCode {
    match main_inner() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn main_inner() -> Result<(), ServerError> {
    match Cli::parse().cmd {
        Command::Index { path } => cmd_index(&path),
        Command::Watch { path } => cmd_watch(&path),
        Command::Query(args) => cmd_query(&args),
        Command::Rescan { path } => cmd_rescan(&path),
        Command::PurgeExcluded { path, yes } => cmd_purge_excluded(&path, yes),
        Command::Lsp { path, stdio: _ } => cmd_lsp(&path),
        Command::Mcp { path, readonly } => cmd_mcp(&path, readonly),
    }
}

fn cmd_lsp(path: &Path) -> Result<(), ServerError> {
    let provider: Arc<dyn standardoc_core::LanguageProvider> = Arc::new(WorkspaceProvider::new());
    let handle = IndexHandle::open(path)?;
    let filters = Arc::new(RwLock::new(ScanFilters::load(handle.workspace_root())));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(ServerError::Io)?;
    runtime.block_on(standardoc_server::serve_lsp(handle, provider, filters))
}

fn cmd_mcp(path: &Path, readonly: bool) -> Result<(), ServerError> {
    let provider: Arc<dyn standardoc_core::LanguageProvider> = Arc::new(WorkspaceProvider::new());
    let handle = if readonly {
        wait_for_db_then_open_readonly(path, READONLY_DB_WAIT)?
    } else {
        IndexHandle::open(path)?
    };
    let filters = Arc::new(RwLock::new(ScanFilters::load(handle.workspace_root())));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(ServerError::Io)?;
    runtime.block_on(standardoc_server::serve_mcp(handle, provider, filters))
}

const READONLY_DB_WAIT: Duration = Duration::from_secs(60);
const READONLY_DB_POLL: Duration = Duration::from_millis(250);

/// Poll for `.standardoc/index.db` until it exists, then open the index in
/// read-only mode. Designed for an MCP daemon spawned in parallel with a
/// primary writer (LSP daemon) on a workspace that may not have been
/// indexed yet — the writer creates the DB during its own boot, and this
/// helper unblocks once the file appears on disk.
fn wait_for_db_then_open_readonly(
    path: &Path,
    timeout: Duration,
) -> Result<IndexHandle, ServerError> {
    let db_path = path.join(".standardoc").join("index.db");
    let deadline = Instant::now() + timeout;
    loop {
        if db_path.exists() {
            return IndexHandle::open_readonly(path).map_err(ServerError::from);
        }
        if Instant::now() >= deadline {
            return Err(ServerError::Io(io::Error::other(format!(
                "readonly: timed out after {:?} waiting for {} — \
                 start a primary writer (LSP daemon, `standardoc watch`, ...) on this workspace first",
                timeout,
                db_path.display()
            ))));
        }
        std::thread::sleep(READONLY_DB_POLL);
    }
}

fn cmd_index(path: &Path) -> Result<(), ServerError> {
    let provider = WorkspaceProvider::new();
    let handle = IndexHandle::open(path)?;
    let filters = ScanFilters::load(handle.workspace_root());
    let progress = ProgressReporter::start(handle.clone());
    let result = cold_start::run(&handle, &provider, &filters);
    progress.stop();
    result?;
    Ok(())
}

fn cmd_watch(path: &Path) -> Result<(), ServerError> {
    let provider: Arc<dyn standardoc_core::LanguageProvider> = Arc::new(WorkspaceProvider::new());
    let handle = IndexHandle::open(path)?;
    let filters = Arc::new(RwLock::new(ScanFilters::load(handle.workspace_root())));

    let progress = ProgressReporter::start(handle.clone());
    let cold_start_result = {
        let guard = filters
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cold_start::run(&handle, provider.as_ref(), &guard)
    };
    progress.stop();
    cold_start_result?;

    let _watcher = spawn_watcher(handle, provider, filters)?;
    eprintln!(
        "watching {} — press Ctrl+C to exit",
        handle_root_display(path)
    );
    wait_ctrl_c();
    eprintln!("shutting down");
    Ok(())
}

fn cmd_query(args: &QueryArgs) -> Result<(), ServerError> {
    let handle = IndexHandle::open(&args.path)?;
    if let Some(fqdn) = args.fqdn.as_deref() {
        return run_fqdn_query(&handle, fqdn, args);
    }
    if let Some(name) = args.name.as_deref() {
        let results = query::symbols_by_name(&handle, name, args.limit)?;
        print_symbol_list(&results);
        return Ok(());
    }
    if let Some(file) = args.file.as_deref() {
        let results = query::symbols_by_file(&handle, file)?;
        print_symbol_list(&results);
        return Ok(());
    }
    if let Some(text) = args.text.as_deref() {
        let results = query::search_text(&handle, text, args.limit)?;
        print_symbol_list(&results);
        return Ok(());
    }
    // clap's ArgGroup guarantees one selector is set.
    unreachable!("clap ArgGroup `selector` is `required(true)`")
}

fn run_fqdn_query(handle: &IndexHandle, fqdn: &str, args: &QueryArgs) -> Result<(), ServerError> {
    if args.edges_from {
        let edges = query::edges_from(handle, fqdn)?;
        print_edge_list(&edges);
        return Ok(());
    }
    if args.edges_to {
        let edges = query::edges_to(handle, fqdn)?;
        print_edge_list(&edges);
        return Ok(());
    }
    match query::symbol_by_fqdn(handle, fqdn)? {
        Some(symbol) => print_symbol_detail(&symbol),
        None => println!("no symbol found for fqdn `{fqdn}`"),
    }
    Ok(())
}

const PURGE_PREVIEW_LIMIT: usize = 20;

fn cmd_purge_excluded(path: &Path, yes_flag: bool) -> Result<(), ServerError> {
    let handle = IndexHandle::open(path)?;
    let filters = ScanFilters::load(handle.workspace_root());
    let candidates = handle.list_paths_matching_ignore(&filters)?;

    if candidates.is_empty() {
        println!("(nothing to purge)");
        return Ok(());
    }

    println!(
        "found {} indexed path(s) matching `.stdignore`:",
        candidates.len()
    );
    for path in candidates.iter().take(PURGE_PREVIEW_LIMIT) {
        println!("  {path}");
    }
    if candidates.len() > PURGE_PREVIEW_LIMIT {
        println!("  ... and {} more", candidates.len() - PURGE_PREVIEW_LIMIT);
    }

    if !confirm_purge(candidates.len(), yes_flag)? {
        println!("aborted");
        return Ok(());
    }

    handle.delete_paths(&candidates)?;
    println!("purged {} path(s)", candidates.len());
    Ok(())
}

fn confirm_purge(count: usize, yes_flag: bool) -> Result<bool, ServerError> {
    if yes_flag {
        return Ok(true);
    }
    if !io::stdin().is_terminal() {
        return Err(io::Error::other(format!(
            "non-interactive shell: pass --yes to purge {count} path(s) without prompting"
        ))
        .into());
    }
    eprint!("purge {count} path(s) from index? [y/N] ");
    let _ = io::stderr().flush();
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "Yes" | "YES"))
}

fn cmd_rescan(path: &Path) -> Result<(), ServerError> {
    let provider = WorkspaceProvider::new();
    let handle = IndexHandle::open(path)?;
    handle.rescan_from_scratch()?;
    let filters = ScanFilters::load(handle.workspace_root());
    let progress = ProgressReporter::start(handle.clone());
    let result = cold_start::run(&handle, &provider, &filters);
    progress.stop();
    result?;
    Ok(())
}

fn handle_root_display(path: &Path) -> String {
    path.canonicalize()
        .map_or_else(|_| path.display().to_string(), |p| p.display().to_string())
}

fn print_symbol_list(symbols: &[RawSymbol]) {
    if symbols.is_empty() {
        println!("(no matches)");
        return;
    }
    for s in symbols {
        println!(
            "{}  ({:?}, {:?})  {}:{}",
            s.fqdn, s.kind, s.visibility, s.location.file, s.location.start_line,
        );
    }
}

fn print_symbol_detail(s: &RawSymbol) {
    println!("{}", s.fqdn);
    println!("  kind:       {:?}", s.kind);
    println!("  visibility: {:?}", s.visibility);
    println!(
        "  location:   {}:{}..{} (cols {}..{})",
        s.location.file,
        s.location.start_line,
        s.location.end_line,
        s.location.start_col,
        s.location.end_col,
    );
    if let Some(module) = &s.module {
        println!("  module:     {module}");
    }
    println!("  language_kind: {}", s.language_kind);
    if let Some(hash) = &s.body_hash {
        println!("  body_hash:  {hash}");
    }
    if let Some(sig) = &s.signature {
        println!("  signature:  {sig:?}");
    }
}

fn print_edge_list(edges: &[RawEdge]) {
    if edges.is_empty() {
        println!("(no edges)");
        return;
    }
    for e in edges {
        let target = match &e.to {
            ResolvedOrUnresolved::Resolved { fqdn } => fqdn.clone(),
            ResolvedOrUnresolved::Unresolved { name } => format!("[unresolved] {name}"),
            ResolvedOrUnresolved::UnresolvedBridge { bridge, name } => {
                format!("[bridge {}] {name}", bridge.as_str())
            }
        };
        println!("{} --{:?}--> {}", e.from_fqdn, e.kind, target);
        for site in &e.sites {
            println!("  {}:{}:{}", site.file, site.line, site.col);
        }
    }
}

fn wait_ctrl_c() {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let install_result = ctrlc::set_handler(move || {
        let _ = tx.send(());
    });
    if let Err(e) = install_result {
        eprintln!("warning: failed to install Ctrl+C handler: {e}");
        return;
    }
    let _ = rx.recv();
}

struct ProgressReporter {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl ProgressReporter {
    fn start(index_handle: IndexHandle) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop);
        let is_tty = io::stderr().is_terminal();
        let handle = std::thread::Builder::new()
            .name("standardoc-progress".into())
            .spawn(move || progress_loop(&index_handle, &stop_clone, is_tty))
            .expect("spawn progress thread");
        Self {
            stop,
            handle: Some(handle),
        }
    }

    fn stop(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for ProgressReporter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn progress_loop(handle: &IndexHandle, stop: &AtomicBool, is_tty: bool) {
    let mut last_print = Instant::now()
        .checked_sub(Duration::from_secs(2))
        .unwrap_or_else(Instant::now);
    let mut printed_anything = false;
    while !stop.load(Ordering::Acquire) {
        if let Ok(Some((done, total))) = handle.cold_start_progress() {
            if is_tty {
                let _ = write!(io::stderr(), "\r[{done}/{total} files indexed]    ");
                let _ = io::stderr().flush();
                printed_anything = true;
            } else if last_print.elapsed() >= Duration::from_secs(1) {
                let _ = writeln!(io::stderr(), "[{done}/{total} files indexed]");
                last_print = Instant::now();
                printed_anything = true;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if is_tty && printed_anything {
        let _ = writeln!(io::stderr());
    }
}

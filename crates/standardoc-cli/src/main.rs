#![allow(clippy::result_large_err)]

mod warn;

use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use clap::{ArgGroup, Args, Parser, Subcommand};
use standardoc_core::{
    IndexHandle, RagPipeline, ScanFilters, SessionsHandle, StorageError, UsagePeriod, cold_start,
    spawn_watcher,
};
use standardoc_ir::{RawEdge, RawSymbol, ResolvedOrUnresolved};
use standardoc_lang_provider::WorkspaceProvider;
use standardoc_rag::embedder::{CandleBgeSmall, Embedder, MockEmbedder, resolve_models_dir};
use standardoc_rag::store::RagStore;
use standardoc_rag::types::EmbedModel;
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

        /// Enable the RAG (prose retrieval) layer alongside the LSP daemon.
        /// The LSP daemon is the primary write-side in the ext VSCode setup,
        /// so this is where the RAG cold-start + watcher belong. Same sidecar
        /// `.standardoc/rag.db` as the MCP `--rag` path.
        #[arg(long)]
        rag: bool,

        /// Embedder backend used when `--rag` is set. Same semantics as the
        /// MCP sub-command (`mock` = deterministic stub, `candle` = BGE-small
        /// downloaded to `~/.cache/standardoc/models/` on first run).
        #[arg(long, default_value = "mock", value_parser = ["mock", "candle"])]
        embedder: String,
    },

    /// Run the MCP daemon. Default transport: stdio. Use `--http` to serve
    /// over HTTP/SSE (singleton shared by multiple chat clients).
    Mcp {
        path: PathBuf,

        /// Open the index in secondary mode: do not acquire the workspace
        /// fs4 lock, do not run cold start, do not spawn the watcher.
        /// Polls for `.standardoc/index.db` for up to 60 s while a primary
        /// writer (LSP daemon, `standardoc watch`, ...) initialises the
        /// workspace. The handle is still R/W under SQLite WAL (v6+),
        /// so `resolve_external` etc. can write external symbols.
        #[arg(long)]
        readonly: bool,

        /// Serve MCP over HTTP/SSE at `127.0.0.1:<port>` instead of stdio.
        /// Pass `0` to let the kernel pick a random ephemeral port. The
        /// resolved endpoint URL is written to
        /// `<path>/.standardoc/mcp.endpoint` for client discovery.
        ///
        /// HTTP mode is the recommended transport for the VSCode
        /// extension and any other long-running client: one daemon per
        /// workspace serves every chat session, eliminating the
        /// per-chat stdio child-spawn cost of the default transport.
        #[arg(long)]
        http: Option<u16>,

        /// Enable the RAG (prose retrieval) layer. Boots a sidecar
        /// `.standardoc/rag.db`, exposes the `fetch_chunks` MCP tool,
        /// and populates `chunk_refs` in `get_context` responses. Off
        /// by default — opt-in keeps the embedding model download
        /// (~130 MB on first run with `--embedder candle`) under user
        /// control.
        #[arg(long)]
        rag: bool,

        /// Embedder backend used when `--rag` is set. `mock` ships a
        /// deterministic zero-network BLAKE3-derived stub (tests,
        /// development, exercising the tool surface without DLing a
        /// model). `candle` loads BGE-small-en-v1.5 from
        /// `~/.cache/standardoc/models/`, downloading it on first run.
        #[arg(long, default_value = "mock", value_parser = ["mock", "candle"])]
        embedder: String,
    },

    /// Print the on-disk schema version of the workspace index, the schema
    /// version this binary supports, and a compatibility flag. Output is JSON
    /// on stdout; exit code is always 0 when the command itself runs. The ext
    /// VSCode uses this as a pre-flight check before spawning the LSP/MCP
    /// daemons.
    SchemaVersion { path: PathBuf },

    /// Wipe rows from the workspace's `usage_stats` telemetry table. Used to
    /// baseline the token-savings counter before a measurement run. Does NOT
    /// touch the index — the daemon may stay up.
    ResetUsage {
        path: PathBuf,

        /// Window to wipe. `today` (last 24 h), `week` (last 7 d), or `all`.
        #[arg(long, value_parser = ["today", "day", "week", "all"])]
        period: String,

        /// Skip the interactive confirmation prompt.
        #[arg(long)]
        yes: bool,
    },

    /// Preview which workspace-relative paths a single `.stdignore`
    /// pattern would match. Output is JSON on stdout
    /// (`{pattern, matches, total_count, truncated, walk_truncated}`).
    /// Backs the VSCode extension's `.stdignore` hover provider — the
    /// extension shells out to this sub-command so the preview uses
    /// the exact same `ignore` crate matcher as the daemon.
    StdignorePreview {
        /// Workspace root to walk.
        path: PathBuf,

        /// Gitignore-syntax pattern to test. Single-line ; comments
        /// (`#`) and blank patterns are accepted and return zero
        /// matches.
        #[arg(long)]
        pattern: String,

        /// Maximum number of matches to include in the response.
        /// `total_count` keeps counting beyond this cap.
        #[arg(long, default_value_t = 20)]
        limit: usize,
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
            if let Some(marker) = fatal_marker_for(&e) {
                eprintln!("{marker}");
            }
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Returns a machine-readable marker line for fatal-config errors that
/// the VSCode extension supervisor (and other daemon supervisors) need
/// to recognise WITHOUT regex-parsing the human-readable error message.
///
/// The shape is stable and parsed by the VSCode extension supervisor at
/// `ext/vscode/src/daemon/supervisor.ts`:
///
/// ```text
/// STDOC_FATAL: <code> <key>=<value> ...
/// ```
///
/// Currently emitted codes:
///
/// - `schema_too_new db=<n> supported=<n>` — the on-disk schema is newer
///   than this binary supports. Fix path: rebuild and re-install the
///   binary (`cargo install --path crates/standardoc-cli --force` for a
///   local source build, or rebuild the bundled VSCode extension). This
///   is the H1 footgun documented in the daemon UX track — a binary
///   pinned to schema vN cannot read a DB migrated to vN+1 by a newer
///   process (cargo test, cargo build of an updated workspace, ...).
///
/// Returns `None` when the error has no structured marker — main() then
/// falls back to the friendly `error: …` line only.
///
/// Adding a new code requires a matching parser update on the supervisor
/// side. Keep the supervisor's `parseFatalMarker` in sync.
fn fatal_marker_for(err: &ServerError) -> Option<String> {
    match err {
        ServerError::Storage(StorageError::SchemaVersionTooNew { db, supported }) => Some(format!(
            "STDOC_FATAL: schema_too_new db={db} supported={supported}"
        )),
        _ => None,
    }
}

fn main_inner() -> Result<(), ServerError> {
    match Cli::parse().cmd {
        Command::Index { path } => cmd_index(&path),
        Command::Watch { path } => cmd_watch(&path),
        Command::Query(args) => cmd_query(&args),
        Command::Rescan { path } => cmd_rescan(&path),
        Command::PurgeExcluded { path, yes } => cmd_purge_excluded(&path, yes),
        Command::Lsp {
            path,
            stdio: _,
            rag,
            embedder,
        } => cmd_lsp(&path, rag, &embedder),
        Command::Mcp {
            path,
            readonly,
            http,
            rag,
            embedder,
        } => cmd_mcp(&path, readonly, http, rag, &embedder),
        Command::SchemaVersion { path } => cmd_schema_version(&path),
        Command::ResetUsage { path, period, yes } => cmd_reset_usage(&path, &period, yes),
        Command::StdignorePreview {
            path,
            pattern,
            limit,
        } => cmd_stdignore_preview(&path, &pattern, limit),
    }
}

fn cmd_schema_version(path: &Path) -> Result<(), ServerError> {
    let supported = standardoc_core::SUPPORTED_SCHEMA_VERSION;
    let db_path = path.join(".standardoc").join("index.db");
    let db_version: Option<u32> = if db_path.exists() {
        let handle = IndexHandle::open_readonly(path)?;
        Some(query::schema_version(&handle)?)
    } else {
        None
    };
    let compatible = db_version.is_none_or(|db| db <= supported);
    let payload = serde_json::json!({
        "db": db_version,
        "supported": supported,
        "compatible": compatible,
    });
    println!("{payload}");
    Ok(())
}

fn cmd_lsp(path: &Path, rag: bool, embedder: &str) -> Result<(), ServerError> {
    let provider: Arc<dyn standardoc_core::LanguageProvider> = Arc::new(WorkspaceProvider::new());
    let handle = IndexHandle::open(path)?;
    let _ = warn::boot_binary_sweep(handle.workspace_root());
    let _ = warn::boot_lockfile_invalidation_sweep(&handle, handle.workspace_root());
    let filters = Arc::new(RwLock::new(ScanFilters::load(handle.workspace_root())));

    let rag_pipeline = if rag {
        Some(build_rag_pipeline(handle.workspace_root(), embedder)?)
    } else {
        None
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(ServerError::Io)?;
    runtime.block_on(standardoc_server::serve_lsp(
        handle,
        provider,
        filters,
        rag_pipeline,
    ))
}

fn cmd_mcp(
    path: &Path,
    readonly: bool,
    http: Option<u16>,
    rag: bool,
    embedder: &str,
) -> Result<(), ServerError> {
    let provider: Arc<dyn standardoc_core::LanguageProvider> = Arc::new(WorkspaceProvider::new());
    let handle = if readonly {
        wait_for_db_then_open_readonly(path, READONLY_DB_WAIT)?
    } else {
        IndexHandle::open(path)?
    };
    if !readonly {
        let _ = warn::boot_binary_sweep(handle.workspace_root());
        let _ = warn::boot_lockfile_invalidation_sweep(&handle, handle.workspace_root());
    }
    let filters = Arc::new(RwLock::new(ScanFilters::load(handle.workspace_root())));

    let rag_pipeline = if rag {
        Some(build_rag_pipeline(handle.workspace_root(), embedder)?)
    } else {
        None
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(ServerError::Io)?;

    match http {
        Some(port) => {
            let bind_addr = format!("127.0.0.1:{port}");
            runtime.block_on(standardoc_server::serve_mcp_http(
                handle,
                provider,
                filters,
                &bind_addr,
                rag_pipeline,
            ))
        }
        None => runtime.block_on(standardoc_server::serve_mcp(
            handle,
            provider,
            filters,
            rag_pipeline,
        )),
    }
}

fn build_rag_pipeline(
    workspace_root: &Path,
    embedder_choice: &str,
) -> Result<Arc<RagPipeline>, ServerError> {
    let model = EmbedModel::bge_small_en_v1_5();
    let store = RagStore::open(workspace_root, model)
        .map_err(|e| ServerError::Io(io::Error::other(format!("rag store open: {e}"))))?;
    let embedder: Arc<dyn Embedder> = match embedder_choice {
        "mock" => Arc::new(MockEmbedder::new()),
        "candle" => Arc::new(load_or_download_candle()?),
        other => {
            return Err(ServerError::Io(io::Error::other(format!(
                "unknown --embedder choice: {other}"
            ))));
        }
    };
    Ok(Arc::new(RagPipeline::with_defaults(
        Arc::new(store),
        embedder,
    )))
}

fn load_or_download_candle() -> Result<CandleBgeSmall, ServerError> {
    let model_dir = resolve_models_dir().join("bge-small-en-v1.5");
    if !CandleBgeSmall::is_present(&model_dir) {
        // Structured stderr markers so the ext supervisor knows to suspend
        // its endpoint-file timeout while the model download is in flight
        // (130 MB on a fresh install can take a couple of seconds on a
        // fast pipe, much longer on residential ADSL — bracketing the
        // network phase tells the supervisor to wait it out instead of
        // killing the daemon mid-fetch).
        eprintln!(
            "STDOC_RAG_DL_START: {{\"model\":\"bge-small-en-v1.5\",\"approx_bytes\":130000000,\"target\":\"{}\"}}",
            escape_for_json(&model_dir.display().to_string()),
        );
        eprintln!(
            "standardoc rag: BGE-small not found at {} — downloading (~130 MB)...",
            model_dir.display()
        );
        let dl_result = CandleBgeSmall::download(&model_dir);
        eprintln!("STDOC_RAG_DL_DONE");
        dl_result
            .map_err(|e| ServerError::Io(io::Error::other(format!("rag model download: {e}"))))?;
        eprintln!("standardoc rag: model downloaded");
    }
    CandleBgeSmall::load(model_dir)
        .map_err(|e| ServerError::Io(io::Error::other(format!("rag model load: {e}"))))
}

/// Minimal JSON string escaping for the marker payload above. Only
/// quote + backslash are escaped — the model dir is filesystem-controlled
/// and otherwise printable.
fn escape_for_json(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

const READONLY_DB_WAIT: Duration = Duration::from_mins(1);
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
    let _ = warn::boot_binary_sweep(handle.workspace_root());
    let _ = warn::boot_lockfile_invalidation_sweep(&handle, handle.workspace_root());
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
    let _ = warn::boot_binary_sweep(handle.workspace_root());
    let _ = warn::boot_lockfile_invalidation_sweep(&handle, handle.workspace_root());
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
        let results =
            query::search_text(&handle, text, args.limit, &query::SymbolFilter::default())?;
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

fn cmd_stdignore_preview(
    workspace: &Path,
    pattern: &str,
    limit: usize,
) -> Result<(), ServerError> {
    let preview = standardoc_core::preview_pattern_matches(workspace, pattern, limit)
        .map_err(|e| io::Error::other(format!("stdignore preview: {e}")))?;
    let json = serde_json::to_string(&preview)
        .map_err(|e| io::Error::other(format!("serialize preview: {e}")))?;
    println!("{json}");
    Ok(())
}

fn cmd_reset_usage(path: &Path, period: &str, yes_flag: bool) -> Result<(), ServerError> {
    let parsed = UsagePeriod::from_str_loose(period).ok_or_else(|| {
        io::Error::other(format!(
            "invalid --period {period:?}: expected `today`, `week`, or `all`"
        ))
    })?;
    let handle = SessionsHandle::open(path)
        .map_err(|e| io::Error::other(format!("open sessions.db: {e}")))?;
    let preview = handle
        .query_usage_stats(parsed)
        .map_err(|e| io::Error::other(format!("count rows: {e}")))?;
    if preview.calls == 0 {
        println!("(nothing to reset for period `{period}`)");
        return Ok(());
    }
    if !confirm_destructive(
        &format!("reset {} usage_stats row(s) for period `{period}`?", preview.calls),
        yes_flag,
    )? {
        println!("aborted");
        return Ok(());
    }
    let deleted = handle
        .reset_usage(parsed)
        .map_err(|e| io::Error::other(format!("reset rows: {e}")))?;
    println!("reset {deleted} row(s) from period `{period}`");
    Ok(())
}

fn confirm_destructive(prompt: &str, yes_flag: bool) -> Result<bool, ServerError> {
    if yes_flag {
        return Ok(true);
    }
    if !io::stdin().is_terminal() {
        return Err(io::Error::other(format!(
            "non-interactive shell: pass --yes to confirm — {prompt}"
        ))
        .into());
    }
    eprint!("{prompt} [y/N] ");
    let _ = io::stderr().flush();
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "Yes" | "YES"))
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

#[cfg(test)]
mod fatal_marker_tests {
    use super::*;

    #[test]
    fn schema_too_new_emits_machine_readable_marker() {
        let err = ServerError::Storage(StorageError::SchemaVersionTooNew {
            db: 99,
            supported: 2,
        });
        assert_eq!(
            fatal_marker_for(&err).as_deref(),
            Some("STDOC_FATAL: schema_too_new db=99 supported=2"),
        );
    }

    #[test]
    fn unrelated_storage_error_returns_no_marker() {
        let err = ServerError::Storage(StorageError::ReadOnlyMissingDatabase {
            path: PathBuf::from("/tmp/nope"),
        });
        assert!(fatal_marker_for(&err).is_none());
    }

    #[test]
    fn io_error_returns_no_marker() {
        let err = ServerError::Io(io::Error::other("disk full"));
        assert!(fatal_marker_for(&err).is_none());
    }

    #[test]
    fn marker_format_starts_with_stable_prefix() {
        // The supervisor parses the line by splitting on the literal
        // `STDOC_FATAL: ` prefix — keep this contract symmetric.
        let err = ServerError::Storage(StorageError::SchemaVersionTooNew {
            db: 42,
            supported: 1,
        });
        let marker = fatal_marker_for(&err).unwrap();
        assert!(marker.starts_with("STDOC_FATAL: "));
        assert!(marker.contains("db=42"));
        assert!(marker.contains("supported=1"));
    }
}

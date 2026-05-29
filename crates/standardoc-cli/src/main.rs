#![allow(clippy::result_large_err)]

mod self_update;
mod sxd_schema;
mod warn;

use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use clap::{ArgGroup, Args, Parser, Subcommand};
use standardoc_core::{IndexHandle, ScanFilters, StorageError, cold_start, spawn_watcher};
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

    /// Remove indexed paths that now match the workspace's `standardoc.sxd` ignore block.
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

    /// Run the `.sxd` LSP backend over stdio. Drives `standarx-dsl-lsp`
    /// with our [`SxdSchema`] plugged in so editors get both syntactic
    /// (from the DSL parser) and schema-aware (from standardoc.sxd's
    /// `ignore` / `project` / `group` rules) diagnostics live as the
    /// user edits. Stateless and workspace-agnostic — every open .sxd
    /// document validates independently.
    LspSxd {
        /// Accepted for vscode-languageclient compatibility; stdio is the only
        /// transport supported and is the default — this flag is ignored.
        #[arg(long, hide = true)]
        stdio: bool,
    },

    /// Long-lived HTTP proxy in front of one or more daemons. Keeps
    /// every MCP client (Claude Code, Copilot Chat, …) connected to a
    /// stable URL across daemon restarts / rebuilds / migrations, and
    /// can serve multiple workspaces from a single bind address:
    ///
    /// ```text
    /// /mcp                  → default (first --workspace)  (back-compat)
    /// /ws/<id>/mcp          → that workspace's daemon
    /// /health               → JSON status of every registered workspace
    /// ```
    ///
    /// `<id>` is a deterministic blake3 short hash of the workspace's
    /// canonical abs path — print it via `standardoc workspace-id <dir>`.
    ///
    /// Bind precedence: `--bind` flag > `127.0.0.1:7700`. The VSCode
    /// extension passes `--bind` from the per-user settings
    /// `standardoc.proxyBind` / `standardoc.proxyPort` ; workspace
    /// `standardoc.sxd` is NOT read here (proxy is a per-machine
    /// singleton, not a per-workspace resource — see Bug F).
    Proxy {
        /// Explicit `host:port` bind override. When omitted, defaults
        /// to `127.0.0.1:7700`.
        #[arg(long)]
        bind: Option<String>,

        /// Workspace root to serve. Repeat the flag to register
        /// multiple workspaces under one proxy instance — the first
        /// entry becomes the default (`/mcp` path). Defaults to the
        /// current working directory when omitted.
        #[arg(long = "workspace", value_name = "DIR")]
        workspaces: Vec<PathBuf>,

        /// How long (in seconds) to keep retrying a request when the
        /// upstream daemon is unreachable before returning `503`.
        #[arg(long, default_value_t = 30)]
        retry_window_secs: u64,
    },

    /// Print the deterministic workspace id used by the proxy's
    /// `/ws/<id>/mcp` routing. blake3-derived from the canonical abs
    /// path of `<dir>`, truncated to 8 hex chars. Exposed so external
    /// scripts / chat clients can build the URL without round-tripping
    /// through the proxy.
    WorkspaceId {
        #[arg(value_name = "DIR")]
        path: PathBuf,
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
    },

    /// Print the on-disk schema version of the workspace index, the schema
    /// version this binary supports, and a compatibility flag. Output is JSON
    /// on stdout; exit code is always 0 when the command itself runs. The ext
    /// VSCode uses this as a pre-flight check before spawning the LSP/MCP
    /// daemons.
    SchemaVersion { path: PathBuf },

    /// Preview which workspace-relative paths a single gitignore-syntax
    /// pattern would match. Output is JSON on stdout
    /// (`{pattern, matches, total_count, truncated, walk_truncated}`).
    /// Backs the VSCode extension's `standardoc.sxd` hover provider —
    /// the extension shells out to this sub-command so the preview uses
    /// the exact same `ignore` crate matcher as the daemon.
    ///
    /// `stdignore-preview` (without the `sxd-` prefix) is kept as an
    /// alias for back-compat with older callers.
    #[command(alias = "stdignore-preview")]
    SxdPreview {
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

    /// Claude Code hook drivers. Each sub-command reads a JSON payload from
    /// stdin (the hook event payload) and writes a JSON response on stdout
    /// (the hook decision). All sub-commands exit 0 and never abort on
    /// malformed or missing input — the only deliberate denial path is the
    /// `pre-tool-hook --mode check` MCP-first policy.
    Claude {
        #[command(subcommand)]
        action: ClaudeAction,
    },

    /// Atomically upgrade the running `standardoc` binary to the latest
    /// published release. Reads the same `version.json` manifest the
    /// VSCode extension consumes ; downloads + sha256-verifies the
    /// archive for the current platform ; extracts via the system
    /// `tar` tool ; swaps the binary in place (Windows: rename
    /// current to `<name>.exe.old`, copy new in ; Unix: atomic
    /// `rename`).
    ///
    /// No-op when the binary is already on the latest version unless
    /// `--force` is passed. `--dry-run` resolves and prints what
    /// would happen without touching the filesystem.
    SelfUpdate {
        /// Resolve the latest version + asset URL and stop. Does not
        /// download or swap the binary.
        #[arg(long)]
        dry_run: bool,

        /// Re-install the latest version even when the binary already
        /// reports it. Useful for recovering from a corrupted install
        /// or after a force-pushed release tag.
        #[arg(long)]
        force: bool,

        /// Pin the update to a specific release tag (e.g. `v1.0.0-beta.3`)
        /// instead of following `releases/latest`. Mainly for
        /// rollbacks ; CI normally omits this.
        #[arg(long, value_name = "TAG")]
        version: Option<String>,
    },
}

#[derive(Subcommand)]
enum ClaudeAction {
    /// PreToolUse / SessionStart driver enforcing the MCP-first discipline.
    ///
    /// * `--mode mark`  — Touch the sentinel
    ///   `<cwd>/.standardoc/mcp_called_this_session` when the inbound tool
    ///   is a standardoc MCP call (the agent has paid the MCP-first toll
    ///   for this session). Wired on PreToolUse with matcher
    ///   `mcp__standardoc__.*`.
    /// * `--mode check` — When the sentinel for the current cwd is absent,
    ///   emit a `deny` PreToolUse permissionDecision JSON; otherwise emit
    ///   `{}` (allow). Wired on PreToolUse with matcher
    ///   `Bash|Read|Grep|Glob` so code exploration is gated behind MCP.
    /// * `--mode reset` — Remove the sentinel. Wired on SessionStart so
    ///   each new chat starts MCP-first-strict regardless of the previous
    ///   chat's history.
    PreToolHook {
        #[arg(long, value_parser = ["mark", "check", "reset"])]
        mode: String,
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
        Command::Lsp { path, stdio: _ } => cmd_lsp(&path),
        Command::LspSxd { stdio: _ } => cmd_lsp_sxd(),
        Command::Proxy {
            bind,
            workspaces,
            retry_window_secs,
        } => cmd_proxy(bind, workspaces, retry_window_secs),
        Command::WorkspaceId { path } => cmd_workspace_id(&path),
        Command::Mcp {
            path,
            readonly,
            http,
        } => cmd_mcp(&path, readonly, http),
        Command::SchemaVersion { path } => cmd_schema_version(&path),
        Command::SxdPreview {
            path,
            pattern,
            limit,
        } => cmd_sxd_preview(&path, &pattern, limit),
        Command::Claude { action } => match action {
            ClaudeAction::PreToolHook { mode } => cmd_claude_pre_tool_hook(&mode),
        },
        Command::SelfUpdate {
            dry_run,
            force,
            version,
        } => cmd_self_update(dry_run, force, version),
    }
}

fn cmd_self_update(dry_run: bool, force: bool, version: Option<String>) -> Result<(), ServerError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(ServerError::Io)?;
    runtime
        .block_on(self_update::run(dry_run, force, version))
        .map_err(|e| ServerError::Io(io::Error::other(format!("self-update: {e}"))))?;
    Ok(())
}

/// File name (under `<cwd>/.standardoc/`) used to record that the agent has
/// called at least one standardoc MCP tool in the current chat. The file is
/// 0-byte; only its presence/absence matters.
const MCP_FIRST_SENTINEL: &str = "mcp_called_this_session";

// Hook semantics: never abort on malformed input — the deny path is a
// JSON decision on stdout, not a process error. The `Result` shape is
// kept for symmetry with sibling `cmd_*` dispatch arms.
#[allow(clippy::unnecessary_wraps)]
fn cmd_claude_pre_tool_hook(mode: &str) -> Result<(), ServerError> {
    use std::io::Read;
    let mut raw = String::new();
    io::stdin().read_to_string(&mut raw).ok();
    let sentinel = resolve_mcp_first_sentinel(&raw);
    let output = pre_tool_hook_decide(mode, &raw, &sentinel);
    println!("{output}");
    Ok(())
}

fn resolve_mcp_first_sentinel(raw_payload: &str) -> PathBuf {
    let cwd: PathBuf = serde_json::from_str::<serde_json::Value>(raw_payload)
        .ok()
        .and_then(|v| v.get("cwd").and_then(|c| c.as_str()).map(PathBuf::from))
        .or_else(|| std::env::var("CLAUDE_PROJECT_DIR").ok().map(PathBuf::from))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    cwd.join(".standardoc").join(MCP_FIRST_SENTINEL)
}

/// Pure-input decision function for the PreToolHook driver. Extracted from
/// [`cmd_claude_pre_tool_hook`] so unit tests can pass a deterministic
/// sentinel path (a tempdir) and a synthetic stdin payload without
/// touching the real filesystem under `<cwd>/.standardoc/`.
fn pre_tool_hook_decide(mode: &str, raw_payload: &str, sentinel: &Path) -> String {
    let payload =
        serde_json::from_str::<serde_json::Value>(raw_payload).unwrap_or(serde_json::Value::Null);
    let tool = payload
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    match mode {
        "mark" => {
            if tool.starts_with("mcp__standardoc__") {
                if let Some(parent) = sentinel.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(sentinel, b"");
                r#"{"marked":true}"#.to_string()
            } else {
                r#"{"marked":false,"reason":"not_standardoc_mcp_tool"}"#.to_string()
            }
        }
        "check" => {
            if sentinel.exists() {
                "{}".to_string()
            } else {
                serde_json::json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "deny",
                        "permissionDecisionReason":
                            "MCP-first: call a standardoc MCP tool (find_symbol / get_context / list_symbols / find_symbols_by_pattern / get_body / current_revision / check_stale) before Bash/Read/Grep/Glob. The Standardoc index is structural and faster for code exploration.",
                        "additionalContext":
                            "Standardoc MCP tools: find_symbol, get_context, list_symbols, find_symbols_by_pattern, find_similar_symbols, get_body, current_revision, check_stale"
                    }
                })
                .to_string()
            }
        }
        "reset" => {
            let _ = std::fs::remove_file(sentinel);
            r#"{"reset":true}"#.to_string()
        }
        _ => r#"{"ok":false,"reason":"unknown_mode"}"#.to_string(),
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

fn cmd_lsp(path: &Path) -> Result<(), ServerError> {
    let provider: Arc<dyn standardoc_core::LanguageProvider> = Arc::new(WorkspaceProvider::new());
    let handle = IndexHandle::open(path)?;
    let _ = warn::boot_binary_sweep(handle.workspace_root());
    let _ = warn::boot_lockfile_invalidation_sweep(&handle, handle.workspace_root());
    let filters = Arc::new(RwLock::new(ScanFilters::load(handle.workspace_root())));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(ServerError::Io)?;
    runtime.block_on(standardoc_server::serve_lsp(handle, provider, filters))
}

fn cmd_lsp_sxd() -> Result<(), ServerError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(ServerError::Io)?;
    runtime.block_on(async {
        standarx_dsl_lsp::run_stdio_with_schemas(vec![Box::new(sxd_schema::SxdSchema)]).await;
    });
    Ok(())
}

fn cmd_proxy(
    bind: Option<String>,
    workspaces: Vec<PathBuf>,
    retry_window_secs: u64,
) -> Result<(), ServerError> {
    let workspaces = if workspaces.is_empty() {
        vec![std::env::current_dir().map_err(ServerError::Io)?]
    } else {
        workspaces
    };
    let bind_addr = resolve_proxy_bind(bind);
    let cfg = standardoc_mcp_proxy::ProxyConfig {
        bind_addr,
        workspaces,
        upstream_retry_window: Duration::from_secs(retry_window_secs),
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(ServerError::Io)?;
    // ProxyError is a distinct error type from ServerError — we surface
    // it as an Io error wrapping the formatted message so the existing
    // CLI dispatch stays uniform without adding a ServerError variant
    // just for this one subcommand.
    runtime
        .block_on(standardoc_mcp_proxy::run(cfg))
        .map_err(|e| ServerError::Io(io::Error::other(format!("proxy: {e}"))))
}

#[allow(clippy::unnecessary_wraps)]
fn cmd_workspace_id(path: &Path) -> Result<(), ServerError> {
    println!("{}", standardoc_mcp_proxy::workspace_id_for(path));
    Ok(())
}

/// Resolve the proxy bind address: CLI `--bind` if passed, else
/// `127.0.0.1:7700`. The proxy is a per-machine singleton ; sibling
/// VSCode windows must converge on the same address for the
/// `probe → register → exit` dedup to work, so per-workspace
/// configuration is intentionally absent. The VSCode extension reads
/// `standardoc.proxyBind` / `standardoc.proxyPort` and passes them
/// here via `--bind`.
fn resolve_proxy_bind(cli_bind: Option<String>) -> String {
    cli_bind.unwrap_or_else(|| "127.0.0.1:7700".to_string())
}

fn cmd_mcp(path: &Path, readonly: bool, http: Option<u16>) -> Result<(), ServerError> {
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

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(ServerError::Io)?;

    match http {
        Some(port) => {
            let bind_addr = format!("127.0.0.1:{port}");
            runtime.block_on(standardoc_server::serve_mcp_http(
                handle, provider, filters, &bind_addr,
            ))
        }
        None => runtime.block_on(standardoc_server::serve_mcp(handle, provider, filters)),
    }
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
        "found {} indexed path(s) matching `standardoc.sxd` ignore block:",
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

fn cmd_sxd_preview(workspace: &Path, pattern: &str, limit: usize) -> Result<(), ServerError> {
    let preview = standardoc_core::preview_pattern_matches(workspace, pattern, limit)
        .map_err(|e| io::Error::other(format!("sxd preview: {e}")))?;
    let json = serde_json::to_string(&preview)
        .map_err(|e| io::Error::other(format!("serialize preview: {e}")))?;
    println!("{json}");
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

#[cfg(test)]
mod fatal_marker_tests;

#[cfg(test)]
mod claude_pre_tool_hook_tests;

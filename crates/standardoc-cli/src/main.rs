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

    /// Bridge the workspace sessions DB <-> a directory of `.md` memo
    /// files. `sync-in` imports every memo under `dir` into the sessions
    /// DB; the extended frontmatter (status, supersedes, created_at) makes
    /// the import fidelity-complete. `sync-out` is the inverse: dump every
    /// row to `<slug>.md` + regenerate `MEMORY.md`. `hook` is the
    /// claude-code PostToolUse driver that auto-runs `sync-in` whenever a
    /// `.md` under the harness memory directory is written.
    Session {
        #[command(subcommand)]
        action: SessionAction,
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
}

#[derive(Subcommand)]
enum SessionAction {
    /// Bulk import every `.md` memo under `dir` into the workspace sessions
    /// DB. Frontmatter `type:` drives the `SessionKind` (feedback, user →
    /// profile, project → lock, reference → profile, other → session);
    /// `status`, `supersedes`, `created_at` round-trip fidelity-complete.
    /// `MEMORY.md` is skipped. UPSERT by slug — safe to re-run.
    SyncIn {
        /// Workspace root (anchors `.standardoc-sessions/sessions.db`).
        workspace: PathBuf,
        /// Source directory of `.md` memo files (e.g. claude-code's
        /// `~/.claude/projects/<hash>/memory`, or a `sessions-export/`
        /// dropped from another workspace).
        dir: PathBuf,
    },
    /// Dump every session row in the workspace DB to `<slug>.md` under
    /// `dir` and (re)write `MEMORY.md` as an index. Inverse of
    /// `sync-in`. Used for cross-machine portability.
    SyncOut { workspace: PathBuf, dir: PathBuf },
    /// Read a Claude Code PostToolUse hook payload from stdin, detect if a
    /// `Write`/`Edit` touched a file under the harness memory directory,
    /// and trigger `sync-in` automatically. Exits with `{"synced": false}`
    /// on no-op so the hook never blocks the agent. Designed to be wired
    /// once at workspace init.
    Hook,
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
        Command::Session { action } => match action {
            SessionAction::SyncIn { workspace, dir } => cmd_session_sync_in(&workspace, &dir),
            SessionAction::SyncOut { workspace, dir } => cmd_session_sync_out(&workspace, &dir),
            SessionAction::Hook => cmd_session_hook(),
        },
        Command::Claude { action } => match action {
            ClaudeAction::PreToolHook { mode } => cmd_claude_pre_tool_hook(&mode),
        },
    }
}

fn cmd_session_sync_in(workspace: &Path, dir: &Path) -> Result<(), ServerError> {
    let handle = SessionsHandle::open(workspace)?;
    let report = standardoc_core::sessions::memory_sync::import_memory_dir(&handle, dir)?;
    println!(
        "{}",
        serde_json::to_string(&report).unwrap_or_else(|_| "{}".to_string())
    );
    Ok(())
}

fn cmd_session_sync_out(workspace: &Path, dir: &Path) -> Result<(), ServerError> {
    let handle = SessionsHandle::open(workspace)?;
    let report = standardoc_core::sessions::memory_sync::export_memory_dir(&handle, dir)?;
    println!(
        "{}",
        serde_json::to_string(&report).unwrap_or_else(|_| "{}".to_string())
    );
    Ok(())
}

/// Marker substring on the file_path that identifies a Claude harness memory
/// write. Matches the Linux `~/.claude/projects/<hash>/memory/` and the
/// Windows `C:\Users\<u>\.claude\projects\<hash>\memory\` layouts after the
/// path is normalised with forward slashes.
const MEMORY_PATH_MARKER: &str = "/.claude/projects/";
const MEMORY_PATH_TAIL: &str = "/memory/";

fn cmd_session_hook() -> Result<(), ServerError> {
    use std::io::Read;
    let mut raw = String::new();
    io::stdin().read_to_string(&mut raw).ok();
    let payload: serde_json::Value = if let Ok(v) = serde_json::from_str(&raw) {
        v
    } else {
        // Malformed payload — never block the agent.
        println!("{{\"synced\":false,\"reason\":\"invalid_json\"}}");
        return Ok(());
    };
    let tool = payload
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let file_path_raw = payload
        .get("tool_input")
        .and_then(|v| v.get("file_path"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let cwd_raw = payload.get("cwd").and_then(|v| v.as_str()).unwrap_or("");
    let file_path = file_path_raw.replace('\\', "/");
    let cwd = cwd_raw.replace('\\', "/");
    let touched_memory = matches!(tool, "Write" | "Edit" | "MultiEdit")
        && file_path.contains(MEMORY_PATH_MARKER)
        && file_path.contains(MEMORY_PATH_TAIL);
    if !touched_memory {
        println!("{{\"synced\":false,\"reason\":\"not_a_memory_write\"}}");
        return Ok(());
    }
    let Some(memory_dir) = memory_dir_from_path(&file_path) else {
        println!("{{\"synced\":false,\"reason\":\"unparsable_memory_path\"}}");
        return Ok(());
    };
    if cwd.is_empty() {
        println!("{{\"synced\":false,\"reason\":\"missing_cwd\"}}");
        return Ok(());
    }
    let workspace = PathBuf::from(cwd);
    let memory_dir = PathBuf::from(memory_dir);
    let handle = SessionsHandle::open(&workspace)?;
    let report = standardoc_core::sessions::memory_sync::import_memory_dir(&handle, &memory_dir)?;
    println!(
        "{}",
        serde_json::json!({ "synced": true, "report": report })
    );
    Ok(())
}

fn memory_dir_from_path(file_path: &str) -> Option<String> {
    let tail_idx = file_path.find(MEMORY_PATH_TAIL)?;
    Some(file_path[..tail_idx + MEMORY_PATH_TAIL.len() - 1].to_string())
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

fn cmd_stdignore_preview(workspace: &Path, pattern: &str, limit: usize) -> Result<(), ServerError> {
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
        &format!(
            "reset {} usage_stats row(s) for period `{period}`?",
            preview.calls
        ),
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

#[cfg(test)]
mod claude_pre_tool_hook_tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn sentinel_in(tmp: &TempDir) -> PathBuf {
        tmp.path().join("mcp_called_this_session")
    }

    #[test]
    fn mark_writes_sentinel_when_tool_is_standardoc_mcp() {
        let tmp = TempDir::new().unwrap();
        let sentinel = sentinel_in(&tmp);
        let payload = r#"{"tool_name":"mcp__standardoc__find_symbol","cwd":"/anywhere"}"#;
        let out = pre_tool_hook_decide("mark", payload, &sentinel);
        assert!(out.contains(r#""marked":true"#), "out={out}");
        assert!(sentinel.exists(), "sentinel must be written");
    }

    #[test]
    fn mark_skips_non_standardoc_tool() {
        let tmp = TempDir::new().unwrap();
        let sentinel = sentinel_in(&tmp);
        let payload = r#"{"tool_name":"Bash"}"#;
        let out = pre_tool_hook_decide("mark", payload, &sentinel);
        assert!(out.contains("not_standardoc_mcp_tool"), "out={out}");
        assert!(!sentinel.exists(), "sentinel must NOT be written");
    }

    #[test]
    fn mark_skips_when_tool_name_missing() {
        let tmp = TempDir::new().unwrap();
        let sentinel = sentinel_in(&tmp);
        let out = pre_tool_hook_decide("mark", r"{}", &sentinel);
        assert!(out.contains("not_standardoc_mcp_tool"), "out={out}");
        assert!(!sentinel.exists());
    }

    #[test]
    fn mark_creates_parent_dirs() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("deep").join(".standardoc");
        let sentinel = nested.join("mcp_called_this_session");
        let payload = r#"{"tool_name":"mcp__standardoc__get_context"}"#;
        let out = pre_tool_hook_decide("mark", payload, &sentinel);
        assert!(out.contains(r#""marked":true"#), "out={out}");
        assert!(sentinel.exists());
    }

    #[test]
    fn check_denies_when_sentinel_absent() {
        let tmp = TempDir::new().unwrap();
        let sentinel = sentinel_in(&tmp);
        let out = pre_tool_hook_decide("check", r"{}", &sentinel);
        assert!(out.contains(r#""permissionDecision":"deny""#), "out={out}");
        assert!(out.contains("MCP-first"));
        assert!(out.contains("find_symbol"));
    }

    #[test]
    fn check_allows_when_sentinel_present() {
        let tmp = TempDir::new().unwrap();
        let sentinel = sentinel_in(&tmp);
        fs::write(&sentinel, b"").unwrap();
        let out = pre_tool_hook_decide("check", r"{}", &sentinel);
        assert_eq!(out, "{}");
    }

    #[test]
    fn check_emits_pretooluse_hook_event_name() {
        // Claude Code requires the hookSpecificOutput.hookEventName to
        // match the firing event, otherwise the JSON is silently
        // ignored. Lock the wire shape.
        let tmp = TempDir::new().unwrap();
        let sentinel = sentinel_in(&tmp);
        let out = pre_tool_hook_decide("check", r"{}", &sentinel);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            parsed
                .get("hookSpecificOutput")
                .and_then(|v| v.get("hookEventName"))
                .and_then(|v| v.as_str()),
            Some("PreToolUse"),
        );
    }

    #[test]
    fn reset_removes_sentinel() {
        let tmp = TempDir::new().unwrap();
        let sentinel = sentinel_in(&tmp);
        fs::write(&sentinel, b"").unwrap();
        let out = pre_tool_hook_decide("reset", r"{}", &sentinel);
        assert!(out.contains(r#""reset":true"#));
        assert!(!sentinel.exists());
    }

    #[test]
    fn reset_is_idempotent_when_sentinel_absent() {
        let tmp = TempDir::new().unwrap();
        let sentinel = sentinel_in(&tmp);
        let out = pre_tool_hook_decide("reset", r"{}", &sentinel);
        // Must not panic; output is the reset confirmation either way.
        assert!(out.contains(r#""reset":true"#));
    }

    #[test]
    fn invalid_json_does_not_panic_in_any_mode() {
        let tmp = TempDir::new().unwrap();
        let sentinel = sentinel_in(&tmp);
        // Mark with garbage payload — must not panic, must not write
        // the sentinel (no tool name resolvable).
        let out = pre_tool_hook_decide("mark", "not json", &sentinel);
        assert!(out.contains("not_standardoc_mcp_tool"), "out={out}");
        assert!(!sentinel.exists());
        // Check with garbage payload — same as a missing sentinel.
        let out = pre_tool_hook_decide("check", "not json", &sentinel);
        assert!(out.contains(r#""permissionDecision":"deny""#));
        // Reset with garbage payload — no-op (file already absent).
        let out = pre_tool_hook_decide("reset", "not json", &sentinel);
        assert!(out.contains(r#""reset":true"#));
    }

    #[test]
    fn unknown_mode_returns_safe_default() {
        let tmp = TempDir::new().unwrap();
        let sentinel = sentinel_in(&tmp);
        let out = pre_tool_hook_decide("nope", r"{}", &sentinel);
        assert!(out.contains("unknown_mode"));
        // Must not implicitly deny — clap's value_parser already
        // forbids this CLI-side, but a defence-in-depth default is
        // "do not block the agent".
        assert!(!out.contains(r#""permissionDecision":"deny""#));
    }

    #[test]
    fn resolve_sentinel_uses_payload_cwd() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path().to_string_lossy().replace('\\', "/");
        let payload = format!(r#"{{"cwd":"{cwd}"}}"#);
        let sentinel = resolve_mcp_first_sentinel(&payload);
        let expected = tmp.path().join(".standardoc").join(MCP_FIRST_SENTINEL);
        assert_eq!(sentinel, expected);
    }

    #[test]
    fn resolve_sentinel_falls_back_to_current_dir_when_payload_lacks_cwd() {
        // The fallback chain is cwd → CLAUDE_PROJECT_DIR → current_dir;
        // we only assert the chain doesn't panic and produces a path
        // ending with the sentinel name + parent `.standardoc`.
        let sentinel = resolve_mcp_first_sentinel(r"{}");
        assert_eq!(
            sentinel.file_name().and_then(|s| s.to_str()),
            Some(MCP_FIRST_SENTINEL),
        );
        assert_eq!(
            sentinel
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str()),
            Some(".standardoc"),
        );
    }
}

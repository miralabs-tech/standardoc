//! Standardoc server — LSP + MCP + Web daemon.
//!
//! Three transports, one binary, **same `Arc<ServerState>`** under the hood.
//! Pick transport at startup:
//! - `--mcp`  : stdio JSON-RPC for AI agents
//! - `--lsp`  : stdio LSP for editors (`VSCode` / Helix / Neovim / Zed)
//! - `--web --port <N>` : HTTP server (REST + SSE + embedded frontend)
//!
//! ```sh
//! standardoc-server --mcp --workspace /path/to/project
//! standardoc-server --lsp --workspace /path/to/project
//! standardoc-server --web --port 4173 --workspace /path/to/project
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

mod lsp;
mod mcp;
mod state;
mod web;
mod worker;

fn main() -> ExitCode {
    let mut mode: Option<Mode> = None;
    let mut workspace: Option<PathBuf> = None;
    let mut port: Option<u16> = None;
    let mut export_out: Option<PathBuf> = None;
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--mcp" => mode = Some(Mode::Mcp),
            "--lsp" => mode = Some(Mode::Lsp),
            "--web" => mode = Some(Mode::Web),
            "--export" => mode = Some(Mode::Export),
            "--port" => {
                port = iter.next().and_then(|v| v.parse::<u16>().ok());
                if port.is_none() {
                    eprintln!("--port requires a valid u16 value");
                    return ExitCode::from(2);
                }
            }
            "--out" => {
                export_out = iter.next().map(PathBuf::from);
            }
            "--workspace" => {
                workspace = iter.next().map(PathBuf::from);
            }
            "--help" | "-h" => {
                print_help();
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument: {other}");
                print_help();
                return ExitCode::from(2);
            }
        }
    }

    let Some(workspace) = workspace else {
        eprintln!("--workspace <path> is required");
        print_help();
        return ExitCode::from(2);
    };

    match mode {
        Some(Mode::Mcp) => match mcp::run(&workspace) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("mcp server error: {err}");
                ExitCode::from(1)
            }
        },
        Some(Mode::Lsp) => {
            // LSP needs a tokio runtime (`tower-lsp` is async-native).
            // MCP does not (blocking stdio). Runtime is created only in LSP
            // branch to avoid paying that cost in MCP mode.
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(err) => {
                    eprintln!("failed to start tokio runtime: {err}");
                    return ExitCode::from(1);
                }
            };
            match rt.block_on(lsp::run(workspace)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("lsp server error: {err}");
                    ExitCode::from(1)
                }
            }
        }
        Some(Mode::Web) => {
            let Some(port) = port else {
                eprintln!("--web requires --port <N>");
                print_help();
                return ExitCode::from(2);
            };
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(err) => {
                    eprintln!("failed to start tokio runtime: {err}");
                    return ExitCode::from(1);
                }
            };
            match rt.block_on(run_web(workspace, port)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("web server error: {err}");
                    ExitCode::from(1)
                }
            }
        }
        Some(Mode::Export) => {
            let Some(out) = export_out else {
                eprintln!("--export requires --out <dir>");
                print_help();
                return ExitCode::from(2);
            };
            eprintln!(
                "standardoc-server: scanning workspace {}…",
                workspace.display()
            );
            let state = match state::ServerState::boot_for_web(&workspace) {
                Ok(s) => std::sync::Arc::new(s),
                Err(err) => {
                    eprintln!("boot failed: {err}");
                    return ExitCode::from(1);
                }
            };
            let adapter: std::sync::Arc<dyn standardoc_web::WebState> =
                std::sync::Arc::new(web::WebStateAdapter::new(std::sync::Arc::clone(&state)));
            eprintln!("standardoc-server: exporting to {}…", out.display());
            match standardoc_web::export::export_to(adapter.as_ref(), &out) {
                Ok(0) => {
                    eprintln!("standardoc-server: exported static-data.json (data-only mode — no frontend bundled in this binary)");
                    ExitCode::SUCCESS
                }
                Ok(n) => {
                    eprintln!("standardoc-server: exported {n} assets + static-data.json");
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("export failed: {err}");
                    ExitCode::from(1)
                }
            }
        }
        None => {
            eprintln!("no transport selected (try --mcp, --lsp or --web)");
            print_help();
            ExitCode::from(2)
        }
    }
}

async fn run_web(workspace: PathBuf, port: u16) -> Result<(), String> {
    use std::sync::Arc;

    // Pre-check: path must exist and be a directory. Without this, scanner
    // errors later with opaque OS messages.
    if !workspace.exists() {
        return Err(format!(
            "workspace path does not exist: {}",
            workspace.display()
        ));
    }
    if !workspace.is_dir() {
        return Err(format!(
            "workspace path is not a directory: {}",
            workspace.display()
        ));
    }

    // Boot index without MCP stdout — no stdout writes in this mode, console
    // output stays available for human logs.
    // Pre-warm syntect grammars + themes in a blocking thread so the first
    // page load doesn't pay the ~200 ms grammar-parse cost.
    tokio::task::spawn_blocking(standardoc_web::highlight::prewarm)
        .await
        .ok();

    let state = state::ServerState::boot_for_web(&workspace)
        .map_err(|err| format!("boot failed for workspace {}: {err}", workspace.display()))?;
    let state = Arc::new(state);
    let adapter: Arc<dyn standardoc_web::WebState> =
        Arc::new(web::WebStateAdapter::new(Arc::clone(&state)));
    let app = standardoc_web::router(adapter);

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|err| format!("cannot bind {addr}: {err}"))?;
    eprintln!("standardoc-server: listening on http://{addr}");
    eprintln!("  workspace: {}", workspace.display());
    eprintln!("  GET  /api/health");
    eprintln!("  GET  /api/index");
    eprintln!("  GET  /api/doc/:key");
    eprintln!("  GET  /api/search?q=...");
    eprintln!("  GET  /api/events  (SSE)");

    // Keep state alive while serving — `Arc` above is cloned in adapter and
    // survives this function while server runs.
    let _state_keep_alive = state;

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|err| format!("axum serve error: {err}"))
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sig) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            sig.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    eprintln!("standardoc-server: shutdown signal received");
}

/// @doc cli.transports.help --help
/// @category meta
/// @since 0.1
/// @usage standardoc-server --help
/// @description
/// Print the transport list with brief usage. Always exits `0`.
fn print_help() {
    eprintln!(
        "standardoc-server — Standardoc daemon\n\n\
         USAGE:\n  \
         standardoc-server --mcp --workspace <path>\n      \
             Run an MCP server on stdio for the given workspace.\n  \
         standardoc-server --lsp --workspace <path>\n      \
             Run an LSP server on stdio (for VSCode / Helix / Neovim / Zed).\n  \
         standardoc-server --web --port <N> --workspace <path>\n      \
             Run an HTTP server on the given port (REST + SSE + embedded UI).\n  \
         standardoc-server --export --out <dir> --workspace <path>\n      \
             Export a static site to <dir> for CDN deployment.\n  \
         standardoc-server --help\n      \
             Show this help."
    );
}

enum Mode {
    /// @doc cli.transports.mcp mcp
    /// @category transport
    /// @since 0.1
    /// @usage standardoc-server --mcp --workspace <path>
    /// @description
    /// Speak the [Model Context Protocol](https://modelcontextprotocol.io/) over
    /// **stdio** (JSON-RPC 2.0). Use this from `.mcp.json` to expose the workspace
    /// to AI agents (Claude Code, Cursor, Zed, Continue, …). See the
    /// [MCP reference](mcp-reference.md) for the full list of tools available.
    ///
    /// The daemon scans once at boot, watches the workspace for changes, and pushes
    /// notifications when the index changes. State stays alive for the lifetime of
    /// the host process.
    Mcp,
    /// @doc cli.transports.lsp lsp
    /// @category transport
    /// @since 0.1
    /// @usage standardoc-server --lsp --workspace <path>
    /// @description
    /// Speak [LSP](https://microsoft.github.io/language-server-protocol/) over **stdio**
    /// for editors (VSCode, Helix, Neovim, Zed, …). Capabilities :
    ///
    /// - Completion on `@`, `{`, `.`, `:` triggers
    /// - Hover, goto-definition (DSL → source), references (source → `.md`)
    /// - Document / workspace symbols, code actions
    /// - **Rename** that propagates `DocKey` changes into all `.md` consumers
    /// - Formatting, push diagnostics on every rescan
    /// - 10 diagnostic codes (STD001-STD008 + STD012-STD013; STD009-STD011 reserved)
    Lsp,
    /// @doc cli.transports.web web
    /// @category transport
    /// @since 0.1
    /// @usage standardoc-server --web --port <N> --workspace <path>
    /// @description
    /// Serve a REST + SSE HTTP API on the given port. Endpoints:
    ///
    /// - `GET /api/health` — `{ "ok": true, "revision": N }`
    /// - `GET /api/index` — full index snapshot
    /// - `GET /api/doc/{key}` — single block detail
    /// - `GET /api/search?q=...` — substring + fuzzy fallback search
    /// - `GET /api/dsl-reference` — markdown DSL reference (same content as MCP `get_dsl_reference`)
    /// - `GET /api/config` — resolved configuration
    /// - `GET /api/pages` — list narrative pages
    /// - `GET /api/page/{*slug}` — full content of one page (also `PUT`, `PATCH`, `DELETE`)
    /// - `GET /api/events` — Server-Sent Events stream (`index_changed`, `diagnostics`, …)
    /// - `GET /api/syntax.css` — syntect-generated CSS for code highlighting
    /// - Fallback `/*` — embedded SPA (only when binary is built with
    ///   `--features standardoc-web/embedded-frontend`, i.e. Standardoc Pro),
    ///   otherwise a placeholder
    ///
    /// **CORS** is wide-open by default (`allow_origin: any`) for local dev and
    /// self-hosted SPAs. Tighten in a reverse-proxy if you expose this beyond
    /// `localhost`.
    Web,
    /// @doc cli.transports.export export
    /// @category transport
    /// @since 0.1
    /// @usage standardoc-server --export --workspace <path> --out <dir>
    /// @description
    /// One-shot static export. Writes `static-data.json` (full index snapshot, all
    /// blocks, pre-rendered pages, resolved source-link config) to `<dir>`. If the
    /// binary was built with `embedded-frontend`, also writes the bundled SPA as a
    /// CDN-deployable site; otherwise it's data-only and consumable by any external
    /// SSG (Astro, Vitepress, Hugo, custom).
    Export,
}

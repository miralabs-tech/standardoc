use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use standardoc_mcp_proxy::{ProxyConfig, run};

/// Long-lived HTTP proxy in front of the standardoc daemon's MCP
/// transport. Keeps the MCP client (Claude Code, Copilot Chat, …)
/// connected to a stable URL across daemon restarts / rebuilds /
/// migrations. File-watches `<workspace>/.standardoc/mcp.endpoint` to
/// track the daemon's actual address and retries on upstream connection
/// refused.
#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Local bind address. Configure the MCP client to point at
    /// `http://<bind>/mcp`. Default `127.0.0.1:7700`.
    #[arg(long, default_value = "127.0.0.1:7700")]
    bind: String,

    /// Workspace root used to locate `.standardoc/mcp.endpoint`. The
    /// proxy reads + watches this file to discover the daemon's
    /// current HTTP URL. Defaults to the current working directory.
    #[arg(long, value_name = "DIR")]
    workspace: Option<PathBuf>,

    /// How long (in seconds) to keep retrying a request when the
    /// upstream daemon is unreachable before returning `503`.
    /// Generous default so daemon rebuilds / cold-start don't surface
    /// as user-visible failures.
    #[arg(long, default_value_t = 30)]
    retry_window_secs: u64,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let workspace = match cli.workspace {
        Some(p) => p,
        None => match std::env::current_dir() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("standardoc-mcp-proxy: cannot resolve current dir: {e}");
                return ExitCode::FAILURE;
            }
        },
    };
    let cfg = ProxyConfig {
        bind_addr: cli.bind,
        workspace_root: workspace,
        upstream_retry_window: Duration::from_secs(cli.retry_window_secs),
    };

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("standardoc-mcp-proxy: tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    let result = rt.block_on(run(cfg));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("standardoc-mcp-proxy: fatal: {e}");
            ExitCode::FAILURE
        }
    }
}

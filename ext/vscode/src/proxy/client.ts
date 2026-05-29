import * as vscode from 'vscode';
import { spawn, type ChildProcess } from 'node:child_process';

/**
 * Long-lived `standardoc proxy` sidecar so external MCP clients
 * (Claude Code CLI, Copilot Chat over HTTP, your own scripts) get a
 * stable URL across daemon restarts and across multiple open VSCode
 * windows.
 *
 * Lifecycle is intentionally loose: the proxy binary's built-in
 * singleton check (probe /health → register-and-exit when an existing
 * proxy is already listening) collapses sibling VSCode windows onto
 * one shared proxy instance. We spawn the child, pipe its stderr to
 * the Standardoc output channel for visibility, and never try to
 * restart it — sibling-instance children exit 0 immediately after
 * registering, the winning child stays up for everyone.
 *
 * On dispose we kill the child best-effort. When the winning child
 * dies that closes the proxy for siblings too; in practice the next
 * MCP call from any other window re-spawns it, which the singleton
 * dedupes back to a single instance. Good enough for V1.
 */
export class ProxyClient implements vscode.Disposable {
  private child: ChildProcess | null = null;

  constructor(
    private readonly workspaceRoot: string,
    private readonly output: vscode.OutputChannel,
  ) {}

  async start(binaryPath: string): Promise<void> {
    if (this.child) return;
    const args = ['proxy', '--workspace', this.workspaceRoot];
    this.output.appendLine(`[proxy] spawning ${binaryPath} ${args.join(' ')}`);
    const child = spawn(binaryPath, args, { stdio: ['ignore', 'pipe', 'pipe'] });
    this.child = child;
    child.stdout?.on('data', chunk =>
      this.output.append(`[proxy] ${chunk.toString()}`),
    );
    child.stderr?.on('data', chunk =>
      this.output.append(`[proxy] ${chunk.toString()}`),
    );
    child.on('exit', (code, signal) => {
      this.child = null;
      // Exit 0 = singleton path (registered with existing proxy, fine).
      // Any other code: log but don't propagate — losing the proxy
      // doesn't impact the local LSP/MCP daemons, just degrades
      // external clients pointed at the stable URL.
      this.output.appendLine(
        `[proxy] child exited code=${code ?? 'null'} signal=${signal ?? 'null'}`,
      );
    });
    child.on('error', err => {
      this.output.appendLine(`[proxy] spawn error: ${err.message}`);
      this.child = null;
    });
  }

  async stop(): Promise<void> {
    const child = this.child;
    if (!child) return;
    this.child = null;
    try {
      child.kill();
    } catch (e) {
      this.output.appendLine(
        `[proxy] kill error: ${e instanceof Error ? e.message : String(e)}`,
      );
    }
  }

  dispose(): void {
    void this.stop();
  }
}

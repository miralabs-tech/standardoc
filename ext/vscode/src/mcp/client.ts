import * as vscode from 'vscode';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { spawn, type ChildProcess } from 'node:child_process';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import { findFatalMarker, type FatalConfig } from '../daemon/fatal-marker';

export type ContextDepth = 1 | 2;

const FIND_SYMBOL_DEFAULT_LIMIT = 20;
/** Cap on the wait for the daemon to write its endpoint file. */
const ENDPOINT_WAIT_MS = 15_000;
const ENDPOINT_POLL_MS = 100;

/**
 * MCP client that supervises a SINGLE long-lived `standardoc mcp --http`
 * child per workspace and connects to it over HTTP/SSE
 * (rmcp `streamable-http` transport). Every other MCP consumer in the
 * editor (Copilot Chat sessions, Claude Code in VSCode chats, the MCP
 * provider for external clients) connects to the same daemon over the
 * same URL — eliminating the per-chat stdio child-spawn cost of the
 * previous transport.
 */
export class McpClient implements vscode.Disposable {
  private client: Client | null = null;
  private child: ChildProcess | null = null;
  private endpointUrl: string | null = null;
  private readonly fatalEmitter = new vscode.EventEmitter<FatalConfig>();
  private fatalFired = false;

  /**
   * Fires once per MCP child-process lifecycle when its stderr emits a
   * structured `STDOC_FATAL: ...` marker. The supervisor uses this to
   * skip the backoff machinery and surface an actionable error to the
   * user (since retrying without a binary upgrade would just re-fail).
   */
  readonly onFatalConfig: vscode.Event<FatalConfig> = this.fatalEmitter.event;

  private readonly exitEmitter = new vscode.EventEmitter<void>();

  /**
   * Fires when the supervised MCP child exits WITHOUT going through
   * `stop()` (crash, OOM-kill, panic with no STDOC_FATAL marker).
   * `stop()` nulls `this.child` before killing, so the exit handler tells
   * a deliberate teardown apart from an unexpected death. The supervisor
   * subscribes to drive its crash/backoff machinery — otherwise a dead
   * MCP daemon paired with a still-running LSP would leave the supervisor
   * stuck at `ready`.
   */
  readonly onExit: vscode.Event<void> = this.exitEmitter.event;

  constructor(
    private readonly workspaceRoot: string,
    private readonly output: vscode.OutputChannel,
    private readonly port: number,
  ) {}

  /** Returns the discovered HTTP URL or `null` if the daemon is not started yet. */
  url(): string | null {
    return this.endpointUrl;
  }

  async start(binaryPath: string): Promise<void> {
    if (this.client) return;

    this.fatalFired = false;
    // Belt-and-suspenders: nuke any leftover endpoint file BEFORE spawning the
    // new daemon. Covers the case where the previous daemon crashed without
    // going through `stop()` (which also performs this cleanup). Without
    // this, `waitForEndpoint` would read the stale URL and racing the fresh
    // bind would surface as "fetch failed".
    const endpointFile = path.join(this.workspaceRoot, '.standardoc', 'mcp.endpoint');
    try {
      await fs.promises.unlink(endpointFile);
    } catch (e: unknown) {
      const code = (e as NodeJS.ErrnoException).code;
      if (code !== 'ENOENT') {
        this.output.appendLine(`[mcp] endpoint pre-spawn cleanup error: ${describe(e)}`);
      }
    }

    const args = ['mcp', this.workspaceRoot, '--readonly', '--http', String(this.port)];
    this.output.appendLine(`[mcp] spawning ${binaryPath} ${args.slice(1).join(' ')}`);
    // stdin is piped (not 'ignore') so the daemon has a death-watch channel:
    // when the extension host dies (force-kill, BSOD, crash), the OS closes
    // the parent end of this pipe, the daemon reads EOF, and exits — the
    // fs4 workspace lock is released without manual Task Manager cleanup.
    // We never write to this pipe; it carries no protocol data.
    const child = spawn(binaryPath, args, { stdio: ['pipe', 'pipe', 'pipe'] });
    this.child = child;
    // Surface an UNEXPECTED death (crash / OOM / panic) so the supervisor
    // can restart. `stop()` nulls this.child before killing, so a still-
    // matching reference here means the daemon died on its own.
    child.on('exit', (code, signal) => {
      if (this.child !== child) return;
      this.child = null;
      this.endpointUrl = null;
      this.output.appendLine(
        `[mcp] daemon child exited unexpectedly (code=${code ?? 'null'} signal=${signal ?? 'null'})`,
      );
      this.exitEmitter.fire();
    });
    this.attachStderrScanner(child);

    const endpoint = await this.waitForEndpoint();
    this.endpointUrl = endpoint;
    this.output.appendLine(`[mcp] daemon endpoint: ${endpoint}`);

    const transport = new StreamableHTTPClientTransport(new URL(endpoint));
    const client = new Client(
      { name: 'standardoc-vscode', version: '0.1.0-beta.2' },
      { capabilities: {} },
    );
    await client.connect(transport);
    this.client = client;
    this.output.appendLine('[mcp] connected');
  }

  /**
   * Wait for the daemon to write `.standardoc/mcp.endpoint`. Aborts
   * early if the child process exits before the file appears (typical
   * cause: port already bound by another workspace's daemon).
   */
  private async waitForEndpoint(): Promise<string> {
    const endpointFile = path.join(this.workspaceRoot, '.standardoc', 'mcp.endpoint');
    const start = Date.now();
    for (;;) {
      if (this.child === null || this.child.exitCode !== null) {
        throw new Error(
          `mcp daemon exited (code=${this.child?.exitCode ?? 'unknown'}) before writing ${endpointFile}`,
        );
      }
      try {
        const raw = await fs.promises.readFile(endpointFile, 'utf8');
        const trimmed = raw.trim();
        if (trimmed.length > 0) return trimmed;
      } catch (e: unknown) {
        const code = (e as NodeJS.ErrnoException).code;
        if (code !== 'ENOENT') throw e;
      }
      const elapsed = Date.now() - start;
      if (elapsed >= ENDPOINT_WAIT_MS) {
        throw new Error(`timed out waiting for ${endpointFile} after ${ENDPOINT_WAIT_MS}ms`);
      }
      await new Promise(r => setTimeout(r, ENDPOINT_POLL_MS));
    }
  }

  private attachStderrScanner(child: ChildProcess): void {
    const stderr = child.stderr;
    if (!stderr) return;
    stderr.on('data', (chunk: Buffer | string) => {
      const text = typeof chunk === 'string' ? chunk : chunk.toString('utf8');
      this.output.append(text);
      if (this.fatalFired) return;
      const marker = findFatalMarker(text);
      if (marker !== null) {
        this.fatalFired = true;
        this.fatalEmitter.fire(marker);
      }
    });
  }

  async findSymbol(query: string, limit: number = FIND_SYMBOL_DEFAULT_LIMIT): Promise<string> {
    return this.callTool('find_symbol', { query, limit });
  }

  async getContext(fqdn: string, depth: ContextDepth): Promise<string> {
    return this.callTool('get_context', { fqdn, depth });
  }

  async stop(): Promise<void> {
    const client = this.client;
    this.client = null;
    if (client) {
      try {
        await client.close();
      } catch (e) {
        this.output.appendLine(`[mcp] client close error: ${describe(e)}`);
      }
    }
    const child = this.child;
    this.child = null;
    this.endpointUrl = null;
    if (child && child.exitCode === null) {
      child.kill();
      await new Promise<void>(resolve => {
        child.once('exit', () => resolve());
        // Force fallback: SIGKILL after 2s if the daemon ignores the polite kill.
        setTimeout(() => {
          try {
            child.kill('SIGKILL');
          } catch {
            // process already gone, ignore
          }
          resolve();
        }, 2000);
      });
    }
    // Delete the stale endpoint file so a subsequent `start()` actually waits
    // for the NEXT daemon to write its endpoint, instead of reading the dead
    // daemon's URL and racing the fresh bind. Race manifests as
    // "fetch failed" when the new daemon hasn't bound its port yet but the
    // file still points at the (now-dead) previous port.
    const endpointFile = path.join(this.workspaceRoot, '.standardoc', 'mcp.endpoint');
    try {
      await fs.promises.unlink(endpointFile);
    } catch (e: unknown) {
      const code = (e as NodeJS.ErrnoException).code;
      if (code !== 'ENOENT') {
        this.output.appendLine(`[mcp] endpoint cleanup error: ${describe(e)}`);
      }
    }
    this.output.appendLine('[mcp] disconnected');
  }

  dispose(): void {
    void this.stop();
    this.fatalEmitter.dispose();
    this.exitEmitter.dispose();
  }

  private async callTool(name: string, args: Record<string, unknown>): Promise<string> {
    const client = this.client;
    if (!client) throw new Error('MCP client not started');
    const result = await client.callTool({ name, arguments: args });
    return extractFirstText(result);
  }
}

function extractFirstText(result: unknown): string {
  const r = result as { content?: Array<{ type?: string; text?: string }> };
  if (!Array.isArray(r.content) || r.content.length === 0) return '';
  const first = r.content[0];
  if (!first || typeof first.text !== 'string') return '';
  return first.text;
}

function describe(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

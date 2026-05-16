import * as vscode from 'vscode';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { spawn, type ChildProcess } from 'node:child_process';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import { findFatalMarker, type FatalConfig } from '../daemon/fatal-marker';
import { DEFAULT_RAG_SETTINGS, ragSpawnFlags, type RagSettings } from '../daemon/rag-flags';

export type ContextDepth = 1 | 2;

const FIND_SYMBOL_DEFAULT_LIMIT = 20;
/** Normal cap : daemon should write the endpoint file within seconds. */
const ENDPOINT_WAIT_MS = 15_000;
/** Extended cap once the daemon emits `STDOC_RAG_DL_START` — the model
 *  download (~130 MB) takes longer on residential ADSL than the normal
 *  cap allows. We still cap so a stuck daemon eventually frees the
 *  supervisor, but with a generous ceiling. */
const ENDPOINT_DL_WAIT_MS = 10 * 60_000;
const ENDPOINT_POLL_MS = 100;
const RAG_DL_START_MARKER = 'STDOC_RAG_DL_START';
const RAG_DL_DONE_MARKER = 'STDOC_RAG_DL_DONE';

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
  private ragSettings: RagSettings = DEFAULT_RAG_SETTINGS;
  /**
   * Flips to `true` when the daemon emits `STDOC_RAG_DL_START` and back
   * to `false` on `STDOC_RAG_DL_DONE`. `waitForEndpoint` consults this
   * flag to switch from the 15 s normal cap to the 10 min DL cap —
   * a fresh `--embedder candle` boot can spend most of that window
   * pulling the ~130 MB BERT weights from HF Hub.
   */
  private ragDownloading = false;

  /**
   * Fires once per MCP child-process lifecycle when its stderr emits a
   * structured `STDOC_FATAL: ...` marker. The supervisor uses this to
   * skip the backoff machinery and surface an actionable error to the
   * user (since retrying without a binary upgrade would just re-fail).
   */
  readonly onFatalConfig: vscode.Event<FatalConfig> = this.fatalEmitter.event;

  constructor(
    private readonly workspaceRoot: string,
    private readonly output: vscode.OutputChannel,
    private readonly port: number,
  ) {}

  /**
   * Updates the RAG flags applied to the next `--rag`/`--embedder` spawn.
   * Does NOT restart the running daemon — the caller (typically the
   * supervisor or the toggle command) is responsible for sequencing
   * stop → setRagSettings → spawn.
   */
  setRagSettings(settings: RagSettings): void {
    this.ragSettings = settings;
  }

  ragSettingsSnapshot(): RagSettings {
    return this.ragSettings;
  }

  /** Returns the discovered HTTP URL or `null` if the daemon is not started yet. */
  url(): string | null {
    return this.endpointUrl;
  }

  async start(binaryPath: string): Promise<void> {
    if (this.client) return;

    this.fatalFired = false;
    this.ragDownloading = false;
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

    const args = [
      'mcp',
      this.workspaceRoot,
      '--readonly',
      '--http',
      String(this.port),
      ...ragSpawnFlags(this.ragSettings),
    ];
    this.output.appendLine(`[mcp] spawning ${binaryPath} ${args.slice(1).join(' ')}`);
    // stdin is piped (not 'ignore') so the daemon has a death-watch channel:
    // when the extension host dies (force-kill, BSOD, crash), the OS closes
    // the parent end of this pipe, the daemon reads EOF, and exits — the
    // fs4 workspace lock is released without manual Task Manager cleanup.
    // We never write to this pipe; it carries no protocol data.
    const child = spawn(binaryPath, args, { stdio: ['pipe', 'pipe', 'pipe'] });
    this.child = child;
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
   *
   * The effective deadline is dynamic: while `ragDownloading` is true
   * (set by the stderr scanner on `STDOC_RAG_DL_START`), the deadline
   * extends to `ENDPOINT_DL_WAIT_MS`. The flag resets on
   * `STDOC_RAG_DL_DONE`, after which the normal cap applies — measured
   * from the start of the wait, so a fast pipe still surfaces a hung
   * daemon promptly.
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
      const cap = this.ragDownloading ? ENDPOINT_DL_WAIT_MS : ENDPOINT_WAIT_MS;
      if (elapsed >= cap) {
        throw new Error(
          `timed out waiting for ${endpointFile} after ${cap}ms` +
            (this.ragDownloading ? ' (during RAG model download)' : ''),
        );
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
      this.scanRagMarkers(text);
      if (this.fatalFired) return;
      const marker = findFatalMarker(text);
      if (marker !== null) {
        this.fatalFired = true;
        this.fatalEmitter.fire(marker);
      }
    });
  }

  private scanRagMarkers(text: string): void {
    if (text.includes(RAG_DL_START_MARKER)) {
      this.ragDownloading = true;
      this.output.appendLine('[mcp] RAG model download started — endpoint wait extended');
    }
    if (text.includes(RAG_DL_DONE_MARKER)) {
      this.ragDownloading = false;
      this.output.appendLine('[mcp] RAG model download finished');
    }
  }

  async findSymbol(query: string, limit: number = FIND_SYMBOL_DEFAULT_LIMIT): Promise<string> {
    return this.callTool('find_symbol', { query, limit });
  }

  async getContext(fqdn: string, depth: ContextDepth): Promise<string> {
    return this.callTool('get_context', { fqdn, depth });
  }

  async fetchChunks(uris: ReadonlyArray<string>): Promise<string> {
    return this.callTool('fetch_chunks', { uris: [...uris] });
  }

  async usageStats(period: 'day' | 'week' | 'all'): Promise<string> {
    return this.callTool('usage_stats', { period });
  }

  async currentRevision(): Promise<string> {
    return this.callTool('current_revision', {});
  }

  async checkStale(pairs: ReadonlyArray<{ fqdn: string; fetched_at_revision: number }>): Promise<string> {
    return this.callTool('check_stale', { pairs: [...pairs] });
  }

  async listSymbols(args: {
    readonly kind?: string;
    readonly visibility?: string;
    readonly module?: string;
    readonly limit?: number;
    readonly include_external?: boolean;
    readonly cursor?: string;
  }): Promise<string> {
    const payload: Record<string, unknown> = {};
    if (args.kind !== undefined) payload.kind = args.kind;
    if (args.visibility !== undefined) payload.visibility = args.visibility;
    if (args.module !== undefined) payload.module = args.module;
    if (args.limit !== undefined) payload.limit = args.limit;
    if (args.include_external !== undefined) payload.include_external = args.include_external;
    if (args.cursor !== undefined) payload.cursor = args.cursor;
    return this.callTool('list_symbols', payload);
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

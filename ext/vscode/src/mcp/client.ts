import * as vscode from 'vscode';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import { findFatalMarker, type FatalConfig } from '../daemon/fatal-marker';

export type ContextDepth = 1 | 2;

const FIND_SYMBOL_DEFAULT_LIMIT = 20;

export class McpClient implements vscode.Disposable {
  private client: Client | null = null;
  private readonly fatalEmitter = new vscode.EventEmitter<FatalConfig>();
  private stderrSub: { dispose(): void } | null = null;

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
  ) {}

  async start(binaryPath: string): Promise<void> {
    if (this.client) return;

    const transport = new StdioClientTransport({
      command: binaryPath,
      args: ['mcp', this.workspaceRoot, '--readonly'],
      stderr: 'pipe',
    });
    const client = new Client(
      { name: 'standardoc-vscode', version: '0.1.0-beta.2' },
      { capabilities: {} },
    );

    this.attachStderrScanner(transport);

    await client.connect(transport);
    this.client = client;
    this.output.appendLine('[mcp] connected');
  }

  /**
   * Tees the MCP child's stderr into the output channel AND scans each
   * chunk for a structured `STDOC_FATAL: ...` marker. The first marker
   * encountered fires `onFatalConfig` exactly once per lifecycle.
   */
  private attachStderrScanner(transport: StdioClientTransport): void {
    const stderr = transport.stderr;
    if (!stderr) return;
    let fatalFired = false;
    const onData = (chunk: Buffer | string): void => {
      const text = typeof chunk === 'string' ? chunk : chunk.toString('utf8');
      this.output.append(text);
      if (fatalFired) return;
      const marker = findFatalMarker(text);
      if (marker !== null) {
        fatalFired = true;
        this.fatalEmitter.fire(marker);
      }
    };
    stderr.on('data', onData);
    this.stderrSub = {
      dispose: (): void => {
        stderr.off('data', onData);
      },
    };
  }

  async findSymbol(query: string, limit: number = FIND_SYMBOL_DEFAULT_LIMIT): Promise<string> {
    return this.callTool('find_symbol', { query, limit });
  }

  async getContext(fqdn: string, depth: ContextDepth): Promise<string> {
    return this.callTool('get_context', { fqdn, depth });
  }

  async stop(): Promise<void> {
    const client = this.client;
    if (!client) return;
    this.client = null;
    this.stderrSub?.dispose();
    this.stderrSub = null;
    await client.close();
    this.output.appendLine('[mcp] disconnected');
  }

  dispose(): void {
    this.stderrSub?.dispose();
    this.stderrSub = null;
    if (this.client) {
      void this.client.close().catch(() => undefined);
      this.client = null;
    }
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

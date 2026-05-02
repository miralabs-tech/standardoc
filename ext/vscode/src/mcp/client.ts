import * as vscode from 'vscode';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';

export type ContextDepth = 1 | 2;

const FIND_SYMBOL_DEFAULT_LIMIT = 20;

export class McpClient implements vscode.Disposable {
  private client: Client | null = null;

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

    await client.connect(transport);
    this.client = client;
    this.output.appendLine('[mcp] connected');
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
    await client.close();
    this.output.appendLine('[mcp] disconnected');
  }

  dispose(): void {
    if (this.client) {
      void this.client.close().catch(() => undefined);
      this.client = null;
    }
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

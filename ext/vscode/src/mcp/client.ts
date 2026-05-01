import * as vscode from 'vscode';
import type { Client } from '@modelcontextprotocol/sdk/client/index.js';

export type ContextDepth = 1 | 2;

export class McpClient implements vscode.Disposable {
  private client: Client | null = null;

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly workspaceRoot: string,
    private readonly output: vscode.OutputChannel,
  ) {}

  async start(_binaryPath: string): Promise<void> {
    throw new Error(
      'TODO: Phase B — spawn `standardoc mcp <workspace> --readonly` over stdio + connect MCP Client',
    );
  }

  async findSymbol(_query: string, _limit?: number): Promise<unknown> {
    throw new Error('TODO: Phase B — call MCP tool `find_symbol`');
  }

  async getContext(_fqdn: string, _depth: ContextDepth): Promise<unknown> {
    throw new Error('TODO: Phase B — call MCP tool `get_context`');
  }

  async stop(): Promise<void> {
    throw new Error('TODO: Phase B — close MCP Client connection');
  }

  dispose(): void {
    void this.client?.close().catch(() => undefined);
  }
}

import * as vscode from 'vscode';

/**
 * Provides Standardoc as an MCP HTTP server to VSCode's chat ecosystem
 * (Copilot Chat, Claude Code in VSCode, …). The actual daemon is
 * supervised by the extension as a single long-lived child — this
 * provider just hands out its endpoint URL so every chat session
 * connects to the shared daemon instead of spawning its own stdio
 * child per chat (the source of the historical
 * "one standardoc.exe per chat" RAM bloat).
 *
 * VSCode's `vscode.lm.registerMcpServerDefinitionProvider` API supports
 * both stdio and HTTP definitions in recent VSCode versions; this
 * provider exclusively emits `McpHttpServerDefinition`.
 */
export class StandardocMcpServerProvider
  implements vscode.McpServerDefinitionProvider<vscode.McpHttpServerDefinition>, vscode.Disposable
{
  private readonly emitter = new vscode.EventEmitter<void>();

  readonly onDidChangeMcpServerDefinitions: vscode.Event<void> = this.emitter.event;

  constructor(
    private readonly endpointResolver: () => string | null,
    private readonly output: vscode.OutputChannel,
  ) {}

  /** Re-emits the definition list — call when the supervisor restarts
   *  and the endpoint URL might have changed. */
  refresh(): void {
    this.emitter.fire();
  }

  async provideMcpServerDefinitions(
    _token: vscode.CancellationToken,
  ): Promise<vscode.McpHttpServerDefinition[]> {
    const endpoint = this.endpointResolver();
    if (endpoint === null) {
      this.output.appendLine(
        '[mcp-provider] no endpoint resolved yet — returning empty definition list',
      );
      return [];
    }
    try {
      return [new vscode.McpHttpServerDefinition('Standardoc', vscode.Uri.parse(endpoint))];
    } catch (e) {
      this.output.appendLine(
        `[mcp-provider] failed to build definition for ${endpoint}: ${e instanceof Error ? e.message : String(e)}`,
      );
      return [];
    }
  }

  dispose(): void {
    this.emitter.dispose();
  }
}

import * as vscode from 'vscode';
import { resolveBinary } from '../daemon/binary';

export class StandardocMcpServerProvider
  implements vscode.McpServerDefinitionProvider<vscode.McpStdioServerDefinition>, vscode.Disposable
{
  private readonly emitter = new vscode.EventEmitter<void>();
  private readonly configSub: vscode.Disposable;

  readonly onDidChangeMcpServerDefinitions: vscode.Event<void> = this.emitter.event;

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly workspaceRoot: string,
    private readonly output: vscode.OutputChannel,
  ) {
    this.configSub = vscode.workspace.onDidChangeConfiguration(e => {
      if (e.affectsConfiguration('standardoc.binaryPath')) {
        this.emitter.fire();
      }
    });
  }

  async provideMcpServerDefinitions(
    _token: vscode.CancellationToken,
  ): Promise<vscode.McpStdioServerDefinition[]> {
    try {
      const binary = await resolveBinary(this.context);
      return [
        new vscode.McpStdioServerDefinition(
          'Standardoc',
          binary.path,
          ['mcp', this.workspaceRoot, '--readonly'],
          {},
          this.context.extension.packageJSON.version,
        ),
      ];
    } catch (e) {
      this.output.appendLine(
        `[mcp-provider] could not resolve binary: ${e instanceof Error ? e.message : String(e)}`,
      );
      return [];
    }
  }

  dispose(): void {
    this.configSub.dispose();
    this.emitter.dispose();
  }
}

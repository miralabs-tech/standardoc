import * as vscode from 'vscode';
import type { LanguageClient } from 'vscode-languageclient/node';

export class LspClient implements vscode.Disposable {
  private client: LanguageClient | null = null;

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly workspaceRoot: string,
    private readonly output: vscode.OutputChannel,
  ) {}

  async start(_binaryPath: string): Promise<void> {
    throw new Error(
      'TODO: Phase B — spawn `standardoc lsp <workspace>` over stdio + start LanguageClient',
    );
  }

  async stop(): Promise<void> {
    throw new Error('TODO: Phase B — gracefully stop LanguageClient');
  }

  dispose(): void {
    void this.client?.dispose();
  }
}

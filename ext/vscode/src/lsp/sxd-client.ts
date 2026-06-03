import * as vscode from 'vscode';
import {
  LanguageClient,
  type LanguageClientOptions,
  type ServerOptions,
  TransportKind,
} from 'vscode-languageclient/node';

/**
 * Dedicated LSP client for `.sxd` (Standardoc workspace config) files.
 *
 * Spawns `standardoc lsp-sxd` which drives the upstream `standarx-dsl`
 * parser plus our SxdSchema layer — syntactic diagnostics from the
 * parser and schema-aware diagnostics (`unknown field`, `project
 * requires path`, ...) from the schema, both published as the user
 * types.
 *
 * Stateless and workspace-agnostic: every open `.sxd` document
 * validates independently, so a single instance can serve all
 * folders. Failure here is non-fatal — the rest of the extension
 * keeps running without `.sxd` live diagnostics.
 */
export class SxdLspClient implements vscode.Disposable {
  private client: LanguageClient | null = null;

  constructor(private readonly output: vscode.OutputChannel) {}

  async start(binaryPath: string): Promise<void> {
    if (this.client) return;

    const args = ['lsp-sxd'];
    this.output.appendLine(`[lsp-sxd] spawning ${binaryPath} ${args.join(' ')}`);
    const serverOptions: ServerOptions = {
      command: binaryPath,
      args,
      transport: TransportKind.stdio,
    };
    const clientOptions: LanguageClientOptions = {
      documentSelector: [{ scheme: 'file', language: 'standardoc-sxd' }],
      outputChannel: this.output,
    };

    const client = new LanguageClient(
      'standardoc-sxd-lsp',
      'Standardoc .sxd',
      serverOptions,
      clientOptions,
    );
    this.client = client;
    await client.start();
  }

  async stop(): Promise<void> {
    const client = this.client;
    if (!client) return;
    this.client = null;
    await client.stop();
  }

  dispose(): void {
    if (this.client) {
      void this.client.stop().catch(() => undefined);
      this.client = null;
    }
  }
}

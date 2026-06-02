import * as vscode from 'vscode';
import {
  LanguageClient,
  type LanguageClientOptions,
  type ServerOptions,
  State,
  TransportKind,
} from 'vscode-languageclient/node';

export type LspState = 'stopped' | 'starting' | 'running';

export class LspClient implements vscode.Disposable {
  private client: LanguageClient | null = null;
  private stateSub: vscode.Disposable | null = null;
  private readonly emitter = new vscode.EventEmitter<LspState>();

  readonly onStateChange: vscode.Event<LspState> = this.emitter.event;

  constructor(
    private readonly workspaceRoot: string,
    private readonly output: vscode.OutputChannel,
  ) {}

  async start(binaryPath: string): Promise<void> {
    if (this.client) return;

    const args = ['lsp', this.workspaceRoot];
    this.output.appendLine(`[lsp] spawning ${binaryPath} ${args.slice(1).join(' ')}`);
    const serverOptions: ServerOptions = {
      command: binaryPath,
      args,
      transport: TransportKind.stdio,
    };
    const clientOptions: LanguageClientOptions = {
      documentSelector: [
        { scheme: 'file', language: 'rust' },
        { scheme: 'file', language: 'typescript' },
        { scheme: 'file', language: 'typescriptreact' },
        { scheme: 'file', language: 'javascript' },
        { scheme: 'file', language: 'javascriptreact' },
        { scheme: 'file', language: 'lua' },
        { scheme: 'file', language: 'c' },
      ],
      outputChannel: this.output,
    };

    const client = new LanguageClient('standardoc', 'Standardoc', serverOptions, clientOptions);
    this.stateSub = client.onDidChangeState(event => this.emitter.fire(toLspState(event.newState)));
    this.client = client;
    await client.start();
  }

  async stop(): Promise<void> {
    const client = this.client;
    if (!client) return;
    this.client = null;
    this.stateSub?.dispose();
    this.stateSub = null;
    await client.stop();
  }

  dispose(): void {
    this.stateSub?.dispose();
    this.stateSub = null;
    if (this.client) {
      void this.client.stop().catch(() => undefined);
      this.client = null;
    }
    this.emitter.dispose();
  }
}

function toLspState(s: State): LspState {
  if (s === State.Running) return 'running';
  if (s === State.Starting) return 'starting';
  return 'stopped';
}

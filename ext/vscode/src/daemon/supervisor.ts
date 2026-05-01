import * as vscode from 'vscode';
import { matcher } from 'matchigo';

export type DaemonState =
  | { kind: 'stopped' }
  | { kind: 'starting' }
  | { kind: 'ready'; pid: number }
  | { kind: 'restarting'; attempt: number }
  | { kind: 'failed'; reason: string };

const describeState = matcher<DaemonState, string>()
  .with({ kind: 'stopped' }, () => 'Stopped')
  .with({ kind: 'starting' }, () => 'Starting')
  .with({ kind: 'ready' }, ({ pid }) => `Ready (pid ${pid})`)
  .with({ kind: 'restarting' }, ({ attempt }) => `Restarting (attempt ${attempt})`)
  .with({ kind: 'failed' }, ({ reason }) => `Failed: ${reason}`)
  .exhaustive();

export class DaemonSupervisor implements vscode.Disposable {
  private state: DaemonState = { kind: 'stopped' };
  private readonly emitter = new vscode.EventEmitter<DaemonState>();

  readonly onDidChangeState: vscode.Event<DaemonState> = this.emitter.event;

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly output: vscode.OutputChannel,
  ) {}

  current(): DaemonState {
    return this.state;
  }

  describe(): string {
    return describeState(this.state);
  }

  async spawn(): Promise<void> {
    throw new Error('TODO: Phase B — resolve binary, spawn daemon child, transition stopped → starting → ready');
  }

  async restart(): Promise<void> {
    throw new Error('TODO: Phase B — graceful stop + spawn with backoff');
  }

  async stop(): Promise<void> {
    throw new Error('TODO: Phase B — signal child, await exit, transition → stopped');
  }

  dispose(): void {
    this.emitter.dispose();
  }
}

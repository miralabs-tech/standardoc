import * as vscode from 'vscode';
import { resolveBinary } from './binary';
import {
  BACKOFF_MS,
  CRASH_WINDOW_MS,
  STABLE_UPTIME_MS,
  describeState,
  type DaemonState,
} from './supervisor-state';
import type { LspClient, LspState } from '../lsp/client';
import type { McpClient } from '../mcp/client';

export { describeState, type DaemonState } from './supervisor-state';

export interface SupervisorDeps {
  readonly lsp: LspClient;
  readonly mcp: McpClient;
}

export class DaemonSupervisor implements vscode.Disposable {
  private state: DaemonState = { kind: 'stopped' };
  private readonly emitter = new vscode.EventEmitter<DaemonState>();
  private crashTimestamps: number[] = [];
  private backoffTimer: ReturnType<typeof setTimeout> | null = null;
  private resetTimer: ReturnType<typeof setTimeout> | null = null;
  private expectedStopping = false;
  private lspStateSub: vscode.Disposable | null = null;

  readonly onDidChangeState: vscode.Event<DaemonState> = this.emitter.event;

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly output: vscode.OutputChannel,
    private readonly deps: SupervisorDeps,
  ) {
    this.lspStateSub = deps.lsp.onStateChange(state => this.onLspStateChange(state));
  }

  current(): DaemonState {
    return this.state;
  }

  describe(): string {
    return describeState(this.state);
  }

  async spawn(): Promise<void> {
    if (this.state.kind === 'starting' || this.state.kind === 'ready') return;
    this.cancelBackoff();
    this.setState({ kind: 'starting' });
    try {
      const binary = await resolveBinary(this.context);
      this.log(`resolved binary (source=${binary.source}, path=${binary.path})`);
      await this.startClientsParallel(binary.path);
      this.setState({ kind: 'ready', pid: 0 });
      this.scheduleResetCounter();
    } catch (e) {
      const reason = describeError(e);
      this.log(`spawn failed: ${reason}`);
      await this.safeStopAll();
      this.handleCrash(reason);
    }
  }

  private async startClientsParallel(binaryPath: string): Promise<void> {
    const [lspR, mcpR] = await Promise.allSettled([
      this.deps.lsp.start(binaryPath),
      this.deps.mcp.start(binaryPath),
    ]);

    if (lspR.status === 'fulfilled' && mcpR.status === 'fulfilled') return;

    if (lspR.status === 'fulfilled') await this.safeStopLsp();
    if (mcpR.status === 'fulfilled') await this.safeStopMcp();

    const failure = lspR.status === 'rejected' ? lspR.reason : (mcpR as PromiseRejectedResult).reason;
    const which = lspR.status === 'rejected' ? 'lsp' : 'mcp';
    throw new Error(`${which} start failed: ${describeError(failure)}`);
  }

  async restart(): Promise<void> {
    await this.stop();
    await this.spawn();
  }

  async stop(): Promise<void> {
    this.expectedStopping = true;
    this.cancelBackoff();
    this.cancelResetCounter();
    this.crashTimestamps = [];
    this.setState({ kind: 'stopped' });
    await this.safeStopAll();
    this.expectedStopping = false;
  }

  private onLspStateChange(s: LspState): void {
    if (s === 'stopped' && !this.expectedStopping && this.state.kind === 'ready') {
      this.handleCrash('LSP client stopped unexpectedly');
    }
  }

  private async safeStopAll(): Promise<void> {
    await Promise.allSettled([this.safeStopLsp(), this.safeStopMcp()]);
  }

  private async safeStopLsp(): Promise<void> {
    try {
      await this.deps.lsp.stop();
    } catch (e) {
      this.log(`lsp stop error: ${describeError(e)}`);
    }
  }

  private async safeStopMcp(): Promise<void> {
    try {
      await this.deps.mcp.stop();
    } catch (e) {
      this.log(`mcp stop error: ${describeError(e)}`);
    }
  }

  private handleCrash(reason: string): void {
    const now = Date.now();
    this.crashTimestamps = this.crashTimestamps.filter(t => now - t < CRASH_WINDOW_MS);
    this.crashTimestamps.push(now);

    if (this.crashTimestamps.length > BACKOFF_MS.length) {
      this.setState({ kind: 'failed', reason });
      return;
    }

    const attempt = this.crashTimestamps.length;
    const delay = BACKOFF_MS[attempt - 1] ?? 0;
    this.setState({ kind: 'restarting', attempt });
    this.log(`scheduling retry in ${delay}ms (attempt ${attempt}/${BACKOFF_MS.length})`);
    this.backoffTimer = setTimeout(() => {
      this.backoffTimer = null;
      void this.spawn();
    }, delay);
  }

  private scheduleResetCounter(): void {
    this.cancelResetCounter();
    this.resetTimer = setTimeout(() => {
      if (this.state.kind === 'ready') {
        this.crashTimestamps = [];
        this.log('crash counter reset after stable uptime');
      }
    }, STABLE_UPTIME_MS);
  }

  private cancelResetCounter(): void {
    if (this.resetTimer) {
      clearTimeout(this.resetTimer);
      this.resetTimer = null;
    }
  }

  private cancelBackoff(): void {
    if (this.backoffTimer) {
      clearTimeout(this.backoffTimer);
      this.backoffTimer = null;
    }
  }

  private setState(next: DaemonState): void {
    this.state = next;
    this.log(`state → ${describeState(next)}`);
    this.emitter.fire(next);
  }

  private log(message: string): void {
    this.output.appendLine(`[supervisor] ${message}`);
  }

  dispose(): void {
    this.cancelBackoff();
    this.cancelResetCounter();
    this.lspStateSub?.dispose();
    this.lspStateSub = null;
    this.emitter.dispose();
  }
}

function describeError(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}

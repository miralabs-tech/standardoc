import * as vscode from 'vscode';
import { BinaryNotFoundError, resolveBinary } from './binary';
import { describeFatalConfig, type FatalConfig } from './fatal-marker';
import { defaultExec, preflightSchemaVersion, type ExecFn } from './preflight';
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
  /**
   * Override the schema-version pre-flight invocation. Useful for tests; if
   * omitted, the supervisor shells out to `<binary> schema-version <root>`.
   */
  readonly preflightExec?: ExecFn;
}

export class DaemonSupervisor implements vscode.Disposable {
  private state: DaemonState = { kind: 'stopped' };
  private readonly emitter = new vscode.EventEmitter<DaemonState>();
  private crashTimestamps: number[] = [];
  private backoffTimer: ReturnType<typeof setTimeout> | null = null;
  private resetTimer: ReturnType<typeof setTimeout> | null = null;
  private expectedStopping = false;
  private lspStateSub: vscode.Disposable | null = null;
  private mcpFatalSub: vscode.Disposable | null = null;

  readonly onDidChangeState: vscode.Event<DaemonState> = this.emitter.event;

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly output: vscode.OutputChannel,
    private readonly deps: SupervisorDeps,
    private readonly workspaceRoot: string,
  ) {
    this.lspStateSub = deps.lsp.onStateChange(state => this.onLspStateChange(state));
    this.mcpFatalSub = deps.mcp.onFatalConfig(config => this.onFatalConfig(config));
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
      let binary;
      try {
        binary = await resolveBinary(this.context);
      } catch (e) {
        if (e instanceof BinaryNotFoundError) {
          this.log(`binary not found — entering awaiting_binary`);
          // No backoff/retry here: the missing binary won't appear by
          // itself. The installer command (triggered by toast or status
          // bar) is the only way out, and it re-calls spawn() on success.
          this.cancelBackoff();
          this.cancelResetCounter();
          this.crashTimestamps = [];
          this.setState({ kind: 'awaiting_binary' });
          return;
        }
        throw e;
      }
      this.log(`resolved binary (source=${binary.source}, path=${binary.path})`);
      const exec = this.deps.preflightExec ?? defaultExec;
      const preflight = await preflightSchemaVersion(binary.path, this.workspaceRoot, exec);
      if (!preflight.ok) {
        this.log(`pre-flight schema check failed: ${preflight.reason}`);
        // Mirror the in-process STDOC_FATAL signal: if the binary itself would
        // refuse the DB, surface it through the same `fatal_config` channel
        // so the existing toast/status-bar pipeline picks it up.
        const config: FatalConfig =
          typeof preflight.db === 'number' && typeof preflight.supported === 'number'
            ? { kind: 'schema_too_new', db: preflight.db, supported: preflight.supported }
            : { kind: 'unknown', code: 'preflight_failed', raw: preflight.reason };
        this.setState({ kind: 'fatal_config', config });
        return;
      }
      this.log(
        `pre-flight ok (db=${preflight.db ?? 'none'}, supported=${preflight.supported})`,
      );
      await this.startClientsParallel(binary.path);
      this.setState({ kind: 'ready' });
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

  /**
   * Stops the daemon then spawns it again. Concurrent calls are
   * serialised via an internal Promise chain : if a restart is already
   * in flight when a second call arrives, the second one queues behind
   * it. Each tail reads the LATEST `ragSettings` snapshot at spawn
   * time, so the daemon always boots with the most-recent settings
   * regardless of how many restart calls overlap.
   *
   * Rationale (T-C) : the `watchRagSettings` callback can fire twice in
   * rapid succession when the user toggles two settings near-
   * simultaneously (e.g. editing settings.json with both lines saved
   * at once, or QuickPick double-fire). Without sequencing,
   * `stop()` on the second call could race against the first call's
   * `spawn()` and leave clients dangling.
   */
  async restart(): Promise<void> {
    const next = this.restartChain.then(() => this.doRestart());
    // Swallow the rejection on the chain itself so subsequent restarts
    // stay sequential ; the returned promise still surfaces the error
    // to the immediate caller.
    this.restartChain = next.catch(() => {});
    return next;
  }

  private restartChain: Promise<void> = Promise.resolve();

  private async doRestart(): Promise<void> {
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

  /**
   * Drives the state machine into `fatal_config` and stops the
   * backoff/retry loop. Reaching `fatal_config` requires the user to
   * fix the host-side condition (typically: rebuild + re-install the
   * binary) — retrying without intervention would just re-fail.
   */
  private onFatalConfig(config: FatalConfig): void {
    this.log(`fatal config marker: ${describeFatalConfig(config)}`);
    this.cancelBackoff();
    this.cancelResetCounter();
    this.crashTimestamps = [];
    this.expectedStopping = true;
    this.setState({ kind: 'fatal_config', config });
    void this.safeStopAll().finally(() => {
      this.expectedStopping = false;
    });
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
    // If we already learned the failure is a fatal-config one (via
    // STDOC_FATAL on stderr) or that the binary is missing, keep that
    // state — `failed` / `restarting` would mask the actionable hint
    // and re-arm the retry machinery against an unfixable condition.
    if (this.state.kind === 'fatal_config' || this.state.kind === 'awaiting_binary') {
      this.log(`ignoring crash signal (${reason}) — already in ${this.state.kind}`);
      return;
    }

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
    this.mcpFatalSub?.dispose();
    this.mcpFatalSub = null;
    this.emitter.dispose();
  }
}

function describeError(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}

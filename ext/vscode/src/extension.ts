import * as vscode from 'vscode';
import { describeFatalConfig } from './daemon/fatal-marker';
import { readRagSettings, watchRagSettings } from './daemon/rag-settings';
import { DaemonSupervisor, type DaemonState } from './daemon/supervisor';
import { LspClient } from './lsp/client';
import { McpClient } from './mcp/client';
import { StandardocMcpServerProvider } from './mcp/serverDefinitionProvider';
import { StatusBarController } from './statusBar';
import { registerCommands } from './commands';
import { maybePromptForInit } from './init/prompt';

const MCP_PROVIDER_ID = 'standardoc.mcp';
const DEFAULT_MCP_HTTP_PORT = 7700;

export function activate(context: vscode.ExtensionContext): void {
  const folder = vscode.workspace.workspaceFolders?.[0];
  if (!folder) {
    return;
  }
  const workspaceRoot = folder.uri.fsPath;

  const output = vscode.window.createOutputChannel('Standardoc');
  context.subscriptions.push(output);

  const port = vscode.workspace
    .getConfiguration('standardoc')
    .get<number>('mcpHttpPort', DEFAULT_MCP_HTTP_PORT);
  output.appendLine(`[mcp] http port from setting: ${port}`);

  const lsp = new LspClient(workspaceRoot, output);
  const mcp = new McpClient(workspaceRoot, output, port);
  const initialRag = readRagSettings();
  mcp.setRagSettings(initialRag);
  output.appendLine(
    `[mcp] rag settings: enabled=${initialRag.enabled} embedder=${initialRag.embedder}`,
  );
  const supervisor = new DaemonSupervisor(context, output, { lsp, mcp }, workspaceRoot);

  context.subscriptions.push(lsp, mcp, supervisor);

  const statusBar = new StatusBarController();
  context.subscriptions.push(statusBar);
  statusBar.update(supervisor.current(), mcp.ragSettingsSnapshot());
  context.subscriptions.push(
    supervisor.onDidChangeState(state => statusBar.update(state, mcp.ragSettingsSnapshot())),
  );

  // Auto-restart the daemon when RAG settings change so the supervisor
  // picks up new spawn flags on the next child process. Debounce-free :
  // each setting change is a single configuration event from VSCode.
  context.subscriptions.push(
    watchRagSettings(next => {
      output.appendLine(
        `[mcp] rag settings changed: enabled=${next.enabled} embedder=${next.embedder} — restarting daemon`,
      );
      mcp.setRagSettings(next);
      statusBar.update(supervisor.current(), next);
      // The daemon retries SQLite open on transient lock-release races
      // (~1.5 s exponential backoff), so a vanilla `restart()` is enough
      // — no extra wait needed on Windows.
      void supervisor.restart().catch(e => {
        output.appendLine(
          `[mcp] rag-driven restart failed: ${e instanceof Error ? e.message : String(e)}`,
        );
      });
    }),
  );

  // Transition-aware toast: surface actionable hints when the daemon
  // moves INTO `fatal_config` or `failed`. Permanent state lives on the
  // status bar; toasts are reserved for the moment of degradation.
  let lastNotifiedKind: DaemonState['kind'] | null = supervisor.current().kind;
  context.subscriptions.push(
    supervisor.onDidChangeState(state => {
      if (state.kind === lastNotifiedKind) return;
      lastNotifiedKind = state.kind;
      if (state.kind === 'fatal_config') {
        void notifyFatalConfig(supervisor, output, state);
      } else if (state.kind === 'failed') {
        void notifyFailed(supervisor, output, state);
      }
    }),
  );

  const spawnSupervisor = (): void => {
    void supervisor.spawn().catch(e => {
      output.appendLine(`activate: spawn failed: ${e instanceof Error ? e.message : String(e)}`);
    });
  };

  registerCommands(context, {
    context,
    supervisor,
    lsp,
    mcp,
    output,
    workspaceRoot,
    spawnSupervisor,
  });

  registerMcpServerProvider(context, () => mcp.url(), output, supervisor);

  output.appendLine('Standardoc extension activated.');

  void maybePromptForInit({
    context,
    workspaceRoot,
    output,
    onOptedIn: spawnSupervisor,
  });
}

function registerMcpServerProvider(
  context: vscode.ExtensionContext,
  endpointResolver: () => string | null,
  output: vscode.OutputChannel,
  supervisor: DaemonSupervisor,
): void {
  const lm = vscode.lm as Partial<typeof vscode.lm>;
  if (typeof lm.registerMcpServerDefinitionProvider !== 'function') {
    output.appendLine(
      '[mcp-provider] vscode.lm.registerMcpServerDefinitionProvider unavailable — skipping',
    );
    return;
  }
  const provider = new StandardocMcpServerProvider(endpointResolver, output);
  context.subscriptions.push(provider);
  context.subscriptions.push(
    vscode.lm.registerMcpServerDefinitionProvider(MCP_PROVIDER_ID, provider),
  );
  // Re-emit the definition list whenever the supervisor moves to `ready`:
  // that is when the MCP daemon has bound its port and the endpoint URL
  // is finally resolvable. Without this refresh consumers would see an
  // empty definition list at activation and never re-poll.
  context.subscriptions.push(
    supervisor.onDidChangeState(state => {
      if (state.kind === 'ready') {
        provider.refresh();
      }
    }),
  );
  output.appendLine(`[mcp-provider] registered (${MCP_PROVIDER_ID})`);
}

async function notifyFatalConfig(
  supervisor: DaemonSupervisor,
  output: vscode.OutputChannel,
  state: Extract<DaemonState, { kind: 'fatal_config' }>,
): Promise<void> {
  const message = describeFatalConfig(state.config);
  const choice = await vscode.window.showErrorMessage(
    `Standardoc daemon halted — ${message}`,
    'Show logs',
    'Try restart anyway',
  );
  if (choice === 'Show logs') {
    output.show(true);
  } else if (choice === 'Try restart anyway') {
    await supervisor.restart();
  }
}

async function notifyFailed(
  supervisor: DaemonSupervisor,
  output: vscode.OutputChannel,
  state: Extract<DaemonState, { kind: 'failed' }>,
): Promise<void> {
  const choice = await vscode.window.showErrorMessage(
    `Standardoc daemon failed: ${state.reason}`,
    'Restart',
    'Show logs',
  );
  if (choice === 'Restart') {
    await supervisor.restart();
  } else if (choice === 'Show logs') {
    output.show(true);
  }
}

export function deactivate(): void {
  // Disposables managed via context.subscriptions.
}

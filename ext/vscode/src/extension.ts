import * as vscode from 'vscode';
import { describeFatalConfig } from './daemon/fatal-marker';
import { DaemonSupervisor, type DaemonState } from './daemon/supervisor';
import { LspClient } from './lsp/client';
import { McpClient } from './mcp/client';
import { StandardocMcpServerProvider } from './mcp/serverDefinitionProvider';
import { StatusBarController } from './statusBar';
import { registerCommands } from './commands';
import { maybePromptForInit } from './init/prompt';

const MCP_PROVIDER_ID = 'standardoc.mcp';

export function activate(context: vscode.ExtensionContext): void {
  const folder = vscode.workspace.workspaceFolders?.[0];
  if (!folder) {
    return;
  }
  const workspaceRoot = folder.uri.fsPath;

  const output = vscode.window.createOutputChannel('Standardoc');
  context.subscriptions.push(output);

  const lsp = new LspClient(workspaceRoot, output);
  const mcp = new McpClient(workspaceRoot, output);
  const supervisor = new DaemonSupervisor(context, output, { lsp, mcp });

  context.subscriptions.push(lsp, mcp, supervisor);

  const statusBar = new StatusBarController();
  context.subscriptions.push(statusBar);
  statusBar.update(supervisor.current());
  context.subscriptions.push(supervisor.onDidChangeState(state => statusBar.update(state)));

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

  registerMcpServerProvider(context, workspaceRoot, output);

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
  workspaceRoot: string,
  output: vscode.OutputChannel,
): void {
  const lm = vscode.lm as Partial<typeof vscode.lm>;
  if (typeof lm.registerMcpServerDefinitionProvider !== 'function') {
    output.appendLine(
      '[mcp-provider] vscode.lm.registerMcpServerDefinitionProvider unavailable — skipping',
    );
    return;
  }
  const provider = new StandardocMcpServerProvider(context, workspaceRoot, output);
  context.subscriptions.push(provider);
  context.subscriptions.push(
    vscode.lm.registerMcpServerDefinitionProvider(MCP_PROVIDER_ID, provider),
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

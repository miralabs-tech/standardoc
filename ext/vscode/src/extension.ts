import * as vscode from 'vscode';
import { describeFatalConfig } from './daemon/fatal-marker';
import { DaemonSupervisor, type DaemonState } from './daemon/supervisor';
import { LspClient } from './lsp/client';
import { McpClient } from './mcp/client';
import { StandardocMcpServerProvider } from './mcp/serverDefinitionProvider';
import { StatusBarController } from './statusBar';
import { registerCommands } from './commands';
import { maybePromptForInit, syncMcpConfigToUrl } from './init/prompt';
import { registerStdignoreHover } from './stdignore/hover';

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
  const supervisor = new DaemonSupervisor(context, output, { lsp, mcp }, workspaceRoot);

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
      } else if (state.kind === 'awaiting_binary') {
        void notifyAwaitingBinary(output);
      } else if (state.kind === 'ready') {
        // Sync `.mcp.json` to the daemon's actual endpoint. When the
        // configured port is already bound (e.g. a sibling VSCode window
        // running standardoc), the daemon falls back to an ephemeral
        // port and writes the real URL to `.standardoc/mcp.endpoint` —
        // external consumers (claude-code CLI, Copilot Chat, ...) must
        // see that URL in `.mcp.json` or they'd hit the dead/wrong port.
        const actualUrl = mcp.url();
        if (actualUrl !== null) {
          void syncMcpConfigToUrl(workspaceRoot, output, actualUrl);
        }
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

  registerStdignoreHover(context, workspaceRoot, output);

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

async function notifyAwaitingBinary(output: vscode.OutputChannel): Promise<void> {
  const choice = await vscode.window.showInformationMessage(
    'Standardoc needs to download the native binary for this platform.',
    { modal: false },
    'Download',
    'Later',
    'Show logs',
  );
  if (choice === 'Download') {
    await vscode.commands.executeCommand('Standardoc.downloadBinary');
  } else if (choice === 'Show logs') {
    output.show(true);
  }
  // 'Later' / dismiss: the status bar keeps a one-click affordance,
  // so we do not pester the user again until the state transitions
  // back into `awaiting_binary` (e.g. after a manual restart).
}

export function deactivate(): void {
  // Disposables managed via context.subscriptions.
}

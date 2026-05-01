import * as vscode from 'vscode';
import { DaemonSupervisor } from './daemon/supervisor';
import { LspClient } from './lsp/client';
import { McpClient } from './mcp/client';
import { StatusBarController } from './statusBar';
import { registerCommands } from './commands';

export function activate(context: vscode.ExtensionContext): void {
  const folder = vscode.workspace.workspaceFolders?.[0];
  if (!folder) {
    return;
  }
  const workspaceRoot = folder.uri.fsPath;

  const output = vscode.window.createOutputChannel('Standardoc');
  context.subscriptions.push(output);

  const supervisor = new DaemonSupervisor(context, output);
  const lsp = new LspClient(context, workspaceRoot, output);
  const mcp = new McpClient(context, workspaceRoot, output);

  context.subscriptions.push(supervisor, lsp, mcp);

  const statusBar = new StatusBarController();
  context.subscriptions.push(statusBar);
  statusBar.update(supervisor.current());
  context.subscriptions.push(supervisor.onDidChangeState(state => statusBar.update(state)));

  registerCommands(context, { supervisor, lsp, mcp });

  output.appendLine('Standardoc extension activated (scaffold mode — Phase B not implemented).');
}

export function deactivate(): void {
  // Disposables are managed via context.subscriptions.
}

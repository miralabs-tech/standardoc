import * as vscode from 'vscode';
import type { DaemonSupervisor } from './daemon/supervisor';
import type { LspClient } from './lsp/client';
import type { McpClient } from './mcp/client';

export interface CommandContext {
  readonly supervisor: DaemonSupervisor;
  readonly lsp: LspClient;
  readonly mcp: McpClient;
}

export function registerCommands(context: vscode.ExtensionContext, ctx: CommandContext): void {
  context.subscriptions.push(
    vscode.commands.registerCommand('Standardoc.daemon.restart', () => commandDaemonRestart(ctx)),
    vscode.commands.registerCommand('Standardoc.findSymbol', () => commandFindSymbol(ctx)),
    vscode.commands.registerCommand('Standardoc.getContext', () => commandGetContext(ctx)),
  );
}

async function commandDaemonRestart(_ctx: CommandContext): Promise<void> {
  throw new Error('TODO: Phase B — Standardoc.daemon.restart');
}

async function commandFindSymbol(_ctx: CommandContext): Promise<void> {
  throw new Error('TODO: Phase B — prompt query, call mcp.findSymbol, render quick-pick');
}

async function commandGetContext(_ctx: CommandContext): Promise<void> {
  throw new Error('TODO: Phase B — derive fqdn from active editor selection, call mcp.getContext, show result');
}

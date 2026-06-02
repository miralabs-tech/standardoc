import * as vscode from 'vscode';
import * as path from 'node:path';
import { spawn } from 'node:child_process';
import type { DaemonSupervisor } from './daemon/supervisor';
import type { LspClient } from './lsp/client';
import type { McpClient } from './mcp/client';
import type { RawSymbolJson, SymbolContextWithNeighborsJson } from './mcp/types';
import { formatSymbolContext, parseToolResult, pickTopFqdn } from './commands-render';
import {
  clearGlobalInitState,
  initializeWorkspace,
  regenerateSkill,
  writeMcpConfig,
} from './init/prompt';
import { resolveBinary } from './daemon/binary';
import { installBinary, InstallError, UnsupportedPlatformError } from './daemon/binary-installer';
import { openGraphViz } from './viz/open';

export interface CommandContext {
  readonly context: vscode.ExtensionContext;
  readonly supervisor: DaemonSupervisor;
  readonly lsp: LspClient;
  readonly mcp: McpClient;
  readonly output: vscode.OutputChannel;
  readonly workspaceRoot: string;
  readonly spawnSupervisor: () => void;
}

export function registerCommands(context: vscode.ExtensionContext, ctx: CommandContext): void {
  context.subscriptions.push(
    vscode.commands.registerCommand('Standardoc.daemon.restart', () => commandDaemonRestart(ctx)),
    vscode.commands.registerCommand('Standardoc.findSymbol', () => commandFindSymbol(ctx)),
    vscode.commands.registerCommand('Standardoc.getContext', () => commandGetContext(ctx)),
    vscode.commands.registerCommand('Standardoc.initWorkspace', () => commandInitWorkspace(ctx)),
    vscode.commands.registerCommand('Standardoc.refreshMcpConfig', () =>
      commandRefreshMcpConfig(ctx),
    ),
    vscode.commands.registerCommand('Standardoc.resetGlobalInitPrompt', () =>
      commandResetGlobalInitPrompt(ctx),
    ),
    vscode.commands.registerCommand('Standardoc.regenerateSkill', () =>
      commandRegenerateSkill(ctx),
    ),
    vscode.commands.registerCommand('Standardoc.daemon.stop', () => commandDaemonStop(ctx)),
    vscode.commands.registerCommand('Standardoc.daemon.start', () => commandDaemonStart(ctx)),
    vscode.commands.registerCommand('Standardoc.purgeExcluded', () => commandPurgeExcluded(ctx)),
    vscode.commands.registerCommand('Standardoc.statusBarMenu', () => commandStatusBarMenu(ctx)),
    vscode.commands.registerCommand('Standardoc.downloadBinary', () => commandDownloadBinary(ctx)),
    vscode.commands.registerCommand('Standardoc.viz.open', () =>
      openGraphViz(ctx.context, ctx.mcp, ctx.output),
    ),
  );
}

async function commandDaemonStop(ctx: CommandContext): Promise<void> {
  try {
    await ctx.supervisor.stop();
    void vscode.window.showInformationMessage('Standardoc daemon stopped');
  } catch (e) {
    void vscode.window.showErrorMessage(`Daemon stop failed: ${describeError(e)}`);
  }
}

async function commandDaemonStart(ctx: CommandContext): Promise<void> {
  const current = ctx.supervisor.current().kind;
  if (current === 'ready' || current === 'starting') {
    void vscode.window.showInformationMessage(`Standardoc daemon already ${current}`);
    return;
  }
  try {
    await ctx.supervisor.spawn();
    void vscode.window.showInformationMessage('Standardoc daemon started');
  } catch (e) {
    void vscode.window.showErrorMessage(`Daemon start failed: ${describeError(e)}`);
  }
}

async function commandPurgeExcluded(ctx: CommandContext): Promise<void> {
  const confirm = await vscode.window.showWarningMessage(
    'Purge symbols matching .stdignore from the index?\n\n' +
      'This will stop the Standardoc daemon, run `standardoc purge-excluded`, then restart.',
    { modal: true },
    'Purge',
  );
  if (confirm !== 'Purge') return;

  let binaryPath: string;
  try {
    const resolved = await resolveBinary(ctx.context);
    binaryPath = resolved.path;
  } catch (e) {
    void vscode.window.showErrorMessage(`Cannot resolve standardoc binary: ${describeError(e)}`);
    return;
  }

  ctx.output.show(true);
  ctx.output.appendLine('[purge] stopping daemon to release workspace lock…');
  await ctx.supervisor.stop();

  const exitCode = await runChildToOutput(binaryPath, ['purge-excluded', ctx.workspaceRoot, '--yes'], ctx.output);

  if (exitCode === 0) {
    ctx.output.appendLine('[purge] complete, restarting daemon…');
    void vscode.window.showInformationMessage('Standardoc: purge complete, daemon restarting');
  } else {
    ctx.output.appendLine(`[purge] failed with exit code ${exitCode}, restarting daemon anyway…`);
    void vscode.window.showErrorMessage(`Purge failed (exit ${exitCode}). See Standardoc output for details.`);
  }

  ctx.spawnSupervisor();
}

function runChildToOutput(
  binaryPath: string,
  args: ReadonlyArray<string>,
  output: vscode.OutputChannel,
): Promise<number> {
  return new Promise(resolve => {
    const child = spawn(binaryPath, [...args], { stdio: ['ignore', 'pipe', 'pipe'] });
    child.stdout?.on('data', chunk => output.append(ensureTrailingNewline(chunk.toString())));
    child.stderr?.on('data', chunk => output.append(ensureTrailingNewline(chunk.toString())));
    child.on('error', err => {
      output.appendLine(`[purge] child spawn error: ${err.message}`);
      resolve(-1);
    });
    child.on('close', code => resolve(code ?? -1));
  });
}

function ensureTrailingNewline(s: string): string {
  return s.endsWith('\n') ? s : s + '\n';
}

async function commandInitWorkspace(ctx: CommandContext): Promise<void> {
  try {
    await initializeWorkspace({
      context: ctx.context,
      workspaceRoot: ctx.workspaceRoot,
      output: ctx.output,
      onOptedIn: ctx.spawnSupervisor,
    });
  } catch (e) {
    void vscode.window.showErrorMessage(`Initialize failed: ${describeError(e)}`);
  }
}

async function commandRefreshMcpConfig(ctx: CommandContext): Promise<void> {
  try {
    await writeMcpConfig({
      context: ctx.context,
      workspaceRoot: ctx.workspaceRoot,
      output: ctx.output,
      onOptedIn: ctx.spawnSupervisor,
    });
  } catch (e) {
    void vscode.window.showErrorMessage(`Refresh .mcp.json failed: ${describeError(e)}`);
  }
}

async function commandResetGlobalInitPrompt(ctx: CommandContext): Promise<void> {
  await clearGlobalInitState(ctx.context);
  void vscode.window.showInformationMessage(
    'Standardoc: global init prompt reset. The init prompt will appear again on workspaces with code.',
  );
}

async function commandRegenerateSkill(ctx: CommandContext): Promise<void> {
  try {
    await regenerateSkill({
      context: ctx.context,
      workspaceRoot: ctx.workspaceRoot,
      output: ctx.output,
      onOptedIn: ctx.spawnSupervisor,
    });
  } catch (e) {
    void vscode.window.showErrorMessage(`Regenerate AI agent skill failed: ${describeError(e)}`);
  }
}

interface StatusBarMenuItem extends vscode.QuickPickItem {
  readonly commandId: string;
}

async function commandDownloadBinary(ctx: CommandContext): Promise<void> {
  try {
    const installed = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: 'Standardoc: downloading binary…',
        cancellable: false,
      },
      async progress => {
        progress.report({ message: 'fetching manifest' });
        return installBinary({
          globalStorageDir: ctx.context.globalStorageUri.fsPath,
          log: line => ctx.output.appendLine(`[install] ${line}`),
        });
      },
    );
    ctx.output.appendLine(
      `[install] installed core=${installed.core_version} protocol=${installed.protocol_version} → ${installed.path}`,
    );
    void vscode.window.showInformationMessage(
      `Standardoc binary installed (core ${installed.core_version}, protocol ${installed.protocol_version}).`,
    );
    ctx.spawnSupervisor();
  } catch (e) {
    const reason =
      e instanceof UnsupportedPlatformError
        ? e.message
        : e instanceof InstallError
          ? e.reason
          : describeError(e);
    ctx.output.appendLine(`[install] failed: ${reason}`);
    void vscode.window.showErrorMessage(`Standardoc download failed — ${reason}`);
  }
}

async function commandStatusBarMenu(_ctx: CommandContext): Promise<void> {
  const items: StatusBarMenuItem[] = [
    { label: '$(play) Start daemon', commandId: 'Standardoc.daemon.start' },
    { label: '$(debug-stop) Stop daemon', commandId: 'Standardoc.daemon.stop' },
    { label: '$(refresh) Restart daemon', commandId: 'Standardoc.daemon.restart' },
    { label: '$(trash) Purge excluded paths', commandId: 'Standardoc.purgeExcluded' },
  ];
  const picked = await vscode.window.showQuickPick(items, {
    placeHolder: 'Standardoc — choose an action',
  });
  if (!picked) return;
  await vscode.commands.executeCommand(picked.commandId);
}

async function commandDaemonRestart(ctx: CommandContext): Promise<void> {
  try {
    await ctx.supervisor.restart();
    void vscode.window.showInformationMessage('Standardoc daemon restarted');
  } catch (e) {
    void vscode.window.showErrorMessage(`Daemon restart failed: ${describeError(e)}`);
  }
}

interface FindSymbolItem extends vscode.QuickPickItem {
  readonly symbol: RawSymbolJson;
}

async function commandFindSymbol(ctx: CommandContext): Promise<void> {
  const query = await vscode.window.showInputBox({
    prompt: 'Search Standardoc symbols by name or FQDN',
    placeHolder: 'e.g. parse_workspace',
  });
  if (!query || !query.trim()) return;

  const raw = await safeFindSymbol(ctx, query, 50);
  if (raw === null) return;

  const parsed = parseToolResult<RawSymbolJson[]>(raw);
  if (parsed.kind === 'indexing') {
    void vscode.window.showInformationMessage(parsed.message);
    return;
  }
  if (parsed.value.length === 0) {
    void vscode.window.showInformationMessage(`No symbols match "${query}"`);
    return;
  }

  const items: FindSymbolItem[] = parsed.value.map(s => ({
    label: s.fqdn,
    description: s.kind,
    detail: `${s.location.file}:${s.location.start_line}`,
    symbol: s,
  }));
  const picked = await vscode.window.showQuickPick(items, {
    placeHolder: `${parsed.value.length} matches — pick a symbol to open`,
    matchOnDescription: true,
    matchOnDetail: true,
  });
  if (!picked) return;

  await openSymbolLocation(ctx.workspaceRoot, picked.symbol);
}

async function commandGetContext(ctx: CommandContext): Promise<void> {
  const editor = vscode.window.activeTextEditor;
  if (!editor) {
    void vscode.window.showInformationMessage('No active editor — open a source file first');
    return;
  }
  const range = editor.document.getWordRangeAtPosition(editor.selection.active);
  if (!range) {
    void vscode.window.showInformationMessage('Place the cursor on a symbol name');
    return;
  }
  const word = editor.document.getText(range);

  const findRaw = await safeFindSymbol(ctx, word, 5);
  if (findRaw === null) return;

  const found = parseToolResult<RawSymbolJson[]>(findRaw);
  if (found.kind === 'indexing') {
    void vscode.window.showInformationMessage(found.message);
    return;
  }
  const fqdn = pickTopFqdn(found.value);
  if (!fqdn) {
    void vscode.window.showInformationMessage(`No symbol matches "${word}"`);
    return;
  }

  let ctxRaw: string;
  try {
    ctxRaw = await ctx.mcp.getContext(fqdn, 1);
  } catch (e) {
    void vscode.window.showErrorMessage(`Get context failed: ${describeError(e)}`);
    return;
  }

  const ctxParsed = parseToolResult<SymbolContextWithNeighborsJson>(ctxRaw);
  if (ctxParsed.kind === 'indexing') {
    void vscode.window.showInformationMessage(ctxParsed.message);
    return;
  }

  ctx.output.appendLine('');
  ctx.output.appendLine(formatSymbolContext(ctxParsed.value));
  ctx.output.show(true);
}

async function safeFindSymbol(
  ctx: CommandContext,
  query: string,
  limit: number,
): Promise<string | null> {
  try {
    return await ctx.mcp.findSymbol(query, limit);
  } catch (e) {
    void vscode.window.showErrorMessage(`Find symbol failed: ${describeError(e)}`);
    return null;
  }
}

async function openSymbolLocation(workspaceRoot: string, symbol: RawSymbolJson): Promise<void> {
  const absolute = path.resolve(workspaceRoot, symbol.location.file);
  const uri = vscode.Uri.file(absolute);
  const doc = await vscode.workspace.openTextDocument(uri);
  const editor = await vscode.window.showTextDocument(doc);
  const pos = new vscode.Position(
    Math.max(0, symbol.location.start_line - 1),
    symbol.location.start_col,
  );
  editor.selection = new vscode.Selection(pos, pos);
  editor.revealRange(new vscode.Range(pos, pos), vscode.TextEditorRevealType.InCenter);
}

function describeError(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

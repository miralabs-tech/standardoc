import * as vscode from 'vscode';
import * as fs from 'node:fs';
import * as net from 'node:net';
import * as path from 'node:path';

const VIZ_HOST = 'localhost';
const VIZ_PORT = 3000;
const VIZ_URL = `http://${VIZ_HOST}:${VIZ_PORT}/shell.html`;
const PROBE_TIMEOUT_MS = 500;
const SERVER_BOOT_GRACE_MS = 2500;
const TERMINAL_NAME = 'Standardoc Viz';

/**
 * Launch (or attach to) the standardoc-graph-viz playground from the
 * extension. Assumes the user is editing the standardoc repo itself —
 * the playground lives at `crates/standardoc-graph-viz/playground/`
 * and isn't shipped with the extension.
 *
 * Flow:
 *   1. Resolve `<workspaceRoot>/crates/standardoc-graph-viz/playground/dev.ps1`.
 *      Bail with an actionable toast if absent (user is in some other repo).
 *   2. TCP-probe port 3000. If something already answers, skip the spawn
 *      and just open the browser — the dev server is presumably alive
 *      from a previous launch.
 *   3. Otherwise spawn `pwsh <dev.ps1>` in a dedicated VSCode terminal
 *      named "Standardoc Viz" so the user can watch boot output / Ctrl-C
 *      cleanly. Wait briefly for the server to come up, then open the
 *      browser.
 */
export async function openGraphViz(
  workspaceRoot: string,
  output: vscode.OutputChannel,
): Promise<void> {
  const playgroundDir = path.join(
    workspaceRoot,
    'crates',
    'standardoc-graph-viz',
    'playground',
  );
  const devScript = path.join(playgroundDir, 'dev.ps1');

  if (!fs.existsSync(devScript)) {
    void vscode.window.showErrorMessage(
      'Graph viz playground not found in this workspace. ' +
        'This command is available when editing the standardoc repository itself.',
    );
    output.appendLine(`[viz] dev.ps1 not found at ${devScript}`);
    return;
  }

  const alreadyUp = await isPortOpen(VIZ_HOST, VIZ_PORT, PROBE_TIMEOUT_MS);
  if (alreadyUp) {
    output.appendLine(`[viz] dev server already listening on :${VIZ_PORT} — opening browser`);
    await vscode.env.openExternal(vscode.Uri.parse(VIZ_URL));
    return;
  }

  output.appendLine(`[viz] spawning dev.ps1 in terminal "${TERMINAL_NAME}"`);
  const terminal = reuseOrCreateTerminal(TERMINAL_NAME, playgroundDir);
  terminal.show(true);
  // PowerShell call operator (&) so the path quoting handles whitespace
  // — Send-Text is a raw string, not a shell-parsed command.
  terminal.sendText(`& '${devScript}'`, true);

  // Give the server a moment to bind before we hand the URL to the
  // browser. SERVER_BOOT_GRACE_MS is enough for the bun dev server's
  // initial bind; the wasm-pack initial build can take longer but the
  // browser hits an empty page until the wasm is ready anyway — that
  // path stays user-visible.
  await delay(SERVER_BOOT_GRACE_MS);
  await vscode.env.openExternal(vscode.Uri.parse(VIZ_URL));
  output.appendLine(`[viz] browser opened at ${VIZ_URL}`);
}

function reuseOrCreateTerminal(name: string, cwd: string): vscode.Terminal {
  const existing = vscode.window.terminals.find(t => t.name === name);
  if (existing) {
    existing.dispose();
  }
  return vscode.window.createTerminal({ name, cwd });
}

function isPortOpen(host: string, port: number, timeoutMs: number): Promise<boolean> {
  return new Promise(resolve => {
    const socket = new net.Socket();
    let settled = false;
    const settle = (open: boolean): void => {
      if (settled) return;
      settled = true;
      socket.destroy();
      resolve(open);
    };
    socket.setTimeout(timeoutMs);
    socket.once('connect', () => settle(true));
    socket.once('timeout', () => settle(false));
    socket.once('error', () => settle(false));
    socket.connect(port, host);
  });
}

function delay(ms: number): Promise<void> {
  return new Promise(resolve => setTimeout(resolve, ms));
}

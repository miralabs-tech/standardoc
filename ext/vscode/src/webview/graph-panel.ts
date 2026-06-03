import * as vscode from 'vscode';
import { McpClient } from '../mcp/client';
import { WebviewMcpRelay } from './relay';

// Hosts the graph-viz shell in a WebviewPanel. The shell bundle
// (`dist/webview/shell.js`) runs the real `mountShell` from the viz
// lib; this host only builds the HTML shell, wires the MCP frame relay
// to the live daemon, and manages the panel lifecycle. Singleton —
// re-invoking the command reveals the existing panel.

export class GraphPanel {
  static current: GraphPanel | undefined;
  static readonly viewType = 'standardoc.graphViz';

  private readonly panel: vscode.WebviewPanel;
  private readonly disposables: vscode.Disposable[] = [];

  static createOrShow(
    context: vscode.ExtensionContext,
    mcp: McpClient,
    output: vscode.OutputChannel,
  ): void {
    const endpoint = mcp.url();
    if (endpoint === null) {
      void vscode.window.showWarningMessage(
        'Standardoc graph: the daemon is not ready yet. Try again once indexing has started.',
      );
      output.appendLine('[viz] open requested but mcp.url() is null — daemon not ready');
      return;
    }

    const column = vscode.window.activeTextEditor?.viewColumn ?? vscode.ViewColumn.One;
    if (GraphPanel.current !== undefined) {
      GraphPanel.current.panel.reveal(column);
      return;
    }

    const webviewRoot = vscode.Uri.joinPath(context.extensionUri, 'dist', 'webview');
    const panel = vscode.window.createWebviewPanel(
      GraphPanel.viewType,
      'Standardoc Graph',
      column,
      {
        enableScripts: true,
        retainContextWhenHidden: true,
        localResourceRoots: [webviewRoot],
      },
    );

    GraphPanel.current = new GraphPanel(panel, webviewRoot, endpoint, output);
  }

  private constructor(
    panel: vscode.WebviewPanel,
    webviewRoot: vscode.Uri,
    endpoint: string,
    output: vscode.OutputChannel,
  ) {
    this.panel = panel;

    // Wire the relay BEFORE setting html so no early webview frame is
    // dropped (the relay registers its onDidReceiveMessage listener in
    // its constructor).
    const relay = new WebviewMcpRelay(panel.webview, endpoint, output);
    this.disposables.push(relay);

    panel.webview.html = this.buildHtml(panel.webview, webviewRoot);

    panel.onDidDispose(() => this.dispose(), null, this.disposables);
  }

  private buildHtml(webview: vscode.Webview, root: vscode.Uri): string {
    const scriptUri = webview.asWebviewUri(vscode.Uri.joinPath(root, 'shell.js'));
    const wasmUri = webview.asWebviewUri(
      vscode.Uri.joinPath(root, 'standardoc_graph_viz_bg.wasm'),
    );
    const nonce = makeNonce();
    const csp = [
      `default-src 'none'`,
      `style-src ${webview.cspSource} 'unsafe-inline'`,
      `script-src 'nonce-${nonce}' 'wasm-unsafe-eval'`,
      // The wasm-bindgen loader fetches the `.wasm` binary as a webview
      // resource; all MCP traffic is postMessage, so no other origins.
      // Component styles inject as runtime <style> (style-src unsafe-inline).
      `connect-src ${webview.cspSource}`,
      `img-src ${webview.cspSource} data: blob:`,
      `font-src ${webview.cspSource}`,
    ].join('; ');

    return `<!doctype html>
<html lang="en">
<head>
<meta charset="UTF-8" />
<meta name="viewport" content="width=device-width, initial-scale=1.0" />
<meta http-equiv="Content-Security-Policy" content="${csp}" />
<title>Standardoc Graph</title>
<style>html, body { margin: 0; padding: 0; width: 100%; height: 100vh; overflow: hidden; }</style>
</head>
<body>
<div id="app"></div>
<script nonce="${nonce}">window.__WASM_URI__ = ${JSON.stringify(wasmUri.toString())};</script>
<script type="module" nonce="${nonce}" src="${scriptUri}"></script>
</body>
</html>`;
  }

  private dispose(): void {
    GraphPanel.current = undefined;
    this.panel.dispose();
    for (const d of this.disposables) d.dispose();
    this.disposables.length = 0;
  }
}

function makeNonce(): string {
  const chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';
  let out = '';
  for (let i = 0; i < 32; i++) out += chars.charAt(Math.floor(Math.random() * chars.length));
  return out;
}

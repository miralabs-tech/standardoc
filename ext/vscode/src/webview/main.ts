// Webview entry for the graph-viz shell. Bundled (browser target) to
// `dist/webview/shell.js` and loaded by GraphPanel's HTML. Supplies the
// two host-owned dependencies to the lib's `mountShell`:
//   - the WASM bindings (init + canvas ctors) from the in-repo `pkg/`;
//   - a postMessage-backed MCP transport tunnelling to the ext host.
//
// `window.__WASM_URI__` is injected by the host as an `asWebviewUri` to
// the copied `standardoc_graph_viz_bg.wasm` (wasm-bindgen --target web
// fetches it by URL).

import init, { FocusGraphCanvas, OverviewCanvas } from '@viz-pkg';
import { mountShell } from '@standarx/standardoc-viz/shell';
import { WebviewClientTransport } from './transport';

declare global {
  interface Window {
    __WASM_URI__: string;
  }
}

const app = document.getElementById('app');
if (app === null) throw new Error('webview: #app missing');

mountShell(app, {
  transport: new WebviewClientTransport(),
  wasm: { init, OverviewCanvas, FocusGraphCanvas },
  wasmUrl: window.__WASM_URI__,
  clientInfo: { name: 'standardoc-vscode-webview', version: '0.0.1' },
}).catch((e: unknown) => {
  const msg = e instanceof Error ? e.message : String(e);
  const status = app.querySelector('[data-shell-status]');
  if (status !== null) status.textContent = `fatal: ${msg}`;
  console.error('[webview] mount failed', e);
});

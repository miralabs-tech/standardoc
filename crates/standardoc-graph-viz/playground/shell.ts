/**
 * Playground shell entry — thin host around the lib's `mountShell`.
 *
 * Everything visual + behavioural lives in `@standarx/standardoc-viz/shell`
 * now (single source of truth shared with the VSCode webview). The
 * playground only supplies the two things a host owns:
 *   - the MCP transport — here a StreamableHTTP transport pointed at
 *     the dev server's `/mcp` proxy (server.ts forwards to the daemon);
 *   - the WASM module — the wasm-bindgen bindings from `../pkg/` plus
 *     the URL its `.wasm` binary is fetched from.
 *
 *   /        → 302 → /shell.html (legacy single-canvas entry retired)
 *   /shell.html → multi-panel shell (this file)
 */

import init, { FocusGraphCanvas, OverviewCanvas } from '../pkg/standardoc_graph_viz.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import { mountShell } from '@standarx/standardoc-viz/shell';

const app = document.getElementById('app');
if (app === null) throw new Error('shell: #app container missing');

const transport = new StreamableHTTPClientTransport(new URL('/mcp', window.location.origin));

mountShell(app, {
  transport,
  wasm: { init, OverviewCanvas, FocusGraphCanvas },
  wasmUrl: '/pkg/standardoc_graph_viz_bg.wasm',
  clientInfo: { name: 'standardoc-graph-viz-shell', version: '0.0.1' },
}).catch((e: unknown) => {
  const msg = e instanceof Error ? e.message : String(e);
  const status = app.querySelector('[data-shell-status]');
  if (status !== null) status.textContent = `fatal: ${msg}`;
  console.error('[shell] mount failed', e);
});

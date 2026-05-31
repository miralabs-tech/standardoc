// Browser entry for the shell harness. Mounts the real `mountShell`
// with the stub WASM + fake MCP transport so Playwright can assert the
// shell's DOM wiring end-to-end without a daemon or a GPU.

import { mountShell } from '../../src/shell/mount';
import { createFakeTransport } from './fake-transport';
import { stubWasm } from './stub-wasm';

const app = document.getElementById('app');
if (app === null) throw new Error('harness: #app missing');

async function boot(): Promise<void> {
  const transport = await createFakeTransport();
  await mountShell(app!, {
    transport,
    wasm: stubWasm,
    wasmUrl: 'data:application/wasm;base64,',
    clientInfo: { name: 'shell-harness', version: '0.0.0' },
  });
}

boot().catch((e: unknown) => {
  const msg = e instanceof Error ? e.message : String(e);
  const status = app.querySelector('[data-shell-status]');
  if (status !== null) status.textContent = `fatal: ${msg}`;
  // Surface for Playwright to read off the page if mount rejects.
  (window as unknown as { __SHELL_ERROR__?: string }).__SHELL_ERROR__ = msg;
  console.error('[harness] mount failed', e);
});

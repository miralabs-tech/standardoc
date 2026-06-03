import { defineConfig, devices } from '@playwright/test';

const PORT = 4321;

// Playwright harness for the multi-panel shell. Boots a bun dev server
// that bundles `tests/shell/harness.ts` (real mountShell + stub WASM +
// fake MCP transport), then drives it in chromium. Deterministic: no
// daemon, no GPU, fixtures only.
export default defineConfig({
  testDir: './tests/shell',
  testMatch: '**/*.spec.ts',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: 0,
  reporter: process.env.CI ? 'github' : 'list',
  use: {
    baseURL: `http://localhost:${PORT}`,
    trace: 'on-first-retry',
  },
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } },
  ],
  webServer: {
    command: `bun tests/shell/serve.ts`,
    url: `http://localhost:${PORT}/`,
    reuseExistingServer: !process.env.CI,
    timeout: 30_000,
    env: { HARNESS_PORT: String(PORT) },
  },
});

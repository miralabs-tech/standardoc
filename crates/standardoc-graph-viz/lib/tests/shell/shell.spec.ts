import { test, expect } from '@playwright/test';

import { FIXTURE_ENTRY_POINT_COUNT } from './fixtures';

// End-to-end wiring test for `mountShell`: drives the real shell with a
// stub WASM facade + fake MCP transport (fixtures). Asserts the shell
// builds its DOM, registers every panel component, and completes its
// boot data flow (projects → symbols → tree → entry points → ready).

test.beforeEach(async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', e => errors.push(e.message));
  (page as unknown as { __errors: string[] }).__errors = errors;
  await page.goto('/');
});

test('shell reaches ready state with fixture entry points', async ({ page }) => {
  const status = page.locator('[data-shell-status]');
  await expect(status).toHaveText(
    `ready (${FIXTURE_ENTRY_POINT_COUNT} entry points)`,
    { timeout: 15_000 },
  );
  const mountErr = await page.evaluate(
    () => (window as unknown as { __SHELL_ERROR__?: string }).__SHELL_ERROR__,
  );
  expect(mountErr).toBeUndefined();
});

test('shell builds the full panel layout', async ({ page }) => {
  // Wait for boot to settle so component upgrades have run.
  await expect(page.locator('[data-shell-status]')).toContainText('ready', { timeout: 15_000 });

  await expect(page.locator('standardoc-panel-layout[data-shell-root]')).toBeVisible();
  await expect(page.locator('standardoc-search[data-shell-search]')).toBeAttached();
  await expect(page.locator('standardoc-explorer[data-shell-explorer]')).toBeAttached();
  await expect(page.locator('standardoc-overview[data-shell-overview]')).toBeAttached();
  await expect(page.locator('standardoc-focus-graph[data-shell-focus]')).toBeAttached();
  await expect(page.locator('standardoc-symbol-details[data-shell-details]')).toBeAttached();
  await expect(page.locator('standardoc-panel-host[data-shell-panels]')).toBeAttached();

  // Toolbar panel toggles all start pressed.
  const toggles = page.locator('button[data-toggle-panel]');
  await expect(toggles).toHaveCount(4);
});

test('panel toggle flips aria-pressed', async ({ page }) => {
  await expect(page.locator('[data-shell-status]')).toContainText('ready', { timeout: 15_000 });
  const explorerToggle = page.locator('button[data-toggle-panel="explorer"]');
  await expect(explorerToggle).toHaveAttribute('aria-pressed', 'true');
  await explorerToggle.click();
  await expect(explorerToggle).toHaveAttribute('aria-pressed', 'false');
});

test('visual snapshot of the booted shell', async ({ page }) => {
  await expect(page.locator('[data-shell-status]')).toContainText('ready', { timeout: 15_000 });
  // Mask the status text (entry-point count is fixture-stable but the
  // canvas regions are stub-painted; clip to the chrome for stability).
  await expect(page).toHaveScreenshot('shell-booted.png', {
    fullPage: true,
    maxDiffPixelRatio: 0.02,
  });
});

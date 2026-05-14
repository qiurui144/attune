import { defineConfig, devices } from '@playwright/test';

/**
 * Attune E2E configuration.
 *
 * Per /home/qiurui/.claude/CLAUDE.md hard-rule:
 *   "MCP 浏览器限制：只允许使用 Chrome (channel='chrome'),
 *    禁止 Chromium/Firefox/WebKit"
 *
 * Therefore we configure ONE project: chrome (channel='chrome').
 * The default Playwright project list (chromium, firefox, webkit) is overridden.
 */
export default defineConfig({
  testDir: './specs',
  fullyParallel: false, // Tests share a single attune-server process; run serially
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  workers: 1, // Single attune-server backend → single worker
  reporter: [['list'], ['html', { open: 'never' }]],

  use: {
    baseURL: process.env.ATTUNE_BASE_URL || 'http://127.0.0.1:18900',
    trace: 'on-first-retry',
    screenshot: 'only-on-failure',
    video: 'retain-on-failure',
  },

  projects: [
    {
      name: 'chrome',
      use: {
        ...devices['Desktop Chrome'],
        channel: 'chrome', // ← per CLAUDE.md, MUST be 'chrome' not 'chromium'
      },
    },
  ],

  // No webServer here — we manage attune-server lifecycle via fixtures
  // (helpers/server.ts) so each test run can use a fresh tempdir vault.
  expect: {
    timeout: 10_000,
  },
  timeout: 60_000,
});

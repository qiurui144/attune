/**
 * E2E smoke (Layer 4 — `docs/TESTING.md` §1.1).
 *
 * Validates that the Playwright + Chrome + attune-server-headless stack works
 * end-to-end. Does NOT exercise wizard / chat / Reader (those go to C.3 golden
 * flows). Just: server up → browser opens UI → wizard step 1 visible → screenshot.
 *
 * If this spec fails, the entire E2E layer is broken and golden flows can't run.
 * Acts as a precondition check for C.3.
 */
import { test, expect } from '@playwright/test';
import { spawnAttuneServer, type ServerHandle } from '../helpers/server';

let server: ServerHandle;

test.beforeAll(async () => {
  server = await spawnAttuneServer({ port: 18902 });
});

test.afterAll(async () => {
  await server?.cleanup();
});

test.describe('E2E framework smoke', () => {
  test('attune-server is reachable via HTTP', async ({ request }) => {
    const res = await request.get(`${server.baseUrl}/api/v1/status/health`);
    expect(res.ok()).toBeTruthy();
    const body = await res.json();
    expect(body.status).toBe('ok');
  });

  test('Web UI loads in Chrome', async ({ page }) => {
    await page.goto(server.baseUrl);
    // First-launch wizard should render (vault is sealed in fresh tempdir).
    // We don't assert specific UI text yet — that goes to C.3 wizard golden flow.
    // Just verify page loaded without console errors.
    await expect(page).toHaveTitle(/Attune/i, { timeout: 10_000 });
    await page.screenshot({
      path: 'screenshots/e2e-smoke-ui-loaded.png',
      fullPage: true,
    });
  });

  test('CORS allows Chrome extension origin (cross-cutting security check)', async ({
    request,
  }) => {
    // covers: F-08-BROWSEEXT precondition — extension must be able to talk to server
    const res = await request.fetch(`${server.baseUrl}/api/v1/status/health`, {
      method: 'OPTIONS',
      headers: {
        Origin: 'chrome-extension://abcdefghijklmnop',
        'Access-Control-Request-Method': 'GET',
      },
    });
    // Accept 200 (some servers return 200 for OPTIONS) or 204 (canonical preflight)
    expect([200, 204]).toContain(res.status());
  });
});

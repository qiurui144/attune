/**
 * E2E golden flow #1 — Wizard 5-step setup.
 *
 * Covers (per docs/FEATURES.md): F-01-VAULT (master password setup),
 * F-09-FORMFACTOR (Step 3 LLM cards), F-16-DISTRIBUTION (Tauri/Web UI loaded).
 *
 * Status: baseline navigation test ✅. Full 5-step automation with form input
 * + assertion is marked .fixme() and tracked as v0.6.x follow-up — completing
 * it requires stable selectors which are subject to UI redesign in v0.6.x
 * (per `docs/superpowers/specs/2026-04-17-product-positioning-design.md` §6.7).
 */
import { test, expect } from '@playwright/test';
import { spawnAttuneServer, type ServerHandle } from '../helpers/server';

let server: ServerHandle;

test.beforeAll(async () => {
  server = await spawnAttuneServer({ port: 18903 });
});

test.afterAll(async () => {
  await server?.cleanup();
});

test.describe('Wizard golden flow (F-01-VAULT + F-09-FORMFACTOR)', () => {
  test('fresh install lands on first-run wizard', async ({ page }) => {
    await page.goto(server.baseUrl);
    // Wizard renders when vault is sealed (fresh tempdir).
    // Don't assert specific button text — just verify page loaded with expected title
    // and contains either Chinese or English wizard prompts.
    await expect(page).toHaveTitle(/Attune/i);

    // Loose body content check — robust across UI revisions
    const bodyText = await page.locator('body').innerText();
    const hasWizardPrompt =
      /主密码|Master Password|首次|Welcome|Setup|设置/.test(bodyText);
    expect(hasWizardPrompt).toBeTruthy();

    await page.screenshot({
      path: 'screenshots/c3-wizard-step1.png',
      fullPage: true,
    });
  });

  test.fixme('walks through all 5 wizard steps and reaches dashboard', async ({ page }) => {
    // TODO (v0.6.x): full automation requires stable wizard selectors.
    // Steps to implement:
    //   1. Master password input → click Next
    //   2. Confirm device secret backup → click Next (acknowledge)
    //   3. LLM card: select "Cloud" or "Skip" (don't depend on real Ollama)
    //   4. Data import: click "Skip" (ingest tested separately in chat.spec)
    //   5. Verify dashboard loaded (chat input visible / sidebar present)
    //
    // Blocker: wizard selectors are subject to redesign in v0.6.x per the
    // 2026-04-17 product-positioning-design.md §6.7 "Tab 减法" rule.
  });
});

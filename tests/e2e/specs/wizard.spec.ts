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

  test('Step 1 Welcome → click 开始设置 → Step 2 Password visible', async ({ page }) => {
    // covers F-01-VAULT (vault setup precondition: arrives on password step)
    await page.goto(server.baseUrl);

    // Wait for Step 1 Welcome heading + CTA button. Default locale is zh-CN
    // (per CLAUDE.md). Locator chain matches both zh "开始设置" and en "Get started"
    // by using button role with text contains.
    const cta = page.getByRole('button', { name: /开始设置|Get started/i });
    await expect(cta).toBeVisible({ timeout: 10_000 });
    await cta.click();

    // Step 2 Password heading visible (zh "设置 Master Password" / en "Set Master Password")
    await expect(
      page.getByText(/设置 Master Password|Set Master Password/i)
    ).toBeVisible({ timeout: 5_000 });

    // Two password inputs visible (Password + Confirm)
    const pwdInputs = page.locator('input[type=password]');
    await expect(pwdInputs).toHaveCount(2, { timeout: 5_000 });
  });

  test('Step 1 → Step 2: password inputs render after CTA click', async ({ page }) => {
    // covers F-01-VAULT precondition (Step 2 form renders correctly)
    //
    // Note: this test intentionally stops at Step 2 form render (does NOT submit).
    // Reason: form submit triggers /vault/setup which mutates server-side state
    // (vault SEALED → UNLOCKED). Tests in this file share a single server
    // (beforeAll), so submitting from one test affects others non-deterministically.
    //
    // The submit + state-transition path is covered by Rust integration tests:
    // - crates/attune-server/tests/vault_setup_test.rs (HTTP-level)
    // - crates/attune-server/tests/system_wizard_full_flow_test.rs (8-step sequence)
    // E2E only verifies the UI form renders + accepts input.

    await page.goto(server.baseUrl);

    // Step 1: click CTA
    await page.getByRole('button', { name: /开始设置|Get started/i }).click();

    // Step 2 form: 2 password inputs + Submit button reachable
    const pwdInputs = page.locator('input[type=password]');
    await expect(pwdInputs).toHaveCount(2, { timeout: 5_000 });

    // Verify input is editable (UI not in disabled/loading state)
    const strongPwd = 'AttuneE2E!Test-Password-2026';
    await pwdInputs.nth(0).fill(strongPwd);
    await pwdInputs.nth(1).fill(strongPwd);

    // Submit button visible + enabled (button can take action — not stuck)
    const submitButton = page.getByRole('button', { name: /下一步|Next/i });
    await expect(submitButton.first()).toBeEnabled({ timeout: 5_000 });

    await page.screenshot({
      path: 'screenshots/c3-wizard-step2-form.png',
      fullPage: true,
    });
  });

  test.fixme('Step 3-5 + WizardDone → dashboard', async () => {
    // TODO (v0.6.x): Step 3 LLM card selection (Skip) → Step 4 Hardware →
    // Step 5 Data (Skip) → WizardDone → dashboard chat input visible.
    //
    // Step 3 has 3 cards (Ollama / Cloud / Skip). Selector relies on i18n text
    // ("跳过" / "Skip later"). Step 4 hardware detection is async + has its own
    // continue button. Step 5 has tri-mode card UI. WizardDone auto-redirects
    // after a brief delay.
    //
    // This is a 3-4 minute test if all steps wait realistic timeouts. Defer to
    // v0.6.x once Step 3-5 selectors are stabilized post-UI-revision.
  });
});

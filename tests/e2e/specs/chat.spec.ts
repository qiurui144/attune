/**
 * E2E golden flow #3 — Chat citation jump → Reader.
 *
 * Covers (per docs/FEATURES.md): F-03-CHAT (RAG chat + B1 citation breadcrumb),
 * F-02-RAG (search backing the chat retrieval), F-04-READER (citation deep-link
 * lands on the right chunk_offset).
 *
 * Status: baseline (chat endpoint reachable) + .fixme() for full citation jump
 * (requires LLM endpoint configured + indexed corpus + UI rendering of
 * citation chips).
 *
 * The chat → citation → Reader jump is the **flagship UX flow** of attune
 * (per F-03-CHAT and B1 design). Full automation needs:
 *  - LLM provider mocked or real (we don't ship one in tempdir)
 *  - Indexed corpus with at least one chunk
 *  - Stable citation chip selectors (subject to v0.6.x UI revision)
 *
 * This is therefore the lowest-priority golden flow for v0.6.1; manual
 * verification is in `tests/MANUAL_TEST_CHECKLIST.md`.
 */
import { test, expect } from '@playwright/test';
import { spawnAttuneServer, type ServerHandle } from '../helpers/server';

let server: ServerHandle;

test.beforeAll(async () => {
  server = await spawnAttuneServer({ port: 18905 });
});

test.afterAll(async () => {
  await server?.cleanup();
});

test.describe('Chat golden flow (F-03-CHAT + F-02-RAG + F-04-READER)', () => {
  test('chat endpoint exists and rejects without auth', async ({ request }) => {
    // F-03-CHAT precondition: route is wired
    const res = await request.post(`${server.baseUrl}/api/v1/chat`, {
      data: { messages: [{ role: 'user', content: 'hello' }] },
    });
    // Vault sealed → 403; or LLM unavailable → 500/503; either is fine for "wired"
    // 404 would mean the route wasn't registered, which IS a regression.
    expect(res.status()).not.toBe(404);
  });

  test('chat sessions endpoint exists', async ({ request }) => {
    // F-03-CHAT session persistence — list endpoint exists pre-unlock
    const res = await request.get(`${server.baseUrl}/api/v1/chat/sessions`);
    expect(res.status()).not.toBe(404);
  });

  test.fixme('user sends chat → citation appears → click jumps to Reader chunk', async () => {
    // Deferred to v0.7+. Implementing this requires three pieces of mock
    // infrastructure not yet built:
    //
    //   1. **Mock OpenAI-compatible HTTP server** (~100 LoC Node.js fixture)
    //      Listens on /v1/chat/completions, returns deterministic JSON with
    //      a citation marker the parser can recognize. Playwright `route()`
    //      cannot intercept — attune uses reqwest from Rust, bypasses
    //      browser network stack.
    //
    //   2. **Mock embedding endpoint** OR live Ollama with bge-m3 pulled.
    //      Without embedding, ingest queue cannot complete and search returns
    //      empty knowledge. CI doesn't have Ollama — needs either skip-when-
    //      missing logic or mock embedding HTTP that returns stable vectors.
    //
    //   3. **Stable Reader chunk_offset jump UX** — the citation chip click →
    //      Reader open at offset path is subject to UI revision in v0.6.x
    //      (per docs/superpowers/specs/2026-04-17-product-positioning-design.md
    //      §6.7 "Tab 减法"). Implementing now risks rewrite when the citation
    //      UX changes.
    //
    // Coverage in the meantime:
    //   - F-03-CHAT routing & session API: ✅ (this file's other 2 tests)
    //   - F-17-PRIVACY PII redact in chat path:
    //     ✅ crates/attune-core/tests/pii_chat_path_redact_test.rs (4 tests)
    //   - Citation field shape (breadcrumb, chunk_offset, confidence):
    //     ✅ crates/attune-core/src/chat.rs unit tests
    //   - Manual verification: tests/MANUAL_TEST_CHECKLIST.md
    //
    // When pieces 1 + 2 + 3 land (likely v0.7), replace this fixme with a
    // real test that covers the flagship UX path. See
    // memory/feedback_module_vs_wiring.md §3 for grep-audit checklist before
    // claiming this scenario "verified".
  });
});

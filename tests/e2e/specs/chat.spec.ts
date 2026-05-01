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
    // TODO (v0.6.x): full flow requires:
    //   1. Setup vault + configure LLM endpoint (need mock for CI)
    //   2. Ingest 1 document
    //   3. Wait for embedding indexing
    //   4. Click chat tab → type query → submit
    //   5. Wait for response with citation chip
    //   6. Verify chip carries source / breadcrumb / chunk_offset / confidence
    //   7. Click chip → Reader modal opens at correct chunk_offset
    //
    // This is the flagship UX flow. Manual verification in
    // tests/MANUAL_TEST_CHECKLIST.md until LLM mock infrastructure is built.
  });
});

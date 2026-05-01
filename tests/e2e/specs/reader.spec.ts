/**
 * E2E golden flow #2 — Reader annotation → RAG boost.
 *
 * Covers (per docs/FEATURES.md): F-04-READER (5 user annotation tags + AI
 * annotations), F-02-RAG (annotation-weighted retrieval).
 *
 * Status: baseline (Reader URL reachable) + .fixme() for full annotation→boost
 * scenario (requires fully indexed item + measurable retrieval delta).
 *
 * The "annotation → RAG boost" assertion (×1.5 for ⭐ Highlight, ×1.2 for
 * 🤔 Question, exclude for 🗑 / 🕰 Outdated) is best validated at the
 * Integration layer with golden retrieval queries — see
 * `crates/attune-core/tests/rag_w3_batch_a_integration.rs`. This E2E flow only
 * validates the user-facing annotation creation UX path.
 */
import { test, expect } from '@playwright/test';
import { spawnAttuneServer, type ServerHandle } from '../helpers/server';

let server: ServerHandle;

test.beforeAll(async () => {
  server = await spawnAttuneServer({ port: 18904 });
});

test.afterAll(async () => {
  await server?.cleanup();
});

test.describe('Reader golden flow (F-04-READER)', () => {
  test('annotations endpoint exists and is auth-gated', async ({ request }) => {
    // covers F-04-READER routes — pre-vault-unlock check that endpoints are
    // wired (must return 401 / 403, not 404).
    // We use --no-auth in the helper, so unauthorized → 403 (vault sealed).
    const res = await request.post(`${server.baseUrl}/api/v1/annotations`, {
      data: { item_id: 'i_nonexistent', offset_start: 0, offset_end: 10 },
    });
    // Vault is sealed (fresh tempdir) so we expect 403 (not 404 not_found).
    expect([401, 403, 422]).toContain(res.status());
  });

  test.fixme('user annotation increases item priority in subsequent search', async () => {
    // TODO (v0.6.x): full flow requires:
    //   1. Setup vault + ingest 2 documents (one tagged ⭐ Highlight)
    //   2. Wait for embedding queue to finish
    //   3. Search query that should match both
    //   4. Assert highlighted doc ranks higher (×1.5 boost from F-04-READER)
    //
    // This is better tested at Integration layer
    // (rag_w3_batch_a_integration.rs already covers this with deterministic
    // assertions), so E2E version is lower priority.
  });
});

# RAG, Embedding, and Reranker Quality Loop Design

## Purpose

Attune needs a generic quality loop for knowledge-base chat and summary that works across model changes, document sets, and deployment modes. The goal is to improve reliability for both small local models and stronger cloud/local models without encoding manual-specific answers, model names, or evaluation prompts into `attune-server`.

This design extends the current RAG architecture around three control points:

- retrieval quality: what was searched, what was found, and whether evidence satisfied the user request;
- embedding quality: whether vectors were produced by the current embedding runtime and are safe to use;
- reranker quality: whether reranking was available, used, effective, or cleanly bypassed.

## Non-Goals

- Do not add corpus-specific or industry-specific answer rules.
- Do not hard-code model names, model families, manual titles, product chips, or benchmark fixture content.
- Do not require scheduler protocol changes before this enhancement is useful.
- Do not make web demo call scheduler directly.
- Do not replace the existing search index, vector store, plugin profile, or scheduler adapter.
- Do not make local development E2E the release source of truth; full E2E remains K3-device based.

## Current Gaps

Recent K3 validation showed that the service can pass API and frontend gates while still needing stronger quality attribution. The current system has several weak seams:

- RAG metadata reports passes and coverage, but it does not expose a unified quality profile spanning retrieval, embedding, rerank, and answer repair.
- Embedding readiness is mostly reported as queue and vector availability, not as a compatibility contract between stored vectors and the active embedding runtime.
- Reranker enablement is a boolean setting plus environment overrides; unavailable or low-signal reranker behavior is not consistently visible in chat or eval reports.
- Small models can cite the right evidence but refuse or under-answer; repair exists, but it is not yet tied to a broader quality trace.
- E2E quality metrics cover answer/citation outcomes, but do not explicitly gate embedding fingerprint compatibility or reranker fallback behavior.

## Architecture

Add a generic `RagQualityProfile` concept at the server boundary. It is not a new LLM prompt and not a domain classifier. It is a structured trace assembled from existing stages:

- query preparation: original query, retrieval semantic query, history-aware additions, expanded queries;
- retrieval stages: first pass, sub-query pass, recovery pass, final candidate count, selected source count;
- evidence quality: evidence needs, satisfied/missing needs, source diversity, primary source, quality label;
- embedding state: provider, model identity when available, vector dimension, fingerprint, stale-vector counts;
- reranker state: requested, capability ready, used, skipped reason, candidate count, actionable score state;
- answer state: scheduler task, model discipline, repair triggered, repair reason, final citation count.

The profile is returned under existing RAG metadata so clients and E2E can inspect it without changing the public chat response shape.

## RAG Enhancements

RAG should remain extractive-first for weak or small local models:

- Keep the original user message for answering.
- Use the retrieval semantic query for search and evidence selection.
- Prefer evidence packs that satisfy requested evidence needs over score-only ranking.
- Record when answer repair changes a low-quality model output into an evidence-backed response.
- Treat missing evidence as an explicit state: answer with a limited response and list what evidence is missing.

RAG selection must stay generic. It may use structural signals such as identifiers, paths, source titles, citation continuity, question mode, and evidence kind. It must not use fixture text or manual-specific keyword lists to force an answer.

## Embedding Enhancements

Add an embedding runtime fingerprint and expose it through diagnostics and indexing state. The fingerprint should be derived from generic properties:

- provider kind;
- scheduler or local embedding model id when available;
- vector dimension;
- normalization or distance metric when available;
- implementation/version string when available.

Every vector-bearing index should persist enough metadata to decide whether vectors are compatible with the current runtime. When compatibility cannot be proven, Attune should:

- mark affected vectors as stale;
- enqueue re-embedding instead of silently using incompatible vectors;
- continue full-text retrieval as a degraded fallback;
- expose stale and pending counts in status/diagnostics.

This is a correctness feature, not a performance optimization. It prevents model swaps from producing apparently successful but semantically invalid retrieval.

## Reranker Enhancements

Reranker behavior should be capability-gated and observable:

- Read the desired rerank policy from settings and environment overrides as today.
- Check scheduler/runtime capability readiness before using a scheduler-backed reranker.
- If unavailable, continue with hybrid lexical/vector order and report `skipped_reason`.
- If used, report candidate count, returned score count, actionable score state, and whether rerank changed top results.
- If returned scores are flat, invalid, or non-actionable, keep the prior order and report a low-signal fallback.

The reranker gate must be capability-based. It should not depend on concrete model names.

## Configuration

Add or standardize these settings under existing app settings:

- `rag.quality_trace.enabled`: default true.
- `embedding.fingerprint.enforce`: default true for scheduler/local embeddings.
- `embedding.fingerprint.reindex_on_mismatch`: default true.
- `rerank.enabled`: existing setting remains authoritative.
- `rerank.require_ready`: default true for scheduler-backed rerank.
- `rerank.min_actionable_score`: optional generic threshold; if unset, use existing core defaults.
- `rerank.trace.enabled`: default true.

Environment overrides may exist for debugging and CI, but package defaults should work without extra env vars.

## Data Flow

1. Upload or directory import creates items and queues embeddings.
2. Embedding worker computes vectors and stores vector metadata with the active embedding fingerprint.
3. Chat/search builds a retrieval semantic query and retrieval plan.
4. Search validates vector compatibility before using vector hits.
5. Search combines lexical and vector candidates, then optionally reranks if capability and settings allow it.
6. RAG orchestration assembles an evidence pack and quality profile.
7. Scheduler or local extractive repair produces the answer.
8. Response metadata includes quality trace, embedding state, reranker state, evidence diagnostics, and answer repair state.
9. E2E gates assert that the final answer is correct and that the quality loop reported the expected non-degraded or explicitly degraded path.

## Error Handling

- Embedding fingerprint mismatch is not fatal to chat. It degrades vector search, schedules re-embedding, and reports degraded retrieval.
- Reranker unavailable is not fatal. It reports fallback and preserves hybrid candidate order.
- Reranker invalid scores are not fatal. They are ignored with trace metadata.
- Evidence quality weak or partial is not fatal. The answer must state limits and avoid unsupported conclusions.
- Scheduler capability probe failures should preserve existing local/full-text fallback behavior.

## Testing

Unit tests:

- RAG quality profile reports query prep, evidence quality, repair state, and selected sources.
- Embedding fingerprint mismatch marks vectors stale and enqueues re-embedding.
- Search does not use incompatible vector results when enforcement is enabled.
- Reranker readiness false yields fallback trace with no reranker call.
- Reranker low-signal scores preserve prior order and report non-actionable scores.

Contract tests:

- `/api/v1/status` and diagnostics expose embedding fingerprint and stale counts.
- `/api/v1/chat` includes quality trace without breaking existing response fields.
- Settings parsing honors the new defaults and overrides.

K3 E2E:

- Clean reset, import, embedding drain, chat, summary, and web-demo Playwright remain required.
- RAG eval must keep answer/citation quality at pass thresholds.
- Embedding stale/reindex scenario must be covered with a controlled fingerprint mismatch.
- Reranker ready and reranker fallback paths must both be covered when scheduler reports capabilities.
- E2E prompts remain natural human questions and must not include hidden answer steering.

## Acceptance Criteria

- Existing K3 release smoke remains green.
- Chat response metadata can explain whether a failure is retrieval, embedding, rerank, evidence, scheduler, or answer-generation related.
- Switching embedding model/config does not silently reuse incompatible vectors.
- Reranker readiness or failure never blocks chat, but is visible in trace metadata.
- Small-model refusal despite cited evidence is repaired only when cited evidence is available and the repair is reported.
- No new hard-coded domain, manual, chip, model, or eval-answer logic is introduced.
- Deb packaging still produces `attune-server`, `attune-web-demo`, and `attune-oss-companion` with package-boundary checks passing.

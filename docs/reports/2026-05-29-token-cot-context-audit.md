# Token Cost / CoT / Context Reload — Read-Only Audit (Agent C)

Date: 2026-05-29 · Scope: attune-core cost/usage/cache/memory/context_budget + cloud llm-gateway/newapi + agent prompts · Mode: read-only (no cargo, no edits).

This audit feeds the "高溢价 output token + 思维链叠加" governance spec. All findings cite file:line.

---

## TL;DR (for the spec)

| Mechanism | Status | One-line verdict |
|---|---|---|
| Cost three layers (zero/local/cloud) | ⚠️ partial | `cost.rs` is a flat USD estimator; the 🆓/⚡/💰 tier split lives only in code comments + `ingest/pipeline.rs:181`, not enforced by a cost type |
| UI shows token + $ | ✅ chat only | Chat returns `cost_usd` (chat.rs:1001/1073) → `TokenChip`; cloud `QuotaView` shows monthly rollup. Agent/annotation/consolidation calls show nothing |
| **A1 LLM cache wired into agent calls** | ❌ **NOT WIRED** | `cache_backend` installed in AppState but **never `.get()/.put()` on any LLM path** — only admin count/clear. Public surface, dead for hits |
| **Usage recorder wired** | ❌ **NOT WIRED** | `set_usage` / `spawn_flusher` have **zero production callers** (tests only). `_usage` discarded at chat.rs:375. Aggregator stays `None` in prod |
| **Output token cap (num_predict/max_tokens)** | ❌ **NONE** | `grep num_predict / "max_tokens"` set-in-body = **0 hits**. No CoT/output ceiling anywhere |
| Context reload (memory L1→L3 + budget) | ✅ mature | `context_budget::plan_context` is window-aware + caps history; `memory_consolidation` + `memory/assembler` tiered recall are real and bounded |
| Cloud per-user quota | ✅ real, ⚠️ no enforce-in-loop | newapi quota set per plan; `/users/me/quota` dashboard reads rollup. No client-side downgrade on exhaustion |
| Cost-aware scheduling (cheap-first) | ❌ none | No "local-first / cheap-model-first" router. Single configured model per call site |

---

## 1. Output token governance (Q1)

**No output token limit exists anywhere.** Searching every `.rs` for `num_predict` / `"max_tokens"` / `max_output_tokens` / `maxOutputTokens` actually set in a request body returns **zero hits**.

- Ollama request builds `options` with only `seed` / `temperature` / `top_p` — no `num_predict` (`llm.rs:563-578`).
- OpenAI-compatible body sets only `seed` / `temperature` / `top_p` — no `max_tokens` (`llm.rs:947-971`).
- The only mention of output caps is *passive complaint* about Ollama's **default** truncation, with salvage parsing: `ai_annotator.rs:329` / `:584` ("Ollama 默认 max_tokens 经常截断…尽量抢救前面成功的 finding").

**CoT / chain-of-thought:** there is no notion of "reasoning tokens" vs "answer tokens." Whatever the model emits (including any CoT preamble) is billed as `tokens_out` and counted whole by `cost::estimate_tokens`. There is **no "简洁输出" constraint** in prompts or call options to suppress CoT bloat. Prompts do ask agents to "只输出一段总结，不要标题、不要列表" (`memory_consolidation.rs` prompt) but that is per-prompt discipline, not a systemic output budget.

**Governance gap:** output is uncapped + CoT uncontrolled + billed in full. This is exactly the "CoT 叠加爆 token" surface the spec targets. There is no `OutputBudget` analog to the input-side `context_budget::BudgetPlan`.

---

## 2. Cost three layers (Q2)

The 🆓 zero / ⚡ local / 💰 cloud contract from CLAUDE.md is **a documented intent, not a typed cost model**:

- `cost.rs` is purely a *cloud-$* estimator: `estimate_tokens` (CJK-aware heuristic, ±15%), `lookup_pricing` (prefix-matched price table, 2026-04 snapshot), `estimate_cost_usd` (returns `None` for unknown/local models). Header comment cites the Cost & Trigger Contract but the code has **no `CostTier` enum**.
- The tier distinction surfaces only as: (a) the `llm_is_local` boolean at `chat.rs:1001` (`cost_usd = None` for local), and (b) scattered comments like `ingest/pipeline.rs:181` ("Internal knowledge linker — 🆓/⚡ tier only").
- `TokenUsage.provider` enumerates `ollama / openai / gemini / cloud_gateway / k3_local / mock` (`usage/types.rs`) — enough to *infer* tier, but nothing maps provider→tier→trigger-policy in code.

**UI display (Q2 cont'd):**
- Chat: real. Route emits `cost_usd` + token counts (`chat.rs:1001`,`:1073`); `useChat.ts:139-141` feeds `lastCostEstimate` → `TokenChip`. Local model → `cost_usd: None` → chip shows "~本地". Matches the contract for the chat send button.
- Cloud monthly: `QuotaView.tsx` shows `llm_tokens_input/output/total/cost` + 3-month history, backed by `/api/v1/users/me/quota` (`accounts/api/quota.py:188`).
- **Not shown:** every non-chat LLM call (AI annotation, memory consolidation, classifier, query_rewrite, skill_evolution, summarize, report, VLM). None thread a per-call cost to UI. The contract's "每个 AI 分析按钮标注本地/云端 + 预估耗时/花费" is **only honored for chat**.

---

## 3. Cache wire — public surface but DEAD for hits (Q3)

This is the headline finding and confirms the A1 Task L "instantiation deferred" warning.

**What exists (A1, spec `2026-05-28-cache-context-token-standard-api.md`):**
- `cache/key.rs::cache_key(model, prompt)` = BLAKE3 32-hex (128-bit) prefix, `0xFF` separator. Sound.
- `cache/memory.rs::MemoryLruCache` — per-scope LRU (LLM/Embed/Search independent, cap 512). Installed at startup: `state.rs:194` (`MemoryLruCache::new(512)`).
- `cache/sqlite_encrypted.rs` (L2) + `store/cache.rs` tables `llm_cache` / `embed_cache`.
- `CacheBackend` async trait with `get/put/clear/count`, frozen "at Task M."

**What is NOT wired — the gap:**
- `cache_backend` is queried **only for admin count/clear** (`store/cache.rs:99-122`, `cache_count`/`cache_clear_scope`). There is **no `.get(CacheScope::Llm, ...)` or `.put(CacheScope::Llm, ...)` on any chat / agent / embed call path.** Grep for those callsites returns only the trait def, the lib.rs doctest, and admin clear.
- The LLM chat path (`chat.rs:375`) calls `self.llm.chat_with_history(&messages)?` directly — **no cache lookup before, no cache store after.**
- The only *live* caches are pre-A1 and separate: the legacy `search_cache: LruCache<u64, CachedSearch>` (`state.rs:82`, used at `routes/search.rs:214`) and the `chunk_summaries` compression cache inside `chat.rs:559-738` (its own DB-backed phase-1/phase-3 logic, unrelated to `cache_backend`).

**Conclusion:** the new unified `CacheBackend` is a frozen public API with **no consumer** — exactly the "public surface but未 wire (aggregator instantiation 推后)" state. `cache_key` is computed nowhere in production. Cache-hit telemetry (`CacheOutcome::Hit`) can never fire because nothing queries the cache.

---

## 4. Usage recorder — also NOT wired (Q3 cont'd)

Parallel to the cache gap:
- `UsageAggregator` (ring buffer + tokio flusher → `usage_events` table) is sound: `record` is sync µs push (`aggregator.rs`), `spawn_flusher` drains every `flush_interval_ms`.
- `state.rs:192` initializes `usage_aggregator: Mutex::new(None)`; accessors `usage()`/`set_usage()` exist (`state.rs:1842-1849`).
- **`set_usage` and `spawn_flusher` have zero non-test callers.** Every `UsageAggregator::new` / `spawn_flusher` hit is in `usage/tests/aggregator_test.rs`. state.rs comments are explicit: "usage aggregator stays None until set_usage is called post-vault-unlock" (`state.rs:1951`) — and nothing calls it.
- `chat.rs:371-375` is the smoking gun: provider returns `(String, TokenUsage)`, code binds `let (raw_response, _usage) = ...` and the comment says routing usage "lives at the route layer (Task U) once UsageAggregator is in AppState (Task L)." **Task U/L did not land** — `_usage` is discarded.
- The chat route *separately* recomputes cost inline (`chat.rs:1001` via `estimate_cost_usd`) for the UI chip, bypassing the `UsageEvent`/recorder pipeline entirely. So `usage_events` table is **never written in production** (only `record_usage` unit tests insert rows; `usage_summary` rollup at `store/usage.rs:112` has no production feeder).

**Net:** the entire A1 telemetry + cache subsystem is built, tested, frozen — and **disconnected from the live call paths.** Cache-hit rate, per-call cost rollup, and failure telemetry are all non-functional in production today.

---

## 5. Context accumulation + reload — the mature part (Q4)

This is the one area that IS wired and bounded.

```
ingest ──► chunk_summaries (150-char per-chunk archival summary, ⚡ tier)
              │
              ▼  (periodic, MAX_BUNDLES_PER_CYCLE=4, MAX_CHUNKS_PER_BUNDLE=50,
                  LOOKBACK 30d, window=1 day, current-day excluded)
        memory_consolidation ──► memories (episodic, L2)  [INSERT OR IGNORE idempotent]
              │
              ▼  (W5+ semantic; memory/semantic.rs)
        memories (semantic, L3)
              │
        ┌─────┴──────────────────────────────────────────┐
        │  CHAT TURN                                       │
        │  memory/assembler.rs: cheap heuristic picks      │
        │   preferred tier (L0 raw chunks vs L2/L3 memory),│
        │   coverage-gate guards against weak memory hits  │
        │                                                  │
        │  context_budget::plan_context(model, sys, user,  │
        │    history):                                     │
        │    window = context_window(model)  [model lookup]│
        │    reserve = clamp(window/4, 512, 4096)          │
        │    available = window - reserve - sys - user     │
        │    → split: knowledge_tokens vs history          │
        │    → trim oldest history (history_dropped)       │
        │  chat.rs:386 trim_history() drops oldest turns + │
        │    inserts an elision marker so model knows       │
        └──────────────────────────────────────────────────┘
```

Bounds that genuinely prevent context blow-up:
- `context_window` maps model→window, **unknown model → conservative 8K** (`context_budget.rs`, `context_window_unknown_is_conservative`).
- `plan_context` reserves response space, splits remaining budget knowledge/history, and **drops oldest history** when over budget (`plan_trims_history_on_small_window`). Oversized inputs use `saturating_sub` → no panic.
- Consolidation caps: `MAX_BUNDLES_PER_CYCLE=4`, `MAX_CHUNKS_PER_BUNDLE=50`, `PREPARE_FETCH_LIMIT=1000`, 30-day lookback → bounded LLM call fan-out per cycle.
- `memory/assembler.rs` uses a zero-LLM heuristic to *prefer* the compact memory tier (L2/L3) for recall/overview queries, with a cosine coverage gate so weak memory hits don't displace raw L0.

This is the input-side budget done right. The **asymmetry** worth flagging to the spec: input context is rigorously budgeted (`context_budget`), but **output has no equivalent** (§1).

---

## 6. Token quota scheduling (cloud) (Q5)

- Per-plan quota matrix in `accounts/config.py:123-126`: individual 100k / pro 5M / pro_plus 20M / enterprise 100M tokens.
- Identity map: `accounts.User → new-api user`, quota = `plan_quota_<plan>` (`newapi_sync.py:4`). Quota write goes through `set_user_quota` (`:157`) which **reads current → applies the delta** via `POST /api/user/manage action=add_quota` because new-api's PUT silently drops the quota field and override mode doesn't refresh the in-memory cache (`:161-167`). Idempotent by design.
- Dashboard: `/api/v1/users/me/quota` (`api/quota.py:188`) queries gateway monthly usage (`_query_gateway_monthly_usage`), computes `remaining = max(0, quota_limit - used)` + `percent_used` (`:213-216`), returns limits + 3-month history.

**Free vs paid difference:** purely the quota ceiling (individual 100k vs pro 5M+). Enforcement is **at the gateway** (new-api decrements; exhausted token → upstream 429). 

**Gap (Q5):** there is **no client-side degradation strategy on quota exhaustion.** The Rust client has no "quota low → switch to local Ollama / cheaper model" path. `_aggregate_history` / dashboard is visibility only. When the gateway 429s, it surfaces as a generic `ErrorKind::Quota` (defined in `usage/types.rs`) — but since the recorder isn't wired (§4), even that classification isn't persisted.

---

## 7. High-premium scenarios + cost-aware scheduling (Q6)

**LLM-judgement (token-heavy) call sites** (vs deterministic): `ai_annotator.rs`, `chat.rs`, `classifier.rs`, `clusterer.rs` (LLM cluster labeling), `context_compress.rs` (per-chunk summary — fan-out heavy), `linker/mod.rs`, `memory/assembler.rs` + `memory/semantic.rs`, `memory_consolidation.rs` (4 bundles/cycle), `pii/mod.rs`, `query_rewrite.rs`, `report.rs`, `search.rs`, `skill_evolution/agent.rs`, `skills/summarize_text.rs`, `vlm.rs`.

**Most token-burning:**
1. `context_compress` chunk summarization inside chat — N LLM calls per turn (one per uncached chunk over the short-text threshold). Mitigated by the chunk-summary cache (the *only* live LLM-output cache, but bespoke, not `CacheBackend`).
2. `memory_consolidation` — up to 4 bundle-summary LLM calls per periodic cycle.
3. `ai_annotator` — per-document multi-finding extraction (known to hit Ollama default truncation).
4. Chat itself — full RAG-injected context + uncapped output.

**Cost-aware scheduling: none.** No "本地优先 / cheap-model-first" router exists. The "tier" hits in grep are unrelated (whisper ASR model tiers `asr.rs`, plugin pricing tier `plugin_encryption.rs`, hardware tier). Each call site uses the single configured `LlmProvider` with no cost-based model selection or local-first fallback. `chat_with_options` exposes seed/temp/top_p for determinism (v1.0.6 T1) — **not** cost knobs.

---

## Answers to the 6 questions (condensed)

1. **Output token governance:** None. No `num_predict`/`max_tokens` set anywhere; CoT billed in full; no "简洁输出" systemic constraint. Asymmetric vs the well-built input `context_budget`.
2. **Cost three layers:** Documented intent only. `cost.rs` is a flat cloud-$ estimator with no `CostTier` type. UI shows token+$ for **chat only** (`chat.rs:1001/1073` → TokenChip) + cloud QuotaView monthly; every other LLM call shows nothing.
3. **Cache hit:** `CacheBackend`/`cache_key` is frozen public API with **no production consumer** — never `.get/.put` on any LLM path; only the legacy `search_cache` and bespoke `chunk_summaries` cache are live. Hit telemetry can't fire.
4. **Context reload:** Mature + bounded. `chunk_summaries → episodic(L2) → semantic(L3)`, `memory/assembler` tiered recall with coverage gate, `context_budget::plan_context` window-aware history trimming. Caps everywhere. The strong part of the system.
5. **Token quota:** Real per-plan matrix (100k→100M) + delta-based newapi sync + `/users/me/quota` dashboard. Free vs paid = ceiling only. No client-side downgrade on exhaustion.
6. **High-premium:** Heaviest = chat chunk-compression fan-out, memory consolidation, ai_annotator, chat output. **No cost-aware/local-first scheduling** — single model per call site.

## Top governance recommendations (for spec author)

- **Wire A1 before adding anything:** call `set_usage` + `spawn_flusher` post-vault-unlock (Task L), and make every `LlmProvider::chat*` site go through cache `get`/`put` + `UsageRecorderGuard` (Task U). Today the entire telemetry/cache layer is dead weight.
- **Add an output budget** symmetric to `context_budget`: set `num_predict`/`max_tokens` per call kind + a CoT-suppression prompt convention, since CoT is the named risk.
- **Add a `CostTier` enum** and route trigger policy (🆓 auto / ⚡ background-pausable / 💰 user-triggered-only) through it instead of scattered comments.
- **Add cheap/local-first scheduling** + quota-exhaustion fallback (gateway 429 / `ErrorKind::Quota` → local Ollama or cheaper model).

# Multi-Layer Memory System — Token-Reduction Design + Implementation Plan

**Date**: 2026-05-18
**Scope**: OSS / free attune (`attune-core` + `attune-server`). **No** dependency on attune-pro or cloud.
**Roadmap**: extends A1 Memory Consolidation (`docs/superpowers/specs/2026-04-27-memory-consolidation-design.md`) and v0.7 Memory Moat.
**Goal**: A tiered memory architecture so context assembly for LLM calls sends the *right tier at the right granularity* instead of dumping raw chunks — measurable token reduction.

---

## 0. TL;DR

attune already has the *ingredients* of a layered memory but uses them disconnected:

- **L0 raw chunks** (`items.content`, vectors, fulltext) — what RAG searches today.
- **L1 chunk summaries** (`chunk_summaries`, the "150-字存档摘要") — built lazily on first chat that touches a chunk; **only used as a cheaper substitute for the same chunk's raw text** inside one answer.
- **L2 episodic memory** (`memories` table, A1 consolidator) — generated every 6h by a background worker, but **never retrieved into any LLM call**. Dead weight today.

The system **does not have a memory tier above L0 that the retriever can choose instead of L0**, and L1/L2 are not in the retrieval search space at all. Every chat answer therefore pays L0 token cost (raw chunk text, truncated by `allocate_budget`) even when an L1/L2 summary would answer the question at a fraction of the tokens.

This plan **wires the existing tiers into one retrieval/assembly path** and adds two missing pieces — a **semantic memory tier (L3)** and a **tier-aware context assembler** — so the assembler can answer broad/recall questions from compact summaries and reserve raw-chunk tokens for precise questions only.

**Headline mechanism**: a query that today injects ~5 raw chunks (≈ `knowledge_chars` budget, 2–8 K chars) can instead inject 1–3 episodic/semantic summaries (≈ 200–600 chars total) when the query is recall-shaped. Net injected-context reduction on recall/overview queries is large (see §5). Precise lookups still go to L0 — no quality loss there.

---

## 1. Current Memory + Context Architecture (as-is)

### 1.1 Storage layers that exist

| Layer | Table / index | Granularity | Built when | Encrypted | Used by |
|-------|---------------|-------------|-----------|-----------|---------|
| L0 raw | `items.content` BLOB, `item_blobs`, `usearch` vectors, `tantivy` FTS | full doc + chunk vectors | ingest pipeline (cost tier 1-2) | yes (content) | `search_with_context` |
| L1 chunk summary | `chunk_summaries(chunk_hash, strategy)` | ~150 char (`economical`) / ~300 char (`accurate`) per chunk | **lazily, first chat touching that chunk** (`context_compress::compress_chunk`) | yes (`summary` BLOB) | `context_compress` only — substitutes the chunk's own raw text in the *same* answer |
| L2 episodic | `memories(kind='episodic')` | ~200 char per 1-day window of 5–50 chunks | 6h background `start_memory_consolidator` worker (cost tier 3) | yes (`summary_encrypted`) | **nothing** — `list_recent_memories` only used by `attune --diag` |
| signals | `skill_signals`, `browse_signals`, `click_events` | event rows | reindex / chat / capture | — | skill evolution, annotation weight |

### 1.2 How context is assembled today (the chat path)

`routes/chat.rs::chat` (the live path; `chat.rs::ChatEngine` is the library twin used in tests):

1. `search_with_context` — 3-stage hybrid: tantivy + vector → RRF fuse → rerank → top-k (`with_defaults_for_rag`, cosine ≥ 0.65). Returns L0 `SearchResult` with full `content`.
2. `annotation_weight` re-weights by user markers.
3. If empty → web search fallback (cached in `web_search_cache`).
4. `context_budget::plan_context(model, …)` computes a `knowledge_tokens` budget from the model's context window; `allocate_budget` slices each result's raw `content` proportionally to score → `inject_content`.
5. `context_compress::compress_batch` replaces each `inject_content` with its L1 summary **if** the chunk is long and `strategy != raw` and a cache hit/LLM call succeeds.
6. `build_rag_system_prompt` concatenates `[i] 《title》(score) <inject_content>` blocks into the system prompt.
7. PII redact → `llm.chat_with_history` → restore → auto-save conversation.
8. History trimmed by `context_budget` (`trim_history`, drops oldest turns when window overflows).

### 1.3 Where tokens are spent today

Per chat call the LLM input is `system_prompt + knowledge + history + user_message`:

- **Knowledge injection is the dominant variable cost.** `plan_context` gives knowledge **half** of the free budget. For a 32 K model that is ~13 K tokens; for 1 M models ~480 K. `allocate_budget` will fill that with raw chunk text up to the budget. Even with L1 compression, each of the top-5 results contributes its *own* ~150–300 char summary — and L1 is **per-chunk**, so 5 chunks from the same document each pay their own summary; redundancy across chunks is never collapsed.
- **History grows unbounded** until `trim_history` drops whole oldest turns. There is no summarization of dropped history — the dropped turns' information is simply lost, and the kept turns are sent verbatim every call.
- **L2 episodic memory is generated but never injected** — its 6h LLM cost (cost tier 3) is *spent with zero token-saving return*. This is the single clearest waste in the current design.
- **No query-shape awareness.** A broad question ("what have I been learning about Rust?") runs the exact same L0 raw-chunk retrieval as a precise one ("what's the signature of `compress_chunk`?"). The broad question pays full raw-chunk tokens for an answer that an episodic/semantic summary would serve in a few hundred chars.
- **`accurate` strategy appends raw head** ("原文摘录: <first 100 chars>") on top of the summary — extra tokens per chunk.

**Net**: tokens are spent at L0 granularity for *every* query, L1 only ever shrinks a chunk to its own summary (no cross-chunk dedup, no tier escalation), and L2 spend is pure loss.

### 1.4 What "memory moat" means today

`tests/memory_moat_integration.rs` + `docs/TESTING.md` R33: the "moat" is currently the **doc-lifecycle signal loop** (5 signal kinds: doc_create/update/delete, citation_hit, annotation_marker) feeding skill evolution + `reindex` consistency — *not* a tiered memory. `memory_consolidation_integration.rs` tests the A1 consolidator in isolation. Neither tests retrieval *from* memory. So "multi-layer memory" is genuinely a new capability, not a rename.

---

## 2. Proposed Layer Model

Keep L0/L1/L2 exactly as they are physically; **add L3, add a router, add demotion**. Nothing is rewritten.

```
┌────────────────────────────────────────────────────────────────────┐
│ L0  RAW          items.content + vectors + FTS                      │
│     granularity: full chunk text     | cost to inject: highest      │
│     answers: precise lookups, exact quotes, code, numbers           │
├────────────────────────────────────────────────────────────────────┤
│ L1  CHUNK SUMMARY  chunk_summaries (economical 150c / accurate 300c) │
│     granularity: 1 summary / chunk   | built lazily on chat         │
│     answers: "what does this section say" — mid-precision           │
├────────────────────────────────────────────────────────────────────┤
│ L2  EPISODIC     memories(kind='episodic')                          │
│     granularity: 1 summary / day-window of 5–50 chunks (~200c)      │
│     answers: "what did I work on last week", time-scoped recall     │
├────────────────────────────────────────────────────────────────────┤
│ L3  SEMANTIC  (NEW)  memories(kind='semantic')                      │
│     granularity: 1 summary / topic cluster, spanning all time       │
│     answers: "what do I know about X" — broad concept recall        │
└────────────────────────────────────────────────────────────────────┘
        ▲ promotion (compaction, LLM, cost tier 3, background)
        ▼ demotion  (cold-archive flag, zero LLM)
```

### 2.1 What lives in each layer — and the compaction that cuts tokens

- **L0 → L1 (already exists)**: lazy on chat. Keep. *Token cut: chunk text → 150c summary.*
- **L1 → L2 (already exists)**: 6h consolidator, day-window. Keep. *Token cut: 5–50 chunk summaries → one ~200c episodic summary.*
- **L2 → L3 (NEW)**: a second consolidator pass. Episodic memories are already topic-poor (time-bucketed). L3 **re-clusters episodic + chunk summaries by topic** using the existing `hdbscan` clusterer (`clusterer.rs`, already in attune-core, zero new dep) over their embedding vectors, then runs **one LLM call per topic cluster** to produce a standing "what the user knows about <topic>" semantic memory. *Token cut: a whole topic's history → one ~300c standing summary.* This runs in the same background worker family, cost tier 3, governed by H1 quota.
- **Demotion (NEW, zero LLM)**: any L2 episodic memory whose `window_end` is older than `COLD_AGE_SECS` (default 180 days) **and** whose source chunks are all superseded by an L3 semantic memory covering the same topic is flagged `cold=1`. Cold memories are excluded from default retrieval (still queryable by explicit time-travel search). This is a pure SQL `UPDATE`, no LLM. It keeps the hot retrieval set small so tier selection stays fast and the injected set stays compact.

### 2.2 Promotion/demotion rules (concrete)

| Transition | Trigger | Cost tier | Idempotency key |
|-----------|---------|-----------|-----------------|
| L0→L1 | chat touches a long chunk | 3 (first time) → 1 (cache hit) | `(chunk_hash, strategy)` |
| L1→L2 | 6h worker, day-window ≥ 5 chunks | 3 | `(kind='episodic', sorted chunk_hashes)` |
| **L2→L3** | 6h worker, topic cluster ≥ `MIN_MEMS_PER_TOPIC` (default 4) episodic/L1 members | 3 | `(kind='semantic', topic_key)` where `topic_key` = sha256 of sorted member memory-ids |
| **L3 refresh** | cluster membership changed (new episodic added to topic) since last build | 3 | superseding insert: new `topic_key`, old row marked `superseded_by` |
| **L2 demote → cold** | `now - window_end > COLD_AGE_SECS` AND covered by an L3 row | **0 (SQL only)** | `cold` column flip |

L3 refresh is the only non-additive case. Handle it the same idempotent way A1 already does: a changed cluster yields a new `topic_key` → a fresh `INSERT OR IGNORE`; the old row gets `superseded_by = <new id>` and drops out of retrieval. No `UPDATE` of summary text in place (keeps the encrypted-blob model append-only and audit-friendly).

### 2.3 Cost-contract compliance

Per CLAUDE.md "成本感知与触发契约":

- **Building stays tier 1-2**: ingest, chunking, vectors, FTS, hdbscan clustering itself (CPU, pure Rust) — all unchanged, no LLM.
- **All summarization is tier 3**: L0→L1 (already user-triggered at chat time), L1→L2 and L2→L3 (background worker, but **gated by H1 `resource_governor` LLM quota** — `TaskKind::MemoryConsolidation` already exists). Demotion is tier 0 (SQL).
- **No new tier-3 spend on the read path**: the tier-aware assembler (§3) only *selects among already-built* L1/L2/L3 rows. It never triggers an LLM call to summarize on the fly for tier escalation — if a needed L1 summary is missing it falls back to L0 raw (current behavior) rather than paying mid-query.
- **UI cost display unchanged**: `cost::estimate_tokens` already drives the send-button chip; with the assembler injecting summaries the chip number simply goes *down*. Add one line to the chip tooltip: "context: L2 memory (≈N tok)" vs "context: L0 raw (≈N tok)" so the user sees *which tier* answered (transparency, no behavior change).

---

## 3. The Tier-Aware Context Assembler

New module `attune-core/src/memory/assembler.rs`. It is the single decision point that replaces the "always inject L0 raw" assumption.

### 3.1 Query-shape classification (zero LLM)

A cheap heuristic classifier `classify_query_shape(query) -> QueryShape` reusing existing primitives:

- **`Recall`** — time words present (`search::parse_time_filter` returns `Some`) e.g. "上周/last month/3 天前". → prefer **L2 episodic** scoped to that window.
- **`Overview`** — broad-intent markers: query contains overview verbs ("总结/回顾/学了什么/what do I know/overview/summarize my") OR is short (≤ 6 content words) and matches a known topic. → prefer **L3 semantic**.
- **`Precise`** — default. Code identifiers, quotes, numbers, long specific phrasing, or `detect_query_domain` hit with specific terms. → **L0 raw** (current path, unchanged).

This is a `match` on existing signals (`parse_time_filter`, `detect_query_domain`, word count, a small keyword set) — no model, no new dependency, runs in microseconds. It is *advisory*: it sets the *preferred* tier; the assembler still verifies coverage (§3.2).

### 3.2 Tier selection + assembly algorithm

```
fn assemble_context(query, history, dek, budget_plan) -> AssembledContext:
    shape = classify_query_shape(query)

    candidates = []
    match shape:
      Recall   -> candidates += search_memories(query, kind=episodic, time_filter)   // L2
      Overview -> candidates += search_memories(query, kind=semantic)                // L3
      _        -> {}

    // always also pull L0 — memory tiers are a *prefix*, not a replacement
    l0 = search_with_context(query, rag_params)            // existing path

    // coverage check: do the memory-tier hits actually score well?
    if shape != Precise and best(candidates).score >= MEMORY_CONFIDENCE (0.70):
        // memory tier answers it — inject compact summaries, cap L0 to 1 verifier chunk
        injected = top_n(candidates, 3) [summaries]  +  top_1(l0) [as a citation anchor]
    else:
        // fall back to current behavior fully
        injected = allocate_budget(l0, budget_plan.knowledge_chars)
        injected = compress_batch(injected, strategy)      // L1 path, unchanged

    return AssembledContext { injected, tier_used, est_tokens }
```

Key properties:

- **Memory tiers are searched, not just listed.** This needs L2/L3 summaries to be **embedded** so vector search can rank them — see §4 (`memory_vectors`). That is the one structural addition that makes L2 finally pay off.
- **L0 is always still consulted.** Even when L3 answers, we keep the single top L0 chunk so the answer has a precise citation/deep-link anchor (`Citation` with `chunk_offset_*`). The moat's deep-linking is preserved.
- **Coverage gate.** If memory-tier hits are weak (`< MEMORY_CONFIDENCE`), we fall straight back to today's L0+L1 path — *no regression possible* for queries the memory tiers can't serve.
- **Hooks into J5 second-retrieval.** If chat confidence `< 3`, the existing secondary retrieval already lowers the threshold; with the assembler it *also* drops the memory-tier preference and forces full L0 — i.e. memory-tier is the fast/cheap first attempt, L0 raw is the safety net.

### 3.3 History compaction (NEW, closes the unbounded-history waste)

Add `compact_history`: when `context_budget::plan_context` reports `history_dropped > 0`, instead of silently discarding the oldest turns, the assembler emits **one** rolling-summary turn. The summary is produced by the L1 summarizer (`generate_summary`, `economical`) over the concatenated dropped turns, **cached by `sha256(dropped turns)`** in `chunk_summaries` (reuse the table with a synthetic `item_id = "conv:<session_id>"`). So a long session pays one summarization the first time it overflows, then the rolling summary is a cache hit. *Token cut: N dropped verbose turns → one ~150c summary, every subsequent call.*

---

## 4. Data Model Changes

All additive. Existing vaults pick them up at next `Store::open` via `CREATE TABLE/INDEX IF NOT EXISTS` + the v0.7-style idempotent `ALTER TABLE` migration pattern already in `store/mod.rs`.

### 4.1 `memories` table — new columns

```sql
ALTER TABLE memories ADD COLUMN topic_key      TEXT;     -- semantic dedup key; NULL for episodic
ALTER TABLE memories ADD COLUMN cold           INTEGER NOT NULL DEFAULT 0;  -- demoted flag
ALTER TABLE memories ADD COLUMN superseded_by  TEXT;     -- id of newer semantic row; NULL if live
CREATE INDEX IF NOT EXISTS idx_memories_cold ON memories(cold, kind);
CREATE UNIQUE INDEX IF NOT EXISTS uq_memories_topic ON memories(kind, topic_key) WHERE topic_key IS NOT NULL;
```

`kind` CHECK already allows `'semantic'` (A1 pre-provisioned it — no CHECK migration needed).

### 4.2 New table `memory_vectors` — makes L2/L3 searchable

Episodic/semantic summaries must be embeddable so `assemble_context` can *rank* them, not just list newest-N. Mirror the vector-sidecar pattern:

```sql
CREATE TABLE IF NOT EXISTS memory_vectors (
    memory_id   TEXT PRIMARY KEY REFERENCES memories(id) ON DELETE CASCADE,
    embedding   BLOB NOT NULL,    -- f16-quantized, same dim as items vectors
    dim         INTEGER NOT NULL,
    model       TEXT NOT NULL,
    created_at  INTEGER NOT NULL
);
```

Loaded into a small in-memory `usearch` index at startup (memories are few — hundreds, not millions — so a dedicated tiny `VectorIndex` is cheap). Embeddings are generated by the **same `EmbeddingProvider`** as documents — embedding is cost tier 2 (local NPU/iGPU), so this respects the contract.

### 4.3 `chunk_summaries` reuse for rolling history

No schema change — the rolling-history summary is stored with `strategy='economical'`, `item_id='conv:<session_id>'`. The existing item-deletion cascade does not touch it (synthetic id), and a session-delete path can `DELETE FROM chunk_summaries WHERE item_id = 'conv:'||?`.

### 4.4 Settings (additive, in `config` / settings JSON)

```
memory.tiered_assembler_enabled : bool   = true
memory.semantic_enabled         : bool   = true
memory.cold_age_days            : int    = 180
memory.memory_confidence        : float  = 0.70
```

All have safe defaults; turning `tiered_assembler_enabled=false` yields exactly today's behavior (escape hatch + A/B baseline).

---

## 5. Token-Reduction Mechanism — Quantified

The mechanism is **tier substitution + cross-source compaction + history rolling**, not vague "less context".

### 5.1 Per-mechanism accounting

Assume a 32 K-window model (qwen2.5 / typical free tier), `knowledge_tokens` budget ≈ 13 K, and a representative knowledge set of 5 retrieved chunks averaging 1.2 K chars each.

| Mechanism | Today | With multi-layer | Saving |
|-----------|-------|------------------|--------|
| **Recall query** ("上周学了什么") | 5 raw chunks compressed to 5×150c L1 = ~750c ≈ 900 tok injected | 2 L2 episodic summaries ~400c + 1 L0 anchor chunk ~150c = ~550c ≈ 660 tok | ~25% on injected knowledge; **and** avoids running 5 separate L1 summarizations if cold |
| **Overview query** ("我对 Rust 了解多少") | 5 raw chunks, often spanning many topics, ~6 K chars raw or 5×150c L1 = ~750c | 2 L3 semantic summaries ~600c + 1 L0 anchor = ~750c **but** answers from a *standing* topic memory → fewer follow-up turns | comparable injected size *per call*, but L3 is pre-built (zero per-call LLM) and reduces multi-turn drilling |
| **Cross-chunk redundancy** (5 chunks, same doc) | each chunk's L1 summary repeats shared context | L2/L3 already collapsed the doc/topic once → one summary, no repeat | eliminates 2–4× duplicated framing text |
| **History overflow** (20-turn session, 8 dropped) | 8 turns silently dropped (info lost) OR kept verbatim ≈ 3–5 K tok | 8 dropped turns → 1 rolling summary ~150c ≈ 180 tok, cached | ~95% on the dropped-history span; also *recovers* info that was being lost |
| **L2 spend** | 6h LLM cost, **0 token return** | same 6h cost, now amortized across every recall query it serves | converts pure waste into ROI |

### 5.2 Why it is real, not hand-wavy

1. **Granularity substitution is arithmetic**: a day-window episodic summary is one ~200c blob in place of 5–50 chunk summaries (≥ 750c, often ≥ 5 K raw). The compaction ratio is the bundle size — measured per `source_chunk_count`.
2. **The compaction already happened at build time** — the per-query read path does *no* extra LLM work, so the saving is free at query time (the cost was paid once, in the background, under quota).
3. **Coverage gate guarantees no negative case**: if memory tiers don't cover the query, we inject exactly what we inject today. Worst case = today; expected case = less.
4. **History rolling recovers lost information** — it is both a token cut *and* a quality gain (dropped turns are currently just gone).

### 5.3 Acceptance metric

Add a `rag_quality_benchmark`-style harness that runs a fixed query set (recall / overview / precise mix) against the GitHub-corpus golden set and records **injected-context token count per query** with `tiered_assembler_enabled` on vs off. Target: **≥ 30% median reduction in injected knowledge tokens on the recall+overview subset, ≤ 0% change on the precise subset, no drop in answer correctness on the golden set.**

---

## 6. Modules to Add / Change

```
attune-core/src/memory/                 NEW module dir
  mod.rs            — re-exports
  assembler.rs      — classify_query_shape, assemble_context, compact_history   [NEW]
  semantic.rs       — L2→L3 consolidation (hdbscan cluster + LLM)               [NEW]
  retrieval.rs      — search_memories: vector search over memory_vectors        [NEW]
attune-core/src/memory_consolidation.rs  — unchanged (L1→L2); semantic.rs sits beside it
attune-core/src/store/memories.rs        — + topic_key/cold/superseded_by cols, insert_semantic_memory,
                                            search/list helpers, demote_cold_memories               [CHANGE]
attune-core/src/store/memory_vectors.rs  — memory_vectors CRUD                                       [NEW]
attune-core/src/store/mod.rs             — schema + idempotent migrations                            [CHANGE]
attune-core/src/store/chunk_summaries.rs — delete-by-conv-id helper for rolling history              [CHANGE]
attune-core/src/chat.rs                  — ChatEngine.search_for_context → call assembler            [CHANGE]
attune-server/src/routes/chat.rs         — live chat path → call assembler                           [CHANGE]
attune-server/src/state.rs               — extend memory consolidator worker: after episodic,
                                            run semantic pass + demotion; build memory_vectors       [CHANGE]
attune-server/src/state.rs               — load memory_vectors into a MemoryVectorIndex at startup   [CHANGE]
config / settings                         — 4 new memory.* keys                                     [CHANGE]
```

No new external crate — `hdbscan` and `usearch` are already dependencies.

---

## 7. Phased Implementation Plan

### Phase 1 — Data model + memory vectors (foundation, no behavior change)

- [ ] `store/mod.rs`: add `topic_key`, `cold`, `superseded_by` columns to `memories` via idempotent `ALTER TABLE` (mirror v0.7 `content_hash` migration); add `idx_memories_cold`, `uq_memories_topic` partial index.
- [ ] `store/memory_vectors.rs`: new file — `put_memory_vector`, `get_memory_vector`, `list_all_memory_vectors`, `delete_memory_vector`; `memory_vectors` table in schema.
- [ ] `store/memories.rs`: add `insert_semantic_memory(dek, topic_key, members, summary, model, now)`, `list_live_memories(kind, include_cold=false)`, `mark_memory_superseded(old, new)`, `demote_cold_memories(now, cold_age_secs) -> usize`.
- [ ] **Files**: `store/mod.rs`, `store/memory_vectors.rs` (new), `store/memories.rs`.
- [ ] **Tests**: migration roundtrip across reopens (extend `migration_roundtrip_test.rs`); `insert_semantic_memory` idempotent on same `topic_key`; `demote_cold_memories` flips only old+covered rows; `memory_vectors` CRUD + cascade-on-memory-delete.
- [ ] **Acceptance**: existing vault opens clean with new columns; all current tests still green; `tiered_assembler_enabled` default off → zero behavior change.

### Phase 2 — Memory retrieval (make L2 searchable, finally use it)

- [ ] `memory/retrieval.rs`: `MemoryVectorIndex` (small `usearch` wrapper); `search_memories(query, kind, time_filter, top_k) -> Vec<MemoryHit>` — embed query, vector-rank live (non-cold) memories of the kind, optional `window_*` time filter.
- [ ] `state.rs`: build `MemoryVectorIndex` at startup from `list_all_memory_vectors`; keep it in `AppState` behind a lock.
- [ ] Consolidator worker: after `apply_consolidation_result`, embed each new episodic summary and `put_memory_vector` (cost tier 2, local embedding).
- [ ] **Files**: `memory/retrieval.rs` (new), `memory/mod.rs` (new), `state.rs`.
- [ ] **Tests**: seed 3 episodic memories, `search_memories` ranks by cosine; time filter excludes out-of-window; cold memories excluded; empty index → empty result, no panic.
- [ ] **Acceptance**: a recall query can retrieve an episodic memory by relevance; still no change to chat output (assembler not wired yet).

### Phase 3 — Semantic tier (L3) + demotion in the worker

- [ ] `memory/semantic.rs`: three-stage like A1 — `prepare_semantic_cycle` (cluster episodic+L1 vectors with `hdbscan`, group by cluster, idempotency-filter by `topic_key`), `generate_one_semantic_memory` (one LLM call per topic cluster, governed by H1 quota), `apply_semantic_result` (`insert_semantic_memory` + `mark_memory_superseded` for refreshed topics).
- [ ] `state.rs::start_memory_consolidator`: after the episodic pass, run the semantic pass, then `demote_cold_memories`. Each LLM call still checks `TaskKind::MemoryConsolidation` quota individually.
- [ ] Embed new semantic summaries → `memory_vectors`.
- [ ] **Files**: `memory/semantic.rs` (new), `state.rs`.
- [ ] **Tests**: clustering groups same-topic episodics; `topic_key` idempotent across reruns; membership change → new row + old `superseded_by` set; demotion only touches old+covered; LLM-quota-exhausted mid-cycle defers cleanly (mirror A1 test); semantic pass with < `MIN_MEMS_PER_TOPIC` skips.
- [ ] **Acceptance**: after running episodic + semantic passes on the golden corpus, `kind='semantic'` rows exist and are searchable; `attune --diag` shows tier counts.

### Phase 4 — Tier-aware assembler + history compaction (the token-saving wire-up)

- [ ] `memory/assembler.rs`: `classify_query_shape` (reuse `parse_time_filter`, `detect_query_domain`, word-count); `assemble_context` (§3.2 algorithm); `compact_history` (§3.3, cache rolling summary in `chunk_summaries` with `conv:` id).
- [ ] `chat.rs::ChatEngine::search_for_context` and `routes/chat.rs`: when `memory.tiered_assembler_enabled`, route through `assemble_context` instead of bare `search_with_context` + `compress_batch`. Coverage gate falls back to current path.
- [ ] J5 secondary retrieval: on `confidence < 3`, force `shape=Precise` (full L0).
- [ ] Add `tier_used` + `est_tokens` to `ChatResponse` / chat JSON; surface in the UI cost chip tooltip ("context: L2 memory" / "L0 raw").
- [ ] Settings: wire 4 `memory.*` keys; default `tiered_assembler_enabled=true` only after Phase-4 tests pass.
- [ ] `store/chunk_summaries.rs`: `delete_conv_summaries(session_id)` for session delete.
- [ ] **Files**: `memory/assembler.rs` (new), `chat.rs`, `routes/chat.rs`, `state.rs`, `store/chunk_summaries.rs`, settings.
- [ ] **Tests**: `classify_query_shape` table-driven (recall/overview/precise cases); `assemble_context` picks L2 for recall when memory hit ≥ confidence, falls back to L0 when weak; precise query never loses L0; `compact_history` produces one cached rolling summary, second call is a cache hit; assembler-off == today's output byte-for-byte.
- [ ] **Acceptance**: token-reduction benchmark (§5.3) shows ≥ 30% median injected-token cut on recall+overview, ≤ 0% on precise, golden-set correctness unchanged.

### Phase 5 — Tests, docs, hardening

- [ ] New `tests/multilayer_memory_integration.rs`: full L0→L1→L2→L3 lifecycle on the GitHub golden corpus; recall/overview/precise query routing; cold demotion; assembler on/off equivalence on precise queries.
- [ ] Extend `tests/memory_moat_integration.rs` with tier-retrieval cases (the moat now includes tiered memory).
- [ ] `rag_quality_benchmark.rs`: add the injected-token-count metric, assembler on/off columns.
- [ ] `docs/TESTING.md`: document the multi-layer memory test matrix under the memory-moat section.
- [ ] `README.md` / `DEVELOP.md` / `RELEASE.md`: describe the tiered memory + token-reduction feature (OSS, free).
- [ ] `tests/MANUAL_TEST_CHECKLIST.md`: add manual acceptance items (ask a recall question, verify cost chip shows "L2 memory").
- [ ] **Acceptance**: full suite green on Linux x86_64; cross-platform `cfg` clean; `cargo clippy` clean.

---

## 8. Design Decisions Needing User Input Before Building

1. **Default `tiered_assembler_enabled = true`?** Recommended on (it's the feature), but it changes chat context shape for every user. Alternative: ship Phase 1–3 silently, default the assembler *off* for one release, flip on in the next after telemetry. **Need a call.**
2. **Semantic (L3) clustering cadence.** L2→L3 with `hdbscan` every 6h is cheap CPU-wise but each topic cluster is one LLM call. With many topics that is several tier-3 calls per cycle. Cap with `MAX_TOPICS_PER_CYCLE` (propose 4, same as A1's `MAX_BUNDLES_PER_CYCLE`)? Or run L3 less often (daily)? **Need a cadence/cap decision.**
3. **Cold-archive age.** `COLD_AGE_SECS` default 180 days. For a heavy daily user that may be too long (hot set grows); for a light user too short. Make it硬-coded default + Settings override (current plan), or硬-code only? **Confirm 180d default.**
4. **History rolling summary in `chunk_summaries` with synthetic `conv:` item_id** — pragmatic reuse vs. a clean dedicated `conversation_summaries` table. Reuse avoids a migration but slightly overloads the table's semantics. **Confirm reuse is acceptable** (the alternative is one more small table).
5. **Memory retrieval as a separate tiny `usearch` index vs. tagging memory vectors into the main document `VectorIndex`.** Separate index keeps memory ranking from polluting document search and is simpler to rebuild; mixed index avoids a second index object. Plan picks **separate**. **Confirm.**
6. **Embedding model drift.** Episodic/semantic memories are embedded with whatever model is active at build time. If the user later switches embedding models, `memory_vectors` become dimension-mismatched. A1 chunk vectors have the same latent issue. Plan: store `model`+`dim`, skip mismatched vectors at load (graceful degrade → those memories just aren't vector-retrievable until re-embedded). **Confirm graceful-skip is acceptable** vs. a forced re-embed migration.

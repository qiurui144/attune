# Memory Consolidation (A1) MVP Design

**Date**: 2026-04-27
**Roadmap**: 12-week strategy v2, Phase 1 W1 F-P0a
**Depends on**: H1 resource_governor (TaskKind::MemoryConsolidation already defined)
**Depended by**: future A2 conflict detection, B2 project-aware chat

[English](2026-04-27-memory-consolidation-design.md) · [简体中文](2026-04-27-memory-consolidation-design.zh.md)

---

## 1. Why

Attune's "self-evolving memory" positioning (mem0 reference) requires a layer above raw chunks: aggregated *episodic* memories that summarize what the user encountered/learned over a time window. Without consolidation, chat retrieval can only see chunk-level fragments; with it, "what did I learn last week?" becomes a single retrieval hit.

This is **MVP scope** — a foundational data model + a working consolidator. Semantic memory (topic-clustered), conflict detection (A2), and chat retrieval integration are explicitly deferred to W5+.

## 2. MVP Scope

| In W1 | Deferred (W5+) |
|-------|---------------|
| Episodic memory: time-window aggregation (default 1-day window) | Semantic memory: topic/concept clustering across time |
| Source: `chunk_summaries` table (already has 150-char summaries) | Source: raw chunks (semantic needs more text) |
| 6-hour worker cycle, 1 LLM call per window bundle | Reactive consolidation on every chunk insert |
| Idempotent: same chunk-hash set → same memory (no duplicates) | Hierarchical memory (memories of memories) |
| Three-stage lock release (mirror skill_evolution) | Real-time conflict detection (A2) |
| H1 governor + LLM quota integration | Chat retrieval surfaces memories (B2) |

## 3. Schema

```sql
-- 加密的"周期总结"记忆。源 chunk_hash 集合作为幂等键。
CREATE TABLE IF NOT EXISTS memories (
    id                    TEXT PRIMARY KEY,
    kind                  TEXT NOT NULL CHECK(kind IN ('episodic')),  -- 'semantic' added in W5+
    window_start          INTEGER NOT NULL,  -- unix epoch seconds
    window_end            INTEGER NOT NULL,
    source_chunk_hashes   TEXT NOT NULL,     -- JSON array of chunk_hash, sorted ascending
    source_chunk_count    INTEGER NOT NULL,  -- == len(source_chunk_hashes), denormalized for indexing
    summary_encrypted     BLOB NOT NULL,     -- AES-GCM(summary text), DEK-encrypted
    model                 TEXT NOT NULL,     -- LLM model used (debug provenance)
    created_at            INTEGER NOT NULL   -- unix epoch seconds
);
CREATE INDEX IF NOT EXISTS idx_memories_window ON memories(window_start, window_end);
CREATE INDEX IF NOT EXISTS idx_memories_created ON memories(created_at DESC);
-- 幂等性键：同一组 chunk 不重复生成 memory
CREATE UNIQUE INDEX IF NOT EXISTS uq_memories_source ON memories(kind, source_chunk_hashes);
```

**No backfill migration needed** — new table, additive change. Existing vaults pick it up at next open without data loss.

## 4. Consolidator API

```rust
// rust/crates/attune-core/src/memory_consolidation.rs

pub const DEFAULT_WINDOW_SECS: u64 = 24 * 3600;       // 1-day windows
pub const MIN_CHUNKS_PER_BUNDLE: usize = 5;           // skip windows with < 5 chunks (signal too thin)
pub const MAX_CHUNKS_PER_BUNDLE: usize = 50;          // LLM prompt budget cap
pub const MAX_BUNDLES_PER_CYCLE: usize = 4;           // protect against thundering 24-LLM-call cycles

/// Per-window bundle of chunks ready for consolidation.
pub struct ConsolidationBundle {
    pub window_start: i64,
    pub window_end: i64,
    pub chunks: Vec<BundleChunk>,
}

pub struct BundleChunk {
    pub chunk_hash: String,
    pub summary: String,   // already 150-char summary from chunk_summaries
    pub item_id: String,
}

/// Phase 1 (vault locked): scan chunk_summaries for un-consolidated windows; bucket by day.
/// Returns None when no eligible bundles (idle cycle).
pub fn prepare_consolidation_cycle(
    store: &Store,
    dek: &Key32,
    now_secs: i64,
) -> Result<Option<Vec<ConsolidationBundle>>>;

/// Phase 2 (no lock): one LLM call per bundle. Returns one summary per bundle (or None on failure).
pub fn generate_episodic_memories(
    llm: &dyn LlmProvider,
    bundles: &[ConsolidationBundle],
) -> Vec<Option<String>>;  // parallel to bundles; None = LLM failed for that bundle

/// Phase 3 (vault locked): write memories with idempotent INSERT. Returns count of new rows.
pub fn apply_consolidation_result(
    store: &Store,
    dek: &Key32,
    bundles: &[ConsolidationBundle],
    summaries: &[Option<String>],
    model: &str,
) -> Result<usize>;

/// Convenience for tests: full single-cycle.
pub fn run_consolidation_cycle(
    store: &Store,
    dek: &Key32,
    llm: &dyn LlmProvider,
    now_secs: i64,
    model: &str,
) -> Result<usize>;
```

## 5. Worker (attune-server)

```rust
pub fn start_memory_consolidator(state: Arc<AppState>) {
    if state.llm.lock().unwrap_or_else(|e| e.into_inner()).is_none() { return; }
    if state.memory_consolidator_running.compare_exchange(false, true, ...).is_err() { return; }

    let governor = global_registry().register(TaskKind::MemoryConsolidation);
    const CYCLE: Duration = Duration::from_secs(6 * 3600);

    std::thread::spawn(move || loop {
        std::thread::sleep(CYCLE);
        if vault_locked { break; }
        if !governor.should_run() { continue; }

        // Phase 1 (locked)
        let (bundles, dek) = { /* prepare_consolidation_cycle */ };
        if bundles.is_empty() { continue; }

        // LLM quota check
        if !governor.allow_llm_call() {
            tracing::info!("Memory consolidator LLM quota exceeded, skipping");
            continue;
        }

        // Phase 2 (no lock) — N LLM calls, one per bundle
        let summaries = generate_episodic_memories(llm, &bundles);

        // Phase 3 (locked) — idempotent INSERT
        let _ = apply_consolidation_result(store, &dek, &bundles, &summaries, model);
    });
}
```

**Quota note**: `allow_llm_call()` reserves one slot per cycle, but Phase 2 may call LLM N times (one per bundle, up to 4). MVP treats this as "best-effort one reservation per cycle" — full per-call accounting deferred to W5+ along with retryable LLM failures.

## 6. Idempotency

The unique index `uq_memories_source(kind, source_chunk_hashes)` is the hard guarantee. Algorithm: sort chunk_hashes ascending → JSON-encode as canonical key → `INSERT OR IGNORE`. Re-running same bundle returns 0 new rows, never duplicates.

This sidesteps the tricky "mark chunks consolidated" approach, which would require a second table + race conditions on partial failures.

## 7. Tests

**Unit (`memory_consolidation.rs::tests`)**:
- `prepare returns None when no chunks` (empty store)
- `prepare buckets chunks by day boundary` (insert chunks at t1, t2 spanning a day → 2 bundles)
- `prepare skips windows below MIN_CHUNKS_PER_BUNDLE`
- `prepare caps at MAX_BUNDLES_PER_CYCLE`
- `generate handles MockLlm returning fixed summary`
- `apply is idempotent` (same bundle twice → 1 row)
- `apply with None summary skips that bundle`

**Integration (`tests/memory_consolidation_integration.rs`)**:
- Full cycle with real Store + tempfile + MockLlmProvider
- Verify `memories` table populated with expected count + chunk_hash set

## 8. What's NOT Done in MVP (Explicit)

- ❌ Semantic memory (topic clustering across time)
- ❌ Chat retrieval surfaces memories (B2 in W5)
- ❌ Conflict detection between memories (A2 in W5)
- ❌ Per-LLM-call quota accounting (1 reservation per cycle suffices for 6h cycle)
- ❌ User-facing memories UI (F1 Profile visualization will surface in W4)
- ❌ Memory export/import (B5 conversation export covers similar ground in W11)
- ❌ Adaptive window sizing (fixed 1-day window MVP)

## 9. W1 Acceptance Checklist

- [ ] `memories` table created in fresh vault open
- [ ] `cargo test -p attune-core memory_consolidation::` all green
- [ ] `cargo test --test memory_consolidation_integration` all green
- [ ] `attune-server` builds with `start_memory_consolidator` referenced
- [ ] Manual: with MockLlm, run cycle on a vault with 10 chunks across 2 days → 2 memories created; rerun → 0 new
- [ ] `rust/RELEASE.md` + `rust/DEVELOP.md` entries
- [ ] `docs/superpowers/specs/2026-04-27-memory-consolidation-design.md` + `.zh.md`
- [ ] git commit + push develop, report SHA

# W3 Batch A Design (C1 + F1 + F2 + F4)

**Date**: 2026-04-27
**Roadmap**: 12-week strategy v4, Phase 1 W3 F-P0c
**Depends on**: W2 batch 1 (commit `f40b811`), H1/A1 commits
**Depended by**: J6 W4 benchmark (citation breadcrumb shows in golden set), W3 batch B G1/G5 Chrome 扩展, W3 batch C K2 parse golden set

[English](2026-04-27-w3-batch-a-design.md) · [简体中文](2026-04-27-w3-batch-a-design.zh.md)

---

## 1. Why this batch

W2 batch 1 left **B1 backend with placeholder fields** (`Citation.breadcrumb=Vec::new()`, `chunk_offset_*=None`) — all consumers (server routes, future frontend, J6 benchmark output) currently get empty values. Closing the placeholder is the highest-priority W3 task; otherwise downstream code accumulates "always-empty" workarounds that become technical debt.

W3 batch A bundles **4 backend-only items** that don't depend on Chrome extension or Tauri frontend:

| Item | Type | Estimate | Source |
|------|------|----------|--------|
| **F1** Web fallback observability log | small fix | 1-line | reviewer P1 #1 |
| **F4** RELEASE.md known-limitation note | docs | 5 min | reviewer P1 |
| **C1** Web search local cache | new feature P0 | medium | 吴师兄 §6 + Readwise pattern |
| **F2** Breadcrumb 透传 indexer→Citation | bridge | medium | reviewer F2 (closes W2 placeholder) |

W3 batch B (G1/G5 Chrome 扩展) and batch C (K2 parse golden set) handled separately.

## 2. F1 + F4 — small fixes

### F1: chat.rs:128 add observability log
```rust
log::info!(
    "J5: secondary retrieval result — local_was_empty={}, broader_count={}",
    knowledge.is_empty(), broader.len()
);
```
This lets ops distinguish "fallback retrieved more docs" vs "fallback equally empty" in production logs.

### F4: rust/RELEASE.md
Update W2 batch 1 entry: change "Known limitation: ... populated in W3 batch 2" to **"RESOLVED in W3 batch A (commit `<sha>`)"** referring to F2 below.

## 3. C1 — Web search local cache

### Why
Per 吴师兄 §6 "高频 query 缓存" + attune cost contract §2 (web search is paid network call). A cache layer turns repeated identical queries into 🆓 lookups.

### Design

**New table** (`store/mod.rs` SCHEMA_SQL):
```sql
-- C1 Web search cache (W3 batch A, 2026-04-27)
-- 加密存储 web_search 结果。query_hash = SHA-256(query) 作为查找键；
-- query_text + results 字段 DEK 加密。30 天 TTL，过期由查询时过滤（不主动 GC）。
CREATE TABLE IF NOT EXISTS web_search_cache (
    query_hash       TEXT PRIMARY KEY,
    query_text_enc   BLOB NOT NULL,
    results_json_enc BLOB NOT NULL,  -- AES-GCM(serde_json::to_string(&Vec<WebSearchResult>))
    created_at_secs  INTEGER NOT NULL,
    ttl_secs         INTEGER NOT NULL DEFAULT 2592000  -- 30 days
);
CREATE INDEX IF NOT EXISTS idx_web_cache_created ON web_search_cache(created_at_secs);
```

**New module**: `rust/crates/attune-core/src/web_search_cache.rs`

```rust
use crate::crypto::Key32;
use crate::store::Store;
use crate::web_search::WebSearchResult;

pub const DEFAULT_TTL_SECS: i64 = 30 * 24 * 3600;

impl Store {
    /// Cache miss returns None; expired entries also return None (lazily filtered).
    pub fn get_web_search_cached(
        &self,
        dek: &Key32,
        query: &str,
        now_secs: i64,
    ) -> Result<Option<Vec<WebSearchResult>>>;

    pub fn put_web_search_cached(
        &self,
        dek: &Key32,
        query: &str,
        results: &[WebSearchResult],
        ttl_secs: i64,
        now_secs: i64,
    ) -> Result<()>;

    pub fn web_search_cache_count(&self) -> Result<usize>;

    /// 显式 GC（用户在 Settings 点 "清空 web 缓存" 时调）
    pub fn clear_web_search_cache(&self) -> Result<usize>;
}
```

### Integration point

`chat.rs` web search fallback path (current code):
```rust
match ws.search(user_message, 3) {
    Ok(web_results) if !web_results.is_empty() => { ... }
}
```

Becomes:
```rust
let cached = self.store.lock()?.get_web_search_cached(dek, user_message, now_secs)?;
let web_results = match cached {
    Some(hits) => {
        log::info!("C1: web search cache HIT for query (saved network call)");
        hits
    }
    None => {
        let fresh = ws.search(user_message, 3)?;
        self.store.lock()?.put_web_search_cached(dek, user_message, &fresh, DEFAULT_TTL_SECS, now_secs)?;
        fresh
    }
};
```

### Tests

- Cache miss → put → hit (deterministic)
- Cache expired (TTL elapsed) → returns None
- Different queries don't collide (SHA-256 collision improbable, but verify hash key uniqueness)
- Encrypted at rest (raw `results_json_enc` blob ≠ JSON text)
- `clear_web_search_cache` returns deleted count

## 4. F2 — Breadcrumb 透传 indexer → Citation

### Why this is critical

W2 batch 1 added `Citation.breadcrumb` + `chunk_offset_*` fields **as API shape only** (always empty). Without F2, every consumer must do `if breadcrumb.is_empty() { fallback }`, becoming a permanent technical debt.

### Design choice: minimal-invasion table

**Avoided**: extending `embed_queue` schema + `VectorMeta` serde fields. That would force migrating all 4 enqueue call sites + breaking `.encbin` backwards-compat for existing vaults.

**Chosen**: a **separate side table** `chunk_breadcrumbs` keyed on `(item_id, chunk_idx)`. Indexer pipeline writes when chunks are sectioned; ChatEngine joins on lookup. Old vaults without entries return empty breadcrumb gracefully.

```sql
-- F2 Chunk breadcrumb metadata (W3 batch A, 2026-04-27)
-- 独立于 embed_queue 的辅助表 — 避免改 VectorMeta serde / embed_queue 列。
-- 老 vault 无此表 → 升级时 IF NOT EXISTS 创建空表 → ChatEngine 查不到时返回空 Vec。
-- breadcrumb 是 chunker SectionWithPath.path 的 JSON 序列化（升序数组）。
-- offset_start/end 是 chunk 在原文 item.content 中的 char-level 区间。
CREATE TABLE IF NOT EXISTS chunk_breadcrumbs (
    item_id          TEXT NOT NULL,
    chunk_idx        INTEGER NOT NULL,
    breadcrumb_json  TEXT NOT NULL,         -- JSON Vec<String>，明文（不含敏感数据）
    offset_start     INTEGER NOT NULL,
    offset_end       INTEGER NOT NULL,
    PRIMARY KEY (item_id, chunk_idx)
);
CREATE INDEX IF NOT EXISTS idx_chunk_breadcrumbs_item ON chunk_breadcrumbs(item_id);
```

**Note on encryption**: breadcrumb is heading text (e.g. "公司手册 > 第三章 福利") — these come from document structure, not from secret content. Decision: store plaintext for query simplicity. If a future user has top-secret heading hierarchies, F2 v2 can encrypt with same chunk_summaries pattern.

### Indexer pipeline change

4 call sites currently use `chunker::extract_sections` (returns `(idx, content)`). Strategy:

**No-touch approach**: Don't change those 4 sites. Instead, **at section creation time** (where caller already has access to original document content), generate breadcrumbs by re-running `extract_sections_with_path` on the same content and writing `chunk_breadcrumbs` rows.

For the four call sites:
- `routes/upload.rs:99` — has `content`
- `routes/ingest.rs:88` — has `body.content`
- `scanner.rs:131` — has `content` from file read
- `scanner_webdav.rs:288` — has `content` from WebDAV

We add a single helper:
```rust
// in store/chunk_breadcrumbs.rs
impl Store {
    pub fn upsert_chunk_breadcrumbs_from_content(
        &self,
        item_id: &str,
        content: &str,
    ) -> Result<usize> {
        // Run extract_sections_with_path once; compute offsets; bulk INSERT OR REPLACE.
    }
}
```

Each call site adds **one line** after `extract_sections`:
```rust
let _ = store.upsert_chunk_breadcrumbs_from_content(&item_id, &content);
```

### SearchResult + ChatEngine integration

`SearchResult` (already in search.rs) is enriched after item decryption — add lookup:

```rust
// In search_with_context, after item decryption:
let breadcrumb_data = ctx.store
    .get_chunk_breadcrumb(&item.id, chunk_idx)
    .ok().flatten()
    .unwrap_or_default();
results.push(SearchResult {
    item_id: item.id,
    score: *score,
    title: item.title,
    content: item.content,
    source_type: item.source_type,
    inject_content: None,
    breadcrumb: breadcrumb_data.0,
    chunk_offset_start: Some(breadcrumb_data.1),
    chunk_offset_end: Some(breadcrumb_data.2),
});
```

**Caveat**: current `SearchResult` doesn't track which chunk in the item matched (we have only `item_id`). For F2 v1, we use the **first chunk's breadcrumb** as a heuristic — accurate enough for top-level navigation, refined in W5+ when indexer tracks per-chunk hits.

`ChatEngine.chat()` then maps `SearchResult.breadcrumb` → `Citation.breadcrumb`, `SearchResult.chunk_offset_*` → `Citation.chunk_offset_*`.

### Tests

- `upsert_chunk_breadcrumbs_from_content` writes correct rows for nested markdown (use existing chunker test corpus)
- Lookup returns `(Vec, start, end)` for known item + chunk_idx
- Lookup returns None for unknown
- ChatEngine integration: chat() with seeded item containing breadcrumbs → Citation.breadcrumb non-empty + offsets present

## 5. Acknowledgments

Per `ACKNOWLEDGMENTS.md` policy:
- **C1 cache**: 吴师兄 §6 ("高频 query 做缓存") + general Readwise/Linkwarden pattern of "snapshot at fetch time"
- **F2 side-table approach**: attune internal design (avoid embed_queue migration risk); inspired by attune's existing chunk_summaries table that successfully uses content-keyed sidecar storage
- **F1 logging philosophy**: standard SRE observability (no specific external source)

## 6. Out of scope

- ❌ J5 secondary retrieval E2E test (F3) — requires full ChatEngine constructor with seeded vault, defer to batch B
- ❌ K2 Parse Golden Set 200 篇 — requires corpus collection + CI pipeline, defer to batch C
- ❌ G1 / G5 Chrome 扩展 — full-stack browser work, separate session
- ❌ Cache GC daemon — F2 v1 lazy-expire on read; W5+ adds H7 background GC if needed
- ❌ Per-chunk SearchResult precision (which chunk matched) — heuristic "first chunk" sufficient for now

## 7. Acceptance Checklist

- [ ] F1: chat.rs:128 region has new log statement
- [ ] F4: rust/RELEASE.md W2 batch 1 entry updated to RESOLVED
- [ ] C1: `cargo test -p attune-core --lib web_search_cache` green
- [ ] C1: `chat.rs` web fallback uses cache (test exercises both miss + hit)
- [ ] F2: `cargo test -p attune-core --lib chunk_breadcrumbs` green
- [ ] F2: integration test: ChatEngine.chat() returns Citation with breadcrumb non-empty when item has structured headings
- [ ] Full lib regression: 415+11 → 415+N+11+M, 0 failures
- [ ] R1 + R2 code review pass
- [ ] ACKNOWLEDGMENTS.md updated for C1 (吴师兄 + Readwise reference)
- [ ] git commit with Inspired-by lines + push develop

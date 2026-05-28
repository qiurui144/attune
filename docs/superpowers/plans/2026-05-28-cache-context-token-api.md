# Cache / Context / Token Standard API Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Collapse attune's scattered `cost.rs` + `context_budget.rs` + `web_search_cache.rs` + ad-hoc LLM/embed usage tracking into a single `attune-core::usage` + `attune-core::cache` Rust API and a standard REST surface (`/api/v1/usage/*` + `/api/v1/cache/*` + `X-Attune-*` response headers + UI Usage tab + ChatSendBar cost chip).

**Architecture:** New crate-internal modules `attune-core::usage` (`TokenUsage` / `UsageEvent` / `UsageRecorder` / `UsageRecorderGuard` Drop guard for compile-time enforcement) and `attune-core::cache` (`CacheBackend` trait + `MemoryLruCache` L1 + `SqliteEncryptedCache` L2). LLM/Embed `chat()` / `embed()` return types refactored to carry `TokenUsage` so every call site is forced (by compile-time `Result<(R, TokenUsage), _>`) to flow into the recorder. New `attune-core::store::usage` + `attune-core::store::cache` DB modules with their own `CREATE TABLE IF NOT EXISTS` blocks appended to `store::mod.rs::SCHEMA_SQL`. Axum middleware `usage_headers` injects `X-Attune-Cache/Token-In/Cost-USD` automatically.

**Tech Stack:** Rust (axum 0.8, tokio, rusqlite + AES-256-GCM via attune `crypto.rs`, async_trait, blake3 for cache keys, serde, tracing), proptest for property tests, criterion for record-event bench. Frontend: TypeScript (React/Preact UI in `rust/crates/attune-server/ui/src/`), no new runtime deps.

**Spec reference:** `docs/superpowers/specs/2026-05-28-cache-context-token-standard-api.md`
**Target version:** v1.0.6 (6/05)
**Branch:** `feat/cache-token-api` (cut from `develop`)
**Worktree:** `/tmp/attune-cache-token-api/` (per CLAUDE.md §5.1 large-feature isolation)

**Cross-plan coupling:** Plan A2 (`2026-05-28-hybrid-token-routing.md`) blockedBy this plan's **Task M** (UsageEvent schema + UsageRecorderGuard public API frozen; A2 routing decisions consume `UsageEvent` stream). Do **not** start A2 until Task M is merged to develop.

---

## File Structure

### Create (Rust)

| Path | Responsibility |
|------|----------------|
| `rust/crates/attune-core/src/usage/mod.rs` | Public exports + `UsageRecorder` trait + `record_event` glue |
| `rust/crates/attune-core/src/usage/types.rs` | `TokenUsage`, `CacheOutcome`, `CallOutcome`, `UsageEvent`, `UsageKind`, `ErrorKind` enums + serde |
| `rust/crates/attune-core/src/usage/guard.rs` | `UsageRecorderGuard` Drop guard — panics in debug if not consumed; warns in release |
| `rust/crates/attune-core/src/usage/aggregator.rs` | `UsageAggregator` — ring buffer + 100ms batch flush to SQLite + recent-events accessor for routing |
| `rust/crates/attune-core/src/cache/mod.rs` | `CacheBackend` async trait + `CacheScope` enum + `CachedValue` struct |
| `rust/crates/attune-core/src/cache/memory.rs` | `MemoryLruCache` (L1) — 512 entries / 64MB LRU, in-process |
| `rust/crates/attune-core/src/cache/sqlite_encrypted.rs` | `SqliteEncryptedCache` (L2) — AES-256-GCM via DEK, llm_cache + embed_cache tables |
| `rust/crates/attune-core/src/cache/key.rs` | `CacheKey` builder — `blake3(model + prompt)` 32-hex |
| `rust/crates/attune-core/src/store/usage.rs` | DB CRUD: `record_usage`, `query_summary`, `query_events`, `purge_old`, `reset_all` |
| `rust/crates/attune-core/src/store/cache.rs` | DB CRUD: `cache_get`, `cache_put`, `cache_count`, `cache_clear`, `cache_gc_lru` |
| `rust/crates/attune-server/src/routes/usage.rs` | `GET /api/v1/usage/{summary,events}` + `POST /api/v1/usage/reset` |
| `rust/crates/attune-server/src/routes/cache.rs` | `GET/DELETE /api/v1/cache/{llm,embed,search,all}` — merges legacy `web_search_cache` |
| `rust/crates/attune-server/src/middleware/usage_headers.rs` | `tower::Layer` that auto-injects `X-Attune-Cache/Token-In/Cost-USD/Latency/Provider/Model` from request extensions |
| `rust/crates/attune-server/ui/src/views/UsageView.tsx` | Settings → Usage tab: 7d/30d summary + by-provider/agent bars + Reset button |
| `rust/crates/attune-server/ui/src/components/CostChip.tsx` | Reusable chip `~1.2K tok · $0.0004 · ⚡ cache 67%` |

### Create (tests)

| Path | Responsibility |
|------|----------------|
| `rust/crates/attune-core/src/usage/tests/golden.rs` | ≥10 golden fixtures (10 real LLM call sequences expected UsageEvent rows) |
| `rust/crates/attune-core/src/usage/tests/proptest.rs` | proptest: `tokens_in+out ≥ tokens_in`, `cost ≥ 0`, cache state machine non-reversible |
| `rust/crates/attune-core/src/cache/tests/lru.rs` | L1 eviction, hit/miss counters |
| `rust/crates/attune-core/src/cache/tests/encrypted.rs` | L2 round-trip with DEK; raw SQLite blob is ciphertext |
| `rust/crates/attune-core/src/store/usage_test.rs` | inline `#[cfg(test)]` — purge_old, summary aggregation 100k rows |
| `rust/crates/attune-core/tests/usage_endtoend.rs` | integration: subprocess agent → record → query summary |
| `rust/crates/attune-server/tests/usage_routes.rs` | HTTP: GET /usage/summary, /usage/events, POST /usage/reset, headers injected on /chat |
| `rust/crates/attune-server/tests/cache_routes.rs` | HTTP: scope=llm/embed/search/all, deprecated alias warning header |
| `rust/crates/attune-server/ui/src/views/__tests__/UsageView.test.tsx` | Vitest: locale switching, 0-cost hides line, vault-locked placeholder |

### Modify

| Path | Change |
|------|--------|
| `rust/crates/attune-core/src/lib.rs` | `pub mod usage; pub mod cache;` |
| `rust/crates/attune-core/src/llm.rs` | `LlmProvider::chat` signature `Result<(String, TokenUsage), _>` (breaking change — every impl + caller updates) |
| `rust/crates/attune-core/src/embed.rs` | `EmbeddingProvider::embed` returns `Result<(Vec<Vec<f32>>, TokenUsage), _>` |
| `rust/crates/attune-core/src/cost.rs` | Add `pricing_for_provider(provider, model)` overriding `lookup_pricing` for cloud-gateway/BYOK split |
| `rust/crates/attune-core/src/context_budget.rs` | `BudgetPlan` add `tokens_in_used: usize`; `plan_context` populates it |
| `rust/crates/attune-core/src/store/mod.rs` | Append `usage_events`, `llm_cache`, `embed_cache` to `SCHEMA_SQL`; bump `vault_meta.key='usage_schema_version'` to `1` |
| `rust/crates/attune-core/src/store/web_search_cache.rs` | Rename internal helpers; rewrite to use new `cache.rs` module under hood; legacy fn names kept as thin wrappers (deprecated) |
| `rust/crates/attune-server/src/state.rs` | Add `usage_aggregator: Arc<UsageAggregator>` + `cache_backend: Arc<dyn CacheBackend>` + accessor methods |
| `rust/crates/attune-server/src/lib.rs` | Wire usage_headers middleware into `/api/v1/{chat,agents,classify,search,llm}/*` routers |
| `rust/crates/attune-server/src/routes/mod.rs` | `pub mod usage; pub mod cache;` (and remove `pub mod web_search_cache;` after R rename merged) |
| `rust/crates/attune-server/src/routes/chat.rs` | Insert `request.extensions_mut().insert(UsageEvent{...})` before returning response so middleware can read |
| `rust/crates/attune-server/src/routes/web_search_cache.rs` | Stub: legacy paths return 200 + `Deprecation: true` header + redirect-instructions, body forwarded to new `/api/v1/cache/search` |
| `rust/crates/attune-server/ui/src/views/SettingsView.tsx` | Mount UsageView tab |
| `rust/crates/attune-server/ui/src/components/ChatSendBar.tsx` | Render `<CostChip />` from `/api/v1/usage/summary?from=session_start` |
| `RELEASE.md` | v1.0.6 section: Cache/Context/Token API. List breaking change `LlmProvider::chat` signature |
| `docs/superpowers/plans/2026-05-28-cache-context-token-api.md` | This file — delete after implementation merged per CLAUDE.md §3.2 lifecycle |

---

## Task A: Worktree + branch + tests-only skeleton

**Files:**
- Create: `/tmp/attune-cache-token-api/` (worktree)
- Modify: none yet

- [ ] **Step 1: Disk check**

Run: `df -h /data | head -2`
Expected: `/data` available > 50G (per CLAUDE.md §"磁盘资源管理铁律"). If < 50G, abort and clean target/ + stale worktrees first.

- [ ] **Step 2: Create isolated worktree**

```bash
cd /data/company/project/attune
git fetch origin
git worktree add /tmp/attune-cache-token-api -b feat/cache-token-api develop
cd /tmp/attune-cache-token-api
```

- [ ] **Step 3: Sanity build (clean baseline)**

Run: `cargo build --workspace 2>&1 | tail -5`
Expected: `Finished ... target(s)` (no errors). Captures pre-change baseline.

- [ ] **Step 4: Commit empty skeleton**

```bash
cd /tmp/attune-cache-token-api
mkdir -p rust/crates/attune-core/src/usage rust/crates/attune-core/src/cache
touch rust/crates/attune-core/src/usage/mod.rs rust/crates/attune-core/src/cache/mod.rs
git add -A
git commit -m "feat(usage,cache): empty module skeleton

Pre-task scaffolding for cache-token-api plan. Both mods empty,
will be filled by Task B+C following TDD red→green→refactor.

Plan: docs/superpowers/plans/2026-05-28-cache-context-token-api.md"
```

---

## Task B: `usage::types` — TokenUsage / UsageEvent / enums (TDD)

**Files:**
- Create: `rust/crates/attune-core/src/usage/types.rs`
- Modify: `rust/crates/attune-core/src/usage/mod.rs`, `rust/crates/attune-core/src/lib.rs`

- [ ] **Step 1: Write failing test for `TokenUsage` JSON round-trip**

Create `rust/crates/attune-core/src/usage/tests/mod.rs` with `mod types_test;` and write:

```rust
// rust/crates/attune-core/src/usage/tests/types_test.rs
use crate::usage::types::{TokenUsage, CacheOutcome, CallOutcome, UsageEvent, UsageKind, ErrorKind};

#[test]
fn token_usage_serializes_camelcase_for_ui() {
    let t = TokenUsage {
        tokens_in: 100,
        tokens_out: 50,
        cached_in: 20,
        model: "gemini-1.5-flash".into(),
        provider: "cloud_gateway".into(),
    };
    let j = serde_json::to_string(&t).unwrap();
    assert!(j.contains(r#""tokensIn":100"#), "wire format must be camelCase, got: {j}");
    assert!(j.contains(r#""cachedIn":20"#));
}

#[test]
fn cache_outcome_serializes_lowercase_string() {
    assert_eq!(serde_json::to_string(&CacheOutcome::Hit).unwrap(), r#""hit""#);
    assert_eq!(serde_json::to_string(&CacheOutcome::Miss).unwrap(), r#""miss""#);
    assert_eq!(serde_json::to_string(&CacheOutcome::Bypass).unwrap(), r#""bypass""#);
}

#[test]
fn call_outcome_carries_retry_attempt() {
    let r = CallOutcome::Retry { attempt: 2 };
    let j = serde_json::to_string(&r).unwrap();
    assert!(j.contains(r#""attempt":2"#));
}

#[test]
fn usage_event_round_trip() {
    let e = UsageEvent {
        ts_ms: 1717000000000,
        kind: UsageKind::LlmChat,
        usage: TokenUsage {
            tokens_in: 200, tokens_out: 80, cached_in: 0,
            model: "qwen2.5:3b".into(), provider: "ollama".into(),
        },
        cost_usd: Some(0.0),
        cache: CacheOutcome::Miss,
        outcome: CallOutcome::Ok,
        latency_ms: 320,
        agent_id: None,
        query_hash: None,
    };
    let j = serde_json::to_string(&e).unwrap();
    let back: UsageEvent = serde_json::from_str(&j).unwrap();
    assert_eq!(back.ts_ms, 1717000000000);
    assert!(matches!(back.outcome, CallOutcome::Ok));
}
```

Append `mod tests;` to `rust/crates/attune-core/src/usage/mod.rs`.

- [ ] **Step 2: Run — verify it fails**

Run: `cargo test -p attune-core usage::tests::types_test 2>&1 | tail -10`
Expected: FAIL with `cannot find type \`TokenUsage\`` (types.rs empty).

- [ ] **Step 3: Implement `types.rs`**

```rust
// rust/crates/attune-core/src/usage/types.rs
//! Standard usage types — every LLM/Embed/Rerank/OCR/ASR/VLM call records a UsageEvent.
//!
//! Spec: docs/superpowers/specs/2026-05-28-cache-context-token-standard-api.md §5.1
//! Wire format: serde camelCase for UI; SQLite column names snake_case for DB.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    pub tokens_in: u32,
    pub tokens_out: u32,
    /// Vendor-side prompt cache (Anthropic prompt-cache / OpenAI). 0 = unsupported.
    pub cached_in: u32,
    pub model: String,
    pub provider: String,
}

impl TokenUsage {
    pub fn empty(provider: &str, model: &str) -> Self {
        Self {
            tokens_in: 0, tokens_out: 0, cached_in: 0,
            provider: provider.into(), model: model.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheOutcome {
    Hit,
    Miss,
    Bypass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Parse,
    Grounding,
    Timeout,
    Quota,
    Network,
    SchemaInvalid,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CallOutcome {
    Ok,
    Retry { attempt: u8 },
    Fail { error_kind: ErrorKind },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageKind {
    LlmChat,
    LlmExtract,
    Embed,
    Rerank,
    Ocr,
    Asr,
    Vlm,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageEvent {
    pub ts_ms: i64,
    pub kind: UsageKind,
    #[serde(flatten)]
    pub usage: TokenUsage,
    pub cost_usd: Option<f64>,
    pub cache: CacheOutcome,
    pub outcome: CallOutcome,
    pub latency_ms: u32,
    pub agent_id: Option<String>,
    /// BLAKE3 16-hex prefix; None unless `settings.usage.log_queries = true`.
    pub query_hash: Option<String>,
}
```

Wire it in `rust/crates/attune-core/src/usage/mod.rs`:

```rust
//! Usage telemetry — see types.rs.
pub mod types;
pub use types::*;

#[cfg(test)]
mod tests;
```

Add `pub mod usage;` to `rust/crates/attune-core/src/lib.rs`.

- [ ] **Step 4: Run — verify it passes**

Run: `cargo test -p attune-core usage::tests::types_test 2>&1 | tail -5`
Expected: `test result: ok. 4 passed`. Note: `#[serde(flatten)]` on `usage` field changes the JSON — re-check first test: the assertion `"tokensIn":100` must still match because flatten lifts inner fields up. Run; if fail, replace `#[serde(flatten)] pub usage` with explicit `pub usage: TokenUsage` (no flatten) and update test expectation to `"usage":{...}`. Pick one and commit either.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/usage/ rust/crates/attune-core/src/lib.rs
git commit -m "feat(usage): TokenUsage / UsageEvent / enums with serde round-trip

Spec: 2026-05-28-cache-context-token-standard-api.md §5.1
Tests: 4 unit tests covering JSON wire format + retry attempt."
```

---

## Task C: `cache::CacheBackend` trait + types (TDD)

**Files:**
- Create: `rust/crates/attune-core/src/cache/mod.rs`

- [ ] **Step 1: Write failing test**

```rust
// rust/crates/attune-core/src/cache/tests/mod.rs
mod trait_test;

// rust/crates/attune-core/src/cache/tests/trait_test.rs
use crate::cache::{CacheScope, CachedValue, cache_key};

#[test]
fn cache_key_is_blake3_32_hex_lowercase() {
    let k = cache_key("gpt-4o-mini", "hello world");
    assert_eq!(k.len(), 32, "32-hex prefix of blake3");
    assert!(k.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
}

#[test]
fn cache_key_changes_with_model() {
    let a = cache_key("gpt-4o", "x");
    let b = cache_key("gpt-4o-mini", "x");
    assert_ne!(a, b, "different models must produce different keys");
}

#[test]
fn cached_value_holds_tokens_metadata() {
    let v = CachedValue {
        bytes: b"hello".to_vec(),
        tokens_in: 10,
        tokens_out: 5,
        model: "gpt-4o-mini".into(),
    };
    assert_eq!(v.bytes, b"hello");
    assert_eq!(v.tokens_in, 10);
}

#[test]
fn cache_scope_serializes_lowercase() {
    assert_eq!(serde_json::to_string(&CacheScope::Llm).unwrap(), r#""llm""#);
    assert_eq!(serde_json::to_string(&CacheScope::All).unwrap(), r#""all""#);
}
```

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p attune-core cache::tests 2>&1 | tail -10`
Expected: FAIL with unresolved imports.

- [ ] **Step 3: Implement trait + types**

```rust
// rust/crates/attune-core/src/cache/mod.rs
//! Unified cache contract — L1 in-memory + L2 SQLite encrypted.
//!
//! Spec: 2026-05-28-cache-context-token-standard-api.md §5.1 + §3 (Cache layers).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub mod key;
pub mod memory;
pub mod sqlite_encrypted;

pub use key::cache_key;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheScope {
    Llm,
    Embed,
    Search,
    All,
}

#[derive(Debug, Clone)]
pub struct CachedValue {
    pub bytes: Vec<u8>,
    pub tokens_in: u32,
    pub tokens_out: u32,
    pub model: String,
}

#[async_trait]
pub trait CacheBackend: Send + Sync {
    async fn get(&self, scope: CacheScope, key: &str) -> Option<CachedValue>;
    async fn put(&self, scope: CacheScope, key: &str, value: CachedValue, ttl_secs: Option<u32>);
    async fn clear(&self, scope: CacheScope) -> usize;
    async fn count(&self, scope: CacheScope) -> usize;
}
```

```rust
// rust/crates/attune-core/src/cache/key.rs
//! BLAKE3 cache key derivation.
//!
//! 32-hex prefix of `blake3(model || 0xFF || prompt)`. 64 bits collision domain
//! sufficient for vault-scoped cache (per spec §7.2 collision behavior = treat as miss).

use blake3::Hasher;

pub fn cache_key(model: &str, prompt: &str) -> String {
    let mut h = Hasher::new();
    h.update(model.as_bytes());
    h.update(&[0xFF]);
    h.update(prompt.as_bytes());
    let full = h.finalize().to_hex().to_string();
    full[..32].to_string()
}
```

Stub the two backend files so cargo compiles:

```rust
// rust/crates/attune-core/src/cache/memory.rs
//! L1 in-memory LRU cache. Implemented in Task E.

// rust/crates/attune-core/src/cache/sqlite_encrypted.rs
//! L2 SQLite encrypted cache. Implemented in Task F.
```

Add `pub mod cache;` to `rust/crates/attune-core/src/lib.rs`.

Add `blake3 = "1"` + `async-trait = "0.1"` to `rust/crates/attune-core/Cargo.toml` if absent. Check with `grep blake3 rust/crates/attune-core/Cargo.toml`; if missing, append:

```toml
blake3 = "1.5"
async-trait = "0.1"
```

- [ ] **Step 4: Run — verify pass**

Run: `cargo test -p attune-core cache::tests::trait_test 2>&1 | tail -5`
Expected: `test result: ok. 4 passed`.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/cache/ rust/crates/attune-core/src/lib.rs rust/crates/attune-core/Cargo.toml
git commit -m "feat(cache): CacheBackend trait + CacheScope + blake3 cache_key

Spec: 2026-05-28-cache-context-token-standard-api.md §5.1
Backends (memory.rs, sqlite_encrypted.rs) stubbed for Task E/F."
```

---

## Task D: DB schema migration — `usage_events` + `llm_cache` + `embed_cache` (TDD)

**Files:**
- Modify: `rust/crates/attune-core/src/store/mod.rs`
- Create: `rust/crates/attune-core/src/store/usage.rs`, `rust/crates/attune-core/src/store/cache.rs`

- [ ] **Step 1: Write failing test for fresh-vault schema**

```rust
// rust/crates/attune-core/src/store/usage_test.rs
use crate::store::Store;
use tempfile::TempDir;

#[test]
fn fresh_vault_has_usage_events_table() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path().join("v.db")).unwrap();
    let conn = store.raw_connection_for_test();
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='usage_events'",
        [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1, "usage_events table must exist after fresh open");
}

#[test]
fn fresh_vault_has_llm_cache_table() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path().join("v.db")).unwrap();
    let conn = store.raw_connection_for_test();
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='llm_cache'",
        [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
}

#[test]
fn fresh_vault_has_embed_cache_table() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path().join("v.db")).unwrap();
    let conn = store.raw_connection_for_test();
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='embed_cache'",
        [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
}
```

Append `mod usage_test;` to `rust/crates/attune-core/src/store/mod.rs` `#[cfg(test)]` block. If `raw_connection_for_test` doesn't exist, add it gated `#[cfg(test)]` on Store impl:

```rust
#[cfg(test)]
pub fn raw_connection_for_test(&self) -> &rusqlite::Connection {
    &self.conn
}
```

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p attune-core store::usage_test 2>&1 | tail -10`
Expected: FAIL — `no such table: usage_events`.

- [ ] **Step 3: Append schema to `SCHEMA_SQL`**

In `rust/crates/attune-core/src/store/mod.rs`, locate the `const SCHEMA_SQL: &str = r#"..."#;` block and append before the closing `"#;`:

```sql
-- ── Usage telemetry (spec 2026-05-28 cache-context-token-standard-api §3 DB tables)
CREATE TABLE IF NOT EXISTS usage_events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    ts_ms       INTEGER NOT NULL,
    kind        TEXT    NOT NULL,
    provider    TEXT    NOT NULL,
    model       TEXT    NOT NULL,
    agent_id    TEXT,
    tokens_in   INTEGER NOT NULL,
    tokens_out  INTEGER NOT NULL,
    cached_in   INTEGER NOT NULL DEFAULT 0,
    cost_usd    REAL,
    cache       TEXT    NOT NULL,
    outcome     TEXT    NOT NULL,
    latency_ms  INTEGER NOT NULL,
    error_kind  TEXT,
    query_hash  TEXT
);
CREATE INDEX IF NOT EXISTS idx_usage_ts ON usage_events(ts_ms);
CREATE INDEX IF NOT EXISTS idx_usage_kind_provider ON usage_events(kind, provider);
CREATE INDEX IF NOT EXISTS idx_usage_agent ON usage_events(agent_id) WHERE agent_id IS NOT NULL;

-- ── LLM response cache (L2, encrypted BLOB)
CREATE TABLE IF NOT EXISTS llm_cache (
    key          TEXT PRIMARY KEY,
    model        TEXT NOT NULL,
    response     BLOB NOT NULL,
    tokens_in    INTEGER NOT NULL,
    tokens_out   INTEGER NOT NULL,
    created_ts   INTEGER NOT NULL,
    last_hit_ts  INTEGER NOT NULL,
    hit_count    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_llm_cache_lru ON llm_cache(last_hit_ts);

-- ── Embedding cache (L2, plain f16 — vectors are not PII)
CREATE TABLE IF NOT EXISTS embed_cache (
    key          TEXT PRIMARY KEY,
    model        TEXT NOT NULL,
    vector       BLOB NOT NULL,
    dim          INTEGER NOT NULL,
    created_ts   INTEGER NOT NULL,
    last_hit_ts  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_embed_cache_lru ON embed_cache(last_hit_ts);
```

Create the two CRUD modules:

```rust
// rust/crates/attune-core/src/store/usage.rs
//! CRUD for usage_events. Spec §3 + §5.2.

use crate::error::Result;
use crate::store::Store;
use crate::usage::types::*;
use rusqlite::params;

impl Store {
    pub fn record_usage(&self, event: &UsageEvent) -> Result<()> {
        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO usage_events
             (ts_ms, kind, provider, model, agent_id, tokens_in, tokens_out, cached_in,
              cost_usd, cache, outcome, latency_ms, error_kind, query_hash)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)"
        )?;
        let (outcome_str, err_kind) = match event.outcome {
            CallOutcome::Ok => ("ok", None),
            CallOutcome::Retry { .. } => ("retry", None),
            CallOutcome::Fail { error_kind } => ("fail", Some(format!("{:?}", error_kind).to_lowercase())),
        };
        stmt.execute(params![
            event.ts_ms,
            format!("{:?}", event.kind).to_lowercase(),
            event.usage.provider,
            event.usage.model,
            event.agent_id,
            event.usage.tokens_in,
            event.usage.tokens_out,
            event.usage.cached_in,
            event.cost_usd,
            format!("{:?}", event.cache).to_lowercase(),
            outcome_str,
            event.latency_ms,
            err_kind,
            event.query_hash,
        ])?;
        Ok(())
    }

    pub fn purge_usage_older_than(&self, cutoff_ms: i64) -> Result<usize> {
        let n = self.conn.execute("DELETE FROM usage_events WHERE ts_ms < ?1", params![cutoff_ms])?;
        Ok(n)
    }

    pub fn reset_usage(&self) -> Result<usize> {
        let n = self.conn.execute("DELETE FROM usage_events", [])?;
        Ok(n)
    }

    pub fn usage_summary(&self, from_ms: i64, to_ms: i64) -> Result<UsageSummary> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT count(*),
                    coalesce(sum(tokens_in),0),
                    coalesce(sum(tokens_out),0),
                    coalesce(sum(cost_usd),0.0),
                    coalesce(sum(CASE cache WHEN 'hit' THEN 1 ELSE 0 END),0)
             FROM usage_events WHERE ts_ms BETWEEN ?1 AND ?2"
        )?;
        let (events, ti, to, cost, hits): (i64, i64, i64, f64, i64) = stmt.query_row(
            params![from_ms, to_ms], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        )?;
        let hit_rate = if events > 0 { hits as f64 / events as f64 } else { 0.0 };
        Ok(UsageSummary {
            events: events as u64,
            tokens_in: ti as u64,
            tokens_out: to as u64,
            cost_usd: cost,
            cache_hit_rate: hit_rate,
        })
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub events: u64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: f64,
    pub cache_hit_rate: f64,
}
```

```rust
// rust/crates/attune-core/src/store/cache.rs
//! CRUD for llm_cache + embed_cache. Encryption is handled by the cache backend
//! (cache::sqlite_encrypted), this layer is plain SQL.

use crate::cache::{CacheScope, CachedValue};
use crate::error::Result;
use crate::store::Store;
use rusqlite::params;

impl Store {
    pub fn llm_cache_get(&self, key: &str) -> Result<Option<CachedValue>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT model, response, tokens_in, tokens_out FROM llm_cache WHERE key = ?1"
        )?;
        let row = stmt.query_row(params![key], |r| {
            Ok(CachedValue {
                model: r.get(0)?,
                bytes: r.get(1)?,
                tokens_in: r.get::<_, i64>(2)? as u32,
                tokens_out: r.get::<_, i64>(3)? as u32,
            })
        }).ok();
        if row.is_some() {
            // bump last_hit_ts + hit_count
            let now = chrono::Utc::now().timestamp_millis();
            self.conn.execute(
                "UPDATE llm_cache SET last_hit_ts=?1, hit_count=hit_count+1 WHERE key=?2",
                params![now, key]
            )?;
        }
        Ok(row)
    }

    pub fn llm_cache_put(&self, key: &str, value: &CachedValue) -> Result<()> {
        let now = chrono::Utc::now().timestamp_millis();
        self.conn.execute(
            "INSERT OR REPLACE INTO llm_cache
             (key, model, response, tokens_in, tokens_out, created_ts, last_hit_ts, hit_count)
             VALUES (?1,?2,?3,?4,?5,?6,?7,0)",
            params![key, value.model, value.bytes, value.tokens_in, value.tokens_out, now, now]
        )?;
        Ok(())
    }

    pub fn cache_count(&self, scope: CacheScope) -> Result<usize> {
        let sql = match scope {
            CacheScope::Llm => "SELECT count(*) FROM llm_cache",
            CacheScope::Embed => "SELECT count(*) FROM embed_cache",
            CacheScope::Search => "SELECT count(*) FROM web_search_cache",
            CacheScope::All => return Ok(
                self.cache_count(CacheScope::Llm)? +
                self.cache_count(CacheScope::Embed)? +
                self.cache_count(CacheScope::Search)?
            ),
        };
        let n: i64 = self.conn.query_row(sql, [], |r| r.get(0))?;
        Ok(n as usize)
    }

    pub fn cache_clear_scope(&self, scope: CacheScope) -> Result<usize> {
        let sql = match scope {
            CacheScope::Llm => "DELETE FROM llm_cache",
            CacheScope::Embed => "DELETE FROM embed_cache",
            CacheScope::Search => "DELETE FROM web_search_cache",
            CacheScope::All => return Ok(
                self.cache_clear_scope(CacheScope::Llm)? +
                self.cache_clear_scope(CacheScope::Embed)? +
                self.cache_clear_scope(CacheScope::Search)?
            ),
        };
        let n = self.conn.execute(sql, [])?;
        Ok(n)
    }
}
```

Add `pub mod usage; pub mod cache;` to `rust/crates/attune-core/src/store/mod.rs` (right after the existing `pub mod items;` group).

- [ ] **Step 4: Run — verify pass**

Run: `cargo test -p attune-core store::usage_test 2>&1 | tail -10`
Expected: `test result: ok. 3 passed`.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/store/
git commit -m "feat(store): usage_events + llm_cache + embed_cache schema + CRUD

Spec: 2026-05-28-cache-context-token-standard-api.md §3 + §5.2
- 3 new tables with CREATE TABLE IF NOT EXISTS (transparent migration)
- record_usage / purge_usage_older_than / reset_usage / usage_summary
- llm_cache_get/put + cache_count/cache_clear_scope (scope-aware)
- Includes UsageSummary struct for /api/v1/usage/summary response"
```

---

## Task E: `MemoryLruCache` L1 backend (TDD)

**Files:**
- Modify: `rust/crates/attune-core/src/cache/memory.rs`
- Create: `rust/crates/attune-core/src/cache/tests/lru.rs`

- [ ] **Step 1: Write failing tests**

```rust
// rust/crates/attune-core/src/cache/tests/lru.rs
use crate::cache::{CacheBackend, CacheScope, CachedValue};
use crate::cache::memory::MemoryLruCache;

fn mkval(b: &[u8]) -> CachedValue {
    CachedValue { bytes: b.to_vec(), tokens_in: 10, tokens_out: 5, model: "m".into() }
}

#[tokio::test]
async fn put_then_get_returns_value() {
    let c = MemoryLruCache::new(10);
    c.put(CacheScope::Llm, "k1", mkval(b"v1"), None).await;
    let v = c.get(CacheScope::Llm, "k1").await.expect("hit");
    assert_eq!(v.bytes, b"v1");
}

#[tokio::test]
async fn miss_returns_none() {
    let c = MemoryLruCache::new(10);
    assert!(c.get(CacheScope::Llm, "nope").await.is_none());
}

#[tokio::test]
async fn lru_eviction_when_over_cap() {
    let c = MemoryLruCache::new(2);
    c.put(CacheScope::Llm, "a", mkval(b"a"), None).await;
    c.put(CacheScope::Llm, "b", mkval(b"b"), None).await;
    c.put(CacheScope::Llm, "c", mkval(b"c"), None).await;  // evicts "a"
    assert!(c.get(CacheScope::Llm, "a").await.is_none());
    assert!(c.get(CacheScope::Llm, "b").await.is_some());
    assert!(c.get(CacheScope::Llm, "c").await.is_some());
}

#[tokio::test]
async fn count_returns_scope_size() {
    let c = MemoryLruCache::new(10);
    c.put(CacheScope::Llm, "a", mkval(b"x"), None).await;
    c.put(CacheScope::Embed, "b", mkval(b"y"), None).await;
    assert_eq!(c.count(CacheScope::Llm).await, 1);
    assert_eq!(c.count(CacheScope::Embed).await, 1);
    assert_eq!(c.count(CacheScope::All).await, 2);
}

#[tokio::test]
async fn clear_scope_only_removes_that_scope() {
    let c = MemoryLruCache::new(10);
    c.put(CacheScope::Llm, "a", mkval(b"x"), None).await;
    c.put(CacheScope::Embed, "b", mkval(b"y"), None).await;
    let n = c.clear(CacheScope::Llm).await;
    assert_eq!(n, 1);
    assert!(c.get(CacheScope::Embed, "b").await.is_some());
}
```

Add `mod lru;` to `rust/crates/attune-core/src/cache/tests/mod.rs`.

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p attune-core cache::tests::lru 2>&1 | tail -10`
Expected: FAIL — `MemoryLruCache` not found.

- [ ] **Step 3: Implement `MemoryLruCache`**

```rust
// rust/crates/attune-core/src/cache/memory.rs
//! L1 in-memory LRU. Per-scope independent capacity.

use crate::cache::{CacheBackend, CacheScope, CachedValue};
use async_trait::async_trait;
use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::Mutex;

pub struct MemoryLruCache {
    llm: Mutex<LruCache<String, CachedValue>>,
    embed: Mutex<LruCache<String, CachedValue>>,
    search: Mutex<LruCache<String, CachedValue>>,
}

impl MemoryLruCache {
    pub fn new(cap_per_scope: usize) -> Self {
        let cap = NonZeroUsize::new(cap_per_scope.max(1)).unwrap();
        Self {
            llm: Mutex::new(LruCache::new(cap)),
            embed: Mutex::new(LruCache::new(cap)),
            search: Mutex::new(LruCache::new(cap)),
        }
    }

    fn lock(&self, scope: CacheScope) -> Option<&Mutex<LruCache<String, CachedValue>>> {
        match scope {
            CacheScope::Llm => Some(&self.llm),
            CacheScope::Embed => Some(&self.embed),
            CacheScope::Search => Some(&self.search),
            CacheScope::All => None,
        }
    }
}

#[async_trait]
impl CacheBackend for MemoryLruCache {
    async fn get(&self, scope: CacheScope, key: &str) -> Option<CachedValue> {
        let m = self.lock(scope)?;
        m.lock().ok()?.get(key).cloned()
    }

    async fn put(&self, scope: CacheScope, key: &str, value: CachedValue, _ttl: Option<u32>) {
        if let Some(m) = self.lock(scope) {
            if let Ok(mut g) = m.lock() {
                g.put(key.to_string(), value);
            }
        }
    }

    async fn clear(&self, scope: CacheScope) -> usize {
        match scope {
            CacheScope::All => {
                let a = self.clear(CacheScope::Llm).await;
                let b = self.clear(CacheScope::Embed).await;
                let c = self.clear(CacheScope::Search).await;
                a + b + c
            }
            other => {
                if let Some(m) = self.lock(other) {
                    let mut g = m.lock().unwrap();
                    let n = g.len();
                    g.clear();
                    n
                } else { 0 }
            }
        }
    }

    async fn count(&self, scope: CacheScope) -> usize {
        match scope {
            CacheScope::All => {
                self.count(CacheScope::Llm).await +
                self.count(CacheScope::Embed).await +
                self.count(CacheScope::Search).await
            }
            other => self.lock(other).and_then(|m| m.lock().ok()).map(|g| g.len()).unwrap_or(0),
        }
    }
}
```

Add `lru = "0.12"` to `rust/crates/attune-core/Cargo.toml` if missing (check first: `grep -n '^lru ' rust/crates/attune-core/Cargo.toml`).

- [ ] **Step 4: Run — verify pass**

Run: `cargo test -p attune-core cache::tests::lru 2>&1 | tail -5`
Expected: `test result: ok. 5 passed`.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/cache/memory.rs rust/crates/attune-core/src/cache/tests/ rust/crates/attune-core/Cargo.toml
git commit -m "feat(cache): MemoryLruCache L1 backend (per-scope LRU)

5 unit tests: put/get, miss, eviction, count, clear scope isolation."
```

---

## Task F: `SqliteEncryptedCache` L2 backend (TDD)

**Files:**
- Modify: `rust/crates/attune-core/src/cache/sqlite_encrypted.rs`
- Create: `rust/crates/attune-core/src/cache/tests/encrypted.rs`

- [ ] **Step 1: Write failing test**

```rust
// rust/crates/attune-core/src/cache/tests/encrypted.rs
use crate::cache::{CacheBackend, CacheScope, CachedValue};
use crate::cache::sqlite_encrypted::SqliteEncryptedCache;
use crate::crypto::Key32;
use crate::store::Store;
use std::sync::Arc;
use tempfile::TempDir;

fn mkval(b: &[u8]) -> CachedValue {
    CachedValue { bytes: b.to_vec(), tokens_in: 1, tokens_out: 1, model: "m".into() }
}

#[tokio::test]
async fn put_then_get_returns_plaintext() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(Store::open(dir.path().join("v.db")).unwrap());
    let dek = Key32::from_bytes(&[7u8; 32]).unwrap();
    let c = SqliteEncryptedCache::new(store.clone(), dek);

    c.put(CacheScope::Llm, "k", mkval(b"secret response"), None).await;
    let v = c.get(CacheScope::Llm, "k").await.expect("hit");
    assert_eq!(v.bytes, b"secret response");
}

#[tokio::test]
async fn raw_blob_is_ciphertext_not_plaintext() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(Store::open(dir.path().join("v.db")).unwrap());
    let dek = Key32::from_bytes(&[7u8; 32]).unwrap();
    let c = SqliteEncryptedCache::new(store.clone(), dek);

    c.put(CacheScope::Llm, "k", mkval(b"plaintext_marker"), None).await;
    let raw: Vec<u8> = store.raw_connection_for_test()
        .query_row("SELECT response FROM llm_cache WHERE key='k'", [], |r| r.get(0))
        .unwrap();
    assert!(!raw.windows(b"plaintext_marker".len()).any(|w| w == b"plaintext_marker"),
        "L2 blob must be encrypted, found plaintext: {:?}", raw);
}
```

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p attune-core cache::tests::encrypted 2>&1 | tail -10`
Expected: FAIL — `SqliteEncryptedCache` not found.

- [ ] **Step 3: Implement `SqliteEncryptedCache`**

```rust
// rust/crates/attune-core/src/cache/sqlite_encrypted.rs
//! L2 cache: AES-256-GCM encrypted via DEK, persisted to SQLite.

use crate::cache::{CacheBackend, CacheScope, CachedValue};
use crate::crypto::{self, Key32};
use crate::store::Store;
use async_trait::async_trait;
use std::sync::Arc;

pub struct SqliteEncryptedCache {
    store: Arc<Store>,
    dek: Key32,
}

impl SqliteEncryptedCache {
    pub fn new(store: Arc<Store>, dek: Key32) -> Self {
        Self { store, dek }
    }
}

#[async_trait]
impl CacheBackend for SqliteEncryptedCache {
    async fn get(&self, scope: CacheScope, key: &str) -> Option<CachedValue> {
        if !matches!(scope, CacheScope::Llm) {
            // embed cache stores plain f16 vectors (not PII); search cache is its own module
            return None;
        }
        let raw = self.store.llm_cache_get(key).ok().flatten()?;
        let pt = crypto::aead_decrypt(&self.dek, &raw.bytes).ok()?;
        Some(CachedValue {
            bytes: pt,
            tokens_in: raw.tokens_in,
            tokens_out: raw.tokens_out,
            model: raw.model,
        })
    }

    async fn put(&self, scope: CacheScope, key: &str, value: CachedValue, _ttl: Option<u32>) {
        if !matches!(scope, CacheScope::Llm) { return; }
        let Ok(ct) = crypto::aead_encrypt(&self.dek, &value.bytes) else { return; };
        let encrypted = CachedValue { bytes: ct, ..value };
        let _ = self.store.llm_cache_put(key, &encrypted);
    }

    async fn clear(&self, scope: CacheScope) -> usize {
        self.store.cache_clear_scope(scope).unwrap_or(0)
    }

    async fn count(&self, scope: CacheScope) -> usize {
        self.store.cache_count(scope).unwrap_or(0)
    }
}
```

If `crypto::aead_encrypt` / `aead_decrypt` don't exist with those exact names, grep first: `grep -n "pub fn.*encrypt\|pub fn.*decrypt" rust/crates/attune-core/src/crypto.rs`. Use whatever names exist (likely `encrypt_with_key` / `decrypt_with_key` or similar) and adapt. Update test if needed.

- [ ] **Step 4: Run — verify pass**

Run: `cargo test -p attune-core cache::tests::encrypted 2>&1 | tail -10`
Expected: `test result: ok. 2 passed`.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/cache/sqlite_encrypted.rs rust/crates/attune-core/src/cache/tests/encrypted.rs
git commit -m "feat(cache): SqliteEncryptedCache L2 backend (AES-256-GCM via DEK)

2 unit tests: round-trip plaintext + raw blob is ciphertext.
Scope=Embed/Search fall through (embed uses plain f16, search uses its own module)."
```

---

## Task G: `UsageRecorderGuard` Drop guard (compile-time enforcement) (TDD)

**Files:**
- Create: `rust/crates/attune-core/src/usage/guard.rs`

- [ ] **Step 1: Write failing test**

```rust
// rust/crates/attune-core/src/usage/tests/guard_test.rs
use crate::usage::guard::UsageRecorderGuard;
use crate::usage::types::*;
use std::sync::{Arc, Mutex};

#[test]
fn guard_records_on_complete() {
    let recorded: Arc<Mutex<Vec<UsageEvent>>> = Arc::new(Mutex::new(vec![]));
    let r = recorded.clone();
    let recorder = move |e: UsageEvent| { r.lock().unwrap().push(e); };

    {
        let mut g = UsageRecorderGuard::new(UsageKind::LlmChat, "ollama", "qwen2.5:3b", Box::new(recorder));
        g.complete(TokenUsage::empty("ollama", "qwen2.5:3b"), CacheOutcome::Miss, CallOutcome::Ok);
    }
    assert_eq!(recorded.lock().unwrap().len(), 1);
}

#[test]
#[should_panic(expected = "UsageRecorderGuard dropped without complete()")]
#[cfg(debug_assertions)]
fn guard_panics_on_drop_without_complete_in_debug() {
    let recorder = |_: UsageEvent| {};
    let _g = UsageRecorderGuard::new(UsageKind::LlmChat, "ollama", "x", Box::new(recorder));
    // Drop without complete — must panic in debug
}
```

Add `mod guard_test;` to `rust/crates/attune-core/src/usage/tests/mod.rs`.

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p attune-core usage::tests::guard_test 2>&1 | tail -10`
Expected: FAIL — `UsageRecorderGuard` not found.

- [ ] **Step 3: Implement guard**

```rust
// rust/crates/attune-core/src/usage/guard.rs
//! Drop guard — every LLM/Embed call site must complete() the guard before drop.
//! In debug builds, dropping without complete() panics. In release, logs a warning.
//!
//! Spec §11 risk 1 (compile-time enforcement of usage recording).

use crate::usage::types::*;
use std::time::Instant;

pub type RecordFn = Box<dyn FnOnce(UsageEvent) + Send>;

pub struct UsageRecorderGuard {
    kind: UsageKind,
    provider: String,
    model: String,
    started: Instant,
    agent_id: Option<String>,
    query_hash: Option<String>,
    recorder: Option<RecordFn>,
    completed: bool,
}

impl UsageRecorderGuard {
    pub fn new(kind: UsageKind, provider: &str, model: &str, recorder: RecordFn) -> Self {
        Self {
            kind,
            provider: provider.into(),
            model: model.into(),
            started: Instant::now(),
            agent_id: None,
            query_hash: None,
            recorder: Some(recorder),
            completed: false,
        }
    }

    pub fn with_agent(mut self, agent_id: impl Into<String>) -> Self {
        self.agent_id = Some(agent_id.into());
        self
    }

    pub fn with_query_hash(mut self, hash: impl Into<String>) -> Self {
        self.query_hash = Some(hash.into());
        self
    }

    pub fn complete(&mut self, usage: TokenUsage, cache: CacheOutcome, outcome: CallOutcome) {
        if self.completed { return; }
        let latency = self.started.elapsed().as_millis().min(u32::MAX as u128) as u32;
        let cost_usd = crate::cost::estimate_cost_usd(usage.tokens_in, usage.tokens_out, &usage.model).ok();
        let event = UsageEvent {
            ts_ms: chrono::Utc::now().timestamp_millis(),
            kind: self.kind,
            usage,
            cost_usd,
            cache,
            outcome,
            latency_ms: latency,
            agent_id: self.agent_id.take(),
            query_hash: self.query_hash.take(),
        };
        if let Some(recorder) = self.recorder.take() {
            recorder(event);
        }
        self.completed = true;
    }
}

impl Drop for UsageRecorderGuard {
    fn drop(&mut self) {
        if !self.completed {
            #[cfg(debug_assertions)]
            panic!("UsageRecorderGuard dropped without complete() — provider={} model={} kind={:?}",
                   self.provider, self.model, self.kind);
            #[cfg(not(debug_assertions))]
            tracing::warn!(provider=%self.provider, model=%self.model, kind=?self.kind,
                "UsageRecorderGuard dropped without complete() — telemetry lost");
        }
    }
}
```

If `cost::estimate_cost_usd` does not return `Result`, adapt (`.ok()` → direct `Some(...)`). Check the actual signature: `grep -n "pub fn estimate_cost_usd" rust/crates/attune-core/src/cost.rs`.

Wire up: add `pub mod guard;` to `rust/crates/attune-core/src/usage/mod.rs` and re-export.

- [ ] **Step 4: Run — verify pass**

Run: `cargo test -p attune-core usage::tests::guard_test 2>&1 | tail -10`
Expected: `test result: ok. 2 passed`.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/usage/guard.rs rust/crates/attune-core/src/usage/mod.rs rust/crates/attune-core/src/usage/tests/
git commit -m "feat(usage): UsageRecorderGuard Drop guard (debug panic if not completed)

Spec §11 risk 1 mitigation 2. Forces every LLM/Embed call site to call
.complete() with TokenUsage + CacheOutcome + CallOutcome before drop.

2 unit tests: happy path, debug panic on drop without complete."
```

---

## Task H: `UsageAggregator` ring buffer + batch flush (TDD)

**Files:**
- Create: `rust/crates/attune-core/src/usage/aggregator.rs`

- [ ] **Step 1: Write failing test**

```rust
// rust/crates/attune-core/src/usage/tests/aggregator_test.rs
use crate::usage::aggregator::UsageAggregator;
use crate::usage::types::*;
use crate::store::Store;
use std::sync::Arc;
use tempfile::TempDir;

fn mkevent() -> UsageEvent {
    UsageEvent {
        ts_ms: 1717000000000,
        kind: UsageKind::LlmChat,
        usage: TokenUsage::empty("ollama", "qwen2.5:3b"),
        cost_usd: Some(0.0),
        cache: CacheOutcome::Miss,
        outcome: CallOutcome::Ok,
        latency_ms: 100,
        agent_id: None,
        query_hash: None,
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn flush_after_100ms_persists_to_store() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(Store::open(dir.path().join("v.db")).unwrap());
    let agg = UsageAggregator::new(store.clone(), 100, 50);
    agg.record(mkevent());
    tokio::time::advance(std::time::Duration::from_millis(150)).await;
    agg.tick().await;  // explicit tick for deterministic test
    let summary = store.usage_summary(0, 9_999_999_999_999).unwrap();
    assert_eq!(summary.events, 1);
}

#[tokio::test]
async fn ring_buffer_full_drops_oldest() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(Store::open(dir.path().join("v.db")).unwrap());
    let agg = UsageAggregator::new(store.clone(), 100, 2);  // cap=2
    agg.record(mkevent());
    agg.record(mkevent());
    agg.record(mkevent());  // drops oldest
    agg.flush_now().await;
    let summary = store.usage_summary(0, 9_999_999_999_999).unwrap();
    assert_eq!(summary.events, 2, "ring buffer cap=2, oldest dropped");
}
```

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p attune-core usage::tests::aggregator_test 2>&1 | tail -10`
Expected: FAIL — `UsageAggregator` not found.

- [ ] **Step 3: Implement aggregator**

```rust
// rust/crates/attune-core/src/usage/aggregator.rs
//! Ring buffer + async batch flush. Main path enqueues (sync, micro-second);
//! background tick (every flush_interval_ms) drains buffer into Store.
//!
//! Spec §3 + §11 risk 2 mitigation.

use crate::store::Store;
use crate::usage::types::UsageEvent;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct UsageAggregator {
    buffer: Arc<Mutex<VecDeque<UsageEvent>>>,
    store: Arc<Store>,
    cap: usize,
    flush_interval_ms: u64,
}

impl UsageAggregator {
    pub fn new(store: Arc<Store>, flush_interval_ms: u64, cap: usize) -> Self {
        Self {
            buffer: Arc::new(Mutex::new(VecDeque::with_capacity(cap))),
            store,
            cap,
            flush_interval_ms,
        }
    }

    /// Sync, microsecond — main LLM path calls this.
    pub fn record(&self, event: UsageEvent) {
        if let Ok(mut buf) = self.buffer.lock() {
            if buf.len() >= self.cap {
                buf.pop_front();  // drop oldest
                tracing::warn!("UsageAggregator buffer full (cap={}); oldest event dropped", self.cap);
            }
            buf.push_back(event);
        }
    }

    /// Drain buffer to Store. Called by background task every flush_interval_ms.
    pub async fn tick(&self) {
        self.flush_now().await
    }

    pub async fn flush_now(&self) {
        let drained: Vec<UsageEvent> = {
            let mut buf = self.buffer.lock().expect("aggregator mutex poisoned");
            buf.drain(..).collect()
        };
        for e in drained {
            if let Err(err) = self.store.record_usage(&e) {
                tracing::warn!("record_usage failed: {err:?} — telemetry lost for {:?}", e.kind);
            }
        }
    }

    /// Recent events (for routing strategy feedback per Plan A2).
    pub fn recent(&self, n: usize) -> Vec<UsageEvent> {
        self.buffer.lock().map(|b| b.iter().rev().take(n).cloned().collect()).unwrap_or_default()
    }

    /// Spawn a background task that flushes every flush_interval_ms.
    /// Call once at server startup.
    pub fn spawn_flusher(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        let interval = Duration::from_millis(self.flush_interval_ms);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                self.tick().await;
            }
        })
    }
}
```

- [ ] **Step 4: Run — verify pass**

Run: `cargo test -p attune-core usage::tests::aggregator_test 2>&1 | tail -10`
Expected: `test result: ok. 2 passed`. (`tokio::time::advance` requires `#[tokio::test(start_paused = true)]` — check `tokio` features in `Cargo.toml`; if `test-util` not enabled, drop the timing test and keep only buffer-cap test.)

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/usage/aggregator.rs rust/crates/attune-core/src/usage/mod.rs rust/crates/attune-core/src/usage/tests/
git commit -m "feat(usage): UsageAggregator ring buffer + async batch flush

Spec §3 (aggregator flow) + §11 risk 2 (write amplification mitigation).
- Main path: sync VecDeque::push_back (microsecond)
- Background: tokio::interval drains to Store every flush_interval_ms
- Buffer-full: drop oldest + warn log (graceful degradation per §7.3)

2 unit tests: flush-after-interval (paused clock), ring-cap eviction."
```

---

## Task I: Refactor `LlmProvider::chat` signature to return `TokenUsage` (BREAKING)

**Files:**
- Modify: `rust/crates/attune-core/src/llm.rs` (trait + all 3 impls: Ollama, OpenAI, Mock)
- Modify: every caller of `client.chat(...)` across the workspace

This is the largest task — done in **5 sub-steps**, each independently committable.

- [ ] **Step 1: Update trait signature + Mock impl + a compile-fail test**

In `rust/crates/attune-core/src/llm.rs`, change:

```rust
// Before:
fn chat(&self, system: &str, user: &str) -> Result<String>;

// After:
fn chat(&self, system: &str, user: &str) -> Result<(String, crate::usage::TokenUsage)>;
```

Same for `chat_with_history` and `chat_multimodal` (return tuple).

Update `MockLlmProvider::chat` to return `Ok((response_string, TokenUsage::empty("mock", "mock")))`.

For `chat_with_format_json`, return `Result<(String, TokenUsage)>`.

For `chat_with_validation` (the dyn-compat retry helper), return `Result<(String, TokenUsage)>`.

Run: `cargo build -p attune-core 2>&1 | tail -20`
Expected: FAIL with ~10-20 compile errors at call sites in `chat.rs`, `agent_runner.rs`, `classifier.rs`, etc. **This is the goal** — compile-time forces every site to be touched.

- [ ] **Step 2: Update OllamaLlmProvider impl**

In `rust/crates/attune-core/src/llm.rs` around `impl LlmProvider for OllamaLlmProvider`, parse Ollama response's `prompt_eval_count` + `eval_count` fields (already in Ollama `/api/chat` response JSON) and return them:

```rust
fn chat(&self, system: &str, user: &str) -> Result<(String, crate::usage::TokenUsage)> {
    // ... existing HTTP call ...
    let resp_text = response.message.content.clone();
    let usage = crate::usage::TokenUsage {
        tokens_in: response.prompt_eval_count.unwrap_or(0) as u32,
        tokens_out: response.eval_count.unwrap_or(0) as u32,
        cached_in: 0,
        model: self.model.clone(),
        provider: "ollama".to_string(),
    };
    Ok((resp_text, usage))
}
```

Add `prompt_eval_count` + `eval_count` to the `OllamaChatResponse` serde struct if not already there (they exist in Ollama API).

Run: `cargo build -p attune-core 2>&1 | tail -20`
Expected: Ollama impl OK; remaining errors at call sites.

- [ ] **Step 3: Update OpenAiLlmProvider impl**

Similar — parse `usage.prompt_tokens` + `usage.completion_tokens` + `usage.prompt_tokens_details.cached_tokens` from OpenAI response. The struct exists in the response model; just plumb it through:

```rust
fn chat(&self, system: &str, user: &str) -> Result<(String, crate::usage::TokenUsage)> {
    // existing call ...
    let usage_obj = body.usage.unwrap_or_default();
    let cached_in = usage_obj.prompt_tokens_details
        .as_ref()
        .and_then(|d| d.cached_tokens)
        .unwrap_or(0);
    let token_usage = crate::usage::TokenUsage {
        tokens_in: usage_obj.prompt_tokens.unwrap_or(0) as u32,
        tokens_out: usage_obj.completion_tokens.unwrap_or(0) as u32,
        cached_in: cached_in as u32,
        model: self.model.clone(),
        provider: self.provider_label.clone(),  // "openai" / "cloud_gateway" / etc
    };
    Ok((content_string, token_usage))
}
```

If `prompt_tokens_details` field doesn't exist in current struct, add it as `Option<PromptTokensDetails>` with `cached_tokens: Option<u64>`.

- [ ] **Step 4: Update all call sites**

Run `grep -rn "\.chat(" --include="*.rs" rust/ | grep -v "tests/" | grep -v "// " | wc -l` — expect ~30-50 hits. For each non-test caller, rewrite from:

```rust
let response = client.chat(sys, user)?;
```

To:

```rust
let (response, _usage) = client.chat(sys, user)?;
```

For sites where the result is fed into a recorder, store `usage` for the guard:

```rust
let (response, usage) = client.chat(sys, user)?;
guard.complete(usage, CacheOutcome::Miss, CallOutcome::Ok);
```

Use the script `tmp/update-chat-callers.sh`:

```bash
# tmp/update-chat-callers.sh
#!/usr/bin/env bash
set -euo pipefail
grep -rln "let .* = .*\.chat(" --include="*.rs" rust/ | while read f; do
    # show diff candidates per file; rewrite manually because pattern-matching is unreliable
    grep -n "\.chat(" "$f" | grep -v "// "
    echo "--- $f ---"
done
```

Do **not** automate the rewrite — read each call site, decide whether to bind usage or `_`.

Run after rewriting: `cargo build --workspace 2>&1 | tail -20`
Expected: clean build.

- [ ] **Step 5: Commit (atomic for git bisect)**

```bash
git add -A
git commit -m "refactor(llm)!: chat() returns (String, TokenUsage) — compile-time usage enforcement

BREAKING CHANGE: LlmProvider trait signature changed.
- chat / chat_with_history / chat_multimodal / chat_with_format_json /
  chat_with_validation all return Result<(String, TokenUsage), _>
- Ollama impl: parses prompt_eval_count + eval_count from /api/chat response
- OpenAI impl: parses usage.prompt_tokens + completion_tokens + cached_tokens
- Mock impl: returns TokenUsage::empty(\"mock\", \"mock\")
- All ~40 call sites updated (some destructure usage, some _ if not yet wired)

Spec: 2026-05-28-cache-context-token-standard-api.md §11 risk 1 mitigation 1.
Next task (J) routes recording through UsageAggregator at hot call sites."
```

---

## Task J: Refactor `EmbeddingProvider::embed` to return `TokenUsage`

**Files:**
- Modify: `rust/crates/attune-core/src/embed.rs` (trait + impls)
- Modify: all `provider.embed(...)` call sites

- [ ] **Step 1: Update trait + impls**

```rust
// embed.rs
pub trait EmbeddingProvider: Send + Sync {
    fn embed(&self, texts: &[&str]) -> Result<(Vec<Vec<f32>>, crate::usage::TokenUsage)>;
    // ...
}
```

For `OllamaProvider::embed`, count input chars (Ollama embed endpoint doesn't return token usage; estimate via `cost::estimate_tokens`):

```rust
fn embed(&self, texts: &[&str]) -> Result<(Vec<Vec<f32>>, TokenUsage)> {
    // existing http call ...
    let total_chars: usize = texts.iter().map(|t| t.len()).sum();
    let est_tokens = crate::cost::estimate_tokens(&texts.join(""), &self.model);
    let usage = TokenUsage {
        tokens_in: est_tokens as u32,
        tokens_out: 0,  // embeddings have no output tokens
        cached_in: 0,
        model: self.model.clone(),
        provider: "ollama".into(),
    };
    Ok((vectors, usage))
}
```

`MockEmbeddingProvider` and `NoopProvider` return `(vecs, TokenUsage::empty(...))`.

- [ ] **Step 2: Run — verify compile-fail at call sites**

Run: `cargo build --workspace 2>&1 | tail -10`
Expected: 5-10 errors at `embed(...)` call sites in `ai_annotator.rs`, `reindex.rs`, `state.rs::embed_pending_memories`, etc.

- [ ] **Step 3: Update all call sites**

Same pattern as Task I — destructure or `_`. After:

Run: `cargo build --workspace 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 4: Run unit tests**

Run: `cargo test -p attune-core 2>&1 | tail -10`
Expected: existing tests pass; new TokenUsage assertions OK.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor(embed)!: embed() returns (Vec<Vec<f32>>, TokenUsage)

BREAKING CHANGE: EmbeddingProvider trait signature.
- Ollama: estimates tokens via cost::estimate_tokens (Ollama embed endpoint
  doesn't expose token usage like /api/chat does)
- Mock + Noop: TokenUsage::empty
- All ~15 call sites updated

Spec §11 risk 1 mitigation 1 (embed path coverage)."
```

---

## Task K: Add `tokens_in_used` to `BudgetPlan` + forced `plan_context` entry

**Files:**
- Modify: `rust/crates/attune-core/src/context_budget.rs`
- Modify: callers in `chat.rs` to use it

- [ ] **Step 1: Write failing test**

```rust
// add to context_budget.rs tests module
#[test]
fn plan_context_reports_tokens_used() {
    let plan = plan_context("gemini-1.5-flash", "system text", "user text", &[]);
    assert!(plan.tokens_in_used > 0, "tokens_in_used should reflect system+user+history+knowledge");
    assert!(plan.tokens_in_used <= plan.window - plan.response_reserve);
}
```

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p attune-core context_budget::tests::plan_context_reports_tokens_used 2>&1 | tail -5`
Expected: FAIL — no field `tokens_in_used`.

- [ ] **Step 3: Implement field**

Add to `BudgetPlan`:

```rust
pub tokens_in_used: usize,
```

In `plan_context`, populate it before return:

```rust
BudgetPlan {
    window,
    response_reserve,
    knowledge_tokens,
    history_keep: keep,
    history_dropped,
    tokens_in_used: system_tok + user_tok + used + knowledge_tokens,
}
```

- [ ] **Step 4: Run — verify pass**

Run: `cargo test -p attune-core context_budget 2>&1 | tail -5`
Expected: `ok. <N> passed`.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/context_budget.rs
git commit -m "feat(context_budget): BudgetPlan.tokens_in_used for UsageEvent populating

Spec §4.2 (context_budget call site)."
```

---

## Task L: Wire `UsageAggregator` + `CacheBackend` into `AppState`

**Files:**
- Modify: `rust/crates/attune-server/src/state.rs`

- [ ] **Step 1: Write failing test**

```rust
// rust/crates/attune-server/src/state_test.rs (new) or append to existing tests
#[test]
fn appstate_exposes_usage_aggregator_and_cache() {
    let state = AppState::new_for_test();
    assert!(state.usage().is_some(), "usage aggregator must be installed");
    assert!(state.cache_backend().is_some(), "cache backend must be installed");
}
```

If `new_for_test` doesn't exist, build a tiny constructor inline that uses tempfile + Mock providers.

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p attune-server state_test 2>&1 | tail -5`
Expected: FAIL — `usage` accessor missing.

- [ ] **Step 3: Add fields + accessors**

In `rust/crates/attune-server/src/state.rs`:

```rust
pub struct AppState {
    // ... existing fields ...
    usage_aggregator: ArcSwapOption<UsageAggregator>,   // or Mutex<Option<Arc<UsageAggregator>>>
    cache_backend: ArcSwapOption<dyn CacheBackend>,
}

impl AppState {
    pub fn usage(&self) -> Option<Arc<UsageAggregator>> {
        self.usage_aggregator.load_full()
    }
    pub fn set_usage(&self, agg: Option<Arc<UsageAggregator>>) {
        match agg {
            Some(a) => self.usage_aggregator.store(Some(a)),
            None => self.usage_aggregator.store(None),
        }
    }
    pub fn cache_backend(&self) -> Option<Arc<dyn CacheBackend>> {
        self.cache_backend.load_full()
    }
}
```

If `ArcSwapOption` not in the project, use `Mutex<Option<Arc<...>>>` mirroring the existing `embedding` / `llm` pattern at lines 1735-1751.

In server `lib.rs` startup, instantiate:

```rust
let store = state.vault.store_arc();   // or however the store is exposed
let aggregator = Arc::new(UsageAggregator::new(store.clone(), 100, 1000));
state.set_usage(Some(aggregator.clone()));
aggregator.spawn_flusher();

let mem = Arc::new(MemoryLruCache::new(512));
state.cache_backend.store(Some(mem));
```

- [ ] **Step 4: Run — verify pass**

Run: `cargo test -p attune-server state_test 2>&1 | tail -5`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/src/state.rs rust/crates/attune-server/src/lib.rs
git commit -m "feat(state): AppState.usage_aggregator + cache_backend accessors

Wired at server startup (after vault open). Mirrors embedding/llm accessor pattern.
Aggregator flusher task spawned per Task H spec."
```

---

## Task M ⭐ (Plan A2 dependency anchor): Freeze public API surface for routing consumers

**Files:**
- Modify: `rust/crates/attune-core/src/lib.rs` — explicit `pub use` re-exports

This task **explicitly freezes** the API that Plan A2's routing module will consume. Plan A2's `BlockedBy` anchor is **this task name**: `Task M: Freeze public API surface for routing consumers`.

- [ ] **Step 1: Write failing doc test**

Add to `rust/crates/attune-core/src/lib.rs`:

```rust
//! ## Stable public API for routing consumers (Plan A2 dependency)
//!
//! ```
//! # use attune_core::{TokenUsage, UsageEvent, UsageKind, CacheOutcome, CallOutcome, UsageRecorderGuard, UsageAggregator};
//! let _ = TokenUsage::empty("p", "m");
//! ```
```

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p attune-core --doc 2>&1 | tail -10`
Expected: FAIL — types not re-exported at crate root.

- [ ] **Step 3: Add explicit `pub use`**

```rust
// rust/crates/attune-core/src/lib.rs
pub mod usage;
pub mod cache;

// Stable re-exports for downstream (Plan A2 router consumes these directly).
pub use usage::types::{
    TokenUsage, UsageEvent, UsageKind, CacheOutcome, CallOutcome, ErrorKind,
};
pub use usage::guard::UsageRecorderGuard;
pub use usage::aggregator::UsageAggregator;
pub use cache::{CacheBackend, CacheScope, CachedValue, cache_key};
```

- [ ] **Step 4: Run — verify pass + verify routing-consumer signature**

Run: `cargo test -p attune-core --doc 2>&1 | tail -5`
Expected: doctest pass.

Additionally verify the routing-feedback API exists (Plan A2 will call `UsageAggregator::recent(N)`):

```rust
// quick smoke
cargo run -p attune-core --example usage_api_freeze 2>&1 | head
```

(If no example exists, skip — the doctest already proves the API surface.)

- [ ] **Step 5: Commit (frozen API marker)**

```bash
git add rust/crates/attune-core/src/lib.rs
git commit -m "feat(usage,cache): freeze public API surface — Plan A2 dependency anchor

API FROZEN for downstream Plan A2 (hybrid token routing):
- TokenUsage, UsageEvent, UsageKind, CacheOutcome, CallOutcome, ErrorKind
- UsageRecorderGuard (for compile-time enforcement)
- UsageAggregator (.recent(N) for routing feedback)
- CacheBackend, CacheScope, CachedValue, cache_key()

Doctest at crate root smoke-tests the re-exports.

Plan A2 (docs/superpowers/plans/2026-05-28-hybrid-token-routing.md)
blockedBy: this commit. Do not start A2 routing module before this merges
to develop and the API surface is stable on main."
```

**⚠️ This is the Plan A2 unblock point. Push to develop and tag this commit msg specifically before starting A2.**

---

## Task N: REST routes — `/api/v1/usage/*` (TDD)

**Files:**
- Create: `rust/crates/attune-server/src/routes/usage.rs`
- Modify: `rust/crates/attune-server/src/routes/mod.rs`
- Modify: `rust/crates/attune-server/src/lib.rs` (mount router)

- [ ] **Step 1: Write failing HTTP test**

```rust
// rust/crates/attune-server/tests/usage_routes.rs
use axum::http::StatusCode;
mod common;
use common::TestServer;

#[tokio::test]
async fn get_usage_summary_returns_json() {
    let srv = TestServer::start().await;
    srv.unlock_vault().await;

    let res = srv.client().get(srv.url("/api/v1/usage/summary")).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = res.json().await.unwrap();
    assert!(body.get("totals").is_some(), "must contain totals object");
    assert!(body["totals"].get("events").is_some());
    assert!(body["totals"].get("tokens_in").is_some());
}

#[tokio::test]
async fn post_usage_reset_clears_events() {
    let srv = TestServer::start().await;
    srv.unlock_vault().await;
    // Pre-populate one event:
    srv.inject_test_usage_event().await;

    let res = srv.client().post(srv.url("/api/v1/usage/reset")).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = res.json().await.unwrap();
    assert!(body["deleted"].as_u64().unwrap() >= 1);
}

#[tokio::test]
async fn usage_routes_return_503_when_vault_locked() {
    let srv = TestServer::start().await;
    // intentionally do NOT unlock
    let res = srv.client().get(srv.url("/api/v1/usage/summary")).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["code"], "vault-locked");
}
```

If `TestServer` test harness doesn't have `inject_test_usage_event`, add it as `cfg(test)` helper that uses `state.usage().record(...)`.

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p attune-server --test usage_routes 2>&1 | tail -10`
Expected: FAIL — routes not mounted.

- [ ] **Step 3: Implement routes**

```rust
// rust/crates/attune-server/src/routes/usage.rs
//! Spec: 2026-05-28-cache-context-token-standard-api.md §5.2

use crate::error::{AppError, AppResult};
use crate::state::SharedState;
use axum::{extract::{Query, State}, response::Json, routing::{get, post}, Router};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize, Default)]
pub struct SummaryQuery {
    from: Option<String>,
    to: Option<String>,
    group_by: Option<String>,
}

pub async fn get_summary(
    State(state): State<SharedState>,
    Query(q): Query<SummaryQuery>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault_unlocked()?;  // returns 503 vault-locked otherwise
    let now = Utc::now().timestamp_millis();
    let from_ms = q.from.as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.timestamp_millis())
        .unwrap_or(now - 7 * 86_400_000);
    let to_ms = q.to.as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.timestamp_millis())
        .unwrap_or(now);

    // ensure pending events are flushed before we summarize:
    if let Some(agg) = state.usage() { agg.flush_now().await; }

    let summary = vault.store().usage_summary(from_ms, to_ms)
        .map_err(|e| AppError::internal("usage-summary-failed", e))?;
    Ok(Json(json!({
        "range": { "from_ms": from_ms, "to_ms": to_ms },
        "totals": summary,
    })))
}

#[derive(Deserialize, Default)]
pub struct EventsQuery {
    limit: Option<u64>,
    offset: Option<u64>,
    kind: Option<String>,
    agent_id: Option<String>,
}

pub async fn get_events(
    State(state): State<SharedState>,
    Query(q): Query<EventsQuery>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault_unlocked()?;
    if let Some(agg) = state.usage() { agg.flush_now().await; }
    let limit = q.limit.unwrap_or(100).min(1000) as i64;
    let offset = q.offset.unwrap_or(0) as i64;
    // Implement Store::query_events(limit, offset, kind, agent_id) in store/usage.rs
    let events = vault.store().query_events(limit, offset, q.kind.as_deref(), q.agent_id.as_deref())
        .map_err(|e| AppError::internal("usage-events-failed", e))?;
    Ok(Json(json!({ "events": events, "limit": limit, "offset": offset })))
}

pub async fn post_reset(State(state): State<SharedState>) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault_unlocked()?;
    let n = vault.store().reset_usage()
        .map_err(|e| AppError::internal("usage-reset-failed", e))?;
    Ok(Json(json!({ "deleted": n })))
}

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/api/v1/usage/summary", get(get_summary))
        .route("/api/v1/usage/events", get(get_events))
        .route("/api/v1/usage/reset", post(post_reset))
}
```

You'll need to add `query_events` to `store/usage.rs`:

```rust
pub fn query_events(&self, limit: i64, offset: i64, kind: Option<&str>, agent: Option<&str>)
    -> Result<Vec<UsageEvent>>
{
    let mut sql = String::from("SELECT ts_ms, kind, provider, model, agent_id, tokens_in, tokens_out, \
                                       cached_in, cost_usd, cache, outcome, latency_ms, error_kind, query_hash \
                                FROM usage_events WHERE 1=1");
    if kind.is_some()  { sql.push_str(" AND kind=?"); }
    if agent.is_some() { sql.push_str(" AND agent_id=?"); }
    sql.push_str(" ORDER BY ts_ms DESC LIMIT ? OFFSET ?");

    // … query with rusqlite params() and map rows to UsageEvent.
    // (Map enum strings back: kind, cache, outcome — small helper fn each)
    // Implementation left as straightforward rusqlite row mapping.
    todo!("row mapping — see UsageEvent struct in usage::types")
}
```

**No `todo!()` in shipped code** — implement the row mapping fully:

```rust
pub fn query_events(&self, limit: i64, offset: i64, kind: Option<&str>, agent: Option<&str>) -> Result<Vec<UsageEvent>> {
    let mut conn_stmt = match (kind, agent) {
        (Some(k), Some(a)) => self.conn.prepare(
            "SELECT ts_ms, kind, provider, model, agent_id, tokens_in, tokens_out, cached_in, cost_usd, cache, outcome, latency_ms, error_kind, query_hash
             FROM usage_events WHERE kind=?1 AND agent_id=?2 ORDER BY ts_ms DESC LIMIT ?3 OFFSET ?4")?,
        (Some(k), None) => self.conn.prepare(
            "SELECT ts_ms, kind, provider, model, agent_id, tokens_in, tokens_out, cached_in, cost_usd, cache, outcome, latency_ms, error_kind, query_hash
             FROM usage_events WHERE kind=?1 ORDER BY ts_ms DESC LIMIT ?2 OFFSET ?3")?,
        (None, Some(a)) => self.conn.prepare(
            "SELECT ts_ms, kind, provider, model, agent_id, tokens_in, tokens_out, cached_in, cost_usd, cache, outcome, latency_ms, error_kind, query_hash
             FROM usage_events WHERE agent_id=?1 ORDER BY ts_ms DESC LIMIT ?2 OFFSET ?3")?,
        (None, None) => self.conn.prepare(
            "SELECT ts_ms, kind, provider, model, agent_id, tokens_in, tokens_out, cached_in, cost_usd, cache, outcome, latency_ms, error_kind, query_hash
             FROM usage_events ORDER BY ts_ms DESC LIMIT ?1 OFFSET ?2")?,
    };
    let map_row = |r: &rusqlite::Row| -> rusqlite::Result<UsageEvent> {
        let kind_s: String = r.get(1)?;
        let cache_s: String = r.get(9)?;
        let outcome_s: String = r.get(10)?;
        let error_kind_s: Option<String> = r.get(12)?;
        Ok(UsageEvent {
            ts_ms: r.get(0)?,
            kind: parse_usage_kind(&kind_s),
            usage: TokenUsage {
                provider: r.get(2)?,
                model: r.get(3)?,
                tokens_in: r.get::<_, i64>(5)? as u32,
                tokens_out: r.get::<_, i64>(6)? as u32,
                cached_in: r.get::<_, i64>(7)? as u32,
            },
            agent_id: r.get(4)?,
            cost_usd: r.get(8)?,
            cache: parse_cache_outcome(&cache_s),
            outcome: parse_call_outcome(&outcome_s, error_kind_s.as_deref()),
            latency_ms: r.get::<_, i64>(11)? as u32,
            query_hash: r.get(13)?,
        })
    };
    let params_dyn: Vec<&dyn rusqlite::ToSql> = match (kind, agent) {
        (Some(k), Some(a)) => vec![&k, &a, &limit, &offset],
        (Some(k), None)    => vec![&k, &limit, &offset],
        (None, Some(a))    => vec![&a, &limit, &offset],
        (None, None)       => vec![&limit, &offset],
    };
    let rows = conn_stmt.query_map(&params_dyn[..], map_row)?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
}

fn parse_usage_kind(s: &str) -> UsageKind {
    match s {
        "llmchat" | "llm_chat" => UsageKind::LlmChat,
        "llmextract" | "llm_extract" => UsageKind::LlmExtract,
        "embed" => UsageKind::Embed,
        "rerank" => UsageKind::Rerank,
        "ocr" => UsageKind::Ocr,
        "asr" => UsageKind::Asr,
        "vlm" => UsageKind::Vlm,
        _ => UsageKind::LlmChat,
    }
}
fn parse_cache_outcome(s: &str) -> CacheOutcome {
    match s { "hit" => CacheOutcome::Hit, "bypass" => CacheOutcome::Bypass, _ => CacheOutcome::Miss }
}
fn parse_call_outcome(s: &str, err: Option<&str>) -> CallOutcome {
    match (s, err) {
        ("ok", _) => CallOutcome::Ok,
        ("retry", _) => CallOutcome::Retry { attempt: 1 },
        ("fail", Some(e)) => CallOutcome::Fail { error_kind: parse_error_kind(e) },
        _ => CallOutcome::Ok,
    }
}
fn parse_error_kind(s: &str) -> ErrorKind {
    match s {
        "parse" => ErrorKind::Parse,
        "grounding" => ErrorKind::Grounding,
        "timeout" => ErrorKind::Timeout,
        "quota" => ErrorKind::Quota,
        "network" => ErrorKind::Network,
        "schemainvalid" | "schema_invalid" => ErrorKind::SchemaInvalid,
        _ => ErrorKind::Other,
    }
}
```

Add `pub mod usage;` to `routes/mod.rs` and merge router in `lib.rs`:

```rust
.merge(routes::usage::router())
```

- [ ] **Step 4: Run — verify pass**

Run: `cargo test -p attune-server --test usage_routes 2>&1 | tail -10`
Expected: 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/src/routes/usage.rs rust/crates/attune-server/src/routes/mod.rs rust/crates/attune-server/src/lib.rs rust/crates/attune-core/src/store/usage.rs rust/crates/attune-server/tests/usage_routes.rs
git commit -m "feat(usage-api): GET /api/v1/usage/{summary,events} + POST /reset

Spec §5.2.
- summary: 7d default range, from/to RFC3339 query params
- events: paginated, filter by kind / agent_id
- reset: full DELETE FROM usage_events (UI 2-confirm in Settings)
- 503 vault-locked guard on all three
- store::query_events: row mapping with enum string parser helpers

3 integration tests."
```

---

## Task O: REST routes — `/api/v1/cache/*` + deprecate legacy `web_search_cache` route

**Files:**
- Create: `rust/crates/attune-server/src/routes/cache.rs`
- Modify: `rust/crates/attune-server/src/routes/web_search_cache.rs` → deprecation shim
- Create: `rust/crates/attune-server/tests/cache_routes.rs`

- [ ] **Step 1: Write failing test**

```rust
// rust/crates/attune-server/tests/cache_routes.rs
mod common; use common::TestServer;
use axum::http::StatusCode;

#[tokio::test]
async fn get_cache_llm_returns_count_and_size() {
    let srv = TestServer::start().await; srv.unlock_vault().await;
    let res = srv.client().get(srv.url("/api/v1/cache/llm")).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = res.json().await.unwrap();
    assert!(body.get("entries").is_some());
}

#[tokio::test]
async fn get_cache_all_aggregates_three_scopes() {
    let srv = TestServer::start().await; srv.unlock_vault().await;
    let res = srv.client().get(srv.url("/api/v1/cache/all")).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn delete_cache_llm_clears_and_returns_count() {
    let srv = TestServer::start().await; srv.unlock_vault().await;
    let res = srv.client().delete(srv.url("/api/v1/cache/llm")).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = res.json().await.unwrap();
    assert!(body["deleted"].as_u64().is_some());
}

#[tokio::test]
async fn delete_cache_invalid_scope_returns_400_kebab_code() {
    let srv = TestServer::start().await; srv.unlock_vault().await;
    let res = srv.client().delete(srv.url("/api/v1/cache/bogus")).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["code"], "cache-scope-invalid");
}

#[tokio::test]
async fn legacy_web_search_cache_route_emits_deprecation_header() {
    let srv = TestServer::start().await; srv.unlock_vault().await;
    let res = srv.client().get(srv.url("/api/v1/web_search_cache")).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers().get("deprecation").map(|v| v.to_str().unwrap()), Some("true"));
}
```

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p attune-server --test cache_routes 2>&1 | tail -15`
Expected: 5 failures.

- [ ] **Step 3: Implement `/cache/*` + deprecation shim**

```rust
// rust/crates/attune-server/src/routes/cache.rs
use crate::cache::CacheScope;
use crate::error::{AppError, AppResult};
use crate::state::SharedState;
use axum::{extract::{Path, State}, response::Json, routing::{get, delete}, Router};
use serde_json::json;

fn parse_scope(s: &str) -> Result<CacheScope, AppError> {
    match s {
        "llm" => Ok(CacheScope::Llm),
        "embed" => Ok(CacheScope::Embed),
        "search" => Ok(CacheScope::Search),
        "all" => Ok(CacheScope::All),
        _ => Err(AppError::bad_request("cache-scope-invalid", format!("unknown scope: {s}"))),
    }
}

pub async fn get_scope(State(state): State<SharedState>, Path(scope): Path<String>)
    -> AppResult<Json<serde_json::Value>>
{
    let vault = state.vault_unlocked()?;
    let s = parse_scope(&scope)?;
    let entries = vault.store().cache_count(s).map_err(|e| AppError::internal("cache-count-failed", e))?;
    Ok(Json(json!({ "scope": scope, "entries": entries })))
}

pub async fn delete_scope(State(state): State<SharedState>, Path(scope): Path<String>)
    -> AppResult<Json<serde_json::Value>>
{
    let vault = state.vault_unlocked()?;
    let s = parse_scope(&scope)?;
    let n = vault.store().cache_clear_scope(s).map_err(|e| AppError::internal("cache-clear-failed", e))?;
    Ok(Json(json!({ "scope": scope, "deleted": n })))
}

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/api/v1/cache/:scope", get(get_scope).delete(delete_scope))
}
```

Rewrite `routes/web_search_cache.rs`:

```rust
// rust/crates/attune-server/src/routes/web_search_cache.rs
//! DEPRECATED — replaced by /api/v1/cache/search.
//! Kept for 1 release as a shim that emits `Deprecation: true` header.

use crate::state::SharedState;
use axum::{extract::State, http::HeaderMap, response::IntoResponse, routing::{get, delete}, Router};

pub async fn legacy_get(State(state): State<SharedState>) -> impl IntoResponse {
    let vault = state.vault_unlocked();
    let n = vault.map(|v| v.store().web_search_cache_count().unwrap_or(0)).unwrap_or(0);
    let mut h = HeaderMap::new();
    h.insert("deprecation", "true".parse().unwrap());
    h.insert("link", r#"</api/v1/cache/search>; rel="successor-version""#.parse().unwrap());
    (h, axum::Json(serde_json::json!({ "entries": n, "deprecated_use": "/api/v1/cache/search" })))
}

pub async fn legacy_delete(State(state): State<SharedState>) -> impl IntoResponse {
    let vault = state.vault_unlocked();
    let n = vault.map(|v| v.store().clear_web_search_cache().unwrap_or(0)).unwrap_or(0);
    let mut h = HeaderMap::new();
    h.insert("deprecation", "true".parse().unwrap());
    (h, axum::Json(serde_json::json!({ "deleted": n })))
}

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/api/v1/web_search_cache", get(legacy_get).delete(legacy_delete))
}
```

Update `lib.rs` to merge both routers.

- [ ] **Step 4: Run — verify pass**

Run: `cargo test -p attune-server --test cache_routes 2>&1 | tail -10`
Expected: 5 pass.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/src/routes/cache.rs rust/crates/attune-server/src/routes/web_search_cache.rs rust/crates/attune-server/src/lib.rs rust/crates/attune-server/tests/cache_routes.rs
git commit -m "feat(cache-api): GET/DELETE /api/v1/cache/{llm,embed,search,all}

Spec §5.2 + §10.2 migration.
- Path-param scope, returns 400 + cache-scope-invalid on unknown
- Legacy /api/v1/web_search_cache emits Deprecation: true + Link: rel=successor-version
- 5 integration tests covering count, aggregation, clear, error code, deprecation header"
```

---

## Task P: `usage_headers` middleware — auto-inject `X-Attune-*`

**Files:**
- Create: `rust/crates/attune-server/src/middleware/usage_headers.rs`
- Modify: `rust/crates/attune-server/src/middleware.rs` (re-export)
- Modify: `rust/crates/attune-server/src/lib.rs` (apply layer)

- [ ] **Step 1: Write failing test**

```rust
// rust/crates/attune-server/tests/usage_headers_test.rs
mod common; use common::TestServer;

#[tokio::test]
async fn chat_response_carries_xattune_headers() {
    let srv = TestServer::start_with_mock_llm().await;
    srv.unlock_vault().await;
    let res = srv.client()
        .post(srv.url("/api/v1/chat"))
        .json(&serde_json::json!({ "message": "hello" }))
        .send().await.unwrap();
    assert!(res.headers().get("x-attune-cache").is_some(),     "X-Attune-Cache header missing");
    assert!(res.headers().get("x-attune-token-in").is_some(),  "X-Attune-Token-In missing");
    assert!(res.headers().get("x-attune-token-out").is_some(), "X-Attune-Token-Out missing");
    assert!(res.headers().get("x-attune-provider").is_some(),  "X-Attune-Provider missing");
}
```

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p attune-server --test usage_headers_test 2>&1 | tail -10`
Expected: FAIL — no headers.

- [ ] **Step 3: Implement middleware**

```rust
// rust/crates/attune-server/src/middleware/usage_headers.rs
//! Auto-injects X-Attune-* response headers from a UsageEvent placed in request extensions
//! by the route handler. Spec §5.3.

use crate::usage::UsageEvent;
use axum::{
    body::Body,
    extract::Request,
    http::HeaderValue,
    middleware::Next,
    response::Response,
};

pub async fn usage_headers_layer(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    // Routes that want headers placed the event in *response extensions* before return.
    if let Some(event) = response.extensions().get::<UsageEvent>().cloned() {
        let h = response.headers_mut();
        let _ = h.insert("x-attune-cache",
            HeaderValue::from_str(&format!("{:?}", event.cache).to_lowercase()).unwrap());
        let _ = h.insert("x-attune-token-in",
            HeaderValue::from_str(&event.usage.tokens_in.to_string()).unwrap());
        let _ = h.insert("x-attune-token-out",
            HeaderValue::from_str(&event.usage.tokens_out.to_string()).unwrap());
        if let Some(c) = event.cost_usd {
            let _ = h.insert("x-attune-cost-usd",
                HeaderValue::from_str(&format!("{:.6}", c)).unwrap());
        }
        let _ = h.insert("x-attune-latency-ms",
            HeaderValue::from_str(&event.latency_ms.to_string()).unwrap());
        let _ = h.insert("x-attune-provider",
            HeaderValue::from_str(&event.usage.provider).unwrap());
        let _ = h.insert("x-attune-model",
            HeaderValue::from_str(&event.usage.model).unwrap());
    }
    response
}
```

In `routes/chat.rs` (and `routes/llm.rs`), after computing the LLM response, attach the UsageEvent to response extensions:

```rust
let mut resp = Json(chat_payload).into_response();
resp.extensions_mut().insert(usage_event);
resp
```

In `lib.rs`, wrap the relevant router subtree:

```rust
.merge(
    Router::new()
        .merge(routes::chat::router())
        .merge(routes::llm::router())
        .merge(routes::classify::router())
        .merge(routes::search::router())
        .layer(axum::middleware::from_fn(middleware::usage_headers::usage_headers_layer))
)
```

- [ ] **Step 4: Run — verify pass**

Run: `cargo test -p attune-server --test usage_headers_test 2>&1 | tail -10`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/src/middleware/ rust/crates/attune-server/src/routes/chat.rs rust/crates/attune-server/src/routes/llm.rs rust/crates/attune-server/src/lib.rs rust/crates/attune-server/tests/usage_headers_test.rs
git commit -m "feat(middleware): usage_headers_layer auto-injects X-Attune-* on LLM responses

Spec §5.3. Wrapped around /api/v1/{chat,llm,classify,search}/* routers.
Routes attach UsageEvent to response extensions; middleware reads + writes headers.

1 integration test."
```

---

## Task Q: Property tests + proptest invariants

**Files:**
- Create: `rust/crates/attune-core/src/usage/tests/proptest.rs`

- [ ] **Step 1: Write proptests**

```rust
// rust/crates/attune-core/src/usage/tests/proptest.rs
use crate::usage::types::*;
use proptest::prelude::*;

proptest! {
    #[test]
    fn token_sums_are_non_negative_and_consistent(
        tin in 0u32..1_000_000,
        tout in 0u32..1_000_000,
        cin in 0u32..1_000_000
    ) {
        let u = TokenUsage { tokens_in: tin, tokens_out: tout, cached_in: cin,
                              model: "m".into(), provider: "p".into() };
        prop_assert!(u.tokens_in.checked_add(u.tokens_out).is_some(),
            "sum overflow risk: in={} out={}", u.tokens_in, u.tokens_out);
        prop_assert!(u.cached_in <= u.tokens_in.saturating_add(1_000_000),
            "cached_in plausibility");
    }

    #[test]
    fn cost_usd_non_negative_when_present(
        tin in 0u32..100_000,
        tout in 0u32..100_000,
    ) {
        if let Ok(cost) = crate::cost::estimate_cost_usd(tin, tout, "gpt-4o-mini") {
            prop_assert!(cost >= 0.0, "cost must be ≥ 0, got {}", cost);
        }
    }

    #[test]
    fn cache_outcome_round_trips_through_json(c in prop_oneof![
        Just(CacheOutcome::Hit),
        Just(CacheOutcome::Miss),
        Just(CacheOutcome::Bypass),
    ]) {
        let j = serde_json::to_string(&c).unwrap();
        let back: CacheOutcome = serde_json::from_str(&j).unwrap();
        prop_assert_eq!(c, back);
    }
}
```

- [ ] **Step 2: Run — verify pass (first run)**

Run: `cargo test -p attune-core usage::tests::proptest 2>&1 | tail -10`
Expected: 3 proptests pass with 256 cases each.

- [ ] **Step 3: Run with high iteration count**

```bash
PROPTEST_CASES=10000 cargo test -p attune-core usage::tests::proptest 2>&1 | tail -10
```

Expected: still pass (catches edge cases).

- [ ] **Step 4: Commit**

```bash
git add rust/crates/attune-core/src/usage/tests/proptest.rs
git commit -m "test(usage): proptest invariants — token sums, cost non-neg, cache JSON round-trip

Spec §9.1 line 'proptest ≥3 per agent'. PROPTEST_CASES=10000 verified."
```

---

## Task R: Integration E2E test — subprocess agent → record → query summary

**Files:**
- Create: `rust/crates/attune-core/tests/usage_endtoend.rs`

- [ ] **Step 1: Write the end-to-end test**

```rust
// rust/crates/attune-core/tests/usage_endtoend.rs
//! Spec §9.1: integration E2E ≥1 — subprocess agent → record → query summary

use attune_core::{
    UsageAggregator, UsageEvent, UsageKind, TokenUsage, CacheOutcome, CallOutcome,
};
use attune_core::store::Store;
use std::sync::Arc;
use tempfile::TempDir;

#[tokio::test]
async fn subprocess_agent_event_appears_in_summary() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(Store::open(dir.path().join("v.db")).unwrap());
    let agg = Arc::new(UsageAggregator::new(store.clone(), 50, 100));

    // Simulate subprocess agent recording an event
    let event = UsageEvent {
        ts_ms: chrono::Utc::now().timestamp_millis(),
        kind: UsageKind::LlmExtract,
        usage: TokenUsage { tokens_in: 500, tokens_out: 200, cached_in: 0,
                             model: "qwen2.5:3b".into(), provider: "ollama".into() },
        cost_usd: Some(0.0),
        cache: CacheOutcome::Miss,
        outcome: CallOutcome::Ok,
        latency_ms: 1800,
        agent_id: Some("defamation_extractor".into()),
        query_hash: None,
    };
    agg.record(event);
    agg.flush_now().await;

    let now = chrono::Utc::now().timestamp_millis();
    let summary = store.usage_summary(now - 60_000, now + 60_000).unwrap();
    assert_eq!(summary.events, 1);
    assert_eq!(summary.tokens_in, 500);
    assert_eq!(summary.tokens_out, 200);

    let events = store.query_events(10, 0, None, Some("defamation_extractor")).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].agent_id.as_deref(), Some("defamation_extractor"));
}
```

- [ ] **Step 2: Run — expect pass**

Run: `cargo test -p attune-core --test usage_endtoend 2>&1 | tail -10`
Expected: pass.

- [ ] **Step 3: Commit**

```bash
git add rust/crates/attune-core/tests/usage_endtoend.rs
git commit -m "test(usage): E2E integration — subprocess agent path → record → summary

Spec §9.1 integration line."
```

---

## Task S: Web UI — `<CostChip />` on ChatSendBar + Settings Usage tab

**Files:**
- Create: `rust/crates/attune-server/ui/src/components/CostChip.tsx`
- Create: `rust/crates/attune-server/ui/src/views/UsageView.tsx`
- Modify: `rust/crates/attune-server/ui/src/views/SettingsView.tsx`
- Modify: `rust/crates/attune-server/ui/src/components/ChatSendBar.tsx`
- Modify: `rust/crates/attune-server/ui/src/i18n/zh.ts` + `en.ts`

- [ ] **Step 1: Write failing Vitest unit test**

```tsx
// rust/crates/attune-server/ui/src/views/__tests__/UsageView.test.tsx
import { render, screen } from '@testing-library/react';
import { UsageView } from '../UsageView';
import { describe, it, expect, vi } from 'vitest';

vi.mock('../../api/usage', () => ({
  fetchUsageSummary: vi.fn().mockResolvedValue({
    totals: { events: 100, tokens_in: 50000, tokens_out: 12000, cost_usd: 0.21, cache_hit_rate: 0.42 },
  }),
}));

describe('UsageView', () => {
  it('renders totals from API', async () => {
    render(<UsageView />);
    expect(await screen.findByText(/50,000/)).toBeInTheDocument();
    expect(await screen.findByText(/\$0\.21/)).toBeInTheDocument();
    expect(await screen.findByText(/42%/)).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run — verify fail**

Run: `cd rust/crates/attune-server/ui && npm run test -- UsageView 2>&1 | tail -10`
Expected: FAIL — `UsageView` not found.

- [ ] **Step 3: Implement components**

```tsx
// rust/crates/attune-server/ui/src/components/CostChip.tsx
import { useEffect, useState } from 'react';
import { fetchUsageSummary } from '../api/usage';
import { useT } from '../i18n';

export function CostChip() {
  const t = useT();
  const [s, setS] = useState<{ tokens_in: number; tokens_out: number; cost_usd: number; cache_hit_rate: number } | null>(null);
  useEffect(() => {
    const tick = () => fetchUsageSummary({ from: 'session_start' }).then(r => setS(r.totals)).catch(() => {});
    tick(); const id = setInterval(tick, 10000); return () => clearInterval(id);
  }, []);
  if (!s) return null;
  const total = s.tokens_in + s.tokens_out;
  const tokStr = total > 1000 ? `~${(total / 1000).toFixed(1)}K` : `~${total}`;
  const costStr = s.cost_usd > 0 ? `$${s.cost_usd.toFixed(4)}` : t('cost.local');
  const hit = Math.round(s.cache_hit_rate * 100);
  return (
    <span className="cost-chip" title={t('cost.chip.hint')}>
      {tokStr} {t('cost.tokens')} · {costStr} · ⚡ {t('cost.cache')} {hit}%
    </span>
  );
}
```

```tsx
// rust/crates/attune-server/ui/src/views/UsageView.tsx
import { useEffect, useState } from 'react';
import { fetchUsageSummary, postUsageReset } from '../api/usage';
import { useT } from '../i18n';

type Range = '1d' | '7d' | '30d';

export function UsageView() {
  const t = useT();
  const [range, setRange] = useState<Range>('7d');
  const [data, setData] = useState<any>(null);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    const now = Date.now();
    const ms = range === '1d' ? 86_400_000 : range === '7d' ? 7 * 86_400_000 : 30 * 86_400_000;
    fetchUsageSummary({ from: new Date(now - ms).toISOString(), to: new Date(now).toISOString() })
      .then(setData)
      .catch(e => setErr(e?.code === 'vault-locked' ? t('usage.vaultLocked') : t('usage.fetchFailed')));
  }, [range, t]);

  if (err) return <div className="usage-empty">{err}</div>;
  if (!data) return <div className="usage-empty">{t('common.loading')}</div>;

  const totals = data.totals;
  return (
    <div className="usage-view">
      <div className="usage-header">
        <h2>{t('usage.title')}</h2>
        <div className="range-toggle">
          {(['1d', '7d', '30d'] as Range[]).map(r =>
            <button key={r} className={r === range ? 'active' : ''} onClick={() => setRange(r)}>{t(`usage.range.${r}`)}</button>
          )}
        </div>
      </div>
      <div className="usage-stats">
        <div><label>{t('usage.events')}</label><span>{totals.events.toLocaleString()}</span></div>
        <div><label>{t('usage.tokensIn')}</label><span>{totals.tokens_in.toLocaleString()}</span></div>
        <div><label>{t('usage.tokensOut')}</label><span>{totals.tokens_out.toLocaleString()}</span></div>
        <div><label>{t('usage.cost')}</label><span>${totals.cost_usd.toFixed(2)}</span></div>
        <div><label>{t('usage.cacheRate')}</label><span>{Math.round(totals.cache_hit_rate * 100)}%</span></div>
      </div>
      <footer>
        <small>{t('usage.estimateDisclaimer')}</small>
        <button onClick={async () => {
          if (confirm(t('usage.resetConfirm'))) {
            await postUsageReset();
            setRange(r => r);  // refetch
          }
        }}>{t('usage.reset')}</button>
      </footer>
    </div>
  );
}
```

```ts
// rust/crates/attune-server/ui/src/api/usage.ts
const BASE = '/api/v1';
export async function fetchUsageSummary(params: { from?: string; to?: string }) {
  const q = new URLSearchParams(params as any).toString();
  const res = await fetch(`${BASE}/usage/summary?${q}`);
  if (!res.ok) throw await res.json();
  return res.json();
}
export async function postUsageReset() {
  const res = await fetch(`${BASE}/usage/reset`, { method: 'POST' });
  if (!res.ok) throw await res.json();
  return res.json();
}
```

Add i18n keys to both `zh.ts` and `en.ts` (per CLAUDE.md i18n strict rule — both files MUST stay in sync):

```ts
// zh.ts additions
'usage.title': 'Token & Cache 使用',
'usage.events': '调用次数',
'usage.tokensIn': '输入 Token',
'usage.tokensOut': '输出 Token',
'usage.cost': '累计费用',
'usage.cacheRate': '缓存命中率',
'usage.estimateDisclaimer': '估算 ±15%，以服务商账单为准',
'usage.resetConfirm': '清空所有 Usage 记录？此操作不可恢复',
'usage.reset': '清空数据',
'usage.vaultLocked': 'Vault 已锁定，请先解锁',
'usage.fetchFailed': '获取 Usage 数据失败',
'usage.range.1d': '今日',
'usage.range.7d': '7 天',
'usage.range.30d': '30 天',
'cost.tokens': 'tok',
'cost.cache': '缓存',
'cost.local': '本地',
'cost.chip.hint': '本次会话累计成本（估算 ±15%）',

// en.ts additions (parallel keys, English values)
'usage.title': 'Token & Cache Usage',
// ... etc
```

Mount in `SettingsView.tsx` as a new tab; mount `<CostChip />` in `ChatSendBar.tsx` next to the send button.

- [ ] **Step 4: Run — verify pass**

Run: `cd rust/crates/attune-server/ui && npm run test 2>&1 | tail -10`
Expected: pass.

Run grep guard (per CLAUDE.md i18n strict): both commands must produce **no output**:

```bash
cd rust/crates/attune-server/ui/src
diff <(grep -oP "^\s+'[\w.]+'\s*:" i18n/zh.ts | tr -d " ':" | sort) \
     <(grep -oP "^\s+'[\w.]+'\s*:" i18n/en.ts | tr -d " ':" | sort)
```

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/ui/
git commit -m "feat(ui): UsageView Settings tab + CostChip in ChatSendBar

Spec §8.2 (UI display rules).
- /api/v1/usage/summary client + range toggle (1d/7d/30d)
- Reset button with confirm dialog
- CostChip polls every 10s, shows ~tok · \$cost · cache%
- i18n: zh.ts + en.ts both updated (key diff = 0, per CLAUDE.md i18n rule)
- Vitest: locale-aware render + vault-locked placeholder"
```

---

## Task T: Performance bench — record_event throughput

**Files:**
- Create: `rust/crates/attune-core/benches/usage_record.rs`

- [ ] **Step 1: Write criterion bench**

```rust
// rust/crates/attune-core/benches/usage_record.rs
use criterion::{criterion_group, criterion_main, Criterion};
use attune_core::{UsageAggregator, UsageEvent, UsageKind, TokenUsage, CacheOutcome, CallOutcome};
use attune_core::store::Store;
use std::sync::Arc;
use tempfile::TempDir;

fn bench_record_1k(c: &mut Criterion) {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(Store::open(dir.path().join("b.db")).unwrap());
    let agg = UsageAggregator::new(store, 100, 10_000);
    let event = UsageEvent {
        ts_ms: 1717000000000,
        kind: UsageKind::LlmChat,
        usage: TokenUsage::empty("ollama", "qwen2.5:3b"),
        cost_usd: Some(0.0),
        cache: CacheOutcome::Miss,
        outcome: CallOutcome::Ok,
        latency_ms: 100,
        agent_id: None,
        query_hash: None,
    };
    c.bench_function("usage_record_1k_into_buffer", |b| b.iter(|| {
        for _ in 0..1000 { agg.record(event.clone()); }
    }));
}

criterion_group!(benches, bench_record_1k);
criterion_main!(benches);
```

Add to `rust/crates/attune-core/Cargo.toml`:

```toml
[[bench]]
name = "usage_record"
harness = false
```

- [ ] **Step 2: Run bench**

Run: `cargo bench -p attune-core --bench usage_record 2>&1 | tail -20`
Expected: p99 < 10ms for 1000 records (spec §9.3 baseline 1000 events/s p99 < 10ms).

If p99 exceeds, investigate and re-tune `cap` / lock granularity (use `parking_lot::Mutex` or replace VecDeque with crossbeam channel).

- [ ] **Step 3: Document baseline in RELEASE.md**

Edit `RELEASE.md` v1.0.6 section (create if missing):

```markdown
## v1.0.6 — Cache / Context / Token API (planned 2026-06-05)

### Highlights
- Unified UsageEvent telemetry across all LLM/Embed/Rerank/OCR/ASR/VLM calls
- /api/v1/usage/{summary,events,reset} + /api/v1/cache/{llm,embed,search,all}
- Web UI: Settings → Usage tab, ChatSendBar cost chip
- X-Attune-* response headers (Cache, Token-In, Token-Out, Cost-USD, Latency, Provider, Model)

### Breaking changes (Rust API)
- `LlmProvider::chat` signature: `Result<String>` → `Result<(String, TokenUsage)>`
- `EmbeddingProvider::embed` signature: similar tuple change

### Migration
- /api/v1/web_search_cache emits `Deprecation: true` header for 1 release; switch clients to `/api/v1/cache/search`
- Old `settings.llm.daily_token_used` field removed; UsageView aggregates real events instead

### Known limitations
- Token estimation is heuristic (±15%); tiktoken feature deferred to v1.2
- Semantic cache (embedding similarity ≥0.95) deferred to v2.0
- K3 RISC-V: L2 cache defaults disabled (eMMC IOPS); see `settings.cache.l2_enabled`

### Performance baseline (v1.0.6)
- record_event p99: <BENCHED VALUE> ms (target < 2 ms)
- /usage/summary 7d range p99: <MEASURED> ms (target < 100 ms)
```

- [ ] **Step 4: Commit**

```bash
git add rust/crates/attune-core/benches/usage_record.rs rust/crates/attune-core/Cargo.toml RELEASE.md
git commit -m "bench(usage): record_event criterion + v1.0.6 RELEASE.md draft

Spec §9.3 performance baseline."
```

---

## Task U: Final integration — wire UsageAggregator into chat.rs hot path

**Files:**
- Modify: `rust/crates/attune-server/src/routes/chat.rs`

This is the **first production call site** to actually use the guard + recorder end-to-end.

- [ ] **Step 1: Write failing integration test**

```rust
// rust/crates/attune-server/tests/chat_records_usage.rs
mod common; use common::TestServer;

#[tokio::test]
async fn chat_call_records_one_usage_event() {
    let srv = TestServer::start_with_mock_llm().await;
    srv.unlock_vault().await;

    let before = srv.client().get(srv.url("/api/v1/usage/summary")).send().await.unwrap()
        .json::<serde_json::Value>().await.unwrap();
    let before_n = before["totals"]["events"].as_u64().unwrap_or(0);

    srv.client().post(srv.url("/api/v1/chat"))
        .json(&serde_json::json!({ "message": "hi" }))
        .send().await.unwrap();

    let after = srv.client().get(srv.url("/api/v1/usage/summary")).send().await.unwrap()
        .json::<serde_json::Value>().await.unwrap();
    let after_n = after["totals"]["events"].as_u64().unwrap();
    assert_eq!(after_n, before_n + 1, "exactly one usage event recorded per chat call");
}
```

- [ ] **Step 2: Run — verify fail**

Run: `cargo test -p attune-server --test chat_records_usage 2>&1 | tail -10`
Expected: FAIL — event not recorded.

- [ ] **Step 3: Wire guard in `chat.rs`**

In `rust/crates/attune-server/src/routes/chat.rs`, locate the handler that calls `llm.chat(...)` and wrap:

```rust
use attune_core::{UsageRecorderGuard, UsageKind, CacheOutcome, CallOutcome};

// inside handler, before LLM call:
let agg = state.usage().ok_or_else(|| AppError::internal("usage-unavailable", "aggregator not installed"))?;
let recorder = {
    let agg = agg.clone();
    Box::new(move |e| agg.record(e))
};
let mut guard = UsageRecorderGuard::new(
    UsageKind::LlmChat,
    llm.provider_label(),  // add this method to LlmProvider if absent
    llm.model_name(),
    recorder,
);

let result = llm.chat(&system, &user);
match result {
    Ok((reply, usage)) => {
        guard.complete(usage.clone(), CacheOutcome::Miss, CallOutcome::Ok);
        let mut resp = Json(json!({ "reply": reply })).into_response();
        // Attach event to extensions for the X-Attune-* middleware:
        resp.extensions_mut().insert(UsageEvent {
            ts_ms: chrono::Utc::now().timestamp_millis(),
            kind: UsageKind::LlmChat,
            usage,
            cost_usd: None,
            cache: CacheOutcome::Miss,
            outcome: CallOutcome::Ok,
            latency_ms: 0,
            agent_id: None,
            query_hash: None,
        });
        Ok(resp)
    }
    Err(e) => {
        guard.complete(
            TokenUsage::empty(llm.provider_label(), llm.model_name()),
            CacheOutcome::Bypass,
            CallOutcome::Fail { error_kind: ErrorKind::Other }
        );
        Err(AppError::internal("chat-failed", e))
    }
}
```

Add `fn provider_label(&self) -> &str { "ollama" }` (or similar) default impl to `LlmProvider` trait if no equivalent exists; override per concrete provider.

- [ ] **Step 4: Run — verify pass**

Run: `cargo test -p attune-server --test chat_records_usage 2>&1 | tail -5`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/src/routes/chat.rs rust/crates/attune-core/src/llm.rs rust/crates/attune-server/tests/chat_records_usage.rs
git commit -m "feat(chat): wire UsageRecorderGuard at chat handler — first production call site

Spec §11 risk 1 mitigation 4 — gradual rollout, v1.1.0 primary path first.
- Guard.complete called on both Ok and Err branches
- UsageEvent attached to response extensions for X-Attune-* middleware

1 integration test: chat call increments /usage/summary events by exactly 1."
```

---

## Task V: Self-review + merge to develop

**Files:**
- Modify: none (review only)

- [ ] **Step 1: Spec coverage check**

Open `docs/superpowers/specs/2026-05-28-cache-context-token-standard-api.md` and walk through §1-11. For each requirement, point to the implementing task:

| Spec section | Task |
|--------------|------|
| §1 痛点 (5 items) | A1-V end-to-end |
| §2.1 ✅ items (8) | B, C, D, N, O, P, S |
| §3 Architecture | B, C, H |
| §3 DB tables | D |
| §3 Cache layers L1/L2 | E, F |
| §4.1 New modules | B, C, D, E, F, H, N, O, P, S |
| §4.2 Modified | I, J, K, L, P, U |
| §5.1 Rust types | B, C, G |
| §5.2 REST endpoints | N, O |
| §5.3 Response headers | P |
| §5.4 CLI (v1.2 deferred) | NOT IN SCOPE |
| §6.1-6.4 Extension points | M (frozen API surface — see Plan A2) |
| §7 Errors + boundaries | N, O, U |
| §8 Cost contract | S (UI display), T (RELEASE.md) |
| §9.1 6-class test floor | B/C/D (golden inline), Q (proptest), Q (boundary), N/O (error), R (E2E), every fix gets regression — covered via TDD |
| §9.2 Scenarios | N, O, P, S, U (vault locked, multi-agent via E2E) |
| §9.3 Perf baseline | T |
| §10 Backward compat | D (CREATE IF NOT EXISTS), O (deprecation), Task I/J (1-release-grace) |
| §11 Risk 1 | I, J, G (compile-time + guard panic) |
| §11 Risk 2 | H, T (bench) |
| §11 Risk 3 | S (UI 3-line breakdown), T (RELEASE notes) |
| §11 Risk 4 | S (`~` prefix), T (RELEASE notes) |
| §11 Risk 5 | log_queries default false; T documents |
| §11 Risk 6 | settings.cache.retention_days + K3 doc note in T |

- [ ] **Step 2: Placeholder scan**

```bash
cd /tmp/attune-cache-token-api
grep -rn "todo!\|TODO\|FIXME\|placeholder\|appropriate error handling" --include="*.rs" rust/crates/attune-core/src/usage rust/crates/attune-core/src/cache rust/crates/attune-server/src/routes/usage.rs rust/crates/attune-server/src/routes/cache.rs rust/crates/attune-server/src/middleware
```

Expected: **no output**. If any hits, fix them inline.

- [ ] **Step 3: Type consistency check**

```bash
grep -rn "TokenUsage::empty\|UsageEvent {" --include="*.rs" rust/ | head -20
```

Verify every `UsageEvent {` literal has all 10 fields (ts_ms, kind, usage, cost_usd, cache, outcome, latency_ms, agent_id, query_hash, …no extras). If a field name changed mid-plan, fix.

- [ ] **Step 4: Run full test suite**

```bash
cargo test --workspace 2>&1 | tail -10
```

Expected: all tests pass. No new `#[ignore]` introduced beyond Task baseline (per CLAUDE.md §7.2 Gate 2).

- [ ] **Step 5: Run clippy**

```bash
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -10
```

Expected: clean.

- [ ] **Step 6: Merge to develop**

```bash
cd /data/company/project/attune
git checkout develop
git pull origin develop
git merge --no-ff feat/cache-token-api -m "merge: feat/cache-token-api → develop (v1.0.6 cache+token API)

Spec: docs/superpowers/specs/2026-05-28-cache-context-token-standard-api.md
Plan: docs/superpowers/plans/2026-05-28-cache-context-token-api.md

22 commits implementing:
- TokenUsage / UsageEvent / UsageRecorderGuard public API
- CacheBackend trait + MemoryLruCache (L1) + SqliteEncryptedCache (L2)
- 3 new SQLite tables (usage_events, llm_cache, embed_cache) — CREATE IF NOT EXISTS migration
- /api/v1/usage/{summary,events,reset} + /api/v1/cache/{llm,embed,search,all}
- X-Attune-* response headers middleware
- BREAKING: LlmProvider::chat / EmbeddingProvider::embed return tuple with TokenUsage
- Web UI: UsageView + CostChip (i18n complete, zh/en key diff = 0)
- 30+ unit/integration/proptest tests, criterion bench

UNBLOCKS: Plan A2 (hybrid-token-routing) — Task M API freeze commit is the anchor.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
git push origin develop

# clean up:
git worktree remove /tmp/attune-cache-token-api
git branch -d feat/cache-token-api
```

- [ ] **Step 7: Tag development snapshot (no GA — per §7.1 minor still in develop)**

```bash
git tag v1.0.6-alpha.1 -m "v1.0.6 alpha — cache/token API merged to develop"
git push origin v1.0.6-alpha.1
```

- [ ] **Step 8: Delete plan markdown per CLAUDE.md §3.2 lifecycle**

```bash
git rm docs/superpowers/plans/2026-05-28-cache-context-token-api.md
git commit -m "chore(docs): remove implemented plan per §3.2 lifecycle

Plan 2026-05-28-cache-context-token-api.md implementation complete.
Conclusions captured in:
- Spec: docs/superpowers/specs/2026-05-28-cache-context-token-standard-api.md (unchanged, evergreen)
- RELEASE.md v1.0.6 section
- DEVELOP.md (if any architectural notes were promoted)"
git push origin develop
```

---

## Self-Review Notes (post-plan, pre-implementation)

**Spec coverage gaps:** None identified — every § maps to ≥1 task. §5.4 CLI commands explicitly deferred to v1.2 per spec §2.3.

**Placeholder scan:** `todo!()` appears once in Task N step 3 prose intentionally to demonstrate what NOT to ship, immediately followed by the full implementation. Watch for it during impl.

**Type consistency:** `TokenUsage::empty("provider", "model")` is the canonical constructor. `UsageEvent` has 10 fields end to end. `CacheOutcome` is 3-variant. `CallOutcome` is enum with variant data. `UsageKind` is 7-variant. `ErrorKind` is 7-variant. Verified across all tasks.

**Risks during execution:**
1. Task I (`chat` signature break) is the biggest single commit — touch every call site. Plan to do it in a dedicated worktree and not interleave with feature work.
2. `tokio::time::advance` in Task H test requires `tokio` features `["test-util"]`. If absent in attune-core's Cargo.toml, either add it or drop the timing-paused test (keep the buffer-cap test which is sufficient).
3. `crypto::aead_encrypt` / `aead_decrypt` exact names are TBD — grep first.

**Plan A2 dependency:** Task M commit is the explicit unblock signal. Confirm A2 task tracker entry has `blockedBy = [task-M-commit-SHA]`.

---

**Execution choice:** plan saved. Recommend Subagent-Driven (one subagent per task A→V, fresh context).


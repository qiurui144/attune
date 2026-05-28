# Spec: Cache / Context / Token 标准 API 化

- **Date**: 2026-05-28
- **Author**: AI architect (auto-drafted, pending user review)
- **Status**: DRAFT — awaiting v1.1+ slot (per CLAUDE.md ⭐ v1.0 GA Roadmap)
- **Tracker**: TBD (用户评审后 TaskCreate)
- **Linked specs**:
  - `2026-04-27-resource-governor-design.md` (governor budget 已实现)
  - `2026-05-22-robust-llm-infra.md` (LLM 多 provider 加固)
  - 全局 CLAUDE.md §3.1 「架构级别设计铁律」(本 spec 即 11 节模板产物)

---

## TL;DR

attune 现状:`crate::cost`(token + USD 估算)、`crate::context_budget`(window plan)、`resource_governor::budget`(任务级 CPU/RAM/LLM-rate)、`web_search_cache`(单独路由)、各 LLM call site 自己拼 usage,**没有统一标准**。LLM gateway / embedding / chat / agent / OCR / VLM / ASR 各自维护一份 cache + token 计数,UI 无法显示统一面板,失败 telemetry 也散落。

本 spec 把以下三类基础设施收敛成一组**对内 Rust trait + 对外 REST 标准 API**:
1. **Cache contract** — 任意 LLM/Embedding/Search call 命中 cache / miss 时,统一 record + UI 可查
2. **Token usage contract** — 每次 LLM call 强制返回 `TokenUsage`,统一聚合 + UI 显示
3. **Context budget contract** — `plan_context()` 是唯一入口,LLM call 必须先经它 → 不再有"小模型 32K 写死 2000 字注入"

---

## 1. 目标定位

### 1.1 解决什么用户痛点

| 痛点 | 现状 | 本 spec 解决 |
|------|------|--------------|
| 「我到底花了多少 token」用户问不到 | `cost.rs` 有估算函数但 UI 不显示累计 | `GET /api/v1/usage/summary` + ChatSendBar 常驻 chip |
| 「这次 query 是缓存命中吗」用户不知道 | `web_search_cache` 单独一个 endpoint,LLM 没 cache 暴露 | `X-Attune-Cache: hit\|miss\|bypass` response header + UI 角标 |
| 「为什么 chat 没引用我刚加的文档」 | search 用 `INJECTION_BUDGET` 写死,gemini 1M 窗口浪费 99% | 强制走 `plan_context()`,UI Settings 可见 `BudgetPlan` |
| 「LLM gateway 单 provider 挂了 fallback 怎么算钱」 | per-provider 各算各的,gateway 不汇总 | `UsageEvent { provider, model, kind, tokens, cost_usd, cache }` 落 DB |
| 失败 telemetry 散落 | `agent × model` 失败率 > 30% UI 提示规则没数据源 | `UsageEvent.outcome ∈ {ok, retry, fail}` 聚合 |

### 1.2 产品 positioning 对齐

- **三层成本** (CLAUDE.md「成本感知与触发契约」):cache hit = 零成本 / 本地推理 = 本地算力 / 远端 token = 时间金钱。本 spec 让三类**首次有同一份数据 schema** 表达
- **隐私优先**(1Password 式):cache key 必须经过 `hash` 不存原文,UsageEvent 中 query 字段可选 + 默认关
- **混合智能**(本地优先):routing 决策(下一份 spec 「Hybrid Token Strategy」)的输入数据由本 spec 提供 — 没标准 cache/token API 就没法做 cost-aware routing

---

## 2. 范围边界

### 2.1 做什么(v1.1.x 完成)

✅ `attune-core::usage` 新 crate-internal module — `TokenUsage` / `CacheOutcome` / `UsageEvent` 三 struct,所有 LLM/Embed/Rerank call site 必须返回
✅ `attune-core::cache` 新 module — `CacheBackend` trait + `MemoryCache` / `SqliteCache` 两实现,LLM/Embed/Search 通用
✅ `attune-server::routes::usage` 新路由文件 — `GET /api/v1/usage/{summary,events,reset}` 三 endpoint
✅ `attune-server::routes::cache` 新路由文件 — 整合 `web_search_cache.rs` + 加 LLM/embed cache count/clear,统一 `GET/DELETE /api/v1/cache/{llm,embed,search,all}`
✅ Response header 标准:`X-Attune-Cache: hit|miss|bypass` + `X-Attune-Token-In` + `X-Attune-Token-Out` + `X-Attune-Cost-USD`
✅ Web UI 「Usage」tab(Settings 模态新增) — 今日/7 天/30 天 token + cost 累计;按 provider/agent 分组;cache hit rate
✅ ChatSendBar 常驻 `~1.2K tok · $0.0004 · ⚡ cache 67%` chip
✅ 配置:`settings.usage.retention_days = 30` / `settings.usage.log_queries = false`(默认隐藏)

### 2.2 不做什么

❌ 真 tokenizer(tiktoken / claude.json)绑定 — 仍走 `cost::estimate_tokens` 启发式(误差 ±15% 够用)
❌ 计费 source-of-truth — usage 数据用于 UI + telemetry,**不是**给 attune-pro 会员配额扣费(那走 `cloud_client::member_session` 服务端 truth)
❌ 跨设备 usage 同步 — 本地 SQLite only,云同步走 attune-pro Pro 会员功能
❌ Cache 跨 vault 共享 — 每个 vault 独立 cache 表(per 隐私边界)
❌ Distributed cache(Redis 等) — single-binary 定位,不引外部依赖

### 2.3 v.x 后续

| 版本 | 增量 |
|------|------|
| v1.2 | tiktoken 真 tokenizer 可选 feature(`--features exact-tokens`) |
| v1.3 | UsageEvent 推送到 attune-pro cloud usage dashboard(opt-in) |
| v2.0 | Semantic cache(embedding 相似度 ≥ 0.95 命中 LLM cache)— 当前 v1.1 只做 exact-hash |

---

## 3. 架构数据流

```
┌──────────────────────────────────────────────────────────────────────────┐
│                              Caller layer                                 │
│   chat.rs · agent_runner.rs · classifier.rs · skill_evolution · embed.rs  │
└────────────┬─────────────────────────────────────────────┬──────────────┘
             │                                              │
             ▼                                              ▼
   ┌──────────────────┐                          ┌────────────────────┐
   │ context_budget   │                          │      cache         │ <-- NEW trait
   │  plan_context()  │ ← FORCED entry           │  CacheBackend      │
   │ → BudgetPlan     │                          │  - get(key) → Hit  │
   └────────┬─────────┘                          │  - put(key, val)   │
            │                                    │  - evict / clear   │
            ▼                                    └─────┬──────────────┘
   ┌──────────────────┐                                │
   │  LLM provider    │     ────────── hit ────────────┘
   │  (OpenAI compat) │
   │                  │     ────────── miss ──┐
   └────────┬─────────┘                       │
            │                                  ▼
            ▼                          ┌──────────────────┐
   ┌──────────────────┐                │  vendor API call │
   │   TokenUsage     │ ← FORCED       │  /v1/chat/comp.. │
   │   {in, out,      │   return       └────────┬─────────┘
   │   cached_in,     │                          │
   │   model, ...}    │                          ▼
   └────────┬─────────┘                ┌──────────────────┐
            │                          │ vendor returns   │
            ▼                          │ usage{ }         │
   ┌──────────────────┐                └──────────────────┘
   │   UsageEvent     │ ← record into SQLite (table usage_events)
   │   record_event() │
   └────────┬─────────┘
            │
            ▼
   ┌──────────────────┐         ┌────────────────────────────────────────┐
   │ Aggregator       │ ─────►  │ HTTP response headers                  │
   │ (worker thread)  │         │  X-Attune-Cache / Token-In / Cost-USD  │
   │                  │ ─────►  │ GET /api/v1/usage/summary              │
   │                  │ ─────►  │ Web UI Usage tab + ChatSendBar chip    │
   └──────────────────┘         └────────────────────────────────────────┘
```

### DB tables(新增,加密 vault 内)

```sql
-- usage_events: 每次 LLM/embed/rerank/cache call 一行(经 record_event)
CREATE TABLE usage_events (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  ts          INTEGER NOT NULL,                      -- unix epoch ms
  kind        TEXT    NOT NULL,                      -- llm_chat / llm_extract / embed / rerank / ocr / asr
  provider    TEXT    NOT NULL,                      -- ollama / openai / gemini / cloud_gateway / k3_local
  model       TEXT    NOT NULL,                      -- qwen2.5:3b / gemini-1.5-flash / ...
  agent_id    TEXT,                                  -- 调用方 agent(null = 直接 chat)
  tokens_in   INTEGER NOT NULL,
  tokens_out  INTEGER NOT NULL,
  cached_in   INTEGER NOT NULL DEFAULT 0,            -- prompt cache hit(Anthropic / OpenAI)
  cost_usd    REAL,                                  -- null = 未知 model
  cache       TEXT    NOT NULL,                      -- hit / miss / bypass
  outcome     TEXT    NOT NULL,                      -- ok / retry / fail
  latency_ms  INTEGER NOT NULL,
  error_kind  TEXT,                                  -- parse / grounding / timeout / quota / network(null = ok)
  query_hash  TEXT                                   -- BLAKE3(query) 16-hex,不存原文
);
CREATE INDEX idx_usage_ts ON usage_events(ts);
CREATE INDEX idx_usage_kind_provider ON usage_events(kind, provider);
CREATE INDEX idx_usage_agent ON usage_events(agent_id) WHERE agent_id IS NOT NULL;

-- llm_cache: response cache(exact prompt hash)
CREATE TABLE llm_cache (
  key          TEXT PRIMARY KEY,                     -- BLAKE3(model + prompt) 32-hex
  model        TEXT NOT NULL,
  response     BLOB NOT NULL,                        -- AES-256-GCM encrypted(DEK)
  tokens_in    INTEGER NOT NULL,
  tokens_out   INTEGER NOT NULL,
  created_ts   INTEGER NOT NULL,
  last_hit_ts  INTEGER NOT NULL,
  hit_count    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_llm_cache_lru ON llm_cache(last_hit_ts);

-- embed_cache: embedding 向量 cache(exact text hash)
CREATE TABLE embed_cache (
  key          TEXT PRIMARY KEY,                     -- BLAKE3(model + text)
  model        TEXT NOT NULL,
  vector       BLOB NOT NULL,                        -- f16 量化,plain(向量本身已 PII-safe)
  dim          INTEGER NOT NULL,
  created_ts   INTEGER NOT NULL,
  last_hit_ts  INTEGER NOT NULL
);
```

### Cache layers(从近到远)

```
L1: in-memory LRU (CacheBackend::MemoryCache)
     - 默认 cap = 512 entries / 64MB,LRU 淘汰
     - llm_chat / embed 通用,进程退出即丢
L2: SQLite encrypted (CacheBackend::SqliteCache)
     - llm_cache + embed_cache 两张表
     - response BLOB 经 DEK 加密(per attune vault model)
     - retention: 默认 30 天,可调 `settings.cache.retention_days`
L3: provider-side prompt caching (Anthropic prompt-cache / OpenAI prompt-cache)
     - 不在 attune 控制,但 TokenUsage.cached_in 字段透传
     - UsageEvent.cache = 'hit' 仅指 L1/L2,vendor 端 prompt cache 单独记录 cached_in
```

---

## 4. 模块边界

### 4.1 新增

| Crate / Path | 角色 |
|--------------|------|
| `attune-core::usage` 新 module | `TokenUsage` / `UsageEvent` / `UsageAggregator` |
| `attune-core::cache` 新 module | `CacheBackend` trait + 2 实现 |
| `attune-core::store::usage` 新文件 | DB CRUD: `record_usage` / `query_summary` / `purge_old` |
| `attune-core::store::cache` 新文件 | DB CRUD: `cache_get` / `cache_put` / `cache_clear` |
| `attune-server::routes::usage` 新 route | `GET /api/v1/usage/{summary,events,reset}` |
| `attune-server::routes::cache` 整合 route | `GET/DELETE /api/v1/cache/{llm,embed,search,all}` |
| `attune-server::middleware::usage_headers` 新 layer | 自动注入 `X-Attune-Cache/Token-In/Cost-USD` |
| Web UI `ui/src/views/UsageView.tsx` 新视图 | Settings → Usage tab |
| Web UI `ui/src/components/ChatSendBar.tsx` 改 | 加 `~tok · $cost · cache%` chip |

### 4.2 改造现有(call site 接入新 API)

| 既有 file | 改动 |
|-----------|------|
| `attune-core/src/llm.rs` | `LlmClient::chat` 返回类型 `Result<ChatResponse>` 改为含 `TokenUsage`;内部走 `CacheBackend::get_or_compute` |
| `attune-core/src/embed.rs` | 同上,`EmbeddingProvider::embed` 走 cache |
| `attune-core/src/agent_runner.rs` | spawn subprocess 时传 `AGENT_ID` env,subprocess 内 record_event 时带上 |
| `attune-core/src/context_budget.rs` | `plan_context()` 加 `BudgetPlan::tokens_in_used` 输出,call site 必须传给 record_event |
| `attune-server/src/routes/chat.rs` | response 走 `usage_headers` middleware |
| `attune-server/src/routes/web_search_cache.rs` | 改名 `cache.rs`,合并 LLM/embed cache 路由 |
| `attune-core/src/cost.rs` | 保留,作为 `estimate_cost_usd` source-of-truth;新增 `pricing_for_provider(provider, model)` 解析 cloud gateway / BYOK 价格差异 |

### 4.3 不动

- `crate::resource_governor::*` — governor 是 worker 任务级 CPU/RAM 限制,与本 spec usage 数据是**两层**(governor 决定能不能跑;usage 记录跑完花了什么)。**不合并**
- `crate::cloud_client::member_session` — 服务端配额 truth 由 cloud gateway 维护,本 spec usage 是本地 telemetry,不互替

---

## 5. API 契约

### 5.1 Rust trait(crate-internal)

```rust
/// attune-core::usage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub tokens_in: u32,
    pub tokens_out: u32,
    /// Vendor-side prompt cache hits(Anthropic prompt-cache / OpenAI),0 = 不支持
    pub cached_in: u32,
    pub model: String,
    pub provider: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheOutcome {
    Hit,     // L1 或 L2 命中
    Miss,    // 未命中,真发起 upstream
    Bypass,  // 用户禁用 cache 或 nocache hint
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallOutcome {
    Ok,
    Retry { attempt: u8 },
    Fail { error_kind: ErrorKind },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageEvent {
    pub ts_ms: i64,
    pub kind: UsageKind,         // LlmChat / LlmExtract / Embed / Rerank / Ocr / Asr / Vlm
    pub usage: TokenUsage,
    pub cost_usd: Option<f64>,
    pub cache: CacheOutcome,
    pub outcome: CallOutcome,
    pub latency_ms: u32,
    pub agent_id: Option<String>,
    pub query_hash: Option<String>,  // 16-hex BLAKE3 prefix,默认 None
}

pub trait UsageRecorder: Send + Sync {
    fn record(&self, event: UsageEvent);
}

/// attune-core::cache
#[async_trait::async_trait]
pub trait CacheBackend: Send + Sync {
    async fn get(&self, key: &str) -> Option<CachedValue>;
    async fn put(&self, key: &str, value: CachedValue, ttl_secs: Option<u32>);
    async fn clear(&self, scope: CacheScope) -> usize;
    async fn count(&self, scope: CacheScope) -> usize;
}

pub enum CacheScope { Llm, Embed, Search, All }

pub struct CachedValue {
    pub bytes: Vec<u8>,
    pub tokens_in: u32,
    pub tokens_out: u32,
    pub model: String,
}
```

### 5.2 REST endpoints

```http
GET /api/v1/usage/summary?from=2026-05-21&to=2026-05-28&group_by=provider
→ 200 OK
{
  "range": { "from": "...", "to": "..." },
  "totals": { "events": 4231, "tokens_in": 1_842_000, "tokens_out": 380_000,
              "cost_usd": 0.84, "cache_hit_rate": 0.42 },
  "by_provider": [
    { "provider": "cloud_gateway", "model": "gemini-1.5-flash",
      "events": 2100, "tokens_in": 1_400_000, "tokens_out": 280_000,
      "cost_usd": 0.42, "cache_hit_rate": 0.35 },
    { "provider": "ollama", "model": "qwen2.5:3b",
      "events": 2131, "tokens_in": 442_000, "tokens_out": 100_000,
      "cost_usd": 0.0, "cache_hit_rate": 0.51 }
  ],
  "by_agent": [
    { "agent_id": "defamation_extractor", "events": 87, "fail_rate": 0.034 }
  ]
}

GET /api/v1/usage/events?limit=100&offset=0&kind=llm_chat
→ 200 OK { "events": [ UsageEvent, ... ], "total": 4231 }

POST /api/v1/usage/reset
→ 200 OK { "deleted": 4231 }     -- 清空本地 usage_events,需用户在 Settings 显式触发

GET /api/v1/cache/llm  → { "entries": 312, "size_bytes": 4_120_000, "hit_rate_7d": 0.42 }
GET /api/v1/cache/embed → { ... }
GET /api/v1/cache/search → { ... }    -- 替代旧 /api/v1/web_search_cache (deprecated alias 保留 1 release)
GET /api/v1/cache/all  → { ... 聚合 }

DELETE /api/v1/cache/llm   → 200 { "deleted": 312 }
DELETE /api/v1/cache/all   → 200 { "deleted_llm": 312, "deleted_embed": 4120, "deleted_search": 87 }
```

### 5.3 Response headers(自动注入)

任何 `/api/v1/{chat,agent,classify,search}/*` 路由 response 包含:

```
X-Attune-Cache: hit | miss | bypass
X-Attune-Token-In: 1234
X-Attune-Token-Out: 567
X-Attune-Cost-USD: 0.0042       # 缺失 = 未知 model,UI 显示「价格未知」
X-Attune-Latency-Ms: 1820
X-Attune-Provider: cloud_gateway
X-Attune-Model: gemini-1.5-flash
```

由 `attune-server::middleware::usage_headers` 通过 `tower::Layer` 注入,call site 不需手动 set。

### 5.4 CLI commands(可选,v1.2)

```sh
attune usage summary --days 7
attune usage events --agent defamation_extractor --tail 20
attune cache clear --scope llm
attune cache stats
```

---

## 6. 扩展点 / 插件接口

### 6.1 加新 LLM provider

新 provider impl `LlmClient` trait 时,返回类型已强制带 `TokenUsage` → 自动接入 usage 体系,不需改 record_event 代码。新 provider 需要在 `cost::pricing_for_provider` 注册定价表,未注册 → `cost_usd = None`(UI 显示「价格未知」)。

### 6.2 加新 cache backend(future Redis / disk-based)

`CacheBackend` trait 已稳定,新 backend impl 后注册到 `AppState::cache: Arc<dyn CacheBackend>`。Trait 异步,可直接接 Redis async client。**注意 vault 加密语义** — 新 backend 必须支持 `CachedValue::bytes` 加密存储,否则 fail-fast。

### 6.3 加新 routing 策略(下一份 spec)

UsageEvent 是 routing 策略输入数据源。新 router(如 `cost_aware_router`)订阅 `UsageAggregator::recent_events()` stream,根据 last-N 事件统计决定下次走本地/cloud。本 spec 不实现 router,只提供 data API。

### 6.4 第三方 agent 接入

Pro plugin pack 内的 agent subprocess 通过环境变量 `ATTUNE_USAGE_RPC` 接 unix socket / TCP loopback,subprocess 内调 `record_event` 时走 RPC 写入主进程 SQLite。Subprocess crash 不丢已 flush 的 event(WAL mode)。

---

## 7. 错误处理 + 边界 case

### 7.1 错误码 kebab

| HTTP / exit | code | 场景 |
|-------------|------|------|
| 503 | `vault-locked` | usage / cache 都要 DEK,vault 锁状态返回 503 |
| 400 | `cache-scope-invalid` | DELETE /api/v1/cache/xxx 不在 {llm,embed,search,all} |
| 500 | `usage-record-failed` | SQLite 写失败(磁盘满 / WAL 锁)— 主流程不阻塞,但 telemetry log warn |
| 500 | `cache-encryption-failed` | DEK 不可用时 put cache,降级为 bypass |
| 500 | `pricing-unknown` | 内部 telemetry,UI cost 字段为 null,**不阻塞 LLM 调用** |

### 7.2 边界 case

| Case | 行为 |
|------|------|
| TokenUsage.tokens_in == 0 但 response 有内容 | 走 `cost::estimate_tokens(prompt)` 回填,outcome 标 `Retry{attempt: 0}` 区分 |
| Provider 返回 usage 字段缺失 | 用启发式估算填,UsageEvent.error_kind = None(不算 fail) |
| Cache key 冲突(BLAKE3 极低概率) | put 时 unique constraint fail → 视为 miss + warn log,不抛错 |
| 用户开 `log_queries = true` 后又关 | 历史 query_hash 保留,新 event 不再带 query_hash |
| `usage_events` 表 > 100k 行 | `purge_old(retention_days)` worker 每日跑,LRU 删 |
| Vault 锁定中触发 ChatSendBar chip 显示 | UI 拉 `/api/v1/usage/summary` 失败 → 显示「— tok · — $」灰色 placeholder |
| Subprocess agent record_event RPC timeout(>500ms) | subprocess 内 buffer,exit 前 batch flush;若 buffer 满 → 丢弃最旧 |
| L1 LRU 与 L2 SQLite 数据不一致 | 始终先查 L1,miss 再查 L2,L2 命中后回填 L1 |
| Web search cache(legacy)与新 cache.search 数据并存 | migration 时把旧 `web_search_cache` 表 rename 为 `cache_search`,加 `scope` 列,逻辑统一 |

### 7.3 Graceful degradation

- `UsageRecorder` 写失败 → 主流程**永不**失败,降级为 in-memory ring buffer(cap 1000),每 5s 重试 flush
- `CacheBackend` 写失败 → 当次降级为 bypass + log warn,不影响 LLM 调用
- Tokenizer / pricing miss → cost_usd = None,UI 显示「价格未知」灰字

---

## 8. 成本契约

### 8.1 三层成本归属

| 操作 | 层级 | UsageEvent.kind | UI 显示位置 |
|------|------|-----------------|-------------|
| 文件 parse / 分词 / FTS query | 🆓 零成本 | **不进** UsageEvent(避免 SQLite 写放大) | 不显示 |
| Embedding 生成(本地 Ollama bge-m3) | ⚡ 本地算力 | `Embed`, provider=ollama, cost_usd=0 | Usage tab「本地算力」分组 |
| 本地 LLM(qwen2.5:3b via Ollama) | ⚡ 本地算力 | `LlmChat`, provider=ollama, cost_usd=0 | 同上 |
| Cache hit(L1/L2) | 🆓 零成本 | `LlmChat`, cache=Hit, cost_usd=0 | ChatSendBar `cache 67%` |
| 远端 LLM(cloud gateway / BYOK) | 💰 时间金钱 | `LlmChat`, provider=cloud_gateway, cost_usd>0 | ChatSendBar `~1.2K tok · $0.0004` |
| OCR 子进程(PP-OCRv5) | ⚡ 本地算力 | `Ocr`, provider=ppocr, cost_usd=0 | Usage tab「本地算力」 |
| ASR 子进程(whisper.cpp) | ⚡ 本地算力 | `Asr`, provider=whisper_cpp, cost_usd=0 | 同上 |
| K3 推理服务 :8080 | ⚡ 本地算力(用户硬件) | provider=k3_local, cost_usd=0 | 同上 |

### 8.2 UI 显示规则(强制)

- **ChatSendBar 常驻 chip**:`~{tokens_in+tokens_out} tok · ${cost_usd}` 或 `~本地 · {latency}s`(本地推理)
- **Settings → Usage tab**:今日 / 7 天 / 30 天 三个 toggle,显示总 token + 总 cost + cache hit rate + 按 provider/agent 分组 bar chart
- **Vault 顶栏**:左侧「锁定 Vault」按钮旁加小字 `今日 $0.84`(可点跳 Usage tab),0 cost 时不显示
- **Background tasks**:任何 ⚡ 本地算力任务运行中,顶栏队列显示 `Embedding · 47/120 · ⚡ 本地`

### 8.3 用户显式触发原则

- Cache `DELETE /api/v1/cache/*` **必须**用户在 Settings 点按钮触发,**永不**后台 auto-purge(除 retention_days TTL)
- Usage reset(`POST /api/v1/usage/reset`)同样 — UI 二次确认
- Query hash 记录默认 `log_queries = false`(per 隐私优先),用户在 Settings 显式 opt-in

---

## 9. 测试矩阵

### 9.1 6 类下限(per CLAUDE.md §6.1)

| 类型 | 下限 | 路径 |
|------|------|------|
| Golden(unit) | ≥10 fixture | `attune-core/src/usage/tests/golden.rs` — 10 个真实 LLM 调用序列 |
| 属性测试 | ≥3 | `proptest!` 验证 `tokens_in+out ≥ tokens_in` / `cost_usd ≥ 0` / `cache 状态机不可逆转` |
| 边界 | ≥5 | 空 prompt / 1M tokens / cache key 冲突 / vault locked / provider 缺 usage 字段 |
| 错误 | ≥3 | SQLite 满 / DEK 不可用 / Provider 5xx |
| 集成 E2E | ≥1 | `attune-server/tests/usage_endtoend.rs` — subprocess agent → record → query summary |
| 回归 fixture | per bug | 每修一个 LLM call 缺 usage 字段的 bug,加一条 golden 锁定 |

### 9.2 测试场景(per §6.1 黑盒视角)

| 场景 | 验证点 |
|------|--------|
| Happy path | UI Chat 一次 → ChatSendBar chip 显示;Usage tab 增 1 event |
| Cache hit | 同 prompt 二次发 → 第二次 cache=Hit + cost_usd=0 + latency < 50ms |
| Multi-agent | 4 agent 并发跑 → 每 agent 各自 event 不混 + agent_id 标记正确 |
| Vault locked 中 | Settings Usage tab 拉 summary 显示 vault-locked 友好错 |
| 30 天滚动 | mock 时间往前推 31 天 → purge worker 清 |
| Cross-provider | Ollama call + cloud_gateway call → summary by_provider 拆开正确 |
| Cache 加密 | L2 SQLite 文件用 sqlite3 直接查 → 看不到 plaintext response |
| Pricing miss | 配 unknown model → cost_usd=null + UI 显示「价格未知」(不崩) |

### 9.3 性能 baseline

| 指标 | 目标 |
|------|------|
| `record_event` p99 | < 2ms(批量写) |
| L1 cache get p99 | < 50µs |
| L2 cache get p99 | < 5ms(SQLite 加密 decrypt) |
| `GET /usage/summary` p99 | < 100ms(7d range,SQL 索引覆盖) |
| usage_events 表 100k 行 SELECT 聚合 | < 200ms |

---

## 10. 向后兼容

### 10.1 Schema versioning

- 新 table 走 attune-core 现有 migration 系统(`store/migrations/`),版本号 +1
- Vault `meta` 表加 `usage_schema_version` 字段,首次启动检测 → 创建表 + backfill 默认配置

### 10.2 Migration path

| 老资产 | 处理 |
|--------|------|
| `web_search_cache` 表(legacy) | rename → `cache_search` 加 `scope='web'` 列;旧路由 `/api/v1/web_search_cache` 保留 1 release deprecated alias 返回 `Warning: deprecated` header |
| `settings.llm.daily_token_used`(若存在) | 不复用 — 新 UsageView 实时聚合 usage_events,删除旧字段 |
| 各 call site 现有 `usage` 字段散落 | 改造期保留旧字段,新增 `TokenUsage` 平行,1 release 后删旧 |

### 10.3 老客户端行为

- Web UI 老版本(无 UsageView)— response header `X-Attune-*` 仅多余字段,不破坏旧逻辑
- Chrome 扩展(只调 `/api/v1/search/*`)— search response 自动带 cache header,扩展可选读取,不读不影响
- attune-cli 老 binary — `usage` 子命令不存在,旧用户不受影响

---

## 11. 风险登记

### 风险 1 ⚠️ HIGH:call site 改造覆盖不全 → telemetry 失真

**描述**:LLM/Embed/Rerank/OCR/ASR/VLM 6 类 call site 散落在 ~20 个 module(`chat.rs` / `agent_runner.rs` / `classifier.rs` / `skill_evolution/` / `ai_annotator.rs` / `query_rewrite.rs` / `intent_router.rs` / ...)。漏改一处 → UsageEvent 漏记 → 用户看到的 cost 比实际少,失去信任。

**缓解**:
1. `LlmClient::chat` 返回类型 `Result<ChatResponse, _>` 改为 `Result<(ChatResponse, TokenUsage), _>` — **编译期强制**所有 caller 处理 usage
2. impl `Drop` for `UsageRecorderGuard` — 持有 LLM call lifetime,Drop 时若未 `record()` 则 panic in debug + warn in release
3. CI 加 `grep -rn "llm_client.chat\|client.embed" --include="*.rs" | xargs check usage record` 静态扫
4. 灰度策略:v1.1.0 先接入 5 个主路径,v1.1.1 接入其余,验证 1 周后强制

### 风险 2 ⚠️ HIGH:SQLite WAL 锁定 / 写放大 → 主流程延迟

**描述**:每次 LLM call 写 usage_events + cache update,高峰期(多 agent 并发)可能 100+ writes/s。SQLite WAL checkpoint + 加密 vault 双写下,p99 可能 > 50ms 阻塞 LLM 返回。

**缓解**:
1. `UsageRecorder` 内 ring buffer + 异步 batch flush(每 100ms 或满 50 条)— 主流程同步只入内存
2. Cache write 走 background tokio task,LLM response 立即返回(cache miss 路径不等 put 完成)
3. SQLite 配 `synchronous=NORMAL` + `journal_mode=WAL` + `wal_autocheckpoint=1000`
4. Bench:压测 1000 events/s p99 < 10ms 才允许 ship
5. 失败兜底:buffer 满 1000 条丢弃最旧 + warn log + UI 提示「telemetry 过载,部分数据丢失」

### 风险 3 ⚠️ MED:Vendor prompt cache 与本地 cache 语义混淆

**描述**:Anthropic claude / OpenAI 自家有 prompt-cache(命中得 `cached_tokens` 字段折扣 90% 价格)。我们 L1/L2 是本地 hash cache(命中得整段省了)。两者并存时:
- 用户 prompt A 第一次发 → L2 miss + Anthropic cache miss → 100% 价格
- 第二次发 → L2 hit → cost_usd=0,**Anthropic 端没记**
- 第三次清 L2 后再发 → L2 miss + Anthropic cache hit(他们 5min TTL)→ 10% 价格,cached_in=N

容易让用户误以为 cache hit rate 包含了 vendor 端。

**缓解**:
1. UsageEvent.cache 仅指 L1/L2,vendor 端 `cached_in` 字段独立显示
2. UI Usage tab 区分:「Attune cache: 42%」「Vendor prompt cache: 18%」「Net new tokens billed: X%」三行
3. RELEASE.md / DEVELOP.md 明示语义

### 风险 4 ⚠️ MED:Cost-usd 估算误差用户索赔

**描述**:启发式 tokenizer ±15% 误差,UI 显示 $0.40 实际 vendor 账单 $0.46。Pro 用户可能截图来索赔。

**缓解**:
1. ChatSendBar chip 显式 `~` 前缀(`~1.2K tok · ~$0.0004`)
2. Settings → Usage tab footer 灰字「估算 ±15%,以 vendor 账单为准」
3. v1.2 引入 tiktoken feature 后切精确模式
4. Pro 用户走 cloud gateway 时,gateway 返回真实 usage(走 vendor 接口),attune 优先用真实值,启发式仅本地推理 / 估算视图

### 风险 5 ⚠️ LOW:Query hash 隐私担忧

**描述**:即使 BLAKE3 16-hex 前缀(64 bit),用户可能担心「我搜过什么」泄露。

**缓解**:
1. 默认 `log_queries = false`,user opt-in
2. Hash 加 per-vault salt(从 DEK 派生),hash 不跨 vault 可比对
3. Settings 显式说明「记录的是 BLAKE3 哈希片段,不可逆」+ 一键清空按钮

### 风险 6 ⚠️ LOW:K3 一体机 SQLite 性能不足

**描述**:K3 SoC eMMC IOPS 较弱,usage_events 高频写可能拖慢主流程。

**缓解**:
1. K3 镜像配置 `usage.batch_flush_ms = 500`(笔电 100ms)
2. K3 retention_days 默认 7 天(笔电 30 天),控制表大小
3. K3 cache L2 默认关闭(只用 L1 in-memory),用户在 Settings 可开

---

## Appendix A:Spec 评审 checklist

- [ ] 11 节全部有实质内容,无 stub
- [ ] 引用真实文件 path(crate / module 名经 grep 验证)
- [ ] DB schema SQL 可在 attune vault 直接 apply
- [ ] API 契约 endpoints 与 routes/mod.rs 注册模式一致
- [ ] 测试矩阵 6 类下限齐(golden / proptest / boundary / error / e2e / regression)
- [ ] 风险登记 ≥ 3 个具体风险(本 spec 列 6 个)+ 缓解
- [ ] Spec 完成不动代码 — 仅 .md 落档

## Appendix B:实施时机

- **v1.0.x 不实施** — v1.0 GA Roadmap 已排满(per 项目 CLAUDE.md ⭐ 节)
- **目标 v1.1.0**(8/15 后)— 与 VLM provider + defamation v3 同 sprint
- **依赖**:无外部 blocker;本地 Rust trait 改造 + DB migration 自足
- **预估**:中型 feature ≈ 3-5 工作日,11 节 plan 评审过后开 worktree

---

**Draft 完成。等待用户评审 → invoke `superpowers:writing-plans` 出 implementation plan。**

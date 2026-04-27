# W3 Batch A 设计稿（C1 + F1 + F2 + F4）

**日期**：2026-04-27
**对应路线图**：12-week 战略 v4 Phase 1 W3 F-P0c
**依赖**：W2 batch 1 (commit `f40b811`)、H1/A1 commits
**被依赖**：J6 W4 benchmark（citation breadcrumb 在 golden set 显示）、W3 batch B G1/G5 Chrome 扩展、W3 batch C K2 parse golden set

[English](2026-04-27-w3-batch-a-design.md) · [简体中文](2026-04-27-w3-batch-a-design.zh.md)

---

## 1. 为什么这一批

W2 batch 1 留了 **B1 backend 占位字段**（`Citation.breadcrumb=Vec::new()`、`chunk_offset_*=None`）— 所有消费方（server routes、未来前端、J6 benchmark 输出）当前都拿到空值。关闭占位是 W3 最高优先任务；否则下游代码累积 "always-empty" 兜底逻辑，变成永久债务。

W3 batch A 打包 **4 个纯后端项**，不依赖 Chrome 扩展或 Tauri 前端：

| 项 | 类型 | 估算 | 来源 |
|----|------|------|------|
| **F1** Web fallback 可观测性日志 | 小修 | 1 行 | reviewer P1 #1 |
| **F4** RELEASE.md known-limitation 注解 | 文档 | 5 分钟 | reviewer P1 |
| **C1** Web search 本地缓存 | 新功能 P0 | 中等 | 吴师兄 §6 + Readwise 模式 |
| **F2** Breadcrumb 透传 indexer→Citation | bridge | 中等 | reviewer F2（关闭 W2 占位） |

W3 batch B（G1/G5 Chrome 扩展）+ batch C（K2 parse golden set）独立处理。

## 2. F1 + F4 — 小修

### F1: chat.rs:128 加可观测性日志
```rust
log::info!(
    "J5: secondary retrieval result — local_was_empty={}, broader_count={}",
    knowledge.is_empty(), broader.len()
);
```
让 ops 在生产日志中区分"fallback 召回更多文档"vs"fallback 同样空"。

### F4: rust/RELEASE.md
更新 W2 batch 1 条目：把 "Known limitation: ... populated in W3 batch 2" 改为 **"RESOLVED in W3 batch A (commit `<sha>`)"** 引用 F2。

## 3. C1 — Web search 本地缓存

### 为什么
per 吴师兄 §6 "高频 query 缓存" + attune 成本契约 §2（web search 是付费网络调用）。缓存层把重复 query 变成 🆓 查找。

### 设计

**新表**（`store/mod.rs` SCHEMA_SQL）：
```sql
-- C1 Web search cache (W3 batch A, 2026-04-27)
-- 加密存储 web_search 结果。query_hash = SHA-256(query) 作为查找键；
-- query_text + results 字段 DEK 加密。30 天 TTL，过期由查询时过滤（不主动 GC）。
CREATE TABLE IF NOT EXISTS web_search_cache (
    query_hash       TEXT PRIMARY KEY,
    query_text_enc   BLOB NOT NULL,
    results_json_enc BLOB NOT NULL,  -- AES-GCM(serde_json::to_string(&Vec<WebSearchResult>))
    created_at_secs  INTEGER NOT NULL,
    ttl_secs         INTEGER NOT NULL DEFAULT 2592000  -- 30 天
);
CREATE INDEX IF NOT EXISTS idx_web_cache_created ON web_search_cache(created_at_secs);
```

**新模块**：`rust/crates/attune-core/src/web_search_cache.rs`（实际是 store/web_search_cache.rs，与 chunk_summaries 同级）

API：`get_web_search_cached` / `put_web_search_cached` / `web_search_cache_count` / `clear_web_search_cache`

### 集成点
chat.rs web search fallback 在 fetch 前查 cache miss 才调网络；fetch 后立即 put。

### 测试
- miss → put → hit（确定性）
- TTL 过期 → 返回 None
- 不同 query 不冲突
- 加密落盘（raw blob ≠ JSON 文本）
- clear_web_search_cache 返回删除数

## 4. F2 — Breadcrumb 透传 indexer → Citation

### 为什么关键
W2 batch 1 加的字段是**API 形状**而非真值。不闭环 → 所有消费方做 `if breadcrumb.is_empty() { fallback }`，永久债务。

### 设计选择：最小入侵的辅助表

**避免**：扩 `embed_queue` schema + `VectorMeta` serde 字段。会强制迁移 4 个 enqueue 调用点 + 破坏老 vault `.encbin` 向后兼容。

**采用**：独立辅助表 `chunk_breadcrumbs`，按 `(item_id, chunk_idx)` 主键。indexer pipeline 写入；ChatEngine 查询时 join。老 vault 无此表 → 升级时 IF NOT EXISTS 创建空 → 查不到时返回空 Vec 优雅降级。

```sql
CREATE TABLE IF NOT EXISTS chunk_breadcrumbs (
    item_id          TEXT NOT NULL,
    chunk_idx        INTEGER NOT NULL,
    breadcrumb_json  TEXT NOT NULL,         -- JSON Vec<String>，明文
    offset_start     INTEGER NOT NULL,
    offset_end       INTEGER NOT NULL,
    PRIMARY KEY (item_id, chunk_idx)
);
CREATE INDEX IF NOT EXISTS idx_chunk_breadcrumbs_item ON chunk_breadcrumbs(item_id);
```

**关于加密**：breadcrumb 是文档结构标题（如"公司手册 > 第三章"）— 来自结构非秘密内容。决策：明文存储便查询。未来若用户有顶密标题层级，F2 v2 可用 chunk_summaries 同款加密。

### Indexer pipeline 改动

4 个调用点当前用 `chunker::extract_sections`。策略：**不改这 4 处**。在 section 创建时（调用方已有原文），增加一行写入 chunk_breadcrumbs：

```rust
let _ = store.upsert_chunk_breadcrumbs_from_content(&item_id, &content);
```

辅助函数 `upsert_chunk_breadcrumbs_from_content` 内部跑 `extract_sections_with_path` + 算 offset + 批量 INSERT OR REPLACE。

### SearchResult + ChatEngine 集成

`SearchResult` 在 item 解密后增加 lookup：
```rust
let breadcrumb_data = ctx.store.get_chunk_breadcrumb(&item.id, 0).ok().flatten();
// 填入 SearchResult.breadcrumb / chunk_offset_*
```

**Caveat**：当前 SearchResult 只追踪 item_id 不追踪具体 chunk_idx 命中。F2 v1 用**第一个 chunk** 的 breadcrumb 作为启发式 — top-level 导航足够；W5+ indexer 追踪 per-chunk 命中后精细化。

ChatEngine.chat() 把 `SearchResult.breadcrumb` → `Citation.breadcrumb`，offsets 同理。

### 测试
- `upsert_chunk_breadcrumbs_from_content` 对嵌套 markdown 写正确行
- lookup 返回 `(Vec, start, end)`
- 未知返回 None
- ChatEngine 集成：chat() 对带结构化标题的 item → Citation.breadcrumb 非空 + offsets 有值

## 5. 致谢

per `ACKNOWLEDGMENTS.md` 政策：
- **C1 cache**：吴师兄 §6 "高频 query 做缓存" + 通用 Readwise/Linkwarden "fetch 时快照" 模式
- **F2 辅助表方法**：attune 内部设计（避免 embed_queue 迁移风险）；灵感来自既有 chunk_summaries 表的"内容键 sidecar 存储"成功模式
- **F1 日志理念**：标准 SRE 可观测性（无特定外部源）

## 6. 不做（明示）

- ❌ J5 二次检索 E2E 测试（F3）— 需完整 ChatEngine 构造 + seeded vault，留 batch B
- ❌ K2 Parse Golden Set 200 篇 — 需语料采集 + CI 流水线，留 batch C
- ❌ G1 / G5 Chrome 扩展 — 全栈浏览器工作，独立会话
- ❌ Cache GC 守护 — F2 v1 读时惰性过期；W5+ 加 H7 后台 GC 如需
- ❌ Per-chunk SearchResult 精度（哪个 chunk 命中）— 启发式"第一个 chunk"够用

## 7. 验收清单

- [ ] F1: chat.rs:128 区域有新日志
- [ ] F4: rust/RELEASE.md W2 batch 1 改 RESOLVED
- [ ] C1: `cargo test -p attune-core --lib web_search_cache` 全绿
- [ ] C1: chat.rs web fallback 用缓存（测试覆盖 miss + hit）
- [ ] F2: `cargo test -p attune-core --lib chunk_breadcrumbs` 全绿
- [ ] F2: 集成测试：ChatEngine.chat() 对结构化文档返回 Citation 含 breadcrumb
- [ ] 全 lib 回归：415+11 → 415+N+11+M，0 失败
- [ ] R1 + R2 code review pass
- [ ] ACKNOWLEDGMENTS.md 更新 C1（吴师兄 + Readwise 引用）
- [ ] git commit + push develop

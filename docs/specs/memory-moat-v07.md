# Memory Moat v0.7 — 安全有效的记忆护城河

**Status**: Spec · Phase A+B 已落地 + 30 轮 review sprint 完成 · Phase C 待开 (v0.7 后续 sprint)
**Owner**: attune-core 维护者
**Last updated**: 2026-05-15
**Trigger**: 2026-05-15 用户对话「优势不在于模型，而在于以安全有效的记忆 / 有效的记忆能在相同模型下实现更好的效果」

---

## 1. 北极星

> **同样的 LLM，挂上 attune 比单跑模型答得更准、更敢用 — 因为记忆是私有的、可审计的、会随用户的真实使用持续变好的。**

不是堆模型参数，不是冲 benchmark；是把 **"用户头脑里那本未整理的笔记"** 变成 LLM 可直接消费的、隐私可控的、自适应改进的工作记忆。

---

## 2. 三档可验证记忆质量

| 档位 | 目标 | 验证手段 |
|------|------|----------|
| **可信** | 没有 stale embedding / orphan 向量 / 假命中 | Phase A 已修 3 个 release-blocker；reindex worker 持续清理；MANUAL_TEST_CHECKLIST.md 新增"编辑后搜索 → 必须返回新内容"用例 |
| **可学** | 用户行为反向调权 — 标重点 / 引用 / 编辑都进入信号回路 | Phase B 已接入 5 类信号 (doc_create/update/delete + citation_hit + annotation_marker)；skill_evolution 周期消费 |
| **可证** | 隐私决策 + 信号去向都可导出审计 | audit log + skill_signals 表 + reindex_queue 流水都可 CSV 导出 (`/api/v1/audit/log.csv`) |

---

## 3. 已落地 (v0.7 sprint 1 = commit 71d82ee 之后)

### Phase A — 文档编辑嵌入功能完全有效（修 3 个 release blocker）

| Bug | 修法 | 影响 |
|-----|------|------|
| `routes/items.rs::update_item` 完全不 re-embed | UpdateOutcome 三态 + reindex::reindex_item 完整 pipeline | UI 编辑后 search 立刻反映新内容 |
| `routes/upload.rs` 同名重传不去重 | content_hash 短路 dedup | 用户拖错文件不浪费 embedding |
| `routes/items.rs::delete_item` 不清向量/FTS | reindex::purge_item_indexes 调 `vectors::delete_by_item_id` + `fulltext::delete_document` (此前 0 处调用) | 删除后无 orphan 向量假命中 |

**架构新增**：
- `attune-core::store::items::compute_content_hash` + `find_item_by_content_hash` + `purge_embed_queue_for_item`
- `attune-core::reindex` 模块 — 协调 store + vectors + fulltext + queue 三资源事务式清理
- `items.content_hash` 列 + migration（SHA-256 hex）
- `reindex_queue` 表 — scanner / scanner_webdav 等无法持锁的 worker 写信号，server 端 `start_reindex_worker` 周期消费
- `AppState::start_reindex_worker` 后台 3s 轮询 worker，vault unlock 时启动

### Phase B — 自学习闭环 3 hook

| Hook | 信号 kind | 写入位点 |
|------|-----------|----------|
| 1 | `doc_create` / `doc_update` / `doc_delete` | upload.rs / items.rs (update + delete) / scanner.rs |
| 2 | `citation_hit` | chat.rs (取 top-5 引用 chunk 喂入) |
| 3 | `annotation_marker` | annotations.rs::create_annotation (ref_id=item_id, query=label) |

**架构新增**：
- `skill_signals` 表加 `kind` + `ref_id` 列 + migration
- `Store::record_signal_event(kind, ref_id, query_opt)` API
- `Store::count_unprocessed_signals_by_kind(kind)` 让 skill_evolution 按 kind 设阈值

---

## 4. 待开（v0.7 sprint 2+）

按 RICE 排序。

### C1 — 文档版本化记忆 (Reach×Impact×Confidence/Effort = 高)

业内对照：除 Rewind 外**所有竞品都用「最新版覆盖」**。attune 保留 v1/v2 让 RAG 引用 diff，是差异化。

**实现**：
```sql
CREATE TABLE item_versions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id TEXT NOT NULL REFERENCES items(id),
    content_hash TEXT NOT NULL,
    content_enc BLOB NOT NULL,           -- AES-256-GCM 加密快照
    snapshot_at TEXT NOT NULL,
    diff_summary TEXT                    -- LLM 生成 1 句话差异（按需懒计算）
);
CREATE INDEX idx_versions_item ON item_versions(item_id, snapshot_at DESC);
```

**触发**：每次 `update_item` 且 `content_changed=true` 时 INSERT 一行（snapshot 旧版本）；保留策略 = 最近 30 天 OR 最近 10 版（择其多）。

**用户价值**：
- 「我两周前对 X 项目的想法」（time-travel RAG）
- diff 引用：「之前你说 A，后来改成 B，让我把最新版作为答案依据」

**Effort**: 1.5 天（schema + UpdateOutcome 写入 + 一个 `/api/v1/items/{id}/versions` 路由）

### C2 — 编辑触发自动重标注 (Reach×Impact 中)

`update_item` 触发 reindex 时同步 enqueue `ai_annotator::re_annotate(item_id)`，让 AI 标注的 ⭐ 重点 / 🤔 存疑 标签随内容更新。

**Effort**: 0.5 天（reindex 路径加 enqueue 一行 + ai_annotator 接 dirty marker）

### C3 — 失败信号到 project_recommender 双向反馈 (Impact 中)

`skill_signals.kind='search_miss'` 累积 5 条同 topic → `project_recommender::suggest_new_project()`：用户经常搜某主题但还没建对应 Project，自动建议。

**Effort**: 1 天（信号聚类 + cosine 阈值）

### C4 — 知识衰减曲线（Ebbinghaus）(Reach 中 长尾)

`access_timeline (item_id, accessed_at, action)` 表 → search 排序加入 `exp(-Δt/τ)` 权重 → "你 14 天没看过 X 该复习了" Reader 旁提示。

**Effort**: 2 天（表 + 索引修改 + UI 提示位）

### C6 — skill_signals.query 字段加密 (R2 F3 隐私 P1)

R2 滚动 review 实测发现：vault 加密策略对 items.content / tags / annotations.content 走 AES-256-GCM，但 **`skill_signals.query` 明文 TEXT 落盘**。chat citation_hit / search_miss 写入路径均把用户原文（截断 512 字）明文存。

**威胁模型**：律师/医生敏感行业 user 在 chat 问"某甲乙丙离婚案策略" → query 落明文 → 攻击者拿到 SQLite 文件（NAS 模式 / 备份 / 磁盘镜像）一律可读 — 违反 Vault 加密承诺。

**实现**：
```sql
ALTER TABLE skill_signals ADD COLUMN query_enc BLOB;
-- query TEXT 列 deprecate 保留向后兼容，新写入只填 query_enc
```
- `Store::record_signal_event` 签名加 `dek: &Key32`
- `Store::record_skill_signal` 同步加 dek
- get_unprocessed_signals 读取时 dek decrypt
- migration：老 vault 升级时 query TEXT 不迁移（保留），新信号入 query_enc

**Effort**: 1 天（schema + record_*_event 5 处 caller 改 + migration + evolver 解密）。

### C7 — embed_queue generation 标记（R10 S3 update 竞态根治）

R10 滚动 review 发现 S3 竞态：embed worker 异步处理 embed_queue，与 reindex 并发时：
- **delete 场景**：item 软删后 embed worker 仍写旧 chunk 向量 → orphan。**已修**（`embed_and_index_batch` 加 `item_exists` 检查，commit R10）。
- **update 场景**：item 被 PATCH → reindex 清旧 chunk 入新 chunk，但 embed worker 已 dequeue 的旧 chunk 任务（标 processing）继续写**旧内容**向量。`item_exists` 救不了（item 还在）。

**根治方案**：embed_queue 加 `generation INTEGER`，items 加 `index_generation INTEGER`。
- reindex_item 时 `items.index_generation += 1`
- enqueue_embedding 写入当前 generation
- embed worker 写向量前比对 task.generation == items.index_generation，不一致 → 丢弃（旧世代 chunk）

**Effort**: 0.5 天（2 列 + enqueue 签名 + worker 比对）。当前 update 竞态窗口小（embedding 通常快于用户连续编辑），优先级 P2。

### C5 — embed_model_version 迁移工具链 (基建)

`vectors.VectorMeta` + `embed_queue` 加 `embed_model_version` 字段；升级 bge-m3 → bge-m3-zh-large 时跑 `attune-cli reindex --model=new` 全量迁移。

**Effort**: 1 天（schema + CLI 子命令 + worker 跨版本 dispatch）

---

## 5. 不做的事

| 想法 | 为什么不做 |
|------|-----------|
| 全局"信任分" / "记忆质量分"展示 | 用户对单一数字无感，反而要解释口径；用三档（可信/可学/可证）更直观 |
| 联邦学习 / 跨用户记忆共享 | 与 L0 隐私底线冲突，不在 OSS attune 范围；attune-pro 行业版才考虑（同律所内共享） |
| LLM Fine-tune 个性化 | 模型不是护城河，记忆才是；微调成本 + 数据 leak 风险都不值 |
| 自动给所有 chunk 生成 metadata 标签 | 太烧 token；ai_annotator 已有按需懒计算路径 |

---

## 6. 与 attune-pro / lawcontrol 边界

- **attune (本仓 OSS)** — 上述 C1/C2/C4/C5 通用基建
- **attune-pro / law-pro** — 律师专属版本化策略（案件卷宗时间线 + 截止日期回滚）
- **lawcontrol** — 律所 B2B 团队共享记忆（B2B SaaS，不在 attune 仓）

技术上独立，可参考设计模式但代码不互联（per CLAUDE.md「三产品矩阵 + 边界」§硬约束）。

---

## 6.4. R1 perf bench 实测（2026-05-15 11:09, release build）

跑 `crates/attune-core/tests/perf_reindex_bench.rs --ignored --nocapture`：

| 文档 | insert_ms | reindex_ms | chunks | purge_ms |
|------|-----------|------------|--------|----------|
| 1 KB | 0.92 | 182.51 | 10 | 7.00 |
| 10 KB | 0.86 | 114.73 | 74 | 7.78 |
| 50 KB | 2.03 | 425.90 | 360 | 8.23 |
| **100 KB** | 1.32 | **834.36** | 714 | 7.76 |
| 500 KB | 3.28 | **1949.39** | 3554 | — |

`update_item` 短路微调测：1.29 / 1.47 / 1.14 ms（content changed / same content / title only）— 短路在 50KB 文档上仅省 1-2ms（不是之前承诺的"~3s/100KB"，那个估算 wrong）。

### 实测结论 → 改 P0 项

- **100KB 文档 reindex 持 vault lock 834ms** 是真 P0（先前 review agent 静态估算 20-80ms 偏离 10 倍）。500KB 1.95s 不可接受。
- 根因：`reindex_item` 内 `N × store.enqueue_embedding` 串行执行（714 个 SQL INSERT × ~1ms），SQLite WAL 下 batch 化可降 5-10 倍。
- purge 持锁 ~7-8ms 与文档大小无关 ✓（HNSW remove 是按 meta 表线性扫描全索引，size 影响小）
- content_hash 短路实际收益 < 5% — 因为 update_item 本身廉价（BLOB 加密 1-2ms 级），短路省不出明显时间；真正价值在于跳过下游 reindex_item 的 800ms+

→ v0.7 sprint 2 加 enqueue_embedding batch API（`enqueue_embedding_batch(Vec<(...)>)`），单 SQL 事务一次 INSERT N 行。

## 6.5. 30 轮 review sprint 修复（2026-05-15）

5 个并行 code-review agent + 自审 30 轮，识别并修复：

| Sev | 编号 | 问题 | Fix commit |
|-----|------|------|-----------|
| **Critical** | R21 S3-1 | scanner_webdav update 完全缺 cleanup → orphan 永久残留 | f6798b9 |
| **P0** | R6 #1 | update_item title-only 不刷 updated_at | 26b1636 |
| **P0** | R6 #3 | reindex_queue 无 attempts 计数 → 毒任务卡队头 | 26b1636 |
| **P0** | R6 #6 | 'reindex' action schema 文档化但 worker 未实现 | 26b1636 |
| **P0** | R17 S4-Q1 | evolver 拉混合 kind batch 污染 LLM prompt | f6798b9 |
| **P1** | R6 #4/#5 | record_signal_event / enqueue_reindex 无 kind/action validation | 26b1636 |
| **P1** | R6 #7 | mark_signals_processed 非事务 | 26b1636 |
| **P1** | R9 #2 | delete_item 缺 dek_db 校验（vault locked 时可绕过） | 26b1636 |
| **P1** | R9 #3 | chat citation_hit 5 次 INSERT 串行 + query 字段冗余 | 26b1636 |
| **P1** | R16 | migrate_items_content_hash INDEX CREATE 嵌在 if 块内 | 26b1636 |
| **P1** | R17 S1-Q4 | update_item 三条 SQL 无事务 → 并发 PATCH race | f6798b9 |
| **P1** | R17 S2-Q1+S3-Q2 | reindex_worker 错误分类 Transient vs Task | f6798b9 |
| **P1** | R21 S2-1 | annotation update/delete 完全无信号 | f6798b9 |

留作 v0.7 后续 sprint（P2）：
- delete 后历史 chat citation 404 → UI 占位符
- citation_hit 同 item_id 1h 内 dedup
- reindex worker 单 task 超时保护
- attempts 达上限任务的运维 reset endpoint
- citation_hit session_id 关联（v0.8 dedicated consumer）

## 6.6. 9 轮滚动 review 修复（2026-05-15，R1-R9）

30 轮 sprint 后追加 9 轮滚动深度审计，每轮聚焦一个维度：

| 轮 | 维度 | 关键发现 + 修复 |
|----|------|----------------|
| R1 | reindex 性能实测 | 100KB reindex=834ms / 500KB=1.95s（agent 静态估算偏 10×）；加 perf_reindex_bench.rs；根因 N×enqueue 串行 → sprint 2 batch API |
| R2 | SQL/auth/输入校验 | P0 /chat/stream 无长度校验 OOM；P1 PATCH /items 无 body 上限；P1 skill_signals.query 明文落盘（→ §C6） |
| R3 | error 泄露/日志 | P1 chat 日志打 query 明文 → 改 debug；P2 8 处 signal 静默 → 加 debug 留痕 |
| R4 | 测试覆盖盲点 | 生产路径无用户可触发 panic ✓；补 7 integration test（vectors_deleted 精确 / 空 section / 白名单全 / ref_id 边界 / 组合分支） |
| R5 | 资源管理 | **P1 worker panic → flag 永久 true 致 worker 僵死** → WorkerFlagGuard RAII |
| R6 | 老 vault 升级 | 3 migration 幂等安全；补 2 migration 测试 |
| R7 | API 向后兼容 | upload dedup 分支 response shape 不一致 → 字段对齐 id；breaking change 仅 update_item，12 caller 全适配 |
| R8 | 文档死链 | README/TESTING 死链修复；README Documentation 节加 v0.7 spec 链接 |
| R9 | release readiness | clippy v0.7 文件 doc 缩进修；workspace 933 tests 全过 |

**滚动 review 累计修**：2 P0 + 3 P1（+ R1-R30 sprint 的 1 Critical + 5 P0 + 9 P1）。

## 7. 验证

每次 v0.7 dot release 必须新增到 `tests/MANUAL_TEST_CHECKLIST.md`：

```
- [ ] 上传 docA → 搜 keyword → 返回 docA
- [ ] /items/{id} PATCH content → 搜旧 keyword 返回 0 / 搜新 keyword 返回 docA   ← Phase A 必通过
- [ ] 上传 docA 第二次同内容 → 返回 status=duplicate, item_id=同一个     ← Phase A 必通过
- [ ] DELETE /items/{id} → 搜 keyword 返回 0（不假命中已删 doc）            ← Phase A 必通过
- [ ] 加 ⭐ 重点 批注 → /api/v1/audit/log 看到 annotation_marker 信号        ← Phase B 必通过
- [ ] chat 引用 docA → /api/v1/audit/log 看到 citation_hit ref_id=docA       ← Phase B 必通过
```

---

## 8. 一句话

**Phase A 让记忆可信、Phase B 让记忆可学、Phase C 让记忆可证。** 这是 attune 在「相同模型下实现更好效果」的具体兑现路径。

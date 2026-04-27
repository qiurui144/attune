# Memory Consolidation (A1) MVP 设计稿

**日期**：2026-04-27
**对应路线图**：12 周战略 v2 Phase 1 W1 F-P0a
**依赖**：H1 resource_governor（`TaskKind::MemoryConsolidation` 已定义）
**被依赖**：未来 A2 conflict detection、B2 project-aware chat

[English](2026-04-27-memory-consolidation-design.md) · [简体中文](2026-04-27-memory-consolidation-design.zh.md)

---

## 1. 为什么做

attune 的"自进化记忆"定位（mem0 参考叙事）需要 chunk 之上的一层：把用户在某段时间窗口接触/学到的内容**周期总结**成 *episodic* memory（情景记忆）。没有这层，chat 检索只能见 chunk 级碎片；有了之后，"我上周学了什么"成为单次检索就能命中。

这是 **MVP 范围** — 只实现数据模型 + 工作机制。语义记忆（按主题聚合）、冲突检测（A2）、chat 检索集成都明确推迟到 W5+。

## 2. MVP 范围

| W1 做 | 推迟到 W5+ |
|-------|-----------|
| 情景记忆：按时间窗口聚合（默认 1 天） | 语义记忆：跨时间按主题聚合 |
| 数据源：`chunk_summaries` 表（已有 150 字摘要） | 数据源：raw chunks（语义需要更多文本） |
| 6 小时 worker 周期，每 bundle 1 次 LLM | 每个 chunk 插入即触发 |
| 幂等：相同 chunk_hash 集合 → 同一 memory（不重复） | 层级 memory（memory of memories） |
| 三阶段锁释放（与 skill_evolution 一致） | 实时 conflict detection（A2） |
| H1 governor + LLM 配额集成 | chat 检索能浮现 memory（B2） |

## 3. Schema

```sql
-- 加密的"周期总结"记忆。源 chunk_hash 集合作为幂等键。
CREATE TABLE IF NOT EXISTS memories (
    id                    TEXT PRIMARY KEY,
    kind                  TEXT NOT NULL CHECK(kind IN ('episodic')),  -- W5+ 加 'semantic'
    window_start          INTEGER NOT NULL,  -- unix epoch 秒
    window_end            INTEGER NOT NULL,
    source_chunk_hashes   TEXT NOT NULL,     -- chunk_hash 数组 JSON，升序
    source_chunk_count    INTEGER NOT NULL,  -- == len(source_chunk_hashes)
    summary_encrypted     BLOB NOT NULL,     -- AES-GCM(摘要正文)，DEK 加密
    model                 TEXT NOT NULL,     -- 生成所用 LLM 型号（追溯用）
    created_at            INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_memories_window ON memories(window_start, window_end);
CREATE INDEX IF NOT EXISTS idx_memories_created ON memories(created_at DESC);
-- 幂等键：同一组 chunk 不重复
CREATE UNIQUE INDEX IF NOT EXISTS uq_memories_source ON memories(kind, source_chunk_hashes);
```

**无需 backfill 迁移** — 新表，纯 additive。已有 vault 下次打开时自动创建，零数据丢失风险。

## 4. Consolidator API

```rust
// rust/crates/attune-core/src/memory_consolidation.rs

pub const DEFAULT_WINDOW_SECS: u64 = 24 * 3600;       // 1 天窗口
pub const MIN_CHUNKS_PER_BUNDLE: usize = 5;           // < 5 chunks 跳过（信号太薄）
pub const MAX_CHUNKS_PER_BUNDLE: usize = 50;          // LLM prompt 预算
pub const MAX_BUNDLES_PER_CYCLE: usize = 4;           // 防止单周期 24 次 LLM 风暴

/// 单个时间窗口内待合并的 chunk 集合。
pub struct ConsolidationBundle { /* ... */ }

/// Phase 1（持 vault 锁）：扫 chunk_summaries 找未合并的窗口；按天分桶。
pub fn prepare_consolidation_cycle(...) -> Result<Option<Vec<ConsolidationBundle>>>;

/// Phase 2（无锁）：每 bundle 一次 LLM 调用。
pub fn generate_episodic_memories(...) -> Vec<Option<String>>;

/// Phase 3（持 vault 锁）：幂等 INSERT。返回新增条数。
pub fn apply_consolidation_result(...) -> Result<usize>;

/// 测试便利：单周期完整跑。
pub fn run_consolidation_cycle(...) -> Result<usize>;
```

## 5. Worker（attune-server）

`start_memory_consolidator(state)`：
- 6 小时周期
- 注册 `TaskKind::MemoryConsolidation` governor
- 三阶段锁释放（与 skill_evolver 同构）
- LLM 配额检查（H1 `allow_llm_call`）

**配额说明**：MVP 在每周期开始 reserve 1 个 slot，但 Phase 2 可能调 N 次 LLM（每 bundle 1 次，最多 4）。这是 best-effort，按周期计配额；按调用计的精确 quota 推到 W5+。

## 6. 幂等性

唯一索引 `uq_memories_source(kind, source_chunk_hashes)` 是硬保证。算法：chunk_hashes 升序排序 → JSON 编码作为 canonical key → `INSERT OR IGNORE`。重跑相同 bundle 返回 0 新增，永不重复。

避开了"标记 chunk 已 consolidated" 的复杂方案（需要二级表 + 部分失败 race condition）。

## 7. 测试

**Unit**：
- 空 store → prepare 返回 None
- 按天边界分桶（t1, t2 跨天 → 2 bundles）
- 跳过 < MIN_CHUNKS_PER_BUNDLE 的窗口
- 不超 MAX_BUNDLES_PER_CYCLE
- generate 用 MockLlm 验证
- apply 幂等（同 bundle 二次 → 1 行）
- apply 跳过 None summary

**Integration**：
- 真实 Store + tempfile + MockLlm 完整跑一周期
- 验证 memories 表填充正确

## 8. MVP 不做的事（明示）

- ❌ 语义记忆（跨时间主题聚合）
- ❌ chat 检索浮现 memory（B2 in W5）
- ❌ memory 之间冲突检测（A2 in W5）
- ❌ 按 LLM 调用计配额（1/周期对 6h 周期足够）
- ❌ 用户面 memories UI（F1 Profile 在 W4 顺带）
- ❌ memory 导出导入（B5 在 W11）
- ❌ 自适应窗口大小（固定 1 天）

## 9. W1 验收清单

- [ ] `memories` 表在新 vault 打开时创建
- [ ] `cargo test -p attune-core memory_consolidation::` 全绿
- [ ] `cargo test --test memory_consolidation_integration` 全绿
- [ ] `attune-server` 引用 `start_memory_consolidator` 编译通过
- [ ] 手动：MockLlm 跑一周期 vault（10 chunks 跨 2 天）→ 2 memories；重跑 → 0 新
- [ ] `rust/RELEASE.md` + `rust/DEVELOP.md` 入口更新
- [ ] git commit + push develop，报告 SHA

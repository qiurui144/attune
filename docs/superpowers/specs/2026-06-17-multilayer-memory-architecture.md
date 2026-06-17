# 多层记忆架构(5+ 层)— 设计规范 (DRAFT)

> 状态:**DRAFT,待用户评审**(§3.1 spec-first;评审通过 → writing-plans → impl)
> 日期:2026-06-17 · 作者:Claude (Opus 4.8) · 关联:[[memory-continuity-and-portability]] (`docs/superpowers/specs/2026-06-15-memory-continuity-and-portability.md`)
> 现状基线:已 ship **L0–L3**(`rust/crates/attune-core/src/memory/`:assembler / retrieval / semantic / consolidation_agent / portability / migration)

---

## 1. 目标定位

**用户痛点**:attune 作为"私有 AI 知识伙伴",其记忆当前止于单 vault 内的 L0–L3。用户提出三诉求:
1. **短期记忆 vs 长期记忆**要分明——当前对话/近期交互(短期、易失、省 token)与沉淀知识(长期、稳定)未显式分层。
2. **跨项目记忆**——同一用户在多个 Project / vault 间的可迁移知识("我是谁/我怎么工作/我已知什么")当前不流动,每个 vault 是孤岛。
3. **做成 5+ 层**——把上述统一进一个清晰的分层记忆体系,各层有明确职责、晋升/衰减规则、成本归属。

**与产品定位对齐**:记忆护城河是 attune 的核心差异点(v0.7 已立"记忆护城河" sprint)。本设计在不破坏**成本契约**(建库不偷跑 LLM)与**隐私优先**(跨项目默认不流动)的前提下,把记忆从"单 vault 4 层"扩成"用户级 6 层"。

---

## 2. 范围边界

### 做什么(本 spec)
- 形式化 **L-W 工作/会话记忆**(短期、易失)与 **L-S 短期滚动记忆**(衰减窗口)两个轻量层,替代当前隐式的 `assembler::compact_history`。
- 新增 **L4 跨项目记忆**(cross-vault portable knowledge):基于已有 `portability.rs` 的导出/导入,升级为**可选的、显式同意的**跨 vault 知识层。
- 新增 **L5 画像/程序性记忆**(profile / procedural):标准化"用户是谁 / 偏好 / 重复工作流"的长期巩固层。
- 定义全 6 层(L-W/L-S 计为短期组)的**晋升 / 衰减 / 巩固 / 遗忘(TTL)**规则与**检索路由**(多层 RRF 融合)。
- 各层**成本归属**(§8)+ **测试矩阵**(§9)+ **向后兼容迁移**(§10)。

### 不做(本 spec 明确排除)
- ❌ 不改 L0/L1/L2/L3 既有实现语义(只在其上加层 + 加路由),除迁移所需。
- ❌ 跨项目记忆**不做后台自动云同步**(§产品决策:跨产品互通走用户主动 export/import;L4 默认本地多 vault,云同步推后 vX)。
- ❌ 不引入"AI 主动猜你需要"行为(违成本契约;建议另见 `2026-06-17-suggestions-and-thirdparty-accounts.md`)。
- ❌ L5 程序性记忆的**自动执行**(只记录工作流模式,不自动跑)推后 vNext。

---

## 3. 架构数据流

```
                      ┌─────────────────────────────────────────────────┐
   用户交互 / 文档摄入  │                                                 │
        │             │   检索路由 (assembler::assemble_context)         │
        ▼             │   多层 RRF 融合 + 覆盖门 + token 预算            │
  ┌───────────┐       │                                                 │
  │ L-W 工作  │◀──────┤  会话内: 当前对话 turns (易失, in-mem, token 预算)│
  │ (session) │       └─────────────────────────────────────────────────┘
  └─────┬─────┘            ▲          ▲          ▲          ▲        ▲
   衰减/滚动│晋升            │L0/L1     │L2        │L3        │L4       │L5
        ▼                   │          │          │          │        │
  ┌───────────┐        ┌────┴───┐ ┌────┴────┐ ┌───┴────┐ ┌───┴───┐ ┌──┴────┐
  │ L-S 短期  │──晋升─▶│L0 raw  │ │L2       │ │L3      │ │L4 跨  │ │L5     │
  │(rolling)  │        │+L1 sum │ │episodic │ │semantic│ │项目   │ │画像/  │
  └───────────┘        │(vault) │ │(vault)  │ │(vault) │ │(user) │ │程序性 │
        衰减/TTL ◀──────┴────────┴─┬───────┬─┴────────┴─┬─────┬─┴───────┘
                                   │巩固(CPU)│           显式同意│ 巩固(LLM 限额)
                                   ▼         ▼                   ▼
                       consolidation_agent  semantic.rs      profile_agent (NEW)
                       (确定性, 无 LLM)      (hdbscan+LLM)    (周期, LLM 限额)
```

**层职责一览**:

| 层 | 名称 | 存储 | 易失性 | 单位 | 现状 |
|----|------|------|--------|------|------|
| **L-W** | 工作/会话记忆 | 内存(in-process) | 易失(会话结束清) | 当前对话 turns | ⚠️ 隐式(compact_history) → **形式化** |
| **L-S** | 短期滚动记忆 | `short_term`(NEW 表) | 衰减(TTL/滚动窗口) | 近期交互摘要 | 🆕 增量 |
| **L0** | 原始 chunks | `items.content`+vectors+FTS | 持久 | chunk | ✅ 已有 |
| **L1** | chunk 摘要 | `chunk_summaries` | 持久 | chunk 摘要 | ✅ 已有 |
| **L2** | 情景记忆 | `memories(kind=episodic)` | 持久(可冷却) | 事件/交互 | ✅ 已有 |
| **L3** | 语义记忆 | `memories(kind=semantic)` | 持久 | 主题知识 | ✅ 已有 |
| **L4** | 跨项目记忆 | `cross_project_memory`(NEW, user-scope) | 持久 | 可迁移知识 | 🆕 增量(基于 portability) |
| **L5** | 画像/程序性 | `memories(kind=profile/procedural)` | 持久(慢巩固) | 用户模型/工作流 | 🆕 增量 |

**数据流关键路径**:
- **写入(晋升链)**:交互 → L-W(内存)→ 滚动落 L-S → 达阈晋升 L2 episodic → 确定性/hdbscan 巩固 L3 semantic → 跨多 vault 共性巩固 L5 画像;L2/L3 中标记 portable 的经**显式同意**进 L4。
- **衰减链**:L-W 会话结束清空;L-S 按 TTL/访问衰减(低分淘汰,不入 L2 的丢弃);L2 长期不访问 → 冷却(已有 cold 概念);L4/L5 慢衰减(用户可手动遗忘)。
- **读取(检索路由)**:`assemble_context` 按 query shape(已有 Recall/Overview/Precise)扩展为多层候选 → 各层向量检索 → **RRF 融合** → 覆盖门 → token 预算裁剪。跨项目层 L4 仅当当前 vault 命中不足且用户开启跨项目时纳入。

---

## 4. 模块边界

| 模块 | 文件 | 改动 |
|------|------|------|
| 记忆门面 | `memory/mod.rs` | 导出新层 API + 层枚举 `MemoryLayer` |
| 工作记忆 | `memory/working.rs` (NEW) | 形式化 L-W:会话 turns 缓冲 + token 预算 + 落 L-S 钩子 |
| 短期记忆 | `memory/short_term.rs` (NEW) | L-S 滚动窗口 + 衰减 + 晋升 L2 判定 |
| 画像/程序 | `memory/profile.rs` (NEW) | L5 巩固(跨 vault 共性 → 用户画像 + 工作流模式) |
| 跨项目 | `memory/cross_project.rs` (NEW) | L4:在 portability 基础上做 user-scope 共享层 + 同意门 |
| 检索路由 | `memory/assembler.rs` | 扩展为 6 层 RRF 融合 + 跨项目纳入门 |
| 检索 | `memory/retrieval.rs` | 索引扩展支持 L4/L5 kind |
| 存储 | `store/short_term.rs` / `store/cross_project.rs` (NEW) + `store/migrations_mem.rs` | 新表 + 迁移 |
| 巩固编排 | `memory/consolidation_agent.rs` + sleep-time worker | 加 L-S→L2 与 L5 周期 |

**跨仓边界**:纯 attune-core,不涉 attune-pro / cloud。L4 跨项目是**本机多 vault**,不调云。

---

## 5. API 契约

```rust
/// 记忆层枚举 — 检索/写入/统计统一维度
pub enum MemoryLayer { Working, ShortTerm, Episodic, Semantic, CrossProject, Profile }

// L-W 工作记忆(会话内,内存)
fn working_push(session_id, turn) -> ();
fn working_context(session_id, token_budget) -> Vec<ContextBlock>;   // 取代 compact_history
fn working_flush_to_short_term(session_id, store, dek) -> Result<usize>;  // 会话结束/窗口满

// L-S 短期记忆(衰减滚动)
fn short_term_insert(store, dek, summary, score) -> Result<String>;
fn short_term_decay_cycle(store, now) -> Result<DecayReport>;        // TTL + 低分淘汰
fn short_term_promote_candidates(store) -> Result<Vec<Candidate>>;   // 达阈 → L2

// L4 跨项目(user-scope,显式同意)
fn cross_project_enable(consent: bool) -> ();                        // 默认 false
fn cross_project_contribute(memory_id, store, dek) -> Result<()>;    // 显式标 portable
fn cross_project_search(query, dek) -> Result<Vec<MemoryHit>>;

// L5 画像/程序性
fn profile_consolidate_cycle(stores: &[VaultRef], llm, dek) -> Result<ProfileResult>;
fn profile_get(dek) -> Result<UserProfile>;

// 检索路由(扩展)
fn assemble_context(query, layers: &[MemoryLayer], token_budget, ...) -> AssembledContext;
```

**HTTP/UI**(attune-server,后续 plan 细化):`GET /api/v1/memory/layers`(各层统计)· `POST /api/v1/memory/cross-project/consent` · `GET /api/v1/memory/profile` · `POST /api/v1/memory/forget`(按层遗忘)。

---

## 6. 扩展点 / 插件接口

- **新层可插拔**:`MemoryLayer` 枚举 + trait `LayerStore { insert / search / decay / promote_to }`,后续加 L6(如团队共享记忆,enterprise)实现该 trait 即接入路由。
- **晋升策略可配**:`PromotionConfig`(已有)扩展到 L-S→L2、L2→L4;阈值进 settings。
- **巩固调度**:sleep-time worker 注册各层 cycle,新层加一行注册。

---

## 7. 错误 + 边界 case

| 场景 | 行为 |
|------|------|
| L-W 会话 token 超预算 | 滚动丢最旧 turn(保留首+尾,§4.5G truncation),不报错 |
| L-S 衰减 cycle 与晋升竞争 | 锁序遵 `fulltext→vectors→vault`(CLAUDE.md);晋升先于衰减,已晋升不淘汰 |
| L4 未同意却调 contribute | 返回 `cross-project-disabled`(kebab code),不静默 |
| L4 跨 vault DEK 不同 | L4 用独立 user-scope DEK(§11 风险);vault DEK 不外泄到 L4 |
| L5 巩固 LLM 不可用 | graceful:跳过本周期,保留旧画像,telemetry 记 fail(§4.5F) |
| 空库 / 冷启动 | 各层空返回空,assembler 回退 L0+L1(已有覆盖门保证无回归) |
| 跨项目隐私误流 | 默认 consent=false;contribute 必显式;遗忘可彻底删 L4 行 |

---

## 8. 成本契约(CLAUDE.md §成本感知)

| 层 | 操作 | 成本层级 |
|----|------|---------|
| L-W | push / context 取 | 🆓 零成本(内存,毫秒) |
| L-S | insert / decay / 晋升判定 | 🆓 零成本(CPU,确定性,无 LLM) |
| L2 巩固 | consolidation_agent(确定性) | ⚡ 本地(CPU) |
| L3 巩固 | semantic.rs(hdbscan + LLM 写摘要) | 💰 LLM,受 `MemoryConsolidation` 限额(已有) |
| L4 | contribute / search(向量) | ⚡ 本地(embedding 复用) |
| L5 | profile_consolidate(LLM 周期) | 💰 LLM,**复用 L3 同限额**,低频(日级) |

**硬约束**:L-W/L-S/L4 检索**绝不触发 LLM**;L3/L5 巩固是 sleep-time 后台、受限额、可在"暂停后台任务"开关下停。建库阶段永不升到 L5。

**省 token 机制(用户诉求"记忆结构如何省 token")**:
- 各层逐级压缩:L0 raw → L1 摘要 → L2 事件摘要 → L3 主题摘要 → L5 画像(最压缩)。检索优先取高层压缩摘要,raw chunk 仅 precise 查询取(已有 assembler 覆盖门)。
- L-W 滚动 truncation(保首尾)避免长会话 token 爆炸 + 保 prompt-cache 前缀稳定(§4.5G)。
- 多层 RRF 后按 token 预算裁剪,高压缩层优先填预算。

---

## 9. 测试矩阵(§6.1 6 类下限)

| 类型 | 用例 |
|------|------|
| happy | 交互→L-W→L-S→L2→L3→L5 全晋升链;跨 vault contribute→L4 search 命中 |
| edge | 空库 / 单 turn / L-S 刚好达阈 / L4 单 vault(无可跨)/ token 预算=0 |
| error | L4 未同意 contribute / L5 LLM 挂 / DEK 错 / 衰减与晋升竞争 |
| adversarial | 跨项目隐私注入(vault A 私密内容不应未同意出现在 vault B 检索)— **P0 隐私测试** |
| 并发 | 多会话同时写 L-W;衰减 cycle 与检索并发(锁序验证,复用 B1 死锁回归) |
| 资源 | 大 L-S(10k 短期)衰减性能;L4 跨 10 vault 检索延迟 |
| 省 token 实测 | 同一 recall 查询:6 层路由 vs L0-only 的 token 数对比(证明省 token,§6.3 引 runs) |
| 多 seed | L3/L5 LLM 巩固 N=3 §4.5;晋升排名稳定性 |

**通过判据**:跨项目隐私隔离 100%(adversarial 0 泄漏)· 省 token 对照实测有数据(非声称)· 全晋升链 E2E 通 · 锁序无死锁回归。

---

## 10. 向后兼容

- 现 L0–L3 数据**零迁移**(语义不变,只加路由);`memory_migrations` 加版本号。
- 新表 `short_term` / `cross_project_memory` 经 `migrations_mem.rs` `CREATE TABLE IF NOT EXISTS` 加;老 vault 打开自动建空表。
- `compact_history` 保留为 L-W 的薄封装(老调用方不破)。
- L4 默认 consent=false → 老用户行为完全不变(跨项目不流动)。
- portability 导出格式加 L-S/L4/L5(版本化 schema,老导出文件仍可导入)。

---

## 11. 风险登记

| 风险 | 缓解 |
|------|------|
| **跨项目隐私泄漏**(L4 把 vault A 私密带到 B) | 默认 consent=false;contribute 必显式逐条;L4 独立 user-scope DEK;adversarial P0 测试钉死;UI 明示"此知识将跨项目可见" |
| L4 DEK 管理(跨 vault 加密边界) | L4 用独立派生 key(不复用任何单 vault DEK);锁屏时 L4 同样 sealed |
| 巩固错误(L5 画像把噪声当模式) | L5 高阈值(跨 ≥N vault 共性才入)+ 用户可见可删 + N=3 多 seed 验证 |
| 存储膨胀(L-S/L2 无界增长) | L-S TTL+低分淘汰;L2 冷却;各层有硬上限 + UI 显示各层占用 |
| 锁序/死锁(新层引入新锁) | 严遵 `fulltext→vectors→vault` 序(CLAUDE.md);新层锁纳入序;复用 B1 回归 |
| 成本契约破坏(检索误触 LLM) | L-W/L-S/L4 检索路径静态保证无 LLM 调用;CI 加 grep 守卫 |
| prompt-cache 失效(L-W 前缀不稳) | L-W truncation 保前缀稳定(§4.5G);监控 cache_hit_rate |

---

## 评审问题(给用户)
1. **层数**:本设计 6 层(L-W/L-S/L0-L3/L4/L5,短期组 2 层)。是否接受?或要更细(如 L5 拆画像/程序性为两层 = 7 层)?
2. **跨项目 L4**:同意"默认本地多 vault + 显式同意 + 暂不云同步"的边界?还是要云同步进 cloud?
3. **L5 画像**:是否要做"程序性记忆(工作流模式)"这半,还是先只做"用户画像"半,程序性推 vNext?
4. 优先级:L4 跨项目 与 L5 画像 哪个先落地?(建议先 L-W/L-S 形式化 + L4,L5 次之)

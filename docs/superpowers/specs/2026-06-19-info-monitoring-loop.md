# Spec: 信息监控闭环 — 订阅 / digest / triage / 去重 / 深度研究 / 源-grounded 问答（主动获取，成本契约安全）(DRAFT)

> **Status**: DRAFT — design review pending. **Spec only — NOT implemented（只规划不实现）。**
> **Date**: 2026-06-19  **Author**: spec-analyst（AI draft, for user review per §3.1）
> **北极星空白**: attune 当前采集 connector 底座强（本地/WebDAV/Email/RSS/CloudDrive/Git/LoginAssist），但**只入库不主动推送** —— 用户"关注某主题、想尽快拿到准确信息"这一北极星诉求**无端到端闭环**：无 digest、无 triage/优先级、无主题级监控、无跨源核实、无显式深研。本 spec 在已有采集 + RAG + web_search 底座上补这条"主动获取 → 被动呈现 → 准确问答 / 深研"的闭环。
> **关联 spec（复用 / 边界，全部已读核实）**:
> - [[2026-06-01-rss-cloud-ingest-connectors]]（#1，ON-HOLD）— RSS/CloudDrive 采集源。本 spec **不 supersede #1，含纳并复用其产出**：#1 负责"源 → vault 入库"，本 spec 负责"入库后的监控 / 摘要 / 推送 / 研究"。关系详见 §2 + §6。
> - [[2026-06-17-suggestions-and-thirdparty-accounts]]（建议引擎，DRAFT）— digest 卡复用其**被动可见 + dismiss/mute + 反偷跑 proptest** 范式；二者是同一"零成本主动呈现"家族，digest 卡是 suggestion card 的一个 `kind`。关系详见 §2 + §4。
> - [[2026-06-17-browser-login-assist-session-capture]]（#66，DRAFT）— 需登录的监控源（会员墙站点）走其会话捕获/复用机制 + consent gate；本 spec 不重复实现登录，仅引用。
> - [[2026-06-17-multilayer-memory-architecture]]（5+ 层记忆，DRAFT）— digest/triage 消费 L1 chunk_summaries 缓存省 token；监控事件可作记忆信号源。
> - [[2026-05-28-privacy-logic-strategy]]（OutboundGate 6-kind）— 深研 web 抓取走 `WebSearch` kind；监控自选源走各 connector 既有 kind。直接复用，**不改** gate 模型。
> - CLAUDE.md §成本感知与触发契约（本 spec 第 8 节的最高约束来源）。

---

## 0. 目录 (TOC)

- [1. 目标定位](#1-目标定位)
- [2. 范围边界](#2-范围边界)
- [3. 架构数据流](#3-架构数据流)
- [4. 模块边界](#4-模块边界)
- [5. API 契约](#5-api-契约)
- [6. 扩展点 / 插件接口 + 与 #1 RSS 的关系](#6-扩展点--插件接口--与-1-rss-的关系)
- [7. 错误 + 边界 case](#7-错误--边界-case)
- [8. 成本契约](#8-成本契约)
- [9. 测试矩阵](#9-测试矩阵)
- [10. 向后兼容](#10-向后兼容)
- [11. 风险登记](#11-风险登记)
- [Appendix A: 代码勘查事实表（grounding ground-truth）](#appendix-a-代码勘查事实表grounding-ground-truth)

---

## 1. 目标定位

### 1.1 用户痛点

attune 北极星是"**对你关注的内容，尽快获取准确的信息**"。当前能力链断在两头：

1. **采集进来了，但不告诉你**。RSS/云盘/邮箱/Git 源经周期 worker 入库（#1 + 既有 connector），但**没有任何"你关注的新信息来了"的呈现**。用户必须主动去搜才知道有新内容 —— 违反"主动进化 / 被动捕获"叙事。
2. **没有"关注主题"的概念**。采集源是按**来源**组织的（这个 feed、这个文件夹），不是按**主题**组织的（"RISC-V 工具链进展""某公司财报"）。用户想"盯住一个话题"，得自己记住哪些源相关。
3. **没有 triage**。即使知道有新内容，也无"哪条最相关 / 最重要"的排序，新信息一视同仁地堆在时间线里。
4. **没有跨源核实**。同一事实在多个源出现时，attune 不交叉验证，用户无法判断"这条是否可信 / 是否多源确认"。
5. **深研要手动拼**。用户想"把这个主题查透"时，得自己反复 search + web_search + 读 + 综合，attune 有零件（`web_search` / doc-intel `deep_summary` / RAG）但**没有把它们串成一次"发起 → 多源搜 → 抽取 → 综合成带引用报告"的显式深研动作**。

### 1.2 本 capability 解决什么

一条**信息监控闭环**：

```
用户声明关注主题/源  →  后台零成本监控（复用采集 worker + 确定性 triage/dedup）
                     →  周期 digest（被动可见，源-grounded，LLM 摘要可关）
                     →  对关注主题的源-grounded 准确问答（复用 RAG + grounding）
                     →  用户显式发起深度研究（多源搜索 + 抽取 + 综合 + 跨源核实，复用 web_search + doc-intel）
```

### 1.3 与产品定位对齐

- **主动进化 = 系统基于本地信号被动呈现机会**（per [[2026-06-17-suggestions-and-thirdparty-accounts]] §1）：监控 + digest 把"主动"落在**零成本确定性层 + 周期批量摘要**，绝不后台偷跑 LLM 猜需求（成本契约 §8）。
- **降低 token + 数据安全**：监控的是**用户自选源 + 自己 KB**，数据在本地 vault；外部抓取（深研）才走 OutboundGate。
- **准确（北极星核心词）**：问答 + digest + 深研全部**源-grounded**（带引用，复用既有 `chat_reliability` grounding + doc-intel 引用 offset），不产出无源断言。
- **OSS 边界**：通用主题监控 / digest / 深研对**任何领域**个人用户有价值（程序员盯 release、研究者盯 arXiv、普通用户盯某话题）→ 进 OSS；行业情报（法规更新监控 / 专利监控 / 竞品情报）= attune-pro **复用本 spec 的规则注册扩展点**（§6），零行业绑定进 OSS。

### 1.4 与 CLAUDE.md 规则映射

| 规则 | 本 spec 落点 |
|------|-------------|
| 全局 §3.1 11 节 spec-first | 本文档 |
| 项目 §成本感知三层 + "分析阶段永远等用户开口" | §8（监控/triage/dedup=零成本；digest LLM 摘要=⚡/💰 周期批量+可关；深研=显式触发） |
| 项目 "不做 AI 主动建议下一个问题 / 猜你需要什么" | §2-OUT（监控只呈现**已存在的确定性事实**：这些源出了新内容、这条与你关注主题匹配；不生成开放式建议问题） |
| 全局 §6.1 6 类测试下限 + §2.3 多 seed | §9（确定性层 6 类下限；深研/digest 摘要 LLM 路径走 N≥3 + floor gate） |
| 全局 §4.5 LLM 兜底（schema-guided / 重试-验证 / few-shot / 3-tier / 弱模型 fallback / telemetry） | §3 + §9（digest 摘要 + 深研综合是 LLM agent，全程套 §4.5） |
| 全局 §1.4 secrets | §11 R8（登录源凭据走 #66 会话加密；不 echo / 不入 log） |
| 项目 OSS 边界 | §2 写死零行业绑定；行业规则走 §6 扩展点在 pro |
| 项目 Lock ordering（fulltext→vectors→vault；embedding 独立） | §11 R5 + §3 持锁设计 |

---

## 2. 范围边界

> **本 sprint 版本归属**：建议作为 `v1.x` 系采集/记忆能力增量的一个 distinct deliverable minor，确切 minor 号按 merge 顺序确定（per 项目 §7.1.7「版本号 merge 时刻确定」）。**强依赖 #1（RSS/CloudDrive 采集源）落地**，因 digest 的价值正比于监控源覆盖面（见 §6 依赖关系）。

### ✅ 本 sprint 做（写死）

**A. 主题/源监控（Watch）—— 零成本确定性**
1. 新概念 `Watch`（关注项）：用户声明一个**关注主题**（关键词集 / 实体集 / 语义 anchor 文本）+ 可选**绑定源**（哪些 connector source 属于该 watch；不绑则全源）。
2. 监控**不新增采集机制** —— 复用既有 connector 周期 worker（RSS `start_rss_sync_worker` / WebDAV / Email / CloudDrive / Git / LoginAssist）。Watch 是**在已入库 item 上的确定性匹配层**：新 item 入库 → 发 `doc_create` 信号（已有，`signals.rs`）→ 监控引擎用**确定性匹配**（关键词 / 实体重叠 / 已有向量相似度，**复用** `search` 的 usearch/tantivy，**不新调 embedding/LLM**）判定该 item 是否命中某 watch。
3. 命中产出 `WatchHit`（watch_id + item_id + 匹配分 + 命中理由）。

**B. digest 推送 —— 被动可见，源-grounded，LLM 摘要可关**
1. 周期（日/周，用户配）聚合每个 watch 的新 `WatchHit` → 一张 **digest 卡**（复用 [[2026-06-17-suggestions-and-thirdparty-accounts]] 的 suggestion card 体系，新增 `kind="digest"`）。
2. digest 卡**被动可见**：进 UI 固定区域（建议卡区域）+ 可选系统通知（默认**关**，per §8/§11 不打扰）。**无红点催促 / 无打断弹窗**。
3. digest 正文**两态**：
   - **默认（零成本）**：标题列表 + 每条的源引用 + extractive 抽取式一句话预览（**零 LLM**，复用 doc-intel `extractive`）。
   - **LLM 摘要（可选，⚡/💰）**：用户在 Settings 开启"digest 智能摘要"后，**周期批量**（生成 digest 时）调 LLM 出"你关注的 X 主题本周要点"摘要 —— 但这是**后台任务队列**任务，**受顶栏"暂停后台任务"开关控制**，可随时关；摘要**源-grounded**（每条要点带 item 引用）。
4. digest 卡 dismiss / mute（复用 suggestion 生命周期）。

**C. triage / 优先级 —— 零成本确定性排序**
1. `WatchHit` 按**确定性可解释分**排序：匹配分（关键词/实体/向量相似度）× 时新性（recency）× 源权重（用户可配）× 用户历史交互（browse_signals/citation_hit，已有）。
2. 高优先突出（digest 卡内排前 + 视觉强调），**不调 LLM**。排序公式**完全可解释**（每条 hit 显示"为什么排这"）。

**D. 去重 —— 跨源/跨时间，零成本确定性**
1. **跨源**：同一内容经不同源进 vault（如同一篇文章 RSS + 云盘都收到）→ 复用既有 `content_hash` 短路（`find_item_by_content_hash`，O(1)）+ 新增**近似去重**（标题 + 高相似向量阈值）合并为一条 digest 条目，标"多源"。
2. **跨时间**：已在过去 digest 出现过的 hit（已读 / 已 dismiss）不重推 —— digest 记 `last_digested_marker` per watch（类比 RSS `last_cursor`）。
3. 去重**纯确定性**（hash + 向量阈值 + marker），**不调 LLM**。

**E. 源-grounded 准确问答 —— 复用 RAG，限定 watch 范围**
1. 对某 watch 的问答：复用 `chat::search_with_context` RAG + `chat_reliability` grounding，但**检索范围限定**到该 watch 命中的 item 集（scoped RAG）。
2. 答案**必带源引用**（既有 chat 已带 citation；本 spec 仅加 watch-scope 过滤参数）。这是北极星"准确"的直接落点。

**F. 深度研究综合 —— 用户显式触发**
1. 用户对某主题**显式发起**"深度研究" → 一个 orchestrated agent：
   - **多源搜索**：vault RAG（`search_with_context`）+ web_search（`web_search`，zero-API 浏览器，走 OutboundGate `WebSearch`）。
   - **抽取**：对命中文档复用 doc-intel `extractive`（零 LLM 预裁）+ `deep_summary`（map-reduce 省 token）。
   - **综合**：reduce 成一份**带引用的研究报告**（叙述 + 引用 offset），输出模式 = 叙述（per agent 输出模式契约）。
2. 深研**全程显式**：用户点"深度研究"按钮才启动，UI 显示预估成本（本地/云端 + tok·$）。

**G. 跨源核实 —— 深研内的确定性 + LLM 判定子步**
1. 深研报告内对**关键事实**做跨源核实：同一 claim 在 ≥2 独立源出现 → 标"多源确认"；仅单源 → 标"单源待证"；多源冲突 → 标"存在分歧"并列出。
2. 核实的**源计数 + 冲突检测是确定性**（claim → 源 set）；**判定两段文字是否表达同一 claim** 的语义步是 LLM（复用 doc-intel `compare` 的语义判定范式，套 §4.5 schema-guided + 重试 + 降级）。

### ❌ 本 sprint 不做（写死，silent scope creep = bug）

- ❌ **新采集机制 / 新 connector** —— 监控复用 #1 + 既有 6 connector，**不新增源类型**。
- ❌ **后台自动 LLM 生成 digest 摘要作为默认** —— 默认 digest 是零成本 extractive；LLM 摘要是**显式开启 + 后台队列可暂停**（§8）。
- ❌ **"AI 主动建议下一个问题 / 猜你需要什么 / 主动追问"** —— 监控只呈现确定性事实，不生成开放式建议（CLAUDE.md 明列禁止行为）。
- ❌ **推送通知默认开 / 红点 / 打断式弹窗** —— digest 被动可见，通知默认关。
- ❌ **任何行业情报规则 / 法规-专利-竞品绑定** —— OSS 边界；行业 watch 规则走 §6 扩展点在 attune-pro。
- ❌ **跨设备 digest / watch 同步** —— 本地态；跨设备走 [[2026-06-15-memory-continuity-and-portability]] 的用户主动 export/import。
- ❌ **深研的全自动 agentic 多轮浏览（点击 / 表单 / 多 hop 爬）** —— 本 sprint 深研 = vault RAG + web_search SERP 抓取 + 综合；多 hop agentic 浏览推 v.next（且须走 #66 ToS/consent + 速率铁律）。
- ❌ **改写 OutboundGate 6-kind / suggestion 卡体系 / connector trait 签名** —— 全部复用，不改。
- ❌ **实时（秒级）监控** —— 监控对齐 connector 周期 worker 节奏（分钟~小时级 poll），非实时流。

### ⏭️ 推迟到 v.next

- 深研全自动 agentic 多 hop 浏览（依赖 #66 落地 + 速率/ToS 铁律）。
- 跨设备 watch/digest 同步。
- LLM 驱动的"主题自动发现"（从用户行为里聚类出 watch 候选）—— 本 sprint watch 由用户**显式声明**。
- 定时 cron 级精细调度（本 sprint 复用 connector worker 的 interval 节奏 + 简单日/周 digest 周期；通用 cron 调度依赖 [[2026-06-10-k3-g5-durable-job-queue]] 之上的调度层，那是另一层）。

---

## 3. 架构数据流

### 3.1 总览（零成本监控层 + 显式 LLM 层 严格分离）

```
┌──── 已有采集层（#1 + 既有 connector，本 spec 不改）────┐
│ RSS / CloudDrive / WebDAV / Email / Git / LoginAssist │
│   周期 worker → ingest_document → vault items          │
│   入库即发 doc_create 信号 (signals.rs，已有)           │
└────────────────────────┬──────────────────────────────┘
                         │ doc_create 信号 + 新 item
                         ▼
┌──────────── 监控引擎（新，确定性 纯 CPU/向量，零 LLM）────────────┐
│ WatchMatcher::evaluate(new_items, watches)                        │
│  ├─ 关键词/实体匹配（复用 entities/tantivy，零成本）              │
│  ├─ 向量相似度（复用已算好的 item 向量 + watch anchor 向量，      │
│  │   usearch HNSW；watch anchor 向量建 watch 时一次性 embed）     │
│  ├─ dedup: content_hash O(1) + 近似(标题+向量阈值) + 跨时间 marker│
│  └─ triage: 可解释分 = match × recency × source_w × interaction  │
│         │                                                         │
│         ▼  Vec<WatchHit>（watch_id, item_id, score, reasons）     │
└─────────┬────────────────────────────────────────────────────────┘
          │
   ┌──────┴───────────────────────────────────────────┐
   ▼ 周期 digest 聚合                                    ▼ 实时单 hit（可选 WS 推）
┌──────────── digest 层 ────────────┐          suggestion_card_new WS（已有通道）
│ DigestBuilder（按 watch 周期聚合）  │
│  默认: 标题+源引用+extractive 预览  │  ←─ 零 LLM（doc-intel extractive 复用）
│  可选: LLM 周期批量摘要（§4.5）     │  ←─ ⚡/💰 后台任务队列, 可暂停, 源-grounded
│         │ → DigestCard(kind=digest, 复用 suggestion card 体系)   │
└─────────┴────────────────────────┘
          │ 被动可见（固定区域）+ 通知(默认关)
          ▼ 用户
   ┌──────┴───────────────────────────────────────────────────────┐
   ▼ (显式) 对 watch 问答                  ▼ (显式) 发起深度研究
┌──── scoped RAG ────┐          ┌──────── DeepResearch orchestrator ────────┐
│ search_with_context │          │ 1 多源搜: vault RAG + web_search(OutGate)  │
│  限 watch.item 集   │          │ 2 抽取: extractive 预裁 + deep_summary     │
│ + chat_reliability  │          │ 3 跨源核实: claim→源set(确定) + 同义判定(LLM)│
│   grounding(带引用) │          │ 4 综合: reduce → 带引用研究报告(叙述)       │
└─────────────────────┘          └────────────────────────────────────────────┘
   💰 显式 LLM（既有 chat 成本路径）        💰 显式 LLM（UI 显示预估 tok·$）
```

**关键不变量（编译期可守）**：`WatchMatcher::evaluate()` 与 `DigestBuilder::build_default()` 的签名**不含 LLM provider 句柄** —— 监控 + 默认 digest 路径在编译期就无法偷跑 LLM（同 [[2026-06-17-suggestions-and-thirdparty-accounts]] §3 反偷跑设计，§9 用 panic-mock 断言 0 次 LLM 调用）。LLM 只可能出现在：(a) digest LLM 摘要（显式开启的 `DigestBuilder::build_llm_summary(..., llm)`，后台队列可暂停）；(b) watch 问答（既有 chat 显式路径）；(c) 深研（显式触发）。

### 3.2 DB tables

复用既有：`items` / `indexed_files` / `skill_signals` / `suggestion_cards`（来自建议引擎 spec）+ usearch 向量 + tantivy FTS。

新增：
```sql
-- 关注项（用户显式声明的监控主题/源）
CREATE TABLE IF NOT EXISTS watches (
    id              TEXT PRIMARY KEY,
    label           TEXT NOT NULL DEFAULT '',
    keywords_json   TEXT NOT NULL DEFAULT '[]',  -- 关键词集
    entities_json   TEXT NOT NULL DEFAULT '[]',  -- 实体集（复用 entities.rs 抽取）
    anchor_text     TEXT NOT NULL DEFAULT '',    -- 语义 anchor（建 watch 时 embed 一次，向量存 usearch 专用 namespace）
    source_ids_json TEXT NOT NULL DEFAULT '[]',  -- 绑定的 connector source id（空=全源）
    match_threshold REAL NOT NULL DEFAULT 0.55,  -- 命中阈值（复用 search 已验证常量量级）
    source_weights_json TEXT NOT NULL DEFAULT '{}', -- 可选 per-source 权重
    digest_period   TEXT NOT NULL DEFAULT 'weekly', -- 'daily' | 'weekly' | 'off'
    llm_summary     INTEGER NOT NULL DEFAULT 0,   -- 0=零成本extractive digest; 1=开启LLM摘要
    notify          INTEGER NOT NULL DEFAULT 0,   -- 系统通知默认关
    last_digested_marker TEXT,                    -- 跨时间去重游标（上次 digest 的最大 item created_at）
    last_digested_at TEXT,
    enabled         INTEGER NOT NULL DEFAULT 1,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_watches_enabled ON watches(enabled);

-- watch 命中（去重后的，供 digest 聚合 + triage 排序）
CREATE TABLE IF NOT EXISTS watch_hits (
    id          TEXT PRIMARY KEY,
    watch_id    TEXT NOT NULL,
    item_id     TEXT NOT NULL,
    score       REAL NOT NULL,
    reasons_json TEXT NOT NULL DEFAULT '[]',  -- 可解释命中/排序理由
    dedup_group TEXT,                          -- 近似去重组（同组=多源同内容）
    digested    INTEGER NOT NULL DEFAULT 0,    -- 是否已进过某次 digest
    created_at  TEXT NOT NULL,
    UNIQUE(watch_id, item_id)                  -- 同 watch 同 item 唯一（去重防线）
);
CREATE INDEX IF NOT EXISTS idx_watch_hits_watch_digested ON watch_hits(watch_id, digested);
```

> 深研报告**不新增持久表**：报告作为一条 `items`（source_type 标 `research_report`）落 vault（用户的研究结论本就是知识），复用既有 item 加密 + 检索；或临时返回不落库（用户选）。

### 3.3 增量 / 去重三层防线（对齐 #1 设计语汇）

1. **入库去重**（#1 已有，复用）：`content_hash` O(1) 短路 + `indexed_files` source_ref 短路 —— 完全相同内容根本不会两次入库。
2. **watch 命中去重**：`watch_hits` 表 `UNIQUE(watch_id, item_id)` + 近似去重（标题归一 + 向量相似度 ≥ 阈值 → 同 `dedup_group`，digest 内合并为一条标"多源"）。
3. **跨时间去重**：`watches.last_digested_marker`（上次 digest 覆盖到的 item created_at）+ `watch_hits.digested` 标志 —— 已 digest / 已 dismiss 的 hit 不重推。

### 3.4 周期调度（复用既有 worker 节奏，不引新调度器）

- **监控匹配**：搭车现有 connector worker —— 每次 worker 完成一轮 ingest（新 item 入库）后，触发一次 `WatchMatcher::evaluate(新 item batch, enabled watches)`（增量，仅算新 item，**非全表扫描**，§11 R3）。
- **digest 生成**：一个轻量 `start_digest_worker`（对齐 `start_rss_sync_worker` 模式），每小时醒来检查"哪些 watch 到了 digest 周期"（now ≥ last_digested_at + period），到期才聚合生成卡。零成本路径直接出卡；LLM 摘要路径**入后台任务队列**（受暂停开关控制）。

---

## 4. 模块边界

### attune-core（`rust/crates/attune-core/`）

| 文件 | 改动 |
|------|------|
| `src/monitoring/mod.rs`（新模块） | `Watch` / `WatchHit` 类型；`WatchMatcher::evaluate()`（确定性，签名**无 LLM 句柄**）；triage 排序；近似去重 |
| `src/monitoring/digest.rs`（新） | `DigestBuilder::build_default()`（零 LLM，extractive）+ `build_llm_summary(..., llm)`（§4.5 套装，显式）；digest 卡组装（复用 suggestion card 类型，kind=digest） |
| `src/monitoring/deep_research.rs`（新） | `DeepResearch` orchestrator：多源搜（RAG+web_search）→ 抽取（extractive/deep_summary 复用）→ 跨源核实 → 综合报告；§4.5 兜底 + N≥3 gate |
| `src/store/watches.rs`（新） | `watches` / `watch_hits` CRUD（对齐 `store/rss_feeds.rs` 风格） |
| `src/store/mod.rs` | 加 `watches` / `watch_hits` CREATE TABLE IF NOT EXISTS + `pub mod watches;` |
| `src/store/signals.rs` | **不改**（消费既有 `doc_create` 信号） |
| `src/store/suggestions.rs`（来自建议引擎 spec） | digest 卡复用其表 + 生命周期（**依赖建议引擎 spec B 落地或并行**；若先行则本 spec 暂建轻量子集，§10） |
| `src/chat.rs` / `src/search.rs` | **薄改**：`search_with_context` 加可选 `scope_item_ids: Option<&[ItemId]>` 参数（watch-scoped RAG）；既有调用传 None 行为不变 |
| `src/document_intelligence/`（extractive / deep_summary / compare） | **不改**，复用 |
| `src/web_search.rs` / `web_search_browser.rs` | **不改**，深研复用 |
| `src/outbound_gate.rs` | **不改**，深研 web 抓取走既有 `WebSearch` kind |

### attune-server（`rust/crates/attune-server/`）

| 文件 | 改动 |
|------|------|
| `src/routes/watches.rs`（新） | watch CRUD + hits 查询 + 手动触发 digest + watch-scoped 问答 + 发起深研 |
| `src/routes/mod.rs` | `pub mod watches;` + nest `/api/v1/monitoring/*` |
| `src/state.rs` | 加 `start_digest_worker`（对齐 `start_rss_sync_worker`）；connector worker 完成 ingest 后 hook `WatchMatcher::evaluate` |
| `src/routes/vault.rs` | unlock 路径启动 `start_digest_worker`（对齐 RSS worker 启动位） |
| `src/routes/suggestions.rs`（建议引擎 spec） | digest 卡经其 GET /suggestions 暴露（kind=digest），或本 spec 加 `/monitoring/digest` 专路由（§5 二选一，评审定） |
| `attune-server/ui/src/views/` | Watch 管理面板（增删改 + 绑源 + digest 周期/摘要开关）+ digest 卡区域（复用建议卡区域）+ 深研入口 + 报告视图 |

### 跨仓边界

- **零跨仓依赖**。不调 attune-enterprise / attune-pro。
- attune-pro vertical plugin 后续经 §6 扩展点注册**行业 watch 规则**（法规更新源模式 / 专利监控 / 竞品）—— 本 spec 只定 trait + OSS 通用规则。

---

## 5. API 契约

挂载前缀：`/api/v1/monitoring/*`（kebab-case，per 项目 API 命名）。

```typescript
// ── Watch 管理 ──
// POST /api/v1/monitoring/watches — 新增关注项（建 anchor 向量一次性 embed=⚡本地）
interface CreateWatchRequest {
  label: string;
  keywords?: string[];
  entities?: string[];
  anchor_text?: string;        // 语义 anchor（可空，则纯关键词/实体匹配）
  source_ids?: string[];       // 绑定 connector source（空=全源）
  match_threshold?: number;    // 默认 0.55
  source_weights?: Record<string, number>;
  digest_period?: "daily" | "weekly" | "off";   // 默认 weekly
  llm_summary?: boolean;       // 默认 false（零成本 digest）
  notify?: boolean;            // 默认 false（不打扰）
}
interface WatchView {
  id: string; label: string; keywords: string[]; entities: string[];
  source_ids: string[]; digest_period: string; llm_summary: boolean;
  notify: boolean; enabled: boolean; last_digested_at: string | null;
  hit_count_pending: number;   // 未 digest 的命中数（确定性计数）
}

// GET    /api/v1/monitoring/watches              → { watches: WatchView[] }
// PATCH  /api/v1/monitoring/watches/:id          { enabled?, digest_period?, llm_summary?, notify?, match_threshold? }
// DELETE /api/v1/monitoring/watches/:id          → 204（hits 级联删；已入库 item 保留）

// ── 命中 / triage ──
// GET /api/v1/monitoring/watches/:id/hits?limit=50 → { hits: WatchHit[] }  按 triage 分降序
interface WatchHit {
  id: string; item_id: string; title: string; score: number;
  reasons: string[];           // 可解释（"关键词 'RVV' 命中 / 向量相似 0.71 / 7天内 / 来源 LWN"）
  dedup_group: string | null;  // 非空=多源同内容
  sources: string[];           // 该 dedup_group 涉及的源（去重后展示"多源"）
  created_at: string;
}

// ── digest ──
// POST /api/v1/monitoring/watches/:id/digest     → 手动触发一次 digest（与 worker 同函数）
interface DigestResponse {
  card_id: string | null;      // 无新命中=null
  entries: number;             // 去重后条目数
  llm_summary_queued: boolean; // true=LLM 摘要已入后台队列（异步出，受暂停开关控制）
  cost_hint: { tier: "free" | "local" | "cloud"; note: string };
}

// ── 源-grounded 问答（watch-scoped RAG，复用 chat）──
// POST /api/v1/monitoring/watches/:id/ask        { question }
interface WatchAskResponse {
  answer: string;
  citations: Array<{ item_id: string; offset_start: number; offset_end: number; snippet: string }>;
  grounded: boolean;           // chat_reliability grounding 结果
  cost_hint: { tier: "local" | "cloud"; note: string };
}

// ── 深度研究（显式触发）──
// POST /api/v1/monitoring/research   { topic, use_web?: boolean, watch_id?: string, persist?: boolean }
interface DeepResearchResponse {
  report_markdown: string;     // 叙述 + 引用
  claims: Array<{
    text: string;
    verification: "multi_source_confirmed" | "single_source" | "conflicting";
    sources: Array<{ kind: "vault" | "web"; ref: string }>;  // 跨源核实
  }>;
  item_id: string | null;      // persist=true 时落 vault 的 research_report item
  token_bill: { input: number; output: number; usd: number; savings_ratio: number };
}
```

### core 关键签名（编译期反偷跑）

```rust
// 监控匹配 —— 签名无 LLM 句柄（编译期无法偷跑 LLM）
pub fn evaluate(new_items: &[ItemMeta], watches: &[Watch], idx: &MatchIndex) -> Vec<WatchHit>;

// 默认 digest —— 零 LLM
impl DigestBuilder {
    pub fn build_default(&self, hits: &[WatchHit], store: &Store) -> DigestCard;          // 无 llm 参数
    pub fn build_llm_summary(&self, hits: &[WatchHit], store: &Store,
                             llm: &dyn LlmProvider) -> Result<DigestCard>;                // 显式 llm，§4.5 套装
}

// 深研 —— 显式 orchestrator
impl DeepResearch {
    pub async fn run(&self, topic: &str, opts: ResearchOpts,
                     rag: &SearchCtx, web: Option<&dyn WebSearchProvider>,
                     llm: &dyn LlmProvider) -> Result<ResearchReport>;
}
```

### CLI（可选）

`attune watch list|add|rm`；`attune research <topic> [--web]`（与 server 对齐，v.next 可补，本 sprint 不强制）。

---

## 6. 扩展点 / 插件接口 + 与 #1 RSS 的关系

### 6.1 扩展点

- **行业 watch 规则**（pro 复用）：`monitoring::WatchRule` trait（`fn matches(&self, item: &ItemMeta, watch: &Watch) -> Option<HitReason>`）。OSS 内置通用规则（关键词/实体/向量）；attune-pro vertical plugin 注册行业规则（law-pro："法规更新源 + 法条变更检测"；patent-pro："专利公告监控"；sales-pro："竞品动态"）—— 零行业绑定进 OSS，规则注册机制本 sprint 定 trait，**插件动态注册留与建议引擎 spec 统一**（§2 写死边界）。
- **深研 source 扩展**：深研的"多源搜"目前 = vault RAG + web_search 两源；新增源（如 #66 登录墙源、特定 API）= 实现 `ResearchSource` trait，零 orchestrator 改动。
- **digest 渲染扩展**：digest 卡正文渲染器可插（默认 extractive；pro 可注册行业摘要模板）。

### 6.2 与 #1 RSS（`2026-06-01-rss-cloud-ingest-connectors`）的关系 —— 明确：**不 supersede，含纳 + 复用**

| 维度 | #1 RSS/CloudDrive connectors | 本 spec 信息监控闭环 |
|------|------------------------------|---------------------|
| **职责** | 源 → vault **入库**（fetch / parse / dedup-into-vault / embed） | 入库后的**监控 / 摘要 / 推送 / 研究** |
| **层级** | 采集层（connector） | 应用层（在 connector 产出之上） |
| **数据流位置** | 上游（produce items + doc_create 信号） | 下游（consume items + 信号） |
| **是否互斥** | **否，互补** | 本 spec **消费** #1 的产出 |

**结论**：
- 本 spec **不 supersede #1** —— #1 是采集底座，本 spec 是其消费者。#1 应**先于或并行**落地（本 spec §2 标"强依赖 #1"）。
- 本 spec **复用** #1 的：connector 周期 worker 节奏（监控搭车其 ingest hook）、`content_hash`/`indexed_files` 去重防线、SSRF 加固（监控不直接出网，但深研 web 抓取复用既有 url_guard）。
- **#1 当前 ON-HOLD** → 本 spec 的监控覆盖面**正比于已落地的 connector**。即使 #1 仍 hold，本 spec 仍可在**既有 connector**（本地文件夹 / WebDAV / Email / Git）上工作（这些已实装）；RSS/CloudDrive 源解 hold 后自动纳入监控（零本 spec 改动，因监控是源无关的 item 层）。
- **建议**：#1 解 hold 与本 spec 同一波采集能力 minor 推进；评审若决定合并，可把本 spec 作为 #1 的"消费层续集"。

---

## 7. 错误 + 边界 case

### 错误码（kebab-case，经 `AppError` → JSON `{"error","code"}`）

| code | HTTP | 触发 | 备注 |
|------|------|------|------|
| `watch-not-found` | 404 | 操作不存在的 watch id | |
| `watch-label-empty` | 400 | label 空 | |
| `watch-no-criteria` | 400 | keywords + entities + anchor_text 全空（无可匹配条件） | |
| `watch-anchor-embed-failed` | 502 | 建 watch 时 anchor embed 失败（embedding provider 不可用） | graceful：仍建 watch，退化为纯关键词/实体匹配 + warning |
| `vault-locked` | 401 | locked 时读 watch/hits/digest（DEK 不可用） | 复用既有语义 |
| `research-web-disabled` | 200(body) | 深研请求 use_web 但 WebSearch kind 被用户禁用 | 退化为仅 vault 深研 + warning，**不报错中断** |
| `research-llm-unavailable` | 503 | 深研/LLM 摘要时无可用 LLM provider | graceful Err + 友好提示（per §4.5） |
| `digest-no-hits` | 200(body) | digest 时无新命中 | `card_id: null`，非错误 |

### 边界 case 矩阵

| 场景 | 期望行为 |
|------|---------|
| watch 无命中 | hits=[]，digest card_id=null（非错误） |
| 100 个 watch × 每轮 500 新 item | 增量匹配只算新 item × enabled watch；§11 R3 验证非 O(N²) |
| 同一文章 RSS + 云盘双源入库 | content_hash 短路（同内容只 1 item）；若内容微差→近似去重合并同 dedup_group，digest 标"多源" |
| 已 dismiss 的 digest 卡内某 hit 又被新 item 强化 | 不复活旧卡；新周期新卡（per suggestion dismiss 语义） |
| watch anchor embed 失败 | 退化纯关键词匹配，不阻塞建 watch |
| 深研 web 被禁 | 退化纯 vault 深研 + warning，不中断 |
| 深研 LLM 不可用 | `research-llm-unavailable` 503，不 panic（§4.5 兜底） |
| 跨源核实：claim 仅单源 | 标 `single_source`（不臆造"已确认"） |
| 跨源核实：多源冲突 | 标 `conflicting` + 并列各源说法（不替用户裁决） |
| 监控匹配规则 panic | 隔离该规则，其它规则继续（per signals 静默约定） |
| LLM digest 摘要时用户点"暂停后台任务" | 队列任务暂停；零成本 digest 已出（不阻塞被动呈现） |

### graceful degradation

- 监控引擎完全不可用 → 退化为"无 digest"（= 当前行为），核心采集/检索不受影响。
- LLM 不可用 → digest 退化为零成本 extractive 版；深研退化为"vault RAG 命中列表 + extractive 预览"（无综合段），明示"LLM 不可用，仅提供检索结果"。
- 单规则/单 hit 错误 → 隔离，不中断整轮。

---

## 8. 成本契约（最高优先，逐条对齐 CLAUDE.md §成本感知）

### 三层成本归属

| 操作 | 成本层 | 触发 | 备注 |
|------|--------|------|------|
| connector 周期采集（复用 #1/既有） | 🆓 零成本 | 周期 worker | 不在本 spec 新增 |
| **watch 匹配（关键词/实体/向量相似度）** | 🆓 **零成本** | 入库后自动（搭车 worker） | 复用已算好的 item 向量；**绝不**为匹配调 LLM |
| **triage 排序** | 🆓 **零成本** | 出 hit 时 | 确定性可解释公式 |
| **去重（content_hash/向量阈值/marker）** | 🆓 **零成本** | 匹配时 | 纯确定性 |
| watch anchor 建立时 embed 一次 | ⚡ 本地算力 | 建 watch（一次性） | 本地 Ollama/ORT，零 API 费 |
| **默认 digest（标题+引用+extractive 预览）** | 🆓 **零成本** | 周期/手动 | doc-intel extractive，**零 LLM** |
| **digest LLM 摘要（可选）** | ⚡/💰 | **用户显式开启**（per-watch `llm_summary`）+ **周期批量** + **后台队列可暂停** | 源-grounded；§4.5 套装；**默认关** |
| watch-scoped 问答 | 💰 时间/金钱 | **用户显式提问** | 走既有 chat 成本路径，UI 已显示 tok·$ |
| **深度研究综合** | 💰 时间/金钱 | **用户显式点"深度研究"** | UI 显示预估本地/云端 + tok·$；省 token 三杠杆（extractive 预裁 + deep_summary map-reduce + chunk_summaries 缓存） |
| 跨源核实同义判定子步 | 💰（深研内） | 深研内 | 复用 compare 语义判定；含在深研成本 |

### 哪些零成本 / 哪些批量 / 哪些显式（守约束的核心回答）

- **🆓 零成本（确定性，绝不偷跑 LLM）**：watch 匹配 / triage 排序 / 去重 / 默认 digest（extractive）。这些是监控闭环的"被动主动"核心 —— 系统主动呈现，但只用确定性本地信号。**架构强制**：`evaluate()` / `build_default()` 签名无 LLM 句柄（编译期守卫，§9 panic-mock proptest）。
- **⚡/💰 批量（显式开启 + 可暂停）**：digest 的 LLM 智能摘要。这是唯一"系统周期触发 LLM"的点，但被三重约束锁死：(1) **per-watch 显式开启**（默认关）；(2) **周期批量**（不是每条新 item 都调，是周期聚合时一次）；(3) **进后台任务队列，受顶栏"暂停后台任务"开关控制**。
- **💰 显式（用户开口才花钱）**：watch-scoped 问答 + 深度研究。100% 用户主动触发，UI 显示预估成本。

### 自检红线（实现期）

- 若实现期发现"为了让 triage 更准 / digest 更聪明，想后台调一次小 LLM 打分/摘要（非用户开启的批量摘要）" → **停，违反成本契约第 2 条**，回头改 spec 评审。
- 若发现"digest 出现时静默预跑 LLM 暖 cache" → **停**（per 建议引擎 spec §8 同款红线）。
- **绝不**做"AI 猜你还想关注什么"的后台 LLM 主题发现（§2-OUT 写死）。

### token / 磁盘估算

- 零成本路径引入 **0 LLM token**。
- watch anchor 向量：每 watch ~1 个向量（usearch f16，~KB 级）。
- digest LLM 摘要（开启时）：每 watch 每周期 1 次 map-reduce，受 deep_summary 省 token 三杠杆压制（短内容旁路零 LLM）。
- 深研：单次受三杠杆 + 用户显式，估算同 doc-intel deep_summary 量级 + web_search 抓取（zero-API，无 token）。
- `watches` / `watch_hits` 表：每 watch ~1KB；每 hit ~200B。
- **无新增大模型 / 二进制捆绑**。

### audit 命令（用户可跑）

```bash
sqlite3 ~/.attune/vault/data.db \
  "SELECT label, digest_period, llm_summary, enabled FROM watches; \
   SELECT watch_id, COUNT(*) FROM watch_hits WHERE digested=0 GROUP BY watch_id;"
```

---

## 9. 测试矩阵（§6.1 六类下限 + §2.3 多 seed）

### 9.1 确定性层（监控 / triage / 去重 / 默认 digest）—— PASS rate = 1.00

> 这些路径**零 LLM**（编译期签名守卫），走 deterministic 6 类下限。multi-seed 不适用（无随机/LLM 方差），但并发 case 固定多线程复跑 N=3 确认无 flake。

| 类 | worked case（输入 → 期望 → 判据） | 工具 |
|----|----|----|
| **happy** | 3 新 item（2 含关键词 "RVV"）+ 1 watch(keywords=["RVV"]) → 2 WatchHit，含可解释 reason | 单元 |
| **happy** | 5 hit → `build_default` 出 DigestCard，每条带源引用 + extractive 预览，0 次 LLM 调用 | 单元 |
| **edge** | watch 无命中 → hits=[]，digest card_id=null；anchor 空但 keywords 非空 → 纯关键词匹配 | 单元 |
| **edge** | triage：同分时按 recency 决胜；source_weight 调高某源 → 该源 hit 排前（可解释分逐项断言） | 单元 |
| **error** | 单 watch 规则 panic → 隔离，其它 watch 仍出 hit；anchor embed 失败 → 退化纯关键词 + warning | 单元 |
| **adversarial** | watch keywords 含正则元字符 / 超长 / 注入 → 当字面量匹配不当 regex 执行；item content 巨量重复词不放大分 | 单元 |
| **去重** | (a) 同 item 两次 evaluate → `UNIQUE(watch_id,item_id)` 不重复 hit；(b) 同内容微差两源 → 同 dedup_group，digest 合一标"多源"；(c) 已 digest 的 hit（digested=1）下轮不重推 | 单元 |
| **concurrent** | connector worker 并发 ingest + digest worker 并发 → Lock ordering(fulltext→vectors→vault) 无死锁；N=3 无 flake | 集成 |
| **resource** | 100 watch × 500 新 item 增量匹配 → 仅算新 item×watch（非全表），耗时线性可观测（§11 R3 守卫） | 单元（规模 case） |
| **反偷跑（最高价值）** | `evaluate()` / `build_default()` 注入会 panic 的 `MockLlmProvider`（或 0-计数断言）→ 证明监控+默认 digest **0 次 LLM 调用** | proptest（CI 硬门） |
| proptest (≥3) | (1) evaluate 对任意信号输入永不触发 LLM 副作用；(2) 同信号集多次 evaluate 的 hit/卡 signature 稳定（幂等）；(3) dismiss 后该 signature 不复现 | proptest |

### 9.2 LLM 层（digest 摘要 / watch 问答 / 深研综合 / 跨源核实同义判定）—— §4.5 + N≥3 floor gate

> 这些是 LLM-driven，**必须**走 CLAUDE.md「Agent 验证铁律」+ §4.5：schema-guided 输出 + 重试-验证(≤3) + few-shot + 3-tier 矩阵(qwen2.5:3b / DeepSeek openai-compat / 强云对照) + 弱模型 fallback + telemetry。gate `#[ignore]` opt-in（对齐 doc-intel `doc_intel_real_llm_gate.rs` 范式），真 provider run-log 落 `reports/runs/<ts>/`。

| 子能力 | floor 判据（N≥3 seed，mean±std） | 兜底 |
|--------|----------------------------------|------|
| digest LLM 摘要 | keypoint-recall ≥ 0.80（要点须源-grounded，每点带 item 引用，grounding-precision ≥ 0.95） | 不可用→退化零成本 extractive digest |
| watch-scoped 问答 | grounded-answer ≥ 0.80（复用 chat_reliability grounding；答案引用必落在 watch.item 集内） | 不可用→`research-llm-unavailable`，仅返回检索命中 |
| 深研综合 | 报告 claim 的引用 grounding-precision ≥ 0.95（不臆造源）；综合覆盖率 keypoint-recall ≥ 0.75 | web 禁→纯 vault；LLM 禁→检索列表+extractive |
| 跨源核实同义判定 | claim 配对 F1 ≥ 0.80（同一 claim 是否多源覆盖；复用 compare 语义判定 floor） | 判定失败→保守标 single_source（不误标 confirmed） |

### 9.3 集成 E2E（≥1，黑盒用户视角）

- `tests/monitoring_subprocess.rs`：起 test server → 注入若干 item（mock 各源）→ 建 watch → 触发监控匹配 → GET hits 验证 triage 排序 → 手动 digest 验证去重 + 卡生成（默认零成本路径，断言 0 次 LLM）→ watch-scoped ask（mock LLM）→ 发起深研（mock web + mock LLM）验证报告含跨源核实标注。对齐 `git_subprocess` / 现有集成测试风格。

### 9.4 回归 fixture

- 反偷跑：「digest 默认路径 0 次 LLM 调用」永久进测试集（CI 硬门，防回归到偷跑）。
- 去重：「同内容两源 → digest 合一标多源」永久 fixture。
- 跨源核实：「单源 claim 不误标 confirmed」永久 fixture（防准确性回归）。
- 每修一个误报/漏报/误标 加 1 fixture。

---

## 10. 向后兼容

### SemVer / schema

- 纯新增：`watches` / `watch_hits` 用 `CREATE TABLE IF NOT EXISTS`，老 vault 升级自动建表，**不影响** `items` / 既有 connector 表。
- `search_with_context` 加可选 `scope_item_ids` 参数（既有调用传 `None` 行为完全不变）—— **非破坏**（内部 API；若签名变更影响多调用方，用 builder 模式或 `Option` 默认值，保证旧调用不改）。
- digest 卡复用 suggestion card 体系：若建议引擎 spec **尚未落地**，本 spec 暂建 `digest_cards` 轻量子集表（与 suggestion 同结构），待建议引擎落地后迁移为 `kind=digest`（§4 标注，迁移 = `INSERT ... SELECT` 一次性）。**首选**：与建议引擎 spec **同波或之后**落地，直接复用其表，避免双表。

### 老 client 行为

- 无 watch = 空表 = 当前行为（无 digest，无监控）。
- Chrome 扩展 / 旧 Web UI 不感知监控（新增 UI 面板属前端增量；REST 向后兼容）。
- 老 vault 首次 unlock → 自动建表 + 启 digest worker（无 watch 即空转，零成本）。

### migration path — worked example

**Before（监控未实装）**：item 入库后仅可被动搜索；无 watch/hits 表。
**After（用户建一个 "RISC-V 工具链" watch 后）**：
```sql
INSERT INTO watches(id,label,keywords_json,anchor_text,digest_period,llm_summary,created_at,updated_at)
VALUES ('w_01','RISC-V 工具链','["RVV","RVA23","autovec"]','RISC-V 编译器与向量扩展进展','weekly',0,...,...);
-- 后续新 item 入库 → WatchMatcher 命中 → watch_hits 增行 → 每周 digest 卡（零成本 extractive）
SELECT title, score FROM watch_hits h JOIN items i ON h.item_id=i.id WHERE h.watch_id='w_01' ORDER BY score DESC;
```
**回滚**：`DELETE /monitoring/watches/w_01` → 级联删 hits；已入库 item 保留。`DROP TABLE watches, watch_hits` 不影响其它能力（仅丢监控配置，items 仍在）。

---

## 11. 风险登记

| # | 风险 | 概率 | 影响 | 缓解 |
|---|------|------|------|------|
| R1 | **成本契约破坏 — 实现期"顺手"给监控/triage/默认 digest 加后台 LLM**（产品最高原则） | 🔴 High | 🔴 High | (1) 架构强制：`evaluate()`/`build_default()` 签名无 LLM 句柄（编译期守卫）；(2) §9 panic-mock proptest 断言 0 次 LLM 调用作 CI 硬门；(3) review 红线条目；(4) digest LLM 摘要只走显式开启+后台队列+可暂停三锁 |
| R2 | **信息过载 — digest 变打扰**（红点/弹窗/通知轰炸，违隐私优先 UX） | Med | High | 写死被动可见 + 通知默认关 + 可 dismiss/mute（复用 suggestion 生命周期）；triage 突出高优先压缩噪声；每 watch 可设 digest_period=off；§9 UX 走 §2.2 用户视角验收 |
| R3 | **triage/匹配 O(N²) 放大**：每新 item × 每 watch × 全 vault 相似度，大 vault 大量 watch 时炸 | Med | High | **增量匹配**：仅算**新 item batch × enabled watch**，不全表重算；向量相似复用已算好的 item 向量 + usearch HNSW 近邻（非暴力 O(N)）；§9 加 100watch×500item 规模 case 验证线性；watch anchor 向量建时一次性算 |
| R4 | **误报/漏报 — 监控匹配不准**（关注主题没收到 / 收到无关内容） | Med | Med | 阈值复用 search 已验证常量量级（match_threshold 0.55 可调）；可解释 reason 让用户看懂为何命中→调阈值/关键词；回归 fixture 累积误报/漏报；triage 把弱命中沉底 |
| R5 | **并发死锁**：connector worker + digest worker + 前台 ask 同时持锁 | Med | High | 严守 Lock ordering（fulltext→vectors→vault；embedding 独立，per 项目铁律）；digest worker 沿用 RSS 三段式（锁外聚合，逐卡短暂持锁）；§9 concurrent N=3 验证 |
| R6 | **跨源核实误判 — 把单源标"多源确认"**（准确性北极星直接受损） | Med | 🔴 High | 源计数+冲突检测确定性（claim→源set）；同义判定 LLM 步走 §4.5 schema+重试，**判定失败保守标 single_source**（不误标 confirmed）；§9 floor F1≥0.80 + 「单源不误标」永久 fixture |
| R7 | **深研失控成本 / web 抓取被封**：深研多源大量 web 抓取烧时间 / 触发源反爬封禁 | Med | Med | 深研显式触发 + UI 预估成本；web_search 复用既有速率限制（zero-API 浏览器，min_interval）；深研有源数/深度上限；登录墙源走 #66 ToS/consent/保守速率铁律（本 spec 不绕过） |
| R8 | **凭据/隐私泄露**：监控登录墙源涉会话凭据；digest/报告进 log | Med | 🔴 High | 登录源凭据走 #66 会话加密（DEK，本 spec 不自存凭据）；日志只打 watch_id/item_id 不打内容；deep research 报告作 item 加密落 vault；per §1.4 不 echo/不入 log |
| R9 | **digest 卡体系依赖建议引擎 spec 未落地** | Med | Low | §10：建议引擎未落地则暂建 `digest_cards` 轻量子集表，待其落地迁移；首选同波/之后落地直接复用，避免双表 |
| R10 | **#1 RSS ON-HOLD → 监控源覆盖面不足** | High | Med | 本 spec 源无关，在既有 connector（本地/WebDAV/Email/Git，已实装）上即可工作；RSS/CloudDrive 解 hold 后零改动纳入；§6.2 建议同波推进 |
| R11 | **弱模型 digest/深研质量崩**（qwen2.5:3b 摘要烂） | Med | Med | §9.2 3-tier 矩阵 + floor gate；弱模型不达 floor → RELEASE.md 标最低 tier（digest LLM 摘要 / 深研 `Requires ≥ <tier>`），弱模型自动退化零成本 extractive digest（§4.5.E）；telemetry 失败率>30% UI 提示切高 tier |
| R12 | **anchor 向量与 item 向量模型不一致**（换 embedding 模型后 watch anchor 旧模型向量，相似度失真） | Low | Med | anchor 向量记 embedding model 版本；换模型时 watch anchor 随 reindex 一并重 embed（复用既有 reindex 管道）；不一致时退化纯关键词匹配 + warning |

---

## Appendix A: 代码勘查事实表（grounding ground-truth）

> 全部 spec-analyst 亲自 Read / grep 核实（截至 develop `fe3b791`），未盲信背景认知。

| 主张 | 核实结果 | 证据 |
|------|---------|------|
| 采集 connector 底座强、6 source 已就位 | **是**。`SourceKind` 7 variant（本地/WebDAV/Email/RSS/CloudDrive/Git/LoginAssist）；RSS 已端到端实装 | `ingest/connector.rs`；#1 spec Appendix A 确认 RSS 实装 |
| 入库有统一管道 + doc_create 信号 | **是**。`ingest_document` 唯一入库；`signals.rs` 有 `doc_create`/`citation_hit` 等 kind + `record_signal_event` | `ingest/pipeline.rs`；`store/signals.rs:16,44-46` |
| connector 周期 worker 节奏可搭车 | **是**。`start_rss_sync_worker` 周期 poll（per-feed interval，到期判断 last_polled_at+interval） | `state.rs:1303,1346` |
| 无 digest / 监控 / triage / 深研实装 | **是，缺口**。grep 无 digest/deep_research/watch 监控模块 | grep `monitoring`/`digest`/`deep_research` 无命中相关实装 |
| web_search 底座（zero-API 浏览器）已有 | **是**。`web_search.rs`（trait+from_settings）+ `web_search_browser.rs`（系统 Chrome 检测+速率） | `web_search.rs:33,57`；`web_search_browser.rs:16,198` |
| chat 已集成 RAG + web_search + grounding | **是**。`chat.rs` 有 `with_web_search` + `search_with_context` + `chat_reliability` grounding + web_search_cache | `chat.rs:23,105,154-166`；agent-capabilities.md §4 chat 行 |
| doc-intel extractive(零LLM) / deep_summary(map-reduce省token) / compare(语义判定) 已有 | **是**。三杠杆省 token + chunk_summaries 缓存 + 真 LLM gate（floor≥0.80, `#[ignore]`） | `document_intelligence/`；agent-capabilities.md §1 |
| OutboundGate 6-kind 含 WebSearch | **是**。Llm/CloudSaas/Webdav/WebSearch/Telemetry/Embedding | `outbound_gate.rs:36-63` |
| suggestion card 体系（被动可见+dismiss/mute+反偷跑） | **规划中**（DRAFT，未实现） | `2026-06-17-suggestions-and-thirdparty-accounts.md` §3 |
| 登录墙源会话捕获机制 | **规划中**（#66 DRAFT，含 ToS/consent/速率铁律） | `2026-06-17-browser-login-assist-session-capture.md` |
| Lock ordering 热点序 | **fulltext→vectors→vault；embedding 独立** | 项目 CLAUDE.md「Lock ordering」节 |
| RSS #1 是否已落地 | **ON-HOLD**（spec DRAFT，未实现；本 spec 含纳复用非 supersede） | `2026-06-01-rss-cloud-ingest-connectors.md` status DRAFT |

**结论对范围的影响**：所有"主动获取+推送"的**零件已在**（connector 采集 / doc_create 信号 / RAG / grounding / web_search / extractive / deep_summary / compare / OutboundGate / suggestion 卡范式），**缺的是把它们串成监控闭环的应用层**（watch 概念 + 确定性匹配/triage/dedup + digest 聚合 + 深研 orchestrator + 跨源核实）。本 spec 是**编排层 spec，不重造底座**；最大风险是成本契约（R1）与准确性（R6），二者均有编译期/测试硬门守卫。

# 写作引擎 — Narrative 生成（Writing Engine：起草 / 改写 / 大纲 / 引用 / 综述）

**Status**: DRAFT — 待用户评审（per `~/.claude/CLAUDE.md §3.1` 11 节门；本文档仅 PLANNING，**不实现**）
**Date**: 2026-06-19
**Owner**: attune 主仓（开源主线，Rust 商用线 `rust/`）
**Target Release**: v1.x（minor 切片在评审通过后由 `superpowers:writing-plans` 拆解；rc 阶段不进新 feature）
**北极星对齐**：补「写论文 / 写文档效率」最大空白 —— 现状 attune 全产品**仅 1 个生成类 agent**（pro `legal_drafter`），
OSS 侧零生成能力（chat / RAG / 抽取 / 批注 全是「读 + 抽 + 判」，**没有「写」**）。

**Extends / 复用（增量，不重复已实现部分；每行 grep 实测 develop @ `fe3b791`）**:
- `document_intelligence/{deep_summary,compare,chapters,extractive,token_bill,model_routing}.rs`
  （**已实现**：省 token 三杠杆 = extractive 预裁 + `chunk_summaries` 缓存 + cheap-MAP/reasoning-REDUCE；`StageLlms` 阶段模型；`TokenBill` 记账；`ModelRouter` 每阶段选模）
- `chat_reliability/agent.rs::evaluate_response`（**已实现**：citation grounding = item_id 引用 token-overlap ≥ 阈值校验 → 写作引擎的 grounding validator 复用同机制）
- `ai_annotator.rs::{findings_schema, validate_findings_json, LocatedFinding}`（**已实现**：schema-guided JSON + `LLM_MAX_ATTEMPTS=3` 重试-验证 + 后端自定位 offset（不让 LLM 输出 offset）→ 改写「批阅模式」复用同定位/重试栈）
- `search.rs:367::search_with_context`（**已实现**：vault-wide RAG，doc-intel 当前仅文档内检索；本 spec 把 vault RAG 接入综述/引用选材）
- `routes/documents.rs::{enforce_gate, is_tier3_*}` + `403 {code:"membership-required"}`（**已实现**：tier-3 LLM 阶段会员门控 → 写作生成阶段复用同门控）
- attune-pro `plugins/law-pro` `legal_drafter`（agent.yaml `output_modes.default=narrative`，红线 `no_hallucinated_citation`，`[请律师确认]` 占位）—— **本 spec 的 OSS 引擎是 legal_drafter 的通用化底座**；legal_drafter v.next 改为消费本引擎 + 法律模板/prompt（见 §6 / §10）。

> **唯一交付物 = 本 spec**（R18：无单独 report）。改动在 doc-intel + chat_reliability + ai_annotator 既有结构上**扩展新模块**，
> 不替换 `StageLlms` / `TokenBill` / grounding validator / retry-validate 任何既有契约。

---

## 0. 目录

- [1. 目标定位](#1-目标定位)
- [2. 范围边界](#2-范围边界)
- [3. 架构数据流](#3-架构数据流)
- [4. 模块边界](#4-模块边界)
- [5. API 契约](#5-api-契约)
- [6. 扩展点 / 插件接口](#6-扩展点--插件接口)
- [7. 错误 + 边界 case](#7-错误--边界-case)
- [8. 成本契约](#8-成本契约)
- [9. 测试矩阵](#9-测试矩阵)
- [10. 向后兼容](#10-向后兼容)
- [11. 风险登记](#11-风险登记)

---

## 1. 目标定位

### 1.1 用户痛点（从用户动词推导）

attune 现在能**读懂、检索、抽取、批注、对比、摘要**知识库，但用户的高频诉求 ——「**帮我把这些写出来**」—— 全产品几乎空白：

| 用户动词 | 现状 | 痛点 |
|---|---|---|
| **起草** | 无（pro legal_drafter 仅法律文书） | 用户已有大纲 + KB 素材，仍要手动一段段敲论文/文档/邮件/报告 |
| **改写 / 润色** | 无 | 想调语气（正式↔口语）、长度（精简↔扩写）、受众（专家↔小白），全靠手改 |
| **大纲 / 结构** | 无（chapters 只能从**已有**文档**提**结构，不能从主题**生**大纲） | 写之前列不出大纲；写完想反向核对结构无工具 |
| **引用管理** | 部分（chat/chapters 答案带 item_id 引用 offset） | 写作时无法「从 KB 选源 → 插入 citation → 出参考文献表」 |
| **文献综述 / 多文档综合** | 部分（doc-intel `compare` 两文档 diff / `deep_summary` 单文档摘要） | 无「跨 N 文档 + KB 多源 → 结构化综述」 |

**核心命题**：把已 ship 的「读/抽/检索/摘要」**接成「写」的输入**，新增一条 **grounded narrative 生成链** ——
任何用户（不限行业）都能从大纲 + KB 素材生成**可回指源、不编事实、成本可见、可迭代**的草稿。

### 1.2 产品定位对齐

- **降 token + 数据安全（非全本地 AI）**：生成走 💰 第三层 LLM（云端为主），但**素材选取/裁剪/缓存全走已 ship 的省 token 三杠杆**（extractive 预裁 + `chunk_summaries` 复用 + cheap-MAP），把喂给生成模型的 token 压到最小。
- **混合智能 / 本地优先**：大纲生成的**结构骨架**、引用的**格式化（GB/T 7714 / APA / IEEE / MLA）**、术语一致性**检测** = 🆓/⚡ 本地零或低 LLM；仅**自由文本 narrative 生成**升级到 💰。
- **成本感知三层 + 永不偷跑**：生成 = 💰 第三层，**必须用户显式触发**（点「生成草稿」/敲回车），建库/分析阶段**绝不后台预生成**（§8）。
- **OSS 通用 vs pro 行业**：通用写作引擎在 **OSS**（任何人写文档/邮件/笔记/论文）；行业起草（法律文书 / 专利权利要求 / 标书）在 **pro**，复用 OSS 引擎 + 行业模板/prompt/红线（§6 / §10）。

### 1.3 北极星三问自检（per `~/.claude/CLAUDE.md §2.3.8`）

1. **服务北极星吗？** ✅ 直补「写论文 / 写文档效率」—— attune 定位「私有 AI 知识伙伴」的「伙伴」一直缺「替我写初稿」这一面。
2. **在追学术指标牺牲产品吗？** ❌ 不追「生成流畅度」类无锚指标；质量 gate 锚在 **grounding（可回指源）+ 事实一致（不编）**，与产品「数据安全 + 可信」一致。
3. **偏离硬件约束吗？** ❌ 生成走云端 token（§4.5H deepseek-v4 文本默认），本地仅跑骨架/格式化，不增本地 GPU 负载。

---

## 2. 范围边界

### 2.1 做（v1.x 首发 scope）

| # | 能力 | 输出模式 | 成本层 |
|---|---|---|---|
| W1 | **起草（draft）**：从大纲 / KB 选材 / 抽取产物 → 生成草稿段落（论文段落 / 文档 / 邮件 / 报告） | narrative（默认）+ structured（分段 + 每段 grounding） | 💰 |
| W2 | **改写 / 润色（rewrite）**：对选定文本调 语气 / 长度 / 受众 / 风格；保事实不漂 | narrative（整段）/ 批阅（review，逐句改写建议**带 offset**） | 💰 |
| W3 | **大纲（outline）**：① 正向 主题/素材 → 大纲；② 反向 草稿 → 提结构 | structured（树形大纲节点） | 正向 💰 / 反向复用 chapters（⚡ list 零 LLM） |
| W4 | **引用管理（cite）**：从 KB 选源（vault RAG）→ 插入 citation 锚 → 生成参考文献表（4 样式） | structured（citation 表 + 文中锚 offset） | 选源 ⚡（search）/ 格式化 🆓（regex）/ — |
| W5 | **文献综述 / 多文档综合（synthesis）**：跨 N 文档 + KB 多源 → 结构化综述 | narrative + structured（按主题分节 + 每节 grounding） | 💰（复用 deep_summary map-reduce + compare） |
| W6 | **模板 / 格式 + 术语一致（通用层）**：用户模板套用（占位填充）+ 全文术语一致性检测/统一 | structured（模板填充结果 + 术语差异表） | 模板填充 🆓 / 术语检测 ⚡（embedding 近邻）|

**贯穿全 W1–W6 的硬契约**：
- **grounding**：每个生成片段（段落 / 综述节 / citation）必须带 `GroundingRef[]`（回指 KB item_id + 字符 offset / 外部源标识），未命中源的「事实性陈述」标 `[需核实]`（通用）/ 占位（pro 行业可重定义为 `[请律师确认]` 等）。
- **§4.5 兜底**：schema-guided 输出 + 重试-验证 ≤3 + few-shot + 弱模型 degrade + 失败 telemetry（复用 ai_annotator / doc-intel 既有栈）。
- **N=3 质量 gate**：grounding-precision ≥ floor + fact-consistency ≥ floor + 格式合规 100%（§9）。

### 2.2 不做（写死，禁 silent scope creep）

| 不做 | 归属 |
|---|---|
| 富文本 WYSIWYG 编辑器 / 实时协作 | 产品 UI v.next；本 spec 只出 API + 草稿文本，不做编辑器 |
| 多人评论 / 修订追踪（track changes 协作语义） | 不做（批注 source 是状态非协作，per CLAUDE.md） |
| 流式输出（SSE streaming） | 不做（per CLAUDE.md 产品决策：spinner 即可） |
| 行业文书生成（起诉状 / 权利要求 / 标书） | **pro**（复用本引擎 + 行业模板，§6/§10），**不在 OSS** |
| 联网现搜外部文献（PubMed / arXiv 实时抓） | v.next；本 spec 引用源限 **已入库 KB**（vault）+ 用户手填外部源 |
| 查重 / 抄袭检测算法 | v.next（§11 风险登记标注，仅做「逐句 grounding 回指」降低抄袭面，不做相似度查重） |
| 图表 / 公式自动生成（生成视觉内容） | 不做（视觉是 OCR/VLM 的**读**方向，非写方向） |
| 自动后台「猜你要写什么」/ 主动建议下一段 | **禁**（per CLAUDE.md 成本契约规则 2：分析/生成永远等用户开口） |

### 2.3 后续 v.next（明确推后，不写进首发）

- 外部文献联网检索接入引用源（W4 扩展）。
- 多模态写作（图文混排生成）。
- 查重 / 相似度（W5/W7 扩展）。

---

## 3. 架构数据流

### 3.1 总数据流（ASCII）

```
                          用户显式触发（点「生成草稿」/「改写」/「综述」）
                                          │  💰 第三层，永不后台偷跑（§8）
                                          ▼
┌──────────────────────────────────────────────────────────────────────────┐
│  attune-server/src/routes/writing.rs   POST /api/v1/writing/{draft,        │
│      rewrite,outline,cite,synthesis,terms}                                  │
│  enforce_gate(is_tier3=true(生成阶段), is_paid)  →  403 membership-required  │
│  PII redact（RedactingLlmProvider 包裹，复用既有）                            │
└──────────────────────────────────────────────────────────────────────────┘
                                          ▼
┌──────────────────────────────────────────────────────────────────────────┐
│  attune-core/src/writing/  (新增模块)                                        │
│                                                                             │
│  ① 选材 / 裁剪 (🆓/⚡ 省 token，复用已 ship)                                  │
│     W1/W5  search::search_with_context (vault RAG)                          │
│            + extractive::extract (零 LLM 预裁) + chunk_summaries 缓存复用     │
│     W3反向 / W5  document_intelligence::{chapters,deep_summary,compare}     │
│  ② 生成 (💰 LLM，schema-guided + retry≤3 + few-shot)                         │
│     StageLlms{cheap,reasoning} via ModelRouter  (cheap=节选改写/cheap-MAP,   │
│            reasoning=narrative 总成/综述 REDUCE)                             │
│  ③ grounding validator (确定性，复用 chat_reliability::evaluate_response)     │
│     每生成片段 → GroundingRef[] token-overlap 校验；未命中 → 标 [需核实]/占位   │
│     ④ 后端自定位 offset (复用 ai_annotator 3 段定位：verbatim→relaxed→prefix) │
│  ⑤ TokenBill 记账 (naive 基线 vs map+reduce 实际) → 挂每个 response 信封      │
└──────────────────────────────────────────────────────────────────────────┘
                                          ▼
        WritingResult{ content / segments[] / grounding_refs[] /
                       citations[] / annotations[](offset) / token_bill /
                       unverified_spans[] }  →  spinner UI 渲染
```

### 3.2 模式 × 复用栈映射

| 能力 | 选材（输入） | 生成 | grounding | offset | 省 token |
|---|---|---|---|---|---|
| W1 draft | outline + search_with_context + 抽取产物 | reasoning narrative | ✅ 段级 | structured 模式给段 offset | extractive 预裁素材 |
| W2 rewrite | 用户选区文本（UTF-16 选区，复用 ai_annotator 选区契约） | cheap（短）/reasoning（重写） | ✅ 保事实校验：改写后 vs 原文事实集一致 | 批阅模式逐句 offset | 仅喂选区 |
| W3 outline 正向 | 主题 + 可选 KB 素材摘要 | reasoning（出树） | 节点可选挂源 | — | 喂 chunk_summaries 非全文 |
| W3 outline 反向 | 草稿全文 | chapters::list（⚡ 零 LLM） | — | 章 offset | 零 LLM |
| W4 cite | search（选源）+ regex 格式化 | 无 LLM（W4 主体零 LLM） | citation = 强 grounding（必命中源） | 文中锚 offset | — |
| W5 synthesis | N 文档 + vault RAG（多源） | map(cheap/源)→reduce(reasoning/节) | ✅ 节级多源 | 节 offset | deep_summary map-reduce + 缓存 |
| W6 terms | 全文 + embedding 近邻 | 模板填充零 LLM；术语统一可选 cheap | — | 术语命中 offset | ⚡ embedding |

### 3.3 DB / cache

- **复用 `chunk_summaries`**（`chunk_hash` + strategy key）：W1/W5 的素材摘要走既有缓存，不新建表。
- **新增缓存 key 命名空间**：`writing:<mode>:<input_hash>`（草稿可迭代 —— 同输入重生成命中缓存，避免重复计费；用户改 prompt 则 key 变、重算）。
- **新增 `writing_drafts` 表（可选，v1.x 评估）**：`{id, item_id?, mode, content_enc(AES-256-GCM), grounding_json_enc, token_bill_json, created_at}` —— 草稿持久化 + 迭代历史；遵字段级加密（per 项目加密模型）。**首发可先内存态不落库**，落库作切片决策。
- **不新增向量索引 / 不改 search schema**。

---

## 4. 模块边界

### 4.1 涉及 crate / module / file

| 层 | 文件 | 新增 / 改 |
|---|---|---|
| core 引擎 | `rust/crates/attune-core/src/writing/mod.rs` | 🆕 模块根：`WritingMode` 枚举 + `WritingResult` + `GroundingRef` + `Citation` |
| core | `…/writing/draft.rs` | 🆕 W1 |
| core | `…/writing/rewrite.rs` | 🆕 W2（消费 ai_annotator 选区 + offset 定位栈） |
| core | `…/writing/outline.rs` | 🆕 W3（反向复用 `document_intelligence::chapters`） |
| core | `…/writing/cite.rs` | 🆕 W4（格式化 = 纯 regex，复用 academic-pro `citation_format` 思路但 OSS 自带通用版） |
| core | `…/writing/synthesis.rs` | 🆕 W5（复用 `deep_summary` map-reduce + `compare`） |
| core | `…/writing/terms.rs` | 🆕 W6（模板填充 + 术语一致 embedding 近邻） |
| core | `…/writing/grounding.rs` | 🆕 复用 `chat_reliability::evaluate_response` 适配生成片段 |
| core | `…/writing/templates.rs` | 🆕 模板 trait `WritingTemplate`（OSS 通用模板 + pro 行业实现扩展点，§6） |
| core 复用 | `chat_reliability/agent.rs` / `ai_annotator.rs` / `document_intelligence/*` / `search.rs` | ♻️ 调用，不改契约 |
| server 路由 | `rust/crates/attune-server/src/routes/writing.rs` | 🆕 6 endpoint + `enforce_gate` 复用 |
| server 注册 | `attune-server/src/routes/mod.rs` / app router | ✏️ 挂载 `/api/v1/writing/*` |
| UI | `attune-server/ui/src/**`（WritingView） | 🆕 v.next（本 spec 出 API；UI 走 i18n 守卫 per CLAUDE.md） |

### 4.2 跨仓边界（硬约束）

- **OSS attune** 提供 `writing` 引擎 + `WritingTemplate` trait + 通用模板（论文段 / 邮件 / 报告 / 笔记 / 通用文书）。
- **attune-pro** 各 vertical 通过 plugin 提供**行业模板 + 行业 prompt + 行业红线**，**消费** OSS `writing` 引擎（不自带生成栈，类比 ADR-0008 视觉共享核心）。`legal_drafter` v.next 重构为「OSS writing engine + 法律模板（起诉状/合同/律师函）+ `no_hallucinated_citation` 红线」。
- attune **不依赖 attune-pro / attune-enterprise**：OSS writing 必须在无 pro 环境完整可用（通用模板齐全）。

---

## 5. API 契约

前缀 `/api/v1/writing/`（kebab-case per OPT-5）。生成阶段全部 tier-3 member-gated（`enforce_gate`）。

### 5.1 共享类型

```jsonc
// GroundingRef — 每个生成片段的源回指（grounding 一等公民）
{ "kind": "kb_item" | "external" | "user_input",
  "item_id": "uuid|null",          // KB 源
  "source_offset": [start, end],    // 源内字符 offset（CJK 安全）
  "external_ref": "string|null",    // 用户手填外部源标识（DOI/URL/书目）
  "overlap_tokens": 7 }             // grounding 校验命中 token 数（复用 chat_reliability）

// WritingResult 信封（所有 endpoint 共用）
{ "content": "string",              // narrative 文本
  "segments": [                     // structured 模式：分段 + 每段 grounding
    { "text": "...", "offset": [s,e], "grounding": [GroundingRef], "verified": true }
  ],
  "citations": [Citation],          // W4：参考文献条目
  "annotations": [                  // 批阅模式：逐句改写建议带 offset
    { "offset": [s,e], "suggestion": "...", "reason": "..." }
  ],
  "unverified_spans": [[s,e]],      // 标 [需核实]/占位 的片段 offset
  "token_bill": TokenBill }         // 复用既有
```

### 5.2 Endpoints

| Method/Path | body 关键字段 | 返回 | output_modes |
|---|---|---|---|
| `POST /draft` | `{outline?, item_ids[], extracted?, audience?, tone?, length?, template_id?}` | WritingResult | narrative(默认) / structured |
| `POST /rewrite` | `{text, selection_offset?, target:{tone?,length?,audience?,style?}, preserve_facts:true}` | WritingResult | narrative / review(批阅，带 offset) |
| `POST /outline` | `{topic?, item_ids[], from_draft?(反向)}` | `{nodes:[{title,children[],source_ref?}]}` | structured |
| `POST /cite` | `{query|item_ids[], style:"gbt7714"\|"apa"\|"ieee"\|"mla", external_refs[]?}` | `{citations:[Citation], inline_anchors:[{offset,citation_id}]}` | structured |
| `POST /synthesis` | `{item_ids[]\|query, structure?:"thematic"\|"chronological", max_sources?}` | WritingResult（按节 + 每节多源 grounding） | narrative / structured |
| `POST /terms` | `{text, template_id?(模板填充), glossary?(术语表), action:"fill"\|"check"\|"unify"}` | `{filled?, term_diffs:[{term,variants[],offsets[]}]}` | structured |

**错误返回**（统一 `AppError` shape `{error,code}`，复用既有）：见 §7。

### 5.3 CLI（可选，对齐既有 doc-intel CLI 风格）

`attune writing draft --outline <f> --items <ids> [--tone formal --length short]` 等子命令；首发可后置，API 优先。

---

## 6. 扩展点 / 插件接口

### 6.1 `WritingTemplate` trait（OSS 定义，pro 实现）

```rust
pub trait WritingTemplate: Send + Sync {
    fn id(&self) -> &str;                       // "academic_paragraph" / "legal_complaint"(pro)
    fn system_prompt(&self) -> &str;            // 行业/通用 system prompt
    fn few_shot(&self) -> &[WorkedExample];     // ≥2 例（§4.5C）
    fn red_lines(&self) -> &[RedLine];          // 通用: [需核实]; pro: no_hallucinated_citation
    fn placeholder_marker(&self) -> &str;       // 通用 "[需核实]"; 法律 "[请律师确认]"
    fn citation_styles(&self) -> &[CiteStyle];  // 该模板默认引用样式
}
```

- **OSS 内置模板**：`academic_paragraph` / `email` / `report` / `note` / `general_doc`（任何用户可用）。
- **pro 扩展**：law `legal_complaint`/`contract`/`lawyer_letter`、patent `claim_drafting`、presales `bid_proposal` —— 各 plugin 注册自己的 `WritingTemplate`，复用 OSS draft/rewrite/cite/grounding 全链。
- **新增模板 = 实现 trait + 在 plugin.yaml 声明**，不改引擎核心（开闭原则）。

### 6.2 引用样式扩展点

`CiteStyle` trait（GB/T 7714 / APA / IEEE / MLA 内置）→ 新样式实现 trait 注册，纯 regex/格式化零 LLM。

---

## 7. 错误 + 边界 case

| 场景 | code（kebab） | HTTP | 行为（graceful degradation） |
|---|---|---|---|
| 未登录/非会员调生成 | `membership-required` | 403 | 复用 `enforce_gate`，引导升级（per 无支付会员模型：优雅引导页） |
| 空 outline + 空 item_ids（draft 无素材） | `no-source-material` | 400 | 拒绝，提示先选素材/写大纲 |
| item_id 不存在 / vault locked | `item-not-found` / `vault-locked` | 404 / 401 | 复用 `AppError::From<VaultError>` |
| LLM 3 次重试后 schema 仍非法 | `generation-unavailable` | 503 | 不 panic；返回 telemetry（agent×model 失败率），UI 提示切高 tier（§4.5F） |
| grounding 全片段未命中源（疑似全幻觉） | （非 error，软信号） | 200 | 返回内容但 `unverified_spans` 覆盖全文 + UI 红色警示「未能回指任何源，请核实」 |
| 改写后事实集与原文不一致（fact drift） | （软信号） | 200 | 标 drift 片段进 `unverified_spans`，不静默接受（per §11 风险 A） |
| 超长输入（>模型上下文） | — | 200 | 走 deep_summary map-reduce 折叠 + extractive 预裁，不 413 |
| 空文本 rewrite / 0 节点 outline | `empty-input` | 400 | 短路拒绝 |
| Unicode / emoji / CJK / 繁简混排 | — | 200 | offset 用 UTF-16 code unit（复用 ai_annotator 契约）；繁简 normalize（吸取 self_evolving_skill 踩坑） |
| 引用样式不支持 | `unsupported-cite-style` | 400 | 列出支持样式 |

**红线（exit/拒绝级）**：
- `no_hallucinated_citation`（通用版 = 文中 citation 必须命中真实 KB item 或用户手填 external_ref，**禁止 LLM 编造书目**）；pro 行业模板可叠加更严红线。

---

## 8. 成本契约

| 能力 | 层级 | 触发 | UI 显示 |
|---|---|---|---|
| W3 反向大纲 / W4 引用格式化 / W6 模板填充 | 🆓 零成本 | 随便跑 | `~本地 · 即时` |
| W4 选源（search）/ W6 术语近邻 | ⚡ 本地算力 | 随建库可跑 | `~本地 · 秒级` |
| W1 draft / W2 rewrite / W3 正向 / W5 synthesis | 💰 时间/金钱 | **必须用户显式触发**（点按钮/敲回车），**永不后台偷跑** | 生成按钮旁常驻 `~N.NK tok · $0.00NN`（云端）/ `~本地 · Ns`；点开展开所选素材 |

**硬规则（per CLAUDE.md 成本契约）**：
1. **建库 / ingest / 文件夹监听阶段绝不触发任何写作生成**（W1–W6 的 💰 部分）—— 只在用户主动点「生成」时跑。
2. **草稿可迭代但每次 LLM call 显示成本**：同输入重生成命中 `writing:<mode>:<hash>` 缓存（不重复计费）；改 prompt → 重算 → 新计费，UI 明示。
3. **TokenBill 挂每个 response**：`savings_ratio_by_token`（省 token 三杠杆生效证据）+ `_by_usd`，可测（复用既有，无 secret 字段，sentinel 守卫）。
4. **省 token 三杠杆默认全开**：W1/W5 喂模型的素材先 extractive 预裁 + chunk_summaries 缓存 + cheap-MAP，把生成输入 token 压到最小。

---

## 9. 测试矩阵

> 生成类怎么测：**不测「写得好不好」（主观），测 grounding（可回指）+ fact-consistency（不编）+ 格式合规（确定性）+ N=3 稳定性**。
> 遵 Agent 验证铁律 6 类下限 + §4.5 + real-LLM N=3 F1 gate（写作引擎引入新 agent，必走 `agent_golden_gate` 等价 harness，从 attune-pro 复制到 attune-core per CLAUDE.md）。

### 9.1 6 类下限（per CLAUDE.md Agent 验证铁律）

| 类型 | 下限 | 写作引擎落地 |
|---|---|---|
| Golden case | ≥10 真实 + 1 sentinel / 每能力（W1/W2/W5 各一套） | `tests/golden/writing_<mode>/cases.yaml`，**GT 人工标注**（grounding span + 必含/禁含事实），**禁 LLM 生成 GT** |
| 属性测试 | ≥3 per 能力 | proptest：① 输出 grounding span 必落在源 offset 界内（不越界，u64 防溢出）；② 改写保 `unverified_spans` 单调（改写不该凭空新增未核实事实）；③ citation 数 = inline 锚数 |
| 边界 case | ≥5 `#[test]` | 空素材 / 超长 / 0 节点 / CJK-emoji / 繁简混排 / 模板缺占位 |
| 异常 / 错误 | ≥3 | schema 3 次失败 → 503；vault locked → 401；非会员 → 403 |
| 集成 E2E | ≥1 subprocess | `tests/writing_subprocess.rs` 真起 server 走 /draft + /cite |
| 回归 fixture | 每修 1 bug 加 1 | 永久进 golden |

### 9.2 生成质量 gate（N=3 real-LLM，ratchet 只升不降）

| 指标 | 定义 | floor（首发，达标后 ratchet） | 怎么测 |
|---|---|---|---|
| **grounding-precision** | 生成事实性片段中「真能回指源」占比（复用 chat_reliability token-overlap 判定） | ≥ **0.90** | golden GT 标注每片段应命中的 item_id+offset；N=3 seed mean±std |
| **fact-consistency**（不编） | 生成内容 vs 源「事实集」无矛盾/无新增未授权事实 | ≥ **0.85** | GT 列「必含事实 / 禁含臆造事实」，micro-F1 |
| **format-compliance** | citation 4 样式格式 100% 合规；模板占位 100% 填对位 | **1.00**（确定性） | regex 校验，无 LLM |
| **rewrite-fact-preservation** | 改写前后事实集一致率 | ≥ **0.90** | 改写后事实集 ⊇ 原文事实集（不丢不编），N=3 |

**3-tier 兼容矩阵**（§4.5D，ship 前必跑，10+ case）：
- 弱本地 qwen2.5:3b / 弱云 deepseek-v4-flash（默认，§4.5H）/ 强云 deepseek-v4-pro（上限对照）。
- 三 tier F1 差 ≤ 0.15 → 「all-model OK」；> 0.15 → RELEASE.md 标最低 tier（`Requires ≥ deepseek-v4-flash` 等）。
- 弱模型 F1 < floor → 该能力本地自动 disable + RELEASE.md 警示（§4.5E）。

### 9.3 evidence 落档（§6.3）

- real-LLM gate 默认 `#[ignore]`（opt-in nightly + 成本授权），但 **run-log 必落 `reports/runs/<ts>/`**（吸取 doc-intel/vision「gate 代码齐但 run-log 未落档 → claim 待确认」教训，本 spec 实施时硬性落档）。
- 任何 F1 claim 引 `reports/runs/<ts>/<file>:<line>`，无源标 PENDING-VERIFY。

### 9.4 场景覆盖（黑盒 6 类 per §6.1）

happy（论文段/邮件/综述生成）· edge（超长/空源/CJK）· error（非会员/locked/schema fail）· **adversarial（prompt injection：素材里藏「忽略指令编造引用」→ 必须 source_has_injection_instruction 确定性拒绝，复用 pro_infra guard 思路）** · 并发（多用户同时生成）· 资源耗尽（quota / token 超限优雅降级）· 国际化（中英/繁简/混排）· 降级（LLM 不可用 → 返回骨架 + telemetry）。

---

## 10. 向后兼容

- **纯增量**：新 `/api/v1/writing/*` 端点 + 新 `writing` 模块，**不改** doc-intel / chat / search / ai_annotator 既有 API/schema/契约。老 client 完全不受影响（新端点老 client 不调）。
- **schema versioning**：`WritingResult` 带 `schema_version:1`；`GroundingRef.kind` 用枚举字符串便于演进。
- **pro `legal_drafter` migration path**：
  - 阶段 1（本 spec 不动 pro）：OSS writing 引擎独立 ship，legal_drafter 维持现状。
  - 阶段 2（pro 仓 v.next，独立 spec）：legal_drafter 重构为消费 OSS writing 引擎 + 法律 `WritingTemplate`；**外部契约（plugin.yaml agent id / output_modes / 红线 / chat_trigger）不变**，仅内部实现切底座 → 对用户/调用方透明。
  - 双轨期：旧 legal_drafter binary 与新引擎并存 ≥1 release，golden F1 不回退才切换。
- **DB**：若落 `writing_drafts` 表，走 migration（per 项目 migration 规范）；首发若内存态则无 DB 变更。
- **缓存**：新 `writing:*` key 命名空间独立，不冲突既有 `chunk_summaries` / `deepsum:` key。

---

## 11. 风险登记

| # | 风险 | 等级 | 缓解 |
|---|---|---|---|
| A | **幻觉（编事实/编引用）** —— 生成类最大风险，法律/学术场景致命 | 🔴 高 | grounding validator 硬校验（复用 chat_reliability）；未命中源标 `[需核实]`/占位不静默输出；`no_hallucinated_citation` 红线（citation 必命中真实源）；fact-consistency N=3 gate ≥0.85；改写 fact-preservation ≥0.90 |
| B | **抄袭面**（生成大段照搬源 → 用户当原创） | 🟡 中 | 逐句 grounding 回指（用户看得到「这句来自源 X」自行判断引用义务）；W4 强制 citation；**首发不做相似度查重（v.next §2.2）**，RELEASE.md 明示「写作引擎产出含源回指，用户须自负引用合规」 |
| C | **成本失控**（用户反复重生成烧 token） | 🟡 中 | 💰 必显式触发 + 永不偷跑；同输入命中缓存不重复计费；TokenBill 常驻显示；省 token 三杠杆默认全开；quota 超限优雅降级（§7） |
| D | **prompt injection**（KB 素材里藏注入指令操纵生成/伪造引用） | 🔴 高 | 调模型前 `source_has_injection_instruction()` 确定性拒绝（复用 pro_infra guard 思路）；adversarial holdout 哨兵 N=3 零泄漏（§9.4） |
| E | **弱模型生成质量塌**（qwen2.5:3b narrative 散/grounding 烂） | 🟡 中 | 3-tier 矩阵 ship 前必跑；弱模型 < floor → 本地 disable + RELEASE.md 标最低 tier（§4.5E）；schema-guided + retry≤3 + few-shot 兜底 |
| F | **grounding 误判**（token-overlap 把「改写措辞」误判未命中，或把巧合 overlap 误判命中） | 🟡 中 | 复用 chat_reliability 既有阈值（`min_grounding_overlap_tokens`）；golden GT 含「语义等价但措辞变」case 校准；下调阈值需 ratchet 评审（吸取 defamation_extractor GT span 歧义教训） |
| G | **scope creep**（被要求加编辑器/协作/查重） | 🟢 低 | §2.2 写死不做；超 scope = 新 spec |
| H | **OSS/pro 边界泄漏**（行业模板/红线漏进 OSS） | 🟡 中 | OSS 仅通用模板（论文/邮件/报告/笔记/通用文书）；行业 `WritingTemplate` 全在 pro plugin；per OSS 边界规则 §4.3 + 历史「行业回灌」踩坑，PR review 必查 |
| I | **并发**（多用户同时生成竞争 LLM/锁） | 🟢 低 | writing 模块无共享可变状态；遵 lock ordering（fulltext→vectors→vault）；生成走 provider clone Arc，不持长锁 |

---

> **wiki 同步提示**：本能力 ship 后同步刷新 `docs/wiki/agents.md` + 新增 `docs/wiki/writing-engine.md`（per CLAUDE.md「wiki 自动同步 docs」），并把 `docs/agent-capabilities.md` 加「写作引擎」节（补 §8.6 观察的「撰写类能力是空白增长点」）。

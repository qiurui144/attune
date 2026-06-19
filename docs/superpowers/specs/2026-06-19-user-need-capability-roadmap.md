# Spec: 用户需求驱动的能力路线图（assistant / writing / info-retrieval；extract→generate→retrieve→proactive 四象限缺口）

> Status: **DRAFT — 战略路线图，先评审后切片实现**
> Date: 2026-06-19
> Author: spec-analyst（AI 起草）
> 范围：**OSS attune + attune-pro 双线的能力地图**（本 spec 是"该有什么"的反推；具体 feature 各自起 §3.1 spec）
> 接地基线：develop（OSS `fe3b791`；pro `6db2528`），引用代码/已存 spec 真实路径（§6.3）
> 性质：本 spec **不是补现有洞的 gap-list**，是**从用户动词反推 attune 该有的完整能力全集**，再把"现状 vs 全集"的差额排成路线图。

---

## 0. 目录

- [1. 目标定位：从三北极星动词推全集](#1-目标定位从三北极星动词推全集)
- [2. 范围边界](#2-范围边界)
- [3. 完整能力树（北极星动词 → 能力）](#3-完整能力树北极星动词--能力)
- [4. 现状对照 map（已有强/弱 / 部分 / 空白）](#4-现状对照-map)
- [5. attune 的结构性偏：强抽取 / 弱生成 / 弱获取 / 弱主动](#5-attune-的结构性偏)
- [6. 优先级路线图（北极星价值 × 缺口 × 工作量）](#6-优先级路线图)
- [7. 前 3 个新方向的 spec 大纲](#7-前-3-个新方向的-spec-大纲)
- [8. 成本契约 + OSS/pro 边界守则](#8-成本契约--ossp ro-边界守则)
- [9. 风险登记](#9-风险登记)

---

## 1. 目标定位：从三北极星动词推全集

用户原话四个横切价值词 + 三个使用场景：

- **横切价值（贯穿所有能力）**：**精准 · 可靠 · 省时间 · 高效率**
- **场景 1 — 忙碌时的助理**：帮我分清轻重缓急、按需取要点、快问快答、别让我自己想起来
- **场景 2 — 写论文 / 文档效率**：帮我起草、改写、综述、对齐格式与术语
- **场景 3 — 关注内容的准确信息获取**：盯住我关心的源、给我准确且有出处的答案、有新信息提醒我

**方法**：把每个场景拆成**用户动词**（triage / 摘要 / 起草 / 综述 / 监控 / 核实 …），每个动词反推出"要做到它，attune 必须具备的能力原子"。横切四价值词转成**每条能力的验收维度**（精准=grounding/F1；可靠=§4.5 兜底+§6.1 六类；省时间=token-bill/缓存/零成本预裁；高效率=端到端无手工拼接 / 主动呈现）。

**北极星对齐**（per 产品定位 spec §1-§2 三大支柱）：主动进化 / 对话伙伴 / 混合智能。本路线图的"主动"系列直接服务支柱 1，"写作/获取"服务支柱 2+3。

---

## 2. 范围边界

**做（本 spec 产出）**：
- 三场景完整能力树（动词 → 能力原子），含 OSS/pro 归属预判
- 能力树 × 现状的覆盖 map（强/弱/部分/空白 四态）
- 结构性偏诊断 + top-N 优先级路线图
- 前 3 新方向（写作 / 信息监控 / 主动助理）的 spec 大纲（供后续起正式 §3.1 spec）

**不做（各自起独立 spec 才实现）**：
- 不在本 spec 写任何实现代码 / plan
- 不重复已存在的 spec：**主动建议引擎 + 第三方账号**已有 [`2026-06-17-suggestions-and-thirdparty-accounts.md`](2026-06-17-suggestions-and-thirdparty-accounts.md)（DRAFT）；**多层记忆**有 [`2026-06-17-multilayer-memory-architecture.md`](2026-06-17-multilayer-memory-architecture.md)；**自动整理案卷**有 [`2026-06-15-auto-organize-folder-to-project.md`](2026-06-15-auto-organize-folder-to-project.md)；**vision 增强**有 [`2026-06-16-vision-understanding-enhancement.md`](2026-06-16-vision-understanding-enhancement.md)。本 spec **引用并定位**它们，不复制。
- 不决定定价 / 改名（沿用产品定位 spec §10 开放议题）

**后续 v.next（写死，防 scope creep）**：agent-spawns-agent 自主编排（flow.rs §2.2 已 defer 到 v2.x）；多模态写作（图文混排生成）。

---

## 3. 完整能力树（北极星动词 → 能力）

> 图例：能力原子后标 **[OSS]** / **[PRO]** 归属预判 + 形态（base 能力 / 新 agent / 扩现有）。
> 输出模式 per agent-output-mode-contract：结构化 / 标记(offset) / 批阅 / 叙述。

```
attune 能力全集
│
├── A. 助理（忙碌）── 动词：分清 / 取要点 / 快答 / 提醒 / 建议 / 批处理
│   ├─ A1 triage 优先级排序（收件箱式：哪些 item 今天该看）       [OSS] 新 base 能力（确定性信号→排序）
│   ├─ A2 按需摘要（选中即出 brief，缓存复用）                     [OSS] 已有=doc-intel deep_summary
│   ├─ A3 快问快答（personal KB RAG，单轮即答）                    [OSS] 已有=chat/RAG
│   ├─ A4 提醒 / 日程感知（截止日 / 跟进项的被动提示）              [OSS] 空白（无 reminder/calendar）
│   ├─ A5 被动建议卡（零成本，绝不偷跑 LLM，点击才升级）            [OSS] 规划中=suggestions spec
│   ├─ A6 批量处理（对一组 item 跑同一动作：批摘要/批标注/批分类）  [OSS] 部分（单 item 链全，批处理编排薄）
│   └─ A7 跨会话连续性（记得上次问到哪）                           [OSS] 已有=memory L0-L3
│
├── B. 写作（论文 / 文档）── 动词：起草 / 改写 / 润色 / 列大纲 / 管引用 / 综述 / 多文档综合 / 套格式 / 统一术语
│   ├─ B1 起草（从大纲/要点/检索结果生成初稿，grounded 带引用）    [OSS] 空白（narrative 生成）★
│   ├─ B2 改写 / 润色 / 扩写 / 缩写（保义改风格/长度）             [OSS] 空白 ★
│   ├─ B3 大纲生成 + 大纲→正文双向                                [OSS] 空白
│   ├─ B4 引用管理（插入/格式化/去重；OSS 通用，pro 行业样式）     [OSS]base + [PRO]academic citation_format 已有
│   ├─ B5 文献综述 / 多文档综合（N 篇→一篇主题综述，跨文档 grounding）[OSS] 空白（doc-intel 仅文档内）★
│   ├─ B6 模板 / 格式套用（论文章节模板 / 文书模板）               [OSS]通用模板 + [PRO]legal_drafter 已有
│   └─ B7 术语一致性（全文术语统一 / 中英对齐）                     [OSS]base + [PRO]term_translation 已有
│
└── C. 信息获取（关注内容）── 动词：监控 / 订阅 / 准确答 / 去重 / 提醒新信息 / 深研综合 / 跨源核实
    ├─ C1 源监控 / 订阅（RSS/Email/Git/云盘/WebDAV 持续 poll）     [OSS] 已有底座=ingest connectors + scheduler
    ├─ C2 新信息提醒（新 entry → 被动卡 / 摘要 digest）            [OSS] 空白（采集有，digest/notify 无）★
    ├─ C3 源-grounded 准确问答（答案锚到源 + 出处必现）            [OSS] 已有=chat/RAG（出处必现是定位铁律）
    ├─ C4 去重（跨源同主题内容折叠）                               [OSS] 部分（content_hash 入口去重，语义跨源去重无）
    ├─ C5 深度研究综合（多源检索→对抗核实→带引用综合报告）         [OSS] 空白（与 B5 共底座）★
    ├─ C6 跨源核实 / 矛盾检测（同事实多源是否一致）                [OSS] 空白（doc-intel compare 仅两文档）
    └─ C7 混合智能（本地无命中→浏览器网络搜索补全）                [OSS] 已有=BrowserSearchProvider
```

横切四价值词如何落到每条（验收维度，§6.1）：
- **精准** → grounding 校验 + holdout F1 ≥ floor（生成类 floor 0.85 / 抽取 0.90）。
- **可靠** → §4.5 A/B/C/E 全装（schema-guided + 重试-验证 + few-shot + 弱模型降级）+ 六类测试下限。
- **省时间** → token-bill 可测 + chunk_summaries 缓存 + extractive 零 LLM 预裁；deep_summary 三杠杆是模板。
- **高效率** → 端到端无手工拼接（flow DAG / 批处理）+ 主动呈现（建议卡 / digest）。

---

## 4. 现状对照 map

> 四态：**强**(有真 LLM gate + 测试齐) / **弱**(有但 gate 弱/未暴露) / **部分**(底座有缺串联或 UI) / **空白**(无实现)。接地引用见括号。

| 能力 | 状态 | 接地 | 缺什么 |
|------|------|------|--------|
| A1 triage 排序 | **空白** | — | 信号底座有（`store/signals.rs`），无"优先级排序→今日清单"层 |
| A2 按需摘要 | **强** | doc-intel `deep_summary`（三杠杆+floor 0.80 gate） | — |
| A3 快问快答 | **强** | `chat.rs`+`search.rs` RAG | — |
| A4 提醒/日程 | **空白** | 无 reminder/calendar route（已核 `routes/` 无） | 全新 |
| A5 建议卡 | **部分(规划中)** | suggestions spec DRAFT；信号底座有 | 规则层+卡片模型+UI（spec 已规划，未实现） |
| A6 批处理 | **部分** | 单 item 链全（upload/annotate/classify）；`agents/flow.rs` DAG 有 | 批量编排 + 进度 UI + 成本批显示 |
| A7 跨会话连续 | **强** | memory L0-L3（`memory/`，L3 已实装 #85） | — |
| B1 起草 | **空白** ★ | 仅 pro `legal_drafter`（行业） | OSS 通用 narrative 起草 agent 不存在 |
| B2 改写/润色 | **空白** ★ | — | 全新（产品全栈最大 narrative 空白） |
| B3 大纲 | **空白** | doc-intel `chapters` 是"读章节"非"写章节" | 全新 |
| B4 引用管理 | **部分** | pro academic `citation_format`（识别）；OSS 无插入/管理 | OSS 通用引用插入/去重层 |
| B5 文献综述/多文档综合 | **空白** ★ | doc-intel `deep_summary` 仅**单文档**；vault-wide RAG 未接进综合 | 跨文档检索→综合 narrative |
| B6 模板格式 | **部分** | pro `legal_drafter` 文书模板 | OSS 通用文档模板引擎 |
| B7 术语一致 | **部分** | pro `term_translation`（中英 claim）；OSS 无全文术语统一 | OSS 通用术语表+一致性检查 |
| C1 源监控订阅 | **部分(强底座)** | `ingest/{rss,git,email}.rs`+connector 框架+`agents/scheduler.rs` | poll 调度落地度 / 用户订阅管理 UI |
| C2 新信息提醒 digest | **空白** ★ | 采集入库有，无"新 entry→摘要 digest / 提醒卡" | 全新（与 A5 卡片层共享） |
| C3 源-grounded 问答 | **强** | chat/RAG + 出处必现（定位铁律 §6.3） | — |
| C4 去重 | **部分** | `content_hash` 入口精确去重 | 语义跨源去重（近似重复折叠） |
| C5 深度研究综合 | **空白** ★ | web_search + RAG 零件在，无多源对抗核实编排 | 全新（与 B5 同底座，deep-research harness） |
| C6 跨源核实 | **空白** | doc-intel `compare` 仅 A-vs-B 两文档 | 多源事实一致性 / 矛盾检测 |
| C7 混合智能 | **强** | `BrowserSearchProvider`（定位 spec §4） | — |

**计数**：强 5 / 部分 7 / 空白 8（其中 ★ 高价值空白 5：B1 B2 B5 C2 C5）。

---

## 5. attune 的结构性偏

> 一句话：**attune 是一台"理解/抽取/检索"很强、但"生成/获取主动呈现"很弱的机器。**

| 维度 | 强度 | 证据 |
|------|------|------|
| **抽取 / 理解（extract）** | 🟢 **强** | doc-intel 6 模块（compare/summary/chapters/extractive/token_bill/routing）+ vision 共享核心 + 4 ai_annotation + pro 14 个有真 F1 的抽取 agent。**全产品最密集**。 |
| **检索 / 问答（retrieve-answer）** | 🟢 **强** | RRF 混合 + rerank + RAG + memory L0-L3 + web_search 混合智能。出处必现是铁律。 |
| **生成 / 写作（generate）** | 🔴 **弱** | **OSS 零通用 narrative agent**；全产品默认 narrative 仅 pro `legal_drafter` 一个（pro catalog §8.6 自述"撰写类是空白增长点"）。deep_summary 是"压缩"非"创作"。 |
| **获取-主动呈现（retrieve-push）** | 🔴 **弱** | 采集 connector 底座强，但**没有把新信息主动推到用户面前**：无 digest、无新信息提醒、无 triage 清单。建议卡仍在 DRAFT。混合智能是"用户问才查"，非"持续盯+主动报"。 |
| **主动 / proactive** | 🟡 **半** | 唯一已落地的"信号→自动"是 skill_evolution（仅检索扩展词，不向用户暴露）；project_recommender 确定性推但不持久化。**主动是产品支柱 1 却最薄。** |

**根因诊断**：attune 历史以"个人知识库 + 记忆"立项，所有投入沉淀在"把已有内容读懂、存好、查准"。三北极星里：
- 场景 1 助理 = 强检索 + **弱主动**（缺 triage/提醒/digest）。
- 场景 2 写作 = **整段缺生成**（B1/B2/B5 全空白）。
- 场景 3 获取 = 强问答 + **弱获取闭环**（采集有但不主动报、不深研、不跨源核实）。

**战略含义**：补"生成"+"主动获取"两条腿，attune 才从"知识库工具"升级为定位 spec 承诺的"主动进化的对话伙伴"。这正是当前结构性偏对齐北极星后的最高杠杆方向。

---

## 6. 优先级路线图

> 排序键 = **北极星价值 × 现状缺口 × (1/工作量)**。每条标：归属 / 形态 / 是否需 §3.1 spec / 依赖。
> 价值/缺口/工作量 各 1-5（工作量越小越优先）。Score = 价值×缺口/工作量。

| # | 能力 | 场景 | 价值 | 缺口 | 工作量 | Score | 归属 | 形态 | 需 spec? | 依赖 / 备注 |
|---|------|------|:---:|:---:|:---:|:---:|------|------|:---:|------|
| 1 | **B1 通用起草 agent** | 写作 | 5 | 5 | 3 | 8.3 | OSS | 新 base agent（narrative，grounded+引用） | ✅ §7.1 | 复用 deep_summary 三杠杆 + RAG context；defamation_extractor 踩坑→§4.5 全装 |
| 2 | **A5 建议卡引擎落地** | 助理 | 5 | 4 | 2 | 10.0 | OSS | 扩现有（信号→规则→卡） | 已有 spec | **最高 Score**：spec 已写，底座全有，纯规则+UI，零新增 LLM |
| 3 | **B2 改写/润色 agent** | 写作 | 4 | 5 | 2 | 10.0 | OSS | 新 base agent（与 B1 共 prompt 框架） | 并入 §7.1 | 与 B1 同 spec 切片；选区→改写，offset 标记输出 |
| 4 | **C2 新信息 digest / 提醒** | 获取 | 5 | 5 | 3 | 8.3 | OSS | 扩现有（采集→digest 卡） | ✅ §7.2 | 复用 A5 卡片层 + 采集 scheduler；零成本摘要+点击深读 |
| 5 | **C1 源订阅管理 UI + 调度落地** | 获取 | 4 | 3 | 2 | 6.0 | OSS | 扩现有（connector 已有，补订阅 CRUD UI + poll 调度） | 并入 §7.2 | ingest connector + scheduler 已有 |
| 6 | **A1 triage 今日清单** | 助理 | 4 | 5 | 3 | 6.7 | OSS | 新 base 能力（确定性优先级排序） | ✅ §7.3 | 信号底座有；纯确定性可零成本；与建议卡同框 |
| 7 | **B5 文献综述 / 多文档综合** | 写作+获取 | 5 | 5 | 4 | 6.3 | OSS | 新 agent（跨文档 RAG→narrative） | ✅ §7.1 扩展 | 需先接 vault-wide RAG 进综合（OSS catalog 缺口#7） |
| 8 | **C5 深度研究综合 harness** | 获取 | 4 | 5 | 4 | 5.0 | OSS | 新 agent（多源检索+对抗核实+综合） | ✅ §7.2 扩展 | 与 B5 共底座 + web_search；deep-research 风格 |
| 9 | **A6 批处理编排** | 助理 | 4 | 3 | 3 | 4.0 | OSS | 扩现有（flow DAG 跑批） | 轻量 spec | flow.rs 有 DAG；补批量入口+进度+成本批显示 |
| 10 | **B4/B7 引用管理 + 术语一致（OSS 通用层）** | 写作 | 3 | 4 | 3 | 4.0 | OSS | 新 base 能力 | 轻量 spec | pro 有行业版；抽通用层下沉 OSS |
| 11 | C4 语义跨源去重 | 获取 | 3 | 4 | 3 | 4.0 | OSS | 扩现有（embedding 近重折叠） | 轻量 | content_hash 已有精确去重 |
| 12 | C6 跨源核实/矛盾检测 | 获取 | 4 | 5 | 5 | 4.0 | OSS | 新 agent（多源事实一致性） | ✅ spec | compare 仅两文档；扩 N 源 |
| 13 | A4 提醒/日程感知 | 助理 | 3 | 5 | 4 | 3.8 | OSS | 新能力（截止日/跟进项提示） | ✅ spec | 需日期抽取 + 提醒卡（依赖 A5） |
| — | **pro 写作扩展**（patent OA 答复 / presales 标书生成 / academic 综述）| 写作 | 高 | 高 | — | — | PRO | 各 vertical 新 agent | 各 pro spec | OSS B1/B5 落地后，pro 复用 base narrative + 行业模板 |

**路线图分期建议**（不绑死版本号，merge 时定，per §7.1.7）：
- **第一刀（最高杠杆，OSS）**：#2 建议卡 → #1+#3 起草/改写（同 spec）→ #4+#5 digest+订阅 UI。补齐"主动"+"生成"两条结构性弱腿的最小闭环。
- **第二刀**：#6 triage → #7 综述 → #8 深研。把"助理"和"获取"做成闭环。
- **第三刀**：#9-#13 长尾 + pro 写作扩展（OSS narrative 下沉后 pro 复用）。

---

## 7. 前 3 个新方向的 spec 大纲

> 仅大纲（§3.1 11 节骨架），供后续起正式 spec 时填实。三方向 = 写作 / 信息监控 / 主动助理。

### 7.1 写作引擎（B1 起草 + B2 改写 + B5 综述）

| § | 要点 |
|---|------|
| 1 目标 | 补全产品最大 narrative 空白；服务北极星支柱 2（写论文/文档效率）+ 价值"省时间/高效率" |
| 2 范围 | 做：起草(从大纲/要点/检索结果)、改写/润色/扩缩、跨文档综述。不做：图文混排(v.next)、行业文书(留 pro 复用 base) |
| 3 数据流 | `(意图+选区/大纲+vault RAG context) → §4.5 schema-guided LLM → grounded narrative + 引用 offset`；起草走"大纲→分段 MAP 生成→REDUCE 衔接"（复用 deep_summary 三杠杆反向）；综述走"跨文档检索→去重→分主题综合" |
| 4 模块 | `attune-core/src/writing/`（drafter / rewriter / synthesizer）；route `routes/writing.rs`；复用 `search::search_with_context`(vault RAG) + `document_intelligence` 缓存 |
| 5 API | POST `/api/v1/writing/{draft,rewrite,synthesize}`；tier-3 member-gated（同 documents.rs `enforce_gate`） |
| 6 扩展点 | pro vertical 注入行业模板 + 术语表（legal_drafter 迁为 base writing 的行业特化）；输出模式 narrative(默认)+marked(改写选区 offset) |
| 7 错误/边界 | 空输入/超长→分块；无 grounding 命中处留占位不编造（学 legal_drafter `[请律师确认]` 模式）；幻觉引用红线 |
| 8 成本 | 💰 第三层显式触发；token-bill 记账；改写短文(<1500 tok)零旁路同 deep_summary |
| 9 测试 | golden ≥10 + proptest ≥3 + 边界 ≥5 + 真 LLM holdout N=3 F1≥0.85（narrative 用 keypoint-recall + grounding-precision；学 doc-intel gate）+ 3-tier 矩阵 |
| 10 兼容 | 新增，无迁移；pro legal_drafter 渐进迁为 base 特化（保留旧路径一周期） |
| 11 风险 | narrative 幻觉(grounding 强制)；弱模型生成质量(§4.5.E 降级/disable)；token 成本(三杠杆+短文旁路) |

### 7.2 信息监控与获取闭环（C1 订阅 + C2 digest + C5 深研）

| § | 要点 |
|---|------|
| 1 目标 | 把"采集底座"升级为"主动盯源+提醒+深研"；服务支柱 3（关注内容准确获取）+ 价值"精准/省时间" |
| 2 范围 | 做：订阅管理 UI、poll 调度落地、新 entry→零成本 digest 卡、点击→深研综合。不做：实时 push 推送(轮询即可)、社交媒体专用抓取 |
| 3 数据流 | `connectors poll(scheduler) → 新 entry 入库 → 确定性 digest(extractive 零 LLM) → 建议卡 → 用户点击 → LLM 深研综合(多源 RAG+web_search+对抗核实) → grounded 报告` |
| 4 模块 | 复用 `ingest/{rss,email,git,...}` + `agents/scheduler.rs`；新 `attune-core/src/digest/`；route `routes/rss.rs` 扩订阅 CRUD + `routes/subscriptions.rs` |
| 5 API | `/api/v1/subscriptions`(CRUD)；`/api/v1/digest`(取/标已读)；`/api/v1/research`(深研，member-gated) |
| 6 扩展点 | 新源类型走 `SourceConnector` trait（已有框架）；digest 规则可插 |
| 7 错误/边界 | feed 网络故障不阻塞 worker(rss.rs 已有)；tight-loop 防护(touch_polled_at)；空源/0 新 entry 不出卡 |
| 8 成本 | C1/C2 全零成本(采集+extractive digest)；C5 深研 💰 显式触发；**digest 绝不后台跑 LLM**(成本契约硬线) |
| 9 测试 | connector 单测已有；新增 digest 确定性测试 + 深研 harness 真 LLM N=3(综合 grounding + 跨源一致性) |
| 10 兼容 | connector 框架已有，向后兼容；订阅表新增 migration |
| 11 风险 | 跨源去重漏(C4 联动)；深研 token 爆(history truncation §4.5.G + budget)；源失效韧性(参考域名失效 spec) |

### 7.3 主动助理（A1 triage + A5 卡 + A4 提醒）

| § | 要点 |
|---|------|
| 1 目标 | 补结构性最弱的"主动"腿；服务支柱 1(主动进化)+场景 1(忙碌助理)+价值"省时间" |
| 2 范围 | 做：今日 triage 清单(确定性排序)、统一建议卡(并入已有 suggestions spec)、截止日/跟进提醒。不做：AI 主动建议下一个问题(成本契约禁止偷跑) |
| 3 数据流 | `多类信号(signals.rs)+item 元数据+日期抽取 → 确定性优先级打分(recency×重要度×未读×截止临近) → 排序清单 + 提醒卡`；**全确定性零 LLM**，点击单项才升级 |
| 4 模块 | 复用 `store/signals.rs` + `project_recommender.rs`；新 `attune-core/src/triage/`；并入 suggestions spec 的卡片层 |
| 5 API | `/api/v1/triage/today`；建议卡复用 suggestions spec API |
| 6 扩展点 | 打分规则可配；卡片类型可扩(triage/digest/organize/reminder 统一卡模型) |
| 7 错误/边界 | 空 vault→空清单不报错；信号缺失→降级按 recency；时区(i18n) |
| 8 成本 | 🆓 全零成本(确定性排序+提醒)；**绝不后台 LLM**；点击单项才进第三层 |
| 9 测试 | 排序确定性 golden + 边界(空/海量 item)+proptest(排序稳定性) |
| 10 兼容 | 新增，无迁移；与 suggestions spec 卡模型统一(避免双卡系统) |
| 11 风险 | 与 suggestions spec 重叠(必须统一卡模型，本 spec 显式收编)；排序规则主观(可配+用户反馈调) |

---

## 8. 成本契约 + OSS/pro 边界守则

**成本契约（CLAUDE.md 三层，本路线图硬约束）**：
- 🆓 零成本：triage 排序 / extractive digest / 去重精确匹配 / 引用格式识别 — 随便跑。
- ⚡ 本地算力：建库摘要 / 分类 / embedding 近重折叠 — 建库阶段自动，可暂停。
- 💰 时间/金钱：起草/改写/综述/深研 — **必须用户显式触发，永不后台偷跑 LLM**。建议卡/digest/triage 都是被动呈现的**确定性产物**，点击才升级到第三层。UI 必须显示成本(token/$或本地/s)。

**OSS / pro 边界守则**：
- **通用能力进 OSS**：起草/改写/综述/triage/digest/订阅/深研对**任何领域个人用户**都有价值 → OSS base。
- **行业特化进 pro**：行业模板/术语表/法律文书/专利 OA 答复/标书生成 → pro，**复用 OSS base narrative**(legal_drafter 迁为 base writing 的行业特化是典范)。
- **不回灌**：OSS 写作 agent 不得内置任何行业 prompt/schema(per OSS 边界审计教训)。

**Agent 验证铁律**：所有新 narrative/agent 走六类测试下限 + 真 LLM holdout N=3 F1≥floor + §4.5 全装 + 3-tier 矩阵；未达标不 ship(defamation_extractor F1 0.09 踩坑前车)。

---

## 9. 风险登记

| 风险 | 缓解 |
|------|------|
| narrative 生成幻觉(精准价值崩) | grounding 强制 + 无命中留占位 + holdout grounding-precision floor |
| 弱模型生成质量差(§4.5 兜底定位) | 3-tier 矩阵实测；不达标 RELEASE.md 标 model tier / 本地 disable |
| 主动卡片系统分裂(triage/digest/suggestions 各做一套) | 本 spec 强制统一卡模型，§7.3 收编 suggestions spec |
| 成本契约破防(digest/triage 误后台跑 LLM) | 确定性产物硬线 + code review 守卫 + 成本 telemetry |
| 跨文档/跨源 token 爆 | 三杠杆(extractive 预裁+缓存+MAP-REDUCE)+history truncation+budget |
| 与已存 spec 重叠/漂移 | 本 spec 显式引用并定位 4 份已存 spec，不复制；实现时回头对齐 |
| pro 写作回灌 OSS(边界破) | OSS base narrative 零行业绑定；行业特化只在 pro 注入 |

---

> **下一步**：本路线图评审通过后，按 §6 第一刀顺序对 #1/#3(写作)、#4/#5(信息监控) 起正式 §3.1 11 节 spec(§7 大纲为骨架)；#2 建议卡直接走已有 suggestions spec。每方向 spec 评审 → writing-plans → 实施(Agent 验证铁律)。

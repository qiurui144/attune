# OSS attune — Agent / 能力权威目录（Capability Catalog）

> **范围**：本文件是 **OSS attune（免费版）侧** 所有 AI agent / 能力的**权威清单**（SSOT），
> 供"有新需求时对照现有能力"使用。**attune-pro 行业 agent（law/medical/patent/sales/...）不在此**
> —— 见 `attune-pro` 仓的对应目录。
> **交叉引用**：开发流程 / 分工 / test-fix-verify 闭环见 [`agent-development-workflow.md`](agent-development-workflow.md)（不重复）；
> 用户视角的 agent 速览见 [`wiki/agents.md`](wiki/agents.md)（混 pro+OSS，偏使用层）；
> 视觉流水线设计见 [`wiki/vision-understanding-pipeline.md`](wiki/vision-understanding-pipeline.md)；
> 省 token 架构见 [`wiki/memory-token-economy.md`](wiki/memory-token-economy.md)。
> **接地原则（§6.3）**：每条均接地真实代码（截至 develop `96c776c`，含 #92 vision / updater / CI）。
> 拿不准处标 **待确认**，不臆造。

## 0. 目录

- [1. Document Intelligence（文档智能）](#1-document-intelligence文档智能)
- [2. 视觉理解（Vision / Non-text + VLM）](#2-视觉理解vision--non-text--vlm)
- [3. AI 批注 Agent（ai_annotation）](#3-ai-批注-agentai_annotation)
- [4. Base 能力（非 domain agent）](#4-base-能力非-domain-agent)
- [5. 覆盖矩阵 + 缺口](#5-覆盖矩阵--缺口)

每条记录字段统一：**ID / 一句话能力 / 输入→输出 / 输出模式 / 真 LLM 状态 / 测试覆盖**。
输出模式四类（per `agent-development-workflow.md` §6 输出模式契约）：**结构化**（typed struct/JSON）/
**标记**（marked，带文本 offset 锚点）/ **批阅**（review，逐章节边注 + 引用）/ **叙述**（narrative）。

---

## 1. Document Intelligence（文档智能）

**模块**：`rust/crates/attune-core/src/document_intelligence/`（spec `docs/superpowers/specs/2026-06-06-oss-document-intelligence.md`）。
**HTTP 入口**：`attune-server/src/routes/documents.rs`（POST `/api/v1/documents/{compare,summarize,chapters}`）；
tier-3 LLM 阶段经路由层 `enforce_gate(is_tier3, is_paid)` 会员门控 → `403 {code:"membership-required"}`（documents.rs:45-167）。
**省 token 三杠杆**（deep_summary 全用，其余部分用）：① extractive 零 LLM 预裁；② `chunk_summaries`
缓存复用（`chunk_hash` + strategy key）；③ cheap-MAP / reasoning-REDUCE 分流。
**真 LLM gate**：`attune-core/tests/doc_intel_real_llm_gate.rs`（3 agent A/B/C，N≥3 seed，多 tier env，floor 全 **≥0.80**，
provider 走 `ATTUNE_LLM_*` env，默认 Ollama qwen2.5:3b / DeepSeek openai-compat）。**注意**：该 gate 全部
`#[ignore]`（opt-in nightly，非默认 CI），**真 provider 执行的 run-log 未在源码树落档 → 待确认是否已对真模型跑过**。

| ID | 一句话能力 | 输入 → 输出 | 输出模式 | 真 LLM 状态 | 测试 |
|----|-----------|-----------|---------|-----------|------|
| `compare` | 文档三层 diff（结构 LCS / 文本 LCS / 语义判定）旧 A vs 新 B | `(a,b,mode,output_mode,member,router,StageLlms)` → `DiffReport`{structural/textual/semantic_verdicts/summary/annotations/token_bill} | **标记**（默认；`annotations[]` 锚到 **doc-b 字符 offset**，CJK 安全）；`structured` 返回原始 payload | **真**。仅语义层 tier-3：变更 span 用 **Cheap** model 逐段判定 + **Reasoning ×1** 总结。schema-guided JSON + ≤3 重试-验证 + 关键词降级（§4.5.A/B/E）。doc 注称真 deepseek F1 0.91→1.00（schema 修复后）；gate floor macro-F1≥0.80 + 零 parse 失败 | 16 `#[test]`；golden `tests/golden/doc_compare_verdict/cases.yaml` **30 人工标注 case**（含 8 numeric）+ 确定性门 `doc_compare_verdict_golden_gate.rs` |
| `deep_summary` ⭐旗舰 | 省 token 多级摘要（brief/standard/detailed），map-reduce | `(full_text,level,item_id,router,StageLlms,store,dek,cfg)` → `(Summary, TokenBill)` | **叙述**（路由 `render_narrative`；模块本身出结构化 `Summary`，无 offset） | **真**。三杠杆全用：extractive 预裁 + chunk_summaries 缓存（`chunk_hash`+`deepsum:<level>`）+ MAP=Cheap/段 + REDUCE=Reasoning ×1 或 ⌈n/16⌉ 折叠。短文(<1500 tok)零 LLM 旁路。floor keypoint-recall≥0.80 | 11 `#[test]`（缓存往返 / naive 基线精确 / 短文旁路 / fan-in 树 / 空文零 LLM） |
| `chapters` | 逐章节阅读 + 跨章节记忆 + **文档内** Q&A（非 vault RAG） | `list(text,keep_ratio)→Vec<ChapterEntry>`；`summarize_chapter(...)` / `ask(...,question,...)` → `ChapterReadResult`{result/cross_chapter_memory_used/annotations/citations/token_bill} | **批阅**（默认；逐章 `annotations[]` 锚到**该章字符 offset**；`ask` 答案带**引用 offset** 回源）；`structured` 去掉 overlay | **真**（Reasoning model）。`list` 零 LLM。跨章记忆=注入前序章节缓存摘要（非 vault-wide RAG，mod.rs:7-10）。floor grounded-answer≥0.80 | 11 `#[test]`（批阅边注 / 跨章记忆 / 引用 offset 对齐 / 非法 idx / 空问题短路 / 降级） |
| `extractive` | 本地零 LLM 抽取式预裁（TF + 位置先验 + 标题命中 + 长度归一，取 top-K 句） | `(block,keep_ratio,heading_words)` → 子集文本（保序） | N/A（出文本） | **无 LLM**（纯本地，三杠杆之 ①）；可插 `ExtractiveScorer` trait | 9 `#[test]` + 3 proptest（输出不大于输入 / CJK split / keep-ratio bound / emoji 不 panic） |
| `vlm_extract` | 扫描件/图片文档经 Vision VLM 提取文本（文本文档跳过 VLM） | `DocSource::{Text,Image}`；`resolve_text(src, &dyn VlmProvider)` → 文本（Image→`vlm.caption`，Text→原样，call-count 0） | N/A（出文本，喂给上述模块） | `VlmProvider`；**模块内仅 mock（RecordingMockVlm）**。**待确认/疑似缺口**：`grep vlm_extract` 仅命中自身 + mod.rs 声明，**documents.rs 零引用** → 实现 + 单测齐，但**未经 doc-intel HTTP 路由暴露** | 5 `#[test]`（image→VLM / text→skip / from_path） |
| `model_routing` | 每阶段 vetted-model 客户端选择（Cheap/Reasoning/Vision 三族，非"一律最贵 tier"） | `app_settings` JSON → `ModelRouter`；`pick(role)` → 逻辑 model 名 | N/A（决策层） | 决策**客户端**完成，gateway 只计费+转发。无 routing block → 全 fallback 到 `default_model`；BYOK/Ollama → `all_same`（弱模型降级 §4.5.E） | 8 `#[test]`（含 partial fallback / unconfigured 报错 / BYOK all_same / 子分组 distinct） |
| `token_bill` | token 节省记账（naive 基线 vs 实际 map+reduce 计费）使省量**可测** | 计数 → `TokenBill`；`savings_ratio_by_token()`（主指标 §8.5）/ `_by_usd()` | N/A（结构化计数，挂每个 report 信封） | **无 LLM**（纯算术）。安全：**无 secret 字段**，sentinel `test-gateway-token-not-real` 断言不存在 | 6 `#[test]`（token 数学 / USD<token / 缓存独立计 / 零 actual→1.0 / 无 secret 结构守卫） |

---

## 2. 视觉理解（Vision / Non-text + VLM）

**模块**：`rust/crates/attune-core/src/ocr/nontext/` + `src/vlm.rs`（#92，ADR-0008 共享视觉核心）。
**设计**：layout 检测 → 逐 region 本地识别 → OCR 交叉验证 → 必要时 VLM 升级（带 grounding + 多模型 failover）。

### 2.1 非文本识别（nontext）

`RegionKind`（mod.rs:32-43）：`Text/Table/Chart/Figure/Formula/Handwriting/Stamp/Signature/Checkbox/FormField`。
`recognize_page` → `RecognizePageResult{regions, correction_report, local_regions, escalated_regions, engine_status, validation_warnings}`，每 region 带 typed `RegionResult`（…V1 枚举）。

| 识别器 | 能力 | 本地输出 | VLM-only 部分 |
|--------|------|---------|--------------|
| `layout.rs`（Stage1） | 版面检测（真 ONNX PicoDet / RapidLayout PP-Structure CDLA，10 类，DFL decode + class-agnostic NMS，score 0.4 / NMS-IoU 0.5，S8 source failover 自动下载） | region bbox + class | — **诚实状态**：检测精度**未对标注集验证（无 mAP）**（mod.rs:250-251） |
| `checkbox.rs` | 🆓 暗像素比阈值 0.10 | `checked` bool | — |
| `stamp_signature.rs` | 🆓 presence：印章=红墨比≥0.02 / 签名=暗比 0.01–0.40 | 存在性 | text/type → VLM |
| `chart.rs` | 🆓 类型/series 不臆造 | type="unknown", series=[], axis=OCR 行 | series 值 → VLM |
| `figure.rs` / `formula.rs` / `handwriting.rs` | 🆓 框定 | figure class / formula raw_ocr / — | caption / latex / 转写 → VLM |
| `table_structure.rs` | SLANet ONNX 适配 | `parse_html_table` 解析 `<tr>/<td>/rowspan/colspan` | **待确认**：真 SLANet 推理未接（model 在→空 HTML→空表；model 缺→`UnrecognizedV1{model-missing}`） |

### 2.2 关键能力

| ID | 一句话能力 | 输入 → 输出 / 逻辑 | 真 VLM 状态 | 测试 |
|----|-----------|------------------|-----------|------|
| `vision.recognize`（**共享能力**, `vision_capability.rs`） | 统一视觉入口（OSS + pro 插件复用，ADR-0008，`CAPABILITY_ID="vision.recognize"` schema v1） | `recognize(image_path,layout_model,table_model,ocr_lines,Option<&VlmRouter>)` → `VisionRecognizeResult{page, vlm_hint}`（funnel `recognize_page`，附 router 的 `VlmHint{suggest_higher_tier,kind_failure_rates}`） | **明确为复用而暴露**：pro 插件（law/medical/patent）**必须**消费它而非自带 VLM；输出零行业绑定 generic `RegionResult` | 4 `#[test]` |
| VLM 升级（`vlm_escalate.rs`） | OCR↔本地识别有歧义时升级到 VLM | `should_escalate(policy,discrepant,used,budget)`：budget 满→否；Off 默认不升；OnDiscrepancy 仅歧义；Aggressive 总升。歧义来自 cross_validate（TAU_HIGH 0.85 / TAU_LOW 0.60，冲突/差异总升级）。重试 3 次反馈 parse/grounding 错；耗尽**保留 parsed-但-ungrounded 值不丢弃** + telemetry | **真**（经 egress 类型门控 `VlmEgressToken` 无公开构造器 + `OutboundGate` PII redact + 图像降采样 1024）。L0-tier 源 → `L0CloudBlocked` 失败关闭 | 26 `#[test]` |
| 多模型 failover（`vlm_router.rs`） | N 候选按优先级**串行** failover | `healthy_ordered`（按 priority 降序 + probe 健康）；**串行非 fan-out**：仅 provider-level error 才换下一候选（parse/grounding 留在同模型 ≤3 循环）；全失败→降级本地。`(kind×model)` 失败率→`should_suggest_higher_tier`（阈值 30%） | **真**（候选可挂多个真 VLM） | 8 `#[test]` |
| grounding（`grounding.rs`） | bbox/空间验证（防 VLM 引用错位/越界） | `validate_grounding`：None→Missing；region_bbox/page 不符→OutOfBounds；零面积→EmptyArea；sub_bbox 未完全含于 region→OutOfBounds（u64 防 u32 溢出假通过）。fail 原因反馈进重试 prompt | 确定性几何校验（喂 VLM 重试） | 15 `#[test]` |
| N=3 eval（`nontext/eval/`） | 多 seed 真 VLM eval（feature `vision-eval`，离线） | `DEFAULT_SEEDS=3`；每 fixture×3 seed 走真 `escalate_region`，metrics（chart series F1 / latex Levenshtein / token-F1 / grounding precision）mean±std per (kind×model)，对 floor `compute_verdict` | **真 lane = 3 个 `#[ignore]` nightly**（需 `DASHSCOPE_API_KEY` + 成本授权），默认 model id `qwen3-vl-plus`，仅断言 floor（value-F1≥0.5 / grounding-precision≥0.99），**eprintln 打印不落档**。无 key→PENDING-KEY 不假通过 | 14 `#[test]`（含 3 ignore） |

> **关于"qwen3-vl-plus F1 1.0"（2026-06-19 收口）**：默认 model id 为 `qwen3-vl-plus`。真 DashScope N=3 run-log 已落档 `reports/runs/20260619-204227_vision-qwen3vl-dashscope/`：known-text **token-F1 1.000±0.000 + grounding-precision 1.000**（floor text-F1≥0.5 / gp≥0.99）+ 真 failover（dead-primary→live qwen）。先前"eprintln 不持久化"已修，CI 新增 DASHSCOPE-gated vision real-VLM step（`ci.yml`）。

### 2.3 OCR（PP-OCR，base）

单引擎 **PP-OCRv5 mobile**（`kreuzberg-paddle-ocr` v4.9 + ONNX Runtime）：DBNet 检测 + 角度分类 + CRNN 识别，
4 个 ONNX model ~16-21MB 自动下载；输出 `RawLine{text,bbox,confidence}` / `OcrOutput`（Tesseract 已移除）。零 LLM。

**真 VLM provider**：`LlmVlmProvider`（vlm.rs:56-102，包任意 `LlmProvider`，图→base64 data URI→`chat_multimodal`）；
真 eval lane 用 `OpenAiLlmProvider` 打 DashScope openai-compat。Mock：`MockVlmProvider`/`RecordingMockVlm`/`ScriptVlm`/`DeadVlm`。
**nontext 模块测试 ≈144 个**（4 个 `#[ignore]`：1 layout-real + 3 real-VLM）。

---

## 3. AI 批注 Agent（ai_annotation）

**插件**：`rust/crates/attune-core/assets/plugins/ai_annotation_{risk,outdated,highlights,questions}/`。
**执行**：`attune-core/src/ai_annotator.rs` → 路由 `attune-server/src/routes/annotations.rs::ai_analyze`（POST `/api/v1/annotations/ai`）。
四者**同一执行代码路径**，仅 `prompt.md` + label/color/角度不同。

**共享形态**：item 内容（整篇或 UTF-16 选区）→ LLM 出 JSON `{"findings":[{"snippet,reason"}]}`（schema `findings[]` of
`{snippet 4-150 char, reason 10-200 char}`）→ **后端自己定位 offset**（不让 LLM 输出 offset），`LocatedFinding`
带 `offset_start/offset_end`（**UTF-16 code unit**），3 段定位（verbatim→relaxed→prefix-anchor），定位不到**静默丢弃**，
落 `source='ai'` annotation。

**§4.5 兜底已实装**（修正旧 audit"无真 LLM"的简化说法）：schema-guided（`findings_schema()` 传 `format=<schema>`）+
重试-验证（`LLM_MAX_ATTEMPTS=3` + `validate_findings_json` 反馈）+ few-shot（2 例）+ PII redact/restore 包裹。

| # | ID | 一句话能力 | 输出模式 |
|---|-----|-----------|---------|
| 1 | `ai_annotation_risk` | 找风险/陷阱/安全漏洞（⚠️ 风险，红） | 结构化 + offset 锚（marked） |
| 2 | `ai_annotation_outdated` | 标可能过时的技术/版本/日期（🕰 过时，黄） | 同上 |
| 3 | `ai_annotation_highlights` | 提炼值得长期记忆的核心要点（⭐ 要点，绿） | 同上 |
| 4 | `ai_annotation_questions` | 抛出疑点/追问（🤔 疑点，蓝） | 同上 |

**真 LLM gate 状态（2026-06-19 reliability backfill 已补齐，原 §5.2 gap #1 收口）**：
- **新 4 角度真 LLM gate** `tests/ai_annotation_real_llm_gate.rs`：risk/outdated/highlights/questions **各**有独立 gate，N≥3 seed，对 human-authored holdout（`tests/golden/ai_annotation/<angle>.yaml`，≥10 case/角度，GT 非 agent 生成）算**真 micro-F1**，floor 0.50（只升不降 ratchet，进 `agent_quality_manifest.yaml`）。
- **静默通过已修**：`require_llm()` 在无模型时 **panic**（fail-to-run，不再返回 Ok 静默绿）；run-vs-SKIP 在 **CI job 层**决定（skip-not-pass），不在测试内。
- **接进 CI**：`ci.yml` `real-llm-secret-gated`（DeepSeek/DashScope，PR/push 触发）+ `nightly-real-llm.yml`（Ollama）两条 lane 都跑。
- **真 DeepSeek N=3 实证**（`reports/runs/20260619-201722_ai-annotation-deepseek/`）：highlights 0.735 / outdated 0.826 / questions 0.544 / risk 0.553 —— 全 ≥ 0.50 floor（4 passed）。questions/risk 贴近 floor（snippet-overlap 评分天然比 doc-intel 4 类 verdict 更模糊），数据如实登记未放水。
- **旧** `real_llm_ai_annotator_produces_located_findings`（ai_annotator.rs:770-809，Highlights only、skip-on-unavailable）保留为快速 smoke，不再是唯一真 LLM 覆盖。
- **待确认（剩余）**：Tier-1 弱本地（qwen2.5:3b）floor 未单独跑（DeepSeek=Tier-2 已过）；RELEASE.md 标 AI 批注最低 tier 为 ≥ weak-cloud。

**测试**：核心 `generate_annotations` 共 ~27 mock 单测（drop/locate/cap/reject/bad-JSON/relaxed/prefix-anchor）+ 4 hardening 单测 + 1 个 `#[ignore]` 真 LLM gate。
**路由 `ai_analyze` 本身无测试**，无 `tests/` 集成/subprocess，无 golden YAML。

---

## 4. Base 能力（非 domain agent）

> 这些是 base/平台能力，**不是 domain agent**，简列对照用。路径均 `rust/`。

| 能力 | 一句话 | 输入 → 输出 | 真 LLM / 确定性 | 代码 |
|------|--------|-----------|----------------|------|
| chat / RAG | 检索→注入→单次 LLM 答（**非流式**，per CLAUDE.md） | `{message,history,session_id}` → JSON 答（spinner UI） | **真 LLM**：PII-redact → `search::search_with_context()` RAG → context-budget → `llm.chat_with_history()` → `chat_reliability::evaluate_response()` grounding | `attune-server/src/routes/chat.rs`；`attune-core/src/chat.rs` + `chat_reliability/` |
| search（RRF 混合） | 向量+BM25 RRF 融合 + 可选 cross-encoder rerank | query → 排序 `SearchResult[]` | **确定性零 LLM**（usearch HNSW + tantivy BM25，RRF_K=60，vec 0.6/FTS 0.4，cross-lang penalty 0.3；rerank=ORT cross-encoder 非 LLM，候选≥5 才跑） | `attune-core/src/search.rs`；`routes/search.rs` |
| organize（文件夹→案卷） | 把一组 item 聚类成可审组提案，确认后归档进 Project | scope → 加密 `OrganizationProposal`（analyze）→ confirm → `apply`（幂等，重复返回 already_applied） | **多确定性**（HDBSCAN floor=5，dir fallback，extractive 标签）；**LLM 可选**（传 LlmProvider 时 tier-3 策略命名）。analyze→apply 生命周期**已实装**（#84）；自动整理 workflow + 精修 UI 是较薄部分 | `attune-core/src/organizer/`；`routes/organize.rs`；`routes/folder_links.rs` |
| self_evolving_skill | SkillClaw 式后台技能进化：search-miss 信号→查询扩展，静默提召回 | `skill_signals(kind='search_miss')`（≥10）→ 查询扩展；`expand_query_with_table()` chat 检索前应用 | **混合**：新 query-pattern 默认**零成本启发式**（共现+停用词）；LLM 词扩展仅 user-enable + governor 允许时；legacy topic 路径 LLM 驱动 | `attune-core/src/skill_evolution/` |
| memory（L0-L3 + 巩固） | 分层记忆：召回/概览查询用紧凑摘要答，省原始 chunk token | query → `AssembledContext`（按 query shape 选 tier） | **L0** 原始 chunk（真）/ **L1** chunk_summaries（真）/ **L2** episodic 6h 窗 worker（真）/ **L3** semantic topic-cluster（**已实装**，`semantic.rs` 真 LLM 调用，旧"L3 未实装"note 已陈旧，#85）。L2→L3 promotion 排名**确定性无 LLM**（access×recency×density），L3 摘要生成步是 LLM | `attune-core/src/memory/`（assembler/retrieval/semantic/consolidation_agent/migration/portability） |
| providers | embedding / rerank / asr / vlm / llm trait + 实现 | — | embed：真 Ollama/OpenAI/ORT(bge) + Mock/Noop；rerank：真 ORT cross-encoder + Mock；llm：真 Ollama/OpenAI(含 gateway/BYOK/DeepSeek) + Redacting wrap + Mock/Noop；asr：whisper.cpp **subprocess**；vlm：cloud-routed（qwen multimodal，**待确认** 生产 impl 类名） | `embed.rs` / `infer/` / `llm.rs` / `asr.rs` / `vlm.rs` / `ocr/` |

---

## 5. 覆盖矩阵 + 缺口

### 5.1 真 LLM/VLM gate 覆盖矩阵

| 能力组 | 真 LLM/VLM gate? | gate 形态 | 默认 CI 跑? | F1/floor 证据 |
|--------|-----------------|----------|------------|--------------|
| doc-intel `compare` | ✅ 有 | `doc_intel_real_llm_gate.rs` A + golden 30 case | ❌ `#[ignore]` | floor macro-F1≥0.80；doc 注 deepseek 0.91→1.00（**待确认源码外 run-log**） |
| doc-intel `deep_summary` | ✅ 有 | 同 gate B | ❌ `#[ignore]` | floor keypoint-recall≥0.80 |
| doc-intel `chapters` | ✅ 有 | 同 gate C | ❌ `#[ignore]` | floor grounded-answer≥0.80 |
| doc-intel `extractive`/`token_bill`/`model_routing` | N/A | 零 LLM | ✅ 确定性测试全跑 | — |
| vision `vision.recognize`/escalate/failover | ✅ 有 | `eval/` N=3 + 3 ignore real-VLM | ❌ `#[ignore]` | floor value-F1≥0.5 / grounding-prec≥0.99；**"qwen3-vl-plus F1 1.0" 须外部 run-log（#92），源码侧待确认** |
| **ai_annotation（4 个）** | ✅ 有（2026-06-19 补齐） | `ai_annotation_real_llm_gate.rs`（4 角度各 1 leg）+ holdout `tests/golden/ai_annotation/` | ❌ `#[ignore]`（secret-gated CI + nightly 跑） | **真 micro-F1，floor 0.50，ratchet**；DeepSeek N=3 实证 hl 0.735 / outdated 0.826 / q 0.544 / risk 0.553（`reports/runs/20260619-201722_ai-annotation-deepseek/`）。静默 pass 已修 |

**结论 — 真 LLM/VLM gate 计数**：
- **doc-intel = 3 个** agent（compare/deep_summary/chapters）有**真 floor≥0.80 gate**（A/B/C，但 `#[ignore]` 非默认 CI）。
- **vision = 有真 N=3 gate**（escalate/failover/grounding，3 个 `#[ignore]` real-VLM，floor 制）。
- **ai_annotation = 4 个角度各有真 micro-F1 gate**（2026-06-19 补齐；floor 0.50 + ratchet，DeepSeek N=3 实证全过）—— 原最大短板已收口。

### 5.2 缺口清单（供新需求对照）

1. ~~**ai_annotation 真 LLM gate 弱**~~ **✅ 已补齐（2026-06-19）**：4 角度各 1 真 LLM leg（`ai_annotation_real_llm_gate.rs`）+ human-GT holdout（≥10/角度）+ 真 micro-F1 floor 0.50 ratchet + 接进 secret-gated/nightly CI + 静默 pass 修复。剩余：Tier-1 弱本地（qwen2.5:3b）floor 单跑待补。
2. ~~**doc-intel real-LLM gate run-log 未落档**~~ **✅ 已落档（2026-06-19）**：DeepSeek N=3 实证 compare macro-F1 1.000（0 parse fail）/ deep_summary recall 0.944 / chapters grounded 1.000，全 ≥ 0.80 floor（`reports/runs/20260619-202901_doc-intel-deepseek/`）—— 坐实 RELEASE v1.3.0 §9.2 "deepseek 实测" claim。gate 仍 `#[ignore]`（secret-gated/nightly CI 跑，非默认 PR）。
3. **`vlm_extract` 未经 doc-intel HTTP 路由暴露**：实现 + 单测齐，但 `documents.rs` 零引用 → 扫描件/图片文档**走不到** doc-intel 的 VLM 文本提取链（**确为缺口**，记 backlog，非本任务 impl）。
4. **`table_structure` 真 SLANet 推理未接**：model 在也只出空 HTML/空表（确定性占位），表格结构识别**尚不可用**（记 backlog，非本任务 impl）。
5. **layout 检测精度未对标注集验证**（无 mAP）：版面检测功能在但准确度无量化证据。
6. ~~**vision real-VLM F1 落档缺**~~ **✅ 已落档（2026-06-19）**：qwen3-vl-plus N=3 known-text token-F1 1.000 + grounding-precision 1.000 + 真 failover（`reports/runs/20260619-204227_vision-qwen3vl-dashscope/`）—— "qwen3-vl-plus F1 1.0" 不再 eprintln-only，源码侧 待确认 收口。新接 `ci.yml` vision real-VLM step（DASHSCOPE-gated）。
7. **知识库级 RAG（vault-wide）未接**：doc-intel chapters/deep_summary 仅**文档内**检索；跨文档 vault RAG 是 v.next（接 `search::search_with_context` 进 chapters ask / deep_summary context）。
8. **organize 自动整理 workflow + 精修 UI 偏薄**：后端 analyze→apply 引擎真，但"自动把文件夹整理成案卷"的端到端 workflow/UI 仍是较弱面。
9. **OSS 当前无 domain agent**：chat/search/RAG/organize/memory 是 base 能力，非 agent；行业 agent 全在 attune-pro。未来 OSS 加 agent 须复制 attune-pro 的 `agent_golden_gate.rs` reliability framework（per CLAUDE.md「Agent 验证铁律」）。

> **wiki 同步提示**：`docs/wiki/agents.md`（May 25）早于 doc-intel + #92 vision，仅列 4 个 ai_annotation + classifier + office helper，**未含 doc-intel 6 模块 / vision.recognize / N=3 gate**。本 catalog 落地后应同步刷新 wiki/agents.md（per CLAUDE.md "wiki 自动同步 docs"）。

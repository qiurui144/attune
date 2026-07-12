# 视觉理解与稳定输出流水线（Vision Understanding & Stable-Output Pipeline）

> 技术专题文档（团队工作语言：中文）。本页讲清 attune 如何**稳定地**从图像 / 扫描件 /
> 非文字内容中提取可用信息：VLM provider 抽象、schema-guided + retry-validate + grounding
> 三件套如何保证输出稳定、OCR/非文字识别流水线、以及失败兜底（failover）。
>
> 数据引代码路径:行号 / spec 路径（per CLAUDE.md §6.3）。**已实现** 与 **PLANNING/spec** 严格区分标注。

## 0. 目录

- [1. VLM provider 抽象与模型选型](#1-vlm-provider-抽象与模型选型)
- [2. 图像 vs 文本路由（vlm_extract）](#2-图像-vs-文本路由vlm_extract)
- [3. 稳定输出三件套：schema-guided + retry-validate + grounding](#3-稳定输出三件套schema-guided--retry-validate--grounding)
- [4. 失败兜底（failover / graceful degrade）](#4-失败兜底failover--graceful-degrade)
- [5. OCR / 非文字识别流水线（现状 + 规划）](#5-ocr--非文字识别流水线现状--规划)
- [6. 成本分层与隐私](#6-成本分层与隐私)

---

## 1. VLM provider 抽象与模型选型

`vlm.rs` 定义 `VlmProvider` trait（同步、dyn-compatible，`vlm.rs:25-31`），划清三类视觉能力边界
（`vlm.rs:3-6`）:
- **OCR**:图里有清晰文字 → 识别字符（PP-OCR，非 VLM）；
- **VLM caption**:描述图的内容/场景/物体；
- **VQA**:针对图回答具体问题。

`LlmVlmProvider`（`vlm.rs:38-84`）是**薄适配器**:持有任意 `LlmProvider`，读图片 → base64 data URI →
调 `chat_multimodal` 带 `Attachment::Image`，**不自己实现 OpenAI vision 协议**（`vlm.rs:33-37`）。
不支持 vision 的底层 provider 会 drop 图片并仅用文本 prompt，返回退化结果（`vlm.rs:36-37`，per §4.5.E）。

**模型选型**（per CLAUDE.md §4.5H — 多模态默认 qwen-3.6/3.7，禁用已下架的 qwen-vl）:
视觉走 `ModelRole::Vision`（`model_routing.rs:24-27`），由客户端侧 `ModelRouter` 解析逻辑模型名，
网关再做 group + 上游路由（`model_routing.rs:1-11`）。无 `model_routing` 配置时优雅退化到 `default_model`
（degenerate-but-usable，`model_routing.rs:62-89`）。BYOK 用户或 edge scheduler 用户按各自 endpoint contract 映射 role；legacy 自管本地模型只作为显式高级配置，弱模型会 degrade（§4.5.E）。

> 注:`model_routing.rs:27` 注释举例仍写 `qwen-vl class`——这是历史文档残留，生产默认应为 qwen-3.6/3.7
> （§4.5H）。逻辑名由 settings/网关决定，代码不硬编码具体模型。

---

## 2. 图像 vs 文本路由（vlm_extract）

文档智能流水线工作在 **TEXT** 上。`vlm_extract.rs` 负责"有文字的源跳过 VLM，无文字的源先抽文字"
（`vlm_extract.rs:1-7`，T-09）:

- `DocSource::from_path`（`vlm_extract.rs:25-39`）:有可用文本 → `Text`；否则按扩展名判图像 → `Image`。
- `resolve_text`（`vlm_extract.rs:51-58`）:`Text` 源**原文返回，不碰 VLM**；`Image` 源走 `vlm.caption()` 抽文字。

**稳定性意义**:vision 是 cheap-VLM tier，**绝不在文本文档上运行**（避免无谓成本与不确定性）。
测试断言:`test_image_doc_routes_to_vision`（`:62-70`，图像源 `total_calls ≥ 1`）+
`test_text_doc_skips_vlm`（`:72-79`，文本源 `total_calls == 0`）。抽出的文字随后流入正常
compare / deep_summary / chapters 流水线，复用第 1 节的省 token 机制。

---

## 3. 稳定输出三件套：schema-guided + retry-validate + grounding

LLM/VLM 在弱模型上易抽烂数据、JSON parse 间歇炸（attune 历史踩坑:defamation_extractor qwen2.5:3b
real F1=0.09，见项目 CLAUDE.md「Agent 模型兜底」）。attune-core 在 `LlmProvider` trait 层固化了
CLAUDE.md §4.5 A/B/C 三件套作为通用底座:

### B. retry-validate 循环（`llm.rs:347-398`）

`chat_with_retry(system, user, max_attempts, validator)`:
- 每次 LLM 输出过 `validator(raw)`，`Ok(())` 立即返回；
- `Err(msg)` → 把校验错误作为**新一轮 user turn 追加进 conversation**，让模型看到错在哪里再重试（真 multi-turn，§4.5G）；
- 最多 `max_attempts`（典型 3）次，耗尽返 `Err`（`llm.rs:389-393`）。
- 用 `&dyn Fn` 而非泛型，保留 trait dyn-compat（caller 可 `&dyn LlmProvider`，`llm.rs:355-356`）。

适用于 **grounding verify / JSON valid / field complete** 等"校验-反馈-重试"场景（`llm.rs:352`）。

### A. schema-guided 输出 + C. few-shot

`chat_few_shot`（`llm.rs:401-417`）构造 `[system, ex1.user, ex1.assistant, ..., user]` 真消息序列
（§4.5C，小模型对"follow this pattern"响应良好）。JSON 格式约束 + few-shot 例子覆盖典型场景 + edge case，
消除"自由文本→自己 regex parse"的不确定性。

### grounding（语义校验）

grounding = 用独立第二意见交叉校验，确保输出有据。在 attune-pro 行业 agent 上有真实证据:
**code_reviewer 的 G7 增强把 grounding 从"line_refs 存在"扩展到语义校验**（attune-pro commit
`f0c4fa5` `test(tech-pro): strengthen code_reviewer LLM agent — G7 semantic grounding + over-report
trim + 16-case hardened holdout (real-DeepSeek N=3)`）。详细 F1 数据见
`docs/benchmarks/2026-06-quality-proof-points.md`。

---

## 4. 失败兜底（failover / graceful degrade）

per CLAUDE.md §4.5.E，弱模型必须**有意义 degrade 而非直接挂**:

| 失败点 | 兜底 | 代码 |
|--------|------|------|
| VLM 底层 provider 不支持 vision | drop 图片，仅文本 prompt，返退化结果 | `vlm.rs:36-37` |
| 单 map 块 VLM/LLM 调用失败/返空 | 降级为该块 extractive 候选，不拖垮整篇 | `deep_summary.rs:233-246` |
| 上下文压缩 LLM 不可用 | 截断原文代替摘要，chat 不挂 | `context_compress.rs:135-144` |
| retry-validate 3 次耗尽 | 返 `Err`（caller 决定 degrade，不 panic） | `llm.rs:389-393` |
| memory bundle LLM 失败 | 返 None，不影响其他 bundle | `memory_consolidation.rs:161-165` |
| 图片文件读取失败 | `caption` 返 `Err`（`std::fs::read` 透传） | `vlm.rs:48-53`，测试 `llm_vlm_caption_errors_on_missing_file :267-274` |

核心原则:`Result::Err` + 用户友好 message，**绝不 panic 上抛**。

---

## 5. OCR / 非文字识别流水线（现状 + 规划）

### 已实现（现状）

- **OCR 引擎**:PP-OCRv5 mobile（ONNX via ORT）+ poppler-utils，subprocess 调用，零 API 费用
  （项目 CLAUDE.md 技术栈）。识别字符 + 简单版面。
- **VLM 抽文字**:扫描件/图片走 `vlm_extract::resolve_text` → `vlm.caption()`（见 §2）。

### 规划中（PENDING — spec 已落盘，未实现）

> ⚠️ 以下为 spec 阶段（PLANNING），尚未 impl，**不声称已交付**:
> `docs/superpowers/specs/2026-06-10-document-nontext-content-recognition.md`（Status: **Draft**，明示
> "本文档仅 PLANNING，未实现"）。

该 spec 的核心命题（spec §1.1）:**OCR 的结构与内容都易错，且单 OCR 不能自我校验**。
单 PP-OCRv5 有三类系统性错误:
1. **结构错误**:不输出 cell 合并标记，跨行合并表头/嵌套表/不规则列宽 → 列错位/行串位；
2. **内容错误**:字符误读（0↔O、l↔1、相似汉字），低 confidence 仅能提示无法纠正；
3. **非文字内容丢失**:图表数值系列、公式、流程图、印章/签名/勾选框直接 drop。

设计解法（spec §1.1 / §1.2）:不是"盲目全 VLM"（贵/慢/本地不可行），而是
**⚡本地非文字模型先行（cheap 第二意见）→ 仅在低置信/OCR 分歧的局部 region 才 💰 升级 VLM**，
通过独立第二意见 + 交叉校验逼近精度。落 L0（零成本 CPU）/ L1（本地 GPU-NPU）/ L2（LLM-VLM）三档成本。

### 共享视觉 agent 架构

per ADR 0008（spec §1.2 引用），视觉理解是**全产品唯一的视觉核心**:所有插件统一从它获取图像能力，
行业插件不自带视觉；高精 VLM 档是该共享 agent 的一个 attune-pro 会员/网关计费 tier（非 per-plugin fork）。
多 provider 架构 spec:`docs/superpowers/specs/2026-05-24-llm-vlm-multi-provider-architecture.md`
（Status: **DRAFT**，phased delivery v1.0 settings 字段 dormant → v1.0.1 cloud /me + new-api VLM channel → v1.1 UI 完整 routing）。

---

## 6. 成本分层与隐私

- **本地优先**:OCR 全本地（PP-OCRv5 CPU）；VLM 是 escalation 而非默认路径（spec §1.2）。
- **数据安全**:非文字识别的 ⚡ 档全本地，敏感文档（合同/证件）不必上云即得结构化结果；
  仅低置信局部 region 才走 💰 VLM，且可被 governor 全局关闭（spec §1.2）。
- **成本可见**:VLM 调用同样产出 token 计费可见（沿用 `token_bill.rs` 机制，见
  `docs/wiki/memory-token-economy.md` §5）。

---

## 关联

- 记忆与省 token 架构 → `docs/wiki/memory-token-economy.md`
- 技术证据汇编 → `docs/benchmarks/2026-06-quality-proof-points.md`
- 非文字识别 spec → `docs/superpowers/specs/2026-06-10-document-nontext-content-recognition.md`
- VLM 多 provider spec → `docs/superpowers/specs/2026-05-24-llm-vlm-multi-provider-architecture.md`

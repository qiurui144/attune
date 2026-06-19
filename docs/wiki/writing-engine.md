# 写作引擎（Writing Engine）— grounded 起草 / 改写

> 北极星「写论文 / 写文档效率」的落地。attune 此前能**读懂、检索、抽取、批注、摘要**知识库，
> 但「帮我把这些写出来」一直是空白。写作引擎补上这条 **grounded narrative 生成链**：任何用户
> 都能从大纲 + 知识库素材生成**可回指源、不编事实、成本可见、可迭代**的草稿。
>
> spec: `docs/superpowers/specs/2026-06-19-writing-engine.md`（11 节）。

## 当前能力（首发）

| 能力 | 端点 | 说明 | 输出模式 |
|---|---|---|---|
| **W1 起草** | `POST /api/v1/writing/draft` | 大纲 + KB 素材 → 草稿段落（论文 / 文档 / 邮件 / 报告 / 笔记，OSS 通用） | narrative + structured（分段 + 每段 grounding） |
| **W2 改写** | `POST /api/v1/writing/rewrite` | 调语气 / 长度 / 受众，**保事实不漂** | narrative / review（逐句建议带 offset） |

后续切片：W3 大纲 · W4 引用 · W5 综述 · W6 术语（见 spec §2.3）。行业起草（法律文书 / 专利
权利要求）在 **pro**，经 `WritingTemplate` trait 复用本引擎，不在 OSS。

## grounding 红线（生成类最大风险 = 幻觉）

写作引擎不测「写得好不好」（主观），而是钉死可信性的三条确定性闸：

1. **逐片段回指源**：每个生成片段经 token-overlap grounding 校验（复用 chat 可靠性同款机制）。
   未能回指任何源的**事实性片段**进 `unverifiedSpans`，UI 标 `[需核实]` / 红色警示，**绝不
   静默当成原创事实输出**。
2. **改写保事实**：改写以**原文为唯一 grounding 源** —— 改写后若出现原文没有的新事实，即标
   fact-drift（不静默接受）。
3. **素材注入防御**：KB 素材在喂模型**之前**做注入指令检测；带「忽略上面指令 / 编造引用 / 你
   现在是…」的中毒素材 → 直接 400 拒绝，**LLM 完全不被调用**。

## 成本契约（💰 第三层）

- 生成 = 💰 时间/金钱，**必须用户显式触发，永不后台偷跑**；选材 / 裁剪 / 骨架 = 🆓/⚡。
- 素材喂模型前先 **extractive 预裁**（省 token 杠杆 1），最小化生成输入 token。
- 每个响应挂 `tokenBill`（naive 基线 vs 实际花费，**无任何 secret 字段**），UI 可显示成本。

## 兜底（§4.5，弱模型可 degrade）

复用 `ai_annotator` 同款 `llm_chat_redacted_hardened` 栈：schema-guided JSON + 重试-验证
（≤3）+ few-shot + PII redact/restore。生成失败（schema ×3 仍非法）→ 503
`generation-unavailable` + telemetry，不 panic。

## 质量证据（real-LLM N=3，deepseek-chat/v4）

| 指标 | 结果 | floor |
|---|---|---|
| draft grounding-precision | **1.000 ± 0.000** | 0.90 |
| draft fact-consistency | **0.972 ± 0.039** | 0.85 |
| rewrite fact-preservation | **0.917 ± 0.068** | 0.90 |
| synthesis (W5) grounding-precision | **0.826 ± 0.038** ⚠ 低于 floor | 0.90 |
| synthesis (W5) fact-consistency | **1.000 ± 0.000** | 0.85 |

> **W5 综述 grounding 已知限制（诚实记录，floor 不下调）**：W5 多源综述的 grounding-precision
> 在 deepseek-v4-flash N=3 实测 **0.826**（经比例重叠召回算法 + 全半角归一从 0.779 提升），
> 安全不变量 **fact-consistency = 1.000（零编造）** 成立。仍 < 0.90 floor。
> **根因 = 确定性 token-overlap 校验器对抽象式综述句的召回天花板**，**不是模型能力差**：
> deepseek-**v4-pro** 同测 **0.816 ± 0.019**，与 flash 持平 → 换更强生成模型无可测增益
> （印证模型策略 §4.5H）。残余缺口是「语义等价但用词远离原文」的释义句，token-overlap 无法
> 信用化；彻底闭合需后续增量的 **LLM-judge grounding 步骤**（而非更强生成模型）。在此之前
> W5 综述标 **Beta / 需 LLM-judge grounding 增量**，floor 0.90 作为该增量必须达到的硬门保留。

run-log：`rust/reports/runs/<ts>-writing-real-llm-deepseek/run.log`（W1/W2）·
`rust/reports/runs/2026-06-20_synthesis-grounding-uplift/`（W5 综述 N=3）。real-LLM gate 默认
`#[ignore]`，接 secret-gated CI lane；golden corpus 为人工标注 GT（≥10 真实 + 1 sentinel /
每能力，**禁 LLM 生成 GT**），阈值 ratchet 只升不降。

## 错误码（kebab，spec §7）

| code | HTTP | 触发 |
|---|---|---|
| `membership-required` | 403 | 未登录 / 非会员调生成 |
| `cloud-llm-disabled` | 403 | 会员但未开启 Privacy 云 LLM 出网 |
| `no-source-material` | 400 | 空大纲 + 空素材 |
| `empty-input` | 400 | 改写空文本 |
| `source-injection-detected` | 400 | 素材含注入指令（LLM 不被调用） |
| `vault-locked` | 401 | 引用 KB item 但 vault 未解锁 |
| `generation-unavailable` | 503 | 兜底重试耗尽 |

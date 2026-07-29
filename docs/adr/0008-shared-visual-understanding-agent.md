# ADR 0008: Shared Visual-Understanding Agent（插件消费，绝不自带视觉）

**Status**: accepted
**Date**: 2026-06-10
**Deciders**: 用户拍板（"所有插件的图像类问题，统一从视觉理解 agents 获取，行业插件内部不自带视觉 agent"）

## Context

attune 多个插件都有图像/视觉理解诉求：OSS 文档非文字识别（表格/图表/公式/印章/签名/手写区域），
attune-pro 行业插件（law-pro 证据图片分类、patent-pro 附图、medical-pro 检验单表）等。

若放任每个插件各自实现视觉/VLM 逻辑，会出现 N 个插件各带一份 VLM 调用 + prompt + 升级阈值 + 缓存 +
telemetry。后果（已在 law-pro `evidence_classifier` 直接内嵌 VLM 调用上看到苗头，见 census）：

- **N× 成本**：同一张图被多个插件各自盲送 VLM，无共享缓存。
- **N× 质量漂移**：每插件 prompt / 阈值 / 重试逻辑各调一套，质量不一致、无法统一回归。
- **N× 维护**：VLM provider 变更、schema 演进、弱模型兜底（§4.5）要在 N 处重复改。
- **§4.3 边界泄漏**：通用视觉能力（generic vision）本应是 OSS-base 共享，散进各 pro 插件 = 通用能力被行业仓重复实现（MEMORY「OSS 边界行业回灌」前科）。

OSS 侧已在建设的「文档非文字内容识别」能力（spec `docs/superpowers/specs/2026-06-10-document-nontext-content-recognition.md`，
正在 isolated worktree 实施）天然就是一份**通用视觉理解核心**：7 类 region 检测 + 本地模型优先识别 +
交叉校验 + 仅对分歧/低置信 region 升级 VLM。它不含任何行业词汇，对任何领域个人通用用户都有价值。

## Decision

**全产品仅有一份共享的「视觉理解能力」（visual-understanding agent / capability），所有插件的图像类需求统一从它获取；任何行业插件都不得自带视觉/VLM 逻辑。**

1. **单一共享视觉核心**：generic vision understanding（表格 / 图表 / figure / 公式 / 印章 / 签名 / 手写区域识别，
   本地模型优先 → 仅对低置信/分歧 region 升级 VLM）由 OSS attune 的非文字识别能力承载，作为**唯一**视觉核心。

2. **以可被 agent 调用的能力暴露**：该核心暴露为一个可被任意插件（OSS 或 pro）调用的 agent/capability 接口；
   插件不直接持有 VLM provider，而是调用共享视觉 agent 拿通用结构化输出。

3. **行业插件只叠语义，绝不内嵌视觉**：law-pro / patent-pro / medical-pro 等**禁止**实现 VLM / 视觉模型逻辑；
   它们调用共享视觉 agent，在其通用结构化输出之上叠加**行业语义**（如「这张表是合同条款表 / 这枚印章是否合规 /
   这张图是专利附图」）。行业插件的价值在语义解释，不在重复造视觉轮子。

4. **付费档仍是同一份 agent**：高精度 / aggressive-VLM 档（全 region VLM + 多模型投票 + 精调 prompt）可由
   attune-pro 会员经 LLM 网关计费门控，但它**仍是同一份共享视觉 agent 的一个 tier**，而非每插件各自 fork 的视觉实现。
   tier 是计费/精度开关，不是代码分叉。

## Consequences

**好处**：
- 视觉成本 / 质量 / prompt / 阈值 / 缓存 / telemetry 收敛到**单一调优与遥测点**，一处改全局生效。
- 对齐 §4.3：generic vision = OSS-base 共享；行业语义 = pro 叠加。杜绝通用视觉能力回灌进行业仓。
- 共享缓存（region_crop hash）让同图跨插件零重复付费。
- 弱模型兜底（§4.5 A/B/C/E/F）只需在共享 agent 实现一次，所有插件受益。

**代价**：
- 共享 agent 接口成为跨 OSS↔pro 的契约，演进须做版本协调（pro 锁 OSS tag，per `oss-pro-strategy.md` 耦合规则）。
- 非文字识别能力**必须暴露为 agent-invocable surface**（当前 spec §5 仅有 REST/CLI，缺 agent 调用面）——
  这是一个实现 delta，已在非文字识别 spec §6 标注为待补的 follow-up（见该 spec 修订）。
- 行业插件作者需理解「调用共享视觉 + 叠语义」的协作模型，不能图省事内嵌一份 VLM。

## Alternatives

- **每插件自带视觉/VLM（被否决）**：实现最快上手，但 N× 成本/质量/维护 + §4.3 边界泄漏，且无统一遥测。
  正是本 ADR 要根除的反模式。
- **每插件一份「精度包」fork 同一视觉核心（被否决）**：表面共享代码，实际仍是多份分叉，prompt/阈值各调，
  统一调优失效，且 tier 应是计费开关而非代码分叉。
- **共享视觉但只暴露 REST/CLI、不暴露 agent 面（被否决为长期形态）**：插件经 HTTP 自调亦可，但无法纳入统一
  agent 编排 / 兜底 / 遥测框架，且与「插件统一从视觉理解 agents 获取」的用户意图不符。短期 REST 可用，
  但 spec 须标注 agent-invocable 暴露为目标形态（已标 delta）。

## References

- spec: `docs/superpowers/specs/2026-06-10-document-nontext-content-recognition.md`（共享视觉核心；§跨切面 + §6 已对齐本 ADR）
- `docs/oss-pro-strategy.md` §4.3（新功能归类决策规则 — generic vision = OSS / 行业语义 = pro）
- ADR 0001（OSS × Pro 边界）、ADR 0007（Model-Agnostic LLM Infra — 弱模型兜底）
- CLAUDE.md §4.5（LLM Agent 兜底原则 — 单一调优/遥测点）、MEMORY「OSS 边界行业回灌」

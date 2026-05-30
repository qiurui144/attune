# ADR 0007: Model-Agnostic LLM Infra（弱模型兜底加固）

- **Status**: Accepted
- **Date**: 2026-05-22

## Context

attune 的 LLM agent（document_classifier / extractor / memory_consolidation /
skill evolution 等）不假设特定 model tier — 用户从 Claude Pro 到 Gemini Flash
到本地 qwen2.5:3b 都有。v1.0 GA 前踩坑（per CLAUDE.md「Agent 模型兜底」案例）：
`defamation_extractor` mock 全过但 qwen2.5:3b real F1 = 0.09，根因是缺
schema-guided 输出 + 重试-验证循环 + few-shot 三项基础设施，导致 JSON parse
间歇炸、多字段抽烂，ship 前才暴露。需要一套 model-agnostic 工具让任何 agent
在弱模型上有意义 degrade 而非直接挂。

## Decision

在 `LlmProvider` trait 上增加 3 个 model-agnostic 工具，所有 agent 统一走：

1. **`chat_with_format_json`** — schema-guided 输出。Ollama 后端加
   `format: <schema>`，OpenAI 兼容加 `response_format`，消除「自由文本 → 自己
   regex parse」的不确定性。
2. **`chat_with_retry`** — 重试-验证循环。LLM call → validator（JSON valid /
   字段全 / grounding）→ fail 时把 validator error 反馈回 LLM 重 call，最多 3 次；
   带退避。3 次后 fail 视为该 agent 在该 model 上不可用（graceful Err，不 panic）。
3. **`chat_few_shot`** — few-shot 上下文。≥2 个 worked example（含 1 个 edge case）
   进 system/user 消息，小模型必要、大模型无害。

配套：失败 telemetry 记录 `(agent × model)` 失败率，> 30% 时 UI 提示切高 tier；
ship 前跑 3-tier 兼容矩阵（弱本地 / 弱云 / 强云），F1 差 > 0.15 则 RELEASE.md
明示最低 tier。

## Consequences

**好处**：agent 可靠性从「Claude 上能跑」升级为「弱模型也有意义 degrade」；
新 agent 复用三工具，不再重复踩 JSON parse / 无 retry 的坑；telemetry 让用户
自助判断是否需要升 tier。

**代价**：每次 LLM call 增加 schema 约束 + 最多 3 次重试的 token/延迟开销；
3-tier 矩阵测试成本进入每个 agent 的 ship gate。

## Implementation 落地

- `LlmProvider` trait（attune-core）新增 3 方法；4 个 attune-pro agent refactor 接入。
- 失败 telemetry：`agent_telemetry.rs`（attune-core）。
- 规则全文固化于全局 CLAUDE.md「LLM Agent 兜底原则」+ 项目 CLAUDE.md
  「Agent 模型兜底」案例。
- 本 ADR 取代并归档设计 spec `2026-05-22-robust-llm-infra.md`（决策已落地）。

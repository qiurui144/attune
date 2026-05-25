# Robust LLM Infra — Model-Agnostic Agent 加固

**日期**：2026-05-22
**仓**：attune-core（LLM trait + 工具）+ attune-pro（4 个 agent refactor）
**版本目标**：v1.0.x（v1.0 GA 已 cut；本特性进入 v1.0.1 / v1.1）

## 0. TL;DR

在 `LlmProvider` trait 上增加 3 个 model-agnostic 工具
（`chat_with_format_json` / `chat_with_retry` / `chat_few_shot`），让任何 agent
不再为各家小模型（qwen2.5:3b / gemini-flash / phi3:mini）单独写 JSON / retry /
few-shot 硬化。挑 4 个 highest-impact LLM agent（defamation / divorce / fact /
self_evolving_skill）做 refactor。

## 1. 目标定位

**用户痛点**：
- 用户切换 LLM 后台（attune Pro Gateway → BYOK Gemini Flash → 本地 qwen2.5:3b），
  agent F1 大幅波动。每个小模型对 prompt / JSON / 标点的敏感度不同。
- 每个 agent 单独硬化 parser（per #58/#59/#64）是 O(N agents × M models) 重复工作。

**产品对齐**：
- 混合智能 / 分层成本契约：本地 LLM 是一等公民，不能假设"只 Claude/GPT-4 才能跑"
- Agent 验证铁律：framework 层硬化 = agent F1 1.00 pass rate 的基础设施

## 2. 范围边界

**做**：
- LlmProvider trait 上加 3 个 default 方法 + Ollama/OpenAI provider 各自重写
- 4 个 LLM agent refactor 用新工具
- mock provider 兼容（chat() fallback 路径不破坏 test）

**不做**：
- 不动 `defamation/extractor.rs` 直到 #64 merge（已有 working diff，避免冲突）
- 不引入新 LLM provider
- 不改 prompt 文案（prompt 调优是 #64 范畴）
- 不动 memory_consolidation（free-form prose 不需要 JSON）
- 不引入 streaming（per CLAUDE.md "Chat 流式输出" 决策）

## 3. 架构数据流

```
caller (agent)
  │
  ├─ chat()                ── single round trip,  free-form text
  ├─ chat_with_history()   ── multi-turn, free-form text
  ├─ chat_multimodal()     ── + image/file attachments
  │
  ├─ chat_with_format_json(system, user, schema?)
  │    │
  │    ├─ Ollama:  POST /api/chat  + format="json" or format=<schema>
  │    └─ OpenAI:  POST /v1/chat/completions  + response_format={type:"json_object"}
  │                                            or response_format={type:"json_schema",
  │                                              json_schema:{name,schema}}
  │
  ├─ chat_with_retry(system, user, validator, max_attempts=3)
  │    │
  │    ├─ attempt 1: chat() → validator → ok? return T
  │    ├─ attempt 2: chat_with_history([sys, user, prev_resp, "你的输出错在: {err},
  │    │                                请重新输出"]) → validator → ok? return T
  │    └─ attempt 3: same as 2
  │
  └─ chat_few_shot(system, user, [(user_ex1, asst_ex1), ...])
       └─ chat_with_history([sys, user_ex1, asst_ex1, ..., user])
```

## 4. 模块边界

| 文件 | 改动 |
|------|------|
| `rust/crates/attune-core/src/llm.rs` | +3 trait default methods + Ollama/OpenAI 重写 |
| `attune-pro/plugins/law-pro/src/divorce/extractor.rs` | call site refactor → `chat_with_format_json` |
| `attune-pro/plugins/law-pro/src/fact_extractor/mod.rs` | call site refactor → `chat_with_format_json` |
| `attune-pro/plugins/law-pro/src/defamation/extractor.rs` | **不动**（等 #64 merge） |
| `rust/crates/attune-core/src/skill_evolution/agent.rs` | `llm_expansion` 用 `chat_with_format_json`（输出 JSON array of strings） |

## 5. API 契约

```rust
// trait LlmProvider 新增 3 个 default 方法

/// Schema-guided JSON generation.
/// - schema=None  → 弱约束 "valid JSON object"（Ollama format="json" / OpenAI json_object）
/// - schema=Some  → 强约束 "valid JSON matching this schema"
///   (Ollama format=<schema>; OpenAI response_format.json_schema)
///
/// 返回的 string **保证 valid JSON**（除非 backend 不支持，此时 fallback 到 chat() + best effort）。
fn chat_with_format_json(
    &self,
    system: &str,
    user: &str,
    schema: Option<&serde_json::Value>,
) -> Result<String>;

/// Validation-loop retry. validator 返回 Ok(T) → 立即返回；Err(String) → 把错误
/// 信息 append 到 conversation 重新 call。最多 max_attempts 次。
fn chat_with_retry<T, V>(
    &self,
    system: &str,
    user: &str,
    max_attempts: usize,
    validator: V,
) -> Result<T>
where
    V: Fn(&str) -> std::result::Result<T, String>;

/// Few-shot context. examples 按顺序插入 user/assistant pair 在最终 user message 前。
fn chat_few_shot(
    &self,
    system: &str,
    examples: &[(String, String)],  // (user_ex, assistant_ex)
    user: &str,
) -> Result<String>;
```

## 6. 扩展点

- 第 4 个 provider（Anthropic native API）若引入，重写 `chat_with_format_json`
  走 Claude tool-use protocol 即可
- 新增 agent 默认走 default impl（chat fallback），按需要重写

## 7. 错误处理 / 边界 case

| 场景 | 行为 |
|------|------|
| validator 全 3 次失败 | 返回 `VaultError::Classification("LLM retry exhausted after N attempts: <last_err>")` |
| schema 非 None 但 provider 不支持 schema mode | Fallback 到 format=json（弱模式），不报错 |
| examples 为空数组 | 等价于直接 chat()，不报错 |
| schema 自身 invalid JSON | 调用方责任（不验证 schema 合法性） |

## 8. 成本契约

`chat_with_retry` 最多 3 次 LLM 调用。调用方需评估 budget；本特性不引入新成本层。

## 9. 测试矩阵

| 类型 | 覆盖 |
|------|------|
| Unit | format=json 两路 request 体格式 verify（serde_json snapshot） |
| Unit | retry：mock provider 2 次失败 + 第 3 次成功 → ok |
| Unit | retry：mock provider 全 3 次失败 → Err with "exhausted" |
| Unit | few-shot：messages 顺序验证（sys, ex1.user, ex1.asst, user_final） |
| Integration | 4 agent 跑现有 mock test 仍 pass |
| Real-LLM | qwen2.5:3b 跑 divorce/fact/self_evolving 3 agent F1 ≥ 现状（defamation 等 #64） |

## 10. 向后兼容

- 旧 `LlmProvider::chat()` / `chat_with_history()` / `chat_multimodal()` 签名不变
- 3 个新方法是 default impl on trait — 旧 provider 实现（外部 crate）零改动通过
- Mock provider 走 default fallback，旧 test 全 pass

## 11. 风险登记

| 风险 | 缓解 |
|------|------|
| OpenAI json_schema 模式各家网关支持参差 | 探测失败 → fallback json_object → 失败 → fallback free-form |
| Ollama format=<schema_object> 需 ollama >= 0.5 | 已是 dev 环境基线，K3 镜像同步升级 |
| validator 写错导致死循环 | 硬 cap max_attempts；validator 是同步函数无 IO |
| #64 同步改 defamation 冲突 | 本 spec 明确不动 defamation；#64 merge 后单独 PR refactor |

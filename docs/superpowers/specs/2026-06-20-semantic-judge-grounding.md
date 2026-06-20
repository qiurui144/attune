# Spec: Semantic-Judge Grounding for Multi-Source Synthesis (W5)

> Date: 2026-06-20 · Feature: `writing/grounding` LLM-judge fallback · Status: DRAFT (spec-first, §3.1)
> Closes the honest below-floor item: synthesis grounding **0.826 < 0.90 floor** while
> fact-consistency **1.000** (real-LLM N=3, deepseek-v4-flash/pro双探针持平 → 非模型能力问题).

## 1. 目标定位 (Goal)

attune 的写作引擎 W5（多源综述 map-reduce）对每个综述句做 **token-overlap 确定性 grounding 校验**。
真 LLM N=3 实测暴露一个**结构性假阴性**：abstractive（改写/概括式）综述句**确实有据**——它忠实复述了
某个 source 的观点——但与该 source span 的**词面重叠不足**被 token-overlap 校验判为 ungrounded。
前 agent 用 deepseek-v4-flash（0.826）与 deepseek-v4-pro（0.816）双 tier 探针**持平**，证明根因不是模型
能力差（pro 无增益，per §4.5H），而是**确定性校验器对改写句的召回上限**。

**用户痛点**：综述功能产出的句子明明有据，UI 却标 `[需核实]`，让用户误以为系统在编造 / 不可信，
削弱"source-attributable, fact-faithful"的产品定位（mod.rs 模块头）。

**本 slice 解法**：在确定性 token-overlap 校验**之后**，对**仅确定性判定为 ungrounded 的综述句**，
追加一个 **LLM-judge fallback**——judge 判断该句是否被某个候选 source span 语义支撑，且**必须**返回
source 里**真实存在**的 `evidence_quote`；代码侧**回链校验**该 quote 确实是 source 子串，否则**不 credit**。
这把 grounding **召回**从 token-overlap 上限提升到语义层，**同时**用回链校验把住"不编造"红线。

**与产品定位对齐**：grounding 是写作引擎的一等公民（mod.rs `GroundingRef`）；judge 升级**只能补回
真实有据句**，不能把无据句变有据——fact-consistency（不编造）是不可退让的第一指标。

## 2. 范围边界 (Scope)

**做（本 slice）**：
- 在 `writing/grounding.rs` 加 **可选** LLM-judge fallback：`ground_segments_with_judge(...)`，
  仅对 `ground_segments` 判 ungrounded 的句子跑 judge。
- judge 走共享 §4.5 hardened stack（schema-guided JSON + 重试≤3 + few-shot + PII redact）。
- **回链校验**：judge 的 `evidence_quote` 必须是某 source 的归一化子串才 credit（防误 credit 编造句）。
- 在 `synthesis.rs` 把 GROUND 步骤从纯确定性升级为"确定性 → judge fallback"，judge 是 💰 层、
  有调用上限（只对 ungrounded 句跑）。
- 纯确定性路径（`ground_segments`）**完全保留不变**，judge 是**叠加**而非替换。
- 测试矩阵：golden + proptest + 边界 + **对抗（喂编造句验 judge 不 credit）** + 回归；
  更新 `writing_real_llm_gate.rs` 的 synthesis 数据（不重复加 gate，不改 floor 常量）。

**不做（本 slice 明确排除）**：
- 不给 W1 draft / W2 rewrite 接 judge（它们 floor 已达标；judge 仅闭合 W5 below-floor）。本 slice
  judge 的 API 设计**通用**，未来可复用，但本 slice 只在 synthesis 接线。
- 不改 `GroundingConfig` 既有 token-overlap 阈值（min_overlap_tokens=3 等保持，ratchet 只升不降）。
- 不改 real-LLM gate 的 floor 常量（`GROUNDING_PRECISION_FLOOR=0.90` 留作硬门）。
- 不引入新 LLM provider；judge 复用调用方传入的 reasoning provider。
- 不做后台 judge（成本契约：judge 只在用户显式触发的 synthesize 动作内跑）。

**后续 v.next**：judge cache（按 (句, source-set) hash 缓存）；draft/rewrite 接 judge（若其 floor 后续抬高）。

## 3. 架构数据流 (Architecture & Data Flow)

```
synthesize() [W5 map-reduce, 💰]
  │
  ├─ MAP (cheap LLM, 1/source)  → per-source key points
  ├─ REDUCE (reasoning LLM, 1)  → thematic sections → content
  │
  └─ GROUND (确定性 → judge fallback)
        │
        ├─[1] ground_segments(segments, ground_sources, cfg)   // 🆓 纯确定性, 不变
        │        → 每句 verified=true/false; 返回 unverified_spans
        │
        └─[2] judge fallback (💰, 仅当存在 ungrounded 句 且 judge provider available)
                 for each seg where !verified AND is a factual claim:
                   ├─ 构造候选 spans: 把每个 source 切成窗口 (sentence-level), 标 span_id
                   ├─ judge LLM call (schema-guided, hardened ≤3 retry, few-shot):
                   │     in : { sentence, candidate_spans:[{span_id, text}] }
                   │     out: { supported: bool, span_id: string, evidence_quote: string }
                   ├─ 回链校验 (RELINK GUARD, 🆓 确定性, 成败关键):
                   │     supported==true
                   │  ∧  span_id ∈ 候选集
                   │  ∧  normalize(evidence_quote) 是 normalize(该 span 对应 source.text) 的子串
                   │  ∧  evidence_quote 长度 ≥ 阈值 (防单字/空串过关)
                   │     → 全真才 credit: seg.verified=true, push GroundingRef{kind=judge-elevated 标记}
                   │     → 任一假 → 不 credit, 句仍 unverified
                   └─ 累计 judge token_bill (judge_llm_tokens)
        │
        └─ 返回最终 unverified_spans (= 确定性 ungrounded − judge credited)
```

**DB tables**：无新增（grounding 是内存内计算，结果进 `WritingResult.segments` / `unverified_spans`）。
**cache layers**：本 slice 无 judge cache（v.next）。

**关键数据结构**：
- `JudgeConfig { enabled, max_judge_calls, min_evidence_chars, span_window_chars }`
- `JudgeVerdict { supported: bool, span_id: String, evidence_quote: String }`（judge JSON 反序列化）
- `GroundingRef` 复用既有结构；judge 升级的 ref 用 `overlap_tokens=0` + 一个语义标记区分（见 §5）。

## 4. 模块边界 (Module Boundaries)

| 文件 | 改动 |
|------|------|
| `rust/crates/attune-core/src/writing/grounding.rs` | **主改**：加 `JudgeConfig` / `JudgeVerdict` / `candidate_spans()` / `relink_verify()` / `ground_segments_with_judge()`；纯 `ground_segments` 不变 |
| `rust/crates/attune-core/src/writing/synthesis.rs` | GROUND 步骤改调 `ground_segments_with_judge`（judge provider = reasoning leg）；judge token 计入 `token_bill`；新增 `judge` 开关字段于 `SynthesisRequest` |
| `rust/crates/attune-core/src/writing/mod.rs` | re-export `JudgeConfig`；可能加 `GroundingKind::JudgeElevated` 或在 `GroundingRef` 加布尔标记 |
| `rust/crates/attune-core/tests/writing_real_llm_gate.rs` | synthesis 用例改走 judge 路径；更新注释/数据（不改 floor 常量）|
| `rust/crates/attune-core/tests/golden/writing_synthesis/cases.yaml` | 复用现有 corpus（GT 不变）|

**跨仓边界**：无。纯 OSS attune-core 内部，judge 复用既有 `LlmProvider` + `pii::llm_chat_redacted_hardened`。

## 5. API 契约 (API Contract)

```rust
/// judge fallback 配置。enabled=false ⇒ ground_segments_with_judge 退化为纯确定性。
pub struct JudgeConfig {
    pub enabled: bool,
    /// judge LLM 调用次数上限（💰 成本契约）。仅对确定性 ungrounded 的事实句计数。
    pub max_judge_calls: usize,
    /// evidence_quote 归一后最小字符数（回链校验，防单字/空串误 credit）。
    pub min_evidence_chars: usize,
    /// 候选 span 窗口（按句切 source；过长窗口稀释回链校验，过短丢上下文）。
    pub span_window_chars: usize,
}
impl Default for JudgeConfig { /* enabled:false (默认确定性), max_judge_calls:8, min_evidence_chars:6, span_window_chars:200 */ }

/// judge 的 schema-guided 输出。
struct JudgeVerdict { supported: bool, span_id: String, evidence_quote: String }

/// 确定性 grounding + 可选 judge fallback。judge_provider 仅在 cfg_judge.enabled 时被调用。
/// 返回最终 unverified_spans（确定性 ungrounded 减去 judge credited 的句）。
pub fn ground_segments_with_judge(
    segments: &mut [Segment],
    sources: &[SourceMaterial],
    cfg: &GroundingConfig,
    cfg_judge: &JudgeConfig,
    judge: Option<&dyn LlmProvider>,
) -> JudgeGroundOutcome;  // { unverified_spans: Vec<[u32;2]>, judge_calls: u32, judge_in/out tokens }
```

**judge LLM 契约**（schema-guided，hardened helper 驱动）：
- system：判断给定句是否被任一候选 span 支撑；必须引用 span 里**真实存在**的文字片段；不支撑则 supported=false。
- schema：`{supported: bool, span_id: string, evidence_quote: string}`，`additionalProperties:false`。
- few-shot：≥2，含一个"句子无据 → supported:false"的负例 + 一个"句子有据 → 引真实 quote"的正例。

**回链校验（确定性，judge 之外）**：credit 当且仅当
`v.supported && candidate_ids.contains(span_id) && norm(source_of(span_id)).contains(&norm(evidence_quote)) && norm(evidence_quote).chars().count() >= min_evidence_chars`。

## 6. 扩展点 / 插件接口 (Extension Points)

- judge API 通用：`ground_segments_with_judge` 不绑 synthesis；draft/rewrite 未来可同样调用。
- `JudgeConfig` 可调；未来加 judge cache 只需在 `ground_segments_with_judge` 入口加 (句, source-hash) lookup。
- judge provider 由调用方注入（synthesis 用 reasoning leg；未来可换更便宜的 judge-tier）。

## 7. 错误 + 边界 case (Errors & Boundaries)

- judge provider `None` 或 `!is_available()` → **跳过 judge**，退化为纯确定性结果（**绝不**因 judge 不可用而 fail synthesis）。
- judge JSON 三次重试仍非法 → 该句**不 credit**（保持 unverified），不 panic，不阻塞其余句。
- judge 返回 `span_id` 不在候选集 → 不 credit。
- `evidence_quote` 不是 source 子串（编造）→ **不 credit**（核心防线）。
- `evidence_quote` 过短（< min_evidence_chars）→ 不 credit。
- 达到 `max_judge_calls` 上限 → 剩余 ungrounded 句保持 unverified（成本封顶，不偷偷继续）。
- 非事实句（确定性已判 verified 的 non-claim）→ judge 不介入。
- judge 失败不改变 fact-consistency：judge 只可能把 unverified→verified（且必过回链），**永不**把 verified→unverified，也**永不** credit 无 source 子串的句。

错误码：synthesis 既有 `WritingError` 不变（judge 失败降级，不新增错误码）。

## 8. 成本契约 (Cost Contract)

- judge = **💰 tier-3**：只在用户显式触发的 `synthesize()` 内跑，**永不后台**（synthesis 本身已是 💰，judge 不改变层级）。
- judge 调用**有上限** `max_judge_calls`，且**只对确定性 ungrounded 的事实句**跑——不是每句、不是每 source。
  典型：综述 5-10 句，多数确定性即 grounded，judge 仅补 1-3 句。
- judge token 计入 `WritingResult.token_bill`（新增 judge leg 计数），UI 可显示 judge 成本。
- judge 默认 `enabled` 在 synthesis 路径开启（这是闭合 below-floor 的目的）；`JudgeConfig::default().enabled=false`
  保证库级默认不偷跑（调用方显式开）。

## 9. 测试矩阵 (Test Matrix)

| 类型 | 用例 | 工具 |
|------|------|------|
| Golden | 复用 `writing_synthesis/cases.yaml`（GT 人工标注，不变）| YAML + real-LLM gate |
| Property (≥3) | ① judge 永不把 verified→unverified；② judge credited 句必有过回链的 GroundingRef；③ disjoint-vocab + 伪造 quote → 永不 credit | proptest |
| 边界 (≥5) | judge None / judge unavailable / max_calls=0 / 非事实句 / evidence 过短 / span_id 越界 | inline `#[test]` |
| 异常/错误 (≥3) | judge JSON 非法×3 → 不 credit；judge supported 但 quote 非子串 → 不 credit；judge supported 但 span_id 不存在 → 不 credit | mock judge |
| **对抗 (核心)** | mock judge **故意**对编造句返回 `supported:true` + 伪造 `evidence_quote`（source 里没有）→ 回链校验**必须**挡住，句仍 unverified（GT 独立计算：编造句的 quote 不在任何 source）| mock judge + 独立 GT |
| 集成 E2E | real-LLM gate synthesis 用例走 judge 路径，N=3 | `writing_real_llm_gate.rs` |
| 回归 | 每个被修 case 进 golden；纯 `ground_segments` 既有 17+ 测试全过 | 既有 + 新增 |

**通过判据**：grounding ≥0.90 **且** fact-consistency 仍 1.0（real-LLM N=3, deepseek-v4-flash）。
fact-consistency 掉 = judge 误 credit = **失败**，必须加强回链直到回 1.0（宁可 grounding 不达标也不编造）。

## 10. 向后兼容 (Backward Compatibility)

- `ground_segments`（纯确定性）签名 + 行为**完全不变**——既有 17+ 测试 + draft/rewrite 路径零影响。
- `ground_segments_with_judge` 是**新增** API；`JudgeConfig::default().enabled=false` ⇒ 它在不传 judge 时
  行为 == `ground_segments`（叠加非替换）。
- `WritingResult` schema：judge-elevated 的 GroundingRef 仍是既有 `GroundingRef` 结构（加一个语义标记字段
  则走 `#[serde(default)]` 加 additive，`WRITING_SCHEMA_VERSION` 不必 bump——老 client 读到忽略未知/默认字段）。
- `SynthesisRequest` 新增 `judge` 开关字段走 `Default`，旧调用方（不设）默认开启 judge（这是修复目的）。
- real-LLM gate floor 常量**不动**（0.90 硬门），仅更新数据/注释。

## 11. 风险登记 (Risk Register)

| # | 风险 | 缓解 |
|---|------|------|
| A | **judge 误 credit 编造句**（成败关键）| 回链校验：evidence_quote 必为 source 归一子串 + 长度阈值；对抗测试故意喂伪造 quote 验证挡住；fact-consistency 必须保持 1.0 否则失败 |
| B | judge 自身 prompt-injection（source 里藏指令骗 judge）| synthesis 入口已 `source_has_injection_instruction` 预筛（judge 之前）；judge 只判"句↔span 支撑"，输出 schema-guided 受限 |
| C | judge 成本爆炸（每句一调）| `max_judge_calls` 上限 + 只对确定性 ungrounded 事实句跑 + token 计入 bill 可见 |
| D | judge 不可用导致 synthesis 失败 | judge None/unavailable → 降级纯确定性，绝不 fail；judge JSON 非法→不 credit 不 panic |
| E | judge 把 grounding 抬过头掩盖真编造 | judge 永不 verified→unverified；credit 必过回链；real-LLM gate fact-consistency 1.0 是独立硬约束 |
| F | tokenizer/grounding 阈值漂移 | `ground_segments` 阈值不动；judge 是叠加 OR，绝不降低既有 fabrication bar |
| G | 非确定性（judge 是 LLM）影响纯单测 | 纯单测用 mock judge（确定性回复）；real-LLM 路径在 `#[ignore]` gate，N=3 multi-seed 报 mean±std |

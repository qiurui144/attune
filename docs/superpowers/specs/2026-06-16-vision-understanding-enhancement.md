# OSS 视觉理解能力增强 — 稳定输出 + 多类扩展 + 可靠兜底（Vision Understanding Enhancement）

**Status**: DRAFT — 待用户评审（per `~/.claude/CLAUDE.md §3.1` 11 节门；本文档仅 PLANNING，**不实现**）
**Date**: 2026-06-16
**Owner**: attune 主仓（开源主线，Rust 商用线）
**Target Release**: v1.x（minor 切片在评审通过后由 `superpowers:writing-plans` 拆解；rc 阶段不进新 feature）
**Extends（增量增强，不重复已实现部分）**:
- `2026-06-10-document-nontext-content-recognition.md`（**已实现**：`ocr/nontext/` 7 类 region recognizer + schema-guided JSON + retry-validate ≤3 + per-call telemetry + VLM egress gate + cross-validation report）
- `2026-05-24-llm-vlm-multi-provider-architecture.md`（**已实现**：`VlmProvider` trait + `state.vlm()` accessor + settings VLM 节）
- `2026-05-28-kb-memory-vs-vlm-llm-bench-validation.md`（KB 端 eval harness；本 spec 把同等 multi-seed rigor 引到 **vision** 路径，该 spec §2.2 显式排除 VLM，本 spec 接棒）
- `docs/adr/0008-shared-visual-understanding-agent.md`（**已 accepted**：全产品唯一共享视觉核心，插件绝不自带视觉）

> 本 spec 是唯一交付物（R18：无单独 report）。所有改动在 `ocr/nontext/` + `vlm.rs` 既有结构上**扩展**，
> 不替换 `VlmProvider` trait、`RegionResult` schema、retry-validate 循环、telemetry record 任何既有契约。

---

## 0. 已实现 vs 本 spec 增量（核实现状，per §6.3 — 先 grep 再写）

> 下表每行基于 `grep -rn` 实测 `ocr/nontext/*.rs` + `vlm.rs` + `state.rs`（2026-06-16 在 develop 上），**不是推测**。
> 凡标 ✅ 已实现的，本 spec **不重做**；本 spec 只做 🆕 列。

| 能力 | 现状（已 ship） | 本 spec 增量（🆕） |
|------|----------------|---------------------|
| 7 类 region 检测 + ⚡ 本地识别 | ✅ `ocr/nontext/{layout,table_structure,chart,figure,formula,handwriting,stamp_signature,checkbox}.rs` | — 不重做；仅补 §1 的**输出 schema 准确性判据**（每类 golden 量化判据，落 §9） |
| schema-guided JSON + retry-validate ≤3 | ✅ `vlm_escalate.rs::{escalation_prompt, parse_vlm_answer, escalate_region}`（`MAX_RETRIES`，validator error 反馈回 prompt，从不编造）| — 不重做；本 spec 在其上加 **grounding validator**（§E2，新增一类 validate 失败原因）|
| per-call telemetry record | ✅ `vlm_escalate.rs::EscalateTelemetry { retry_count, error_kind, ... }`（每次 VLM call 一条）| 🆕 **聚合 + 阈值 + UI hint**：(kind×model) 失败率 > 30% → UI 提示切高 tier（§4.5 F 落地；现仅有 per-call record，无聚合/阈值/surface）|
| VLM egress gate + redact + 缩放 | ✅ `vlm_escalate.rs::{gate_vlm_egress, downscale_for_egress, VlmEgressToken}` | — 不重做 |
| cross-validation 纠错报告 | ✅ `mod.rs::OcrCorrectionReport / CorrectionEntry`（含 region-level `bbox`）| — 不重做 |
| `VlmProvider` trait + `state.vlm()` | ✅ `vlm.rs::VlmProvider{caption,vqa}` + `state.rs:2205 fn vlm()`（单 provider）| 🆕 **模型矩阵 failover**（§3.3）：💰 tier 候选注册表 + 健康探测 + 跨 qwen-3.6/3.7 failover（现 `state.vlm()` 是**单** `Option<Arc<dyn VlmProvider>>`，无候选/探测/failover；`layout.rs:22` 留 `TODO` 待接）|
| 💰 tier 默认模型 | ⚠️ `layout.rs:22-23` 注释 `TODO route 💰 to qwen3.6/3.7`（**未接线**）| 🆕 落地默认锁 qwen-3.6/3.7 多模态（§4.5H），矩阵 + degrade（§7） |
| grounding（VLM 抽取定位回原图/原文）| ❌ region 有 `bbox`，但 VLM 抽取的 **content**（cell 文本 / series 数值 / LaTeX）**无强制定位回 source**，无 grounding validator | 🆕 **grounding 契约 + validator**（§E2 / §5 `GroundingRef`）|
| multi-seed (N=3) real-VLM eval gate | ❌ 全无 vision eval harness（`grep eval` 仅命中 KB 端 chat.rs + TESTING.md）| 🆕 **N=3 real-VLM eval gate（F1≥floor）+ 3-tier 矩阵**（§9.2，复用 `2026-05-28` 的 multi-seed rigor 心智引到 vision）|
| 弱模型 degrade（VLM 不可用/能力不足）| ✅ 部分：VLM 不可用 → 保留 local + 标 `vlm_unavailable`（既有 §7） | 🆕 **能力分级 degrade**：弱 VLM F1 < floor → 该 kind 自动 disable + RELEASE.md 标最低 tier（§4.5 E 落地）|
| agent-invocable 共享视觉暴露 | ❌ 仅 REST `/api/v1/ocr/recognize` + CLI（`2026-06-10` §6.5 标的 impl delta 未补）| 🆕 **agent/capability invocation 入口**（§5.4，满足 ADR 0008「以可被 agent 调用的能力暴露」）|

**一句话定位**：检测/识别核心 + schema-guided + retry + telemetry record + egress gate **已 ship**；
本 spec 增量 = **稳定输出的最后一公里（grounding + N=3 eval gate）+ 可靠的最后一公里（模型矩阵 failover + 失败聚合阈值 + 能力分级 degrade）+ 共享 agent 的 agent-invocable 暴露面**。

---

## 1. 目标定位

### 1.1 用户痛点

`ocr/nontext/` 的检测与本地识别已 ship，但**视觉理解的「可信度」尚未闭环**，三类系统性问题仍在：

1. **输出不稳/不可信**：VLM 抽取（chart series 数值 / formula LaTeX / table cell 修正）虽有 schema-guided + retry，
   但 **同一张图跨次/跨 seed 结果可能漂**（VLM sampling），且抽出的值**无法定位回原图哪个像素区域 / 原文哪一行**
   —— 用户拿到「Q1=1.2M」但无从核验它来自图里哪根柱子，法律/财务证据场景不可接受（per `2026-05-28` 风险 A/C：无 seed → metric 全噪声）。
2. **可靠性不足**：💰 tier 当前是**单 `VlmProvider`**，该 provider 故障/限流/超时 → 整条 escalation 退化为纯 local；
   弱 VLM（qwen-3.6-flash）在 chart/formula 上可能 F1 远低于强 tier，却**默认启用**、出门让用户踩坑（违反 §4.5 E）。
3. **质量无据**：「视觉理解极致精度」是核心卖点，但当前**无 vision multi-seed eval gate** —— 任何精度 claim 都无 N seed × CI 支撑（同 `2026-05-28` 对 KB 端的判定，vision 端尚未补）。

**核心命题**：检测/识别已 ship，下一步是**让视觉输出「稳、可定位、可靠兜底、有据」**——
schema-guided 之上加 **grounding（定位回原图/原文）+ N=3 real-VLM eval gate**；单 provider 之上加 **qwen-3.6/3.7 模型矩阵 failover + 能力分级 degrade + 失败聚合阈值**。

### 1.2 产品定位对齐

- **混合智能 / 本地优先**：增量全部叠在「⚡ local 先行 → 💰 仅分歧/低置信 region 升级」既有路径上；failover/grounding 不改变 local-first，不增加默认 💰 调用量。
- **成本感知最高优先**（§成本契约）：N=3 eval 是**离线 gate**（开发/ship 前跑，非用户请求路径）；grounding 是 🆓 比对逻辑；failover 仅在主 provider 失败时切候选，不增量盲送。
- **降低 token + 数据安全**（M2）：增量不新增出网点；failover 候选仍走既有 `gate_vlm_egress`（redact + 缩放 + outbound gate 非 no-op，MEMORY P0）。
- **共享视觉 agent（ADR 0008）**：本 spec 是这**唯一**共享视觉核心的稳定性/可靠性增强 + 补齐 agent-invocable 暴露面；增量全部留在 OSS attune base，行业插件**消费**之，绝不自带视觉。
- **模型选型默认（§4.5H）**：💰 tier 多模态默认锁 **qwen-3.6 / qwen-3.7**（qwen-vl 已下架禁用）；不掺 claude/gpt 生产 token。

### 1.3 与全局规则映射

| 规则 | 本 spec 对齐点 |
|------|----------------|
| §3.1 11 节 spec-first | 本文档；评审通过才进 writing-plans |
| §4.5 A schema-guided | ✅ 已 ship（`vlm_escalate.rs`）；本 spec 不重做，加 grounding validator（§E2）|
| §4.5 B 重试-验证 ≤3 | ✅ 已 ship；本 spec 在 validator 增 grounding 失败类（§E2）|
| §4.5 D 多模型矩阵 | 🆕 N=3 + 3-tier vision 矩阵（§9.2）|
| §4.5 E 弱模型 fallback | 🆕 能力分级 degrade + RELEASE.md 标最低 tier（§7）|
| §4.5 F 失败 telemetry | 🆕 per-call record（已 ship）→ **聚合 + 阈值 + UI hint**（§7）|
| §4.5 H 模型选型默认 | 🆕 💰 tier 默认 qwen-3.6/3.7（§3.3 / §4.5H）|
| §成本契约 三档 | §8（N=3 eval 离线、grounding 🆓、failover 不增量盲送）|
| §6.1 6 类下限 + §2.3 multi-seed | §9（各非文字类型 golden + N=3 real-VLM）|
| §6.3 baseline SOP | §9.3 量化判据 + raw log + 阈值校准 |
| ADR 0008 共享视觉 agent | §5.4 agent-invocable 暴露 + §X-Cutting 边界不变量 |

---

## 2. 范围边界

### 2.1 In Scope（本能力做什么 — 5 个增量）

**I1. Grounding 契约 + validator（稳定输出）**
- VLM 抽取的每个 content 单元（cell 文本 / series 数据点 / LaTeX token 串 / caption / 手写转写 / 印章文字）
  必须携带 **`GroundingRef`**：定位回 **(a) source region bbox + page**（必填，区域级已有）+ **(b) 可选 sub-bbox / OCR line ref**（精化到原图局部 / 原文 OCR 行）。
- 新增 **grounding validator**（接到既有 retry-validate 循环）：VLM 输出若声称一个值却给不出可定位的 source ref，或 ref 越界 / 指向空白区 → 判 `grounding_fail`，反馈回 prompt 重试（≤3，复用既有 `MAX_RETRIES`）。
- 用户/下游可用 `GroundingRef` 在 UI 高亮原图对应区域（定位回原图）/ 跳转原文 OCR 行（定位回原文）。

**I2. N=3 real-VLM eval gate（稳定输出，离线 gate）**
- 新增 vision eval harness：对每个 VLM-escalation kind（chart series / formula LaTeX / table 修正 / caption / handwriting / stamp text）
  跑 **N=3 seed**（per §2.3 / §4.5 D），报 mean ± std；F1 / 准确率 **< floor** → gate FAIL（该 kind 在该 model 上不可 ship 默认启用）。
- gate 是**离线**（ship 前 / develop→main 前跑，复用 `2026-05-28` rigor 心智），**非用户请求路径**。

**I3. VLM 模型矩阵 failover + 默认锁 qwen-3.6/3.7（可靠）**
- 💰 tier 从**单 provider** → **候选注册表 + 健康探测 + 优先级 failover**（layout.rs:22 TODO 落地）：
  主 provider 失败（不可达 / 5xx / 超时 / 限流 / 能力不支持）→ 按优先级切下一候选；全失败 → degrade 到纯 local（既有 §7 路径不变）。
- 默认候选锁 **qwen-3.6 / qwen-3.7 多模态**（per §4.5H；DashScope）；env 可切 OpenAI-compat（`*_BASE_URL`/`*_MODEL`/`*_API_KEY`），默认值锁这两者。

**I4. 弱模型能力分级 degrade + 失败聚合阈值（可靠）**
- per-call telemetry（已 ship）→ **聚合**到 `(region_kind × model)` 失败率；> 30% → **UI hint「建议切高 tier 模型」**（§4.5 F 落地）。
- 依 §9.2 3-tier 矩阵结果：弱 tier 某 kind F1 < floor → 该 (kind, weak-model) 组合**自动 disable**（degrade 到纯 local 或拒绝该 kind 的 💰 升级），RELEASE.md 标该 kind 最低 tier（§4.5 E 落地）。

**I5. agent-invocable 共享视觉暴露（ADR 0008 impl delta）**
- 补 `2026-06-10` §6.5 标的 impl delta：新增 **agent/capability invocation 入口**（复用既有 agent dispatch / capability registry 机制），
  插件经 attune agent 调用机制直接 invoke 拿结构化 `RegionResult` / `RecognizePageResult`（输出即既有 typed schema），不必经 REST 自调。

### 2.2 Out of Scope（不做 / 推后 v.next）

- ❌ **重做检测/识别核心**：7 类 recognizer + schema-guided + retry + egress gate **已 ship**，本 spec 不动。
- ❌ **新增 region 类型**：维持既有 7 类 + 既有 `RegionKind`；新增类型 = 新 spec。
- ❌ **训练自有视觉模型 / 自有 VLM**：只用现成 qwen-3.6/3.7 + 现成 local ONNX；选型实测在 plan 阶段。
- ❌ **VLM provider 自身 non-determinism 根治**：跨 day 模型漂是上游问题（per `2026-05-28` 风险 C），只 mitigate（seed/temp0 透传 + manifest record fingerprint + 本地候选作 canonical baseline），不承诺消除。
- ❌ **行业语义判定**（印章合规 / 合同条款表 / 病历表语义）：→ attune-pro 插件，消费共享 agent 叠语义（§X-Cutting）。
- ❌ **视频/音频 modality**：ASR 另有链路；本 spec 仅文档图像/PDF。
- ❌ **chart 复杂统计反演**（误差棒/对数轴/多 y 轴）：v1 仅主流柱/折线/饼近似 series（沿 `2026-06-10` 边界）。
- ❌ **手写数学公式 → LaTeX**：印刷公式在 R4 in scope（已 ship），手写公式推 v.next。
- ❌ **BYOK 多 VLM 用户侧显式 fallback 配置 UI**：本 spec failover 是 💰 tier **内部**候选矩阵；用户侧多 provider 配置 UI 沿 `2026-05-24` v1.2 backlog。
- ❌ **自动写回 vault 原文**：grounding/纠错是**建议**，用户显式 accept 才落（沿既有语义）。

### 2.3 Scope 锁定声明

本能力 = **I1 grounding 契约+validator + I2 N=3 real-VLM eval gate + I3 模型矩阵 failover（默认 qwen-3.6/3.7）+ I4 弱模型分级 degrade + 失败聚合阈值 + I5 agent-invocable 暴露**。
任何超出（新 region 类型 / 自有模型训练 / 行业语义 / 视频 / 用户侧多 VLM 配置 UI）= 新 feature，需新 spec，
**不允许在 implementation 中 silent scope creep**。

---

## 3. 架构数据流

### 3.1 增量数据流（ASCII，🆕 标本 spec 新增；其余为既有 ship 阶段）

```
输入文档 (PDF/PNG/JPG)
   │
   ▼
[已 ship] Stage1 layout 检测 → Stage2 ⚡ local 识别 → Stage3 🆓 cross-validate
   │ (escalate 候选: 低置信 / 冲突 / 分歧 region)
   ▼ (仅当 governor 允许 💰 + VLM 可用)
┌──────────────────────────────────────────────────────────────────────────┐
│ Stage4 — VLM 升级仲裁 (💰)                                                 │
│                                                                            │
│  [已 ship] gate_vlm_egress (redact + 缩放 + outbound gate)                 │
│       │                                                                    │
│       ▼                                                                    │
│  🆕 VlmRouter::select() ── 候选注册表 + 健康探测 + 优先级 ───┐             │
│       │  primary: qwen-3.7-multimodal (DashScope)             │  I3        │
│       │  fail(5xx/timeout/limit/capability) → next:           │ failover  │
│       │  qwen-3.6-multimodal → ... → 全失败 → degrade local   │            │
│       ▼                                                       │            │
│  [已 ship] escalation_prompt (schema-guided JSON)             │            │
│       │                                                       │            │
│       ▼                                                       │            │
│  [已 ship] parse_vlm_answer → validator ── 失败 ─┐            │            │
│       │                                          │ retry ≤3   │            │
│       │  🆕 + grounding validator (I1):          │ (既有循环) │            │
│       │     抽取值是否带可定位 GroundingRef?      │            │            │
│       │     ref 越界 / 指向空白 / 缺失 → grounding_fail ──────┘            │
│       ▼ (valid + grounded)                                                 │
│  RegionResult { ..., grounding: Vec<GroundingRef> }  🆕                    │
│       │                                                                    │
│       ▼                                                                    │
│  🆕 telemetry 聚合 (I4): record → (kind×model) 失败率                      │
│       └─→ > 30% → UI hint「建议切高 tier」                                 │
└──────────────────────────────────────────────────────────────────────────┘
   │
   ▼
合并 → RecognizePageResult.regions[] (含 grounding) + OcrCorrectionReport
   │
   ├─→ REST /api/v1/ocr/recognize (已 ship)
   ├─→ CLI (已 ship)
   └─→ 🆕 agent-invocable capability (I5): 插件 invoke 拿 typed RegionResult


离线（非请求路径，ship 前 / develop→main 前跑）:
┌──────────────────────────────────────────────────────────────────────────┐
│ 🆕 Vision Eval Gate (I2)                                                    │
│   golden fixtures (每 kind ≥10 人工标注 GT)                                │
│      │  FOR model IN {qwen-3.6-flash, qwen-3.6-plus, qwen-3.7-max}          │
│      │    FOR seed IN {0,1,2}  (N=3)                                       │
│      │      run escalate → F1 / 准确率                                     │
│      ▼                                                                      │
│   aggregate mean ± std per (kind × model)                                  │
│      │  F1 < floor → gate FAIL (该 kind 该 model 不 ship 默认启用)         │
│      │  tier 差 > 0.15 → RELEASE.md 标最低 tier                            │
│      ▼                                                                      │
│   reports/runs/<ts>/vision-eval.json (raw log, §6.3)                       │
└──────────────────────────────────────────────────────────────────────────┘
```

### 3.2 模型矩阵（💰 tier 候选 — 默认锁定，per §4.5H）

> 候选清单 + 优先级；最终 priority/默认值在 plan 阶段按 §调研纪律实测（§9.2 3-tier 矩阵）后定，不在 spec 拍死准确率。

| 优先级 | 候选 model | 后端 | 角色 | tradeoff |
|--------|-----------|------|------|----------|
| 9（primary） | `qwen-3.7-max`（多模态） | DashScope | 强 tier 上限对照 + 默认强精度 | 最贵、最准；chart/formula 首选 |
| 7 | `qwen-3.7-plus` / `qwen-3.6-plus` | DashScope | 平衡 tier | 性价比；多数 kind 够用 |
| 5 | `qwen-3.6-flash` | DashScope | 弱云端 tier | 便宜；§9.2 矩阵决定哪些 kind 它 F1 < floor 须 disable |
| —（env 覆盖） | OpenAI-compat `*_MODEL` | `*_BASE_URL` | BYOK / 自定义 | env 显式切；默认值不锁这里（per §4.5H 默认锁 qwen-3.6/3.7）|

- **禁用 qwen-vl**（已下架，§4.5H 反模式）。
- **不掺 claude/gpt 生产 token**（§4.5H）；强 tier 对照仅在 §9.2 离线 eval 可临时配作上限参照，**不进生产默认**。
- 健康探测：`VlmRouter` 启动 + 周期 probe 各候选 `is_available()`（既有 `VlmProvider` 已有，可扩 cheap probe）；不可用候选跳过。

### 3.3 缓存层（复用既有 + 增量）

| 缓存 | key | 复用/新增 |
|------|-----|----------|
| VLM 仲裁缓存 | `sha256(region_crop) + vlm_model_name + prompt_ver` | ✅ 既有（`2026-06-10` §3.3）|
| 🆕 grounding 不进 key | grounding 是同一次抽取的副产物，随仲裁结果一起缓存 | failover 切 model → cache key 含 model_name，天然不串 |
| 🆕 vision-eval 结果 | `reports/runs/<ts>/`（离线，不进请求缓存）| 新增（§6.3 raw log）|

failover 切到不同候选 → cache key 的 `vlm_model_name` 不同，**不会**串用另一 model 的缓存（正确隔离）。

---

## 4. 模块边界

### 4.1 新增 vs 扩展（全部在 attune-core，遵 wasm-safe：native dep 留 native 侧 `#[cfg(feature="nontext")]`）

```
crates/attune-core/src/ocr/nontext/
├── vlm_escalate.rs        [ext] escalate_region 接 VlmRouter（替换直持单 Arc<dyn VlmProvider>）；
│                                validator 链增 grounding_fail 分支（接既有 retry 循环）
├── mod.rs                 [ext] RegionResult 各变体增 grounding 字段（向后兼容，§10）；
│                                RecognizePageResult 增聚合 telemetry 视图
├── vlm_router.rs          [new] I3: 候选注册表 + 健康探测 + 优先级 failover（VlmRouter）
├── grounding.rs           [new] I1: GroundingRef 类型 + grounding validator（🆓 比对逻辑）
└── eval/                  [new] I2: vision eval harness（离线，feature-gate "vision-eval"）
    ├── mod.rs             [new] golden 加载 + N=3 runner + F1/准确率 + mean±std + gate verdict
    └── metrics.rs         [new] per-kind 准确性判据（cell-F1 / series 相对误差 / LaTeX 编辑距离 / 二分类 acc）

crates/attune-core/src/
├── vlm.rs                 [ext] VlmProvider 增 cheap `probe()` default impl（供 router 健康探测）；
│                                trait 本身不破坏（default 实现，§10）
└── telemetry/ (或既有位置) [ext] I4: (kind×model) 失败率聚合器 + 30% 阈值 surface

crates/attune-server/src/
├── routes/ocr.rs          [ext] recognize 响应回传 grounding + telemetry-hint 字段（新增可选字段，§10）
├── state.rs               [ext] state.vlm() 单 Option → 注入 VlmRouter（accessor 兼容：
│                                state.vlm() 仍返回当前 primary，新增 state.vlm_router()）
├── routes/<capability>.rs [ext] I5: agent-invocable 入口（复用既有 agent dispatch / capability registry）
└── routes/cost.rs         [reuse] failover 不增量盲送，成本估算逻辑不变
```

### 4.2 跨模块依赖

- `vlm_router.rs` 依赖既有 `vlm::VlmProvider` + 既有 `gate_vlm_egress`（出网仍经 gate，非 no-op，MEMORY P0）+ governor。
- `grounding.rs` 依赖 `ocr::{BBox, RawLine}`（定位回 OCR 行）+ region crop 坐标（定位回原图）—— 纯 🆓 逻辑，可单测全覆盖。
- `eval/` feature-gate `vision-eval`（默认关，仅离线/CI），依赖 golden fixtures + 真 VlmRouter（real-VLM，per §9.2 ship 前必跑）。
- I5 agent 入口复用既有 agent dispatch / capability registry（**不新建** agent 框架）；输出即既有 typed `RegionResult`。

---

## 5. API 契约

### 5.1 REST（既有入口增可选字段，向后兼容 §10）

`POST /api/v1/ocr/recognize`（既有）响应 `RegionResult` 增 grounding；新增顶层 telemetry hint：

```jsonc
{
  "regions": [
    {
      "kind": "chart", "bbox": {...}, "page": 0, "source": "vlm", "confidence": 0.88,
      "result": {
        "schema": "chart_v1", "chart_type": "bar",
        "series": [
          { "name": "Q1", "value": 1.2e6,
            "grounding": { "region_bbox": {...}, "page": 0,        // I1: 定位回原图
                           "sub_bbox": {...},                       // 可选: 精化到该柱
                           "ocr_line_ref": "line:12" } }            // 可选: 定位回原文 OCR 行
        ]
      }
    }
  ],
  "vlm_hint": { "suggest_higher_tier": false, "kind_failure_rates": { "chart:qwen-3.6-flash": 0.12 } } // I4
}
```

无 grounding 字段（local-only / 旧响应）= 既有行为。`vlm_hint` 缺省 = 无聚合数据。

### 5.2 CLI（既有命令增 flag）

```
attune-cli ocr recognize <file> [...既有...] [--show-grounding]   # 打印每抽取值的 source ref
attune-cli ocr eval --kinds chart,formula --seeds 3 [--tiers qwen-3.6-flash,qwen-3.7-max]  # I2 离线 gate
```

`ocr eval` 仅 feature `vision-eval` 编译时存在；生产 binary 不含（同 `2026-05-28` eval-mode 安全边界心智）。

### 5.3 Typed Schema（serde；增量字段全部可选/带 default，§10）

```rust
// ocr/nontext/grounding.rs  — I1
#[derive(Serialize, Deserialize, Clone)]
pub struct GroundingRef {
    pub region_bbox: BBox,                 // 必填: 抽取值所属 region (已有区域级 bbox)
    pub page: u32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub sub_bbox: Option<BBox>,            // 可选: 精化到原图局部 (柱/cell/公式片段)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ocr_line_ref: Option<String>,      // 可选: 定位回原文 OCR 行 (RawLine 索引/id)
}

// grounding validator 失败原因 (接既有 retry-validate 循环的 error_kind)
pub enum GroundingFail {
    Missing,          // 声称有值但无 GroundingRef
    OutOfBounds,      // ref 越出 region/page 边界
    EmptyArea,        // ref 指向空白/无内容区域
}

// RegionResult 各变体增 grounding (示例; 全部 #[serde(default)])
//   ChartV1.series[].grounding: GroundingRef
//   TableV1.cells[].grounding: Option<GroundingRef>
//   FormulaV1.grounding: Option<GroundingRef>
//   ... (HandwritingV1 / StampV1 / FigureV1 caption 同)

// ocr/nontext/vlm_router.rs  — I3
pub struct VlmCandidate { pub provider: Arc<dyn VlmProvider>, pub model: String, pub priority: u8 }
pub struct VlmRouter { /* 候选注册表 + 健康探测 */ }
impl VlmRouter {
    pub fn select(&self) -> Option<&VlmCandidate>;   // 按优先级 + 健康
    pub fn on_failure(&self, model: &str, kind: FailKind);  // 标记 + 触发 failover
}

// I2 eval (feature "vision-eval")
pub struct VisionEvalVerdict {
    pub per_kind_model: Vec<KindModelScore>,   // {kind, model, f1_mean, f1_std, n_seed}
    pub gate_pass: bool,                        // 任一默认启用组合 F1 < floor → false
    pub min_tier_per_kind: HashMap<RegionKind, String>,
}

// I4 telemetry 聚合
pub struct VlmFailureAggregator { /* (kind×model) → 失败率 */ }
impl VlmFailureAggregator {
    pub fn record(&self, kind: RegionKind, model: &str, ok: bool);
    pub fn should_suggest_higher_tier(&self, kind: RegionKind, model: &str) -> bool; // > 30%
}
```

### 5.4 agent-invocable 暴露（I5，ADR 0008 impl delta）

复用既有 agent dispatch / capability registry，暴露一个 capability（**非新 agent 框架**）：

```
capability: vision.recognize
  input:  { item_id | image_bytes_b64, kinds?, vlm_escalation? }
  output: RecognizePageResult (即 §5.3 typed schema, 含 regions[].grounding)
  调用方: 任意插件 (OSS / pro) 经 attune agent/capability 调用机制 invoke
  约束:  输出零行业绑定; 行业语义由调用插件叠加 (§X-Cutting)
```

短期插件可经 REST `/api/v1/ocr/recognize` 自调过渡；agent-invocable 是 ADR 0008 目标形态，本 spec 列为必交付（plan 文件清单显式列出）。

---

## 6. 扩展点 / 插件接口

### 6.1 加一个新 💰 候选 model

1. `vlm_router.rs` 注册表加 `VlmCandidate { provider, model, priority }`（默认锁 qwen-3.6/3.7，新增 BYOK 走 env 覆盖）。
2. 自动进入健康探测 + 优先级 failover 序列。
3. 跑 §9.2 3-tier 矩阵确认该 model 各 kind F1 ≥ floor；< floor 的 kind 标 disable（§7 I4）。

### 6.2 加一个新 grounding source 类型

- `GroundingRef` 加可选字段（如 `pdf_obj_ref`），保持 `#[serde(default)]` 向后兼容；grounding validator 加对应越界检查分支。

### 6.3 加一个新 VLM 后端

- 实现既有 `VlmProvider` trait（`caption`/`vqa` + 新 `probe()` default）即插入 router 注册表；**不改 trait 签名**（§10）。

### 6.4 插件消费共享视觉（ADR 0008）

- 插件经 §5.4 `vision.recognize` capability 拿 typed `RegionResult`，通过既有 `RegionRecognizer` 扩展点（`2026-06-10` §6.4）叠加**语义解释**；**绝不**在插件内实现 VLM/视觉调用（ADR 0008 硬约束，评审须验）。

---

## 7. 错误处理 + 边界 case

| 场景 | 处理 | 错误码 / 退出 |
|------|------|--------------|
| **VLM grounding 缺失/越界/空白** | grounding validator 判 `grounding_fail`，反馈回 prompt 重试（≤3，复用既有循环）；3 次仍失败 → 保留值但标 `validation_warnings:["ungrounded"]`，**不丢值不编造** | 200 + telemetry |
| **primary VLM 失败（5xx/超时/限流/不可达）** | `VlmRouter::on_failure` 标记 + 切下一候选；记 failover telemetry | 200（用户无感）|
| **全部候选失败** | degrade 到纯 local（既有路径），region.source=Local + 标 `vlm_unavailable`；**绝不 panic** | 200 + telemetry |
| **provider 能力不支持 vision（误配 text-only model）** | router 探测/调用即判 capability fail → 切候选；全不支持 → degrade | 200 + telemetry |
| **弱 model 某 kind F1 < floor（§9.2 已知）** | 该 (kind, weak-model) **自动 disable** 💰 升级（degrade 纯 local），UI hint 建议切高 tier | 200 |
| **(kind×model) 累计失败率 > 30%** | UI hint「建议切高 tier 模型」（§4.5 F）；不阻塞主流程 | 200 |
| **N=3 eval gate FAIL（离线）** | 该 kind 该 model 不 ship 默认启用；RELEASE.md 标最低 tier；CI gate 红 → 不 tag（§9.2）| 非请求路径，gate verdict |
| **vision-eval 跑在生产 binary** | feature `vision-eval` 默认不编译进生产；若误启 → 启动检查 panic（同 `2026-05-28` eval-mode 边界）| exit 1 |
| **failover 切 model 后 cache 不串** | cache key 含 model_name，天然隔离（§3.3）| — |
| **grounding ref 指向已被 redact 区域** | egress redact 在 gate（既有），grounding 指原图坐标非 redacted 副本；validator 用原图坐标系校验 | 200 |

**失败 telemetry（I4 聚合，§4.5 F 落地）**：per-call record（已 ship）→ 聚合 `{region_kind, vlm_model, error_kind(parse/grounding/timeout/capability/gate), retry_count, failover_used}`；
(kind×model) 失败率 > 30% → UI hint。**绝不静默 swallow → None**。

---

## 8. 成本契约

### 8.1 三档映射（per §成本与触发契约 — 增量全部不抬高默认 💰 量）

| 档 | 本 spec 增量哪部分 | 触发 |
|----|---------------------|------|
| 🆓 **零成本** | grounding validator 比对（I1）、telemetry 聚合（I4）、failover 决策逻辑、cross-validate（既有） | 随既有链路跑 |
| ⚡ **本地算力** | local 识别（既有，不变）、健康探测 cheap probe（I3，可纯 HTTP HEAD 级） | 建库阶段自动 |
| 💰 **时间/金钱** | VLM 升级仲裁（既有，**量不变**）；failover 仅在主 provider 失败时切候选（**不并发盲送多 model**）；N=3 eval 是**离线** gate | 既有「仅低置信/分歧 region」触发；eval 是 ship 前离线，非用户路径 |

**核心不变量（增量不破坏既有成本契约）**：
1. **建库阶段永不升 💰**（既有）；增量不改。
2. **failover ≠ 并发多送**：一次只调一个候选，失败才切下一个；典型「1 region 1 次 VLM 调用」，failover 只在故障时多 1-2 次（不是每次都 fan-out）。
3. **N=3 eval 离线**：开发/CI/ship 前跑，**计入开发成本而非用户账单**；启动须 `--confirm-cost`（同 `2026-05-28` §8.2 心智），real-VLM call 走授权（§1.3 算力授权）。
4. **grounding 不额外调 VLM**：是同一次抽取要求 VLM 在 schema 里一并给出 source ref（prompt 内要求），不新增 round-trip。
5. **UI 成本显示**（既有）+ failover 时 UI 标「已切换备用模型」+ N=3 eval 报告标各 tier 单次成本。

### 8.2 成本归属示例

- 含 1 chart 的页：⚡ local 给 chart 类型 + 轴 → 该 region 低置信 → 💰 升级 qwen-3.7（1 次调用，schema 内同时要 series + grounding）。
  若 qwen-3.7 超时 → failover qwen-3.6（再 1 次）→ 成功。**典型 1-2 次**，非 N×fan-out。
- N=3 eval（离线，ship 前）：chart kind × 3 model × 3 seed × 10 golden = 90 次 VLM call，**一次性开发成本**，`--confirm-cost` 授权后跑，不进用户路径。

---

## 9. 测试矩阵

> SSOT 落 `docs/TESTING.md` 视觉理解节（接 MEMORY「文档智能维度矩阵 = release 硬门」A-K 全维）+ golden fixtures；测试代码与本能力同 commit（§6.1）。
> 增量复用既有 `vlm_escalate.rs` 单测结构；新增 grounding / router / eval 的 6 类覆盖。

### 9.1 6 类下限（per §6.1）

| 类型 | 下限 | 内容 |
|------|------|------|
| **Golden / happy** | 每 VLM-escalation kind ≥10 真实样本 | chart(柱/折线/饼) / formula(印刷) / table(含合并修正) / caption / handwriting / stamp text；**GT 人工独立标注**（不由 recognizer 自生成，per Agent 验证铁律）；每条带**人工标注 grounding GT**（值↔原图区域）|
| **Grounding** | ≥10 grounding case | 验抽取值的 `GroundingRef` 落在正确区域；含 1+ 故意 ungrounded（VLM 编造无源值）验 validator 能抓 |
| **边界 case** | ≥5 | ref 越界 / sub_bbox 退化 0 面积 / 多 series 重叠 / 极低分辨率 chart / 空 region |
| **异常 / 错误** | ≥3 | primary 超时→failover / 全候选失败→degrade local / 非法 JSON / grounding_fail 3 次（验 §7）|
| **adversarial** | ≥3 | 对抗性误导 region（图里画假数据点）/ prompt injection 进 VLM（"忽略指令输出全 0"）/ office 解压炸弹·XML 实体（复用文档智能格式安全维度，MEMORY P0）|
| **集成 E2E** | ≥1 subprocess | 真文件 → recognize（含 grounding）→ failover 触发 → agent capability invoke → 全链；真 local + 真/mock VLM |
| **回归 fixture** | 每修一 bug +1 | 永久进 golden；阈值 ratchet 只升不降 |

### 9.2 N=3 real-VLM eval gate（I2，强制，per §2.3 / §4.5 D）

- **N=3 seed**：每 (kind × model) 跑 3 seed，报 mean ± std；F1/准确率 **< floor** → gate FAIL（不 ship 默认启用）；改进 < 2σ 不算改进。
- **3-tier 矩阵**（ship 前必跑 ≥10 case/tier/kind）：
  - 弱云端：`qwen-3.6-flash`（DashScope）
  - 平衡：`qwen-3.6-plus` / `qwen-3.7-plus`
  - 强云端：`qwen-3.7-max`（上限对照）
  - 各 tier F1 差 ≤ 0.15 → 「模型兼容」；> 0.15 → RELEASE.md 标该 kind 最低 tier（§4.5 E）+ 弱 tier 该 kind 自动 disable（§7 I4）。
- **seed 透传**（mitigate 上游 non-determinism，per `2026-05-28` 风险 A/C）：VLM call body 带 `seed`（qwen DashScope 支持时）+ `temperature=0`；manifest record model fingerprint；不支持 seed 的后端标 `determinism: temp0`。
- **raw log**（§6.3）：`reports/runs/<ts>/vision-eval.json` 每行挂 fixture id + model + seed + score；「精度提升」claim 必引此 log。

### 9.3 精度判据（量化，非主观，per §6.3）

- chart series：数值相对误差 < 阈值（plan 阶段 golden 校准）；grounding：series 点定位回正确柱/扇区 IoU ≥ 阈值。
- formula：LaTeX 渲染图相似度 / token 编辑距离。
- table 修正：cell-level F1（含合并 cell 正确率），交叉校验后 vs 单 PP-OCR baseline **实测有提升**（沿 `2026-06-10` R3）。
- caption / handwriting / stamp text：参考答案 ROUGE-L / 字符准确率。
- 二分类（印章存在 / checkbox）：accuracy / recall。
- **「极致/稳定」claim 必须 N=3 + baseline 对照 + raw log；阈值 τ/floor 必须 golden 校准非拍脑袋。**

---

## 10. 向后兼容

### 10.1 既有契约不破坏

- `VlmProvider` trait：新增 `probe()` 给 **default impl**（调既有 `is_available()`）→ 现有 `LlmVlmProvider`/`MockVlmProvider`/`RecordingMockVlm` 不改即编译通过；`caption`/`vqa` 签名不变。
- `RegionResult` 各变体增 grounding 字段：全部 `#[serde(default, skip_serializing_if)]`；旧序列化数据（无 grounding）反序列化为 `None`/空 → 等价旧行为。
- `state.vlm()` accessor 保留（返回当前 primary）；新增 `state.vlm_router()`；老 caller 不改。
- `RecognizePageResult` / `OcrCorrectionReport` 仅**增**可选字段（`vlm_hint`），既有字段语义不变。
- `parse_vlm_answer` / `escalate_region` / `EscalateTelemetry` / `gate_vlm_egress` 签名保留；grounding validator 接入是**内部**链路扩展，对外 API 不变。

### 10.2 Schema versioning

- `GroundingRef` 随所属 `RegionResult` 的 `*V1` 版本走；未来演进加 `V2`，旧 `V1` 保留一个 release 周期（沿既有纪律）。
- `vision.recognize` capability schema 带 `schema_version`；插件 caller 检查不匹配 fail-fast。

### 10.3 数据迁移

- **无 DB schema 变更**：grounding / telemetry 聚合是运行时产物（不入 items 表）；若持久化报告 → 走既有 sidecar 加密表模式 + `schema_version`，migration 幂等。
- 老 region 数据 `grounding: None` 视为「未跑增量」，重新 recognize 时 lazy 生成（仿 content_hash lazy backfill）。

### 10.4 Feature flag

- `vision-eval` 默认关（仅离线/CI 编译）；`nontext`（既有）门控所有 native 模型依赖。关闭 `vision-eval` 不影响生产路径。

---

## 11. 风险登记

| # | 风险 | 影响 | 缓解 |
|---|------|------|------|
| **R1** | **grounding GT 标注昂贵**：每抽取值↔原图区域人工标注耗时 | golden 集建设慢 | 先在高价值 kind（chart/table/formula）建 grounding GT；caption/handwriting 用区域级 grounding（不强求 sub-bbox）；分批 backfill（per §6.2 R18）|
| **R2** | **failover 掩盖根因**：主 provider 静默故障被 failover 兜住，运维不知 | 主 provider 长期挂无人修 | failover 必记 telemetry + 周期 surface「primary X 失败率 Y%」；不只 UI 还进 audit/log（防 R2 类「兜底掩盖」）|
| **R3** | **N=3 eval 成本 + 上游漂**：real-VLM eval 烧 token，且跨 day model 漂导致 gate flaky | 开发成本 + false regression | `--confirm-cost` 授权（§1.3）；manifest record fingerprint + 跨 run 用 paired stat（per `2026-05-28` 风险 C）；本地候选作 canonical baseline；eval 是离线 gate 非每 PR 必跑（develop→main 前 + RELEASE 前）|
| **R4** | **qwen-3.6/3.7 多模态能力分布未知**：某 kind 弱 tier F1 远低 | 弱 tier 默认启用出门踩坑 | §9.2 3-tier 矩阵 ship 前必跑；< floor 的 (kind,model) 自动 disable + RELEASE.md 标最低 tier（§4.5 E）；不靠直觉「应该能行」|
| **R5** | **grounding validator 误判**：把对的标 ungrounded（VLM 给了正确值但 ref 格式略偏）| 噪音 retry / 浪费 token | validator 纯逻辑可单测全覆盖；ungrounded 不丢值（保留 + warning，§7）；阈值（IoU/越界容差）golden 校准 |
| **R6** | **并发 / 锁**：VlmRouter 健康探测 + failover 引入新锁，与既有 OCR/search 三锁 ABBA | 死锁（MEMORY B1 前科）| router 探测用独立 Mutex/原子，不与 search 三锁（fulltext→vectors→vault）交叠；探测异步不持其他锁；锁序文档化 |
| **R7** | **隐私 / 出网**：failover 切候选仍上云、grounding ref 暴露原图坐标 | 隐私泄露 | 所有候选走既有 `gate_vlm_egress`（redact+缩放+outbound gate 非 no-op，MEMORY P0）；保守 governor 全本地不出网；grounding ref 是坐标非内容，UI 明示「将上传云端 VLM」|
| **R8** | **scope creep**：行业语义 / 用户侧多 VLM 配置 UI / 新 region 类型混入 | 违反 §2 边界 | §2.2 显式排除；§X-Cutting per-sub-feature §4.3 判定；评审验 `RegionKind`/默认模型清单无行业词、无 qwen-vl |
| **R9** | **agent-invocable 暴露重造 agent 框架**：I5 误新建一套 dispatch | 冗余 + 维护负担 | I5 复用既有 agent dispatch / capability registry，**不新建**；plan 文件清单须显式标「复用 X 机制」|

---

## 跨切面（X-Cutting）：OSS vs attune-pro 边界（per `docs/oss-pro-strategy.md` §4.3 + ADR 0008）

> **共享视觉理解 agent（ADR 0008，已 accepted）**：本 spec 全部增量叠在这**唯一**共享视觉核心上。
> 通用稳定性/可靠性增强（grounding / N=3 gate / 模型矩阵 failover / 失败聚合 / agent-invocable 暴露）
> 对**任何个人通用用户**有价值 → **OSS attune**（共享核心本体）；行业插件**消费**之，绝不自带视觉。

### 套 §4.3 决策（逐增量）

| 增量 | §4.3 判定 | 归属 |
|------|-----------|------|
| I1 grounding 契约 + validator | 通用质量保障（任何人都想核验抽取值来源）| ✅ **OSS attune** |
| I2 N=3 real-VLM eval gate | 通用 self-validation（类比 KB 端 `2026-05-28` ✅ OSS）| ✅ **OSS attune** |
| I3 模型矩阵 failover（qwen-3.6/3.7）| 通用可靠性（任何用户都需 VLM 不挂）| ✅ **OSS attune** |
| I4 弱模型分级 degrade + 失败聚合 | 通用兜底（§4.5 E/F 通用规则）| ✅ **OSS attune** |
| I5 agent-invocable 暴露 | ADR 0008 共享核心的暴露面，零行业绑定 | ✅ **OSS attune** |
| 高精「全 region VLM + 多模型投票」精度 tier | 增值精度服务，pro 会员/网关计费门控；**仍是同一份共享 agent 的一个 tier，不分叉视觉实现** | 💰 **attune-pro 会员门控的共享-agent tier**（非 per-plugin fork）|
| 行业语义（印章合规 / 合同条款表 / 病历表 / 专利附图语义）| 专属行业 → Q2；**调用共享 agent 拿通用 `RegionResult` + grounding，仅叠语义** | 💰 **attune-pro** 纵向 plugin（law/medical/patent-pro），消费共享 agent |

### 边界不变量（评审须验）

- 本 spec OSS 部分**零行业绑定**：`GroundingRef` / VlmRouter 候选清单 / eval golden 的 kind **无任何**法律/医疗/专利专属词（防 §4.3 泄漏，MEMORY「OSS 边界行业回灌」前科）。
- **默认模型锁 qwen-3.6/3.7**，**禁用 qwen-vl**，**不掺 claude/gpt 生产 token**（§4.5H）。
- **行业插件绝不自带视觉/VLM**（ADR 0008 硬约束）：评审验 law/patent/medical-pro 不直接持 `VlmProvider`、不实现 VlmRouter，所有图像需求经 §5.4 `vision.recognize` capability。发现内嵌 = 违反 ADR 0008，打回。

---

## 评审清单（spec → 评审 → writing-plans 前）

- [ ] §2 scope 边界用户确认（5 增量 I1-I5；显式排除新 region 类型 / 自训模型 / 行业语义 / 用户侧多 VLM 配置 UI）
- [ ] §0 已实现 vs 增量划分用户认可（不重做已 ship 的检测/识别/schema-guided/retry/telemetry-record/egress-gate）
- [ ] §3.2 模型矩阵默认锁 qwen-3.6/3.7 + 优先级方向认可（plan 阶段 §9.2 实测定准确率/优先级）
- [ ] §5.4 I5 agent-invocable 暴露：plan 必须显式标「复用既有 agent dispatch / capability registry，不新建框架」（R9）
- [ ] §9.2 N=3 + 3-tier 矩阵 + floor 阈值：plan 阶段 golden set 实测校准（§6.3）；real-VLM eval 走算力授权（§1.3）
- [ ] §11 R1 grounding GT 标注成本：plan 阶段定优先 kind + 分批 backfill 计划
- [ ] §X-Cutting OSS/pro 拆分 + ADR 0008 边界不变量用户确认（共享核心进 OSS；高精档 = pro 会员门控同一 agent tier；行业插件叠语义不内嵌视觉）
- [ ] 评审通过 → invoke `superpowers:writing-plans` 出 implementation plan（minor 切片 + 文件清单 + commit 分批 + GA 验收清单）
```

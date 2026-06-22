# K3 调度层集成 — OpenAI/Ollama-compat 收口(改动最小)

> **状态**: DRAFT(spec-first,§3.1 11 节)
> **日期**: 2026-06-22
> **作者**: K3 调度层集成 agent
> **前置**: INT-3(`infer/catalog.rs` 已落,task #125) / `docs/superpowers/specs/2026-06-10-k3-integration-gaps.md`(G1-G8,attune-k3 仓回填)/ attune-k3 全栈 spec(`/data/company/project/attune-k3/docs/superpowers/specs/2026-06-10-attune-k3-fullstack.md`)
> **用户拍板**: k3-scheduler 对外 = OpenAI/Ollama-compat,监听 **:8090**(推理统一收口,禁旁路直连 worker);attune 端 **0 业务代码改动**,只配 base_url。

---

## 1. 目标定位

attune-k3 一体机(个人智算存一体机,Law Edition 首发)把所有本地推理(embedding / rerank / ASR / OCR / 可选本地 LLM)统一收口到一个 **k3-scheduler** 进程,对外暴露 **OpenAI 兼容 `/v1/*` + Ollama 兼容 `/api/*`** 协议,监听 **:8090**。attune 客户端不直连任何 worker、不感知 RVV/IME/llama.cpp 细节 —— 它把已有的 HTTP provider(`embed.rs::OpenAiEmbeddingProvider` / `OllamaProvider`、`llm.rs::OpenAiLlmProvider` / `OllamaLlmProvider`)的 `base_url` 指向 `:8090` 即可。

**用户痛点**:
- K3 一体机命脉 = 「数据不出门」(隐私优先,Law Edition 处理敏感卷宗)。本地能力必须永不出网。
- K3 是 riscv64 弱算力(LLM 1.4 t/s),不适合交互/并发 LLM;重 LLM 默认应走云端会员 token,本地能力(检索/embedding/OCR/ASR)走 K3 :8090 零费用。
- 此前 attune 对 K3 的接入分散(probe 用 :8080、Ollama 默认 :11434),与「:8090 统一收口」设计不一致。

**与产品 positioning 对齐**:attune 三层成本契约(§成本感知)—— 本地能力 = 零成本/本地算力层(K3 :8090),重 LLM = 时间/金钱层(云端 token 或本地慢 LLM,用户显式触发)。K3 把「本地算力层」整体搬到一体机 :8090 收口点。

## 2. 范围边界

**做(本 spec)**:
- catalog `riscv-k3` tier 扩展:embedding/rerank/asr/ocr(+既有 llm)resolve 到 k3-scheduler :8090,ep=`k3-scheduler` 标记**预置模型→不走下载**。
- per-capability 路由(catalog 驱动,最小逻辑):本地能力 → K3 :8090;重 LLM → 策略(K3 本地 或 云端会员 token,默认云,可配)。
- wizard「K3 一体机」profile:探测本地 :8090 健康 + 配云账号 + 因预置模型**跳过模型下载步**。i18n zh/en parity。
- :8090 健康 probe + failover:挂 → 本地能力降级(CPU / 报错友好),LLM 降级云端;健康度进模型状态面板。接 S8 ModelStack failover + §4.5 graceful degrade。
- 隐私边界:本地能力永不出网(loopback/RFC-1918 :8090 = local_destination);仅用户显式选云 LLM 才经 OutboundGate + RedactingLlmProvider + doc_privacy。
- attune-k3 仓 config:K3 部署 profile(各 provider base_url → :8090)+ VERSIONS pin 占位 + 开机配置说明。**无业务代码**。

**不做(后续 / 他仓)**:
- k3-scheduler 自身实现(归属 k3-scheduler 仓,D1,不在本清单)。本 spec 假定 :8090 已实现 OpenAI/Ollama-compat 契约。
- G1 MCP server / G2 scoped token / G3 vault locked-mode / G5 durable queue / G7 并发基线 —— attune roadmap 另排期(见 §对齐 G1-G8)。
- 真 K3 设备端到端验证(§7.3,标 PENDING-真机,本轮不连真机)。
- RK1820/RK3588 NPU tier(已在 catalog,非本 spec 范围)。

**写死**:本轮**不改 attune 业务代码**(vault / 检索 / 插件 / skill)。若实现中发现必须改业务代码 → 停下报告(可能 scheduler 协议没对齐)。

## 3. 架构数据流

```text
┌──────────────────────── attune-k3 一体机 (riscv64, 局域网/loopback) ─────────────────────────┐
│                                                                                              │
│   attune-server-headless (24h 常驻)                                                          │
│     ├─ embed.rs::OpenAiEmbeddingProvider{ base_url=http://127.0.0.1:8090/v1 }  ──┐           │
│     ├─ reranker (经 :8090 /v1 rerank 或 catalog engine)                          │           │
│     ├─ ASR / OCR (经 :8090)                                                      ├─►  :8090  │
│     └─ llm.rs::OpenAiLlmProvider / OllamaLlmProvider{ base_url=:8090 }(可选本地)─┘  k3-      │
│            │                                                                       scheduler │
│            │  重 LLM 默认策略=云端 ─────────────────────────────────────────────►(OpenAI/   │
│            ▼                                                                        Ollama-   │
│   OutboundGate (local_destination=false) → RedactingLlmProvider → doc_privacy      compat)   │
│            │  仅用户显式选云 + 非 L0 + redact 后                                    收口        │
└────────────┼─────────────────────────────────────────────────────────────────────┘  ▼      │
             ▼                                                                     llama.cpp/  │
   云端会员网关 gateway.engi-stack.com (重 LLM)                                     RVV/IME/    │
                                                                                    ONNX worker │
                                                                                   (attune 不感知)│
```

**隐私不变量(硬保证)**:`:8090` 在 K3 上是 loopback(同机)或 RFC-1918(LAN 一体机)→ `embedding_endpoint_is_local()` / `is_local_probe_target()` 判 `local_destination=true` → L0「永不出网」内容经 K3 本地能力**不触发** `L0CloudBlocked`,合法留在设备内。云端 LLM 是唯一 `local_destination=false` 路径,必经 OutboundGate(disabled/vault-locked/L0-cloud/redactor 五关)。

**DB tables**:无新增。复用 `items.privacy_tier`(L0 判定)、`embedding_queue`(#82 已接 gate)。
**cache layers**:catalog 缓存(`~/.local/share/attune/catalog/`,已有);:8090 健康度 = 进程内 `AtomicBool`(类 `embedding_is_local`),不落库。

## 4. 模块边界

| crate / 文件 | 改动性质 | 说明 |
|---|---|---|
| `attune-core/assets/model-catalog.default.yaml` | **数据**(catalog) | `riscv-k3` tier 加各 role,ep=`k3-scheduler`,verdict/source 齐 |
| `attune-core/src/infer/catalog.rs` | **catalog 测试** | 加 `riscv-k3` tier resolve 测试 + per-capability 断言(无业务逻辑改动) |
| `attune-server/src/routes/llm.rs` | **配置常量** | probe 候选端口 :8080 → **:8090**(K3 收口口),subnet 扫描端口同步 |
| `attune-server/src/routes/settings.rs` | **配置默认值** | `FormFactor::K3Appliance` 默认 LLM/embedding base_url 指 :8090(provider 抽象不变) |
| `attune-server/ui/src/wizard/Step3LLM.tsx` | **wizard 默认值 + 文案** | K3 端点默认 :8090;探测/选择走 :8090 |
| `attune-server/ui/src/wizard/Step4Hardware.tsx` | **wizard 分支** | K3 profile 预置模型 → 跳过下载步 |
| `attune-server/ui/src/i18n/{zh,en}.ts` | **i18n** | 新 key 双语 parity |
| (attune-k3 仓) `config/k3-scheduler.profile.toml` + `VERSIONS` + `docs/` | **集成配置** | base_url→:8090 / pin 占位 / 开机说明,**无业务代码** |

**判定**:全部落在 catalog(数据)/ wizard(UI)/ provider 配置(常量+默认值)层。`vault.rs` / `search` / `chat.rs` 核心逻辑 / 插件 / skill —— **0 改动**。

## 5. API 契约

**复用既有,无新 endpoint**:
- `POST /api/v1/llm/probe-k3` —— 候选端口从 :8080 改 :8090(请求/响应 schema 不变:`{found,endpoint,checked}`)。
- `GET /api/v1/ai_stack` —— K3 tier 时 recommendation 标 `prebundled`(预置,download_mb=0)。
- `PATCH /api/v1/settings` —— K3 profile 写 `llm.endpoint` / `embedding.endpoint` = `http://127.0.0.1:8090/v1`(既有字段,值变)。
- `GET /api/v1/ai-stack/catalog`(已有)—— 反映 `riscv-k3` tier resolve 结果 + :8090 健康度。

**k3-scheduler 对外契约(被消费方,k3-scheduler 仓实现)**:
- OpenAI 兼容:`GET /v1/models`、`POST /v1/embeddings`、`POST /v1/chat/completions`、(可选)`POST /v1/rerank`。
- Ollama 兼容:`GET /api/tags`、`POST /api/embed`、`POST /api/chat`。
- attune 既有 provider 已实现这两套客户端协议,故 attune 侧零新增协议代码。

## 6. 扩展点 / 插件接口

- **新底座能力经 catalog 加 role**:catalog `Role` enum 已支持 embedding/rerank/ocr/asr/llm;k3-scheduler 加新能力 → catalog 加 entry + base_url,attune 自动 resolve,无需改 attune 代码。
- **新硬件 tier**:`tier_for_hardware()` 注释已说明 riscv-k3 / rk* tier 由专用部署路径显式指定(非通用硬件探测)。K3 部署 profile 显式设 tier=`riscv-k3`。
- **per-capability 路由策略可配**:重 LLM「K3 本地 vs 云端」由 settings `llm.endpoint` 决定,用户/部署 profile 可切,无需改代码。

## 7. 错误 + 边界 case

| case | 行为 | 错误码 |
|---|---|---|
| :8090 探测超时/挂 | 本地能力降级:embedding 回退 CPU/ONNX in-process;UI 标「K3 调度未就绪」 | `k3-scheduler-unreachable`(probe found=false,非 panic) |
| :8090 挂 + 重 LLM 需求 | LLM 降级云端会员 token(若已配)或报错友好「本地 LLM 不可用,请配云端」 | graceful Result::Err |
| L0 内容 + K3 :8090(local) | **允许**(local_destination=true,数据留设备) | — |
| L0 内容 + 云端 LLM | **拒绝**(L0CloudBlocked,redaction 也不放行) | `l0-cloud-blocked` |
| catalog 无 riscv-k3 各 role(老 baseline) | resolve 回退 cpu-fallback(§10 兼容,老部署零影响) | — |
| 用户传非 local probe 端点 | 经 OutboundGate(kind=Llm),refuse 则静默丢弃,本地探测继续 | outbound_audit log |

graceful degradation 全程:任何 :8090 失败 → 本地 CPU 兜底 / 云端兜底,**绝不 panic、不 silent swallow**(失败有 log + UI 健康度)。

## 8. 成本契约

| 能力 | 成本层 | 触发 | UI 显示 |
|---|---|---|---|
| embedding / rerank / OCR / ASR(K3 :8090) | ⚡ 本地算力(K3 一体机零费用) | 建库阶段自动 | 模型状态面板「K3 调度 · 本地」 |
| 重 LLM(K3 本地 qwen2.5-0.5b) | ⚡ 本地算力(慢,1.4 t/s) | 用户显式触发 | chip「~本地 · 慢」 |
| 重 LLM(云端会员 token) | 💰 时间/金钱 | 用户显式触发 | chip「~N tok · $X」(既有) |

K3 把「本地算力层」从笔电分散底座搬到一体机 :8090 单点;成本归属不变,UI 既有成本契约直接复用。

## 9. 测试矩阵(§6.1 六类)

| 类型 | 用例 | 工具 |
|---|---|---|
| **happy** | catalog resolve(`riscv-k3`,Embedding/Rerank/Ocr/Asr/Llm)命中 :8090/k3-scheduler ep;probe :8090 found | `#[test]` catalog.rs / llm.rs |
| **edge** | riscv-k3 tier 缺某 role → 回退 cpu-fallback;:8090 vs 127.0.0.1:8090 等价判 local | `#[test]` |
| **error** | :8090 不可达 → probe found=false 不 panic;malformed catalog → builtin 兜底 | `#[test]` + mock |
| **adversarial / 隐私** | L0 + :8090(local)允许;L0 + 云端 LLM 拒;`http://8090@evil.com` userinfo 绕过判 non-local | `#[test]` outbound_gate + state.rs |
| **concurrent / failover** | :8090 挂 → 本地能力降级 CPU + LLM 降级云端(状态切换) | `#[test]` health/failover |
| **resource / mock-compat** | 起本地 mock OpenAI/Ollama-compat HTTP(单 TcpListener)验 attune→:8090 `/v1/embeddings` + `/v1/models` 契约对接(§1.6 纯离线例外) | `#[test]` mock(复用 embed.rs 既有 TcpListener 模式) |
| **i18n** | wizard 新 key zh/en parity + grep 双守卫 0 输出 | 守卫脚本 |
| **真机 §7.3** | K3_IP env 真 :8090 端到端(embedding/LLM/OCR) | **PENDING-真机**(本轮不连) |

通过判据:catalog/路由/probe/failover/隐私 deterministic PASS rate = 1.00;mock-compat 契约对接 PASS;clippy `-D warnings` 干净;i18n 双守卫 0。

## 10. 向后兼容

- catalog `riscv-k3` tier 各 role 是**新增/扩展**,老 baseline resolve 回退 cpu-fallback → 老部署零影响(catalog 覆盖层不是硬依赖,catalog.rs §14 注释已立)。
- probe 端口 :8080 → :8090 **变更**:K3 一体机镜像由 attune-k3 仓统一部署 :8090,镜像与 attune tag 配对(VERSIONS pin),不存在「老 K3 用 :8080」的存量(K3 GA 未发,greenfield)。文档/RELEASE 标注此口径。
- provider 协议不变(OpenAI/Ollama-compat),settings 字段不变(仅默认值随 FormFactor::K3Appliance 变),老 settings.json 不破。
- schema_version 不变(catalog schema v1,新 role 用既有 ModelChoice 字段)。

## 11. 风险登记

| 风险 | 缓解 |
|---|---|
| **隐私回归**:误把 :8090 判 non-local → L0 泄漏 | `embedding_endpoint_is_local()` 已 host-anchored(防 userinfo/前缀绕过,state.rs:2660 + 测试守);加 adversarial 测试钉死 |
| **k3-scheduler 协议漂移**::8090 未真实现 OpenAI/Ollama-compat | mock-compat 测试钉死 attune 侧契约;真机 §7.3 PENDING 待 k3-scheduler 仓 D1 ship 后验;协议不符 → 停下报告(不改 attune 业务代码硬填) |
| **被迫改业务代码** | 范围边界写死「0 业务代码改动」;触发即停报告(§硬约束) |
| **:8090 单点故障** = 一体机本地能力全停 | failover 降级 CPU/云端;健康度上报 UI;接 S8 ModelStack + §4.5 |
| **riscv64 平台债**(G8) | 本 spec 不涉编译(全 config/catalog/wizard,跨平台无 arch 特异代码);riscv64 真编译走 §跨平台 rv-gcc,不在本轮 |

---

## 切片表(§7.1.4)

| 切片 | 主题 | 关键交付 | 改动层 | 状态 |
|---|---|---|---|---|
| K3-S1 | catalog riscv-k3 tier 扩展 | model-catalog.default.yaml + catalog.rs 测试 | 数据/测试 | 本 spec |
| K3-S2 | probe + settings 默认 :8090 | llm.rs probe 端口 / settings.rs K3 默认 base_url | 配置常量/默认值 | 本 spec |
| K3-S3 | wizard K3 profile | Step3/Step4 + i18n zh/en | UI/i18n | 本 spec |
| K3-S4 | :8090 健康 + failover | health probe + 降级路由 + 状态面板 | 配置/路由策略 | 本 spec |
| K3-S5 | attune-k3 config | profile.toml + VERSIONS + docs | 集成配置(无业务码) | 本 spec |
| K3-真机 | §7.3 端到端 | K3_IP 真 :8090 | — | **PENDING-真机** |

---

## 对齐 G1-G8(`2026-06-10-k3-integration-gaps.md`)

| G | 缺口 | 本方案覆盖? |
|---|---|---|
| **G4** | embedding engine 与 Ollama 耦合疑点 / 可配指向任意 OpenAI 兼容 endpoint | ✅ **本 spec 覆盖**:`OpenAiEmbeddingProvider{base_url}` 已存在,K3 profile 指 :8090 验证「embedding 可经配置指向任意 OpenAI 兼容 endpoint」(G4 验收即本 spec mock-compat 测试 + K3 profile) |
| **G6** | headless 二等公民 / 首次开箱纯 Web | ⚠️ **部分**:wizard K3 profile(纯 Web 完成探测+配置+跳下载)推进 G6「首次开箱纯 Web」一面;完整 headless↔桌面对齐 audit 仍随 K3 v0.2 清单 |
| **G8** | riscv64 平台债 | ➖ **不覆盖**(本 spec 全 config/catalog/wizard,无 arch 特异代码);riscv64 编译随 K3 v0.2,本 spec 不引入新平台债 |
| G1 | MCP server | ➖ attune v1.1 另立 spec |
| G2 | agent scoped token | ➖ attune v1.1 另立 spec |
| G3 | vault locked-mode | ➖ 安全评审另排期 |
| G5 | durable job queue | ➖ v1.1 |
| G7 | 多终端并发基线 | ➖ 按 K3 v0.5 实测定级 |

**重排说明**:本 spec 把 D1(k3-scheduler :8090 OpenAI/Ollama-compat 收口,gaps 文档 §协作约定列为「k3-scheduler 仓另行评审」)的 **attune 消费侧**落地 —— attune 0 业务代码、纯配置对接 :8090,作为 G4 的具体兑现 + G6 的 wizard 一面;D1 服务端实现仍归 k3-scheduler 仓。

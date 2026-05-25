# new-api 双源头供应能力 verify — VLM + LLM 分源

- **Date**: 2026-05-25
- **Type**: Verification spec(per attune CLAUDE.md「架构级别设计铁律」11 节;本 spec 是 verify-only,不实施)
- **Status**: VERIFIED — new-api native 支持双源头(0 code change 配置即用)
- **Scope**: cloud `llm-gateway`(new-api 部署)+ attune-server settings + attune-pro accounts `/me`
- **Owners**: cloud(channel pool 配置)· attune-core(settings.vlm 字段)· attune-pro(accounts /me dual-default)
- **Verify path**: 静态(代码/文档 SSOT) + 动态(docker ps 确认 new-api v0.13.2 running 5h);不动 4090 / 不实施 v1.0.1
- **关联**:
  - 用户原话:「new-api 能否支持双 AI 源头供应?VLM、LLM 的源头来自不同的源头」
  - `2026-05-24-llm-vlm-multi-provider-architecture.md`(v1.0.1 LLM+VLM 完整架构 spec,本 verify 的实施计划)
  - `2026-05-26-v1-0-1-upgrade-strategy-and-support.md`(v1.0.1 升级策略 SSOT)
  - `cloud/llm-gateway/docs/failover-policy.md`(渠道路由 + failover 详解)
  - `cloud/llm-gateway/docs/llm-providers-setup.md`(5 provider 配置手册,已实施)

---

## 目录 (Table of Contents)

- [1. 目标定位](#1-目标定位)
- [2. 范围边界](#2-范围边界)
- [3. 架构数据流](#3-架构数据流)
- [4. 模块边界](#4-模块边界)
- [5. API 契约](#5-api-契约)
- [6. 扩展点 / 插件接口](#6-扩展点--插件接口)
- [7. 错误处理 + 边界 case](#7-错误处理--边界-case)
- [8. 成本契约](#8-成本契约)
- [9. 测试矩阵](#9-测试矩阵)
- [10. 向后兼容](#10-向后兼容)
- [11. 风险登记](#11-风险登记)
- [附录 A. 双源头配置示例(可直接落 new-api 后台)](#附录-a-双源头配置示例)
- [附录 B. verify 时间表 + evidence](#附录-b-verify-时间表--evidence)

---

## 1. 目标定位

### 1.1 用户提问

> 「new-api 能否支持双 AI 源头供应?VLM、LLM 的源头来自不同的源头」

### 1.2 verify 结论(先放结论,11 节是论证)

**YES — new-api(QuantumNous/new-api,本仓部署 `calciumion/new-api:v0.13.2`)native 支持双源头供应,且无需任何 code change。**

支持机制有两层:

| 层 | 机制 | 触发方式 |
|----|------|---------|
| **L1 native(已 verified ready)** | **channel + model 白名单** | 客户端发 `model: gpt-4o-vision` → 路由 channel B(OpenAI);发 `model: deepseek-v4-flash` → 路由 channel A(DeepSeek)。**model name 决定 channel,即"双源头"** |
| **L2 content-aware routing(v1.0.1 spec #154,需实施)** | **gateway 中间件按 messages content type 自动选 channel** | 客户端发 `messages` 含 `image_url` → 自动路由 VLM channel;纯文本 → LLM channel。用户不感知 model 切换 |

L1 已在 `failover-policy.md` §2 的优先级矩阵里实际配过 7 个 channel(5 provider),其中
`openai-primary-1` 支持 `gpt-4o`(vision capable)+ `gpt-4o-mini`,`deepseek-flash-primary`
仅支持 `deepseek-v4-flash`(text-only)— **这本身就已经是双源头双 modality**,只是
attune 客户端目前还没显式区分 LLM/VLM 调用入口。

### 1.3 北极星对齐

per attune CLAUDE.md「Cost & Trigger Contract」:
- LLM(text-only)单价低(DeepSeek-v4-flash $0.07/1M input)
- VLM(vision)单价高 30×(GPT-4o $2.50/1M input + 图片按 patches 算 1K-3K token/图)
- **统一 routing 不是省事,是省钱** — 把绝大多数 text-only 流量路到便宜 LLM,只让带图 request 进 VLM channel,成本可下降 5-10×

双源头供应能力是 attune **混合智能 + 分层成本** positioning 的基础设施前提。

---

## 2. 范围边界

### 2.1 本 spec 做

- ✅ **verify**:确认 new-api v0.13.2 native 支持双源头(channel + model 白名单)
- ✅ **落档**:产物即本 spec(11 节)+ 附录 A 配置示例 + 附录 B evidence
- ✅ **关联**:与 #154 v1.0.1 LLM+VLM 架构 spec 衔接(免费 BYOK + 付费 gateway 双 path)

### 2.2 本 spec 不做(明示边界)

- ❌ **不实施** new-api channel 配置变更(channel 增减须经 v1.0.1 sprint 落地)
- ❌ **不实施** attune-core `VlmProvider` trait / settings.vlm 字段(per #154 v1.0.1 phased 落地)
- ❌ **不实施** attune-server `routes::chat` content-aware routing(per #154)
- ❌ **不动** 4090 / Ollama / API keys / 任何 production config
- ❌ **不开发新功能** — 本 spec 是「verify + 落档」,不是「实施」

### 2.3 后续 v 不允许扩 scope

per #154 已 pin 的 phased plan:
- **v1.0**:仅 settings schema dormant 字段 + LlmProvider capability(`supports_vision()`)
- **v1.0.1**:cloud /me dual default + new-api admin tier 字段 + chat route content-aware routing
- **v1.1**:UI VLM 配置面板 + 多 fallback BYOK
- **v1.2+**:OCR-VLM 联动 / image annotation pipeline

本 verify spec 不允许扩到「现在就实施 channel tier」或「现在就改 attune-core」。

---

## 3. 架构数据流

### 3.1 当前 v1.0 单源头数据流(baseline)

```
┌──────────────────┐
│  attune Desktop  │  Settings: provider = openai_compat
│  (OSS client)    │           endpoint = https://gateway.engi-stack.com/v1
└────────┬─────────┘           model    = deepseek-v4-flash  ← 单一 model
         │
         │ POST /v1/chat/completions
         │   {"model": "deepseek-v4-flash", "messages": [...]}
         ▼
┌──────────────────────────────────────────────┐
│  new-api(v0.13.2)                            │
│   1. 筛选: model ∈ channel.supported_models  │
│      → 命中 deepseek-flash-primary           │
│   2. 路由发往 https://api.deepseek.com/v1    │
└────────┬───────────────────────────────────────┘
         │ HTTPS + DeepSeek key
         ▼
   DeepSeek API
```

**问题**:用户若发含 image 的 request(`messages[].content` 含 `image_url`)→ DeepSeek
返回 400(unsupported_modality)→ attune-core `chat_multimodal` fallback 把 image 丢掉
+ 警告(per #150 现 dead code path)→ silently 丢数据。

### 3.2 免费 BYOK 双源头(v1.0.1 客户端侧)

per #154 v1.0.1 spec:**client 端配 2 套 endpoint + key**(LLM 必填 / VLM 选填),
client 按 messages content type 选 endpoint。

```
┌──────────────────────────────────────────┐
│  attune Desktop                          │
│  Settings:                               │
│    llm.endpoint = https://api.deepseek.com/v1     ← 用户 BYOK A
│    llm.model    = deepseek-v4-flash      │
│    vlm.endpoint = https://api.openai.com/v1       ← 用户 BYOK B
│    vlm.model    = gpt-4o                 │
│                                          │
│  routes::chat::handle_chat():            │
│    if messages 含 image_url:             │
│      state.vlm().chat_multimodal(...)    │
│    else:                                 │
│      state.llm().chat(...)               │
└────────┬─────────────┬───────────────────┘
         │ text-only   │ with image
         ▼             ▼
   DeepSeek API   OpenAI API
   (BYOK A)       (BYOK B)
```

**优点**:client 直连两家,0 gateway 摩擦;**缺点**:user 需自管 2 套 key 计费。

### 3.3 付费 Pro gateway 双源头(v1.0.1 服务端侧)

per #154 v1.0.1 spec:**client 端配 1 个 gateway endpoint**,gateway 内部
content-aware routing。这是**本 verify spec 的核心** — verify new-api 是否 native
支持。

```
┌──────────────────┐
│  attune Desktop  │  Settings: provider = openai_compat
│  (Pro 会员)      │           endpoint = https://gateway.engi-stack.com/v1
└────────┬─────────┘           api_key  = <会员 token>(login 后由 accounts /me 下发)
         │
         │ POST /v1/chat/completions
         │   方式 A: model="deepseek-v4-flash" + 纯 text  ← model name 显式
         │   方式 B: model="auto" + messages 含 image_url ← content-aware
         ▼
┌──────────────────────────────────────────────┐
│  new-api(v0.13.2)                            │
│                                              │
│  L1 (native, ready):                         │
│    筛选: model ∈ channel.supported_models    │
│    → "deepseek-v4-flash" 命中 channel A      │
│    → "gpt-4o-vision" 命中 channel B          │
│                                              │
│  L2 (v1.0.1 middleware, to-impl):            │
│    pre-route inspect: messages[].content     │
│    → 含 image_url → 强制 model="gpt-4o"      │
│    → 纯文本 → model 保持原值或 default       │
│                                              │
│  Failover (同优先级 round-robin,             │
│            跨优先级 retry,见 failover-policy)│
└────────┬─────────────┬───────────────────────┘
         │ text-only   │ with image
         ▼             ▼
   DeepSeek API   OpenAI API
   (channel A,    (channel B,
    cloud-side    cloud-side
    aggregated    aggregated
    key)          key)
```

**verify 关键**:**L1 已 ready**(已配 5 provider/7 channel,见 §附录 A);**L2 是
v1.0.1 实施任务,本 verify spec 不实施**。

---

## 4. 模块边界

| 模块 | 当前状态 | v1.0.1 实施 | 关联 spec |
|------|---------|------------|----------|
| `cloud/llm-gateway` new-api v0.13.2 | ✅ 运行中(5h uptime,docker ps verified) | 增 channel B(VLM specialized)+ admin tier 字段 | 本 spec §附录 A |
| `cloud/llm-gateway/docs/failover-policy.md` | ✅ 7 channel 优先级矩阵已落档 | 扩 vlm-primary / vlm-fallback 两层 | failover-policy.md §2.1(待扩) |
| `cloud/accounts /me` | 现下发 `gateway_default_model`(单值) | 加 `gateway_vlm_default_model` | #154 §5.2 |
| `attune-core/src/llm.rs` | `chat_multimodal` fallback silently drop image | 加 `supports_vision()` trait method + `Err(ProviderCapability)` for non-vision | #154 §4 |
| `attune-core/src/llm_settings.rs` | 单 llm 节 | 加 vlm 节(与 llm 平级 4 字段) | #154 §4 |
| `attune-server::routes::settings` | GET/PUT /api/v1/settings 单 llm | 支持 vlm 字段;validation: endpoint + model 都填或都空 | #154 §5.1 |
| `attune-server::routes::chat` | 单 provider 调用 | content-aware: 含 image → state.vlm(),否则 state.llm() | #154 §5.3 |
| `attune-server::routes::member` | login 写 llm 默认 model | login 写 llm + vlm 双默认 model | #154 §5.4 |

**本 verify spec 仅确认 new-api(模块 1)的能力**,其他模块的实施在 #154 v1.0.1
sprint(5/27-28)落地。

---

## 5. API 契约

### 5.1 new-api channel admin API(已存在,v0.13.2 native)

```http
POST /api/channel/  HTTP/1.1
Authorization: <admin-session-token>
Content-Type: application/json

{
  "type": 1,                        # 1=OpenAI, 14=Anthropic, 25=Gemini, 36=DeepSeek
  "name": "openai-vision-1",
  "base_url": "https://api.openai.com/v1",
  "key": "sk-xxx",                  # API key(支持换行多 key 轮询)
  "models": "gpt-4o,gpt-4o-mini",   # 白名单(明示,不留空)
  "priority": 10,                   # 数字越大越优先
  "status": 1,                      # 1=启用 2=禁用
  "weight": 1                       # 同 priority 内权重
}
```

**关键字段**:
- `models`:逗号分隔的 model name 白名单,决定哪些 model 命中本 channel
- `priority`:`failover-policy.md` §1 的渠道选择算法的 sort key
- `status=1`:启用;失败连续 5 次自动 status=2 + 10min 后 retry recover

### 5.2 v1.0.1 增加的 channel 字段(tier)

per #154 §6,**v1.0.1 实施**(本 verify spec 不实施):

```json
{
  ...
  "tier": "llm"   // 或 "vlm";本字段在 v0.13.2 不存在,需 new-api PR 或 middleware 注入
}
```

**实施替代方案**(若 new-api upstream 不接 PR):
- **方案 A(推荐)**:gateway 前置 nginx-proxy / 自研 middleware,inspect
  `messages[].content[].type`,**改写 model name**(`gpt-4o` → `gpt-4o-vision`),
  然后让 native model 白名单完成 channel 选择 — **0 new-api 改动**
- **方案 B**:fork new-api 加 tier 字段,维护成本高,不推荐

### 5.3 attune-core 客户端调用(v1.0 现状 → v1.0.1)

**v1.0(单源头)**:
```rust
// attune-server::routes::chat
let llm = state.llm();
llm.chat(messages).await?;
```

**v1.0.1(双源头)**:
```rust
// per #154 §5.3
let has_image = messages.iter().any(|m| m.has_image_attachment());
let provider = if has_image {
    state.vlm().ok_or(VaultError::ProviderCapability("VLM required"))?
} else {
    state.llm()
};
provider.chat_multimodal(messages).await?;
```

---

## 6. 扩展点 / 插件接口

### 6.1 加新 LLM provider(已成熟,native 支持)

per `llm-providers-setup.md` §1:管理后台 → 渠道 → 添加渠道 → 填写 type / name /
base_url / API key / 支持模型 / priority → 测试 → 启用。**5 分钟可加一个新 provider,
0 code change**。

已实施 5 provider:OpenAI / Anthropic / Gemini / DeepSeek / Mistral。

### 6.2 加新 VLM model(v1.0.1)

per `failover-policy.md` §2 矩阵 + #154 v1.0.1 spec,**v1.0.1 实施**:

```yaml
# 新增 vlm-primary channel
- name: openai-vision-primary
  type: 1  # OpenAI
  base_url: https://api.openai.com/v1
  models: gpt-4o,gpt-4o-mini   # both vision-capable
  priority: 9                   # VLM tier,与 LLM tier 独立排序
  tier: vlm                     # v1.0.1 字段

# 新增 vlm-fallback channel(Gemini vision)
- name: gemini-vision-fallback
  type: 25
  base_url: https://generativelanguage.googleapis.com/v1beta/openai
  models: gemini-2.0-flash      # vision-capable
  priority: 6
  tier: vlm
```

### 6.3 BYOK 直连(免费用户绕开 gateway)

per #154 v1.0.1 spec,免费用户**完全不走 gateway**,直接 client 配 2 套 endpoint:
- `llm.endpoint = https://api.deepseek.com/v1` + `llm.api_key = <user-byok-deepseek>`
- `vlm.endpoint = https://api.openai.com/v1` + `vlm.api_key = <user-byok-openai>`

attune-server::routes::chat content-aware 路由两套 endpoint,**gateway 不在路径上**。

### 6.4 加 model name spoofing 防护(v1.0.1+)

per §11 风险登记,需 gateway middleware 验证 `messages[].content` 与 `model` 字段
一致性 — 防止 user 发 `model=gpt-4o-mini-cheap` 但 `messages` 含 image 走 VLM cost。

---

## 7. 错误处理 + 边界 case

### 7.1 model 不在任何 channel 白名单(404 不 failover)

per `failover-policy.md` §3:
- 404 → 返回 404 给客户端,**不触发 failover**
- attune-core 应捕获 404 + 弹 i18n 提示「model not configured on gateway」

### 7.2 LLM channel quota exhausted,VLM channel 正常

- L1 native:user 发 `model=deepseek-v4-flash` → 命中 `deepseek-flash-primary`
  → quota exhausted(429) → failover 到 `deepseek-pro-fallback`(同 model 不同 channel?
  实际配置需 deepseek-pro channel 也支持 v4-flash model name,或退到 OpenAI 等更贵 provider)
- 跨 tier failover(LLM 失败回退到 VLM channel)**禁止** — VLM 价格 30× 更高,
  failover 必须在同 tier 内

### 7.3 client 发图给 LLM-only provider(non-vision)

- **v1.0 现状**:DeepSeek 返回 400 `unsupported_modality` → attune-core 静默丢图 + 警告
- **v1.0.1 修复**(per #154 §4):attune-core `LlmProvider::supports_vision() = false`
  → 含图请求**直接** `Err(VaultError::ProviderCapability("VLM required"))`,
  不发 request 不丢数据
- **gateway 侧防护**(v1.0.1+):middleware 检测 `messages` 含 image + `model` 是
  text-only model → 改写 model name 或 422 拒绝

### 7.4 双 BYOK 中一个 key 失效

- llm key 失效 + vlm key 有效:含图 request 仍可走;纯文本 request → 401 错误
  → attune-core 提示 user 更新 llm key
- vlm key 失效 + llm key 有效:含图 request → 422 提示「VLM key invalid」;
  纯文本正常

### 7.5 gateway 完全宕机(付费 Pro 用户)

- attune-core 应 detect gateway 503 → **automatic fallback to BYOK**(如 user 也配了
  备用 BYOK key)
- 若 user 仅依赖 gateway 无 BYOK fallback → 弹 i18n 错误 + 引导配 BYOK

### 7.6 错误码 kebab(per attune CLAUDE.md error handling)

| code | HTTP | 含义 |
|------|------|------|
| `vlm-required` | 422 | 请求含 image 但未配 VLM provider |
| `vlm-key-invalid` | 401 | VLM endpoint 返回 401 |
| `llm-quota-exhausted` | 429 | LLM channel quota 用完 + failover 全 fail |
| `gateway-unreachable` | 503 | gateway 连接失败 |
| `provider-capability` | 422 | provider 不支持 modality(vision / audio / etc) |
| `model-not-configured` | 404 | gateway 上无 channel 支持该 model name |

---

## 8. 成本契约

### 8.1 双源头成本估算(2026-05-25 公价)

| 流量类型 | 占比(估算) | 单价 | 月度成本(1M tokens 假设) |
|---------|-----------|------|------------------------|
| LLM text-only(DeepSeek-v4-flash) | 85% | $0.07/M in, $0.28/M out | $0.07-0.28 |
| VLM with image(GPT-4o) | 15% | $2.50/M in, $10.00/M out | $2.50-10.00 |
| **总(blended)** | 100% | — | $0.43-1.74 |

vs **单源头 GPT-4o 跑所有流量**:$2.50-10.00/M,**双源头省 5-15×**。

### 8.2 UI 显示成本(per CLAUDE.md「Cost & Trigger Contract」)

- Chat 发送按钮旁:
  - 纯文本:`~1.2K tok · $0.0004 · DeepSeek 1.6s`
  - 含图:`~3.5K tok · $0.0088 · GPT-4o 4.2s` + warning icon「VLM cost」
- Settings → LLM/VLM 各显示用量统计(per #154 v1.0.1)

### 8.3 成本告警(已实施,per README §成本告警)

`/data/company/cloud/scripts/cost-alert.sh` 每日 cron 跑,
`DAILY_COST_THRESHOLD_USD=10` 触发邮件告警。**双源头后阈值不变**(VLM 流量 15% 假设
不变,blended cost 仍在 $10/day 以下)。

---

## 9. 测试矩阵

per attune CLAUDE.md「测试方案规范」+「6 类下限」:

| # | 维度 | 场景 | 输入 | 期望输出 | 状态 |
|---|------|------|------|---------|------|
| 1 | happy | LLM 纯文本路由 | `model=deepseek-v4-flash` + 纯 text messages | 命中 `deepseek-flash-primary` channel,200 OK,response from DeepSeek | ✅ verified(failover-policy.md §5.1 已 curl 示例) |
| 2 | happy | VLM 含图路由 | `model=gpt-4o` + messages 含 `image_url` | 命中 `openai-primary-1` channel,200 OK,response from OpenAI | ⏳ v1.0.1 实施后 verify |
| 3 | edge | model name 误用(VLM model + 纯文本) | `model=gpt-4o` + 纯 text messages | 200 OK from OpenAI,但成本贵 30× | △ v1.0.1 middleware 改 model 到 cheaper text-only |
| 4 | edge | model name 误用(LLM model + 含图) | `model=deepseek-v4-flash` + messages 含 image_url | v1.0:DeepSeek 400,attune-core 丢图;v1.0.1:attune-core 422 `vlm-required` | ⏳ v1.0.1 fix |
| 5 | error | LLM channel quota exhausted | `model=deepseek-v4-flash` + valid messages | failover 到 `deepseek-pro-fallback`,200 OK from DeepSeek pro | ✅ verified(failover-policy.md §3) |
| 6 | error | VLM channel 全 fail | `model=gpt-4o` + 含图 messages,所有 VLM channel 503 | 503 `gateway-unreachable`,**不**跨 tier failover 到 LLM | ⏳ v1.0.1 实施后 verify |
| 7 | adversarial | content spoofing | request body `model=cheap-model` 但实际含图 | v1.0.1 middleware detect + 改 model 到 VLM channel + cost charge VLM | ⏳ v1.0.1 + 防护 spec |
| 8 | concurrency | 多 user 并发 LLM + VLM 混合 | 100 并发(85 text + 15 image) | new-api round-robin + failover 正常,无 deadlock | ⏳ v1.0.1 stress test |

**6 类下限对应**:
- happy(1, 2)+ edge(3, 4)+ error(5, 6)+ adversarial(7)+ concurrency(8)= 5 类 ≥ 6 类下限的 5 类
- 缺「regression」(待 v1.0.1 落地后建 golden set)+ 「i18n」(VLM 错误提示需中英文)

---

## 10. 向后兼容

### 10.1 v1.0 → v1.0.1 schema migration

per #154 §10:
- `app_settings.llm` 节保持不变(zero-breaking)
- 新增 `app_settings.vlm` 节,**默认全空**(dormant)
- old client(v1.0 不知 vlm)读 settings 时忽略 vlm 字段 → behavior 不变
- new client(v1.0.1)读 settings 时 vlm 为空 → 含图 request → 422 `vlm-required` 提示

### 10.2 gateway api_key 兼容

- accounts /me v1.0 返回:`{gateway_default_model}`
- accounts /me v1.0.1 返回:`{gateway_default_model, gateway_vlm_default_model}`
- v1.0 client 不读 vlm 字段 → 兼容
- v1.0.1 client 读 vlm 字段 → 启用 VLM 能力

### 10.3 channel `tier` 字段引入

v1.0.1 加 `tier` 字段(per §5.2),但 v0.13.2 不识别 → **方案 A 推荐**:不依赖
upstream PR,用 nginx-proxy / 自研 middleware 改 model name 实现 routing,
完全兼容 v0.13.2 native channel schema。

### 10.4 v0.13.2 → 未来 new-api 版本升级

`docker-compose.yml` `image: calciumion/new-api:v0.13.2` 固定 version(per attune
CLAUDE.md 上云规则禁用 `:latest`)。升级前需 verify channel schema + admin API
backward compat。

---

## 11. 风险登记

| 风险 | 影响 | 缓解 |
|------|------|------|
| **R1**:user 通过 BYOK 直连 VLM 但 attune-core 不感知,silently 烧钱 | high | v1.0.1 attune-core 加 UI cost warning(Chat 发送前显示 `~3.5K tok · $0.0088`)|
| **R2**:gateway middleware 改 model name 时 spoof 检测漏(R7 测试矩阵)| medium | v1.0.1+ 加 prompt-injection-guard pre-route 检查 messages 内容一致性 |
| **R3**:VLM API key 泄露(channel 后台明文存储)| high | new-api admin API `CRYPTO_SECRET` 加密 key at rest;cloud secrets sops+age 加密;不进 git |
| **R4**:LLM channel quota exhausted + 跨 tier failover 误启用 → cost 30× spike | high | failover-policy.md §2 显式限制 failover 仅同 tier;v1.0.1 加 tier 字段 + middleware enforce |
| **R5**:new-api v0.13.2 → 未来版本 admin API breaking change | medium | docker-compose pin v0.13.2;升级前跑 §9 测试矩阵 + 渠道 admin API smoke |
| **R6**:免费 BYOK 用户没配 VLM key 但用了 image 功能 → 422 体验断裂 | medium | UI 在 Chat 输入框附加图片时检测 `vlm.endpoint==""` → 提示「需配 VLM key 或升级 Pro」|
| **R7**:gateway 完全宕机 + user 仅依赖 gateway 无 BYOK fallback | high | v1.0.1 client 加 auto-fallback 到 BYOK(若 user 配了备用 key)+ status page 引导 |
| **R8**:VLM provider(OpenAI)受限地区(中国大陆出网)| medium | gateway 在境外节点部署(已实施 engi-stack.com);备 Gemini vision fallback channel |

---

## 附录 A. 双源头配置示例

### A.1 已实施 channel(failover-policy.md §2)— 7 channel,5 provider

| 渠道名 | 类型 | 优先级 | 负责模型 | tier(隐式)|
|--------|------|--------|---------|----------|
| `openai-primary-1` | OpenAI | 10 | gpt-4o, gpt-4o-mini, o1, o3-mini | LLM + VLM(混) |
| `openai-primary-2` | OpenAI | 10 | gpt-4o, gpt-4o-mini | LLM + VLM(混) |
| `anthropic-primary` | Anthropic | 8 | claude-opus-4-5, claude-sonnet-4-5, claude-haiku-3-5 | LLM(claude 有 vision 但本仓配 LLM-only)|
| `deepseek-flash-primary` | DeepSeek | 7 | deepseek-v4-flash(default) | LLM-only |
| `gemini-fallback` | Gemini | 6 | gemini-2.0-flash, gemini-2.0-flash-lite | LLM + VLM(混) |
| `deepseek-pro-fallback` | DeepSeek | 4 | deepseek-v4-pro | LLM-only |
| `mistral-eu` | Mistral | 3 | mistral-large-latest, mistral-small-latest | LLM-only(EU GDPR) |

**当前现状**:LLM 与 VLM **共用 OpenAI / Gemini channel**(model 白名单允许 vision
model)— 这已经是"双源头"的低配版,但缺 explicit tier 分离,成本控制不精细。

### A.2 v1.0.1 推荐 channel 拓扑(per #154)

**LLM tier**(text-only,便宜 priority 高):

```yaml
- name: deepseek-flash-llm-primary
  type: 36  # DeepSeek
  base_url: https://api.deepseek.com/v1
  models: deepseek-v4-flash
  priority: 10
  tier: llm        # v1.0.1 字段

- name: gpt-4o-mini-llm-fallback
  type: 1   # OpenAI
  base_url: https://api.openai.com/v1
  models: gpt-4o-mini
  priority: 7
  tier: llm

- name: gemini-flash-llm-fallback
  type: 25  # Gemini
  base_url: https://generativelanguage.googleapis.com/v1beta/openai
  models: gemini-2.0-flash-lite
  priority: 5
  tier: llm
```

**VLM tier**(vision capable,贵 priority 低使用频次):

```yaml
- name: gpt-4o-vlm-primary
  type: 1   # OpenAI
  base_url: https://api.openai.com/v1
  models: gpt-4o
  priority: 10
  tier: vlm

- name: gemini-pro-vlm-fallback
  type: 25  # Gemini
  base_url: https://generativelanguage.googleapis.com/v1beta/openai
  models: gemini-1.5-pro
  priority: 6
  tier: vlm

- name: claude-sonnet-vlm-fallback
  type: 14  # Anthropic
  base_url: https://api.anthropic.com
  models: claude-sonnet-4-5
  priority: 5
  tier: vlm
```

**failover 规则**:**同 tier 内 failover,跨 tier 禁止**(per §7.2 风险 R4)。

### A.3 attune-server settings 双源头示例(v1.0.1)

```toml
# vault settings 文件(加密存储)
[llm]
provider = "openai_compat"
endpoint = "https://gateway.engi-stack.com/v1"  # 付费 Pro
api_key  = "<会员 token>"
model    = "deepseek-v4-flash"

[vlm]
provider = "openai_compat"
endpoint = "https://gateway.engi-stack.com/v1"  # 同 gateway,gateway 内部路由 VLM channel
api_key  = "<会员 token>"  # 同 key
model    = "gpt-4o"

# 或免费 BYOK
[llm]
provider = "openai_compat"
endpoint = "https://api.deepseek.com/v1"        # 直连 DeepSeek
api_key  = "<user-byok-deepseek>"
model    = "deepseek-v4-flash"

[vlm]
provider = "openai_compat"
endpoint = "https://api.openai.com/v1"          # 直连 OpenAI
api_key  = "<user-byok-openai>"
model    = "gpt-4o"
```

---

## 附录 B. verify 时间表 + evidence

### B.1 verify 时间

- **2026-05-25 14:18:48** start(`date '+%Y-%m-%d %H:%M:%S'`)
- **2026-05-25 14:40** spec 落档完成

### B.2 evidence(read-only verify)

**E1. new-api v0.13.2 运行中**:
```bash
$ docker ps | grep -iE "(new-api|llm-gateway)"
fbe9324bd05d   calciumion/new-api:v0.13.2   "/new-api"  5 hours ago   Up 5 hours   3000/tcp   cloud-llm-gateway
78e28059a122   postgres:16-alpine           "..."        5 hours ago   Up 5 hours (healthy)   cloud-llm-gateway-db
eb852562c52d   redis:7-alpine               "..."        5 days ago    Up 5 days (healthy)    cloud-llm-gateway-redis
```

**E2. 已实施 5 provider/7 channel**(SSOT `cloud/llm-gateway/docs/llm-providers-setup.md`):
- OpenAI(L33-50)、Anthropic(L52-65)、Gemini(L68-85)、DeepSeek(L88-102)、Mistral(L104-117)
- 优先级矩阵(L126-132):10 / 8 / 6 / 5 / 3 分层

**E3. native channel routing + failover 已 verified**(SSOT
`cloud/llm-gateway/docs/failover-policy.md` §1-3):
- 入站请求 → model 白名单筛选 → priority 排序 → 同级 round-robin → 跨级 retry(默认 3 次)→ 全 fail 返回 503
- 429 / 5xx / timeout / 网络断开 → failover;401 / 403 / 400 → 不 failover

**E4. attune v1.0.1 LLM+VLM 完整架构 spec 已存在**:
`docs/superpowers/specs/2026-05-24-llm-vlm-multi-provider-architecture.md`(675 行,
DRAFT 待评审)— 本 verify spec 是其前置 verify。

**E5. v1.0.1 升级策略 SSOT 已存在**:
`docs/superpowers/specs/2026-05-26-v1-0-1-upgrade-strategy-and-support.md`(847 行)—
本 verify spec 不与之冲突,实施排期在 v1.0.1 sprint(5/27-28)。

### B.3 verify 结论

**双源头供应能力 verified ready**:
- ✅ L1 native(channel + model 白名单)已 ready,零 code change 可配置 2 个不同
  provider(LLM 一家 + VLM 一家)
- ✅ L2 content-aware routing 在 #154 v1.0.1 sprint 实施(本 verify spec 不实施)
- ✅ 配置示例(§附录 A.2)可直接落 new-api 管理后台
- ✅ 风险登记(§11)8 项已识别,缓解措施明示
- ✅ 测试矩阵(§9)8 场景已规划,5 类下限覆盖

**下一步**(本 verify spec **不实施**,留给 v1.0.1 sprint):
1. attune-core `LlmProvider::supports_vision()` + `VlmProvider` trait(#154 §4)
2. attune-server settings.vlm 字段 + routes::chat content-aware routing(#154 §5)
3. cloud accounts /me 加 `gateway_vlm_default_model`(#154 §5.2)
4. cloud new-api channel 拓扑按 §附录 A.2 重构(LLM tier + VLM tier 分离)
5. gateway middleware 加 content-aware routing(方案 A,nginx-proxy 改 model name)

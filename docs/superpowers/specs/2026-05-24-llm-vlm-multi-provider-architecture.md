# LLM × VLM 多 Provider 架构 — 免费 BYOK + 付费 new-api 双 tier 方案

- **Date**: 2026-05-24
- **Type**: Architecture spec（per attune CLAUDE.md「架构级别设计铁律」11 节）
- **Status**: DRAFT — 待评审
- **Scope**: attune-core / attune-server / attune-pro cloud accounts / cloud new-api gateway
- **Owners**: attune-core(LLM/VLM trait & settings) · attune-pro(cloud /me 扩展) · cloud(new-api channel pool)
- **Phased delivery**: v1.0 settings 加 VLM 字段（dormant）→ v1.0.1 cloud /me + new-api 加 VLM channel → v1.1 UI 完整 routing
- **关联**: 
  - `2026-05-19-agent-self-learning-design.md`（LLM gate）
  - `2026-04-19-data-infrastructure-design.md`（settings schema）
  - `2026-05-22-robust-llm-infra.md`（chat trait infra）
  - #89（gateway failover spec）
  - #150（`chat_multimodal` 现 dead，本 spec 让它真起来）
  - `attune-core/src/llm.rs:85` `chat_multimodal` trait method（fallback impl 已在）
  - `attune-core/src/llm_settings.rs::merge_gateway_into_settings`（gateway 合并）

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
- [附录 A. 套餐定价建议](#附录-a-套餐定价建议)
- [附录 B. v1.0 / v1.0.1 / v1.1 phased plan](#附录-b-phased-plan)

---

## 1. 目标定位

### 1.1 用户痛点

attune v1.0 当前 LLM 配置只有**单一 provider 单一 model** 概念：

- `app_settings.llm.endpoint` + `app_settings.llm.api_key` + `app_settings.llm.model` 一组三元
- 所有 LLM 调用（chat / classify / agent verify / summarize / OCR-vision-fallback）共用这一组
- `chat_multimodal` trait method 存在但默认 fallback 是「丢图 + 警告」（per #150 现 dead），真正多模态走不通

这对两类用户都不友好：

- **免费 BYOK 用户**：自带 DeepSeek-v4-flash key 想跑 text-only（便宜 ≈ $0.07/1M token），但同一份 settings 一旦要分析截图就必须切到 GPT-4o-mini（贵 30×），切换体验断裂；很多用户压根不知道 DeepSeek 不支持 vision，输入图片后 silently 丢图
- **付费 attune Pro 用户**：期望「login 一次，所有 AI 能力 just work」，但 cloud /me 当前只下发 `gateway_default_model`（单值），无法同时声明「LLM 跑 deepseek-v4-flash + VLM 跑 gpt-4o」

### 1.2 产品 positioning 对齐

per `2026-04-17-product-positioning-design.md`「混合智能 + 分层成本」+ CLAUDE.md 「Cost & Trigger Contract」：

- LLM（text-only）属于**第三层「时间 / 金钱」**，但单价低（DeepSeek $0.07/$0.28 per 1M in/out）
- VLM（vision）同属第三层，但单价数量级高（GPT-4o $2.50/$10.00 per 1M token，且图片按 patches 算 token，1 张图 ≈ 1K-3K token）
- **统一 routing 不是省事，是省钱** — 把绝大多数 text-only 流量路到便宜 LLM，只让带图的 request 进 VLM channel，成本可下降 5-10×

### 1.3 北极星

- 免费 user：**两套 BYOK**（LLM 必填 / VLM 选填），UI 明示「带图任务需要 VLM key」，零误用
- 付费 user：**login 即配齐**，cloud /me 一次下发 LLM + VLM 默认 model，gateway 内部 content-aware routing，user 不感知 LLM/VLM 分离
- 任何用户：**model 误用绝不静默丢数据**（image 给到 non-vision provider → graceful 提示 + 拒绝，不沿用「drop images + warning」）

---

## 2. 范围边界

### 2.1 做（本 spec 覆盖）

| 模块 | 改动 |
|------|------|
| `attune-core::llm` | 现 `LlmProvider::chat_multimodal` 行为升级 — 增 `fn supports_vision(&self) -> bool { false }`；non-vision provider 收到 `Attachment::Image` → `Err(VaultError::ProviderCapability("VLM required"))` 而非静默丢图；保留「文件附件拼到 user text」行为 |
| `attune-core::llm` | 新 `VlmProvider` trait（继承 `LlmProvider`）`supports_vision() = true`；OpenAI-compat provider 当 `endpoint` 含 `/v1/chat/completions` 且 `model ∈ VISION_MODELS` 时自动 impl |
| `attune-core::llm_settings` | settings schema 加 `vlm` 节（与 `llm` 平级，同 4 字段：`endpoint / api_key / model / provider`）；`merge_gateway_into_settings` 接收 `vlm_default_model: Option<&str>` 参数 |
| `attune-server::routes::settings` | `GET/PUT /api/v1/settings` 支持 `vlm` 字段；validation：endpoint + model 都填或都空 |
| `attune-server::routes::member` | login 走 cloud `/me` 拿 LLM + VLM 两套 default model，分别写 vault settings；`MeResponse` 加 `gateway_vlm_default_model: Option<String>` |
| `attune-server::routes::chat` | 调用入口 inspect `messages[].content`：含 image 的 request → `state.vlm()`，否则 → `state.llm()`；缺 vlm 配置 + 含图 → 422 + i18n 提示 |
| cloud `accounts /me` | response 加 `llm_default_model` + `vlm_default_model`（与现 `gateway_default_model` 兼容 alias） |
| cloud `new-api` admin | channel tier 字段：`tier ∈ {"llm", "vlm"}`；priority 矩阵；request-routing middleware 看 `content[].type == "image_url"` 路 VLM channel；同 tier 内 failover（复用 #89 spec） |

### 2.2 不做（v1.0.x 不在范围）

- 真 VLM E2E pipeline（OCR-VLM 协同、image → bbox → annotation）— v1.1 ocr / office helper 联动 spec 出
- UI 「VLM 配置」面板设计 — 沿用 LLM 面板 UX，v1.1 UI design 单独 spec
- VLM provider 多选 / 自动 failover BYOK fallback — v1.1
- 视频 / audio modality — 长期 backlog（attune 不做实时音视频）
- gateway 内部不同 channel 间 user-side 显式选择 — gateway 对 user 透明，user 改不了 channel

### 2.3 后续 v 不允许扩 scope（pin 死边界）

- v1.0 仅落 settings schema + LlmProvider capability 字段 + member route 兼容 — **dormant**，UI 不展示 VLM 配置
- v1.0.1 才开 cloud channel + chat route content-aware routing
- v1.1 才开 UI / 多 fallback

---

## 3. 架构数据流

### 3.1 总体数据流（ASCII）

```
┌────────────────────────────── 免费 BYOK 用户 ──────────────────────────────┐
│                                                                              │
│  Settings UI                                                                 │
│  ┌─────────────────────────┐    ┌─────────────────────────┐                  │
│  │ LLM provider (必填)     │    │ VLM provider (选填)     │                  │
│  │   endpoint              │    │   endpoint              │                  │
│  │   api_key               │    │   api_key               │                  │
│  │   model                 │    │   model                 │                  │
│  └────────┬────────────────┘    └────────┬────────────────┘                  │
│           │                              │                                   │
│           ▼                              ▼                                   │
│      app_settings.llm                app_settings.vlm                        │
│           │                              │                                   │
└───────────┼──────────────────────────────┼───────────────────────────────────┘
            │                              │
            │                              │
┌───────────┼──────────────────────────────┼─── attune-server (本地) ─────────┐
│           ▼                              ▼                                   │
│  state.llm() : Arc<dyn LlmProvider>    state.vlm() : Arc<dyn VlmProvider>    │
│           │                              │                                   │
│           │     ┌────────────────────────┘                                   │
│           │     │                                                            │
│  ┌────────▼─────▼──────────┐                                                 │
│  │ chat route dispatcher   │                                                 │
│  │  ── inspect content:    │                                                 │
│  │     has image_url? ──┬─→ vlm() ──→ POST endpoint + key (VLM)              │
│  │                      └─→ llm() ──→ POST endpoint + key (LLM)              │
│  └─────────────────────────┘                                                 │
└──────────────────────────────────────────────────────────────────────────────┘


┌─────────────────────────── 付费 Pro 会员 ───────────────────────────────────┐
│                                                                              │
│  attune client login → cloud accounts /me                                    │
│                                                                              │
│  cloud accounts /me response:                                                │
│    {                                                                         │
│      tier: "pro",                                                            │
│      gateway_url: "https://gw.engi-stack.com/v1",                                 │
│      gateway_token: "sk-pro-xxx",                                            │
│      llm_default_model: "deepseek-v4-flash",                                 │
│      vlm_default_model: "gpt-4o",                                            │
│      // legacy alias (v1.0 兼容): gateway_default_model = llm_default_model  │
│    }                                                                         │
│                                                                              │
│  attune-server::member.rs::apply_gateway_to_vault_settings 写两套:           │
│    app_settings.llm = { endpoint: gw, api_key: tok, model: deepseek-... }    │
│    app_settings.vlm = { endpoint: gw, api_key: tok, model: gpt-4o }          │
│                                                                              │
│       │                                  │                                   │
│       └────────┬─────────────────────────┘                                   │
│                │ (共用 gw url + token)                                       │
│                ▼                                                             │
│  cloud llm-gateway (new-api)                                                 │
│    ┌─────────────────────────────────────────────────────────────┐           │
│    │ routing middleware:                                          │           │
│    │   if any msg.content[].type == "image_url" → tier=VLM        │           │
│    │   else → tier=LLM                                            │           │
│    │                                                              │           │
│    │ channel pool:                                                │           │
│    │  ┌── tier=LLM ──────────────────────────────┐                │           │
│    │  │ ch1: deepseek-v4-flash (priority 9)      │                │           │
│    │  │ ch2: deepseek-v4-pro    (priority 7)     │                │           │
│    │  │ ch3: qwen-2.5-72b       (priority 5)     │                │           │
│    │  │ ch4: claude-haiku       (priority 3)     │                │           │
│    │  └──────────────────────────────────────────┘                │           │
│    │  ┌── tier=VLM ──────────────────────────────┐                │           │
│    │  │ ch5: gpt-4o             (priority 9)     │                │           │
│    │  │ ch6: gemini-1.5-pro     (priority 7)     │                │           │
│    │  │ ch7: claude-sonnet      (priority 5)     │                │           │
│    │  │ ch8: qwen-vl-max        (priority 3)     │                │           │
│    │  └──────────────────────────────────────────┘                │           │
│    │                                                              │           │
│    │ 同 tier 内 failover (per #89): priority 高 → 低, quota 控制   │           │
│    └─────────────────────────────────────────────────────────────┘           │
└──────────────────────────────────────────────────────────────────────────────┘
```

### 3.2 内部组件视图

```
attune-core
 └── llm.rs
      ├── trait LlmProvider           # text-only 契约
      │    ├── chat()                  # 主入口
      │    ├── supports_vision() -> false   # 新增（默认 false）
      │    └── chat_multimodal()       # 默认行为：见 image → Err
      │
      └── trait VlmProvider : LlmProvider   # vision 契约（新 trait）
           ├── supports_vision() -> true (override)
           └── chat_multimodal()       # 真 vision impl (OpenAI / Gemini / Claude)

 └── llm_settings.rs
      ├── struct AppSettings
      │    ├── llm: LlmConfig          # 已有
      │    └── vlm: Option<LlmConfig>  # 新增（同结构，可选）
      ├── gateway_should_apply()       # 已有
      └── merge_gateway_into_settings(
            settings, gw_url, gw_token,
            llm_default_model: Option<&str>,
            vlm_default_model: Option<&str>,   # 新增参数
        )

attune-server
 └── AppState
      ├── llm: Arc<Mutex<Arc<dyn LlmProvider>>>     # 已有
      └── vlm: Arc<Mutex<Option<Arc<dyn VlmProvider>>>>  # 新增（可空）
      └── pub fn vlm(&self) -> Option<Arc<dyn VlmProvider>>  # accessor

 └── routes/
      ├── settings.rs  # GET/PUT 加 vlm 字段处理
      ├── member.rs    # apply_gateway 加 vlm_default_model 参数
      ├── chat.rs      # dispatcher：inspect image_url → 选 vlm() / llm()
      └── llm.rs       # /api/v1/llm/test 加 ?target=vlm 参数

cloud accounts
 └── /me response 字段
      ├── gateway_default_model           # legacy (= llm_default_model)
      ├── llm_default_model               # 新
      └── vlm_default_model               # 新（Option）

cloud llm-gateway (new-api)
 └── channels[].tier ∈ {"llm","vlm"}     # schema migration
 └── middleware: content-type routing
 └── failover within tier (per #89)
```

---

## 4. 模块边界

### 4.1 涉及 crate / module / file

**attune-core**:
- `src/llm.rs`：trait `LlmProvider` 加 `supports_vision()`；新 trait `VlmProvider`；`chat_multimodal` default impl 调整为「见 image → Err」
- `src/llm_settings.rs`：`AppSettings` 加 `vlm: Option<LlmConfig>`；`merge_gateway_into_settings` 签名扩参
- `src/llm/openai_compat.rs`（若已分离）/ `src/llm.rs` OpenAI provider：当 model 匹配 `VISION_MODELS` 名单或 caller 显式声明 vlm 时 impl `VlmProvider`

**attune-server**:
- `src/state.rs`：`AppState` 加 `vlm` 字段 + accessor
- `src/routes/settings.rs`：schema validation + UI 数据回传
- `src/routes/member.rs`：`apply_gateway_to_vault_settings` 接受 vlm_default_model；写两套 settings
- `src/routes/chat.rs`：dispatcher 函数 `select_provider(content) -> ProviderKind::{Llm,Vlm}`
- `src/routes/llm.rs`：test connection 端点支持 `?target=llm|vlm`

**attune-pro cloud**:
- `accounts/serializers.py` 或对应 Rust impl：`UserMeResponse` 加 `llm_default_model` + `vlm_default_model`
- `accounts/views.py::me`：从 plan / user config 读两个字段

**cloud new-api (vendored fork)**:
- `model/channel.go`：`Channel` struct 加 `Tier string`（"llm"|"vlm"）
- `controller/relay.go`：request middleware inspect `messages[].content[].type`
- DB migration：`ALTER TABLE channels ADD COLUMN tier VARCHAR(8) DEFAULT 'llm'`

### 4.2 跨仓边界

| 仓 | 改动文件数（估算） | 是否独立 PR |
|----|---|---|
| attune (本仓) | core 2 + server 5 = 7 | 一个 PR per phase |
| attune-pro / cloud accounts | 2 | 独立 PR，version bump 兼容 |
| cloud new-api (vendored) | 3 + migration | 独立 PR |

---

## 5. API 契约

### 5.1 attune-server REST

**`GET /api/v1/settings`** （response 加 vlm 节）:

```json
{
  "llm": {
    "provider": "openai_compat",
    "endpoint": "https://api.deepseek.com/v1",
    "model": "deepseek-v4-flash",
    "api_key": "sk-***"
  },
  "vlm": {
    "provider": "openai_compat",
    "endpoint": "https://api.openai.com/v1",
    "model": "gpt-4o",
    "api_key": "sk-***"
  }
}
```

`vlm` 为 `null` / 缺省 → UI 显示「未配置 VLM」+ 灰化所有图相关 action。

**`PUT /api/v1/settings`** （request body 同上 schema，validation）:
- `llm.endpoint` + `llm.model` 必填
- `vlm` 可省（null）；若非 null，则 endpoint + model 必填，api_key 可空（gateway 共用 LLM 的）
- error code:
  - `llm-endpoint-required`
  - `vlm-incomplete`（endpoint 填了 model 没填）
  - `vlm-model-unknown-capability`（warning，不 reject）

**`POST /api/v1/llm/test?target=llm|vlm`** :
- `target=llm`（default）→ 跑 `state.llm().chat("","ping")` ≤ 5s timeout
- `target=vlm` → 跑 `state.vlm().chat_multimodal("","describe", &[Attachment::Image{small_test_png}])` ≤ 10s timeout
- success: `{"ok": true, "model": "...", "latency_ms": 234}`
- failure: `{"ok": false, "error": "...", "code": "vlm-not-configured" | "provider-unreachable" | ...}`

### 5.2 chat dispatcher 内部

```rust
enum ProviderKind { Llm, Vlm }

fn select_provider(messages: &[ChatMessage]) -> ProviderKind {
    let has_image = messages.iter().any(|m| {
        m.attachments.iter().any(|a| matches!(a, Attachment::Image { .. }))
    });
    if has_image { ProviderKind::Vlm } else { ProviderKind::Llm }
}
```

`POST /api/v1/chat` 入口若 `select_provider() == Vlm` 且 `state.vlm().is_none()` → 422：

```json
{"error": "vision request requires VLM provider configuration", "code": "vlm-not-configured"}
```

### 5.3 cloud accounts /me

```json
{
  "user_id": "...",
  "tier": "pro",
  "plan": "pro_plus",
  "gateway_url": "https://gw.engi-stack.com/v1",
  "gateway_token": "sk-pro-xxx",
  "gateway_default_model": "deepseek-v4-flash",
  "llm_default_model": "deepseek-v4-flash",
  "vlm_default_model": "gpt-4o"
}
```

v1.0 client 只读 `gateway_default_model`（向后兼容）；v1.0.1 client 读 `llm_default_model` + `vlm_default_model`。

### 5.4 cloud new-api channel admin

```json
PUT /admin/channels/{id}
{
  "name": "deepseek-v4-flash",
  "tier": "llm",        // 新字段
  "priority": 9,
  "models": ["deepseek-v4-flash", "deepseek-chat"],
  "base_url": "https://api.deepseek.com/v1",
  "api_key": "sk-deepseek-..."
}
```

routing middleware（pseudo）:

```go
func RouteByTier(req *RelayRequest) (Channel, error) {
    hasImage := false
    for _, m := range req.Messages {
        for _, c := range m.Content {
            if c.Type == "image_url" { hasImage = true; break }
        }
    }
    tier := "llm"
    if hasImage { tier = "vlm" }
    return selectChannelByTierAndPriority(tier, req.Model)
}
```

---

## 6. 扩展点 / 插件接口

### 6.1 加新 LLM provider（cloud 侧）

1. cloud admin 后台「新增 channel」→ `tier=llm` + priority < 9 (preserve DeepSeek-flash 为最优)
2. 自动进入 failover 序列
3. 无需 attune client 变更（gateway 对 user 透明）

### 6.2 加新 VLM provider（cloud 侧）

1. cloud admin 后台「新增 channel」→ `tier=vlm` + priority
2. attune client 通过 cloud /me 拿到 `vlm_default_model`，若需切默认 model（如从 gpt-4o → gemini-1.5-pro），admin 改 user record `vlm_default_model` 即可，client 下次 login 同步

### 6.3 BYOK 用户切 provider

- v1.0：直接改 settings.llm / settings.vlm 字段
- v1.x：UI 引导式 wizard，支持「已知 provider 模板」（DeepSeek / OpenAI / Anthropic / Google / Qwen）一键填好 endpoint，user 只填 key + model

### 6.4 多 VLM provider 自动 fallback（v1.2 backlog）

- BYOK 用户可在 settings 配多组 vlm（`vlm: LlmConfig` → `vlm_providers: Vec<LlmConfig>`）
- 不在本 spec 范围

---

## 7. 错误处理 + 边界 case

### 7.1 错误码（kebab-case，per attune `AppError` 规约）

| code | HTTP | 触发场景 | UI 行为 |
|------|------|---------|---------|
| `vlm-not-configured` | 422 | chat 含图但 settings.vlm null | 弹窗「请在设置中配置 VLM provider 或移除图片」 |
| `vlm-incomplete` | 400 | PUT settings vlm 字段不全 | 表单内联错误 |
| `vlm-model-unsupported` | 400 | 模型名不在已知 vision 名单 + 未通过 test | warning 不 reject，记 audit |
| `provider-unreachable` | 502 | endpoint 网络失败 | 显示 retry / 检查网络 |
| `provider-capability-mismatch` | 422 | 显式调 vlm 但 provider supports_vision=false | 内部 bug 提示 |
| `gateway-channel-exhausted` | 503 | cloud gateway 该 tier 所有 channel 都失败 | 「服务繁忙，请稍后」+ audit |
| `vlm-quota-exceeded` | 429 | Pro 用户 VLM quota 用尽 | 显示当前用量 + 升级链接 |

### 7.2 graceful degradation

1. **VLM 不可用但 OCR 可用**：用户上传 PDF 截图 → 先走 OCR-first（PP-OCRv5），结果文本进 LLM；只在 user 显式点「让 AI 看图」时才升 VLM
2. **VLM endpoint timeout**：单次 chat 超过 30s → fallback 提示 user「VLM 响应慢，是否改用 OCR + LLM？」
3. **付费用户 cloud /me 拿不到 vlm_default_model**：保持 vlm settings 为 null，UI 提示「Pro VLM 功能即将开放（v1.0.1）」

### 7.3 边界 case

| case | 行为 |
|------|------|
| user 把 `vlm.endpoint` 设成等于 `llm.endpoint`（共用 gateway）| 合法；test_vlm 时 model 字段决定是否走 vision channel |
| user 配 vlm 但 model 不是 vision 模型（如 `deepseek-v4-flash`）| `vlm-model-unsupported` warning；test 时 server 调 chat_multimodal 应回 `provider-capability-mismatch` |
| Pro 用户登录前已配 BYOK | per `gateway_should_apply` — 已有 vlm config 不覆盖；audit log 记 "user has own VLM config" |
| Pro 用户 logout | vlm settings 不自动清；user 显式「重置 AI 配置」才清 |
| Image attachment 但 size > 10 MB | 422 `image-too-large`（先于 provider 选择） |
| 同一 chat 多轮中混合（轮 1 无图 → llm；轮 2 有图 → vlm）| 按每轮独立 select_provider；context 保留（history 一并传） |
| 文本附件（CSV / md）+ 图同存 | text 拼到 user content，image 走 vlm；attachments[] 两类并存 |
| 没装 `image` crate / preview 失败 | image data 仍透传给 VLM provider（base64 dataurl），不在 client 解码 |

---

## 8. 成本契约

per CLAUDE.md「Cost & Trigger Contract」：

### 8.1 三层归属

| 调用类型 | 层级 | trigger | UI 显示 |
|---------|------|---------|---------|
| chat text-only → LLM | 💰 第三层（金钱） | user 显式 send | `~1.2K tok · ¥0.0003` |
| chat with image → VLM | 💰 第三层（金钱） | user 显式 send + 含图 | `~3.5K tok (image: 2K) · ¥0.0080` 🖼 |
| OCR-first（PP-OCRv5）| ⚡ 第二层（本地算力） | 后台 / 主动 | `~本地 · 1.2s` |
| Office helper 文档摘要 → LLM | 💰 第三层 | 用户点「分析」| 同 chat |

### 8.2 BYOK 免费用户

- 完全自费，attune 不收任何抽成
- chat 发送前 UI 显示估算（按 endpoint 已知 pricing；未知 provider 显示 `~? tok · 计费方自定`）
- 月度统计：settings 页显示「本月 LLM 用量 / VLM 用量」（local-only，不上报 cloud）

### 8.3 Pro 付费用户（cloud gateway）

- gateway 计费透明：dashboard 显示「LLM tokens used / VLM tokens used / quota remaining」
- LLM quota 用尽 → 不影响 VLM quota
- VLM quota 用尽 → 不影响 LLM quota
- 套餐设计见附录 A

### 8.4 UI 强制显示

每次 chat send 按钮旁：
- text-only: `LLM · deepseek-v4-flash · ~1.2K tok · ¥0.0003`
- with image: `VLM · gpt-4o · ~3.5K tok · ¥0.0080 · 🖼 1 image`

Settings 页两个 chip：「LLM provider: deepseek-v4-flash @ DeepSeek」「VLM provider: gpt-4o @ OpenAI」

---

## 9. 测试矩阵

per 「Agent 验证铁律」6 类下限。本 spec 涉及 provider 路由 + capability 检测，非 agent，但仍参考结构。

### 9.1 单元测试（attune-core）

| 类型 | 数量 | 文件 |
|------|------|------|
| LlmProvider trait default `supports_vision()` 返 false | 1 | `llm.rs::tests` |
| `chat_multimodal` 默认 impl 见 Image → Err | 1 | 同上 |
| `chat_multimodal` 默认 impl 仅 TextFile → 拼接 + chat | 1（已有，扩） | 同上 |
| `VlmProvider` impl supports_vision = true | 1 | 同上 |
| `merge_gateway_into_settings` 接受 vlm_default_model → 写 vlm 节 | 3（None / Some(空) / Some(model)） | `llm_settings.rs::tests` |
| `merge_gateway_into_settings` user 已配 BYOK vlm → 不覆盖 | 1 | 同上 |
| `AppSettings` deserialize 老 json（无 vlm 字段）→ vlm = None | 1 | 同上 |
| `AppSettings` deserialize 新 json with vlm | 1 | 同上 |

### 9.2 集成测试（attune-server）

| 类型 | 文件 |
|------|------|
| `PUT /api/v1/settings` 含 vlm → 持久化 + `GET` 回传 | `tests/settings_vlm.rs` |
| `PUT /api/v1/settings` vlm.endpoint 填 vlm.model 空 → 400 `vlm-incomplete` | 同 |
| `POST /api/v1/chat` 文本 → 走 llm() mock provider | `tests/chat_routing.rs` |
| `POST /api/v1/chat` 含 image_url + vlm 已配 → 走 vlm() mock | 同 |
| `POST /api/v1/chat` 含 image_url + vlm 未配 → 422 `vlm-not-configured` | 同 |
| `POST /api/v1/llm/test?target=vlm` 无 vlm config → 422 | `tests/llm_test_endpoint.rs` |
| member login Pro → cloud /me mock 返 vlm_default_model → settings.vlm 写入 | `tests/member_vlm.rs` |
| member login Pro → user 已 BYOK vlm → 不覆盖 | 同 |

### 9.3 属性测试（proptest）

- `chat_multimodal` 输入任意 mix of TextFile + Image attachments：non-vision provider 总是 Err，vision provider 总是不 panic（≥ 100 cases）
- `select_provider(messages)` for any messages: 含 image → Vlm，否则 → Llm（≥ 100 cases）
- `merge_gateway_into_settings` idempotent：连续两次同输入 → 同输出（≥ 100 cases）

### 9.4 边界 / 异常

| case | 测试位置 |
|------|---------|
| image 0 字节 | unit |
| image 11 MB（超阈） | integ |
| 100 张 image 一次 chat | integ |
| vlm endpoint 返 5xx | integ（mock server） |
| vlm endpoint timeout 35s | integ |
| settings.vlm.model = "" | unit (validation) |

### 9.5 E2E

- Playwright：Settings UI 配 BYOK vlm key → chat 上传图 → 验证 request body 走 vlm endpoint
- Playwright：Pro 用户 login → settings 页自动展示 LLM + VLM 两套已填值
- 不在 v1.0 必须，v1.1 UI 上线后补

### 9.6 回归 fixture

每修一个 bug：reproducer 进 golden set（YAML / Rust fixture），不允许只在本地修验证就 close

---

## 10. 向后兼容

### 10.1 schema migration

| version | settings json | client 行为 |
|---------|--------------|------------|
| v0.x → v1.0 | `{llm: {...}}` (no vlm) | vlm 字段缺 → 视为 None；正常 |
| v1.0 | `{llm: {...}, vlm: null}` 或 `{llm: {...}, vlm: {...}}` | 两种都 accept |
| v1.0.1+ | 同 v1.0 schema | 加 routing 但 schema 不变 |

**SQLite 持久化**：`app_settings` 是 JSON BLOB（不是结构化字段），无需 DB migration，serde 默认 `Option<LlmConfig> = None` 处理 missing。

### 10.2 cloud /me 字段兼容

| field | v1.0 client 读 | v1.0.1 client 读 |
|-------|-----------|--------------|
| `gateway_default_model` | ✅ | ✅（fallback） |
| `llm_default_model` | ✗（忽略） | ✅（优先） |
| `vlm_default_model` | ✗（忽略） | ✅（写 vlm 节） |

cloud 同时下发三字段，新老 client 各取所需。**deprecate `gateway_default_model` 时机**：v1.2 + 6 个月。

### 10.3 trait API 兼容

- `LlmProvider::chat()` / `chat_multimodal()` 签名不变
- 新增 `supports_vision()` 有 default impl → 现有 provider 实现不破坏
- `VlmProvider` 是新 trait，老 provider 不被强制 impl

### 10.4 client config migration

老用户升级到 v1.0：
- 现有 `llm` config 保留
- 弹一次性提示「v1.0 新增 VLM 功能（v1.0.1 上线），稍后请配置」
- 不强制 user 行动

---

## 11. 风险登记

| ID | 风险 | 严重度 | 缓解 |
|----|------|-------|------|
| R1 | Pro user 不知 gateway LLM/VLM 计费透明度，看到 ¥¥ 怀疑黑箱 | 高 | settings 页 + dashboard 显示「LLM ¥0.0003/调用 · VLM ¥0.008/调用」明细，每次 send 显示估算 |
| R2 | VLM 单价高 5-30×，user 误把所有 chat 走 VLM → 月底账单爆炸 | 高 | routing middleware 严格 content-aware（无 image → 强制 LLM）；UI 不允许 user 显式「强制走 VLM」 |
| R3 | 免费 user BYOK vlm endpoint 配错（如把 DeepSeek 当 vlm） | 中 | 配置时 `POST /llm/test?target=vlm` 跑真 image 测试；失败 → block save，给清晰错误 |
| R4 | cloud gateway 内部 LLM/VLM 误路由（content-type 检查 bug） | 高 | 加单测 + audit log（每个 request 记 tier）；周报对比 tier 误路由率 |
| R5 | user 改 model 到 channel 不支持的（如 `gpt-5`）| 中 | gateway 返 404，client graceful 提示「model 不存在，请检查」+ 推荐已知 model 列表 |
| R6 | OCR-first 与 VLM 边界混乱（什么时候走哪个）| 中 | 默认 OCR-first（便宜 + 快），user 显式「让 AI 看图」才升 VLM；UI 双按钮明示 |
| R7 | v1.0 dormant + v1.0.1 激活间隔 user 已配 BYOK vlm → cloud /me 想下发被 gateway_should_apply 拒 | 低 | per spec 行为正确：user 优先；audit log 记原因 |
| R8 | new-api fork drift（上游 schema 变更）| 中 | vendored fork 锁版本；每季 sync 一次 + 跑兼容 test |
| R9 | non-vision provider 收到 image 改为 Err → 老 caller 期待 fallback 字符串 | 中 | 内部 caller 已全部走 select_provider 路由（不会跨调）；compat layer 提供「downgrade to text」选项给 explicit caller |
| R10 | settings vlm 字段加密（含 api_key）字段级 AES → 老 vault 无此字段，opening 时 panic | 低 | per Rust 商用线设计，字段级加密通过 serde transparent，新字段缺省走 None 路径，不 panic；测试覆盖 |

---

## 附录 A. 套餐定价建议

**前置说明**：本节是**建议**，待 user / 商务决策。报告给用户的版本应保持 ¥ 单位 + 一目了然。

### A.1 配额单位与典型用量

- 1 张图（attune 内典型 1080p screenshot）≈ 1.5K-2.5K input token（VLM 按 patch 算）
- 1 次 chat text-only 平均 1.5K token in + 500 token out ≈ 2K token
- 1 次 chat with image 平均 4K token in + 800 token out ≈ 5K token

### A.2 套餐矩阵（建议）

| 套餐 | 月价 | LLM token | VLM 调用数（≈ token）| 目标人群 |
|------|------|----------|---------------------|---------|
| **Free（注册即送）** | ¥0 | 5 万 | 0 | 试用 + 极轻量个人 |
| **Lite 个人** | ¥29 | 100 万 | 0 | 不用图的 KB 用户 |
| **Basic 个人** | ¥99 | 300 万 | 100 调用（≈ 50 万 token）| 偶尔分析截图 / 文档 |
| **Pro 个人** | ¥299 | 1000 万 | 500 调用（≈ 250 万 token）| 频繁多模态 + agent 自动化 |
| **Pro+ 个人** | ¥599 | 3000 万 | 2000 调用（≈ 1000 万 token）| 重度 / 创作者 |
| **企业 / 一体机** | 自定义 | 自定义 | 自定义 | K3 / 团队 |

### A.3 超额策略

- 超额 LLM：按 ¥0.001 / 1K token 计（≈ DeepSeek-flash 终端价 2× markup）
- 超额 VLM：按 ¥0.05 / 调用 计（≈ GPT-4o 终端价 1.5× markup）
- 超 200% 配额 → 自动 throttle 提示升级，不直接 block（避免 user 工作中断）

### A.4 与现行 attune Pro 关系

- 当前 attune Pro 仅区分 free / pro / pro_plus / enterprise（per `member.rs`）
- 本 spec 不要求立即细分到 6 档，但**至少需要分 free / pro / pro_plus**，且每档明示 LLM + VLM quota
- 落地建议：v1.0.1 cloud accounts 加 quota 字段（`llm_quota` / `vlm_quota` / `llm_used` / `vlm_used`）+ dashboard，下发到 client 显示

---

## 附录 B. v1.0 / v1.0.1 / v1.1 phased plan

### Phase v1.0（5/25 GA，本 spec 落 dormant 部分）

**Time budget**: 1 天 implementation + 0.5 天 review

**Deliverable**:
- attune-core/src/llm.rs：trait 加 `supports_vision()` default false + `VlmProvider` 新 trait
- `chat_multimodal` 默认行为：见 Image → Err（**breaking** for explicit caller，但内部全部走 select_provider 不受影响）
- attune-core/src/llm_settings.rs：`AppSettings.vlm: Option<LlmConfig>` + `merge_gateway_into_settings` 加 vlm_default_model 参数
- attune-server::AppState：加 `vlm` 字段（值 None）+ accessor
- attune-server::routes::settings：accept / persist vlm 字段（但 UI 不展示）
- attune-server::routes::member：apply_gateway 兼容 vlm_default_model（cloud /me 暂不下发，传 None）
- 测试：9.1 全部 + 9.2 settings_vlm.rs

**不做**: chat dispatcher routing / llm/test target / UI / cloud /me / new-api channel

**Exit criteria**: 现有所有测试通过 + 新增 vlm settings round-trip 通过 + `cargo clippy` 干净

### Phase v1.0.1（GA 后 1 周内，**主激活**）

**Time budget**: 2 天 implementation + 1 天 E2E

**Deliverable**:
- attune-server::routes::chat：select_provider dispatcher + 缺 vlm 含图 → 422
- attune-server::routes::llm：test endpoint 加 `?target` 参数
- cloud accounts /me：返 `llm_default_model` + `vlm_default_model`（基于 plan 默认值）
- cloud new-api：channel.tier 字段 + DB migration + routing middleware
- cloud admin 后台：channel tier 编辑 UI
- 测试：9.2 chat_routing + member_vlm + llm_test_endpoint 全部

**Exit criteria**: BYOK 用户 / Pro 用户两条路径都能跑通端到端含图 chat；金额估算 UI 显示正确

### Phase v1.1（约 6/15，UI 完整化）

**Time budget**: 3 天 design + 5 天 implementation

**Deliverable**:
- Settings UI：LLM provider + VLM provider 双面板（共用同 4 字段模板）
- 配置 wizard：「已知 provider」一键填模板（DeepSeek / OpenAI / Anthropic / Gemini / Qwen）
- chat 输入框：上传图片时实时显示「将用 VLM provider · 预估 ¥0.008」
- dashboard：LLM / VLM 月度用量 + quota 进度条
- 测试：Playwright E2E 覆盖配置 → chat → 发送整链路

### Phase v1.2+ backlog（非本 spec）

- BYOK 多 VLM provider fallback（`vlm_providers: Vec<LlmConfig>`）
- 自动 model 推荐（根据用户文档类型 / 历史用量）
- VLM ↔ OCR 智能切换（高分辨率截图先 OCR、低分辨率走 VLM）
- 视频 / audio modality（长期）

---

**END OF SPEC**

待评审：本文档 commit 进 develop 即触发评审。无评审 / 评审未过禁止进入 implementation plan 阶段。

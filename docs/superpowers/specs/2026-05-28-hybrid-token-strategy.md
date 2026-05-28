# Spec: Hybrid Token Strategy(边缘计算 + Token Agent 路由)

- **Date**: 2026-05-28
- **Author**: AI architect (auto-drafted, pending user review)
- **Status**: DRAFT — awaiting v1.1+ slot
- **Tracker**: TBD
- **Linked specs**:
  - `2026-05-28-cache-context-token-standard-api.md`(本 spec 的 data 输入由其提供)
  - `2026-04-27-resource-governor-design.md`(任务级 budget)
  - `2026-05-22-robust-llm-infra.md`(LLM 多 provider 加固)
  - 全局 CLAUDE.md §1.3「算力资源授权」+ §4.5「LLM Agent 兜底原则」
  - 项目 CLAUDE.md「成本感知与触发契约」+「硬件感知的默认底座」

---

## TL;DR

attune 当前已有「三层成本」表(零成本 CPU / 本地算力 NPU/GPU / 远端 token),但**没有可执行的路由策略**。
什么任务必须走 edge(本地)、什么允许走 token agent(远端)、什么动态二选一、如何 cost-aware
fallback —— 散落在各 `chat.rs` / `classifier.rs` / `embed.rs` 内 hardcoded `if config.provider == ...`,
缺统一 router + 缺规则表 + 缺可见度。

本 spec 把混合 token 策略**明确成一份产品/工程双语 contract**:
1. **Capability classification matrix** — 全 capability 表,标 edge-only / hybrid / token-only
2. **Routing rules** — 输入(query / hardware / user tier / cost budget / latency SLO)→ 决策(本地 / cloud / fallback chain)
3. **Cost-aware fallback** — 三 tier 链(本地兜底 → cloud 主路径 → 备用 provider),用前面 spec 的 UsageEvent 实时数据驱动切换
4. **Docs 强制展示** — 这 contract 出现在 `docs/HYBRID-TOKEN-STRATEGY.md`(产品级 SSOT)+ 引用进 README / DEVELOP

---

## 1. 目标定位

### 1.1 解决什么用户痛点

| 痛点 | 现状 | 本 spec 解决 |
|------|------|--------------|
| 「为什么这个搜索这么慢?」 | search 走 embed → 本地 bge-m3,用户不知道在用本地算力 | UI 透明显示:每个 capability 标 ⚡ 本地 / 💰 远端 |
| 「我的 API key 快用完了能不能少用」 | 没 cost-aware,跑满到 quota error | 实时 budget tracking + 接近上限自动切本地 fallback |
| 「我 K3 一体机为啥还要远端 token」 | K3 镜像形态预装 LLM 但代码默认走 cloud | 硬件检测 → K3 形态默认 routing.profile = `edge_first` |
| 「LLM gateway 挂了怎么办」 | 单 provider hardcoded,挂了用户 chat 全 fail | fallback chain:cloud_primary → cloud_secondary → local_ollama → degrade message |
| 「跑哪个模型用户说了不算」 | settings 一个 model 字段写死 | 任务级 routing(extract 用 cheap model / chat 用 main / vision 单选)+ 用户可 override |
| 「我没网络能用吗」 | 部分 capability 假设有 cloud | offline mode 自动检测 + degrade 到 edge-only,UI 明示哪些 disabled |

### 1.2 产品 positioning 对齐

- **三层成本**:本 spec 是「成本感知与触发契约」节的可执行实现
- **隐私优先**:`edge_first` profile = 默认不出本地 → 满足 1Password 式承诺
- **混合智能**:本地优先,云端补强 — 不是「all-local」也不是「all-cloud」,本 spec 给出每 capability 的具体落点
- **硬件感知**:复用 `platform::detector` 与 wizard hardware 检测,routing 默认 profile 由硬件决定
- **K3 一体机**:Edge-first profile,LLM 走 K3 :8080,可完全离线

---

## 2. 范围边界

### 2.1 做什么(v1.1 + v1.2 分批)

✅ **v1.1.0**:
- `attune-core::routing` 新 module — `CapabilityRouter` + `RoutingProfile` + `RoutingDecision`
- Capability classification matrix(全 14 capability 表)落 `docs/HYBRID-TOKEN-STRATEGY.md` SSOT
- 三种 profile:`edge_first` / `balanced`(默认)/ `cloud_first`,wizard 选硬件后自动推荐
- 任务级 model override — `settings.routing.{chat,extract,classify,vision}` 各自一个 routing rule
- Cost budget watcher — 接近 attune-pro Pro 配额 80% 自动切 fallback,UI 红点
- Offline detection — 检测 cloud_gateway / Ollama / K3 :8080 可达性,自动选 alive 路径
- README + DEVELOP + 官网 wiki 引用 `docs/HYBRID-TOKEN-STRATEGY.md`

✅ **v1.2.0**:
- Semantic-aware routing — 短 query(<200 char)走 edge / 长文档(>10K char)走 cloud 长上下文
- Adaptive fallback — 历史 UsageEvent.outcome 数据驱动 router 学习(provider X 7 天失败率 > 20% → 降权)
- A/B routing experiment — 用户开 `routing.experiment=true` 时,系统记录两套路径对照数据

### 2.2 不做什么

❌ 自动 model 比价 / API 比价采购 — 走 attune-pro cloud gateway 单源
❌ 多用户负载均衡(单用户单机定位)
❌ Provider 自动 spawn(不会自动启 Ollama daemon — per CLAUDE.md §1.3 算力授权)
❌ 完全自动 / 学习驱动 — 用户始终可手动 override,routing 仅 recommend
❌ Distributed routing(K3 + 笔电协同推理) — v2.x 才考虑

### 2.3 v.x 后续

| 版本 | 增量 |
|------|------|
| v1.3 | Local LLM auto-pull(检测硬件够 → 提示 ollama pull qwen2.5,不强制) |
| v1.4 | Multi-K3 联机(家庭多设备共享 K3 推理服务) |
| v2.0 | 真正 distributed inference(笔电 prefill + K3 decode) |

---

## 3. 架构数据流

```
┌─────────────────────────────────────────────────────────────────────────┐
│  Caller(chat / agent / classify / search / extract)                    │
│  intent: { kind, prompt_len, latency_slo, user_tier, override? }        │
└──────────────────────────────┬──────────────────────────────────────────┘
                                │
                                ▼
                ┌──────────────────────────────────┐
                │   CapabilityRouter (NEW)         │
                │   .decide(intent) → Decision     │
                └──────┬───────────────────────────┘
                       │
            ┌──────────┴────────────────────────────┐
            ▼                                        ▼
   ┌─────────────────┐                    ┌────────────────────┐
   │ Inputs          │                    │ Rules engine       │
   │ - profile       │                    │ - capability matrix│
   │ - hardware      │                    │ - cost budget      │
   │ - usage_summary │                    │ - latency SLO      │
   │ - offline_state │                    │ - fallback chain   │
   │ - user_override │                    └────────┬───────────┘
   └────────┬────────┘                             │
            └──────────────┬──────────────────────┘
                           ▼
                ┌──────────────────────────────────┐
                │   RoutingDecision                │
                │   {                              │
                │     primary: ProviderEndpoint,   │
                │     fallback_chain: Vec<...>,    │
                │     reason: String,              │
                │     est_cost_usd: Option<f64>,   │
                │     est_latency_ms: u32          │
                │   }                              │
                └────────┬─────────────────────────┘
                         │
       ┌─────────────────┼─────────────────┐
       ▼                 ▼                 ▼
   ┌─────────┐     ┌────────────┐    ┌─────────────────┐
   │ Edge:   │     │ Token Agent │    │ Vendor cloud    │
   │ Ollama  │     │ (cloud GW)  │    │ direct (BYOK)   │
   │ K3:8080 │     │ engi-stack  │    │ OpenAI/Claude...│
   └────┬────┘     └─────┬──────┘    └─────────┬───────┘
        │                │                      │
        └────────┬───────┴──────────────────────┘
                 ▼
       ┌──────────────────────┐
       │ UsageEvent recorded  │ ← per cache/context/token spec
       │ → feedback to router │   (fail rate → demote provider)
       └──────────────────────┘
```

### Inputs 数据来源

| Input | 来源 | 更新频率 |
|-------|------|---------|
| `RoutingProfile` | `settings.routing.profile` 用户配置 | 仅用户改时 |
| `HardwareSnapshot` | `crate::platform::detector` 启动时探测 | 启动 + 用户点「刷新硬件」 |
| `UsageSummary` | 新 `usage_events` 表(per 上一份 spec)聚合 | 实时(router 决策前 5s cache) |
| `OfflineState` | `cloud_client::health_check` + Ollama `/api/tags` + K3 `/health` | 每 30s |
| `BudgetWatcher` | attune-pro cloud gateway `/v1/usage/quota`(Pro 用户) | 每 60s |
| `UserOverride` | 当次 request 的 `?provider=` query 参数 | 仅当次 |

### 三种 Routing Profile

```
┌────────────────┬─────────────────┬─────────────────┬──────────────────┐
│   profile      │  edge_first     │  balanced(默认)│  cloud_first     │
├────────────────┼─────────────────┼─────────────────┼──────────────────┤
│ Chat           │ Ollama 本地     │ cloud_gateway   │ cloud_gateway    │
│                │ ↓ cloud fb      │ ↓ Ollama fb     │ ↓ BYOK fb        │
│ Embedding      │ 本地 bge-m3     │ 本地 bge-m3     │ cloud embed      │
│ OCR            │ 本地 PP-OCR     │ 本地 PP-OCR     │ 本地 PP-OCR(无云)│
│ ASR            │ 本地 whisper    │ 本地 whisper    │ 本地 whisper     │
│ VLM            │ disabled(本地无)│ cloud_gateway   │ cloud_gateway    │
│ Extract(agent) │ Ollama qwen3b   │ cloud_gateway   │ cloud_gateway    │
│ Classify       │ Ollama qwen3b   │ Ollama qwen3b   │ cloud cheap      │
│ 触发条件      │ K3 / 旗舰本地    │ 任何笔电        │ Pro 用户主推     │
└────────────────┴─────────────────┴─────────────────┴──────────────────┘
```

---

## 4. 模块边界

### 4.1 新增

| Path | 角色 |
|------|------|
| `attune-core/src/routing/mod.rs` | `CapabilityRouter` 主入口 |
| `attune-core/src/routing/profile.rs` | `RoutingProfile` enum + 三种 profile 默认规则 |
| `attune-core/src/routing/decision.rs` | `RoutingDecision` + `ProviderEndpoint` |
| `attune-core/src/routing/rules.rs` | Capability matrix 加载 + 决策树 |
| `attune-core/src/routing/budget.rs` | Cost budget watcher(订阅 usage_events) |
| `attune-core/src/routing/health.rs` | Provider 健康探测(30s 循环) |
| `attune-server/src/routes/routing.rs` | `GET/POST /api/v1/routing/{profile,decide,health}` |
| `docs/HYBRID-TOKEN-STRATEGY.md` | 产品级 SSOT,展示 matrix + profile 选择指南 |
| `ui/src/views/SettingsView/RoutingSection.tsx` | Settings 内 routing 配置面板 |

### 4.2 改造现有

| File | 改动 |
|------|------|
| `attune-core/src/llm.rs` | `LlmClient` 选择走 router 决定的 ProviderEndpoint,**不再** hardcoded provider |
| `attune-core/src/embed.rs` | 同上 |
| `attune-core/src/intent_router.rs` | 与本 spec 的 `CapabilityRouter` 区分:`intent_router` 是 query → capability,本 spec 是 capability → provider,两层 cascade |
| `attune-core/src/agent_runner.rs` | 启动 subprocess 前调 router 决定 provider,通过 env 传给 subprocess |
| `attune-core/src/platform/detector.rs` | 加 `recommend_routing_profile() → RoutingProfile` |
| `attune-server/src/routes/llm.rs` | 接受 `?provider=` query 透传 router 作 override |
| Wizard `ui/src/views/Wizard/Step4Hardware.tsx` | 硬件 detect 后显示推荐 profile,user 可选 |

### 4.3 不动

- `resource_governor` — 任务级 CPU/RAM/LLM-rate limit 与 routing 互补;先 router 决定路径,再 governor 决定能不能跑
- `cost.rs` pricing table — 复用,routing decision 中的 `est_cost_usd` 调它
- `intent_router.rs` — query → capability 意图分类,与本 spec 的 capability → provider 是两层

---

## 5. API 契约

### 5.1 Rust types

```rust
/// attune-core::routing

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoutingProfile {
    EdgeFirst,
    Balanced,
    CloudFirst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Capability {
    Chat,
    ChatLong,        // > 32K context
    Embed,
    Rerank,
    Classify,
    ExtractAgent,    // LLM-driven extractor
    Vision,
    Ocr,
    Asr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderEndpoint {
    pub provider: String,        // ollama / cloud_gateway / openai / k3_local / ppocr / whisper_cpp
    pub model: String,
    pub base_url: String,
    pub cost_tier: CostTier,     // Free / LocalCompute / Paid
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CostTier { Free, LocalCompute, Paid }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Intent {
    pub capability: Capability,
    pub prompt_tokens_est: u32,
    pub latency_slo_ms: Option<u32>,
    pub user_tier: UserTier,
    pub override_provider: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UserTier { Free, Pro, Enterprise }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingDecision {
    pub primary: ProviderEndpoint,
    pub fallback_chain: Vec<ProviderEndpoint>,
    pub reason: String,                    // 人类可读决策解释
    pub est_cost_usd: Option<f64>,
    pub est_latency_ms: u32,
    pub degraded: bool,                    // true = 因 offline/quota 等降级
}

#[async_trait::async_trait]
pub trait CapabilityRouter: Send + Sync {
    async fn decide(&self, intent: Intent) -> RoutingDecision;
    async fn report_outcome(&self, endpoint: &ProviderEndpoint, ok: bool);
    async fn current_profile(&self) -> RoutingProfile;
    async fn set_profile(&self, profile: RoutingProfile);
}
```

### 5.2 REST endpoints

```http
GET /api/v1/routing/profile
→ 200 OK { "profile": "balanced", "recommended": "edge_first", "reason": "K3 hardware detected" }

POST /api/v1/routing/profile
{ "profile": "edge_first" }
→ 200 OK { "applied": true }

POST /api/v1/routing/decide
{
  "capability": "Chat",
  "prompt_tokens_est": 1200,
  "latency_slo_ms": 5000,
  "user_tier": "Pro"
}
→ 200 OK
{
  "primary": {
    "provider": "cloud_gateway",
    "model": "gemini-1.5-flash",
    "base_url": "https://gateway.engi-stack.com/v1",
    "cost_tier": "Paid"
  },
  "fallback_chain": [
    { "provider": "ollama", "model": "qwen2.5:3b", ... },
    { "provider": "cloud_gateway", "model": "deepseek-chat", ... }
  ],
  "reason": "balanced profile + Pro user + latency budget allows cloud",
  "est_cost_usd": 0.0008,
  "est_latency_ms": 1800,
  "degraded": false
}

GET /api/v1/routing/health
→ 200 OK
{
  "providers": [
    { "provider": "cloud_gateway", "alive": true, "p50_latency_ms": 320, "fail_rate_24h": 0.01 },
    { "provider": "ollama", "alive": true, "p50_latency_ms": 80, "fail_rate_24h": 0.00 },
    { "provider": "k3_local", "alive": false, "reason": "tcp connect refused 192.168.x.x:8080" }
  ]
}

GET /api/v1/routing/matrix
→ 200 OK
{
  "capabilities": [
    { "capability": "Chat", "edge_first": "ollama:qwen2.5:3b", "balanced": "cloud:gemini-1.5-flash", "cloud_first": "cloud:gemini-1.5-pro" },
    ...
  ]
}
```

### 5.3 Provider 决策树(rules.rs 伪代码)

```rust
fn decide_for_chat(profile: RoutingProfile, intent: Intent, ctx: RoutingContext) -> Decision {
    // 1. 强制规则:用户 override 最高优先
    if let Some(p) = intent.override_provider {
        return Decision::from_override(p);
    }

    // 2. Offline:若 cloud 不通,自动降级 edge
    if !ctx.health.cloud_alive() && !matches!(profile, RoutingProfile::EdgeFirst) {
        return Decision::degraded_to_local(/* reason: cloud offline */);
    }

    // 3. Budget guard:Pro 用户接近 quota 80%,降级 edge
    if intent.user_tier == UserTier::Pro && ctx.usage.pro_quota_pct() > 0.80 {
        return Decision::degraded_to_local(/* reason: quota near limit */);
    }

    // 4. Long context:> 32K → 强制走 cloud(本地小模型窗口不够)
    if intent.prompt_tokens_est > 32_000 {
        return Decision::cloud_long_context();
    }

    // 5. Latency SLO:< 500ms 不容许 cloud round-trip,强制 local
    if intent.latency_slo_ms.map_or(false, |s| s < 500) {
        return Decision::local_for_latency();
    }

    // 6. 按 profile 默认
    match profile {
        EdgeFirst => Decision::edge_with_cloud_fb(),
        Balanced => Decision::cloud_with_edge_fb(),
        CloudFirst => Decision::cloud_only_with_byok_fb(),
    }
}
```

### 5.4 CLI commands

```sh
attune routing show                          # 当前 profile + recommend
attune routing set --profile edge_first
attune routing decide --capability chat --tokens 1200   # dry-run 看决策
attune routing health                        # provider 状态表
attune routing matrix                        # 全 capability x profile 表(也可看 docs/HYBRID-TOKEN-STRATEGY.md)
```

---

## 6. 扩展点 / 插件接口

### 6.1 加新 provider

新 provider impl `LlmClient` / `EmbeddingProvider` trait 后,在 `routing::rules` 注册到对应 capability:

```rust
// 在 rules.rs 加表条目
register_provider(Capability::Chat, ProviderEndpoint {
    provider: "anthropic_direct".into(),
    model: "claude-3-5-sonnet".into(),
    base_url: "https://api.anthropic.com/v1".into(),
    cost_tier: CostTier::Paid,
});
```

Router 自动把新 provider 加入 fallback 候选,优先级由 `routing.priority_overrides` 配置。

### 6.2 加新 routing profile

`RoutingProfile` 是 enum 但用户可在 `settings.routing.custom_profiles` 配 YAML 自定义:

```yaml
routing:
  custom_profiles:
    privacy_paranoid:                       # 用户自定义
      chat: { primary: ollama, fallback: [] }    # 不许 cloud fallback
      embed: { primary: ollama, fallback: [] }
      vision: disabled                          # 直接禁用
```

加载走 `routing::profile::parse_custom_profile`,Router 把它当 RoutingProfile::Custom 处理。

### 6.3 加新 routing 策略(v1.3+)

新策略 impl `RoutingStrategy` trait 替换默认决策树:

```rust
pub trait RoutingStrategy: Send + Sync {
    fn decide(&self, intent: Intent, ctx: &RoutingContext) -> Decision;
}
```

例如 `AdaptiveRoutingStrategy`(v1.2)基于 last-N usage events 学习哪个 provider 当前最快/最便宜,Router 实例化时注入。

### 6.4 第三方插件 routing 钩子

Plugin pack 可在 `plugin.yaml` 声明:

```yaml
routing_overrides:
  - capability: ExtractAgent
    when: agent_id == "defamation_extractor"
    prefer: { provider: cloud_gateway, model: deepseek-v3 }
```

Router 在决策时优先检查 plugin overrides。

---

## 7. 错误处理 + 边界 case

### 7.1 错误码 kebab

| HTTP | code | 场景 |
|------|------|------|
| 400 | `routing-profile-invalid` | POST /routing/profile 不在 enum |
| 400 | `routing-capability-unknown` | decide 时 capability 名错 |
| 503 | `routing-all-providers-down` | 主路径 + 全 fallback 都不可达 |
| 503 | `routing-quota-exhausted` | Pro 用户配额 100% + edge 也禁用 |
| 500 | `routing-config-corrupt` | YAML 解析失败 |

### 7.2 边界 case

| Case | 行为 |
|------|------|
| 用户配 edge_first 但本地无 Ollama 也无 K3 | wizard 阻断 + 提示「edge_first 需要本地推理后端」 |
| Cloud gateway 间歇性 5xx(20% 失败率) | health watcher 标 demoted,3 次成功后恢复 |
| User override 指向 unhealthy provider | Router 警告 log,仍按 user 意愿尝试,fail 后自动 fallback |
| 多 capability 并发请求同一 provider 5xx | rate limiter 5s window 内不重试同 provider,直接走 fallback |
| Profile 切换瞬间 chat 进行中 | 已发起的 request 保持原 routing,新 request 用新 profile |
| K3 :8080 偶发延迟 > 5s | latency exponential moving avg,p99 > SLO 时降权(per AdaptiveStrategy) |
| Offline → 半在线(部分 provider 通) | 局部健康表标记,decide 按可达 subset 决策 |
| Settings 删除当前 active profile | 自动回退 balanced(默认) |
| BYOK 用户 API key 失效 | 第一次 401 → fallback chain,标 key invalid + UI 红点提示 |

### 7.3 Graceful degradation

```
Cloud gateway down + Ollama down + K3 down(全死)→
  UI 显示:「LLM 服务不可用,以下功能暂停:Chat / Extract / Classify。
            搜索 / OCR / 文档管理仍可用(纯本地)」
  Routing.decide() 返回 Err(routing-all-providers-down)
  Chat 路由收到此错 → 返回 503 + 友好 message + 跳 Settings 提示
```

---

## 8. 成本契约

### 8.1 三层成本 matrix(强制 docs SSOT)

下表是 `docs/HYBRID-TOKEN-STRATEGY.md` 的核心内容,产品 wiki + 官网 + README 都引用本表:

| Capability | Edge(零成本/本地算力) | Hybrid(成本可选) | Token Only(必须远端) |
|------------|---------------------|-------------------|----------------------|
| **文件 parse / FTS query** | ✅ 总是本地 CPU | — | — |
| **Embedding** | ✅ bge-m3 / Ollama | bge-small ORT(低配机) | cloud embed(可选,极少) |
| **Rerank** | ✅ tantivy BM25 + RRF | — | cloud rerank(v1.2 可选) |
| **OCR** | ✅ PP-OCRv5 mobile | — | — |
| **ASR** | ✅ whisper.cpp(small/medium/large) | — | — |
| **Classify(简单)** | ✅ Ollama qwen3b | cloud cheap(deepseek-chat) | — |
| **Chat(短 query <32K)** | ⚡ Ollama qwen3b(慢但 free) | ★ cloud_gateway(默认) | BYOK direct |
| **Chat(长 query >32K)** | — | — | ✅ cloud gemini-1.5-pro 1M |
| **Extract(agent LLM judgement)** | ⚡ Ollama(F1 ~0.6) | ★ cloud(F1 ~0.85) | — |
| **Vision(VLM)** | — | — | ✅ cloud_gateway VLM provider(v1.1+) |
| **Skill evolution(后台)** | ⚡ Ollama 静默 | — | — |
| **Memory consolidation(后台)** | ⚡ Ollama 静默 | — | — |
| **Annotation suggestion** | — | ★ cloud(显式触发) | — |
| **Deep summarize(用户触发)** | ⚡ Ollama(可选) | ★ cloud(默认) | — |

**红线**:
- 任何 ⚡ 本地算力任务**永不**默认升级到远端;
- 任何 💰 远端任务**必须**用户显式触发或在 settings 显式 opt-in;
- 后台任务(skill evolution / memory consolidation)**绝不**走远端 token

### 8.2 UI 显示位置

| 显示 | 位置 |
|------|------|
| 每次操作的层级图标 | 按钮 / chip 上 🆓 / ⚡ / 💰 三色标记 |
| 实时成本预估 | ChatSendBar `~1.2K tok · $0.0004` |
| 累计 cost | 顶栏「今日 $0.84」+ Settings Usage tab |
| 当前 profile | Settings 顶部 + Vault 顶栏小字「Profile: balanced」 |
| 路由决策解释 | 调试模式 Chat 消息底部「Routed to cloud_gateway:gemini-1.5-flash because: balanced profile」 |
| Provider health | Settings → Routing → Health 表 + 接近 quota 时顶栏红点 |
| Offline 通知 | Toast「Cloud unavailable, switched to local」 |

### 8.3 用户显式触发原则

- Profile 切换 — 用户在 Settings 显式选择
- 任何「升级到远端」的操作 — 按钮显式标 💰 + 预估成本
- 自动降级(quota / offline)— Toast 通知,不静默切

---

## 9. 测试矩阵

### 9.1 6 类下限

| 类型 | 下限 | 路径 |
|------|------|------|
| Golden | ≥10 fixture | `attune-core/src/routing/tests/golden.rs` — 10 个 (profile × capability × hardware) 组合期望决策 |
| 属性测试 | ≥3 | `proptest!`:任意 intent → decide 返回 primary + non-empty chain;cost_usd ≥ 0;degraded=true 时 reason 非空 |
| 边界 | ≥5 | 全 provider down / quota 100% / offline / override invalid / profile 切换中 |
| 错误 | ≥3 | YAML config corrupt / unknown capability / unhealthy override |
| 集成 E2E | ≥1 | `attune-server/tests/routing_endtoend.rs` — 启动 mock cloud + mock ollama,跑完整 chat → 看 routing decision + UsageEvent 落表 |
| 回归 | per bug | 每修一个 routing bug 加 fixture |

### 9.2 关键场景

| 场景 | 期望 |
|------|------|
| K3 镜像启动 + wizard 选硬件 | 自动推荐 `edge_first` |
| Pro 用户笔电 + balanced + 跑 chat | primary=cloud_gateway,fallback=[ollama, byok] |
| Cloud 5xx 三次 | demoted 标志生效,下次同 capability primary 跳过 cloud |
| 配额 80% | UI 红点 + 下次自动 fallback edge |
| 用户开飞行模式 | health 全 dead,routing 返回 degraded,Chat 走 ollama |
| Long doc(50K tokens)summarize | primary=cloud_gateway:gemini-1.5-pro(1M 窗口) |
| User override `?provider=ollama` | 即使 balanced 也走 ollama |

### 9.3 性能 baseline

| 指标 | 目标 |
|------|------|
| `router.decide()` p99 | < 5ms(纯内存表查 + cache 健康状态) |
| Health probe 单 provider | < 200ms(超时 1s) |
| Profile 切换生效 | < 50ms(原子替换 + 通知 caller) |
| Fallback 触发到下一 provider | < 100ms(主 fail → fallback 决策 + dispatch) |

### 9.4 黑盒视角验证

- **用户故事 A**:用户买了 Pro,在咖啡店 WiFi 不稳,跑 chat
  期望:第一次 cloud 成功;第二次 cloud 5xx 自动 ollama 完成;Toast「Switched to local」;Usage tab 体现混合 events
- **用户故事 B**:用户 K3 一体机,完全离线
  期望:wizard 推荐 edge_first;所有 chat / extract 走 K3 :8080 / Ollama;Vision / VLM 显示「需要在线」灰显
- **用户故事 C**:Pro 用户当月 token 跑爆
  期望:跑到 80% 顶栏红点;90% 弹 modal「即将达到配额,建议切 edge_first 或购买 Add-on」;100% 全部 edge

---

## 10. 向后兼容

### 10.1 Schema versioning

- `settings.routing.*` 是新字段,旧 vault 升级时默认 `profile = balanced`
- Routing 决策 log 不落 vault DB(只走 UsageEvent),无 migration

### 10.2 Migration path

| 老资产 | 处理 |
|--------|------|
| `settings.llm.provider` 单字段 hardcoded | 升级时迁移为 `settings.routing.chat.primary` + profile=cloud_first |
| `settings.llm.model` 单 model | 升级时拆为 `routing.chat.model`、`routing.classify.model` 等 |
| 老 chat / agent 代码直接调 `LlmClient::new(...)` | 改造期保留 `LlmClient::new_legacy` 兼容 1 release,然后强制走 router |

### 10.3 老客户端行为

- Web UI 不更新 — 看不到新 RoutingSection 设置,routing 走默认 `balanced`,不破坏
- Chrome 扩展 — 不感知 routing,只调 API,所有路径透明
- attune-cli 老版本 — `routing` 子命令未知,旧用户不受影响

### 10.4 文档迁移

- `docs/HYBRID-TOKEN-STRATEGY.md` 是新 SSOT,以下旧文档需 cross-link:
  - README「成本感知」节 → 链到新 doc
  - DEVELOP「LLM 集成」节 → 链到新 doc
  - 官网 wiki「Pricing / Privacy」页 → 嵌入 matrix 表
- 不再维护「LLM 提供商策略」分散在 CLAUDE.md 多处的描述,统一指向新 doc

---

## 11. 风险登记

### 风险 1 ⚠️ HIGH:Routing 决策错误导致用户超额扣费

**描述**:Pro 用户配 `balanced`,但某次 long context query 触发 cloud_gateway gemini-1.5-pro,
单次 ~$0.05。如果 router bug 导致频繁误判(本应走 edge 却走了 cloud),用户月底账单超预算来索赔。

**缓解**:
1. 决策树每条规则附 reason 字符串,审计可追溯
2. UI Settings Routing 提供 `recent_decisions` 表(最近 50 次)+ Cost breakdown
3. 单次 cost > $0.10 时强制弹 modal 二次确认
4. 灰度策略:v1.1.0 默认 `cloud_first` profile **降级**为 `balanced`,5% 用户 opt-in `cloud_first`,验证 1 周后扩大
5. Pro 配额 watcher 接近 80% 主动降级,**绝不**让用户跑到 100%

### 风险 2 ⚠️ HIGH:Fallback 风暴导致 thundering herd

**描述**:cloud_gateway 全局 5xx 时,所有 attune 实例同时切 fallback,可能打挂本地 Ollama
或 K3:8080(共享后端的家庭多设备场景)。

**缓解**:
1. Fallback 加 jitter(0-500ms 随机延迟)
2. health watcher 标 demoted 后,5s window 内同 provider 不再尝试
3. 同 capability 并发 > 5 时,后到 request 直接排队,不并发打 fallback
4. K3 / Ollama 接收方加 rate limit,attune 客户端遇 429 立刻退避

### 风险 3 ⚠️ HIGH:用户期望与实际路由不符的「信任问题」

**描述**:用户配了 `edge_first` 期望"绝不出本地",但某次 fallback 触发了 cloud_gateway(因为
本地 Ollama 崩了)— 用户事后看 UsageEvent 发现 query 出去了 → 信任崩塌。

**缓解**:
1. `edge_first` profile 默认 fallback_chain 为空 — 本地挂就直接 fail,**不**自动 cloud
2. 配置项 `routing.allow_cloud_fallback = false`(edge_first 默认)/true(balanced 默认)
3. UI Settings Routing 大字提示「edge_first 模式下,本地 LLM 不可用时 Chat 将失败,绝不出本地」
4. Privacy paranoid 自定义 profile 模板供用户复制

### 风险 4 ⚠️ MED:Capability matrix 更新滞后于 provider 能力变化

**描述**:Cloud gateway 上线新模型(如 gemini-2.5-flash 上下文 2M),需更新 matrix。
但 attune 客户端发版周期慢,用户的 matrix 表过时,无法用上新模型。

**缓解**:
1. `routing.matrix_source` 配置:`builtin`(默认)/ `remote`(cloud gateway 提供 latest matrix)
2. Remote matrix 走 attune-pro `/v1/routing/matrix` endpoint,客户端启动时拉一次,本地缓存 24h
3. Builtin matrix 内置最低保证 — 即使 remote 拉不到也能跑
4. Matrix update 进 RELEASE.md 单独节,显式说明

### 风险 5 ⚠️ MED:Wizard 推荐错误 profile

**描述**:硬件检测假阴性(检测不到 NVIDIA GPU 但其实有)→ 推荐 cloud_first 而非 edge_first,
用户花了不必要的 token。

**缓解**:
1. Wizard 后允许用户在 Settings 一键切换 profile
2. 推荐附 reason 字符串:「因为未检测到 GPU,推荐 cloud_first。如有独显请手动切 edge_first」
3. detector 多种探测路径(per `platform::detector` 已有逻辑)
4. v1.2 加 Profile A/B,用户跑几次后系统建议「检测到本地 LLM 充足,建议切 edge_first」

### 风险 6 ⚠️ MED:K3 一体机场景缺测

**描述**:K3 镜像形态是关键差异化卖点,但 CI 不跑 RISC-V,routing K3 路径可能未经实测就发版。

**缓解**:
1. K3 路径有专门 nightly job(per attune CLAUDE.md K3 ready task #72)
2. 用户验收前 K3 一体机 wizard + chat 至少在物理设备跑过 10 case(对应 task #72)
3. K3 路径有 SQLite mock + integration test
4. RELEASE.md 明示「K3 路径在 desktop tag 时未完成 CI 自动化测试」

### 风险 7 ⚠️ LOW:Offline detection 误判

**描述**:用户企业网络 cloud_gateway 域名被 DNS 污染,health probe 失败 → router 切 edge,
但其实 cloud 可达只是 DNS 问题。

**缓解**:
1. Health probe 多路径:DNS + TCP + HTTP 三层检测,任一通即视为 alive
2. 失败 reason 明示给 UI(network / dns / 5xx / timeout 区分)
3. Settings 加「强制重试 cloud」按钮,bypass health cache

### 风险 8 ⚠️ LOW:Routing decision 暴露 sensitive metadata

**描述**:`POST /api/v1/routing/decide` 返回 `reason` 字符串可能含 user_tier / quota 状态等,
若 attune-server 同时暴露给第三方 plugin,可能泄露。

**缓解**:
1. `reason` 字段仅在 user 本地可见,不跨进程
2. Plugin RPC 接口删 `reason`,只返回 endpoint
3. UsageEvent 中的 routing reason 进加密 vault 表

---

## Appendix A:Spec 评审 checklist

- [ ] 11 节全部有实质内容,无 stub
- [ ] Capability matrix 表覆盖 attune 当前全部 9 capability
- [ ] 三种 profile 的具体规则齐(edge_first / balanced / cloud_first)
- [ ] Provider endpoint schema 与 LlmClient / EmbeddingProvider trait 兼容
- [ ] 风险登记 ≥ 3 个(本 spec 列 8 个)+ 缓解
- [ ] 与 `2026-05-28-cache-context-token-standard-api.md` 集成点明确(UsageEvent feed back router)
- [ ] docs/HYBRID-TOKEN-STRATEGY.md 内容草稿已经在本 spec §3+§8,可直接抽到该文件

## Appendix B:实施时机

- **v1.0.x 不实施** — 已排满
- **v1.1.0**(8/15 后)— Capability matrix + 三 profile + offline detection + 基础 fallback
- **v1.2.0** — Adaptive routing(usage-driven)+ semantic routing(query 长度)
- **依赖**:必须先实施 `2026-05-28-cache-context-token-standard-api.md`(UsageEvent 是 routing 输入数据源)
- **预估**:中-大型 feature,5-7 工作日 implementation + 2-3 天测试

## Appendix C:docs/HYBRID-TOKEN-STRATEGY.md 抽取规则

本 spec §3 架构图 + §5.3 决策树 + §8.1 三层成本 matrix 三段是产品 SSOT,
v1.1 实施时**先**抽出独立 `docs/HYBRID-TOKEN-STRATEGY.md`,
其余实施细节(模块边界 / 错误码 / 测试 / 风险)留在本 spec 仅供开发者。

抽出文件位置:`/data/company/project/attune/docs/HYBRID-TOKEN-STRATEGY.md`
README / DEVELOP / 官网 wiki 添加链接引用。

---

**Draft 完成。等待用户评审 → invoke `superpowers:writing-plans` 出 implementation plan。**

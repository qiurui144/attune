# Spec: 零成本主动建议引擎 + 第三方账号统一管理 (DRAFT)

> Status: **DRAFT — 待评审，先规划不实现**
> Date: 2026-06-17
> Author: spec-analyst (AI 起草)
> 关联：
> - [[2026-06-17-multilayer-memory-architecture]] — 5+ 层记忆架构（信号源 / 画像层）
> - [[2026-06-15-memory-continuity-and-portability]] — 维度迁移 + 导出/导入（账号凭据是可迁移资产之一）
> - [[2026-05-28-privacy-logic-strategy]] — OutboundGate 6-kind 出网受控（第三方账号复用）
> - CLAUDE.md §成本感知与触发契约（本 spec 的最高约束来源）

本 spec 含两个**互相独立、同一会话规划**的能力：

- **能力 A：零成本主动建议引擎** — 基于确定性本地信号生成被动可见的建议卡；用户**显式点击**才升级到 LLM。绝不后台偷跑 LLM。
- **能力 B：第三方账号统一管理** — 把散落的外部账号凭据（LLM BYOK / WebDAV / Git / 云盘 / Email …）收编进一个加密存储 + 统一列表/增删/测试连接的管理面。

两者共用本 spec 但**可独立切片实现**（A 不 blocks B）。

---

## 0. 现状核实（§6.3，已有 vs 增量）

> 所有"已有"项均引代码路径，未臆造。

### A. 建议引擎相关现状

| 机制 | 路径 | 状态 |
|------|------|------|
| 确定性信号事件表 `skill_signals` + `record_signal_event(kind, ref_id, query)` | `attune-core/src/store/signals.rs:55` | **已有** |
| 已知 kind 白名单（typo guard） | `signals.rs:15` — `search_miss / doc_create / doc_update / doc_delete / citation_hit / annotation_marker / click_through / dwell` | **已有** |
| 按 kind 计未处理信号 `count_unprocessed_signals_by_kind` | `signals.rs:77` | **已有** |
| `citation_hit` 聚合 `count_citation_hits_for_refs` | `signals.rs:152` | **已有** |
| 信号 purge（90 天）`purge_processed_signals_older_than_days` | `signals.rs:183` | **已有** |
| SkillClaw 风格后台技能进化（search_miss → LLM 扩展词，**静默生效**） | `attune-core/src/skill_evolution/` | **已有** — 注意：这是**唯一**已存在的"信号→LLM"路径，但它消费的是 search_miss 且产出仅是检索扩展词，不向用户暴露问题/不偷跑生成式对话 |
| Project 推荐（确定性，纯关键词/实体重叠，**不调 LLM**）`recommend_for_file` / `recommend_for_chat` | `attune-core/src/project_recommender.rs:55,103` | **已有** — 推荐结果不持久化，WebSocket 推前端 |
| 文件夹整理→案卷聚类引擎 `organizer::analyze_items` | `attune-core/src/organizer/mod.rs:28` | **已有** |
| 行为画像（browse_signals） | `attune-core/src/store/browse_signals.rs` | **已有** |

**结论（A）**：信号采集底座 + 一条确定性推荐管道 + 聚类引擎**全部已有**。缺的是 **(1) 一个统一的"确定性规则 → 建议卡"引擎**把现有多类信号汇成用户可见卡片；**(2) 建议卡数据模型 + 生命周期（忽略/关闭/去重）**；**(3) 卡 → 用户点击 → 升级 LLM 的显式触发契约**。**增量 ≈ 规则层 + 卡片层 + UI，零新增 LLM 自动调用。**

### B. 第三方账号相关现状

| 凭据类型 | 存储方式 | 加密 | 路径 |
|---------|---------|------|------|
| WebDAV remote（url/user/password） | `webdav_remotes` 表，`password_enc` BLOB | ✅ **DEK AES-256-GCM**（per-row） | `store/webdav_remotes.rs:43` |
| Email IMAP（host/port/user/password） | `email_accounts` 表，`password_enc` BLOB | ✅ **DEK AES-256-GCM** | `store/email_accounts.rs` |
| LLM BYOK / 会员 gateway token | `app_settings.llm.api_key` —— **vault meta JSON 明文字段** | ❌ **未字段级加密**（依赖整个 vault meta 加密兜底，但与 WebDAV/Email 模式不一致，无独立 DEK 字段加密） | `llm_settings.rs:51` `merge_gateway_into_settings` |
| Git 私有仓 token | **仅进程内存**，不落盘 | n/a（不持久） | `ingest/git.rs:42` — 每次 ingest 需重传 token |
| 会员账号（account_id/license_id/quota） | `MemberState` 客户端态 | （会话态，非凭据存储） | `member_session.rs` |
| 出网受控 | `OutboundGate::enforce`（6 kind：Llm/CloudSaas/Webdav/WebSearch/Telemetry/Embedding） | n/a | `outbound_gate.rs:36` |
| 字段级加解密原语 `crypto::encrypt/decrypt(Key32)` + DEK 包裹 | — | ✅ | `crypto.rs:72,106` |

**结论（B）**：字段级 DEK 加密原语 + 两个已加密的凭据表（WebDAV/Email）**已有**，是参照模板。缺的是 **(1) 统一 credentials 抽象**（一张表 / 一套 API 管所有 provider 类型，而非每类一张表 + Git 不落盘 + LLM 明文三套并存）；**(2) LLM BYOK key 迁到字段级加密**（修补现状不一致）；**(3) Git token 可选持久化**（用户希望免每次重输）；**(4) 统一 UI：列表/增删改/测试连接/状态**。**增量 ≈ 统一 model + LLM/Git 收编 + 测试连接 + UI。WebDAV/Email 已加密，迁移=适配到统一抽象（保留旧表或 view）。**

---

## 1. 目标定位

**用户痛点 A**：attune 是"主动进化 + 混合智能"知识伙伴，但当前所有 LLM 价值都要用户**先想到**才会触发——系统从不"提醒"用户"这堆文件可以整理成案卷""这个查询反复落空，要不要补资料"。痛点是**发现成本高**：好功能藏在用户不知道的地方。

**对齐 positioning**：主动进化 = 系统基于本地信号**被动呈现**机会，而**不是**违反成本契约去后台偷跑 LLM 猜需求。建议引擎让"主动"落在零成本确定性层，把"花钱/花时间"的决定权 100% 交还用户。

**用户痛点 B**：用户的外部账号（OpenAI/Claude key、坚果云 WebDAV、GitHub PAT、邮箱、未来云盘）目前**散落在不同设置角落、加密程度不一**（WebDAV 加密、LLM 明文、Git 根本不存要每次重输）。痛点是**无统一视图 + 安全不一致 + 无连接体检**。

**对齐 positioning**：1Password 式"私密" + 数据安全是产品并列最高原则。统一账号管理把"我连了哪些外部服务、各自健康吗、凭据安全吗"变成一屏可见、一致加密、可一键测试。

---

## 2. 范围边界

### 能力 A — 做

- 确定性**规则引擎**：消费已有信号（search_miss / doc_create / citation_hit / annotation_marker / browse 画像）+ organizer 聚类结果 + project_recommender 输出 → 产出 4 类建议卡：**整理类 / 补充类 / 检索优化类 / 画像类**。
- 建议卡**数据模型 + 生命周期**：生成、去重、用户忽略（dismiss）、永久关闭某类（mute kind）、过期清理。
- 卡 → **用户显式点击** → 才执行对应动作（其中"升级 LLM"的动作走现有显式触发路径，UI 标本地/云端 + 预估成本，per 成本契约 §UI 显示成本）。

### 能力 A — 不做（写死，禁止 scope creep）

- ❌ **任何后台自动 LLM 调用来生成建议内容**（违反成本契约第 2 条"分析阶段永远等用户开口"）。规则层纯确定性。
- ❌ "AI 主动建议下一个问题 / 猜你需要什么"（CLAUDE.md 明列的禁止产品行为）——建议卡只指向**已存在的确定性机会**（这些文件实体重叠高、这个查询反复 miss），不生成开放式问题。
- ❌ 推送通知 / 打断式弹窗 / 红点催促（建议卡是**被动可见**，放在固定区域，可忽略可关）。
- ❌ 建议卡持久化跨设备同步（v.next 才考虑；本版本本地态）。

### 能力 B — 做

- 统一 `credentials` 抽象：provider_type + 加密凭据 + 元数据 + 状态。
- 收编 **LLM BYOK key**（迁字段级加密）、**Git token**（可选持久化）进统一存储；**WebDAV/Email 适配**到统一抽象（保留物理表，加统一读写 facade 或 view）。
- 统一 API：列表 / 新增 / 编辑 / 删除 / **测试连接** / 状态查询。
- 复用 `OutboundGate`（所有测试连接 + 实际使用走 gate，sealed/locked 时拒绝）。
- 锁屏（vault locked）时凭据**不可读**（DEK 不可用）。

### 能力 B — 不做（写死）

- ❌ 凭据**云端托管 / 跨设备自动同步**（凭据永远本地，per §数据隔离；跨设备走 §2 portability 的用户主动 export/import）。
- ❌ OAuth 授权流自动化（v.next；本版本只管"用户已有的 key/密码/token"录入与管理）。
- ❌ 改写 OutboundGate 的 6-kind 模型 / 隐私策略（直接复用）。
- ❌ 会员 gateway token 的下发逻辑（`merge_gateway_into_settings` 不动；它是 cloud 下发路径，本能力管的是**用户自配 BYOK**，会员锁定态由 `MemberState` 决定 UI 灰显）。

---

## 3. 架构数据流

### 能力 A 数据流（全程零 LLM 直到用户点击）

```
[已有信号源 — 确定性]                  [规则引擎 — 确定性 纯 CPU 毫秒级]        [建议卡层]            [用户]
 skill_signals (search_miss …) ──┐
 organizer::analyze_items 聚类 ──┤      SuggestionRule::evaluate()
 project_recommender 实体重叠  ──┼───►  - 阈值判断 (no LLM)          ───►  SuggestionCard ───► 固定区域被动显示
 browse_signals 画像           ──┤      - 去重 (signature hash)            (kind/title/                │
 doc lifecycle (doc_create …)  ──┘      - mute/dismiss 过滤                 action/cost_hint)          │ 点击 (显式)
                                                                                                       ▼
                                                                              ┌──────────────────────────────┐
                                                                              │ action 执行:                  │
                                                                              │  • 整理类 → 打开 OrganizeWizard│ (本地聚类已算, 仅在
                                                                              │  • 检索优化 → 触发已有技能进化 │  用户确认归类时才可能
                                                                              │  • 补充类 → 打开搜索/web 入口  │  调 LLM, 走现有显式路径)
                                                                              │  • 画像类 → 打开画像视图       │
                                                                              │  ⚡ 升级 LLM 仅当 action 本身  │
                                                                              │     就是已有的显式 LLM 操作    │
                                                                              └──────────────────────────────┘
```

**关键不变量**：规则引擎 `evaluate()` 的输入输出**不含任何 LLM provider 句柄**——编译期就无法偷跑 LLM。LLM 只可能在用户点击后、由 action 路由到**已存在的显式触发 handler**（OrganizeWizard 确认 / Chat 发送 / web search）里发生，且那些 handler 已带成本 UI。

**建议卡不持久化生成内容**：卡本身存轻量元数据（kind + 引用 ref_ids + signature + dismissed 标志），可落一张 `suggestion_cards` 表用于去重/dismiss 记忆；**不**存 LLM 产出。

### 能力 B 数据流

```
[UI: 账号管理 tab]          [统一 credentials API]                [加密存储 — DEK]
  列表/增删改/测试  ──►  POST/GET/DELETE /api/v1/credentials  ──►  credentials 表
                          PUT  …/credentials/{id}                  (provider_type, label,
                          POST …/credentials/{id}/test            secret_enc BLOB[DEK],
                              │                                     meta_json, status, …)
                              ▼
                       OutboundGate::enforce(kind)  ──► 测试连接发真请求 (vault unlocked + enabled + 非 sealed)
                              │
                       provider adapter (per type):
                        • llm_byok   → GET {endpoint}/v1/models
                        • webdav     → PROPFIND depth:0  (复用 sync/webdav)
                        • git        → ls-remote (复用 ingest/git auth_url)
                        • email      → IMAP LOGIN + NOOP (复用 ingest/email)
```

**DB tables**：
- 能力 A：`suggestion_cards`（id, kind, signature TEXT UNIQUE, ref_ids JSON, created_at, dismissed INTEGER, action_kind）+ `suggestion_mutes`（kind TEXT PK, muted_at）。
- 能力 B：`credentials`（id TEXT PK, provider_type TEXT, label TEXT, secret_enc BLOB, meta_json TEXT, status TEXT, last_tested_at, created_at, updated_at）。**WebDAV/Email 保留现表**；统一 facade 读时 union，或迁移期内双写——见 §10。

**Cache layers**：无新增持久 cache。建议卡 evaluate 结果可在内存短缓存（debounce，避免每次 WebSocket 推送重算），TTL 级别秒级。

---

## 4. 模块边界

| crate / module | 角色 | 新增/改 |
|----------------|------|---------|
| `attune-core/src/suggestions/`（新模块） | 规则引擎 `evaluate()` + 卡片类型 + 去重 | **新增** |
| `attune-core/src/store/suggestions.rs`（新） | `suggestion_cards` / `suggestion_mutes` CRUD | **新增** |
| `attune-core/src/store/credentials.rs`（新） | 统一 `credentials` CRUD（DEK 加密，仿 webdav_remotes.rs） | **新增** |
| `attune-core/src/credentials/`（新） | provider adapter（测试连接 trait + per-type impl，复用现有 webdav/git/email/llm 客户端） | **新增** |
| `attune-core/src/store/webdav_remotes.rs` / `email_accounts.rs` | 适配统一 facade（保留物理表） | **改（薄）** |
| `attune-core/src/llm_settings.rs` | BYOK key 读取改走 credentials（gateway 下发逻辑不动） | **改** |
| `attune-core/src/ingest/git.rs` | token 可从 credentials 读（仍支持 in-memory 传入） | **改（薄）** |
| `attune-core/src/outbound_gate.rs` | **不改**，直接复用 | — |
| `attune-server/src/routes/suggestions.rs`（新） | GET 建议卡 / dismiss / mute；WS 推送 | **新增** |
| `attune-server/src/routes/credentials.rs`（新） | credentials REST + test | **新增** |
| `attune-server/ui/src/views/` | 建议卡区域（嵌 Workbench/首页）+ Settings 新 `accounts` tab | **新增/改** |

跨仓边界：无。attune-pro vertical plugin 可后续向规则引擎注册自定义建议规则（扩展点 §6），但本 spec 只定 OSS 通用规则。

---

## 5. API 契约

### 能力 A

```
GET    /api/v1/suggestions                 → { cards: [SuggestionCard] }   # 当前活跃、未 dismiss、未 mute
POST   /api/v1/suggestions/{id}/dismiss    → 204                            # 忽略单卡
POST   /api/v1/suggestions/mute            { kind }  → 204                  # 永久关闭某类
DELETE /api/v1/suggestions/mute            { kind }  → 204                  # 恢复某类
WS     suggestion_card_new                  (server→client 推新卡, 已有 WS 通道)
```

`SuggestionCard`（typed）：
```jsonc
{
  "id": "uuid",
  "kind": "organize | enrich | retrieval | profile",
  "title": "8 份文件实体高度重叠，可整理成一个 Project",
  "detail": "涉及实体：合同甲方A / 项目X …",
  "ref_ids": ["item_id1", "..."],
  "action_kind": "open_organize_wizard | open_search | run_skill_evolution | open_profile",
  "cost_hint": { "tier": "free | local | cloud", "note": "整理预览免费；确认归类时按你的设置可能调用本地/云端模型" },
  "created_at": "rfc3339"
}
```

> `cost_hint.tier` 对建议卡本身永远是 `free`（生成卡零成本）；`note` 描述**点击后**动作的成本层级，让用户知情。

### 能力 B

```
GET    /api/v1/credentials                 → { items: [CredentialView] }   # secret 永不返回明文
POST   /api/v1/credentials                 { provider_type, label, secret, meta } → CredentialView
PUT    /api/v1/credentials/{id}            { label?, secret?, meta? }       → CredentialView
DELETE /api/v1/credentials/{id}            → 204
POST   /api/v1/credentials/{id}/test       → { status: "ok|auth_failed|unreachable|disabled|vault_locked", latency_ms?, detail? }
```

`CredentialView`（**绝不含 secret 明文**）：
```jsonc
{
  "id": "uuid",
  "provider_type": "llm_byok | webdav | git | email | clouddisk",
  "label": "我的 OpenAI key",
  "secret_masked": "sk-…last4",          // 仅尾 4 位
  "meta": { "endpoint": "https://api.openai.com/v1", "model": "..." },  // 非 secret 字段
  "status": "untested | ok | auth_failed | unreachable | disabled",
  "last_tested_at": "rfc3339|null"
}
```

CLI（可选，与 server 对齐）：`attune credentials list|add|test|rm`。

---

## 6. 扩展点 / 插件接口

- **能力 A 规则注册**：`suggestions::SuggestionRule` trait（`fn evaluate(&self, ctx: &SignalContext) -> Vec<SuggestionCard>`）。OSS 内置 4 规则；attune-pro vertical plugin 后续可注册行业规则（如 law-pro："这批文件像同一案件卷宗"）。本 spec 只定 trait + OSS 规则，**插件注册机制留 v.next**（写死边界）。
- **能力 B provider 类型扩展**：`CredentialProvider` trait（`provider_type()` + `async fn test(&self, secret, meta, gate) -> TestResult`）。新增"云盘"等类型 = 实现 trait + 加 OutboundKind 映射（若需新出网类型则同步扩 OutboundGate，但本 spec 复用现有 6 kind，clouddisk 暂归 Webdav-like）。

---

## 7. 错误 + 边界 case

| 场景 | 行为 | code (kebab) |
|------|------|--------------|
| 建议规则 evaluate 内部失败 | **静默忽略该规则**，不阻塞其它卡（per signals 静默约定） | — |
| 信号表为空 / 未达阈值 | 返回 `cards: []`（不是错误） | — |
| dismiss 不存在的卡 | 幂等 204 | — |
| credentials 测试时 vault locked | `OutboundError::VaultLocked` → `status: vault_locked` | `vault-locked` |
| credentials 测试时该出网 kind 被用户禁用 | `OutboundError::Disabled` → `status: disabled` | `outbound-disabled` |
| 测试时认证失败 (401/403) | `status: auth_failed`（**不**记录 secret 到 log） | `auth-failed` |
| 测试时网络不可达 / 超时 | `status: unreachable` | `unreachable` |
| 读 credential 但 vault locked | 404/401（DEK 不可用，无法解密） | `vault-locked` |
| secret 解密失败（DEK 错/数据损坏） | 返回错误，**不** panic（仿 webdav `VaultError::Crypto`） | `crypto-error` |
| 超长 label / meta | 边界校验拒绝（仿 signals ref_id ≤128 思路） | `invalid-input` |
| SSRF：credential endpoint 指向内网 | 复用 `net::url_guard`（git/webdav 已有） | `url-blocked` |

**Graceful degradation**：建议引擎完全不可用时，产品退化为"无建议卡"（=当前行为），不影响任何核心功能。账号管理不可用时退化为现有分散设置（WebDAV/LLM 设置仍各自工作）。

---

## 8. 成本契约（最高优先，逐条对齐 CLAUDE.md §成本感知）

| 层 | 能力 A | 能力 B |
|----|--------|--------|
| 🆓 零成本（CPU 毫秒） | **建议卡生成全部在此层**：规则引擎纯确定性，读已有信号 + 已算好的聚类/实体重叠。**绝不**为生成建议调 LLM | credentials CRUD / 列表 / 解密读取 |
| ⚡ 本地算力（秒） | 无（建议生成不触发 embedding/classify） | 测试连接发轻量请求（PROPFIND / ls-remote / /v1/models / IMAP NOOP），非 LLM |
| 💰 时间/金钱（LLM） | **仅当用户点击建议卡的 action 且该 action 本身是已有的显式 LLM 操作**（如 OrganizeWizard 确认归类时的标题生成、Chat 发送）。此时走**现有**显式触发 handler，UI 已显示本地/云端 + 预估成本 | 无（账号管理不调 LLM） |

**UI 必须显示成本（硬约束）**：
- 建议卡显示 `cost_hint`：卡本身标"💡 建议（免费）"，action 标"点击后：本地预览免费 / 确认归类时按设置可能调用模型"。
- **绝不**在建议卡出现时静默预跑 LLM 暖 cache。
- 顶栏后台任务队列已有"暂停"开关（per 现状）；建议引擎不进后台 LLM 队列（它根本不入队 LLM 任务）。

**自检红线**：若实现期发现"为了让建议卡更聪明，想后台调一次小 LLM 摘要" → **停，违反成本契约第 2 条**，回头改 spec 评审。

---

## 9. 测试矩阵（§6.1 六类下限）

### 能力 A

| 类型 | 用例（下限） |
|------|------|
| Golden / happy | 给定 N 条 search_miss + organizer 聚类结果 → 期望生成对应 retrieval/organize 卡（≥6 fixture，覆盖 4 kind） |
| 属性测试 (proptest ≥3) | (1) evaluate 对任意信号输入**永不返回含 LLM 调用的副作用**（用 mock 断言 0 次 provider 调用）；(2) 同一信号集多次 evaluate 卡 signature 稳定（去重幂等）；(3) dismiss 后该 signature 不再出现 |
| 边界 (≥5) | 空信号集 / 单条信号未达阈值 / 阈值边界 / 全部 mute / ref_ids 超量 |
| 异常 (≥3) | 单规则 panic 被隔离、其它规则仍出卡；信号表损坏读失败静默；dismiss 不存在 id 幂等 |
| 集成 E2E (≥1) | server 起 → 注入信号 → GET /suggestions 返回卡 → dismiss → 复查消失 |
| 回归 fixture | 每修一个误报/漏报加 1 golden |

**关键反偷跑测试（最高价值）**：`evaluate()` 注入一个会 panic 的 `MockLlmProvider`（或断言计数器），证明**生成建议卡路径 0 次 LLM 调用**。这是成本契约的机器可执行守卫。

### 能力 B

| 类型 | 用例 |
|------|------|
| Golden / happy | add llm_byok/webdav/git/email → list 返回 masked → test 返回 status |
| 属性 (≥3) | (1) secret 入库后 `secret_enc` BLOB **不含明文**（仿 `debug_raw_webdav_password_enc` 断言）；(2) CredentialView 序列化**永不**含明文 secret；(3) round-trip 加解密一致 |
| 边界 (≥5) | 空 secret / 超长 label / 重复 label / 未知 provider_type / meta 非法 JSON |
| 异常 (≥3) | vault locked 时 read/test → vault_locked；禁用 kind → disabled；SSRF endpoint → url-blocked |
| 集成 E2E (≥1) | mock WebDAV/HTTP server → add → test ok → 改 secret → test auth_failed |
| 安全 / adversarial | secret 不入 log（grep 测试输出）；测试响应不回显 secret；锁屏后无法解密 |

---

## 10. 向后兼容

- **能力 A**：纯新增，无 schema 破坏。`suggestion_cards` / `suggestion_mutes` 新建表，老 vault migration = `CREATE TABLE IF NOT EXISTS`。无建议卡 = 当前行为。
- **能力 B 迁移路径**（重点）：
  1. **WebDAV/Email 不破坏**：保留 `webdav_remotes` / `email_accounts` 物理表（周期 worker 直读路径不变）。统一 `credentials` 提供 facade：读时 union 现表 + 新表；或迁移期对 WebDAV/Email 仅做**只读投影**进统一列表（写仍走原 API）。**首选**：新表只收编 llm_byok + git，WebDAV/Email 以 view 形式出现在统一 UI（避免双写一致性风险）。
  2. **LLM BYOK 迁移**：现状 `app_settings.llm.api_key` 明文。新版本写入 `credentials`（字段级加密），读取优先 credentials，**回退**读 `app_settings.llm.api_key`（老 vault 未迁移）。提供一次性 lazy 迁移：首次读到明文 key → 写入加密 credentials → 清空明文字段。**会员 gateway token 路径不动**（`merge_gateway_into_settings` 仍写 settings；它是 cloud 下发非用户 BYOK，且会员态 UI 灰显）。
  3. **Git token**：现状不落盘。新增**可选**持久化（默认仍 in-memory；用户勾选"记住"才入 credentials）。老行为（每次传 token）保留。
  4. schema 版本：沿用现有 vault migration 机制，新表 + lazy 迁移函数，老 client 读新 vault 时忽略未知表（rusqlite 容忍）。

---

## 11. 风险登记

| 风险 | 等级 | 缓解 |
|------|------|------|
| **建议引擎实现期"顺手"加后台 LLM**（违反成本契约——产品最高原则） | 🔴 高 | (1) 架构强制：`evaluate()` 签名不含 provider 句柄；(2) proptest 断言 0 次 LLM 调用作为 CI 硬门；(3) review 红线条目 |
| 建议卡变成打扰（红点/弹窗催促）违背隐私优先 UX | 中 | 写死被动可见 + 可 mute；无推送通知；UI review 走 §2.2 用户视角 |
| LLM BYOK 明文→加密迁移丢 key（用户 chat 突然失效） | 🔴 高 | lazy 迁移先写新再清旧（顺序保证）；迁移失败保留明文回退；迁移加 golden 测试 |
| 统一 facade 与 WebDAV/Email 双写不一致 | 中 | 首选只读投影（不双写）；WebDAV/Email 写仍走原表原 API |
| credential secret 泄漏（log / 测试响应 / 序列化） | 🔴 高 | CredentialView 无明文字段（编译期）；test 不回显；secret 不入 log；adversarial grep 测试；§1.4 |
| 测试连接被当 SSRF 跳板 | 中 | 复用 `net::url_guard`（git/webdav 已有）；走 OutboundGate |
| 锁屏后凭据读取绕过 DEK | 中 | DEK 不可用即无法解密（与 WebDAV/Email 同保证）；test/read 在 locked 时拒绝 |
| 规则误报率高（建议不相关） | 中 | 阈值复用已验证常量（RECOMMEND_THRESHOLD=0.6 等）；回归 fixture 累积；可 mute |

---

## 切片建议（评审后转 plan）

| 切片 | 内容 | 依赖 |
|------|------|------|
| A1 | suggestions 规则引擎 + 卡模型 + store（零 LLM，含反偷跑 proptest） | 无 |
| A2 | suggestions REST + WS + UI 建议卡区域 | A1 |
| B1 | 统一 credentials store（DEK 加密）+ provider adapter trait + test 连接 | 无（与 A 并行） |
| B2 | LLM BYOK lazy 迁移 + Git token 可选持久化 + WebDAV/Email 只读投影 | B1 |
| B3 | credentials REST + Settings `accounts` tab UI | B1 |

A 与 B 互不依赖，可并行 worktree（per §并行开发）。

---

> **评审检查点**：(1) 成本契约是否真零偷跑（§8 + §11 高风险项）；(2) LLM BYOK 迁移是否丢 key 安全；(3) 统一 facade 是否值得 vs 维持分散（§10 迁移复杂度）；(4) 建议卡 UX 是否真"被动不打扰"。

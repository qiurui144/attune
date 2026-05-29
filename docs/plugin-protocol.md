# Attune Plugin Protocol — Skill / Agent / MCP 接入标准

**Status**: Draft (未定版, 实装中迭代)
**Stakeholders**: attune (OSS) + attune-pro + 第三方插件作者

> 本 spec 是产品定义阶段的**活文档**, 不标版本号. 实装稳定后才打 v1 tag 并归档.

---

## 1. 设计原则 (用户拍板汇总)

| # | 原则 |
|---|------|
| 1 | 三角色 Skill / Agent / MCP 清晰划分 |
| 2 | 付费 / 免费协议完全统一 (一份 plugin.yaml schema, 用户感受不到差异) |
| 3 | subprocess + MCP 双轨, 不做 dynamic library load |
| 4 | OSS 只做文档分类 + 简单理解, 不碰行业分析 |
| 5 | plugin.yaml schema 向后兼容追加 |
| 6 | 案件库 = evidence_pool 文件夹, 用户扔什么分析什么, AI 主动分类 |
| 7 | agent 不阻塞缺失证据 (软追问标 missing) vs 红线硬阻塞 (借条不存在 reject) |
| 8 | 律师选案件类型 0 容错 (方向 A: 律师选 → agent 主动要证据), attune 不推断 case kind |
| 9 | 全部并行实装, 不分 P0/P1/P2/P3 阶段 |
| 10 | 会员账号-设备绑定 1:2 (一号最多 2 设备) |
| 11 | "旧定义有问题的分析后删, 不更新" — 纪律, 避免 stale 内容污染 |

---

## 2. 三角色定义

### Skill — 原子能力 (纯函数)

- 输入 → 输出确定, 同样输入永远同样输出 (可缓存)
- 不调多个 Skill, 不做多步推理
- 可调 LLM 但只调一次, 用于"分类 / 抽取 / 翻译"

| 域 | Skill | 用途 |
|----|-------|------|
| OSS 内部 (不暴露独立 agent) | `extract_entities` | 抽 [人名 / 日期 / 金额 / 地点 / 组织] |
| OSS 内部 | `parse_chinese_date` | "2023年1月15日" → "2023-01-15" |
| OSS 内部 | `classify_chunk_kind` | chunk → {borrowing_doc / contract / bank_statement / chat / receipt / other} |
| OSS 暴露 | `summarize_text` | 文本摘要 (单文档 < 5K tokens, 默认启用) |
| OSS 暴露 | `summarize_document_set` | 多文档汇总 (**默认禁用**, 大量 token, 用户主动开) |
| 付费插件 | `extract_loan_terms` (law-pro) | 借条 OCR → 本金/利率/起算日 |
| 付费插件 | `extract_patent_claims` (patent-pro) | 申请文件 → 权利要求结构 |

### Agent — 场景专家 (编排器)

- 多步推理 + 调多个 Skill + 调 LLM 多次
- 业务红线 (合规边界)
- 输出 audit_trail (可审计推理链)
- 声明 `case_kinds` / `consumes_evidence_kinds`

**OSS 暴露 (仅 1 个核心)**:
| Agent | 用途 |
|-------|------|
| `document_classifier_agent` | 文档分类 + 简单理解 (parties / amount / dates 抽取) |

OSS **不内置**: entity_extraction_agent (合并到内部 Skill) / meeting_minutes_agent / summary_agent (改 Skill 默认禁用).

**付费插件 Agents (例)**:
| Plugin | Agent | 用途 |
|--------|-------|------|
| law-pro | `civil_loan_agent` | 民事借贷, 强证据 + 公式 |
| law-pro | `marriage_property_agent` | 婚姻共同财产, 弱证据 + LLM 论证 |
| law-pro | `criminal_defense_agent` | 刑事辩护, 量刑情节 |
| patent-pro | `patent_oa_response_agent` | OA 答复 |
| sales-pro | `pricing_proposal_agent` | 售前报价 |

### MCP Server — 外部数据源

实现 Model Context Protocol (stdio 或 http), 暴露 `tools` 给 Agent 调用.

**生命周期 (心跳常驻, 用户拍板)**:
- 启动: 默认 `eager` (attune 启动时常驻), 重型可选 `lazy` (调用时启动)
- 健康: 每 30s 调 `tools/ping`, 失败 3 次重启
- 内存: 每 MCP 5-50MB 长期持有 (桌面单用户场景接受)
- **不做池化** (cold start 律师不接受 + 复杂度高)

**MCP 数量限制**: 不强制上限 (用户拍板). plugin.yaml 声明 `resources.external_apis` 让用户透明感知.

**例**:
| MCP | tools |
|-----|-------|
| `lpr_history_mcp` (law-pro) | `get_lpr_at_date` `list_lpr_changes` |
| `court_judgment_mcp` (law-pro) | `search_judgments` `get_full_text` |
| `gmail_mcp` (官方) | `send_email` `search_inbox` |
| `playwright_mcp` (官方, 重型 lazy) | `browse_page` `screenshot` |

---

## 3. 案件库 (CaseVault) 模型

```
Vault
└── Project
    ├── case_metadata.json
    │   ├── kind: civil-loan | civil-marriage | ...  ← 律师选 (方向 A 拍板)
    │   ├── parties: [{name, role: plaintiff/defendant/our_client_is}]
    │   ├── case_no
    │   └── classified_evidence: [...]  ← AI 自动分类填的, 律师不改
    └── evidence_pool/  ← 律师扔什么是什么
        ├── 借条扫描.pdf
        ├── 工商流水.pdf
        ├── 微信截图.png
        └── ...
```

### 自动分类流程 (agent 自动重跑)

```
[Project.evidence_pool 新增文件]
    ↓ event: file_added
[OSS::agent[document_classifier_agent] 自动跑]
    ↓
[ClassifiedEvidence 写入 case_metadata.classified_evidence]:
    file: 借条扫描.pdf
    kind: 借条
    confidence: 0.92
    parties: [甲方=张三, 乙方=任其坤]
    amount: 500000
    dates: [2023-01-15]
    raw_chunks: [chunk_id_123, ...]
    ↓ event: evidence_classified
[已注册的 case agent (per kind) 自动重跑]
    ↓
[Agent 输出更新到 Project.computation.json]
```

### Stage 工作流 (per `lawyer-rigorous-confirmation-workflow` 4 阶段, 已并入此 spec)

```
Stage 1 — 事实层抽取
  ├── bank_aggregator (parse_postal_bank_text 等结构化 parser)
  ├── document_classifier_agent
  └── 仅可证明事实, 不做"性质推断"

Stage 2 — 追问清单生成 (agent 输出 missing_evidence + soft_followups)

Stage 3 — 律师人工补充 (UI 表单, 律师显式确认 / 标记 / 填写 null 字段)

Stage 4 — 合规计算
  ├── 仅在律师 Stage 3 确认关键事实后才执行
  ├── 公式严格执行 (interest_calculator 等 lib)
  ├── audit_trail 含事实来源 + 公式 + 法条
  └── 红线硬阻塞 (借条不存在 → reject)
```

### Agent 输出 schema (统一)

```rust
pub struct AgentOutput {
    pub agent_id: String,
    pub case_id: String,
    pub computation: serde_json::Value,        // 业务自定义
    pub audit_trail: String,                   // 可审计推理链
    pub red_lines_violated: Vec<String>,       // 硬阻塞: ["借条不存在"]
    pub missing_evidence: Vec<String>,         // 软追问 (不阻塞)
    pub followups_for_lawyer: Vec<String>,     // 律师补全清单
    pub confidence: f64,                       // 0-1 整体置信度
}
```

---

## 4. plugin.yaml schema (向后兼容追加 + 加密)

### 加密

| Plugin tier | yaml 加密 |
|-------------|---------|
| free (OSS / 社区) | ❌ 明文 (审计透明) |
| paid / trial (attune-pro 等) | ✅ Argon2id + AES-256-GCM (复用 vault 加密体系) |

加密形式: `<plugin>.attunepkg` 含 `plugin.yaml.enc`, 装载时由 plugin_loader 解密.

### Schema yaml

```yaml
# 元数据 (现有, 保留)
id: law-pro
name: Attune Pro 律师助手
vendor: attune-pro
license: LicenseRef-Proprietary
version: "0.2.0"
attune_min_version: "0.6.2"
maturity: stable | beta | alpha

# 定价 (新增)
pricing:
  tier: free | trial | paid
  trial_quota: 10
  price_url: https://engi-stack.com/pro/law-pro

# 信任级别 (复用 plugin_sig.rs)
trust: Official | Trusted | Unsigned

# 资源声明 (hint 形式, 用户拍板不强制硬限制)
resources:
  total_max_llm_tokens_per_call: 10000
  total_max_cpu_seconds: 30
  external_apis: []  # 数量不限制

# 案件类型注册 (用户拍板付费插件注册 kind→agent)
registers_case_kinds:
  - kind: civil-loan
    label: 民事-借贷纠纷
    default_agent: civil_loan_agent
  - kind: civil-marriage
    label: 婚姻-财产分割
    default_agent: marriage_property_agent

# 现有字段保留: chat_trigger / capabilities / workflows
# (实装迭代中渐进迁移)

# Skills
skills:
  - id: extract_loan_terms
    description: "从借条 OCR 文本提取本金/利率/起算日"
    inputs: { text: { type: string, required: true } }
    outputs: { principal: number?, rate: number?, rate_type: string?, start_date: string? }
    runtime: rust_binary
    binary: bin/skill_extract_loan_terms
    cost: { llm_tokens: 500, cpu_seconds: 2 }
    cacheable: true

# Agents
agents:
  - id: civil_loan_agent
    description: "民事借贷纠纷 — 强证据型, 公式严格执行"
    case_kinds: [civil-loan]
    consumes_evidence_kinds: [借条, 银行流水, 微信记录, 收据]
    hard_red_lines:
      - "borrowing_relationship_established"
    soft_followups:
      - "interest_rate_specific_value"
      - "start_date_explicit"
      - "paid_interest_records"
    runtime: rust_binary
    binary: bin/agent_civil_loan
    requires_skills: [extract_loan_terms, classify_chunk_kind]
    requires_mcps: [lpr_history]
    cost: { llm_tokens: 5000, cpu_seconds: 8 }
    chat_trigger:
      enabled: true
      keywords: [本金, 利息, 借贷, 应付, 应收]
      min_keyword_match: 1
      priority: 10
      description: "借贷纠纷本息合规计算"
      exclude_patterns: ["利息税"]

# MCP Servers (心跳常驻)
mcp_servers:
  - id: lpr_history
    description: "中国央行 LPR 历史"
    transport: stdio
    command: ["bin/mcp_lpr_history"]
    tools_exposed: [get_lpr_at_date, list_lpr_changes_in_year]
    lifecycle: eager  # eager | lazy
    heartbeat_interval_seconds: 30
    restart_on_failure: 3

# UI 组件
ui_components:
  - id: civil_loan_stage3_form
    target: agent:civil_loan_agent
    html: ui/civil_loan_stage3.html
```

---

## 5. 执行模式

### Subprocess (复用 capability_dispatch.rs)

attune-core 已实装 `capability_dispatch::dispatch()` (subprocess + timeout + exit code 透传).

```rust
let result = capability_dispatch::dispatch(
    &CapabilityInvocation::new(plugin_dir.join("bin/agent_civil_loan"))
        .args(["--evidence", evidence_json_path])
        .env("LLM_ENDPOINT", "...")
        .timeout(Duration::from_secs(30))
)?;
```

### MCP (新建 mcp_client.rs, 心跳常驻)

```rust
let mcp = mcp_client::spawn_stdio_eager(plugin_dir.join("bin/mcp_lpr_history")).await?;
let lpr = mcp.call_tool("get_lpr_at_date", json!({"date": "2019-01-15"})).await?;
mcp.start_heartbeat(Duration::from_secs(30), 3);
```

---

## 6. 当前架构 gap (按需实装)

| 模块 | 现状 | 缺口 |
|------|------|------|
| `plugin_loader.rs` PluginManifest | 有 capabilities + workflows | 加 pricing / resources / registers_case_kinds / skills / agents / mcp_servers / ui_components |
| | 无加密 | 加 .attunepkg 解密 (Argon2id+AES-GCM) |
| | trust 验签 | 联动 pricing.tier (paid 必须 Trusted/Official) |
| `plugin_registry.rs` | match_chat_trigger ✅ (已实装) | 升级支持 per-agent chat_trigger / 加 agents_by_case_kind / list_skills / list_mcp_servers / all_registered_case_kinds |
| `capability_dispatch.rs` | subprocess ✅ (已实装) | 复用 |
| `mcp_client.rs` | ❌ 不存在 | 新建 (stdio + http + 心跳 + 工具注册) |
| `chat.rs` | match_chat_trigger 提示 ✅ | 加 dispatch 链路: 命中 → confirm → dispatch agent → 包装 audit_trail 回 response |
| OSS 通用 agent | ❌ 0 个 | 实装 `document_classifier_agent` + 5 内部 skill |
| 案件库 | Project schema 有基础 | 加 case_metadata.json + 自动分类触发链 |
| Stage 3 UI runtime | ❌ 不存在 | 通用 form runtime (yaml component → HTML) |
| 设备绑定 | ❌ 不存在 | accounts 服务 1:2 device 表 + attune 客户端 check |

---

## 7. 并行实装计划 (用户拍板全部并行)

约 30 天总量, 适合 4-5 人团队同时推进.

| 工程师 | 任务 | 工作量 |
|--------|------|------|
| **A (协议)** | plugin_loader schema 升级 + 加密 + trust↔pricing 联动 | 5 天 |
| **B (注册中心)** | plugin_registry 升级 + chat.rs dispatch 链路 | 5 天 |
| **C (MCP)** | mcp_client.rs 新建 (stdio + http + 心跳 + 工具注册) | 7 天 |
| **D (OSS Agent)** | 5 内部 skill + summarize_text/document_set + document_classifier_agent | 6 天 |
| **E (案件库)** | Project.case_metadata + 自动分类触发链 + evidence_classified event 重跑 | 3 天 |
| **F (UI)** | Stage 3 UI runtime + form yaml → HTML | 5 天 |
| **G (会员设备)** | accounts 1:2 device 绑定 + attune 客户端 check + 解绑 UI | 6 天 |

里程碑:
- Week 1: A B C 协议 + 注册 + MCP 主干上线 (单元测试)
- Week 2: D E OSS agent + 案件库自动分类
- Week 3: F UI runtime + G 设备绑定
- Week 4: attune-pro 第一批 agent (civil_loan / marriage_property) 重构到新协议

---

## 8. 会员账号-设备绑定 (1:2)

### 规则

- 1 个账号 = 最多 2 台设备激活
- 第 3 台设备登录: UI 提示选择"踢下线某台" 或 "取消"
- 离线时使用 cached license 30 天有效
- 跨设备 (笔电 ↔ K3 一体机) 共享同一账号但占 2 个 slot

### Device Fingerprint

```rust
pub struct DeviceFingerprint {
    pub device_id: String,       // 本地生成 UUID v4 (持久化到 vault)
    pub hostname: String,
    pub os: String,
    pub cpu_brand: String,
    pub hardware_uuid: Option<String>,
    pub form_factor: String,     // laptop | desktop | k3_appliance
}
```

### 云端 accounts 服务 (attune-cloud 仓)

```sql
CREATE TABLE devices (
    device_id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts(id),
    fingerprint_signature BYTEA NOT NULL,
    hostname TEXT, os TEXT, cpu_brand TEXT, form_factor TEXT,
    activated_at TIMESTAMPTZ DEFAULT NOW(),
    last_seen_at TIMESTAMPTZ DEFAULT NOW(),
    deactivated_at TIMESTAMPTZ
);
CREATE INDEX idx_account_active ON devices(account_id) WHERE deactivated_at IS NULL;
-- 限制: 同一 account_id 最多 2 个 active device (业务层 + 数据库约束)
```

### API endpoints

```
POST /api/v1/devices/register
  body: { account_id, fingerprint }
  200: { device_id, license_token }
  409 max_devices_reached: { existing_devices: [...] }

POST /api/v1/devices/{id}/deactivate
  body: { confirm: true }

GET /api/v1/devices?account_id=...
```

### attune 客户端启动逻辑

```rust
async fn boot_device_check() -> Result<DeviceLicense> {
    let local_fp = DeviceFingerprint::collect_local();
    let cached = read_local_cache();

    if has_internet() {
        match accounts_client::verify_or_register(&cached, &local_fp).await {
            Ok(lic) => { cache_locally(&lic); return Ok(lic); }
            Err(MaxDevicesReached(existing)) => {
                let chosen = ui::show_device_choice_modal(existing).await?;
                accounts_client::deactivate_device(chosen).await?;
                let lic = accounts_client::register_device(&local_fp).await?;
                return Ok(lic);
            }
        }
    }

    // 离线: cached 30 天有效
    if let Some(lic) = cached.filter(|l| l.is_within_30_days()) {
        return Ok(lic);
    }
    Err("offline + cached license expired")
}
```

### UI

- 设置 → 我的会员 → 已绑设备列表 (hostname / 最后在线 / 当前)
- 踢下线 / 退出本设备

---

## 9. 测试规范 (并入 lawyer golden set 思路)

每个 agent 至少覆盖两层 (per CLAUDE.md 测试金字塔):

| 层 | 用途 |
|----|------|
| Unit (skill) | 纯函数 + 边界 |
| Integration (agent) | agent 编排 + skill 调用 + LLM mock |
| Corpus (真实 GitHub 知识库语料) | 检索质量 |
| E2E (端到端真实 LLM + 真实 OCR) | 律师真实案件验证 |
| Golden Case (真实匿名案件) | 业务红线 PASS / FAIL (精确到分) |

**Golden case 规则**:
- 真实匿名案件 + 法务专家 ground truth 标注
- yaml schema (`tests/golden/case-N.yaml`): parties + facts_stage1 + facts_stage3 + expected_calculation + business_red_lines + reviewer
- 法务专家 `reviewer.approved=true` 才计入 CI
- 不公开 raw PDF (法律风险)

**禁止**:
- 随机生成测试数据
- "any integer" / "any string" 空洞断言
- 假设结论数字当 ground truth (如假设月利 1% 算出的 ¥6,668,227.20 不能写进测试)

---

## 10. 不在本 spec 范围

- Plugin 安装 / 升级 / 卸载工具链 (单独 spec)
- Plugin Hub 商店 / 评分 (M3+)
- Plugin 国际化 i18n (M4+)
- 跨平台分发包 .attunepkg binary layout 详细 (单独 spec)
- accounts 服务 SaaS 部署细节 (attune-cloud 仓)

---

## 11. Changelog

| 日期 | 变更 |
|------|------|
| 2026-05-10 | 初稿 (替代 attune-plugin-protocol-2026-05-10.md v1, 整合用户 11 决策点 + 案件库 + 设备绑定 + 9 个旧 spec 删除) |

### 已删旧 spec (避免引用错乱)

attune:
- `attune-plugin-protocol-2026-05-10.md` (v1) — 已并入本 spec

attune-pro:
- `m3-roadmap-2026-05-10.md`
- `lawyer-renqiqi-final-result-2026-05-10.md` (含假设结论数字, 风险)
- `dual-axis-product-audit-2026-05-09.md`
- `law-pro-bank-statement-aggregator-design.md` (含 ICBC 启发式残留)
- `law-pro-vertical-workflow-design.md` (vertical workflow 概念被 case_kinds + agents 替代)
- `patent-pro-vertical-workflow-design.md`
- `academic-pro-vertical-workflow-design.md`
- `lawyer-golden-set-test-matrix.md` (思路并入 §9 测试规范)
- `lawyer-rigorous-confirmation-workflow.md` (4 阶段 SOP 并入 §3 案件库 Stage 工作流)

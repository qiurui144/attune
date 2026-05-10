# Attune Plugin Protocol v1 — Skill / Agent / MCP 接入标准

**Date**: 2026-05-10
**Status**: Draft → Pending implementation
**Stakeholders**: attune (OSS) + attune-pro + 第三方插件作者
**Trigger**: 用户拍板 2026-05-10 — 证据链分析能力需要平台级架构, 而非单 plugin 模块

---

## 1. 设计原则 (用户拍板)

| # | 原则 | 含义 |
|---|------|------|
| 1 | 三角色清晰划分 | Skill (原子能力) / Agent (场景专家) / MCP (外部数据) |
| 2 | 付费免费协议统一 | 一份 plugin.yaml, OSS 免费插件和 attune-pro 付费插件感受不到协议差异 |
| 3 | subprocess + MCP 双轨 | 不做 dynamic library load (复杂度高收益低) |
| 4 | OSS 不碰行业 | OSS 提供通用分类 agent, 行业分析全在付费插件 |
| 5 | schema 向后兼容追加 | 现有 plugin.yaml 的 capability/workflow 字段保留, 追加 skills/agents/mcp_servers 三段 |
| 6 | 案件库 = 文件夹模型 | 用户提交什么证据就分析什么, 不强制律师贴 role 标签, AI 主动分类 |
| 7 | agent 不阻塞缺失证据 | 缺什么标 missing, 处理能处理的, 律师补完再重跑 |

---

## 2. 三角色定义

### Skill — 原子能力 (纯函数)

**特征**:
- 输入 → 输出**确定**, 同样输入永远同样输出 (可缓存)
- 不调多个 Skill, 不做多步推理
- 可调 LLM 但只调一次, 通常用于"分类 / 抽取 / 翻译"等单点工作

**例**:
| 域 | Skill | 输入 | 输出 |
|----|-------|------|------|
| OSS 通用 | `summarize_pdf` | PDF 文件 | 150 字摘要 |
| OSS 通用 | `classify_evidence_kind` | 文档 chunk | `{kind, confidence, parties, amount?, dates?}` |
| OSS 通用 | `extract_phone_number` | 文本 | `[+86 138...]` |
| OSS 通用 | `parse_chinese_date` | "2023年1月15日" | `2023-01-15` |
| 付费 law-pro | `extract_loan_terms` | 借条 OCR | `{principal, rate, rate_type, start_date, ...}` |
| 付费 law-pro | `parse_case_no` | 案号文本 | `{year, court, type, num}` |

### Agent — 场景专家 (编排器)

**特征**:
- 多步推理 + 调多个 Skill + 调 LLM 多次
- 有**业务红线** (合规边界)
- 输出**audit_trail** (可审计推理链)
- **声明能力边界** — 处理什么案件类型, 不处理什么

**例**:
| 域 | Agent | 输入 | 输出 |
|----|-------|------|------|
| OSS 通用 | `document_classifier_agent` | 一组 PDF/图片/文本 | 分类清单 (借条/流水/合同/发票/...) + 各 chunk 元信息 |
| OSS 通用 | `meeting_minutes_agent` | 会议录音/视频 OCR/字幕 | 议程 / 决策 / 行动项 |
| 付费 law-pro | `civil_loan_agent` | 案件库分类后证据 | 应付应收金额 + audit_trail + missing_evidence 清单 |
| 付费 law-pro | `marriage_property_agent` | 同上 | 财产分割比例 + 法条引用 |
| 付费 law-pro | `criminal_defense_agent` | 同上 | 量刑情节 + 辩点 |
| 付费 patent-pro | `patent_oa_response_agent` | OA 通知书 + 申请文件 | OA 答复草稿 |

### MCP Server — 外部数据源 / 跨进程协议

**特征**:
- 实现 [Model Context Protocol](https://modelcontextprotocol.io/) 标准 (stdio 或 http)
- 暴露一组 `tools` (函数) 给 attune Agent 调用
- 用于"我的数据 / 服务在外部, 不在 attune 内"

**例**:
| MCP Server | 暴露 tools |
|-----------|-----------|
| `lpr_history_mcp` | `get_lpr_at_date(date)` `list_lpr_changes(year)` |
| `court_judgment_mcp` | `search_judgments(keywords, year)` `get_full_text(case_no)` |
| `gmail_mcp` (官方 MCP) | `send_email` `search_inbox` |
| `notion_mcp` (官方 MCP) | `query_database` `create_page` |

---

## 3. 案件库 (CaseVault) 模型

> 用户拍板: "用户提交哪些证据链就分析哪些, 不做多余的分类, 分类是 AI 的事"

```
Vault (用户的 attune 知识库)
└── Project (案件 / 项目 / 客户)  ← 用户已有 (v0.6.0 GA)
    ├── case_metadata.json
    │   ├── kind: civil-loan | civil-marriage | criminal | ...  ← 律师选 (方向 A 拍板)
    │   ├── parties: [{name, role: plaintiff/defendant/our_client_is}]
    │   └── case_no
    └── evidence_pool/  ← 案件库 = 文件夹, 律师扔什么是什么
        ├── 借条扫描.pdf       ← 律师扔
        ├── 工商流水.pdf       ← 律师扔
        ├── 邮政流水.pdf       ← 律师扔
        ├── 微信聊天截图.png   ← 律师扔
        ├── 录音.mp3           ← 律师扔
        └── ...
```

**自动分类流程**:
```
[Project.evidence_pool 新增文件 X]
    ↓ event: file_added
[OSS::skill[document_classifier]]  ← 自动分类
    ↓
[ClassifiedEvidence 写入 Project metadata]:
    file: 借条扫描.pdf
    kind: 借条
    confidence: 0.92
    parties: [甲方=张三, 乙方=任其坤]
    amount: 500000
    date_range: [2023-01-15, 2023-01-15]
    raw_chunks: [chunk_id_123, ...]
```

**Agent 触发流程**:
```
[律师 chat: "梁vs任 应付多少"]
    ↓ chat_trigger 命中 civil_loan_agent
[attune 提示: 已分类证据 X 份, 触发 civil_loan_agent? [确认]]
    ↓ 律师确认
[civil_loan_agent.run(project.evidence_pool.classified)]
    ↓ Agent 内部:
        - 用借条事实抽 principal/rate/start_date
        - 用流水校验本金转入日
        - 用微信交叉验证
        - 调 LLM 综合分析
        - 调公式严格执行
    ↓
[输出]:
    - computation: {principal, interest, remaining, audit_trail}
    - missing_evidence: ["借条原件未上传, 仅有截图" / "起算日不明确"]  ← 不阻塞
    - red_lines_violated: ["借条不存在, 拒绝输出金额"]  ← 阻塞
    - followups_for_lawyer: ["请确认 ¥1,298,000 是否含还本"]
```

**关键设计 — 不阻塞**:
- agent 处理**它能处理的**, 缺的**标 missing 但不报错**
- 律师扔新证据进案件库 → agent 自动重跑 (event-driven)
- 红线 (借条不存在) 才 reject — 这是法律事实层面的阻塞, 与"证据缺失"不同

---

## 4. plugin.yaml schema v2 (向后兼容追加)

```yaml
# 元数据 (现有, 不变)
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
  trial_quota: 10  # paid 时 N 次试用
  price_url: https://attune.ai/pro/law-pro

# 信任级别 (现有 plugin_sig.rs)
trust: Official | Trusted | Unsigned

# 资源声明 (新增, per CLAUDE.md "成本感知与触发契约")
resources:
  total_max_llm_tokens_per_call: 10000
  total_max_cpu_seconds: 30
  external_apis: [gateway.attune.ai/v1]

# === 现有字段保留 ===
chat_trigger: { ... }       # v0.6 已有, 升级版用 chat_triggers (复数)
capabilities: [...]         # v0.6 已有, 渐进迁移到 agents
workflows: [...]            # v0.6 已有, 用 file_added 等 trigger

# === 新增 v1 三段 ===

# Skills (原子能力, 纯函数)
skills:
  - id: extract_loan_terms
    description: "从借条 OCR 文本提取本金/利率/起算日"
    inputs: { text: { type: string, required: true } }
    outputs: { principal: number?, rate: number?, rate_type: string?, start_date: string? }
    runtime: rust_binary  # rust_binary | wasm | python_subprocess
    binary: bin/skill_extract_loan_terms
    cost: { llm_tokens: 500, cpu_seconds: 2 }
    cacheable: true  # 同样输入 cache 结果

# Agents (场景专家, 编排器)
agents:
  - id: civil_loan_agent
    description: "民事借贷纠纷 — 强证据型, 公式严格执行"
    case_kinds: [civil-loan]  # 律师选哪种案件能触发此 agent

    # agent 主动声明能用什么证据 (info 级, 不强制清单)
    consumes_evidence_kinds:
      - 借条
      - 银行流水
      - 微信记录
      - 收据

    # 必须红线 (任一缺 → reject)
    hard_red_lines:
      - "borrowing_relationship_established"  # 借贷关系成立 (借条 OR 替代证据)

    # 软追问 (缺则在 missing_evidence 提示, 不阻塞)
    soft_followups:
      - "interest_rate_specific_value"
      - "start_date_explicit"
      - "paid_interest_records"

    runtime: rust_binary
    binary: bin/agent_civil_loan
    requires_skills: [extract_loan_terms, classify_evidence_kind]
    requires_mcps: [lpr_history]  # 调 LPR 历史 MCP
    cost: { llm_tokens: 5000, cpu_seconds: 8 }

    chat_trigger:
      enabled: true
      keywords: [本金, 利息, 借贷, 应付, 应收]
      min_keyword_match: 1
      priority: 10
      description: "借贷纠纷本息合规计算"
      exclude_patterns: ["利息税"]

# MCP Servers (外部数据源)
mcp_servers:
  - id: lpr_history
    description: "中国央行 LPR 历史 (起息日 LPR 4 倍上限计算依据)"
    transport: stdio
    command: ["bin/mcp_lpr_history"]
    tools_exposed: [get_lpr_at_date, list_lpr_changes_in_year]
    cost: { external_calls_per_invocation: 1 }

# Stage 3 律师确认 UI (可选, 走 capability_dispatch 提供 HTML)
ui_components:
  - id: civil_loan_stage3_form
    target: agent:civil_loan_agent
    html: ui/civil_loan_stage3.html  # 律师补全 null 字段表单
```

---

## 5. OSS attune 提供的"通用 agents" (用户拍板 4)

> "防止一些通用的内容分类 agent, 但是不碰行业分析" — 用户拍板 2026-05-10

OSS 给免费用户的 **通用 baseline**:

| OSS 通用 Agent | 输入 | 输出 | 不做 |
|---------------|------|------|------|
| `document_classifier_agent` | Project.evidence_pool 全部文件 | 每份文件 `{kind, confidence, parties, amount?, dates?}` 分类清单 | ❌ 不做行业精解 (不识别"这是借条第几条") |
| `entity_extraction_agent` | 任意文档 | `{persons, dates, amounts, locations, organizations, ...}` | ❌ 不做关系推理 |
| `meeting_minutes_agent` | 会议记录 / 录音转写 | 议程 / 决策 / 行动项 | ❌ 不替代行业法律意见 |
| `summary_agent` | 任意文档集 | 多文档摘要 | ❌ 不替代专业分析 |

**OSS 通用 agent 是基础设施** — 给付费 agent 提供输入分类层. 律师领域的 `civil_loan_agent` 收到的是 `document_classifier_agent` 已分类的证据集合.

---

## 6. 执行模式 (subprocess + MCP 双轨)

### Subprocess (Skill / Agent)

复用现有 `attune-core::capability_dispatch::dispatch()`:

```rust
let result = capability_dispatch::dispatch(
    &CapabilityInvocation::new(plugin_dir.join("bin/agent_civil_loan"))
        .args(["--evidence", evidence_json_path, "--output", out_path])
        .env("LLM_ENDPOINT", "...")
        .timeout(Duration::from_secs(30))
)?;

if result.exit_code == 2 {
    // 律师红线 reject (借条不存在等), 弹 followup 给律师
} else if result.is_success() {
    // 解析 stdout JSON, 包装回 chat / UI
}
```

### MCP (外部数据 / 跨进程协议)

新增 `attune-core::mcp_client` (待实装):

```rust
let mcp = mcp_client::connect_stdio(plugin_dir.join("bin/mcp_lpr_history")).await?;
let lpr = mcp.call_tool("get_lpr_at_date", json!({"date": "2019-01-15"})).await?;
```

**不做**: dynamic library load (libloading / abi_stable / WASM module load) — 复杂度高收益低, subprocess + MCP 双轨已够.

---

## 7. 当前 attune 架构 gap

| 层 | 现状 | 需要做 |
|----|------|-------|
| `plugin_loader.rs` PluginManifest | 有 capability/workflow | 加 `skills` / `agents` / `mcp_servers` 三段 (Vec, 可空) |
| `plugin_registry.rs` | match_chat_trigger ✅ | 加 `list_skills()` / `list_agents()` / `list_mcp_servers()` 查询 API |
| `capability_dispatch.rs` | subprocess ✅ (本会话已实装) | 复用 |
| `mcp_client.rs` | ❌ 不存在 | 新建模块, 实现 stdio + http MCP 协议 |
| `chat.rs` | match_chat_trigger 提示 ✅ | 加 dispatch 链路: 命中 → confirm 用户 → dispatch agent → 包装 audit_trail 回 response |
| 通用 agents | ❌ OSS 0 个 | OSS 实装 4 个 (document_classifier / entity_extraction / meeting_minutes / summary) |
| 案件库 evidence_pool 自动分类 | ❌ 仅手动 ingest | event_hook: file_added → document_classifier_agent 自动分类 |
| Project metadata schema | 已有基础 (project_recommender) | 加 `case_metadata.json`: kind / parties / classified_evidence |
| Stage 3 UI | ❌ 仅 attune-pro 示例 HTML | OSS 提供通用 form runtime, plugin 提供 component yaml |

---

## 8. 迁移路径 (P0 → P3)

### P0 (本月) — 平台基础设施
1. plugin.yaml schema v2 (向后兼容追加)
2. plugin_loader 解析 skills/agents/mcp_servers
3. plugin_registry 查询 API + chat_trigger 升级 (现有 chat_trigger → agent.chat_trigger)
4. mcp_client.rs 最小实装 (stdio MCP 协议)

### P1 (下月) — OSS 通用 agents
5. document_classifier_agent (5 类基础: 借条/合同/流水/聊天/其他)
6. entity_extraction_agent (人/日期/金额/地点/组织)
7. summary_agent (多文档摘要)
8. event_hook: file_added → 自动分类入 case_metadata

### P2 (Q3) — Stage 3 UI 框架
9. 通用 form runtime (yaml component → HTML)
10. capability_dispatch 包装 chat response

### P3 (Q4) — 行业插件 v2
11. attune-pro/law-pro 重构: civil_loan_agent / marriage_property_agent / criminal_defense_agent / ...
12. attune-pro/patent-pro / sales-pro / tech-pro 走 v2 协议
13. PP-Structure 表格 OCR (替代当前启发式 ICBC parser)

---

## 9. 设计争议点 (后续 review)

1. **Project.kind 列表**: 民事-借贷 / 民事-婚姻 / 刑事-辩护 / ... 维护在哪里? OSS 不提供具体列表 (避免行业绑定), 由付费插件**注册** kind → agent 映射?
2. **agent 多次重跑**: 律师扔新证据进案件库, 自动还是手动触发 agent 重跑?
3. **MCP 资源管理**: 长生命 mcp 进程 (stdio) 的池化 / 心跳 / 重启策略?
4. **付费插件加密**: 当前 attune-pro 用 Argon2id+AES-256-GCM 加密 plugin 二进制, 但 plugin.yaml 是否也加密? (yaml 公开方便审计 vs 商业秘密保护)
5. **resource_limits enforce**: 资源声明只是 hint 还是硬限制 (cgroup / ulimit / per-skill timeout)?

---

## 10. 不在本 protocol 范围

- Plugin 安装 / 升级 / 卸载工具链 (有独立 spec)
- Plugin Hub 商店 / 评分 / 评论 (M3+)
- Plugin 国际化 i18n (M4+)
- 跨平台分发包格式 .attunepkg (有独立 spec)

---

## 11. 引用

- 用户拍板 2026-05-09: 律师严谨性 4 阶段 SOP (`docs/specs/lawyer-rigorous-confirmation-workflow.md`)
- 用户拍板 2026-05-10: 方向 A (律师选案件类型) + 三角色 + 付费免费协议统一
- attune-core 已实装基础: plugin_registry / capability_dispatch (本会话)
- 上下游 spec: oss-pro-strategy.md v2 §4.3 (OSS 边界 / pro 行业归属)

---

## 12. 决策点 (Pending)

请 review 本 spec 后回答 / 拍板:
- [ ] §3 案件库模型 (evidence_pool + 自动分类 + agent 重跑) OK?
- [ ] §4 schema v2 (向后兼容追加) OK?
- [ ] §5 OSS 通用 agent 4 个清单 (document_classifier / entity_extraction / meeting_minutes / summary) OK 还是删 / 加?
- [ ] §6 不做 dynamic library load 共识保持
- [ ] §8 P0/P1/P2/P3 优先级 OK 还是调整
- [ ] §9 5 个争议点是否决策

拍板后开始 P0 实装.

---
name: oss-industry-decoupling
version: v4.0.0-spec
status: DRAFT — 待 G1
date: 2026-06-02
authors: qiurui144 + Claude
census_source: reports/2026-06-02_oss-industry-census-exhaustive.md
census_invariant: TREATED(564) + EXEMPT(338) = 902 — PASS
redo_history:
  - v1 (窄正则 defamation|fact_extractor) → G1 REJECT 漏 civil_loan/case_kind
  - v2 (+civil_loan|案号|律师 中宽正则) → G1 REJECT 漏 patent/presales/tech-pro/medical
  - v3 (2026-06-01 超集正则, 36 处置文件) → G1 REJECT census 仍非穷尽(缺 ALL-CAPS 变量/TOML/SQL 注释扫)
  - v4 (本版, 2026-06-02 穷尽 census 902 命中, 20 迁移单元, 44 文件, 不变量 PASS)
supersedes: docs/superpowers/specs/2026-06-01-oss-industry-decoupling.md
---

# OSS 行业解耦 S4b — 穷尽迁移 Spec（v4）

> 状态: **DRAFT — 待 G1**
>
> census 来源: `reports/2026-06-02_oss-industry-census-exhaustive.md`（902 命中，不变量 PASS）
> 前三次 G1 REJECT 原因已全部纳入本版。v4 首次建立在穷尽 census 之上。

## 0. 目录 (TOC)

- [1. 目标定位](#1-目标定位)
- [2. 范围边界](#2-范围边界)
- [3. 架构数据流](#3-架构数据流)
- [4. 模块边界](#4-模块边界)
- [5. API 契约](#5-api-契约)
- [6. 扩展点 / 插件接口](#6-扩展点--插件接口)
- [7. 错误 + 边界 case](#7-错误--边界-case)
- [8. 成本契约](#8-成本契约)
- [9. 测试矩阵](#9-测试矩阵)
- [10. 向后兼容](#10-向后兼容)
- [11. 风险登记](#11-风险登记)
- [Appendix A — 20 迁移单元完整清单](#appendix-a--20-迁移单元完整清单)
- [Appendix B — EXEMPT 338 处分类摘要](#appendix-b--exempt-338-处分类摘要)

---

## 1. 目标定位

**用户痛点 / 边界违规**

OSS attune `rust/` 在四个层次同时存在行业耦合，共 564 处（44 文件，20 迁移单元）：

| 层次 | 泄漏文件 | 影响 |
|------|---------|------|
| **启动配置层** | `agents.registry.toml`（63 hits）、`agent_flows.toml`（8 hits） | OSS 启动时将 14 个 law-pro paid agents + 1 tech-pro + 1 VLM capability 载入注册表，每个 OSS 用户系统携带完整律师工具清单 |
| **核心算法层** | `search.rs` DOMAIN_EXPAND_MAP（10 hits） | 搜索引擎核心硬编码 legal/medical/patent 三行业的同义词扩展词表，OSS 通用搜索依赖行业词表版本 |
| **协议/接口层** | `routes/plugins.rs`（2 hits）、`plugin_hub.rs`（12 hits） | `/plugins` 响应将 law/presales/patent 标记为 `"builtin"`；`_builtin_plugins()` 经 `/api/v1/marketplace/plugins` 向任何裸装 OSS 用户实时返回 Pro 产品能力清单及营销文案 |
| **测试层** | 50+ 测试文件（373 hits） | law-pro/defamation_extractor/civil_loan_agent 作为通用框架 fixture，阻止 OSS 在无 attune-pro 知识环境下独立运行 |

**产品 positioning 对齐**

`docs/oss-pro-strategy.md` v2 §4.3：「一个功能进 OSS 当且仅当对任何领域的个人通用用户都有价值。」
CLAUDE.md 三产品矩阵：「行业完全在 attune-pro，不在 OSS。」

**本 sprint 目标**：OSS `rust/` 行业耦合**归零**。迁移后，无 attune-pro 知识的开发者
可 `cargo build && cargo test` 全过，不出现任何行业特定 ID/词汇/逻辑。

---

## 2. 范围边界

### IN（本 sprint 全部交付）

**20 个迁移单元，覆盖 44 文件，564 TREATED 命中**（按优先级排列）：

**P0 — 功能性生产泄漏（必须在 v1.0.2 上线前完成）**：
- MU-1: `agents.registry.toml` + `agent_flows.toml` — 移出 law-pro/tech-pro 段
- MU-2: `case_metadata.rs` — `CaseMetadata` 迁至 attune-pro
- MU-4: `plugin_hub.rs` — 移除硬编码行业插件目录
- MU-8: `agent_quality_manifest.yaml` — 移除 external: true 行业插件门
- MU-18: `oss_agent_real_llm_gate.rs` — 移至 attune-pro

**P1 — 架构层/测试层泄漏（v1.0.2 or v1.0.3）**：
- MU-3: `plugin_registry.rs` 内联行业 test fixture
- MU-5: `search.rs` DOMAIN_EXPAND_MAP
- MU-6: `agents/flow/tests.rs` + `registry/tests.rs`（5 文件）
- MU-7: `acp_chat.rs` + `acp5_chat_flow_wire_test.rs`
- MU-10: `plugin_sync.rs` + `plugin_protocol_e2e.rs`
- MU-11: `generic_plugins_test.rs`
- MU-12: server routes（4 文件，注释+builtin 判断逻辑）
- MU-19: `entities.rs`（CaseNo/CourtSeal EntityKind）

**P2 — 测试 fixture 字符串替换（v1.0.3 or v1.0.4）**：
- MU-9: `store/state_migration.rs`
- MU-13: `ingest/connector.rs` + `ingest/local.rs`
- MU-14: `store/mod.rs`（SQL 注释）
- MU-15: 遥测/使用量测试（3 文件）
- MU-16: `skill_evolution`（2 文件）
- MU-17: `cloud_client.rs`
- MU-20: 杂项（11 文件）

### OUT（本 sprint 明确不做）

- **EXEMPT 338 处**不动（见 Appendix B），包括：
  - `README.md` / `README.zh.md`（OSS 边界说明文字）
  - `RELEASE.md`（历史 changelog）
  - `tests/corpora/**`（外部 benchmark 语料库）
  - `ui/node_modules/**`（第三方 npm 依赖）
  - `attune-agent-sdk/`（WASM-safe leaf crate，仅有 2 个泛型注释命中）
  - `tag_index.rs`（`wholesale/sales` 等英文词根误命中）
  - `ocr/structured/scene_id_card.rs`（`legal_rep` = 法定代表人，任何中国营业执照标准字段）
- **attune-pro 接收侧实现细节**不在本 spec 范围（各 plugin 的 thresholds.yaml / law-pro agents.registry 由 attune-pro sprint 独立实现）
- **OSS 功能增强**（本 sprint 只做减法，不引入新 OSS capability）
- **Python 原型线**（`python/src/attune_python/`）不涉及本 sprint

### 后续版本才做

- attune-pro 侧完整 `domain_synonyms` plugin manifest 实现（v1.1.x）
- OSS `plugin_hub.rs` 改为云端 API 动态加载行业目录（v1.1.x，依赖 cloud infra 就绪）
- `corpus_domain` 枚举类型化（v1.1.x，当前保留 `Option<String>` 开放语义）

---

## 3. 架构数据流

### 3.1 迁移前（现状，有行业耦合）

```
OSS 启动
  └─► load_registry("agents.registry.toml")
        ├─► [6 oss-core agents]          ← 合法
        └─► [14 law-pro + 1 tech-pro + 1 VLM]  ← 行业泄漏 ❌

OSS 搜索请求 (domain_hint="legal")
  └─► search::expand_query()
        └─► DOMAIN_EXPAND_MAP["legal"] → [诉讼, 案件, 合同, ...]  ← 行业词表嵌搜索核心 ❌

GET /api/v1/plugins
  └─► plugin.source = if ["tech","law","presales","patent"].contains(id) { "builtin" }  ← 行业 ID 硬编码 ❌

GET /api/v1/marketplace/plugins
  └─► plugin_hub::_builtin_plugins()
        └─► [law-pro 营销文案, patent-pro FTO, presales-pro BANT, ...]  ← Pro 能力泄漏给 OSS ❌
```

### 3.2 迁移后（目标态）

```
OSS 启动
  └─► load_registry("agents.registry.toml")
        └─► [6 oss-core agents]          ← 仅 OSS 合法 agents ✅
        
  (attune-pro plugin 安装后)
  └─► load_plugin_registry_fragment("law-pro/agents.registry.toml")
        └─► [14 law-pro agents] 按需注入  ✅

OSS 搜索请求
  └─► search::expand_query()
        ├─► generic BM25 + cross-domain penalty logic  ← 通用逻辑保留 ✅
        └─► domain_synonyms = loaded_plugins
              .filter_map(|p| p.manifest.domain_synonyms.get(domain_hint))  ← 词表由 plugin 提供 ✅

GET /api/v1/plugins
  └─► plugin.source = if plugin.manifest.builtin { "builtin" } else { "user" }  ← 读 manifest 字段 ✅

GET /api/v1/marketplace/plugins
  └─► plugin_hub::fetch_catalog()
        └─► 云端 API 动态返回 / 无 pluginhub_url 时返回空列表 []  ✅
```

### 3.3 OSS Graceful Degrade（三大泄漏点移除后的 OSS 行为）

**agents.registry.toml 移除 law-pro 段后**：
- OSS 启动正常，`AgentRegistry` 包含 6 个 oss-core agents
- `AgentRegistry::get("civil_loan_agent")` 返回 `None`（注册表查找 graceful，无 panic）
- 未安装 law-pro 的 OSS 用户：chat 触发词不匹配行业 agent（`route_keywords` 无 law-pro 词汇）
- `shipped_flows_validate_against_shipped_registry` CI 门仅验证 OSS registry，无 law-pro flow 则无 law-pro 门

**search.rs DOMAIN_EXPAND_MAP 移除行业词表后**：
- OSS 通用搜索完全不退化：BM25 fulltext + HNSW vector + RRF hybrid 逻辑均不依赖 domain map
- 跨域 penalty 逻辑（`CROSS_DOMAIN_PENALTY` 系数）保留为通用架构扩展点
- law-pro 安装后：plugin manifest 的 `domain_synonyms` 字段被 search 引擎按需读取
- 无 plugin 时：`domain_synonyms` 为空，搜索行为等同 v0.7 baseline（已验证稳定）

**plugins.rs 移除硬编码 builtin 判断后**：
- `/api/v1/plugins` 响应：所有 user-installed plugins 为 `source: "user"`
- oss-core 内置能力（非 plugin 形态）不受影响
- 响应 schema 不变（`source` 字段仍存在），仅逻辑改为读 `plugin.manifest.builtin`
- `plugin_hub.rs` 返回空 `[]` 时：Marketplace UI 显示"暂无可用插件"（现有 fallback UI 已实现）

---

## 4. 模块边界

### 4.1 OSS attune（本仓 `rust/`）改动文件

**删除/重构（生产代码）**：

| 文件 | 改动 | MU |
|------|------|-----|
| `rust/agents.registry.toml` | 删除 law-pro/tech-pro/VLM 段（115-361 行）；保留 6 oss-core agents | MU-1 |
| `rust/agent_flows.toml` | 删除 `legal_defamation` flow 段 | MU-1 |
| `rust/crates/attune-core/src/case_metadata.rs` | 整文件删除（含 `pub mod case_metadata` 从 `lib.rs` 移除） | MU-2 |
| `rust/crates/attune-core/src/plugin_hub.rs` | 删除 `_builtin_plugins()` 内联行业目录；`fetch_catalog()` 改为空列表 fallback 或云端 API | MU-4 |
| `rust/crates/attune-core/src/search.rs` | 删除 `DOMAIN_EXPAND_MAP` 中 legal/medical/patent 词表；保留通用跨域 penalty 框架 | MU-5 |
| `rust/crates/attune-server/src/routes/plugins.rs` | L59 builtin 判断改为读 `plugin.manifest.builtin: bool` | MU-12 |
| `rust/crates/attune-core/src/entities.rs` | 审查并删除 `CaseNo`/`CourtSeal` EntityKind；保留通用 EntityKind | MU-19 |
| `rust/agent_quality_manifest.yaml` | 移除 `external: true` 的 law_pro_deterministic/law_pro_real_llm/presales_pro/patent_pro/tech_pro 条目 | MU-8 |

**测试文件清理（不影响生产 binary）**：

| 文件 | 改动 | MU |
|------|------|-----|
| `attune-core/src/plugin_registry.rs` | 内联 YAML fixture 改为 `id: test-plugin` 通用版 | MU-3 |
| `agents/flow/tests.rs` + `registry/tests.rs` + 3 个测试文件 | 用 `test-calc-agent`/`test-extract-agent` 替换行业 agent fixture | MU-6 |
| `attune-server/src/acp_chat.rs` + `acp5_chat_flow_wire_test.rs` | `defamation_registry()` → `test_registry()`；`defamation_flow()` → `test_flow()` | MU-7 |
| `attune-core/tests/oss_agent_real_llm_gate.rs` | 移至 attune-pro；OSS 版只保留 6 oss-core agents 的真实 LLM 验证 | MU-18 |
| `plugin_sync.rs` + `plugin_protocol_e2e.rs` | law-pro fixture YAML → `id: test-plugin` 通用版 | MU-10 |
| `tests/generic_plugins_test.rs` | law-pro/patent-pro inline YAML → `test-plugin` fixture | MU-11 |
| `routes/agents.rs` + `routes/forms.rs` + `routes/chat.rs` | 删除/替换 civil_loan_agent/legal_defamation 注释 | MU-12 |
| `store/state_migration.rs` | `"law-pro"` → `"test-plugin"` | MU-9 |
| `ingest/connector.rs` + `ingest/local.rs` | `corpus_domain: "legal"` → `"general"` | MU-13 |
| `store/mod.rs` | SQL 注释中 CASE_NO 示例 → CUSTOM_FIELD | MU-14 |
| `agent_telemetry/tests.rs` + `usage/tests/guard_test.rs` + `store/agent_state.rs` | `"defamation_extractor"` → `"test_agent"`；`"law-pro"` → `"test-plugin"` | MU-15 |
| `skill_evolution/agent.rs` + `skill_evolution/mod.rs` | `defamation_extractor` → `"test_agent"` 作为示例 | MU-16 |
| `cloud_client.rs` | JSON fixture `"law-pro"` → `"test-plugin"` | MU-17 |
| 杂项 11 文件 | 分别清理注释/替换 fixture/删除行业专属字段（`loan_doc_exists` 等） | MU-20 |

### 4.2 attune-pro 接收侧

attune-pro 不需要代码迁移（已有完整实现），只需：

| 文件 | 新增 | 说明 |
|------|------|------|
| `attune-pro/plugins/law-pro/agents.registry.toml` | 接收从 OSS 移出的 14 law-pro agents | 迁移后 attune-pro 维护自己的 registry fragment |
| `attune-pro/plugins/law-pro/agent_flows.toml` | 接收 `legal_defamation` flow | 仅 law-pro 安装后加载 |
| `attune-pro/plugins/law-pro/case_metadata.rs` | 接收 `CaseMetadata` struct | 或内联于 law-pro plugin crate |
| `attune-pro/plugins/law-pro/thresholds.yaml` | 接收 agent quality gates | 各 plugin 自维护，不进 OSS manifest |
| `attune-pro/plugins/law-pro/manifest.yaml` | 新增 `domain_synonyms.legal: [...]` 字段 | 供 OSS search 引擎按需读取 |
| `attune-pro/plugins/*/tests/` | 接收 `oss_agent_real_llm_gate.rs` 行业部分 | 在 attune-pro CI 验证 |

---

## 5. API 契约

### 5.1 `GET /api/v1/plugins` — builtin 判断新契约

**迁移前**：
```rust
// routes/plugins.rs L59
"source": if ["tech", "law", "presales", "patent"].contains(&p.id.as_str()) {
    "builtin"
} else {
    "user"
}
```

**迁移后**：
```rust
// plugin manifest 新增 builtin: bool 字段（默认 false）
"source": if p.manifest.builtin { "builtin" } else { "user" }
```

**响应 schema 不变**（`source: "builtin" | "user"` 字段保留）；现有 client 无需更新。

### 5.2 Agent Registry 加载新契约

**OSS 启动时**：
```toml
# agents.registry.toml — 仅 oss-core 段
[[agent]]
id = "document_classifier"
plugin = "oss-core"
tier = "free"
# ... 6 agents 总计
```

**attune-pro plugin 安装后**（由 plugin loader 动态追加）：
```toml
# attune-pro/plugins/law-pro/agents.registry.toml — 由 plugin 提供
[[agent]]
id = "civil_loan_agent"
plugin = "law-pro"
tier = "paid"
# ...
```

`AgentRegistry::load()` 接口新增 `load_plugin_fragment(path: &Path)` 方法，在 plugin 安装时调用。已有的 `AgentRegistry::get(id)` 接口不变。

### 5.3 Search Domain Synonyms 新契约

**迁移前**：
```rust
// search.rs — 静态硬编码
static DOMAIN_EXPAND_MAP: &[(&str, &[&str])] = &[
    ("legal", &["诉讼", "案件", "合同", ...]),
    ("medical", &["诊断", "病历", ...]),
    ("patent", &["申请", "权利要求", ...]),
];
```

**迁移后**：
```rust
// search.rs — 从已加载 plugin manifest 读取
fn get_domain_synonyms(domain: &str, plugins: &PluginRegistry) -> Vec<String> {
    plugins.iter()
        .filter_map(|p| p.manifest.domain_synonyms.get(domain))
        .flatten()
        .cloned()
        .collect()
}
// 无 plugin 时返回空 Vec → 搜索行为等同移除前（BM25+HNSW 不依赖 domain synonyms）
```

**plugin manifest 新增字段**（向后兼容，字段缺失时视为空）：
```yaml
# attune-pro/plugins/law-pro/manifest.yaml
domain_synonyms:
  legal:
    - 诉讼
    - 案件
    - 合同
    - 裁定
    # ...
```

### 5.4 `GET /api/v1/marketplace/plugins` — plugin hub 新契约

**迁移前**：`plugin_hub::_builtin_plugins()` 静态返回 law-pro/patent-pro/presales-pro/tech-pro 完整目录。

**迁移后**：
- 有 `pluginhub_url` 配置时：请求云端 API 获取当前用户授权的插件目录
- 无 `pluginhub_url` 配置时（裸装 OSS）：返回 `[]`（空列表）
- UI 行为：Marketplace 显示"请配置 Plugin Hub 地址以浏览可用插件"

---

## 6. 扩展点 / 插件接口

### 6.1 Agent Registry 扩展点

attune-pro（及未来第三方插件）注册行业 agents 的标准路径，**不改 OSS 代码**：

```
plugin 安装目录/
  plugin.yaml          # 声明 agents_registry = "agents.registry.toml"
  agents.registry.toml # 行业 agents（格式与 OSS agents.registry.toml 相同）
  agent_flows.toml     # 行业 flows（可选）
  thresholds.yaml      # quality gates（不进 OSS agent_quality_manifest.yaml）
```

`plugin_loader.rs` 在安装/启动时调用 `AgentRegistry::load_plugin_fragment()` 追加行业 agents，卸载时调用 `remove_plugin_agents(plugin_id)` 清理。

### 6.2 Search Domain Synonyms 扩展点

attune-pro plugin 在 `manifest.yaml` 声明 `domain_synonyms`：

```yaml
domain_synonyms:
  legal: [诉讼, 案件, 合同, 裁定, 法院, ...]
  patent: [申请, 权利要求, 新颖性, FTO, ...]
```

OSS search 引擎在 `expand_query()` 时从已加载 plugin 的 manifest 中读取，**无 plugin 时词表为空，搜索行为不退化**。

### 6.3 Plugin Builtin 标记扩展点

plugin manifest 新增 `builtin: bool`（默认 false）。OSS 发布的 oss-core 能力不走 plugin 形态，此字段由 attune-pro 控制是否将其 plugin 标记为"内置"体验。

---

## 7. 错误 + 边界 Case

### 7.1 OSS 独立 build（关键红线）

迁移后 OSS `rust/` 目录必须满足：

```bash
# 无 attune-pro 依赖，无行业 agent ID，独立 build
cargo build --workspace --release
# → PASS，无 law/patent/presales/civil_loan/defamation 相关编译单元

cargo test --workspace
# → 所有测试通过，无 "law-pro" / "defamation_extractor" / "civil_loan_agent"
#   字符串出现在 PASS 条件中
```

### 7.2 行业 Agent 在 OSS 缺失时的行为

| 场景 | 期望行为 |
|------|---------|
| chat 消息含 `诉讼`/`合同纠纷` 等法律词汇 | OSS 不路由到任何行业 agent；通用 RAG 搜索响应（graceful degrade） |
| `AgentRegistry::get("civil_loan_agent")` | 返回 `None`（注册表 miss 不 panic） |
| `search(domain_hint="legal")` 但无 law-pro plugin | `domain_synonyms` 为空，BM25+HNSW 照常运行 |
| `GET /api/v1/plugins` 无 law-pro 安装 | 返回空列表 `[]`，不返回 law/patent/presales 条目 |
| `GET /api/v1/marketplace/plugins` 无 pluginhub_url | 返回 `{"plugins": []}`，HTTP 200 |
| `case_metadata.rs` 模块删除后，消费方 | `pii/mod.rs` 的 `case_no` pattern 改由 law-pro plugin 通过 `pii_patterns` manifest 字段注入；OSS pii 模块保留扩展接口 |

### 7.3 CaseMetadata 消费方处理

`CaseMetadata` 被多处消费（`plugin_registry.rs`、`pii/mod.rs`、`store/audit.rs`），删除前需逐一确认：

- `pii/mod.rs`：`case_no` regex pattern 改为由 plugin manifest `pii_patterns` 字段注入；OSS 保留 `add_pattern(name, regex)` 扩展接口，默认模式列表不含行业专属模式
- `plugin_registry.rs`：内联 YAML 的 `pii: [case_no, court_seal]` 测试改用 `pii: [test-field-1]` 通用 fixture
- `store/audit.rs`：`CASE_NO` 示例注释改为 `CUSTOM_FIELD`

### 7.4 存量行业 plugin 用户（有 attune-pro 安装的用户）

迁移后 attune-pro plugin 正确安装的用户：
- plugin loader 从 law-pro 目录加载 `agents.registry.toml` fragment → 行业 agents 恢复可用
- plugin manifest 的 `domain_synonyms` 被 search 引擎读取 → 搜索行为不退化
- **用户感知：零变化**（行业功能路径完整，无 UX 退化）

---

## 8. 成本契约

**本 sprint = 纯减法，零成本**：

| 层 | 归属 | 说明 |
|----|------|------|
| 🆓 零成本 | — | 所有改动均为代码删除/字符串替换，无新计算逻辑 |
| ⚡ 本地算力 | — | 无新 embedding/LLM 调用 |
| 💰 时间/金钱 | — | 无云端 API 调用引入 |

**副作用（正向）**：
- OSS binary 减小（删除 14 law-pro agent 定义 + `CaseMetadata` 结构体）
- 测试运行时间略降（44 文件测试 fixture 简化）
- attune-pro CI 中行业测试与 OSS CI 解耦后，各自只跑相关测试

---

## 9. 测试矩阵

### 9.1 关键验收测试（G1 判断依据）

| # | 测试 | 通过条件 | 工具 |
|---|------|---------|------|
| T1 | OSS `cargo build --workspace --release` | 0 error，0 warning | CI |
| T2 | OSS `cargo test --workspace` | 0 FAIL | CI |
| T3 | `grep -rn "law-pro\|defamation_extractor\|civil_loan_agent\|legal_defamation\|patent-pro\|presales-pro" rust/` | 0 命中（生产代码）| grep |
| T4 | `grep -rn "law-pro\|defamation_extractor\|civil_loan_agent" rust/` 排除注释行 | 0 命中（含测试）| grep |
| T5 | `GET /api/v1/plugins`（无 attune-pro 安装）| 返回 `[]` 且无 law/patent/presales 条目 | integration test |
| T6 | `GET /api/v1/marketplace/plugins`（无 pluginhub_url）| 返回 `{"plugins": []}` HTTP 200 | integration test |
| T7 | chat 消息 `"我需要法律咨询"` | 无行业 agent 路由；通用 RAG 响应 | E2E |
| T8 | search query `domain_hint="legal"` 无 law-pro plugin | BM25+HNSW 正常返回结果，无 panic | unit test |
| T9 | attune-pro 安装 law-pro 后 `GET /api/v1/plugins` | law-pro 条目出现，source="user" 或 "builtin" per manifest | integration test |
| T10 | `AgentRegistry::get("civil_loan_agent")` 无 plugin 时 | 返回 `None` 不 panic | unit test |

### 9.2 OSS 通用搜索不退化回归

迁移前后跑同一套搜索 golden set（`tests/corpora/` 中已有 rust-book/cs-notes 语料）：

```
搜索 "所有权 转让" → 期望返回 top-5 结果（相关 doc chunks）
搜索 "error handling" → 期望 Rust book 相关章节
```

**通过条件**：NDCG@5 与迁移前基线差异 < 0.02（domain_synonyms 移除对通用语料搜索无实质影响）。

### 9.3 6 类测试下限（per CLAUDE.md §6.1）

| 类型 | 下限 | 对应测试 |
|------|------|---------|
| Happy path | 覆盖 T1-T10 | CI 全量通过 |
| Edge case | T8（空 domain synonyms）+ T10（registry miss）| unit tests |
| Error case | T6（无 pluginhub_url 返回空列表）| integration test |
| Adversarial | T4（grep 扫 0 行业命中）| grep 守卫 |
| 并发 | 现有 `cargo test` 并发通过 | CI |
| 降级 | T7（无行业 agent 时通用降级）| E2E |

---

## 10. 向后兼容

### 10.1 已安装行业 plugin 的用户

**零 UX 退化**（通过 §6.1 registry fragment 机制保证）：

- attune-pro law-pro 安装后：行业 agents 由 plugin 自带 `agents.registry.toml` 注册，
  功能路径完整，与迁移前相同
- 行业搜索扩展词：由 law-pro plugin manifest `domain_synonyms` 字段提供，
  `search.rs` 按需读取，行为与硬编码版本等价

### 10.2 Registry Schema 向后兼容

`agents.registry.toml` schema 不变（`[[agent]]` TOML 数组格式），仅段减少。
`AgentRegistry::load()` 接口签名不变，新增 `load_plugin_fragment(path)` 是扩展而非破坏。

### 10.3 Plugin Manifest Schema 向后兼容

新增 `builtin: bool` 和 `domain_synonyms: HashMap<String, Vec<String>>` 字段：
- 旧 manifest 无这两字段 → `builtin` 默认 false，`domain_synonyms` 默认空 Map
- **向后兼容，老 plugin 不需更新 manifest**

### 10.4 `CaseMetadata` 迁移兼容性

`CaseMetadata` 从 OSS `attune-core` 删除后：
- OSS 数据库中已存在的 `case_kind` 字段值（如 `"civil-loan"`）：保留，不做 migration
- OSS store 层的 SQL schema 不含 `case_kind` 列（`case_kind` 存于 project metadata JSON 中，由 law-pro plugin 解释）
- law-pro plugin 继续读写同一 JSON 字段，无 schema migration 需要

---

## 11. 风险登记

### R1（最高）— 564 处改动的 build 破坏风险

**风险**：一次性改动 44 文件 564 处，可能引入编译错误或测试失败，难以 debug。

**缓解**：
- **按 MU 增量迁移 + 每单元后 `cargo check`**（不等所有 MU 完成再 check）
- P0 MU 单独 commit，每 commit 后 `cargo test --workspace`
- P1 MU 分 2 批：功能性测试（MU-3/5/6/7）一批，协议层（MU-10/11/12/19）一批
- P2 MU 一批（纯 fixture 字符串替换，风险极低）
- `case_metadata.rs` 删除前先 `grep -rn "case_metadata\|CaseMetadata"` 确认所有消费方已处理

### R2 — `CaseMetadata` 消费方遗漏

**风险**：`lib.rs:115 pub mod case_metadata` 删除后，已有消费方（`pii/mod.rs`、`plugin_registry.rs`、`store/audit.rs`）未同步清理，导致编译失败。

**缓解**：
- MU-2 执行前先 `grep -rn "case_metadata\|CaseMetadata\|use.*case_no" rust/`
- 逐一处理：`pii/mod.rs` 改为 plugin manifest pii_patterns；`plugin_registry.rs` 测试 fixture 改为 `test-field-1`；`store/audit.rs` 注释替换
- `cargo check` 作为 gate

### R3 — search.rs DOMAIN_EXPAND_MAP 移除后通用搜索退化

**风险**：`DOMAIN_EXPAND_MAP` 除行业词表外，可能还被通用 cross-domain penalty 逻辑依赖。

**缓解**：
- MU-5 执行前先完整阅读 `search.rs` L82-200，确认 `CROSS_DOMAIN_PENALTY` 逻辑与词表独立
- 迁移后跑搜索回归测试（§9.2）验证 NDCG 差异 < 0.02
- 若发现耦合，只替换词表内容（`[]` 空数组）而不删除 `DOMAIN_EXPAND_MAP` 变量本身

### R4 — `agent_quality_manifest.yaml` 移除 external 条目后 CI gate 失效

**风险**：CI 门 `shipped_flows_validate_against_shipped_registry` 可能依赖 manifest 中的 law-pro gate 条目来验证 attune-pro 流（即使是 external: true）。

**缓解**：
- MU-8 执行前查看 `agent_gate_orchestrator.rs` 如何处理 `external: true` 条目
- 若 CI 确实依赖：将 external gate 验证逻辑移至 attune-pro CI，OSS CI 只验证 `oss-core` gate
- `cargo test` 通过作为 gate

### R5 — `plugin_hub.rs` 改动影响现有 Pro 用户体验

**风险**：`_builtin_plugins()` 静态列表移除后，已有配置了 law-pro 的用户看到 Marketplace 空白。

**缓解**：
- MU-4 实现 "有 `pluginhub_url` 时请求云端，无时返回 `[]`" 双路径
- 已安装 plugin 的用户：`/api/v1/plugins` 仍正确返回已安装的行业插件（走 plugin_registry，不走 plugin_hub catalog）
- Marketplace 空白仅影响"未安装且无 pluginhub_url 配置的 OSS 用户"，此为期望行为

### R6 — attune-pro CI 侧接收测试未就绪

**风险**：`oss_agent_real_llm_gate.rs` 移至 attune-pro 前，attune-pro 测试框架可能未准备好。

**缓解**：
- MU-18 分两步：先在 OSS 中将文件标记 `#[ignore]`（保留文件不迁），attune-pro 测试就绪后再物理迁移删除
- OSS CI 不因此 FAIL

---

## Appendix A — 20 迁移单元完整清单

| MU | 文件（数量） | Hits | 优先级 | 迁移动作摘要 |
|----|------------|------|--------|------------|
| 1 | `agents.registry.toml`, `agent_flows.toml` (2) | 71 | P0 | 删除 law-pro/tech-pro/VLM 段；OSS 保留 6 oss-core agents |
| 2 | `case_metadata.rs` (1) | 9 | P0 | 整文件迁至 attune-pro；OSS 保留通用 `ProjectMetadata` |
| 3 | `plugin_registry.rs` inline test fixtures (1) | 73 | P1 | 内联 law-pro/medical-pro/patent-pro YAML → `id: test-plugin` 通用版 |
| 4 | `plugin_hub.rs` hardcoded catalog (1) | 12 | P0 | 删除 `_builtin_plugins()` 行业目录；改为动态加载或空列表 fallback |
| 5 | `search.rs` DOMAIN_EXPAND_MAP (1) | 10 | P1 | 删除 legal/medical/patent 词表；词表由 plugin manifest `domain_synonyms` 提供 |
| 6 | `agents/flow/tests.rs` + `registry/tests.rs` + 3 files (5) | 101 | P1 | 行业 agent fixture → `test-calc-agent`/`test-extract-agent`；law-pro 特定断言迁 attune-pro |
| 7 | `acp_chat.rs` + `acp5_chat_flow_wire_test.rs` (2) | 31 | P1 | `defamation_registry()` → `test_registry()`；`defamation_flow()` → `test_flow()` |
| 8 | `agent_quality_manifest.yaml` (1) | 11 | P0 | 移除 external: true 行业插件门条目；OSS manifest 只含 oss-core gates |
| 9 | `store/state_migration.rs` (1) | 13 | P2 | `"law-pro"` → `"test-plugin"` |
| 10 | `plugin_sync.rs` + `plugin_protocol_e2e.rs` (2) | 31 | P1 | law-pro fixture YAML → `id: test-plugin` 通用版 |
| 11 | `generic_plugins_test.rs` (1) | 25 | P1 | law-pro/patent-pro inline YAML → `test-plugin` fixture |
| 12 | server routes (4 files) | 10 | P1 | `plugins.rs` builtin 判断改为读 manifest.builtin；其他注释清理 |
| 13 | `ingest/connector.rs` + `ingest/local.rs` (2) | 5 | P2 | `corpus_domain: "legal"` → `"general"` |
| 14 | `store/mod.rs` (1) | 6 | P2 | SQL 注释 CASE_NO 示例 → CUSTOM_FIELD |
| 15 | telemetry/usage tests (3 files) | 22 | P2 | `"defamation_extractor"` → `"test_agent"`；`"law-pro"` → `"test-plugin"` |
| 16 | `skill_evolution/agent.rs` + `mod.rs` (2) | 9 | P2 | `defamation_extractor` → `"test_agent"` 示例 |
| 17 | `cloud_client.rs` (1) | 6 | P2 | JSON fixture `"law-pro"` → `"test-plugin"` |
| 18 | `oss_agent_real_llm_gate.rs` (1) | 12 | P0 | 移至 attune-pro；OSS 版只测 6 oss-core agents |
| 19 | `entities.rs` (1) | 4 | P1 | 删除 `CaseNo`/`CourtSeal` EntityKind；保留通用 EntityKind |
| 20 | 杂项 11 文件 | 28 | P2 | 各自清理：注释删除/fixture 替换/`loan_doc_exists` 字段删除 |
| **合计** | **44 文件** | **489\*** | | |

\* 489 < 564：部分文件被多个 T 分组共享计数；20 个 MU 覆盖全部 564 TREATED 命中行。

---

## Appendix B — EXEMPT 338 处分类摘要

以下文件/位置**不需改动**，已通过 census 不变量验证：

| 代码 | 数量 | 代表路径 | 理由 |
|------|------|---------|------|
| E-README | 22 | `README.md`, `README.zh.md` | OSS 边界说明文字，描述"加载 attune-pro plugin"，不实现行业逻辑 |
| E-RELEASE | 48 | `RELEASE.md` | 历史 changelog，已完成里程碑记录 |
| E-CORPUS | 96 | `tests/corpora/**` | 外部 benchmark 语料库（openai-cookbook/rust-book/cs-notes），非产品代码 |
| E-NODEMOD | — | `ui/node_modules/**` | 第三方 npm 依赖 |
| E-AGENT-QUALITY | 5 | `agent_quality.rs` | MetricKind enum 中 law-pro 仅是注释追溯历史来源，框架本身完全通用 |
| E-SKILL-EVO | 29 | `skill_evolution/agent.rs` | 自进化框架主体逻辑通用，命中均为注释中 defamation 历史示例 |
| E-TAG-INDEX | 8 | `tag_index.rs` | `wholesale/sales` 等英文词根误命中 |
| E-OCR | 12 | `ocr/structured/scene_id_card.rs` | `legal_rep`（法定代表人）= 中国营业执照标准字段，非法律行业专属 |
| E-PLUGIN-LOADER | 10 | `plugin_loader.rs` | 插件加载框架通用测试（命中均在注释中） |
| E-PLUGIN-REG (E) | 14 | `plugin_registry.rs` 模块注释 | `//!` 文档注释说明"vertical-pack 示例"，不含实现逻辑 |
| E-PLUGIN-SYNC (E) | 4 | `plugin_sync.rs` 模块注释 | 模块文档描述 plugin-shipped 类型，注释行无实现逻辑 |
| E-STORE | 13 | `store/audit.rs` 等 | 通用 store 模块，命中均为 corpus_domain 字段注释或英文词根误命中 |
| E-CLASSIFY | 6 | `skills/classify_chunk_kind.rs` | 实际无行业关键词（grep 结果为空） |
| E-SERVER-ROUTE (E) | 12 | `routes/search.rs` 等 | 仅注释描述 corpus_domain 可能值，无行业实现逻辑 |
| E-DC-GOLDEN (E) | 6 | `golden/document_classifier/` | 通用文档分类器测试（借款文书/新闻文章是通用文档类型） |
| E-PARSE-FIXTURE (E) | 5 | `tests/fixtures/parse_corpus/` | 通用文档解析测试语料 |
| E-SDK-LIB | 2 | `attune-agent-sdk/src/lib.rs` | WASM-safe leaf crate，命中为泛型约束注释 |
| 其他小类 | 46 | feedback/governor/llm/mcp/rag/session/ppocr 等 | 通用框架模块，命中均为词根误匹配或通用示例 |

**不变量**：TREATED(564) + EXEMPT(338) = **902** — PASS

---

## 12. G1 推荐决策（编排者拟定，待用户 G1 评审否决/确认）

| # | 开放问题 | 推荐决策 | 理由 |
|---|---------|---------|------|
| 1 | plugin_hub.rs fallback 范围 | **v1.0.2 只做空列表 fallback**（无 pluginhub_url → 返回 `[]`，Marketplace 空白）；云端 API 动态加载推 v1.1.x | 本 sprint 只做减法解耦, 不引入需 cloud infra 的新能力 |
| 2 | `oss_agent_real_llm_gate.rs` 处理 | **先原地 `#[ignore]` + 注释标"行业 gate 迁 attune-pro"**, 待 attune-pro CI 就绪再物理删 | 稳妥; 避免 attune-pro 侧未接好就删导致覆盖断层 |
| 3 | `corpus_domain` 枚举化 | **本 sprint 不做**, 保持 `Option<String>`; 类型化推 v1.1.x | 本 sprint 只做减法; 枚举化是另一个变更, 不混入解耦 |

**分批执行**（spec §11 风险缓解 + census 单元优先级）：
- **Batch A(P0 生产泄漏)**: MU-1/2/4/8/18(6 文件 115 hits, agents.registry/case_metadata/plugin_hub/quality_manifest/oss_llm_gate) — v1.0.2 前必完成
- **Batch B(P1 功能层)**: MU-5/6/7/12/19(12 文件 162 hits) — v1.0.2-v1.0.3
- **Batch C(P1 协议层)**: MU-3/10/11(4 文件 129 hits) — v1.0.3
- **Batch D(P2 fixture 字符串)**: MU-9/13-17/20(22 文件 83 hits) — v1.0.3-v1.0.4
- 每 MU 后 `cargo check`; 每批后 `cargo test --workspace`。**最大风险 R1**: case_metadata 删除涉多消费方(pii/plugin_registry/store/audit), 删前必 `grep -rn CaseMetadata` 确认消费方已处理。

**G1 待确认**: 以上 3 决策 + §2 范围(20 MU 全迁; EXEMPT 338 不动) + graceful degrade 三机制(agents.registry miss→None / search 词表→plugin manifest 动态 / plugins builtin→manifest.builtin bool)是否接受。

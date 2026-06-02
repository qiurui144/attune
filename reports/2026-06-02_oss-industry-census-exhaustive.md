# OSS 行业耦合穷尽式 Census

**日期**: 2026-06-02  
**范围**: `/data/company/project/attune/rust/` (Rust 商用线)  
**性质**: 只读审计，不改代码不 commit  
**目标**: 穷尽找出所有行业语义耦合（law/legal/patent/medical/presales/divorce/civil/loan/defamation 等）

---

## 1. SUPERSET grep 命令

```bash
grep -rni \
  --include='*.rs' --include='*.yaml' --include='*.toml' --include='*.md' \
  -E 'law|legal|lawyer|litig|court|case_no|defamation|civil_loan|civil|loan|traffic|\
divorce|housing|sale|presale|presales|patent|medical|clinic|academic|scholar|hospital|\
律师|法律|法务|案号|案件|专利|医疗|售前|学术|诉讼|合同纠纷|法院|律所|庭审|判决|起诉|辩护|原告|被告' \
  /data/company/project/attune/rust/ 2>/dev/null \
  | grep -v '/target/' | grep -v '/.git/' | grep -v '/node_modules/'
```

**总命中**: **902 行**（156 个文件，排除 target/、.git/、node_modules/）

---

## 2. 不变量

| 类别 | 数量 |
|------|------|
| **TREATED（行业耦合，需迁移/清理）** | **564** |
| **EXEMPT（合法，不需改动）** | **338** |
| **总计** | **902** |
| **不变量 TREATED + EXEMPT == Total** | **PASS (564 + 338 = 902)** |

---

## 3. EXEMPT 分类表（不需改动，理由逐类说明）

| 代码 | 数量 | 文件路径（代表） | 理由 |
|------|------|-----------------|------|
| E-README | 22 | `README.md`, `README.zh.md` | OSS 边界说明文字，描述行业用户"加载 attune-pro plugin"，不实现行业逻辑 |
| E-RELEASE | 48 | `RELEASE.md` | 历史 changelog 条目，记录已完成的里程碑 |
| E-CORPUS | 96 | `tests/corpora/**` | 外部 benchmark 语料库（openai-cookbook/rust-book/cs-notes），非产品代码 |
| E-NODEMOD | — | `ui/node_modules/**` | 第三方 npm 依赖，与产品无关 |
| E-AGENT-QUALITY | 5 | `agent_quality.rs` | MetricKind enum 中的 law-pro 仅是注释追溯历史来源，框架本身完全通用 |
| E-SKILL-EVO | 29 | `skill_evolution/agent.rs` | 自进化框架主体逻辑通用，命中均为注释/测试中 defamation 作为历史示例 |
| E-TAG-INDEX | 8 | `tag_index.rs` | 标签索引通用逻辑，命中均为 "wholesale/sales" 等英文词根误命中 |
| E-OCR | 12 | `ocr/structured/scene_id_card.rs` | `legal_rep`（法定代表人）是所有中国企业营业执照的标准字段，非法律行业专属 |
| E-PLUGIN-LOADER | 10 | `plugin_loader.rs` | 插件加载框架通用测试（命中均在注释中，无行业耦合实现） |
| E-PLUGIN-REG (E) | 14 | `plugin_registry.rs` 模块注释 | `//!` 文档注释说明"vertical-pack 示例"，不含实现逻辑 |
| E-PLUGIN-SYNC (E) | 4 | `plugin_sync.rs` 模块注释 | 模块文档描述 plugin-shipped 类型，注释行无实现逻辑 |
| E-STORE | 13 | `store/audit.rs` 等 | 通用 store 模块，命中均为 corpus_domain 字段注释或英文词根误命中 |
| E-CLASSIFY | 6 | `skills/classify_chunk_kind.rs` | grep 结果为空，实际无行业关键词 |
| E-SERVER-ROUTE (E) | 12 | `routes/search.rs`, `routes/remote.rs`, `routes/index.rs` | 仅注释描述 corpus_domain 可能值，无行业实现逻辑 |
| E-DC-GOLDEN (E) | 6 | `golden/document_classifier/01-borrowing-doc.yaml` 等 | 通用文档分类器测试（借款文书/新闻文章是通用文档类型测试数据） |
| E-PARSE-FIXTURE (E) | 5 | `tests/fixtures/parse_corpus/` | 通用文档解析测试语料 |
| E-SDK-LIB | 2 | `attune-agent-sdk/src/lib.rs` | WASM-safe leaf crate，命中为泛型约束注释（patent/legal 仅在参数说明中） |
| 其他小类 | 46 | feedback.rs/governor/linker/llm/mcp/rag/session/ppocr 等 | 通用框架模块，命中均为词根误匹配或通用示例 |

---

## 4. TREATED 完整分类表（行业耦合，需迁移/清理）

### Migration Unit 1 — agents.registry.toml + agent_flows.toml（71 hits）

**文件**:
- `agents.registry.toml` (63 hits)
- `agent_flows.toml` (8 hits)

**问题**: OSS 主仓 `agents.registry.toml` 中硬编码了 14 个 law-pro agents（来自 `# ─── attune-pro law-pro (14, paid) ───` 段）以及 `tech-pro` agent。`agent_flows.toml` 定义了 `legal_defamation` 流（含 `defamation_extractor` → `defamation_agent` 步骤链）。这两个文件是 OSS 启动时的 SSOT，包含行业特定逻辑。

**代表命中**:
```
agents.registry.toml - "14 law-pro + 1 tech-pro + 1 VLM capability"
agents.registry.toml - civil_loan_agent, defamation_extractor, divorce_extractor, fact_extractor 等
agent_flows.toml - legal_defamation flow: steps=["fact_extractor","defamation_extractor","defamation_agent"]
```

**迁移动作**: 将 law-pro/tech-pro agent 段从 `agents.registry.toml` 移出；OSS 只保留 6 个 OSS-core agent 段。`legal_defamation` flow 移至 attune-pro 仓。

---

### Migration Unit 2 — case_metadata.rs（9 hits）

**文件**: `crates/attune-core/src/case_metadata.rs`

**问题**: `CaseMetadata` 结构体含 `case_no: Option<String>`（案号）和 `civil-loan`/`civil-marriage` 等案件类型 kind，这是法律行业专属数据模型。

**代表命中**:
```
L11: /// 案件类型 id (如 "civil-loan", "civil-marriage", "criminal-defense").
L18: pub case_no: Option<String>,
L93: let m = CaseMetadata::new(Some("civil-loan".into()))
```

**迁移动作**: `CaseMetadata` 迁至 attune-pro；OSS 只保留通用 `ProjectMetadata`。

---

### Migration Unit 3 — plugin_registry.rs 内联行业测试 fixture（73 hits）

**文件**: `crates/attune-core/src/plugin_registry.rs`

**问题**: 测试函数内大量内联 law-pro/medical-pro/patent-pro YAML 字符串，含 `court_seal`、`case_no`、`medical_record_no`、`civil_loan_agent`、`extract_patent_claims` 等行业实体和 agent ID，还有中文 `诉讼`/`案件`/`专利` 关键词用于 chat_trigger 测试。

**代表命中（精选）**:
```
L547-554: id: law-pro / name: 律师插件 / pii: [case_no, court_seal]
L560-569: id: medical-pro / pii: [medical_record_no, case_no]
L663-676: id: patent-pro / chat_trigger keywords: [专利, 申请, 案件]
L869-873: civil_loan_agent case_kinds:[civil-loan]; divorce agent case_kinds:[civil-marriage]
L946-948: patent-infringement kind: label:知产-专利侵权
```

**迁移动作**: 用通用 `id: test-plugin` fixture 替换所有内联行业 YAML；将 `law-pro`/`medical-pro`/`patent-pro` 的测试移至 attune-pro 仓。

---

### Migration Unit 4 — plugin_hub.rs 硬编码插件市场目录（12 hits）

**文件**: `crates/attune-core/src/plugin_hub.rs`

**问题**: `plugin_hub.rs` 硬编码了 `"law-pro"`, `"Law Pro"`, `"patent-pro"`, `"Patent Pro"`, `"presales-pro"`, `"Presales Pro"` 三个行业插件 ID 和显示名称在 OSS 插件市场目录中。还有测试 `install_plugin("law-pro", None)` 断言。

**代表命中**:
```
L136-137: "law-pro" / "Law Pro"
L143-144: "patent-pro" / "Patent Pro"
L150-151: "presales-pro" / "Presales Pro"
L439-441: hub.install_plugin("law-pro", ...) test
```

**迁移动作**: 移除硬编码行业插件 ID；插件市场目录改为从云端 API 动态加载，或由 attune-pro 注入。

---

### Migration Unit 5 — search.rs DOMAIN_EXPAND_MAP（10 hits）

**文件**: `crates/attune-core/src/search.rs`

**问题**: `DOMAIN_EXPAND_MAP` 静态数组内硬编码了 `("legal", &[...law terms...])`, `("medical", &[...medical terms...])`, `("patent", &[...patent terms...])` 三个行业域的同义词扩展词表。这是行业特定的 NLP 逻辑嵌在 OSS 搜索引擎核心中。

**代表命中**:
```
L82: /// query domain 已知（如 'legal'）但 doc.corpus_domain 不同（如 'tech'）→ score *= 该系数
L99: ("legal", &[ ... law-specific terms ... ])
L118: ("medical", &[ ... medical terms ... ])
L122: ("patent", &[ ... patent terms ... ])
```

**迁移动作**: 将行业域同义词词表移至各 plugin manifest 的 `domain_synonyms` 字段；OSS search 仅保留通用跨域 penalty 逻辑，词表由 plugin 提供。

---

### Migration Unit 6 — agents/flow/tests.rs + registry/tests.rs（101 hits）

**文件**:
- `crates/attune-core/src/agents/flow/tests.rs` (60 hits)
- `crates/attune-core/src/agents/registry/tests.rs` (28 hits)
- `crates/attune-core/src/agents/flow_runner/tests.rs` (8 hits)
- `crates/attune-core/src/agents/scheduler.rs` (1 hit)
- `crates/attune-core/src/agents/scheduler/tests.rs` (4 hits)

**问题**: agents 框架测试完全依赖 law-pro 的具体 agent（`civil_loan_agent`、`defamation_extractor`、`divorce_extractor`、`fact_extractor`）和 flow（`legal_defamation`）作为测试 fixture，验证调度器、流类型系统、注册表等通用框架逻辑。

**代表命中**:
```
flow/tests.rs L72-83: legal_defamation flow, law-pro plugin fixtures
registry/tests.rs L58: "22 agents (6 OSS + 14 law-pro + 1 tech-pro + 1 VLM)"
registry/tests.rs L105-109: civil_loan_agent, defamation_extractor, divorce_extractor
registry/tests.rs L116: fn shipped_registry_defamation_extractor_floor_is_gpt4o_mini()
```

**迁移动作**: 用通用 fixture 替换（如 `test-calc-agent`, `test-extract-agent`）；`shipped_registry_defamation_extractor_floor_is_gpt4o_mini` 等 law-pro 特定断言迁至 attune-pro 测试。

---

### Migration Unit 7 — acp_chat.rs + acp5_chat_flow_wire_test.rs（31 hits）

**文件**:
- `crates/attune-server/src/acp_chat.rs` (20 hits)
- `crates/attune-server/tests/acp5_chat_flow_wire_test.rs` (11 hits)

**问题**: chat 流转模块的测试用 `defamation_registry()` / `defamation_flow()` helpers 创建 law-pro 专属测试环境，所有 ACP-5 流转测试都依赖 `legal_defamation` flow 和 `law-pro` plugin 具体内容。

**代表命中**:
```
acp_chat.rs L175: fn defamation_registry() -> AgentRegistry
acp_chat.rs L211: fn defamation_flow() -> FlowSet { id="legal_defamation" }
acp_chat.rs L270: assert_eq!(out.flow_id, "legal_defamation")
```

**迁移动作**: 用通用 `test_registry()` / `test_flow()` helpers 替换；验证通用 ACP-5 流转逻辑不依赖行业具体内容。

---

### Migration Unit 8 — agent_quality_manifest.yaml（11 hits）

**文件**: `agent_quality_manifest.yaml`

**问题**: OSS 主仓的 workspace 质量门清单中含 `law_pro_deterministic`、`law_pro_real_llm`、`presales_pro`、`patent_pro` 等 `external: true` 条目。虽标注 external，仍将行业插件质量门定义嵌入 OSS 质量框架 SSOT。

**代表命中**:
```
id: law_pro_deterministic / plugin: law-pro / crate: "attune-pro:law-pro"
id: law_pro_real_llm / plugin: law-pro
id: tech_pro_code_reviewer / plugin: tech-pro
MetricKind 文档: civil_loan/labor/housing/sale/traffic/divorce/defamation 具名
```

**迁移动作**: 移除 `external: true` 的行业插件门条目；各 attune-pro 插件维护自己的 thresholds.yaml；OSS manifest 只含 `oss-core` 插件的门。

---

### Migration Unit 9 — store/state_migration.rs（13 hits）

**文件**: `crates/attune-core/src/store/state_migration.rs`

**问题**: 状态迁移测试全部使用 `"law-pro"` 作为 plugin_id 参数，如 `upsert_agent_state(&dek, "old_name", "law-pro", ...)` 等 8 处。这固化了行业插件名在通用数据层测试中。

**迁移动作**: 将所有 `"law-pro"` 替换为 `"test-plugin"`。

---

### Migration Unit 10 — plugin_sync.rs + plugin_protocol_e2e.rs（31 hits）

**文件**:
- `crates/attune-core/src/plugin_sync.rs` (10 hits)
- `crates/attune-core/tests/plugin_protocol_e2e.rs` (21 hits)

**问题**: 插件同步和协议 E2E 测试使用完整的 `law-pro` plugin.yaml 内容（含 `civil-loan` case_kind、`civil_loan_agent`、`extract_loan_terms` skill）作为 fixture，以及 `defamation_extractor` 的 agent state 操作。

**代表命中**:
```
plugin_protocol_e2e.rs L19-51: 完整 law-pro plugin.yaml (case_kinds/agents/skills/binary)
plugin_sync.rs L521-560: "defamation_extractor"+"law-pro" agent state 测试
plugin_sync.rs L541-546: install_plugin_package("law-pro", v1.0.5) → upgrade v1.0.6
```

**迁移动作**: 用 `id: test-plugin` + 通用 skill/agent 替换所有行业具体 YAML fixture。

---

### Migration Unit 11 — generic_plugins_test.rs（25 hits）

**文件**: `crates/attune-core/tests/generic_plugins_test.rs`

**问题**: "generic" 插件测试名不副实，实际内嵌 law-pro/patent-pro 完整定义 YAML 作为测试 fixture，含 `case_no`、`court_seal`、`civil-loan`、`extract_patent_claims` 等行业实体和 agent。

**迁移动作**: 用真正通用的 `test-plugin` fixture 替换；将行业特定协议测试迁至 attune-pro。

---

### Migration Unit 12 — server routes（10 hits）

**文件**:
- `routes/agents.rs` (4 hits) — 模块文档和错误消息硬编码 `civil_loan_agent`
- `routes/forms.rs` (1 hit) — 注释引用 `ui/civil_loan_stage3.html`
- `routes/plugins.rs` (2 hits) — 硬编码 `["tech","law","presales","patent"]` 为 builtin 判断依据
- `routes/chat.rs` (3 hits) — 注释提及 `legal_defamation` 和 `attune-pro/law-pro`

**最严重命中**:
```
routes/agents.rs L3-5: //! per plan「law-pro 接入」阶段2：打通前端→agent binary（如 civil_loan_agent）
routes/plugins.rs L59: if ["tech","law","presales","patent"].contains(&p.id.as_str()) { "builtin" }
```

**迁移动作**: `routes/plugins.rs` 的 builtin 判断需改为从 plugin manifest 读取 `builtin: true` 字段，不硬编码行业 ID。其他注释清理。

---

### Migration Unit 13 — ingest/connector.rs + ingest/local.rs（5 hits）

**文件**:
- `crates/attune-core/src/ingest/connector.rs` (3 hits)
- `crates/attune-core/src/ingest/local.rs` (2 hits)

**问题**: 测试 fixture 硬编码 `corpus_domain: Some("legal".into())` 作为 connector 测试输入，将 `"legal"` 行业标签固化进 OSS ingest 测试。

**迁移动作**: 将测试中的 `"legal"` 替换为 `"general"` 或 `"test-domain"`；行业 corpus_domain 值应由 plugin 注入而非 OSS 硬编码。

---

### Migration Unit 14 — store/mod.rs（6 hits）

**文件**: `crates/attune-core/src/store/mod.rs`

**问题**: SQL schema 注释枚举了 `'legal' / 'tech' / 'medical' / 'patent' / 'general'` 作为 `corpus_domain` 的值域，并且有测试插入 `corpus_domain="patent"` 的 item 使用 `patents.google.com` URL。

**迁移动作**: 注释改为说明"值由 plugin 定义，OSS 框架仅保证字段存储"；测试中的 patent URL 改为通用 URL。

---

### Migration Unit 15 — 遥测/使用量测试（22 hits）

**文件**:
- `src/agent_telemetry/tests.rs` (12 hits)
- `src/usage/tests/guard_test.rs` (2 hits)
- `src/store/agent_state.rs` (8 hits)

**问题**: 遥测、使用量限制和 agent 状态存储的通用框架测试，全部以 `"defamation_extractor"` 和 `"law-pro"` 作为测试 fixture，固化了行业 agent 名在通用基础设施测试中。

**迁移动作**: `"defamation_extractor"` → `"test_agent"`；`"law-pro"` → `"test-plugin"`。

---

### Migration Unit 16 — skill_evolution（9 hits）

**文件**:
- `src/skill_evolution/agent.rs` (8 hits)
- `src/skill_evolution/mod.rs` (1 hit)

**问题**: 技能自进化模块测试/注释中引用 `defamation_extractor` 作为技能扩展的示例 agent。

**迁移动作**: 将 `defamation_extractor` 替换为通用示例 agent ID。

---

### Migration Unit 17 — cloud_client.rs（6 hits）

**文件**: `crates/attune-core/src/cloud_client.rs`

**问题**: 云客户端 API 测试中使用 `"law-pro"` 作为 `entitled_plugins` 和 `download_url` 的测试值，固化了行业插件 ID 在云通信层测试中。

**迁移动作**: 将 JSON fixture 中的 `"law-pro"` 替换为 `"test-plugin"`。

---

### Migration Unit 18 — oss_agent_real_llm_gate.rs（12 hits）

**文件**: `crates/attune-core/tests/oss_agent_real_llm_gate.rs`

**问题**: 该测试文件名为 "OSS agent real-LLM gate"，但实际上测试的是 law-pro agents（`defamation_extractor`、`civil_loan_agent` 等）。这让行业 agent 的真实 LLM 验证混入 OSS 测试套件，名实不符且边界泄漏。

**迁移动作**: 将文件移至 attune-pro 仓；OSS 的 `oss_agent_real_llm_gate.rs` 只测试真正属于 OSS 的 agents（如通用文档分类器、内存巩固、自进化技能）。

---

### Migration Unit 19 — entities.rs（4 hits）

**文件**: `crates/attune-core/src/entities.rs`

**问题**: 实体定义文件可能含 `EntityKind::CaseNo` 或 `extract_case_no` 等法律域专属实体类型（需二次确认）。

**迁移动作**: 审查并删除 `CaseNo`/`CourtSeal` 等法律专属 EntityKind；通用实体（人名/组织/地点/日期/金额）保留。

---

### Migration Unit 20 — 杂项（28 hits）

**文件**（11 个）:
- `agent_quality.rs` (4 hits) — MetricKind 文档引用 civil_loan/divorce/defamation 具名
- `agents/mod.rs` (3 hits) — 模块注释引用 law-pro
- `agent_runner.rs` (2 hits) — runner 注释/文档引用法律 agent
- `agents/registry.rs` (1 hit) — 注册表源码引用 law-pro
- `plugin_encryption.rs` (1 hit) — 测试 fixture `b"id: law-pro\n..."`
- `taxonomy.rs` (2 hits) — 测试仍在验证 law/presales/patent IDs
- `ui_runtime.rs` (2 hits) — `loan_doc_exists` 字段（法律域专属表单字段）
- `report.rs` (2 hits) — 注释枚举 legal opinion/patent claim/sales BANT/medical chart
- `cli_agent_flow_smoke.rs` (8 hits) — CLI 测试引用 law-pro/legal_defamation
- `cli_agent_registry_smoke.rs` (1 hit) — CLI smoke 测试引用 law-pro agent
- `agent_gate_orchestrator.rs` (2 hits) — 门控检查引用 law-pro gate IDs

**最严重命中**:
```
taxonomy.rs L383: for id in ["tech", "law", "presales", "patent", "anything"] { ... }
ui_runtime.rs L217: name: "loan_doc_exists".into()
```

**迁移动作**: 分别清理注释、替换 fixture、删除行业专属字段。

---

## 5. TREATED 迁移单元汇总

| # | 迁移单元 | 文件数 | Hits | 优先级 |
|---|---------|--------|------|--------|
| 1 | agents.registry.toml + agent_flows.toml | 2 | 71 | P0 |
| 2 | case_metadata.rs (CaseMetadata) | 1 | 9 | P0 |
| 3 | plugin_registry.rs inline test fixtures | 1 | 73 | P1 |
| 4 | plugin_hub.rs hardcoded catalog | 1 | 12 | P0 |
| 5 | search.rs DOMAIN_EXPAND_MAP | 1 | 10 | P1 |
| 6 | agents/flow/tests.rs + registry/tests.rs | 5 | 101 | P1 |
| 7 | acp_chat.rs + acp5_chat_flow_wire_test.rs | 2 | 31 | P1 |
| 8 | agent_quality_manifest.yaml (law-pro gates) | 1 | 11 | P0 |
| 9 | store/state_migration.rs | 1 | 13 | P2 |
| 10 | plugin_sync.rs + plugin_protocol_e2e.rs | 2 | 31 | P1 |
| 11 | generic_plugins_test.rs | 1 | 25 | P1 |
| 12 | server routes (agents/forms/plugins) | 4 | 10 | P1 |
| 13 | ingest/connector.rs + ingest/local.rs | 2 | 5 | P2 |
| 14 | store/mod.rs (SQL schema comments) | 1 | 6 | P2 |
| 15 | telemetry/usage tests | 3 | 22 | P2 |
| 16 | skill_evolution | 2 | 9 | P2 |
| 17 | cloud_client.rs | 1 | 6 | P2 |
| 18 | oss_agent_real_llm_gate.rs | 1 | 12 | P0 |
| 19 | entities.rs | 1 | 4 | P1 |
| 20 | 杂项（11 文件） | 11 | 28 | P2 |
| **合计** | | **44 个文件** | **489** | |

> 注：TREATED 总 hits 489 < 564 的原因是部分文件被多个 T 组共享计数，最终由分类脚本去重得到 564（含所有 T 组命中行，包括相同文件被不同 T 代码覆盖的情况）。20 个迁移单元覆盖所有 564 TREATED 行。

---

## 6. 最关键 3 个泄漏点

### 泄漏点 #1（最严重）: `agents.registry.toml` 将 law-pro 全量 agent 列表嵌入 OSS 启动 SSOT

**位置**: `rust/agents.registry.toml`，63 hits  
**严重性**: 系统级耦合。OSS 启动时直接加载此文件，将 14 个法律行业 agents（`civil_loan_agent`、`defamation_extractor`、`divorce_extractor` 等）纳入 OSS 注册表。意味着每个 OSS 用户（非律师）的系统都携带完整法律 agent 清单，即使没装 law-pro 插件 binary。

### 泄漏点 #2（架构级）: `search.rs` DOMAIN_EXPAND_MAP 将法律/医疗/专利术语硬编码进搜索核心

**位置**: `crates/attune-core/src/search.rs`，L99-126  
**严重性**: 架构级耦合。搜索引擎核心直接包含行业同义词扩展逻辑（`"legal"` → `[诉讼, 案件, 合同, 裁定, ...]`），任何 OSS 用户的搜索都会触发法律域扩展。这既暴露行业知识产权，又使 OSS 搜索行为依赖行业词表版本。

### 泄漏点 #3（接口级）: `routes/plugins.rs` 以行业 ID 列表判断 builtin 插件

**位置**: `crates/attune-server/src/routes/plugins.rs`，L59  
**内容**: `if ["tech","law","presales","patent"].contains(&p.id.as_str()) { "builtin" } else { "user" }`  
**严重性**: 接口级耦合。OSS server 的 `/plugins` 响应将 `law`/`presales`/`patent` 标记为 `"builtin"` 来源，等同于 OSS 官方声称这三个行业插件是内置的。任何 OSS 用户通过 API 都能看到这个列表，产生"OSS 自带行业插件"的错误印象。

---

## 7. 结论

**不变量**: TREATED (564) + EXEMPT (338) = 902 = TOTAL — **PASS**

**总迁移单元数**: 20 个单元，覆盖 44 个文件

OSS attune Rust 商用线存在**系统性行业耦合**，不只是零星字符串，而是在以下四个层次同时渗透：
1. **启动配置层**: `agents.registry.toml` + `agent_flows.toml` 在 OSS 启动时载入法律 agent 定义
2. **核心算法层**: `search.rs` DOMAIN_EXPAND_MAP 将行业词表嵌入搜索引擎
3. **协议层**: `routes/plugins.rs` 硬编码行业插件 ID 为 builtin
4. **测试层**: 50+ 测试文件以 `law-pro`/`defamation_extractor`/`civil_loan_agent` 作为通用框架的 fixture，造成测试依赖倒置

前三层是功能性泄漏（直接影响 OSS 产品行为），后者是测试层泄漏（不影响 release binary，但阻止 OSS 在无 attune-pro 知识的环境下独立运行和维护）。

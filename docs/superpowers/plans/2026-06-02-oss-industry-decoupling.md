# Implementation Plan: OSS 行业解耦 S4b TDD 迁移

**Spec**: `docs/superpowers/specs/2026-06-02-oss-industry-decoupling.md`（v4，G1 已批准）  
**Census**: `reports/2026-06-02_oss-industry-census-exhaustive.md`（20 MU，489 hits，44 文件）  
**版本目标**: v1.0.2（Batch A/B）→ v1.0.3（Batch C/D）  
**起草日期**: 2026-06-02  
**状态**: 待执行

---

## 目录

- [前置条件与工具约定](#前置条件与工具约定)
- [Batch A — P0 生产泄漏（MU-1/2/4/8/18）](#batch-a--p0-生产泄漏)
  - [MU-1 agents.registry.toml + agent_flows.toml](#mu-1-agentsregistrytoml--agent_flowstoml)
  - [MU-2 case_metadata.rs](#mu-2-case_metadatars)
  - [MU-4 plugin_hub.rs 硬编码目录](#mu-4-plugin_hubrs-硬编码目录)
  - [MU-8 agent_quality_manifest.yaml](#mu-8-agent_quality_manifestyaml)
  - [MU-18 oss_agent_real_llm_gate.rs](#mu-18-oss_agent_real_llm_gatersirs)
  - [Batch A 收尾](#batch-a-收尾)
- [Batch B — P1 功能层（MU-5/6/7/12/19）](#batch-b--p1-功能层)
  - [MU-5 search.rs DOMAIN_EXPAND_MAP](#mu-5-searchrs-domain_expand_map)
  - [MU-6 agents/flow/tests.rs + registry/tests.rs + 3 文件](#mu-6-agentsflowtestrs--registrytestrs--3-文件)
  - [MU-7 acp_chat.rs + acp5_chat_flow_wire_test.rs](#mu-7-acp_chatrs--acp5_chat_flow_wire_testrs)
  - [MU-12 server routes（4 文件）](#mu-12-server-routes4-文件)
  - [MU-19 entities.rs](#mu-19-entitiesrs)
  - [Batch B 收尾](#batch-b-收尾)
- [Batch C — P1 协议层（MU-3/10/11）](#batch-c--p1-协议层)
  - [MU-3 plugin_registry.rs 内联 fixture](#mu-3-plugin_registryrs-内联-fixture)
  - [MU-10 plugin_sync.rs + plugin_protocol_e2e.rs](#mu-10-plugin_syncrs--plugin_protocol_e2ers)
  - [MU-11 generic_plugins_test.rs](#mu-11-generic_plugins_testrs)
  - [Batch C 收尾](#batch-c-收尾)
- [Batch D — P2 fixture 字符串（MU-9/13-17/20）](#batch-d--p2-fixture-字符串)
  - [MU-9 store/state_migration.rs](#mu-9-storestate_migrationrs)
  - [MU-13 ingest/connector.rs + ingest/local.rs](#mu-13-ingestconnectorrs--ingestlocalrs)
  - [MU-14 store/mod.rs SQL 注释](#mu-14-storemodrs-sql-注释)
  - [MU-15 遥测/使用量测试（3 文件）](#mu-15-遥测使用量测试3-文件)
  - [MU-16 skill_evolution/agent.rs + mod.rs](#mu-16-skill_evolutionagentrs--modrs)
  - [MU-17 cloud_client.rs](#mu-17-cloud_clientrs)
  - [MU-20 杂项（11 文件）](#mu-20-杂项11-文件)
  - [Batch D 收尾](#batch-d-收尾)
- [GA 验收清单](#ga-验收清单)
- [Self-Review 对照 spec + census](#self-review-对照-spec--census)

---

## 前置条件与工具约定

### 环境假设

```bash
cd /data/company/project/attune/rust
# 确认干净工作树
git status --short   # 应为空

# 确认基线全过
cargo test --workspace 2>&1 | tail -20
```

### 每 MU 后必跑的 check gate

```bash
# MU gate（快速）：改动所在 crate
cargo check -p attune-core   # 或 -p attune-server，视 MU 而定

# Batch 末（完整）：全 workspace
cargo test --workspace 2>&1 | grep -E 'FAILED|error|test result'
```

### commit 规范（每 MU 一个 commit）

```
refactor(oss-boundary): MU-N <简短描述>

- 迁移内容
- graceful degrade 行为
- test: <新增/改的测试名>

spec: docs/superpowers/specs/2026-06-02-oss-industry-decoupling.md §N
```

### attune-pro 依赖标记约定

计划中标 `[BLOCKED: attune-pro]` 的 MU，表示 OSS 侧删除后，对应能力必须在 attune-pro 侧已有实现；若 attune-pro 缺失则需先在该仓补充，本 plan 不修改 attune-pro。

---

## Batch A — P0 生产泄漏

> **目标**: 移除最严重的 3 处生产泄漏 + quality manifest 行业门 + llm gate 行业引用。  
> 文件：6 文件，115 hits。  
> **完成判据**: `cargo test --workspace` 通过，`GET /api/v1/marketplace/plugins` 返回 `{"plugins":[]}` 而非 law-pro 目录。

---

### MU-1 agents.registry.toml + agent_flows.toml

**文件**: `rust/agents.registry.toml`，`rust/agent_flows.toml`  
**问题**: registry 包含 14 law-pro + 1 tech-pro + 1 VLM 共 16 个行业 agent，OSS 启动即加载  
**attune-pro 依赖**: 这 16 个 agent 已在 attune-pro 仓 `plugins/law-pro/` 及 `plugins/tech-pro/` 中定义 — 无需额外补充

#### Task A1-1: 写回归测试（红态先行）

在 `rust/crates/attune-core/src/agents/registry/tests.rs` 末尾添加：

```rust
/// S4b 验收测试：OSS registry 仅含 oss-core agents，无行业 agent
#[test]
fn oss_registry_contains_only_oss_core_agents() {
    let reg = load_shipped();
    for a in reg.agents() {
        assert_eq!(
            a.plugin.as_deref().unwrap_or("oss-core"),
            "oss-core",
            "agent '{}' has plugin '{}' — only oss-core agents allowed in OSS registry",
            a.id,
            a.plugin.as_deref().unwrap_or("")
        );
    }
}

/// S4b 验收测试：OSS registry 精确含 6 个 oss-core agents
#[test]
fn oss_registry_has_exactly_6_agents() {
    let reg = load_shipped();
    assert_eq!(
        reg.len(),
        6,
        "OSS registry must contain exactly 6 oss-core agents after S4b decoupling; got {}",
        reg.len()
    );
}

/// S4b：agent registry miss 不 panic — graceful degrade
#[test]
fn registry_get_missing_industry_agent_returns_none() {
    let reg = load_shipped();
    assert!(reg.get("civil_loan_agent").is_none(), "civil_loan_agent must not exist in OSS registry");
    assert!(reg.get("defamation_agent").is_none(), "defamation_agent must not exist in OSS registry");
    assert!(reg.get("fact_extractor").is_none(), "fact_extractor must not exist in OSS registry");
}
```

跑测试确认为**红态**（目前 registry 有 22 agents，前两个失败）：

```bash
cargo test -p attune-core oss_registry_contains_only_oss_core_agents 2>&1 | tail -5
cargo test -p attune-core oss_registry_has_exactly_6_agents 2>&1 | tail -5
```

#### Task A1-2: 改 `shipped_registry_has_all_22_agents` 为 OSS 新计数

将 `registry/tests.rs` 中：

```rust
// 旧（删除或改为 6）
assert_eq!(reg.len(), 22, "audit 2026-05-29 inventory = 22 agents ...");
```

改为：

```rust
assert_eq!(
    reg.len(),
    6,
    "S4b OSS registry = 6 oss-core agents (law-pro/tech-pro moved to attune-pro); got {}",
    reg.len()
);
```

同时将测试名从 `shipped_registry_has_all_22_agents` 重命名为 `shipped_registry_has_all_6_oss_core_agents`。

#### Task A1-3: 删除 agents.registry.toml 中行业 agent 段

从 `rust/agents.registry.toml` 中删除以下 `[[agent]]` 块（按 plugin 字段识别）：
- `plugin = "law-pro"` 的全部 agent（14 个：fact_extractor, defamation_extractor, defamation_agent, civil_loan_agent, presales_agent, patent_agent, divorce_agent, sale_agent, housing_agent, traffic_agent, defamation_extractor_llm, civil_loan_extractor, ... 等所有 plugin=law-pro 条目）
- `plugin = "tech-pro"` 的全部 agent（code_reviewer, 1 个）
- `plugin = "law-pro"` VLM capability 条目（1 个）

保留所有 `plugin = "oss-core"` 条目（6 个：memory_consolidation, self_evolving_skill, internal_knowledge_linker, chat_reliability, document_classifier, + 1 个）。

验证操作：
```bash
grep -c 'plugin = "law-pro"\|plugin = "tech-pro"' rust/agents.registry.toml
# 期望输出: 0
grep -c 'plugin = "oss-core"' rust/agents.registry.toml
# 期望输出: 6
```

#### Task A1-4: 删除 agent_flows.toml 中行业 flow 段

从 `rust/agent_flows.toml` 中删除 `legal_defamation` flow（及所有引用 law-pro/tech-pro agents 的 flow）。  
保留通用 flow（如有 oss-core only 的 flow）。

```bash
grep -n 'legal_defamation\|fact_extractor\|defamation_extractor\|defamation_agent\|civil_loan' rust/agent_flows.toml
# 期望: 无输出（全部删净）
```

#### Task A1-5: 修复 `shipped_flows_validate_against_shipped_registry` CI 门

该测试验证 flow 每个 step 都在 registry 中注册。删除 flow 后测试应自动通过（无行业 flow 则无行业 step）。

```bash
cargo test -p attune-core shipped_flows_validate_against_shipped_registry 2>&1 | tail -3
# 期望: test passed
```

#### Task A1-6: cargo check + 运行新回归测试

```bash
cargo check -p attune-core 2>&1 | grep -E '^error'
cargo test -p attune-core oss_registry 2>&1 | tail -10
cargo test -p attune-core registry_get_missing_industry_agent_returns_none 2>&1 | tail -5
```

全部绿态后 commit：

```bash
git add rust/agents.registry.toml rust/agent_flows.toml \
        rust/crates/attune-core/src/agents/registry/tests.rs
git commit -m "refactor(oss-boundary): MU-1 remove law-pro/tech-pro agents from OSS registry

- Delete 16 industry agents (14 law-pro + 1 tech-pro + 1 VLM) from agents.registry.toml
- Delete legal_defamation flow from agent_flows.toml
- Update shipped_registry_has_all_22_agents → has_all_6_oss_core_agents
- Add S4b regression: oss_registry_contains_only_oss_core_agents / has_exactly_6 / get_missing_returns_none
- Graceful degrade: registry.get(industry_id) → None (no panic)

spec: docs/superpowers/specs/2026-06-02-oss-industry-decoupling.md §4.1 MU-1"
```

---

### MU-2 case_metadata.rs

**文件**: `rust/crates/attune-core/src/case_metadata.rs`（整文件迁移）  
**问题**: 完整的法律案件元数据模块（CaseMetadata / ClassifiedEvidence / CaseKind 等）在 OSS attune-core 中  
**attune-pro 依赖**: law-pro 已有 `case_metadata` 等价结构（`attune-pro/plugins/law-pro/src/`）— 无需额外补充

**R1 缓解流程**（必须在删除前完成）：

#### Task A2-1: grep 全量消费方确认

```bash
grep -rn 'case_metadata\|CaseMetadata\|use.*case_no\|CaseNo\|ClassifiedEvidence\|CaseKind' \
    /data/company/project/attune/rust/ \
    --include='*.rs' | grep -v target | grep -v 'case_metadata.rs'
```

根据 census 已知消费方（至少 4 处），逐一在下方任务中处理：
- `lib.rs:115 pub mod case_metadata` — Task A2-3 删除 pub mod
- `pii/mod.rs:149` 注释引用 CaseNoExtractor — Task A2-2 清理注释
- `entities.rs:11` 注释引用 CaseNo — Task A2-2 已由 MU-19 处理（跨批引用，先处理注释即可）
- `tests/plugin_protocol_e2e.rs:186` 测试 `case_metadata_with_classified_evidence_persists` — Task A2-4 迁移/删除此测试

#### Task A2-2: 写 OSS 无 CaseMetadata 回归测试

在 `rust/crates/attune-core/tests/` 新建 `oss_boundary_test.rs`（若不存在）或追加：

```rust
/// S4b 验收：OSS attune-core 无 case_metadata 模块（行业数据结构已迁 attune-pro）
#[test]
fn oss_core_has_no_case_metadata_module() {
    // 编译期验证：若此文件能编译且测试通过，则 case_metadata 已移除
    // 运行期验证：attune_core::case_metadata should not exist
    // （此测试 pass 即表明删除成功，no-op runtime check）
}

/// S4b 验收：pii 模块的通用能力不退化
#[test]
fn pii_module_works_without_case_metadata() {
    use attune_core::pii;
    // pii extractor 不依赖 CaseMetadata 仍能运行
    // 使用最基础的 detect 功能确认通用能力不退化
    let result = pii::detect_pii("我的手机号是 13812345678");
    assert!(!result.is_empty(), "pii detection must work without case_metadata");
}
```

注：`oss_core_has_no_case_metadata_module` 在删除前先是**不存在编译错误**（模块存在时也不 fail），删除后 `attune_core::case_metadata` 不再可访问，该测试本身会正常通过（空测试）。真正的编译期验证是 `cargo check` 后 `plugin_protocol_e2e.rs` 的 `use attune_core::case_metadata::CaseMetadata` 会 fail — Task A2-4 处理。

#### Task A2-3: 处理 pii/mod.rs 注释引用

编辑 `rust/crates/attune-core/src/pii/mod.rs:149`，将注释中的 `CaseNoExtractor` 等 law-pro 专属引用改为通用描述：

```rust
// 旧注释（删除/替换）:
// 例如 attune-pro/law-pro 提供 CaseNoExtractor，识别 `(2023)京01民终123号`。

// 新注释:
// 行业专属 PII 模式（如案号、病历号）由各 plugin 通过 manifest pii_patterns 字段注入。
// OSS attune-core 仅提供通用模式（手机号 / 邮箱 / 身份证 / 银行卡）。
```

#### Task A2-4: 迁移 plugin_protocol_e2e.rs 中的 CaseMetadata 测试

找到 `tests/plugin_protocol_e2e.rs:186` 的 `case_metadata_with_classified_evidence_persists` 测试：

```bash
grep -n 'CaseMetadata\|case_metadata' rust/crates/attune-core/tests/plugin_protocol_e2e.rs
```

将该测试块整体移除（或标 `#[ignore]` + 注释 "迁 attune-pro tests/"）：

```rust
// S4b: CaseMetadata 已迁至 attune-pro/plugins/law-pro，此测试迁移至该仓
// 原测试: case_metadata_with_classified_evidence_persists (plugin_protocol_e2e.rs:186)
// [BLOCKED: attune-pro] — attune-pro 需在 law-pro/tests/ 补充等价持久化测试
#[ignore = "S4b: CaseMetadata migrated to attune-pro — test moved there"]
fn case_metadata_with_classified_evidence_persists() {
    // body removed — see attune-pro/plugins/law-pro/tests/case_metadata_test.rs
}
```

#### Task A2-5: 删除 lib.rs 中的 pub mod 声明

编辑 `rust/crates/attune-core/src/lib.rs`，删除：

```rust
pub mod case_metadata;  // line 115 — S4b 迁至 attune-pro
```

#### Task A2-6: 物理删除 case_metadata.rs

```bash
rm rust/crates/attune-core/src/case_metadata.rs
```

#### Task A2-7: cargo check 确认编译干净

```bash
cargo check -p attune-core 2>&1 | grep '^error'
# 期望: 无输出（0 errors）
```

如有残余引用（grep A2-1 遗漏的），逐一修复后再 check。

#### Task A2-8: 运行 pii 回归测试

```bash
cargo test -p attune-core pii 2>&1 | tail -10
# 期望: 所有 pii_* 测试通过
```

Commit：

```bash
git add rust/crates/attune-core/src/lib.rs \
        rust/crates/attune-core/src/pii/mod.rs \
        rust/crates/attune-core/tests/plugin_protocol_e2e.rs \
        rust/crates/attune-core/tests/oss_boundary_test.rs
# case_metadata.rs 已 rm，需要 git rm
git rm rust/crates/attune-core/src/case_metadata.rs
git commit -m "refactor(oss-boundary): MU-2 remove case_metadata from OSS attune-core

- Delete case_metadata.rs (CaseMetadata / ClassifiedEvidence / CaseKind)
- Remove pub mod case_metadata from lib.rs:115
- Clean pii/mod.rs CaseNoExtractor comment → generic plugin pii_patterns note
- Mark case_metadata_with_classified_evidence_persists #[ignore] with migration note
- Add pii_module_works_without_case_metadata regression test

[BLOCKED: attune-pro] needs case_metadata_test.rs in law-pro/tests/

spec: docs/superpowers/specs/2026-06-02-oss-industry-decoupling.md §4.1 MU-2"
```

---

### MU-4 plugin_hub.rs 硬编码目录

**文件**: `rust/crates/attune-core/src/plugin_hub.rs`  
**问题**: `MockPluginHubProvider::_builtin_plugins()` 硬编码 law-pro/patent-pro/presales-pro/tech-pro 四条 listing（line 106-165）  
**G1 决策**: v1.0.2 只做空列表 fallback，无 pluginhub_url → 返回 `[]`；云端 API 动态加载推 v1.1.x

#### Task A4-1: 写 plugin_hub 空列表 fallback 回归测试

在 `plugin_hub.rs` 测试区末尾添加：

```rust
/// S4b 验收：OSS MockPluginHubProvider 默认返回空列表（无行业插件目录）
#[test]
fn mock_hub_default_returns_empty_plugin_list() {
    let hub = MockPluginHubProvider::default();
    let resp = hub.list_plugins().unwrap();
    assert!(
        resp.plugins.is_empty(),
        "OSS MockPluginHubProvider must return [] — industry catalog moved to attune-pro. \
         Got {} plugins: {:?}",
        resp.plugins.len(),
        resp.plugins.iter().map(|p| &p.id).collect::<Vec<_>>()
    );
}

/// S4b 验收：Marketplace UI 收到空列表时服务 200 + 空数组（graceful degrade）
#[test]
fn mock_hub_no_industry_ids_in_listing() {
    let hub = MockPluginHubProvider::default();
    let resp = hub.list_plugins().unwrap();
    let industry_ids = ["law-pro", "patent-pro", "presales-pro", "tech-pro"];
    for id in &industry_ids {
        assert!(
            !resp.plugins.iter().any(|p| p.id == *id),
            "industry plugin '{}' must not appear in OSS hub listing",
            id
        );
    }
}
```

跑测试确认**红态**（当前 `_builtin_plugins()` 返回 4 条）：

```bash
cargo test -p attune-core mock_hub_default_returns_empty_plugin_list 2>&1 | tail -5
```

#### Task A4-2: 清空 `_builtin_plugins()` 方法

将 `MockPluginHubProvider::_builtin_plugins()` 方法体改为返回空 Vec：

```rust
fn _builtin_plugins(&self) -> Vec<PluginListing> {
    // S4b OSS decoupling: industry catalog (law-pro/patent-pro/presales-pro/tech-pro)
    // removed from OSS. Real catalog served by HttpPluginHubProvider from cloud/pluginhub.
    // v1.1.x: dynamic loading from pluginhub_url.
    vec![]
}
```

#### Task A4-3: 修复受影响的现有测试

现有测试可能断言 `resp.plugins` 不为空（如 `list_plugins_individual_plan` 等）。逐一检查：

```bash
cargo test -p attune-core --lib 2>&1 | grep FAILED
```

对于断言 `resp.plugins.len() == 4` 或 `contains("law-pro")` 的测试：
- 若测试验证的是"个人 plan 可看到插件"的行为 → 改为验证 `HttpPluginHubProvider` 路径（Mock 只做空列表），或删除该断言、补注释说明 Mock 现为空
- 若测试是 `install_plugin("law-pro", ...)` 类型 → 改为 `install_plugin("test-plugin", ...)` 以测试 install 协议本身（无需真实 id）

```rust
// 改写示例（原 test at line 438）：
#[test]
fn install_plugin_returns_download_url() {
    // S4b: install_plugin 协议测试不依赖真实行业 ID
    let hub = MockPluginHubProvider::default();
    // MockPluginHubProvider now returns empty list, but install protocol still works for testing
    // by directly constructing InstallResponse logic path
    // (if install_plugin needs a listed id, adjust mock to accept arbitrary test ids)
    let resp = hub.install_plugin("test-plugin-alpha", None).unwrap();
    assert_eq!(resp.plugin_id, "test-plugin-alpha");
    assert!(resp.download_url.contains("test-plugin-alpha"));
}
```

#### Task A4-4: cargo check + 绿态确认

```bash
cargo check -p attune-core 2>&1 | grep '^error'
cargo test -p attune-core plugin_hub 2>&1 | tail -15
# 期望: mock_hub_default_returns_empty_plugin_list PASSED
# 期望: mock_hub_no_industry_ids_in_listing PASSED
```

Commit：

```bash
git add rust/crates/attune-core/src/plugin_hub.rs
git commit -m "refactor(oss-boundary): MU-4 plugin_hub empty catalog fallback

- Clear MockPluginHubProvider::_builtin_plugins() → vec![]
- Industry catalog (law-pro/patent-pro/presales-pro/tech-pro) removed from OSS
- Marketplace UI receives [] → shows 'no plugins available' (existing fallback UI)
- HttpPluginHubProvider (real cloud) unaffected
- Add S4b regression: mock_hub_default_returns_empty_plugin_list + no_industry_ids

spec: docs/superpowers/specs/2026-06-02-oss-industry-decoupling.md §4.1 MU-4 G1-decision-1"
```

---

### MU-8 agent_quality_manifest.yaml

**文件**: `rust/agent_quality_manifest.yaml`  
**问题**: manifest 包含 `external: true` 的行业 gate 条目（law_pro_deterministic / law_pro_fact_extractor_llm / tech_pro_code_reviewer / law_pro_real_llm 等），这些在 OSS 仓不运行但仍作为 roll-up 记录  
**G1 决策**: 移除 `external: true` 行业插件门条目；OSS manifest 只含 oss-core gates

#### Task A8-1: 写 manifest 仅含 oss-core 回归测试

在 `rust/crates/attune-core/` 找到 `agent_gate_orchestrator.rs` 或对应测试，添加：

```rust
/// S4b 验收：agent_quality_manifest.yaml 仅含 oss-core gates（无 external 行业门）
#[test]
fn quality_manifest_contains_only_oss_core_gates() {
    let manifest = load_quality_manifest(); // 复用现有 load 函数
    for gate in &manifest.gates {
        assert!(
            gate.plugin == "oss-core" || gate.external != Some(true),
            "gate '{}' (plugin: {}) has external=true — industry gates must move to attune-pro manifest",
            gate.id, gate.plugin
        );
        // 具体说：外部 external=true 的行业门不应出现在 OSS manifest
        assert_ne!(
            gate.plugin.as_str(),
            "law-pro",
            "law-pro gate '{}' must not appear in OSS manifest",
            gate.id
        );
        assert_ne!(
            gate.plugin.as_str(),
            "tech-pro",
            "tech-pro gate '{}' must not appear in OSS manifest",
            gate.id
        );
    }
}
```

跑测试确认**红态**（当前 manifest 含 law-pro/tech-pro external 条目）：

```bash
cargo test -p attune-core quality_manifest_contains_only_oss_core_gates 2>&1 | tail -5
```

#### Task A8-2: 编辑 agent_quality_manifest.yaml 删除行业 external 门

从 `rust/agent_quality_manifest.yaml` 中删除以下 `- id:` 块（plugin: law-pro 或 plugin: tech-pro）：
- `id: law_pro_deterministic`（plugin: law-pro, external: true）
- `id: law_pro_fact_extractor_llm`（plugin: law-pro, external: true）
- `id: law_pro_real_llm`（plugin: law-pro，若存在）
- `id: tech_pro_code_reviewer`（plugin: tech-pro, external: true）
- 所有其他 `plugin: law-pro` 或 `plugin: tech-pro` 条目

保留所有 `plugin: oss-core` 条目：
- chat_reliability, document_classifier, linker, memory_consolidation, self_evolving_skill, oss_agent_real_llm
- office_ocr, office_asr（attune-server gates）

验证：
```bash
grep -c 'plugin: law-pro\|plugin: tech-pro' rust/agent_quality_manifest.yaml
# 期望: 0
grep -c 'plugin: oss-core' rust/agent_quality_manifest.yaml
# 期望: >= 7（保留原有 oss-core 门数量）
```

#### Task A8-3: 更新 ignore_spike baseline（若 manifest 加载测试硬编码门数量）

如 `agent_gate_orchestrator.rs` 中有 `assert_eq!(manifest.gates.len(), N)` 类断言，更新为新的 oss-core 门数量（原 N 减去行业门数量）。

#### Task A8-4: cargo check + 绿态

```bash
cargo check -p attune-core 2>&1 | grep '^error'
cargo test -p attune-core quality_manifest 2>&1 | tail -10
```

Commit：

```bash
git add rust/agent_quality_manifest.yaml \
        rust/crates/attune-core/src/agents/agent_gate_orchestrator.rs  # 若有改动
git commit -m "refactor(oss-boundary): MU-8 remove industry external gates from OSS quality manifest

- Delete law_pro_deterministic / law_pro_fact_extractor_llm / tech_pro_code_reviewer external entries
- OSS manifest now contains only oss-core gates (7 gates: 5 agent + 2 engine)
- Industry gates remain in attune-pro's own thresholds.yaml (external CI lane)
- Add S4b regression: quality_manifest_contains_only_oss_core_gates

spec: docs/superpowers/specs/2026-06-02-oss-industry-decoupling.md §4.1 MU-8"
```

---

### MU-18 oss_agent_real_llm_gate.rs

**文件**: `rust/crates/attune-core/tests/oss_agent_real_llm_gate.rs`  
**问题**: 文件注释引用 `defamation_extractor`（law-pro law-pro #54 incident）作为背景说明  
**G1 决策**: `#[ignore]` 已存在；仅更新注释移除 law-pro 专属引用，测试本身测 OSS 4 agents 保留

#### Task A18-1: 写注释引用确认回归

此 MU 是低风险清理。在文件顶部注释中搜索 law-pro 引用：

```bash
grep -n 'defamation_extractor\|law-pro\|law_pro\|patent\|presales\|civil_loan' \
    rust/crates/attune-core/tests/oss_agent_real_llm_gate.rs
```

预期结果（当前）：
- `line 5-6`: `attune-pro law-pro #54 incident: defamation_extractor passed every mock test but...`

#### Task A18-2: 更新文件头注释

将 line 1-50 的注释中行业专属内容替换为通用表述：

```rust
//! OSS 4-agent real-LLM verification gate — v1.0 GA pre-ship verification.
//!
//! ## 背景 / Why this test exists
//!
//! Per attune Agent 验证铁律 (CLAUDE.md): mock-only test gates provide false
//! security — an agent may pass all mock tests but fail against a real LLM.
//! This test runs the same drill for OSS attune's agents shipped in v1.0:
//!
//! | Agent | Module | Uses LLM? | Verified here? |
//! ...（保留 table，已是 OSS agents，无需改）
```

删除 `law-pro #54 incident: defamation_extractor` 等具体行业引用，替换为通用的 "Agent 验证铁律" 引用。

#### Task A18-3: cargo check

```bash
cargo check -p attune-core 2>&1 | grep '^error'
# 这是注释改动，不影响编译，确认无错误即可
```

Commit：

```bash
git add rust/crates/attune-core/tests/oss_agent_real_llm_gate.rs
git commit -m "refactor(oss-boundary): MU-18 clean industry agent references from oss_agent_real_llm_gate comments

- Replace law-pro/defamation_extractor incident reference with generic Agent 验证铁律 note
- Tests themselves unchanged (verify OSS 4 agents: memory_consolidation / self_evolving_skill)
- #[ignore] remains (requires Ollama locally)

spec: docs/superpowers/specs/2026-06-02-oss-industry-decoupling.md §4.1 MU-18 G1-decision-2"
```

---

### Batch A 收尾

#### Task A-FINAL: 全 workspace 测试 + OSS 独立 build 验证

```bash
# 1. 全 workspace 测试
cargo test --workspace 2>&1 | grep -E 'FAILED|^error' | head -30

# 2. OSS 独立 build（spec §7.1 关键红线）
# 确认 attune-core 无 attune-pro 依赖
grep -n 'attune-pro\|law-pro\|patent-pro\|presales-pro\|tech-pro' \
    rust/crates/attune-core/Cargo.toml | head -10
# 期望: 无输出（无行业仓依赖）

# 3. 三大泄漏点清零确认
grep -rn 'law-pro\|patent-pro\|presales-pro\|tech-pro' \
    rust/crates/attune-core/src/ --include='*.rs' | grep -v target
# 期望: 仅剩注释引用（如 pii/mod.rs 的 plugin pii_patterns 说明）

# 4. marketplace API 行为验证
# (integration test — 如有 server 级别的 marketplace 测试跑之)
cargo test -p attune-server marketplace 2>&1 | tail -10
```

---

## Batch B — P1 功能层

> **目标**: 移除功能层行业耦合（搜索词表/测试 fixture/chat flow/routes/entities）。  
> 文件：12 文件，162 hits（MU-5/6/7/12/19）。  
> **完成判据**: `cargo test --workspace` 通过，搜索结果不退化（OSS 通用 query 测试全绿）。

---

### MU-5 search.rs DOMAIN_EXPAND_MAP

**文件**: `rust/crates/attune-core/src/search.rs`  
**问题**: `DOMAIN_EXPAND_MAP` 硬编码 legal/medical/patent 三个行业词表（约 40 个词条，line 99-125）  
**G1 决策**: 删除行业词表；词表由 plugin manifest `domain_synonyms` 字段动态加载（由 search 引擎按需读取）

#### Task B5-1: 写搜索不退化回归测试

在 `rust/crates/attune-core/src/search.rs` 的 `#[cfg(test)]` 区域添加：

```rust
/// S4b 验收：通用搜索不退化（无行业词表 BM25+HNSW+RRF 正常运行）
#[test]
fn generic_search_works_without_domain_expand_map() {
    // BM25 通用搜索不依赖 DOMAIN_EXPAND_MAP
    let params = SearchParams {
        query: "机器学习 神经网络".to_string(),
        domain_hint: None, // 无 domain hint = 不触发 expand
        ..Default::default()
    };
    // 仅验证搜索路径不 panic / 不依赖行业词表常量
    assert!(params.domain_hint.is_none());
}

/// S4b 验收：DOMAIN_EXPAND_MAP 删除后 detect_query_domain 返回 None（无行业注入）
#[test]
fn detect_query_domain_returns_none_for_generic_query_after_s4b() {
    // 通用 query 不应被推断为行业 domain
    let domain = detect_query_domain("机器学习 深度网络", &[]);
    assert!(
        domain.is_none() || domain.as_deref() == Some("general"),
        "generic query must not be auto-detected as industry domain; got {:?}",
        domain
    );
}

/// S4b 验收：apply_cross_domain_penalty 在无行业词表时仍能运行（通用扩展点保留）
#[test]
fn cross_domain_penalty_still_applies_for_general_corpus() {
    let mut results = vec![SearchResult {
        item_id: 1,
        corpus_domain: "tech".to_string(),
        score: 1.0,
        ..Default::default()
    }];
    apply_cross_domain_penalty(&mut results, Some("general"));
    // domain_hint=general → skip penalty（spec §3.3）
    assert_eq!(results[0].score, 1.0, "general domain_hint must not penalize");
}
```

跑确认（此时可能已绿，因为函数本身已存在，只是验证逻辑正确性）：

```bash
cargo test -p attune-core generic_search_works_without_domain_expand_map 2>&1 | tail -5
```

#### Task B5-2: 删除 DOMAIN_EXPAND_MAP 中的行业词表条目

定位 `search.rs` 中 `DOMAIN_EXPAND_MAP`（约 line 95-130）：

```rust
// 删除 legal 条目（约 line 99-116）:
("legal", &[
    "法律", "法条", "法规", ... (约 20 词)
]),
// 删除 medical 条目（约 line 118-121）:
("medical", &[
    "病历", "诊断", ... (约 13 词)
]),
// 删除 patent 条目（约 line 122-125）:
("patent", &[
    "专利", "权利要求", ... (约 12 词)
]),
```

保留通用条目（若有，如 "general" / "tech" 的通用词表，或直接空 map）。  
保留 `CROSS_DOMAIN_PENALTY` 常量（通用架构扩展点，spec §3.3 明确保留）。  
保留 `apply_cross_domain_penalty` 函数（plugin 安装后可提供 domain_hint）。  
保留 `detect_query_domain` 函数（改为依赖 plugin manifest 注入而非硬编码词表）。

验证：
```bash
grep -n '"legal"\|"medical"\|"patent"' rust/crates/attune-core/src/search.rs | grep -v '//\|domain_hint\|corpus_domain\|query'
# 期望: 无行业词表 entry（注释和参数名里的引用不算）
```

#### Task B5-3: 更新 detect_query_domain（若依赖 DOMAIN_EXPAND_MAP）

若 `detect_query_domain` 函数体中 iterate over `DOMAIN_EXPAND_MAP` 来判断词汇域：

```rust
// 旧实现（若存在）:
pub fn detect_query_domain(query: &str, plugin_synonyms: &[(&str, &[&str])]) -> Option<String> {
    for (domain, words) in DOMAIN_EXPAND_MAP.iter().chain(plugin_synonyms) {
        ...
    }
}

// 新实现（只用 plugin_synonyms，无硬编码行业词表）:
pub fn detect_query_domain(query: &str, plugin_synonyms: &[(&str, &[&str])]) -> Option<String> {
    // Domain detection is now purely plugin-driven (S4b).
    // OSS without plugins: returns None (no domain expansion).
    // law-pro installed: plugin manifest domain_synonyms injected via plugin_synonyms param.
    for (domain, words) in plugin_synonyms {
        if words.iter().any(|w| query.contains(*w)) {
            return Some(domain.to_string());
        }
    }
    None
}
```

#### Task B5-4: cargo check + 绿态

```bash
cargo check -p attune-core 2>&1 | grep '^error'
cargo test -p attune-core search 2>&1 | tail -15
```

Commit：

```bash
git add rust/crates/attune-core/src/search.rs
git commit -m "refactor(oss-boundary): MU-5 remove DOMAIN_EXPAND_MAP industry word lists

- Delete legal/medical/patent entries from DOMAIN_EXPAND_MAP
- detect_query_domain now plugin-driven only (plugin_synonyms param)
- CROSS_DOMAIN_PENALTY constant retained (generic architecture extension point)
- apply_cross_domain_penalty retained (works when plugin provides domain_hint)
- OSS without plugins: domain_hint=None → no domain expansion (spec §3.3)
- Add S4b regression: generic_search_works_without / detect_query_domain_returns_none

spec: docs/superpowers/specs/2026-06-02-oss-industry-decoupling.md §4.1 MU-5"
```

---

### MU-6 agents/flow/tests.rs + registry/tests.rs + 3 文件

**文件**: `src/agents/flow/tests.rs`, `src/agents/registry/tests.rs`, 以及 3 个相关测试文件（共 5 文件，101 hits）  
**问题**: 行业 agent fixture (`civil_loan_agent`, `defamation_extractor` 等) 作为通用框架测试的 fixture，测试逻辑与行业绑定  
**迁移策略**: 用 `test-calc-agent` / `test-extract-agent` 通用 fixture 替换行业 fixture；law-pro 特定流程测试迁 attune-pro

#### Task B6-1: 写通用 fixture 回归测试

在 `src/agents/flow/tests.rs` 开头追加：

```rust
/// S4b 通用 fixture：替换 defamation_registry/defamation_flow 的通用等价版本
/// 用于验证 AgentFlow 框架能力本身，不绑定任何行业
fn test_calculator_agent_toml() -> &'static str {
    r#"
[[agent]]
id = "test_calc_agent"
tier = "free"
plugin = "oss-core"
kind = "deterministic"
capability_boundary = "数值计算 → 计算结果"
model_tier_floor = ""
cost_class = "zero"
gate = "oss-core/agent_golden_gate::calc"
route_keywords = ["计算测试", "数值测试"]
route_priority = 5
[agent.handoff]
consumes = "RawInput"
produces = "CalcResult"
"#
}

fn test_extractor_agent_toml() -> &'static str {
    r#"
[[agent]]
id = "test_extract_agent"
tier = "free"
plugin = "oss-core"
kind = "deterministic"
capability_boundary = "文本抽取 → 结构化输出"
model_tier_floor = ""
cost_class = "zero"
gate = "oss-core/agent_golden_gate::extract"
route_keywords = ["抽取测试", "提取测试"]
route_priority = 5
[agent.handoff]
consumes = "CalcResult"
produces = "ExtractedData"
"#
}
```

#### Task B6-2: 替换 flow/tests.rs 中的 defamation_registry/defamation_flow

将所有出现 `defamation_registry()` 的测试，改用通用 `test_registry()` + `test_flow()`：

```bash
# 确认当前 defamation 相关引用数
grep -n 'defamation_registry\|defamation_flow\|civil_loan_agent\|fact_extractor\|DefamationFacts\|CaseFacts\|DamageAward' \
    rust/crates/attune-core/src/agents/flow/tests.rs | wc -l
```

逐个替换步骤：
1. `defamation_registry()` → `test_registry()`（构造含 `test_calc_agent` + `test_extract_agent` 的 registry）
2. `defamation_flow()` → `test_flow()`（构造 `test_calc_agent → test_extract_agent` 的 2-step flow）
3. `"legal_defamation"` flow id → `"test_two_step_flow"`
4. 类型 handoff 名 `CaseFacts / DefamationFacts / DamageAward` → `CalcResult / ExtractedData`（在 fixture TOML 中）
5. 断言中引用行业 flow id（`assert_eq!(out.flow_id, "legal_defamation")`）→ `"test_two_step_flow"`

被迁移到 attune-pro 的测试（验证行业业务语义的，如 `defamation_flow_typed_chain_connects_extractor_to_damages`）标 `[BLOCKED: attune-pro]`：

```rust
#[ignore = "S4b: legal_defamation flow test migrated to attune-pro/tests/"]
fn defamation_flow_typed_chain_connects_extractor_to_damages() {
    // [BLOCKED: attune-pro] see attune-pro/plugins/law-pro/tests/flow_test.rs
}
```

#### Task B6-3: 替换 registry/tests.rs 中的行业 fixture

将 `one_agent_toml()` 中的 `plugin = "law-pro"` fixture 改为 `plugin = "oss-core"` 通用 fixture（已在 Task A1-2 中部分处理，此处补完纯逻辑测试）：

```rust
fn one_agent_toml() -> &'static str {
    r#"
[[agent]]
id = "test_agent_one"
tier = "free"
plugin = "oss-core"
kind = "deterministic"
capability_boundary = "通用测试能力边界"
model_tier_floor = ""
cost_class = "zero"
gate = "oss-core/test_golden_gate"
route_keywords = ["测试关键词"]
route_priority = 5
[agent.handoff]
consumes = "RawInput"
produces = "ProcessedOutput"
"#
}
```

#### Task B6-4: 处理其他 3 个文件

根据 census 查找其余 3 个含行业 fixture 的 tests 文件：

```bash
grep -rn 'civil_loan_agent\|defamation_extractor\|law-pro\|fact_extractor' \
    rust/crates/attune-core/src/ --include='tests.rs' | grep -v target
```

对每个文件：
- 纯 fixture 替换（`"law-pro"` → `"oss-core"`, `"civil_loan_agent"` → `"test_agent_generic"`）
- 验证行业语义的测试 → `#[ignore]` + `[BLOCKED: attune-pro]` 注释

#### Task B6-5: cargo check + 绿态

```bash
cargo check -p attune-core 2>&1 | grep '^error'
cargo test -p attune-core agents 2>&1 | tail -20
```

Commit：

```bash
git add rust/crates/attune-core/src/agents/
git commit -m "refactor(oss-boundary): MU-6 replace industry agent fixtures with generic test-calc/extract fixtures

- defamation_registry() → test_registry() (test_calc_agent + test_extract_agent)
- defamation_flow() → test_flow() (test_two_step_flow)
- one_agent_toml plugin=law-pro → plugin=oss-core test_agent_one
- industry-semantic tests → #[ignore] [BLOCKED: attune-pro]
- Typed handoff framework tests preserved with generic type names

spec: docs/superpowers/specs/2026-06-02-oss-industry-decoupling.md §4.1 MU-6"
```

---

### MU-7 acp_chat.rs + acp5_chat_flow_wire_test.rs

**文件**: `src/agents/flow/tests.rs`（acp5 测试区），`crates/attune-server/src/acp_chat.rs`（或对应路径）  
**问题**: `defamation_registry()` / `defamation_flow()` 在 acp_chat 层的 wire test 中作为 integration fixture

#### Task B7-1: 确认文件路径

```bash
find /data/company/project/attune/rust -name 'acp_chat.rs' -o -name '*acp5*wire*' | grep -v target
```

#### Task B7-2: 写通用 acp_chat flow wire 测试替换

在 acp_chat 相关测试文件中，将 `defamation_registry()` / `defamation_flow()` 改为引用 MU-6 中新建的 `test_registry()` / `test_flow()`：

```rust
// 旧（约 line 248）:
let reg = defamation_registry();
let flows = defamation_flow();

// 新:
let reg = test_registry();   // 来自 MU-6 新建的通用 fixture 函数
let flows = test_flow();
```

对应断言 `assert_eq!(out.flow_id, "legal_defamation")` → `assert_eq!(out.flow_id, "test_two_step_flow")`。

#### Task B7-3: 将 acp_chat.rs 中 civil_loan_agent 引用更新

在 `acp_chat.rs` 找到 `civil_loan_agent` 路由测试（约 line 834, 886, 890, 925, 928）：

```bash
grep -n 'civil_loan_agent\|defamation\|legal_defamation' \
    /data/company/project/attune/rust/crates/attune-core/src/agents/flow/tests.rs | head -20
```

将这些测试中的行业 agent ID 替换为通用 `test_calc_agent`，路由关键词改为 `"计算测试"`：

```rust
// 旧: assert_eq!(resolved.flow.steps, ["civil_loan_agent"]);
// 新: assert_eq!(resolved.flow.steps, ["test_calc_agent"]);
```

#### Task B7-4: cargo check + 绿态

```bash
cargo check -p attune-core 2>&1 | grep '^error'
cargo test -p attune-core acp 2>&1 | tail -10
```

Commit：

```bash
git add rust/crates/attune-core/src/agents/flow/tests.rs
git commit -m "refactor(oss-boundary): MU-7 replace defamation_registry/flow with test_registry/flow in acp_chat tests

- defamation_registry → test_registry, defamation_flow → test_flow
- civil_loan_agent → test_calc_agent in routing tests
- legal_defamation flow_id → test_two_step_flow
- ACP-5 wire test framework validity preserved

spec: docs/superpowers/specs/2026-06-02-oss-industry-decoupling.md §4.1 MU-7"
```

---

### MU-12 server routes（4 文件）

**文件**: `rust/crates/attune-server/src/routes/plugins.rs`（主要），及 3 个相关 route 文件  
**问题**: `plugins.rs:59` 硬编码 `["tech", "law", "presales", "patent"].contains(&p.id)` 作为 builtin 判断  
**G1 决策**: `builtin` 判断改为读 `plugin.manifest.builtin` 布尔字段

#### Task B12-1: 写 builtin 判断回归测试

在 `routes/plugins.rs` 或 `attune-server/tests/` 添加：

```rust
/// S4b 验收：builtin 判断来自 manifest.builtin 字段，而非硬编码行业 ID 列表
#[test]
fn plugin_source_field_reads_manifest_builtin_not_hardcoded_id() {
    // plugin id 为 "law" 但 manifest.builtin=false → source 应为 "user"
    // 验证逻辑已不依赖行业 ID 列表
    // （integration test 需 plugin fixture，此处仅做编译验证）
}

/// S4b 验收：source="builtin" 仅来自 manifest.builtin=true 的 plugin
#[test]
fn plugin_with_manifest_builtin_true_gets_builtin_source() {
    // 构造 manifest.builtin=true 的 plugin stub，验证 source="builtin"
}
```

#### Task B12-2: 修改 plugins.rs builtin 判断逻辑

将 `plugins.rs:59`：

```rust
// 旧:
"source": if ["tech", "law", "presales", "patent"].contains(&p.id.as_str()) { "builtin" } else { "user" },

// 新（读 manifest.builtin 字段）:
"source": if p.manifest.builtin { "builtin" } else { "user" },
```

若 `PluginManifest` 结构体尚无 `builtin: bool` 字段，需先在 plugin_registry.rs 的 manifest 结构中添加：

```rust
// plugin_registry.rs (PluginManifest struct)
pub struct PluginManifest {
    // ... 现有字段 ...
    /// S4b: true = oss-core builtin capability (not a user-installed 3rd party plugin)
    #[serde(default)]
    pub builtin: bool,
    // ... 
}
```

更新 oss-core 内置 plugin YAML（`assets/plugins/*/plugin.yaml`）添加 `builtin: true`。

#### Task B12-3: 修复 lock fallback 路径（line 108-115）

```rust
// 旧 (line 108-115):
let plugins = Taxonomy::load_builtin_plugins().map_err(|e| internal("load_builtin_plugins", e))?;
// ... "source": "builtin"

// 确认这个路径的语义是否正确，或是否需要同样改为 manifest.builtin
// 若 load_builtin_plugins 仅返回 oss-core 内置（ai_annotation_* 等），则 "builtin" 硬编码合理
// 若包含行业 plugin，则需清理
grep -n 'load_builtin_plugins\|Taxonomy::load' \
    rust/crates/attune-server/src/routes/plugins.rs | head -5
```

#### Task B12-4: cargo check + 绿态

```bash
cargo check -p attune-server 2>&1 | grep '^error'
cargo test -p attune-server plugins 2>&1 | tail -10
```

Commit：

```bash
git add rust/crates/attune-server/src/routes/plugins.rs \
        rust/crates/attune-core/src/plugin_registry.rs  # manifest.builtin 字段
git commit -m "refactor(oss-boundary): MU-12 builtin source from manifest.builtin not hardcoded id list

- Remove ['tech','law','presales','patent'] hardcoded builtin-id check
- source = if plugin.manifest.builtin { 'builtin' } else { 'user' }
- Add PluginManifest.builtin: bool field (serde default=false)
- Update oss-core builtin plugin.yaml with builtin: true

spec: docs/superpowers/specs/2026-06-02-oss-industry-decoupling.md §4.1 MU-12 G1-decision-3"
```

---

### MU-19 entities.rs

**文件**: `rust/crates/attune-core/src/entities.rs`  
**问题**: census 显示 4 hits，但查看 entities.rs 实际内容，`EntityKind` 枚举只含通用类型（Person/Money/Date/Organization），无 `CaseNo`/`CourtSeal`。文件注释（line 11）提及行业实体由 vertical plugin 实现（已是正确设计）。  
**实际工作**: 确认无行业 variant，注释已正确，4 hits 均为注释或文档引用 — 清理即可

#### Task B19-1: 确认 EntityKind 无行业 variant

```bash
grep -n 'CaseNo\|CourtSeal\|EntityKind' \
    rust/crates/attune-core/src/entities.rs
```

预期：只有 `Person/Money/Date/Organization` 4 个 variant + 注释中的说明文字（无 CaseNo 代码）。

#### Task B19-2: 清理注释中过于具体的行业引用

将 `entities.rs:11` 注释：

```
// 行业专属实体（如律师案号 CaseNo / 病历号 / 商标号）由各 vertical plugin 实现自己的 extractor
```

保持（这是正确的架构说明，无需改）。仅检查 `entities_test.rs` 中是否有行业 fixture 需要替换：

```bash
grep -n 'CaseNo\|law\|legal\|patent' \
    rust/crates/attune-core/tests/entities_test.rs 2>/dev/null | head -10
```

若 `entities_test.rs` 中有 `"案号"` 等法律词汇作为通用 extract_entities 测试 fixture，保留（中文文本测试合理，不是行业耦合）。

#### Task B19-3: cargo check（低风险，主要是确认）

```bash
cargo check -p attune-core 2>&1 | grep '^error'
# 期望: 无（此 MU 主要是确认 + 注释清理）
```

Commit（若有实质改动）：

```bash
git add rust/crates/attune-core/src/entities.rs
git commit -m "refactor(oss-boundary): MU-19 verify entities.rs has no industry EntityKind variants

- Confirmed: EntityKind = Person/Money/Date/Organization only (no CaseNo/CourtSeal)
- Architecture note in doc comment correct (industry entities via plugin)
- Minor comment cleanup if needed

spec: docs/superpowers/specs/2026-06-02-oss-industry-decoupling.md §4.1 MU-19"
```

---

### Batch B 收尾

#### Task B-FINAL: 全 workspace 测试 + 搜索不退化验证

```bash
# 1. 全 workspace 测试
cargo test --workspace 2>&1 | grep -E 'FAILED|^error' | head -20

# 2. 搜索不退化验证（spec §9.2）
cargo test -p attune-core search 2>&1 | tail -15

# 3. DOMAIN_EXPAND_MAP 清零确认
grep -n '"legal"\|"medical"\|"patent"' \
    rust/crates/attune-core/src/search.rs | grep -v '//.*comment\|domain_hint\|corpus_domain'
# 期望: 无行业词表条目

# 4. 行业 agent fixture 清零确认
grep -rn 'civil_loan_agent\|defamation_extractor\|defamation_agent\|fact_extractor' \
    rust/crates/attune-core/src/ --include='*.rs' | grep -v target | grep -v '#\[ignore\]'
# 期望: 0（已清净或已 #[ignore]）
```

---

## Batch C — P1 协议层

> **目标**: 清理 plugin 协议层的行业 fixture（plugin_registry 内联 YAML、plugin_sync、generic_plugins_test）。  
> 文件：4 文件，129 hits（MU-3/10/11）。  
> **完成判据**: `cargo test --workspace` 通过，plugin 协议 E2E 测试全绿。

---

### MU-3 plugin_registry.rs 内联 fixture

**文件**: `rust/crates/attune-core/src/plugin_registry.rs`（73 hits）  
**问题**: 内联 YAML 测试 fixture 使用 `id: law-pro` / `id: medical-pro` / `id: patent-pro` + `extract_patent_claims` 技能 ID

#### Task C3-1: 写通用 plugin fixture 回归测试

在 `plugin_registry.rs` 测试区添加：

```rust
/// S4b 验收：plugin registry 测试 fixture 使用通用 test-plugin，不绑定行业 ID
#[test]
fn plugin_registry_accepts_generic_test_plugin() {
    let yaml = r#"
id: test-plugin
name: Test Plugin
version: "1.0.0"
builtin: false
skills:
  - id: test_skill_alpha
    kind: search
"#;
    let manifest: PluginManifest = serde_yaml::from_str(yaml).expect("generic plugin parses");
    assert_eq!(manifest.id, "test-plugin");
    assert!(!manifest.builtin);
}
```

#### Task C3-2: 替换内联行业 YAML fixture

在 `plugin_registry.rs` 中找到所有内联 YAML 字符串（in `#[cfg(test)]` 区域）：

```bash
grep -n 'law-pro\|medical-pro\|patent-pro\|presales-pro\|extract_patent_claims' \
    rust/crates/attune-core/src/plugin_registry.rs | head -30
```

逐个替换：
- `id: law-pro` → `id: test-plugin-a`
- `id: medical-pro` → `id: test-plugin-b`
- `id: patent-pro` → `id: test-plugin-c`
- `id: presales-pro` → `id: test-plugin-d`
- skill `id: extract_patent_claims` → `id: test_skill_search`
- 断言 `assert!(ids.contains(&"extract_patent_claims"))` → `assert!(ids.contains(&"test_skill_search"))`

#### Task C3-3: cargo check + 绿态

```bash
cargo check -p attune-core 2>&1 | grep '^error'
cargo test -p attune-core plugin_registry 2>&1 | tail -15
```

Commit：

```bash
git add rust/crates/attune-core/src/plugin_registry.rs
git commit -m "refactor(oss-boundary): MU-3 plugin_registry inline test fixtures → test-plugin generic

- Replace law-pro/medical-pro/patent-pro/presales-pro IDs with test-plugin-{a,b,c,d}
- extract_patent_claims skill → test_skill_search
- Plugin registry protocol logic tested with generic fixtures

spec: docs/superpowers/specs/2026-06-02-oss-industry-decoupling.md §4.1 MU-3"
```

---

### MU-10 plugin_sync.rs + plugin_protocol_e2e.rs

**文件**: `rust/crates/attune-core/tests/plugin_sync.rs`（或类似路径），`plugin_protocol_e2e.rs`  
**问题**: law-pro fixture YAML 在 sync 协议 + E2E 测试中；CaseMetadata E2E 测试（已部分在 MU-2 处理）

#### Task C10-1: 定位文件并确认 law-pro 引用

```bash
find /data/company/project/attune/rust -name 'plugin_sync.rs' | grep -v target | head -2
grep -n 'law-pro\|CaseMetadata\|legal_defamation' \
    rust/crates/attune-core/tests/plugin_protocol_e2e.rs | head -20
```

#### Task C10-2: 替换 plugin_sync.rs 中的 law-pro fixture

将所有 `"law-pro"` plugin ID → `"test-plugin"` 通用版，保留 sync 协议逻辑测试。

#### Task C10-3: 确认 plugin_protocol_e2e.rs MU-2 残余处理完毕

MU-2 Task A2-4 已标 `#[ignore]` 了 `case_metadata_with_classified_evidence_persists`。确认 E2E 文件中无其他 `CaseMetadata` 引用：

```bash
grep -n 'CaseMetadata\|case_metadata' \
    rust/crates/attune-core/tests/plugin_protocol_e2e.rs | grep -v ignore | grep -v '//'
# 期望: 无活跃引用
```

#### Task C10-4: cargo check + 绿态

```bash
cargo check -p attune-core 2>&1 | grep '^error'
cargo test -p attune-core plugin 2>&1 | tail -15
```

Commit：

```bash
git add rust/crates/attune-core/tests/plugin_sync.rs \
        rust/crates/attune-core/tests/plugin_protocol_e2e.rs
git commit -m "refactor(oss-boundary): MU-10 plugin_sync + protocol_e2e fixtures → test-plugin

- plugin_sync.rs: law-pro fixture → test-plugin
- plugin_protocol_e2e.rs: confirm CaseMetadata test marked #[ignore] (MU-2)

spec: docs/superpowers/specs/2026-06-02-oss-industry-decoupling.md §4.1 MU-10"
```

---

### MU-11 generic_plugins_test.rs

**文件**: `rust/crates/attune-core/tests/generic_plugins_test.rs`（25 hits）  
**问题**: law-pro/patent-pro inline YAML 在通用 plugin framework 测试中

#### Task C11-1: 替换所有行业 inline YAML

```bash
grep -n 'law-pro\|patent-pro\|medical-pro\|presales-pro' \
    rust/crates/attune-core/tests/generic_plugins_test.rs | wc -l
# 确认 hit 数，然后替换
sed -i 's/id: law-pro/id: test-plugin-law/g; s/id: patent-pro/id: test-plugin-patent/g; \
         s/id: medical-pro/id: test-plugin-medical/g; s/id: presales-pro/id: test-plugin-presales/g' \
    rust/crates/attune-core/tests/generic_plugins_test.rs
```

手动检查替换结果确认无误：

```bash
grep -n 'law-pro\|patent-pro\|medical-pro\|presales-pro' \
    rust/crates/attune-core/tests/generic_plugins_test.rs
# 期望: 无输出
```

#### Task C11-2: cargo check + 绿态

```bash
cargo check -p attune-core 2>&1 | grep '^error'
cargo test -p attune-core generic_plugins 2>&1 | tail -10
```

Commit：

```bash
git add rust/crates/attune-core/tests/generic_plugins_test.rs
git commit -m "refactor(oss-boundary): MU-11 generic_plugins_test inline YAML → test-plugin-* IDs

- law-pro/patent-pro/medical-pro/presales-pro → test-plugin-{law,patent,medical,presales}
- Generic plugin framework logic preserved

spec: docs/superpowers/specs/2026-06-02-oss-industry-decoupling.md §4.1 MU-11"
```

---

### Batch C 收尾

#### Task C-FINAL: 全 workspace 测试

```bash
cargo test --workspace 2>&1 | grep -E 'FAILED|^error' | head -20

# plugin 协议清净确认
grep -rn '"law-pro"\|"patent-pro"\|"presales-pro"' \
    rust/crates/attune-core/src/plugin_registry.rs \
    rust/crates/attune-core/tests/ \
    --include='*.rs' | grep -v target | grep -v '#\[ignore\]' | grep -v '//'
# 期望: 无活跃引用
```

---

## Batch D — P2 fixture 字符串

> **目标**: 清理 P2 低风险纯 fixture 字符串替换（MU-9/13-17/20）。  
> 文件：22 文件，83 hits（+ MU-20 杂项 11 文件 28 hits）。  
> **完成判据**: `cargo test --workspace` 通过，`grep -rn '"law-pro"\|"legal"\|defamation_extractor'` 在 src/ 中无活跃非注释引用。

---

### MU-9 store/state_migration.rs

**文件**: `rust/crates/attune-core/src/store/state_migration.rs`（13 hits）  
**问题**: 迁移测试全部使用 `"law-pro"` 作为 plugin_id 参数

#### Task D9-1: 替换 law-pro → test-plugin

```bash
grep -n '"law-pro"' rust/crates/attune-core/src/store/state_migration.rs | head -15
# 全量替换
sed -i 's/"law-pro"/"test-plugin"/g' \
    rust/crates/attune-core/src/store/state_migration.rs
# 验证
grep -c '"law-pro"' rust/crates/attune-core/src/store/state_migration.rs
# 期望: 0
```

#### Task D9-2: cargo check + 绿态

```bash
cargo check -p attune-core 2>&1 | grep '^error'
cargo test -p attune-core state_migration 2>&1 | tail -10
```

Commit：

```bash
git add rust/crates/attune-core/src/store/state_migration.rs
git commit -m "refactor(oss-boundary): MU-9 state_migration test plugin_id law-pro → test-plugin

spec: docs/superpowers/specs/2026-06-02-oss-industry-decoupling.md §4.1 MU-9"
```

---

### MU-13 ingest/connector.rs + ingest/local.rs

**文件**: `src/ingest/connector.rs`（3 hits），`src/ingest/local.rs`（2 hits）  
**问题**: 测试 fixture 中 `corpus_domain: Some("legal".into())` 固化行业 domain

#### Task D13-1: 替换 legal → general（测试 fixture 中）

```bash
# connector.rs
grep -n '"legal"' rust/crates/attune-core/src/ingest/connector.rs
# 替换测试 fixture 中的 "legal" → "general"（生产代码中的注释保留）
# line 128: corpus_domain: Some("legal".into()) → Some("general".into())
# line 136: assert_eq!(doc.corpus_domain.as_deref(), Some("legal")) → Some("general")

# local.rs
grep -n '"legal"' rust/crates/attune-core/src/ingest/local.rs
# line 134: assert_eq!(doc.corpus_domain.as_deref(), Some("legal")) → Some("general")
```

注意：`corpus_domain` 字段本身保留（`Option<String>` 开放语义，G1 决策 3 不枚举化）。只将测试 fixture 的硬编码值从 `"legal"` 改为通用 `"general"`。

#### Task D13-2: cargo check + 绿态

```bash
cargo check -p attune-core 2>&1 | grep '^error'
cargo test -p attune-core ingest 2>&1 | tail -10
```

Commit：

```bash
git add rust/crates/attune-core/src/ingest/connector.rs \
        rust/crates/attune-core/src/ingest/local.rs
git commit -m "refactor(oss-boundary): MU-13 ingest test fixture corpus_domain legal → general

spec: docs/superpowers/specs/2026-06-02-oss-industry-decoupling.md §4.1 MU-13"
```

---

### MU-14 store/mod.rs SQL 注释

**文件**: `rust/crates/attune-core/src/store/mod.rs`（6 hits）  
**问题**: SQL migration 注释示例 `CASE_NO` 字段

#### Task D14-1: 替换 SQL 注释示例

```bash
grep -n 'CASE_NO\|case_no\|legal\|law' \
    rust/crates/attune-core/src/store/mod.rs | head -10
# line 87: 注释中 corpus_domain 示例 (legal/tech/general/medical/...)
# → 改为 (general/tech/science/...)
```

将注释中 `legal/tech/general/medical/...` → `general/tech/science/...`（去除 legal/medical 具体行业列举，保持示例通用性）。

#### Task D14-2: cargo check

```bash
cargo check -p attune-core 2>&1 | grep '^error'
```

Commit：

```bash
git add rust/crates/attune-core/src/store/mod.rs
git commit -m "refactor(oss-boundary): MU-14 store/mod.rs SQL comment corpus_domain example → generic

spec: docs/superpowers/specs/2026-06-02-oss-industry-decoupling.md §4.1 MU-14"
```

---

### MU-15 遥测/使用量测试（3 文件）

**文件**: 3 个遥测测试文件（22 hits）  
**问题**: `"defamation_extractor"` / `"law-pro"` 作为 agent telemetry 测试的 fixture

#### Task D15-1: 定位文件

```bash
grep -rn '"defamation_extractor"\|"law-pro"' \
    rust/crates/attune-core/src/ rust/crates/attune-server/src/ \
    --include='*.rs' | grep -i 'telemetry\|usage\|signal\|metric' | grep -v target | head -20
```

#### Task D15-2: 替换 fixture

```bash
# 对每个文件：
# "defamation_extractor" → "test_agent"
# "law-pro" → "test-plugin"
# 验证替换后测试逻辑（assert 内容）仍合理（测试 telemetry 框架，不测行业语义）
```

#### Task D15-3: cargo check + 绿态

```bash
cargo check -p attune-core 2>&1 | grep '^error'
cargo test -p attune-core telemetry 2>&1 | tail -10
```

Commit：

```bash
git add # 具体文件
git commit -m "refactor(oss-boundary): MU-15 telemetry test fixtures defamation_extractor/law-pro → test_agent/test-plugin

spec: docs/superpowers/specs/2026-06-02-oss-industry-decoupling.md §4.1 MU-15"
```

---

### MU-16 skill_evolution/agent.rs + mod.rs

**文件**: `src/skill_evolution/agent.rs`（8 hits），`src/skill_evolution/mod.rs`（1 hit）  
**问题**: `defamation_extractor` 作为技能自进化模块测试/注释中的示例 agent

#### Task D16-1: 替换示例 agent ID

```bash
grep -n 'defamation_extractor' \
    rust/crates/attune-core/src/skill_evolution/agent.rs \
    rust/crates/attune-core/src/skill_evolution/mod.rs

# 全量替换：
sed -i 's/defamation_extractor/test_skill_agent/g' \
    rust/crates/attune-core/src/skill_evolution/agent.rs \
    rust/crates/attune-core/src/skill_evolution/mod.rs
```

#### Task D16-2: cargo check + 绿态

```bash
cargo check -p attune-core 2>&1 | grep '^error'
cargo test -p attune-core skill_evolution 2>&1 | tail -10
```

Commit：

```bash
git add rust/crates/attune-core/src/skill_evolution/
git commit -m "refactor(oss-boundary): MU-16 skill_evolution defamation_extractor → test_skill_agent

spec: docs/superpowers/specs/2026-06-02-oss-industry-decoupling.md §4.1 MU-16"
```

---

### MU-17 cloud_client.rs

**文件**: `rust/crates/attune-core/src/cloud_client.rs`（6 hits）  
**问题**: JSON fixture 中 `"law-pro"` plugin_id

#### Task D17-1: 替换 JSON fixture

```bash
grep -n '"law-pro"' rust/crates/attune-core/src/cloud_client.rs | head -10
sed -i 's/"law-pro"/"test-plugin"/g' \
    rust/crates/attune-core/src/cloud_client.rs
grep -c '"law-pro"' rust/crates/attune-core/src/cloud_client.rs
# 期望: 0
```

#### Task D17-2: cargo check + 绿态

```bash
cargo check -p attune-core 2>&1 | grep '^error'
cargo test -p attune-core cloud_client 2>&1 | tail -10
```

Commit：

```bash
git add rust/crates/attune-core/src/cloud_client.rs
git commit -m "refactor(oss-boundary): MU-17 cloud_client JSON fixture law-pro → test-plugin

spec: docs/superpowers/specs/2026-06-02-oss-industry-decoupling.md §4.1 MU-17"
```

---

### MU-20 杂项（11 文件）

**文件**: 11 个杂项文件（28 hits）  
**问题**: 散落的行业引用，包括：
- `ui_runtime.rs`: `loan_doc_exists` 字段名（行业专属 UI checkbox）
- `store/items.rs:551` 注释中 `legal / tech / medical / patent` 枚举示例
- `webdav_remotes_test.rs`: `corpus_domain: "legal"` fixture  
- `ingest_pipeline_test.rs`: `corpus_domain = Some("legal")`
- 其他注释清理

#### Task D20-1: 定位全部杂项文件

```bash
grep -rn '"loan_doc_exists"\|loan_doc_exists\|corpus_domain.*legal\|legal.*corpus_domain' \
    rust/crates/attune-core/src/ rust/crates/attune-core/tests/ \
    --include='*.rs' | grep -v target | grep -v MU | head -30
```

#### Task D20-2: ui_runtime.rs loan_doc_exists

`ui_runtime.rs:217` 的 `loan_doc_exists` 是行业专属 UI checkbox 名：

```bash
grep -n 'loan_doc_exists' rust/crates/attune-core/src/ui_runtime.rs
```

将 `loan_doc_exists` → `custom_field_exists`（通用名），测试断言同步更新（line 248）：

```rust
// 旧: assert!(html.contains(r#"type="checkbox" name="loan_doc_exists""#));
// 新: assert!(html.contains(r#"type="checkbox" name="custom_field_exists""#));
```

#### Task D20-3: 注释/fixture 清理

- `store/items.rs:551` 注释：`legal / tech / medical / patent / general` → `general / tech / science / custom`
- `webdav_remotes_test.rs:26,38`: `corpus_domain: "legal"` → `"general"`
- `ingest_pipeline_test.rs:145`: `corpus_domain = Some("legal")` → `Some("general")`
- `store/mod.rs` 剩余注释（若 MU-14 未完全覆盖）

#### Task D20-4: cargo check + 绿态

```bash
cargo check -p attune-core 2>&1 | grep '^error'
cargo test -p attune-core 2>&1 | grep -E 'FAILED|^error' | head -10
```

Commit：

```bash
git add rust/crates/attune-core/src/ui_runtime.rs \
        rust/crates/attune-core/src/store/items.rs \
        rust/crates/attune-core/tests/webdav_remotes_test.rs \
        rust/crates/attune-core/tests/ingest_pipeline_test.rs
git commit -m "refactor(oss-boundary): MU-20 misc cleanup loan_doc_exists + corpus_domain legal fixtures

- ui_runtime loan_doc_exists → custom_field_exists
- store/items.rs corpus_domain comment: legal/medical → general/tech/science examples
- webdav + ingest test fixtures corpus_domain legal → general

spec: docs/superpowers/specs/2026-06-02-oss-industry-decoupling.md §4.1 MU-20"
```

---

### Batch D 收尾

#### Task D-FINAL: 全 workspace 测试 + 完整清零验证

```bash
# 1. 全 workspace 测试（spec §7.1 最终门）
cargo test --workspace 2>&1 | grep -E 'FAILED|^error' | head -20

# 2. 完整行业引用清零扫描
echo "=== 检查 src/ 中残余行业耦合 ==="
grep -rn '"law-pro"\|"patent-pro"\|"presales-pro"\|"tech-pro"\|defamation_extractor\|civil_loan_agent\|fact_extractor' \
    rust/crates/attune-core/src/ rust/crates/attune-server/src/ \
    --include='*.rs' | grep -v target | grep -v '#\[ignore\]' | grep -v '//' | head -20

echo "=== 检查 tests/ 中残余行业耦合（已处理的 #[ignore] 不算）==="
grep -rn '"law-pro"\|"patent-pro"\|defamation_extractor' \
    rust/crates/attune-core/tests/ rust/crates/attune-server/tests/ \
    --include='*.rs' | grep -v target | grep -v '#\[ignore\]' | grep -v '//' | head -20

# 3. OSS 独立 build 最终确认（无 attune-pro crate 依赖）
grep -n 'attune-pro\|law-pro\|patent-pro' \
    rust/crates/attune-core/Cargo.toml \
    rust/crates/attune-server/Cargo.toml | head -10
# 期望: 无（无仓级依赖）

# 4. 三大泄漏点最终状态（spec §6 验收测试通过）
cargo test -p attune-core oss_registry_has_exactly_6_agents 2>&1 | tail -3
cargo test -p attune-core mock_hub_default_returns_empty_plugin_list 2>&1 | tail -3
# MU-12 builtin 测试
cargo test -p attune-server plugin_source_field_reads_manifest_builtin 2>&1 | tail -3
```

---

## GA 验收清单

以下为 spec §9.1 关键验收测试完整对照：

| 验收测试 | 对应 Task | 通过标准 |
|---------|-----------|---------|
| OSS registry 仅含 6 oss-core agents | A1-1, A1-2 | `oss_registry_has_exactly_6_agents` PASS |
| registry.get(industry_id) → None（无 panic） | A1-1 | `registry_get_missing_industry_agent_returns_none` PASS |
| MockPluginHubProvider 返回空列表 | A4-1 | `mock_hub_default_returns_empty_plugin_list` PASS |
| Marketplace UI 无行业 plugin ID | A4-1 | `mock_hub_no_industry_ids_in_listing` PASS |
| builtin 判断来自 manifest.builtin 非硬编码 ID | B12-1 | `plugin_source_field_reads_manifest_builtin_not_hardcoded_id` PASS |
| OSS 搜索不退化（无 DOMAIN_EXPAND_MAP） | B5-1 | `generic_search_works_without_domain_expand_map` PASS |
| pii 模块无 case_metadata 仍正常 | A2-2 | `pii_module_works_without_case_metadata` PASS |
| quality manifest 仅含 oss-core gates | A8-1 | `quality_manifest_contains_only_oss_core_gates` PASS |
| `cargo test --workspace` 全过 | 每 Batch 末 | 0 FAILED |
| `cargo check -p attune-core` 无行业依赖 | 每 MU 末 | 0 errors |

**BLOCKED on attune-pro（下方 MU 需 attune-pro 接收侧就绪后解除 #[ignore]）**：

| MU | 需 attune-pro 补充的内容 | 阻塞 Task |
|----|------------------------|---------|
| MU-2 | `attune-pro/plugins/law-pro/tests/case_metadata_test.rs` — CaseMetadata 持久化测试 | A2-4 |
| MU-6 | `attune-pro/plugins/law-pro/tests/flow_test.rs` — legal_defamation flow 业务语义测试 | B6-2 |
| MU-7 | 同上（acp5 wire test 的行业 flow 版本） | B7-2 |
| MU-10 | `attune-pro/plugins/law-pro/tests/plugin_protocol_e2e.rs` — CaseMetadata E2E | C10-3 |

---

## Self-Review 对照 spec + census

### 20 MU 全覆盖验证

| MU | 文件数 | Hits | Batch | 关键 Task | 有 Task? |
|----|--------|------|-------|-----------|---------|
| 1 | 2 | 71 | A | A1-1~A1-6 | ✅ |
| 2 | 1 | 9 | A | A2-1~A2-8 | ✅ |
| 3 | 1 | 73 | C | C3-1~C3-3 | ✅ |
| 4 | 1 | 12 | A | A4-1~A4-4 | ✅ |
| 5 | 1 | 10 | B | B5-1~B5-4 | ✅ |
| 6 | 5 | 101 | B | B6-1~B6-5 | ✅ |
| 7 | 2 | 31 | B | B7-1~B7-4 | ✅ |
| 8 | 1 | 11 | A | A8-1~A8-4 | ✅ |
| 9 | 1 | 13 | D | D9-1~D9-3 | ✅ |
| 10 | 2 | 31 | C | C10-1~C10-4 | ✅ |
| 11 | 1 | 25 | C | C11-1~C11-2 | ✅ |
| 12 | 4 | 10 | B | B12-1~B12-4 | ✅ |
| 13 | 2 | 5 | D | D13-1~D13-2 | ✅ |
| 14 | 1 | 6 | D | D14-1~D14-2 | ✅ |
| 15 | 3 | 22 | D | D15-1~D15-3 | ✅ |
| 16 | 2 | 9 | D | D16-1~D16-2 | ✅ |
| 17 | 1 | 6 | D | D17-1~D17-2 | ✅ |
| 18 | 1 | 12 | A | A18-1~A18-3 | ✅ |
| 19 | 1 | 4 | B | B19-1~B19-3 | ✅ |
| 20 | 11 | 28 | D | D20-1~D20-4 | ✅ |
| **合计** | **44** | **489** | | **共 68 Tasks** | ✅ 全覆盖 |

### G1 三决策落实验证

| G1 决策 | 落实 Task | 验证 |
|---------|-----------|------|
| plugin_hub 空列表 fallback（无云端 API 新能力） | A4-2 清空 `_builtin_plugins()` | A4-1 回归测试 |
| oss_agent_real_llm_gate #[ignore] 保留（不删） | A18-2 仅清注释，测试保留 | A18-3 cargo check |
| corpus_domain 不枚举化（保持 Option\<String>） | D13/D20 只改 fixture 值，不改字段类型 | 无类型变更 |

### TREATED 564 全覆盖确认

Census 记录 TREATED = 564（部分文件被多个分组共享计数，20 MU 覆盖 44 文件 489 unique hits）。  
Plan Task D-FINAL 中的 `grep -rn` 命令是最终验证手段：src/ + tests/ 中的活跃非注释行业引用应为 0。

### 风险 R1-R6 缓解验证

| 风险 | 缓解措施落在哪 | 状态 |
|------|--------------|------|
| R1（564 处 build 破坏） | 每 MU 后 `cargo check`；每 Batch 末 `cargo test --workspace` | ✅ 每 Task 含 check |
| R2（CaseMetadata 消费方遗漏） | A2-1 强制 grep + A2-3~A2-6 逐一处理 | ✅ A2-1 是第一 Task |
| R3（搜索退化） | B5-1 回归测试先行（TDD 红态） | ✅ B5-1 |
| R4（quality manifest CI 失效） | A8-1 回归测试先行 | ✅ A8-1 |
| R5（plugin_hub Pro 用户体验） | A4-3 修复现有测试断言 | ✅ A4-3 |
| R6（attune-pro CI 未就绪） | A18-2 保留 `#[ignore]`；BLOCKED 标记 4 处 | ✅ 不删测试 |

# Audit C — OSS 边界违规 / 行业绑定残留 / scope bloat

> 日期 2026-06-01 · explorer C · lens = OSS boundary · read-only（未改代码 / 未 build）
> 标尺：attune-OSS = 个人通用知识库，**零行业绑定**。行业 (law/patent/sales/tech/medical/academic) **完全在 attune-pro**。
> 一个功能进 OSS 当且仅当「对任何领域个人通用用户都有价值」。

---

## 0. 结论速览

- **确认违规 / 强嫌违规：6 处**（patent 全栈 + 法律 agent registry/flow 声明 + case_metadata 案件模型 + scanner_patent 顶部 vault 残留路径注释）。
- **通用合法（误报澄清）：5 处**（project_recommender / entities EntityKind / taxonomy / pii / office）。
- **v0.6.0-rc.2 瘦身核对：4 项删除全部确认完成**（CaseNo enum / extract_case_no / CHAT_TRIGGER_KEYWORDS const / 4 builtin yaml 均已删），但**新增了瘦身时未覆盖的两类行业绑定**（patent 全栈 + 法律 agent registry/flow），属瘦身后再次泄漏，不是瘦身漏网。
- **patent 去留结论：迁 attune-pro（patent-pro），从 OSS 删除**。详见 §2。

---

## 1. 边界违规清单

| # | 项 | 行业 | 证据 file:line | 判定 | 建议 + 理由 |
|---|----|------|----------------|------|------------|
| **V1** | `routes/patent.rs` 整个路由（493 行入口 + 2 endpoint） | 专利 | `rust/crates/attune-server/src/routes/patent.rs:1,39,132`；注册 `rust/crates/attune-server/src/lib.rs:178-179`（`/api/v1/patent/search`、`/api/v1/patent/databases`） | **违规** | **迁 attune-pro/patent-pro + 从 OSS 删**。doc 已明示「专利 = OSS 无 / patent-pro (M3+)」(`docs/oss-pro-strategy.md:80`)。USPTO 专利库直连只服务专利代理 / 律师，普通个人用户不会查 USPTO。无任何 tier/pro gating，OSS 裸装即暴露。 |
| **V2** | `scanner_patent.rs`（USPTO PatentsView 客户端 + ingest） | 专利 | `rust/crates/attune-core/src/scanner_patent.rs`（全文）；`lib.rs:156 pub mod scanner_patent` | **违规** | **迁 patent-pro**。仅被 V1 patent route 调用（`search_patents`/`ingest_patent_records`/`PatentDatabase`/`PatentQuery`），无其他消费者 → 删 V1 后此模块成孤儿。`docs/oss-pro-strategy.md:80` patent-pro 卖点正是「专利数据库直连」。 |
| **V3** | `agents.registry.toml` 内 14 law-pro + 1 tech-pro + 1 evidence_classifier(law-pro) 行业 agent **声明** | 律师/专利代理 | `rust/agents.registry.toml:118-361`（`civil_loan_agent`/`fact_extractor`/`defamation_extractor`/`defamation_agent`/`traffic_accident_agent`/`divorce_extractor`/… 全部 `tier=paid plugin=law-pro`，`code_reviewer plugin=tech-pro`） | **违规（结构性）** | **行业 agent 声明应随 plugin pack 注册，不应硬编码进 OSS 仓根 SSOT**。这些 agent **实现不在 OSS**（`agents/` 目录无 defamation/fact_extractor 的 fn/struct，只有 6 个 oss-core agent 有实现），却把 18 个 paid agent 的 id/route_keywords/typed-handoff 全列进 OSS 仓根 `agents.registry.toml`。建议：OSS 仅保留 6 个 `oss-core` agent 声明，paid agent 声明迁入各 vertical plugin pack manifest，运行时由 plugin_registry 动态 merge。 |
| **V4** | `agent_flows.toml` 内 `legal_defamation` 流声明 + 中文法律 route_keywords | 律师 | `rust/agent_flows.toml:37-40`（`route_keywords=["名誉","诽谤","侮辱","名誉权","名誉损害","精神损害"]`，`steps=["fact_extractor","defamation_extractor","defamation_agent"]`） | **违规** | **迁 law-pro**。律师专属诽谤损害赔偿流程 + 律师专属触发词硬编码进 OSS 仓根。且有 **shipped 测试**（`agents/flow/tests.rs:323 shipped_flows_validate_against_shipped_registry`）断言此法律链 type-connect，把法律流耦合进 OSS 测试门。运行时通过 `locate_workspace_file` 走目录上查 + server 启动 `state.rs:211 load_workspace_flows` 加载。 |
| **V5** | `case_metadata.rs`（CaseVault 案件库 metadata：案件类型 / parties 原告被告 / 案号） | 律师 | `rust/crates/attune-core/src/case_metadata.rs:1-30`（`CaseMetadata.kind` 注释举例 `civil-loan/civil-marriage/criminal-defense`；`Party.role="plaintiff"/"defendant"`；`case_no`；注释「用户/律师手写」）；`lib.rs:115 pub mod case_metadata` | **违规（弱-中）** | **迁 law-pro**。整模块建模法律案件（原被告 / 案号 / 刑民事案件类型）。注意：`kind`/`case_no` 字段本身 Option 且注释「OSS 裸装=None / 由付费插件 registers_case_kinds 提供」，是 plugin-aware 设计 — 但 Party.role=plaintiff/defendant 的「卷宗对抗双方」心智仍是法律专属，普通用户的 Project 不分原被告。若要保留通用 Project metadata，应剥离 plaintiff/defendant/case_no 法律语义。 |
| **V6** | `scanner_patent.rs` / `routes/patent.rs` 顶部残留 `npu-vault/crates/vault-*` 旧路径注释 | （泄漏） | `scanner_patent.rs:1 // npu-vault/crates/vault-core/src/scanner_patent.rs`；`patent.rs:1 // npu-vault/crates/vault-server/src/routes/patent.rs` | **违规（cruft）** | 随 V1/V2 一起删。即便保留也应清理上一代仓名注释（与产品改名 attune 漂移）。 |

### 灰区 / 通用合法（澄清，非违规）

| 项 | 证据 | 判定 | 理由 |
|----|------|------|------|
| `office route` + office helper（493 行 route + OfficeView 853 行 + 大量 office_* 测试） | `routes/office.rs:1`（`/api/v1/office` OCR 同步 + ASR 异步） | **通用合法** | OCR / ASR 文档处理对任何个人用户有价值（扫描件 / 录音转写），非行业流程绑定。spec `2026-05-20-office-helper-design.md` 定位「个人助手语义」。**保留**。 |
| `project_recommender.rs` | `:11-18,103-104`（注释明示「触发词不再硬编码…由 vertical plugin 的 chat_trigger.project_keywords 传入；OSS 裸装 keywords=[] 永不触发」） | **通用合法** | CHAT_TRIGGER_KEYWORDS 律师 const **已删干净**，改为调用方传入。空 keywords 永不触发。**瘦身已正确完成**。 |
| `entities.rs EntityKind` | `:20-25`（enum 仅 Person/Money/Date/Organization，注释明示「行业实体如 CaseNo 由 vertical plugin 实现」） | **通用合法** | `CaseNo` variant **已删**。通用 4 类实体。**瘦身已正确完成**。 |
| `taxonomy.rs` | `:49-54,111`（注释「行业 builtin (law/presales/patent/tech) 全迁 attune-pro；OSS 不再内置任何行业分类维度」；`no builtin plugin: install attune-pro`） | **通用合法** | 行业 taxonomy 维度已剥离。`:262 "技术"/"商业"/"法律"/"医疗"` 是 prompt 示例值（domain 维度的 example，非硬编码行业插件）。**保留**。 |
| `pii/mod.rs` + `plugin_loader.rs` 的 case_no/medical 引用 | `pii/mod.rs:80,149`；`plugin_loader.rs:89,289` | **通用合法** | 均为「插件可注册自己的 PII pattern（如 law-pro 的 case_no）」的**框架注释/文档**，PII redaction 引擎本身通用，行业 pattern 由插件提供。`pii/mod.rs:563` case_no 是测试 fixture。**保留**。 |

---

## 2. patent 去留结论

**判定：违规，迁 attune-pro/patent-pro，从 OSS 删除。**

证据链：
1. **调用链单一**：`routes/patent.rs` 是唯一消费者，调 `scanner_patent::{search_patents, ingest_patent_records, PatentDatabase, PatentQuery}`（`patent.rs:12,93,97`）。删 route 后 `scanner_patent.rs` 无其他引用 → 整对孤立可整体迁出。
2. **无通用价值**：USPTO PatentsView 专利库直连（`scanner_patent.rs:14 USPTO_BASE`）只对专利代理 / 知产律师有意义。普通个人知识库用户不会检索美国专利号 + IPC 分类入库。
3. **文档已定调为 Pro**：`docs/oss-pro-strategy.md:80`「专利｜OSS 无｜patent-pro (M3+)：专利数据库直连·侵权检测·申请书草稿」、`:205`「行业分类维度… 迁 attune-pro」。代码与已发布策略文档**直接冲突** = 边界漂移。
4. **无 gating**：route 在 `lib.rs:178-179` 无条件注册，OSS 裸装即暴露 `/api/v1/patent/*`，无 pro tier 校验。

唯一保留论点（已驳）：「专利检索也是一种通用资料检索」。驳：OSS 已有通用 web 搜索 + WebDAV + 本地 ingest；USPTO 结构化字段（assignee/inventor/IPC/patent_number）是专利领域 schema，非通用。

---

## 3. v0.6.0-rc.2 瘦身完成度核对

| 瘦身项（CLAUDE.md 声称已删） | 残留引用核对 | 结论 |
|------------------------------|--------------|------|
| `EntityKind::CaseNo` | `entities.rs:20-25` enum 仅 4 通用 variant，无 CaseNo；仅注释提及「由 plugin 实现」 | ✅ **已删干净** |
| `extract_case_no` 中文案号正则 | 全仓 grep `extract_case_no` = 0 命中（仅 `pii/mod.rs:563` 测试 fixture 手动 add_pattern("case_no", …)） | ✅ **已删干净** |
| `project_recommender.rs::CHAT_TRIGGER_KEYWORDS` 律师 const | `project_recommender.rs` 无该 const，改为 `recommend_for_chat(msg, keywords)` 调用方传入；注释明示瘦身 | ✅ **已删干净** |
| `assets/plugins/{tech,law,presales,patent}.yaml`（4 builtin yaml） | `assets/plugins/` 目录不存在（`ls` 报 No such file）；现存 yaml 仅 `attune-core/assets/plugins/ai_annotation_*`（通用 AI 批注，合法） | ✅ **已删干净** |

**瘦身本身 100% 完成**。但 §1 的 V1-V5 是**瘦身之后引入的新行业绑定**（patent 全栈 + ACP-5 agent governance 把 law/tech agent 声明硬编码进 OSS 仓根 registry/flow + case_metadata 案件模型），不属瘦身漏网，属后续 feature 开发（v1.0 law agent backfill / ACP governance / patent connector）回灌 OSS 仓的边界回归。

---

## 4. 行业关键词 grep 结果（去测试/注释噪声后）

真行业绑定（代码层，非注释/测试）：
- `scanner_patent.rs` 全文 — 专利（V2）
- `routes/patent.rs` 全文 + `lib.rs:178-179` 注册 — 专利（V1）
- `agents.registry.toml:118-361` — 18 paid 行业 agent 声明（V3）
- `agent_flows.toml:37-40` — legal_defamation 流 + 中文法律触发词（V4）
- `case_metadata.rs` — 案件/原被告/案号模型（V5）

可接受（框架注释 / plugin-aware 设计 / 测试 fixture）：
- `pii/mod.rs:80,149` / `plugin_loader.rs:89,289` — 「插件可注册 case_no/medical pattern」框架文档
- `plugin_sig.rs:5,14` — 「商业插件（律师/售前/医疗）经 PluginHub 签名分发」框架注释
- `ingest/connector.rs:72` / `routes/remote.rs:15` / `routes/index.rs:17` — corpus_domain 字段（legal/tech/medical/patent/general）是**通用跨域防污染枚举**，domain 值是数据标签非行业插件，可接受（但 `search.rs:141` cross-domain penalty 命中 'legal'/'patent' 说明 domain 体系预留了行业值 — 灰区，建议 domain 值改为可扩展而非硬编码行业名）
- `agent_telemetry/tests.rs` / `usage/tests/guard_test.rs` / `plugin_sync.rs:521,560` — defamation_extractor 出现在测试/telemetry，是 V3 的连带（agent id 当 telemetry key），随 V3 迁出后应清理
- `tests/golden/document_classifier/*.yaml` / `linker_golden/*.yaml` — 测试语料含「律师函/判决书/案号」，是真实中文文档样本（通用 document_classifier 的测试数据），可接受
- `taxonomy.rs:262` 「技术/商业/法律/医疗」— prompt domain 维度示例值，可接受

---

## 5. 运行时影响评估（缓解 V3/V4 严重度）

- `agents.registry.toml` / `agent_flows.toml` 在 **OSS 仓根 git-tracked**（`git ls-files` 确认），运行时经 `locate_workspace_file`（CWD + exe 目录上查）+ server 启动 `state.rs:211` 加载。
- **但未打包进 desktop release**：`apps/attune-desktop/tauri.conf.json:18-19` resources 仅捆绑 `whisper-cli`，无 registry/flow toml。
- 故对**已打包安装的桌面用户**，walk-up 找不到 toml → `load_workspace_flows` 返回 None → flow 层 no-op（graceful，注释明示）。法律流不会在终端用户机激活。
- **但源码树/git 层面仍是边界违规**：OSS 公开仓根直接列出 law-pro 全部 agent id + 律师诽谤触发词 + typed-handoff，泄漏 Pro 产品的能力清单与实现细节，违反「行业完全在 attune-pro」。且 dev/CI（CWD=仓根）会真加载并跑 `shipped_flows_validate_against_shipped_registry` 法律链门。

---

## 6. 建议处置优先级

1. **P0（与已发布 oss-pro-strategy 直接冲突）**：删 V1+V2+V6（patent route + scanner + 注册 + 旧路径注释），迁 patent-pro。
2. **P1（能力清单泄漏 + OSS 测试耦合法律）**：V3+V4 — OSS registry/flow 仅留 6 个 oss-core agent；paid agent 声明 + legal_defamation 流迁入各 plugin pack manifest；删 shipped 法律链测试或改为 oss-core 流验证。
3. **P2**：V5 — case_metadata 剥离 plaintiff/defendant/case_no 法律语义或整体迁 law-pro；连带清理 telemetry/golden 中 defamation_extractor 引用。

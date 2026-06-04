---
name: oss-boundary-realignment
version: v0.1.0-spec
status: DRAFT
date: 2026-06-01
authors: qiurui144 + Claude
template_version: 1
---

# Spec: OSS 边界归位 — patent 全栈迁出 + 行业 agent 声明/流/测试解耦

> 把审计 P0 两条「行业代码回灌 OSS」归位：从 OSS attune 删除 patent 可执行能力（route + scanner），
> 并把 18 个 paid 行业 agent 声明 + legal_defamation 流 + 律师触发词 + 案件法律模型移出 OSS SSOT/测试门，
> 使 OSS 仓恢复「零行业绑定」北极星。能力本身不丢失 —— 概念上落到 attune-pro（cross-repo assumption，本 sprint 不操作 pro 仓）。

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
- [Appendix A — 代码勘查事实表](#appendix-a--代码勘查事实表-fileline-ground-truth)

---

## 1. 目标定位

### 解决的痛点

审计（`reports/2026-06-01_audit-C-oss-boundary.md`）拉出 OSS attune 仓内 **6 处行业绑定残留**（V1–V6），
其中两条是 **P0**：行业（专利 / 律师）能力以**可执行代码 + SSOT 声明 + 测试门**形态回灌进 OSS 公开仓，
与**已发布**的 `docs/oss-pro-strategy.md:80,205` 策略文档**直接冲突**。具体危害：

1. **专利全栈裸暴露**：OSS 裸装即注册 `/api/v1/patent/search` + `/api/v1/patent/databases`（`lib.rs:178-179`，无 pro/tier gating），
   USPTO PatentsView 直连只服务专利代理 / 知产律师，对「任何领域个人通用用户」无价值 —— 违反 OSS 北极星。
2. **能力清单泄漏**：OSS 仓根 `agents.registry.toml:118-361` 列出 18 个 `tier=paid` 行业 agent 的 id / route_keywords / typed-handoff，
   `agent_flows.toml:37-40` 列出律师专属诽谤损害流 + 中文触发词（名誉/诽谤/侮辱/名誉权/名誉损害/精神损害）。
   公开仓泄漏 Pro 产品能力清单与实现细节。
3. **OSS 测试门耦合法律链**：`flow/tests.rs:323`、`acp5_chat_flow_wire_test.rs:68`、`cli_agent_flow_smoke.rs:29` 等
   **多个 OSS 测试**硬断言 legal_defamation 流 type-connect —— OSS CI 脆弱性绑死在律师 domain schema 上。

### 与产品 positioning 对齐

attune-OSS 北极星 = **零行业绑定的通用知识库**；判定标尺 = 「对任何领域个人通用用户都有价值」才留 OSS
（CLAUDE.md「OSS attune 边界规则」+ `oss-pro-strategy.md` v2 §4.3）。本 sprint 是 v0.6.0-rc.2 边界瘦身的**延续**
（瘦身删了 CaseNo/extract_case_no/CHAT_TRIGGER_KEYWORDS/4 yaml，本次清的是瘦身**之后**由 v1.0 law backfill / ACP-5 governance / patent connector 回灌的新债）。

### 与全局/项目 CLAUDE.md 规则映射

| 规则 | 映射 |
|------|------|
| 全局 §3.1 架构级设计铁律（rename/迁移类 → spec-first） | 本 spec 即 spec-first；§10 migration + §11 risk 为重头 |
| 项目「OSS attune 边界规则」 | §1/§2 判定标尺；patent + 行业声明完全在 attune-pro |
| 项目「Rust 商用线约定」API path 命名（旧 path alias 1 release 周期） | §5 决定 patent 是否走 deprecation alias |
| 全局 §4.2.3（多文件 reset/stash conflict-marker） | §11 R8；多文件删除/改动 commit 纪律 |
| 项目「Agent 验证铁律」+ §6.1 6 类测试下限 | §9 测试矩阵以**回归 + grep gate** 为核心 |
| 全局 §1.1.7 Pre-Create Gate | 已执行：无同主题 spec、kebab-case、长期 SSOT（见 Appendix A.7） |

---

## 2. 范围边界

### 关键判断：1 sprint 双 sub-scope（**建议**），不拆 2 minor

**建议：单 sprint `oss-boundary-realignment`，内含 2 个 sub-scope（SS-A patent 迁出 / SS-B 行业声明+流+测试解耦），合并为 1 个 minor（建议 v1.0.x 下一可用号，按 merge 顺序拿号 per §7.1.7）。**

理由：
1. **同一主题、同一标尺、同一审计来源**：两条 P0 都是「行业代码回灌 OSS」的同一类债，由同一份 audit-C 拉出，共享 §1 判定标尺与 §11 风险族（跨仓协调 / 测试解耦）。拆 2 minor 会让 RELEASE.md 出现两条语义重复的 minor，违反「每 minor = distinct deliverable」的反面（强行拆同质 deliverable）。
2. **耦合点交叉**：SS-A 删 patent route 后，`scanner_patent.rs` 成孤儿（仅 V1 调用，Appendix A.2）；SS-B 删 registry 声明后，OSS 测试门需重锚 —— 两者都触碰 OSS 测试绿/grep gate（§9），统一一次跑回归比分两次更省 wall-clock 且避免中间态（删了 patent 但 registry 还泄漏，反之亦然）半归位的尴尬发布。
3. **风险可控**：合并改动文件 ≤ ~12 个（Appendix A 全列），单 worktree 单 PR 即可；不构成「跨多 day 大 feature」必须拆的体量。
4. **何时该拆的反证（已排除）**：若 patent 迁移需**先**等 attune-pro/patent-pro 接住才能删（硬前置依赖），SS-A 会被 cross-repo blocker 卡住，此时应拆出 SS-A 为独立 minor 等 pro 就绪。但本仓**无法验证** pro 状态（跨仓硬约束），故按 §10 sequencing 处理为「assumption + 协调点」：若协调点确认 pro 未就绪 → SS-A 降级为「软删 + feature-flag off」（见 §10），仍可与 SS-B 同 sprint。

### v1.0.x（本 sprint，SS-A patent 迁出）做

- 从 OSS 删除 `routes/patent.rs`（整文件）+ `lib.rs:178-179` 两条 route 注册 + `routes/mod.rs` 中 `patent` 模块声明。
- 从 OSS 删除 `attune-core/src/scanner_patent.rs`（整文件）+ `attune-core/src/lib.rs:156 pub mod scanner_patent`。
- 删 `scanner_patent.rs:1` / `patent.rs:1` 顶部旧仓名注释 `npu-vault/...`（V6 cruft，随迁一起清）。
- 同步删 README.md:56 / README.zh.md:41 的 USPTO patent search feature 行（doc 漂移），README.md:55,160 的 patent 行业引用改为「由 attune-pro 提供」措辞。
- 决定老 client 行为：**硬删 404**（见 §5 理由：grep 确认无 OSS client 调用）。

### v1.0.x（本 sprint，SS-B 行业声明/流/测试解耦）做

- `agents.registry.toml`：**仅保留 6 个 `plugin=oss-core` agent 声明**（`:25-113`），删除 14 law-pro + 1 tech-pro + 1 evidence_classifier 共 18 个 `tier=paid` 声明（`:115-361`）。
- `agent_flows.toml`：删除 `legal_defamation` 流（`:37-47`）及其律师触发词。文件保留为「OSS 无内置行业流」状态（空 flow 集合或仅含 oss-core 通用流，见 §6）。
- OSS 测试解耦：把硬断言 legal_defamation 的 OSS 测试**重锚到 OSS-only fixture**（inline toml，不依赖 shipped 行业声明）或删除：`flow/tests.rs:323 shipped_flows_validate_against_shipped_registry`、`acp5_chat_flow_wire_test.rs:60-71,77-`、`cli_agent_flow_smoke.rs:25-`、连带 `flow/tests.rs:341,878,909,927` 等用法。
- 清理 telemetry / golden 中 `defamation_extractor` 作为 telemetry key 的连带引用（`agent_telemetry/tests.rs`、`usage/tests/guard_test.rs`、`plugin_sync.rs:521,560`），改用 OSS agent id（如 `document_classifier`）作 fixture key。
- grep gate：CI 加守卫，确保 OSS 仓根代码层（非注释/测试 fixture）不再出现行业关键词（patent / 名誉 / 诽谤 / 原告 / 被告 / plaintiff / defendant + paid agent id）。

### 不做（写死，禁止 silent scope creep）

- **不**改 V5 `case_metadata.rs` 的字段结构（plaintiff/defendant/case_no 剥离）。audit 判 V5 为 **P2（弱-中）**，且 `kind`/`case_no` 已是 plugin-aware（Option，OSS=None）；剥离 Party.role 法律语义需重设计通用 Project metadata schema，属独立 spec。本 sprint 仅在 §6/§11 标记为后续。
- **不**操作 attune-pro 私有仓（跨仓硬约束）。patent/行业代码「落到 attune-pro 哪里」= 外部 cross-repo 依赖，仅在 §6/§10/§11 标 assumption + 协调点。
- **不**改 `corpus_domain` 枚举（legal/tech/medical/patent/general）—— audit §4 判为「通用跨域防污染枚举」，domain 值是数据标签非行业插件，灰区保留（仅 §11 标注后续可改为可扩展）。
- **不**删 P2 冗余项（capture/ / mcp_client.rs / verify_strict / html_to_text 合并）—— 走 §5.1 小改路径独立 develop commit，不进本 sprint。
- **不**改任何 OSS 通用能力（vault/ingest/search/chat/RAG/classify/cluster/office）。

### 推迟到 v.next

- V5 `case_metadata` 通用化重设计（剥离法律对抗语义 or 整体迁 law-pro）—— 独立 spec。
- `corpus_domain` 行业硬编码值改为可扩展注册 —— 独立 spec。
- patent-pro 在 attune-pro 仓的接收实现（cross-repo，由 attune-pro 仓自己的 spec/plan 承接）。

---

## 3. 架构数据流

### 删除前（OSS 当前 = 边界泄漏态）

```
                          ┌─────────────────────────────────────────────┐
                          │           OSS attune 仓 (公开)               │
 Chrome ext / Web UI ──► /api/v1/* router (lib.rs)                       │
                          │   ├─ /api/v1/patent/search   ◄── V1 违规      │
                          │   ├─ /api/v1/patent/databases◄── V1 违规      │
                          │   └─ ... 通用 endpoints                       │
                          │        │                                      │
                          │   routes/patent.rs ──► scanner_patent.rs ──► USPTO PatentsView (外网)
                          │        (V1)                (V2 孤儿候选)        │
                          │                                               │
                          │  SSOT toml (git-tracked, walk-up 加载):       │
                          │   agents.registry.toml  [6 oss-core | 18 paid]◄── V3 泄漏
                          │   agent_flows.toml      [legal_defamation]    ◄── V4 泄漏
                          │        │                                      │
                          │  state.rs:211 load_workspace_flows ─► chat 路由 (acp_chat)
                          │        │                                      │
                          │  OSS 测试门 (CI, CWD=仓根):                    │
                          │   flow/tests.rs / acp5_chat_flow_wire / cli_* ◄── 硬断言 legal_defamation
                          └─────────────────────────────────────────────┘
```

### 删除后（OSS 目标 = 零行业绑定）

```
                          ┌─────────────────────────────────────────────┐
                          │           OSS attune 仓 (公开)               │
 Chrome ext / Web UI ──► /api/v1/* router (lib.rs)                       │
                          │   └─ 仅通用 endpoints                          │
                          │      (patent route 已删 → 404 unknown route)  │
                          │                                               │
                          │  scanner_patent.rs ── 已删 (整文件移出)        │
                          │                                               │
                          │  SSOT toml:                                   │
                          │   agents.registry.toml  [仅 6 oss-core]       │
                          │   agent_flows.toml      [无行业流 / 仅 oss 流] │
                          │        │                                      │
                          │  state.rs:211 load_workspace_flows ─► 加载 6 oss-core；
                          │        无行业流 → chat 走 free-form RAG (graceful)
                          │                                               │
                          │  OSS 测试门: inline OSS-only fixture 验证       │
                          │   (不再依赖 shipped 行业声明)                  │
                          └─────────────────────────────────────────────┘

   ┌──── cross-repo assumption (本 sprint 不操作, §10 协调点) ────┐
   │  attune-pro/patent-pro:  接收 patent route + scanner_patent  │
   │  attune-pro/<vertical>-pro manifest: 接收 18 paid agent 声明  │
   │  attune-pro/law-pro:     接收 legal_defamation 流 + 触发词     │
   └──────────────────────────────────────────────────────────────┘
```

### DB / cache / 状态影响

- **无 schema 变更、无 migration DDL**。patent 入库走的是**通用** `store.insert_item(..., "patent", ...)`（`scanner_patent.rs:171-179`），仅 `source` 字段值为字面量 `"patent"`，无 patent 专属表。
- **已入库的 patent items**：以普通 item 形态留在 vault（content + embedding 已生成），删 route/scanner **不删用户已有数据**（见 §10）。
- **cache 无影响**：摘要缓存按 chunk_hash，与来源无关。

---

## 4. 模块边界

| 改动 | crate / module / file | 操作 |
|------|----------------------|------|
| SS-A | `attune-server/src/routes/patent.rs` | **删整文件** |
| SS-A | `attune-server/src/routes/mod.rs`（`pub mod patent;`） | 删模块声明 |
| SS-A | `attune-server/src/lib.rs:178-179` | 删 2 条 `.route(...)` |
| SS-A | `attune-core/src/scanner_patent.rs` | **删整文件** |
| SS-A | `attune-core/src/lib.rs:156`（`pub mod scanner_patent;`） | 删模块声明 |
| SS-A | `rust/README.md:55,56,160` + `rust/README.zh.md:41` | 删/改 patent feature 引用 |
| SS-B | `rust/agents.registry.toml:115-361` | 删 18 paid agent 声明（保留 `:25-113` 6 oss-core） |
| SS-B | `rust/agent_flows.toml:24-47` | 删 legal_defamation 流 + 注释 |
| SS-B | `attune-core/src/agents/flow/tests.rs` | 重锚/删 legal_defamation 断言（`:323,341,878,909,927`） |
| SS-B | `attune-server/tests/acp5_chat_flow_wire_test.rs` | 重锚 OSS-only flow fixture（`:60-,77-`） |
| SS-B | `attune-cli/tests/cli_agent_flow_smoke.rs` | 重锚/删（`:25-,40-,55-`） |
| SS-B | `attune-core/src/agent_telemetry/tests.rs`、`usage/tests/guard_test.rs`、`plugin_sync.rs:521,560` | defamation_extractor telemetry key → OSS agent id |
| SS-B (CI) | `.github/workflows/*` 或 `scripts/` | 加 grep 守卫 step |

**不碰**（边界外）：`case_metadata.rs`（V5 推迟）、`corpus_domain` 枚举、`project_recommender.rs` / `entities.rs` / `taxonomy.rs` / `pii/mod.rs`（audit 已澄清通用合法）、`routes/office.rs`（通用合法）、所有 vault/ingest/search/chat 通用路径。

**跨仓边界（硬约束）**：attune-pro 接收 patent + 行业声明 = 外部依赖，本 sprint 仅声明 assumption，不读不写 pro 仓。

---

## 5. API 契约

### 被删除的 endpoint

```yaml
# 删除前 (OSS lib.rs:178-179)
removed_endpoints:
  - method: POST
    path: /api/v1/patent/search
    request:   # routes/patent.rs:19-34 PatentSearchRequest
      q:          string          # 必填, ≤500 bytes
      limit:      integer = 10    # 1..=20
      database:   string = "uspto"
      ipc_filter: string?         # 如 "G06F"
      auto_ingest: boolean = false
    response_200: { records: PatentRecord[], total: integer }   # patent.rs serde_json::Value
  - method: GET
    path: /api/v1/patent/databases
    response_200: { databases: [{ id: "uspto", name: "USPTO", desc: string }] }
```

### 老 client 行为 — **硬删 404，不做 alias**

```yaml
old_client_behavior:
  decision: hard_remove_404         # 不走 §API-path-命名 的 alias 1-release 宽限
  rationale:
    - grep 确认: OSS 内**无任何 client** 调用 /api/v1/patent
      (extension/ 0 命中; rust 内仅 lib.rs 注册 + README 文档; 见 Appendix A.1)
    - patent 是行业能力, 按 OSS 北极星本就不该对通用用户暴露 → 无「平滑迁移老用户」诉求
    - alias 宽限是为「OSS 内部已有 caller 需要时间迁移」, 此处无 caller, alias 纯增维护面
  after_removal:
    - 任意请求 /api/v1/patent/*  →  axum 默认 404 Not Found (unknown route)
    - 该能力的正确去处: 安装 attune-pro/patent-pro 后由 pro 注册同形 endpoint (cross-repo, §10)
```

### SS-B：声明/流删除对路由契约的影响

```yaml
flow_routing_contract:
  before:
    - chat message 命中 ["名誉","诽谤",...] → resolve_flow → legal_defamation (3-step)
  after:
    - 行业触发词不再命中任何 OSS 内置流 → resolve_flow 返回 None → chat 降级 free-form RAG
    - 这是既有 graceful 路径 (state.rs:209-210 注释明示; flow.rs 已支持 None)
  oss_core_flows_remain: true   # 6 个 oss-core agent 声明保留, 通用 agent run 路由不受影响
```

---

## 6. 扩展点 / 插件接口

### patent 能力如何在 Pro 重新接入（cross-repo assumption）

- **assumption A1**：attune-pro/patent-pro 在自己的 plugin pack 内提供 patent route + scanner，经 PluginHub 签名分发，运行时由 plugin loader 注册 `/api/v1/patent/*`（与 OSS 删除的形态同构，保证 Pro 用户能力不丢）。**本仓不验证、不实现**；列为 §11 R2 协调点。
- 现有扩展机制支撑：`routes/agents.rs::run_agent`（`lib.rs:173`）+ `plugin_registry` 动态 merge + `marketplace::install_plugin`（`lib.rs:176`）已是「paid agent 经插件注册」的运行时通道。

### 18 paid agent 声明的正确归宿

- OSS `agents.registry.toml` 仅留 6 oss-core；paid agent 声明应随各 vertical plugin pack 的 manifest 分发，运行时由 `load_workspace_flows` / plugin_registry **动态 merge**（registry.rs 已支持 from_path + validate_against，扩展为 merge 多源是 pro 仓职责）。
- **OSS 侧扩展点保证**：`load_workspace_flows` 容忍「仅 oss-core 声明」（无行业 agent）正常加载并验证；行业 agent 缺席时 chat 走 free-form RAG，不 panic（state.rs:223-226 已有 graceful 分支）。

### legal_defamation 流的归宿

- 迁 attune-pro/law-pro 的 plugin manifest（含触发词 + typed-handoff 链）。OSS `agent_flows.toml` 保留为「可被 pro 流 merge 的空/通用基底」。

### 后续通用 Project metadata 扩展点（V5 推迟项预留）

- 若后续通用化 `case_metadata`：通用 Project 用 `tags`/`notes`/通用 `parties`（无 plaintiff/defendant 法律语义），法律对抗双方语义由 law-pro 插件通过 `registers_case_kinds` 注入。**本 sprint 不实现**。

---

## 7. 错误 + 边界 case

| 错误码 (kebab) | 触发 | 行为 |
|---------------|------|------|
| `unknown-route` (HTTP 404) | 删除后请求 `/api/v1/patent/*` | axum 默认 404，无 patent handler |
| `flow-none-fallback` (非错误，info log) | 行业触发词 chat 命中但无内置流 | resolve_flow → None → free-form RAG（graceful，既有路径） |
| `registry-load-ok-6` (info log) | 启动加载仅 6 oss-core | `load_workspace_flows` 成功，`reg.len()==6`；不应 Err |
| `merge-conflict-marker` (build fail) | 多文件删除 reset/stash 残留 marker | §11 R8 强制 grep `^<<<<<<<` 守卫 |

### 边界 case 矩阵

- **空 registry / 空 flows**：删 paid 声明后 `agents.registry.toml` 仍 ≥ 6 条；`agent_flows.toml` 可能 0 flow → `FlowSet::from_toml_str("")` 必须返回空集而非 Err（验证 flow.rs 行为；若 Err 则需保留 1 个 oss-core 通用流占位）。
- **已入库 patent items**：vault 中 `source="patent"` 的旧 item 删 scanner 后仍可 search/被 RAG 引用（普通 item），不报错（§10）。
- **walk-up 找不到 toml**（打包桌面用户）：既有 graceful None（audit §5），删除后行为不变。
- **CLI `agent flow run legal_defamation`**：删流后该 CLI 调用应返回「flow not found」明确错误而非 panic（重锚 cli_agent_flow_smoke.rs 时验证）。

### graceful degradation

- 全链对「行业 agent/流缺席」已有 graceful 设计（state.rs / flow.rs None 分支），本 sprint 的删除**复用**该路径，不新增 fail-fast。

---

## 8. 成本契约

| 维度 | 估算 | 归属 |
|------|------|------|
| **磁盘** | **净减** ~700–900 LOC 源码（patent.rs ~493L + scanner_patent.rs ~360L + registry 246L 删除段 + flow 24L），二进制略减（删 USPTO reqwest blocking 路径）。无新增磁盘。 | 🆓 零成本 |
| **token** | **0**。纯删除 + 测试重锚 + 文档改，无 LLM 调用、无 embedding 重算。 | 🆓 零成本 |
| **wall-clock** | 工程实施估 ~3–5 小时（删文件 + 解耦 ~10 测试断言 + grep gate + 全量 `cargo test --workspace` 回归 ~5–10 min/轮 × 多轮）。**诚信声明**：此为估算，实际以 commit 时间戳为准（per §1.2）。 | ⚡ 本地算力（cargo build/test CPU） |
| **本地算力** | cargo 编译 + 测试（CPU）；无 GPU/NPU。 | ⚡ |

### 审计命令（用户可跑验证归位完成）

```bash
# 1. patent 残留 (应 0 命中, 排除 reports/)
grep -rn "scanner_patent\|/api/v1/patent\|PatentQuery\|USPTO" rust/ \
  --include="*.rs" --include="*.toml" --include="*.md" | grep -v reports/

# 2. paid 行业 agent 声明残留 (应仅匹配注释, 不匹配 [[agent]] tier=paid)
grep -n 'tier = "paid"' rust/agents.registry.toml          # 期望 0 行

# 3. 律师触发词 / legal flow 残留 (代码层, 应 0)
grep -rn "legal_defamation\|名誉\|诽谤\|侮辱" rust/agents.registry.toml rust/agent_flows.toml

# 4. OSS 测试门不含法律 case (重锚后, 应 0)
grep -rln "legal_defamation\|defamation_extractor" rust/crates/*/tests/ rust/crates/*/src/**/tests.rs

# 5. 全量回归绿
cd rust && cargo test --workspace
```

---

## 9. 测试矩阵

> 核心是**回归**：删后 `cargo test --workspace` 仍绿 + OSS 测试门不再含法律 case + grep gate 确认无残留行业关键词。

| 类型 (§6.1 6 类下限) | 用例 | 输入 | 期望 | 通过判据 |
|---------------------|------|------|------|---------|
| **happy path** | T1 通用 endpoint 不受影响 | 删 patent 后启动 server，调 `/api/v1/search` | 200 正常 | 通用功能 0 回归 |
| **happy path** | T2 registry 加载 6 oss-core | 启动 `load_workspace_flows` | Ok，`reg.len()==6` | info log + 测试断言 len |
| **edge case** | T3 空/仅 oss flow 集 | `agent_flows.toml` 删 legal_defamation 后加载 | Ok（空或仅 oss 流），不 Err/panic | `FlowSet` 解析成功；若空集 Err 则补占位流 |
| **edge case** | T4 已入库 patent item 仍可检索 | vault 含 `source="patent"` 旧 item，search 关键词 | 命中返回，无 scanner 依赖 | search 结果含该 item，无 500 |
| **error case** | T5 删后请求 patent endpoint | `POST /api/v1/patent/search` | 404 unknown-route | 状态码 == 404 |
| **error case** | T6 CLI 跑已删流 | `attune agent flow run legal_defamation` | 明确 "flow not found"，非 panic | 退出码 ≠ 0 + 可读错误 |
| **adversarial** | T7 行业触发词 chat 注入 | chat "他诽谤侮辱我，要求名誉权赔偿" | 不路由到行业流，降级 free-form RAG（无能力泄漏） | resolve_flow → None；无 legal flow 命中 |
| **adversarial** | T8 grep gate（关键词扫描） | 全仓 grep 行业关键词（代码层，排除注释/测试 fixture/reports/） | 0 命中 | §8 审计命令 1–4 全 0 |
| **concurrent / 多并发** | T9 启动期并发加载 registry | 多 server 实例同时 `load_workspace_flows` | 各自加载只读 toml，无竞争 | 并发测试无 panic/data race（只读文件） |
| **resource / 资源** | T10 二进制/编译资源 | 删后 `cargo build --release` | 编译通过，无 orphan 引用（scanner_patent 已无 caller） | build 0 error；clippy `-D warnings` 干净 |
| **回归 (重头)** | T11 全量 workspace | `cargo test --workspace` | 全绿（含重锚后的 flow/cli/server 测试） | 0 fail；新 `#[ignore]` 不突增（§7.2 Gate2） |
| **回归** | T12 测试门解耦验证 | OSS 测试集中搜 legal case 断言 | 重锚到 OSS-only inline fixture 或删除 | §8 审计命令 4 == 0 |
| **回归** | T13 first-parent / 文档漂移 | README patent 行已删，与代码一致 | 无 doc drift（§7.2 Gate1） | README grep patent == 仅「pro 提供」措辞 |

### 通过判据（量化）

- `cargo test --workspace` PASS rate == 1.00（deterministic，§7.2 Gate2 / Agent 验证铁律）。
- `cargo clippy --workspace --all-targets -D warnings` 干净。
- §8 审计命令 1–4 全部 0 命中（代码层；reports/ 与「pro 提供」措辞除外）。
- 新增 `#[ignore]` 个数 ≤ 既有 + 0（删除/重锚不应引入跳过）。

### multi-seed（per §2.3）

- 本 sprint 为 deterministic 删除/重构，**无 LLM 评估指标**，不涉及 multi-seed。T7 adversarial 的 resolve_flow 是确定性路由，单跑即可。

---

## 10. 向后兼容

### SemVer 策略

- patent endpoint **硬删除 = breaking change**（移除公开 API surface）→ 严格按 SemVer 应触发 minor bump（0.x 阶段 minor 表示 breaking 可接受）。归入下一可用 v1.0.x minor（按 merge 顺序拿号，§7.1.7）。RELEASE.md「Breaking」节必列：`/api/v1/patent/*` removed from OSS。
- registry/flow 行业声明删除：对 OSS 通用用户**无行为变化**（行业流本就 graceful no-op），但对源码/CI 是结构变更 → 同 minor。

### schema versioning

- **无 DB schema 变更、无数据迁移 DDL**。patent items 走通用 `items` 表 + `source` 字段，删 scanner 不改表结构。

### 老 client 行为

```yaml
old_client_matrix:
  - client: OSS 内部 (extension / web UI / CLI)
    calls_patent: false        # grep 确认 0 调用 (Appendix A.1)
    impact: 无
  - client: 直接 curl /api/v1/patent (理论外部脚本)
    after: 404 unknown-route
    migration: 安装 attune-pro/patent-pro 恢复同形 endpoint
  - data: vault 内已入库 patent items
    after: 保留为普通 item, 可 search / RAG 引用, 不丢失
```

### 能力不丢的 sequencing（worked old→new 例 + 前置依赖）

**关键前置依赖**：patent-pro 是否已在 attune-pro 接住 = cross-repo 协调点（本仓无法验证，§11 R2）。

```yaml
# Worked example — patent search 调用的 old → new 迁移
old:   # OSS v1.0.x-1 (删除前): OSS 裸装即可调
  request:  POST http://localhost:18900/api/v1/patent/search  {"q":"neural network","database":"uspto"}
  served_by: OSS attune routes/patent.rs → scanner_patent → USPTO
new:   # 本 sprint 后: OSS 不再提供
  request:  POST http://localhost:18900/api/v1/patent/search  {"q":"neural network"}
  oss_response: 404 Not Found (unknown-route)
  to_restore:
    step1: 安装 attune-pro/patent-pro plugin pack (经 PluginHub / marketplace install)
    step2: pro plugin 运行时注册同形 /api/v1/patent/search (cross-repo assumption A1)
    step3: 同请求由 pro plugin 服务 → 能力恢复, 数据仍在用户本地 vault

sequencing_decision_tree:
  - if  pro/patent-pro 已接住 (协调点确认):
      action: OSS 硬删 404 (本 spec 主路径)
  - elif pro/patent-pro 未就绪:
      action: SS-A 降级为「feature-flag off + 保留代码但默认不注册路由」一个 release 周期
              (避免 OSS 删了而 Pro 没接, 造成 Pro 用户能力真空)
              → 此时 SS-A 与 SS-B 仍同 sprint, 仅 patent 物理删除推迟到下个 minor
  - else (默认, 无法验证 pro 状态):
      action: 按主路径硬删, 在 RELEASE.md「Migration」明示「patent 用户需待 patent-pro 发布」
              并在 §11 R2 标 HIGH 协调点, merge 前必须人工确认 pro 状态
```

### RELEASE.md 必填节（per §1.1.4 / §7.2 Gate1）

- Highlights：OSS 边界归位（patent 迁出 + 行业声明解耦）。
- Breaking：`/api/v1/patent/*` removed from OSS。
- Migration：上面 worked example + sequencing。
- Known Limitations：patent 能力需 attune-pro/patent-pro（cross-repo，发布状态见协调点）。

---

## 11. 风险登记

| # | 风险 | 概率 | 影响 | 缓解 |
|---|------|------|------|------|
| **R1** | 删可执行 patent 能力 = breaking change，外部脚本/未知 client 直 curl `/api/v1/patent` 收 404 | Low | Med | grep 确认 OSS 内 0 caller（Appendix A.1）；RELEASE.md Breaking+Migration 明示；硬删前 §10 sequencing 决策树确认 |
| **R2** | **跨仓协调**：OSS 删了 patent 但 attune-pro/patent-pro **尚未接住** → Pro 用户能力真空 | Med | High | 本仓不可验证 pro 状态（硬约束）→ 列为 merge 前**人工确认协调点**；未确认则走 §10 降级路径（feature-flag off 保留代码一周期） |
| **R3** | 删 `agents.registry.toml` 18 paid 声明**破路由 / 破启动加载** | Med | Med | T2 验证 `load_workspace_flows` 仅 6 oss-core 仍 Ok；保留的 6 个 oss-core 声明完整；运行时行业 agent 缺席已 graceful（state.rs:223-226） |
| **R4** | **测试解耦遗漏**：legal_defamation 断言散落 ≥10 处（flow/tests、acp5_wire、cli_smoke、telemetry、guard、plugin_sync），漏改一处 → `cargo test` 红 | High | Med | Appendix A.4 全列引用点；§8 审计命令 4 grep gate 兜底；分文件逐个重锚到 OSS-only inline fixture；每改一文件即跑该 crate test |
| **R5** | OSS 测试**重锚后失去覆盖**（删 legal flow 测试后多步 flow 路径无测试） | Med | Med | 不删测试逻辑，**重锚**到 OSS-only 多步 flow inline fixture（如 oss-core 两步链）保留 flow executor / typed-handoff / governed-runner 覆盖；§9 T11/T12 验证 |
| **R6** | `agent_flows.toml` 删 legal_defamation 后**变空集**，`FlowSet` 解析 Err → 启动 fail | Low | Med | T3 验证空集解析；若 Err 则保留 1 个 oss-core 通用流占位（§6 扩展点） |
| **R7** | **scope creep 到 V5 case_metadata**：实施中顺手改案件模型 → 超 scope | Med | Low | §2 写死 V5 不做；case_metadata.rs 在「不碰」清单；超 scope = 新 spec |
| **R8** | **多文件删除 conflict-marker**（§4.2.3）：reset/stash/cherry-pick 重排残留 `<<<<<<<` 提进 commit | Med | Med | 每次 reset/stash 后强制 `grep -n "^<<<<<<<\|^=======$\|^>>>>>>>"` + `wc -l` + `git diff` 验证（§4.2.3 三步）；优先 cherry-pick 少用 stash+reset |
| **R9** | **doc 漂移**：删 route 但 README.md:56 / README.zh.md:41 patent feature 行未同步 → §7.2 Gate1 fail | Med | Low | §2/§4 列入 README 改动；§8 审计命令 1 含 .md；双语 README 同步（§1.1.3） |
| **R10** | **telemetry key 重命名引入静默 bug**：defamation_extractor 作为 telemetry/golden key 改 OSS id 时漏改一处 → 测试 fixture 不一致 | Med | Low | Appendix A.4 列 telemetry/guard/plugin_sync 引用点；改后跑对应 crate test 验证 |
| **R11** | **二进制行为变化**：删 scanner_patent 移除 reqwest blocking 路径，意外影响其他 blocking client | Low | Low | scanner_patent 自建独立 reqwest::blocking::Client（`:193`），无共享；grep 确认无其他模块引用其 client |
| **R12** | **grep gate 误杀通用合法引用**：CI grep 守卫把 corpus_domain="patent" / pii case_no 测试 fixture / taxonomy 示例值误判违规 | Med | Low | gate 精确匹配代码层（`tier="paid"` / `[[agent]]` / route_keywords 行业词），排除注释行/测试 fixture/golden 语料（audit §4 已分类哪些可接受） |

> 风险登记 ≥ 10 条（12 条），rename/迁移类 + 跨仓 + 测试解耦三大风险族齐全。

---

## Appendix A — 代码勘查事实表 (file:line ground truth)

> 以下为实际 Read/Grep 核实的事实（非盲信 audit 报告）。reports/ 为审计来源，本表为独立复核。

### A.1 patent route 注册 + caller 核实

| 事实 | 证据 |
|------|------|
| patent 2 route 无条件注册 | `rust/crates/attune-server/src/lib.rs:178-179`（`.route("/api/v1/patent/search", post(...))` + `databases`） |
| Chrome extension **无** patent 调用 | `grep -i patent extension/` → **0 命中** |
| OSS 内 `/api/v1/patent` 引用仅 = 注册 + 文档 + scanner 内部 URL | grep 全仓：`lib.rs:178,179`（注册）、`README.md:56`/`README.zh.md:41`（doc）、`scanner_patent.rs:7,16`（USPTO_BASE，非 OSS endpoint）。**无 client 调用** |
| patent.rs 是 scanner_patent 唯一消费者 | `patent.rs:12,60,71,93,97` 调 `search_patents`/`ingest_patent_records`/`PatentDatabase`/`PatentQuery`；全仓 grep 这些符号仅 `patent.rs` + `scanner_patent.rs` 自身（含测试 `:351,356,359`），**无第三方** |

### A.2 scanner_patent 模块

| 事实 | 证据 |
|------|------|
| 模块声明 | `rust/crates/attune-core/src/lib.rs:156 pub mod scanner_patent;` |
| USPTO 直连 | `scanner_patent.rs:16 const USPTO_BASE = "https://search.patentsview.org/api/v1/patent/"`；`:192 search_uspto`；`:193` 独立 reqwest::blocking::Client |
| 入库走通用 store（无 patent 专属 schema） | `scanner_patent.rs:163-188 ingest_patent_records` → `store.insert_item(dek, title, content, url, "patent", None, None)`（`:171-179`）+ 通用 `chunker::chunk` + `enqueue_embedding` |
| 旧仓名注释 cruft (V6) | `scanner_patent.rs:1 // npu-vault/crates/vault-core/...`；`patent.rs:1 // npu-vault/crates/vault-server/...` |

### A.3 registry / flow 声明 + 运行时加载

| 事实 | 证据 |
|------|------|
| 6 oss-core agent 声明（保留） | `agents.registry.toml:25-113`（document_classifier / memory_consolidation / linker / chat_reliability / self_evolving_skill / skill_evolution_cycle，全 `plugin="oss-core"`） |
| 18 paid 行业声明（删除段起点） | `agents.registry.toml:115` 注释「attune-pro law-pro (14, paid)」；`:117-130` civil_loan_agent `tier="paid" plugin="law-pro"`；至 `:361` |
| legal_defamation 流声明 | `agent_flows.toml:36-47`（`id="legal_defamation"`，`route_keywords=["名誉","诽谤","侮辱","名誉权","名誉损害","精神损害"]`，`steps=["fact_extractor","defamation_extractor","defamation_agent"]`） |
| 运行时加载 | `state.rs:211 attune_core::agents::load_workspace_flows("agents.registry.toml","agent_flows.toml")`；失败 → None → free-form RAG（`:223-226` graceful，不 panic） |
| graceful None 设计 | `state.rs:208-210` 注释「Absent files / parse / validation failure → None ... never panic — spec §11 R8」 |

### A.4 OSS 测试门耦合法律链（解耦目标点）

| 测试 | 证据 | 处理 |
|------|------|------|
| shipped flow 校验 | `flow/tests.rs:322-334 shipped_flows_validate_against_shipped_registry`（断言 `flows.get("legal_defamation")` + steps == 3 法律 agent） | 重锚 OSS-only inline fixture 或删 |
| defamation dedupe 路由 | `flow/tests.rs:340-358`（`resolve_flow("他诽谤侮辱我...")` → `legal_defamation`）；连带 `:878,909,927`（"他诽谤我"/"诽谤案"/route 用法） | 重锚/删 |
| chat wire | `acp5_chat_flow_wire_test.rs:60-71`（`flows.get("legal_defamation").is_some()`）+ `:77-`（defamation message 跑 governed runner，`:111 out.flow_id=="legal_defamation"`） | 重锚 OSS-only flow |
| CLI smoke | `cli_agent_flow_smoke.rs:29-34`（stdout 含 legal_defamation/CaseFacts/DefamationFacts/defamation_*）+ `:42,48,60`（run legal_defamation） | 重锚/删 |
| telemetry / guard / sync 连带 | `agent_telemetry/tests.rs`、`usage/tests/guard_test.rs`、`plugin_sync.rs:521,560`（defamation_extractor 作 key） | key 改 OSS agent id |
| legal_defamation/defamation 全仓出现 | grep files_with_matches = **24 文件**（含 RELEASE.md / chat.rs / acp_chat.rs / 多 test） | 逐文件分类：toml 声明删、test 重锚、运行时 chat.rs 不含硬编码（仅经 flow 路由）需复核 |

### A.5 case_metadata（V5，本 sprint **不碰**）

| 事实 | 证据 |
|------|------|
| 模块声明 | `attune-core/src/lib.rs:115 pub mod case_metadata;` |
| 法律语义字段 | `case_metadata.rs:10-35`（`kind` Option「civil-loan/civil-marriage/criminal-defense」、`Party.role` "plaintiff"/"defendant"/"third_party"、`case_no` Option） |
| plugin-aware（OSS=None） | `:12-13` 注释「由付费插件 registers_case_kinds 提供; OSS 裸装 = None」 |
| caller（仅自身 + 测试） | grep：`case_metadata.rs` 自身 + `lib.rs:115` + `tests/plugin_protocol_e2e.rs:186-203`（测试）。**无 OSS 通用运行时路径依赖** |

### A.6 已澄清的通用合法（误报，不动）

per audit-C §1 灰区表 + 独立确认：`routes/office.rs`（OCR/ASR 通用）、`project_recommender.rs`（keywords 调用方传入）、`entities.rs EntityKind`（4 通用类，CaseNo 已删）、`taxonomy.rs`（行业维度已剥离）、`pii/mod.rs`（框架注释 + fixture）、`corpus_domain` 枚举（通用数据标签）。

### A.7 Pre-Create Gate（§1.1.7）核实

| 问 | 结果 |
|----|------|
| 重复检查 | `docs/superpowers/specs/` 无 oss-boundary / patent 迁移 同主题 spec（仅 2026-06-01 wasm / rss-cloud 两个无关 spec） |
| 生命周期 | 长期 SSOT 设计 spec → `docs/superpowers/specs/`（正确白名单位置） |
| 命名 | `2026-06-01-oss-boundary-realignment.md` kebab-case，无版本号绑定 ✓ |

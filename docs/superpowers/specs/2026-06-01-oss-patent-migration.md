---
name: oss-patent-migration
version: v0.1.0-spec
status: DRAFT
date: 2026-06-01
authors: qiurui144 + Claude
template_version: 1
supersedes: docs/superpowers/specs/2026-06-01-oss-boundary-realignment.md (patent 部分 §3/§5/§10/Appendix A.1-A.2)
sprint_id: oss-patent-migration (S4a, 拆自 S4 oss-boundary-realignment G1 REJECT)
---

# Spec: OSS patent 全栈硬删 (S4a)

> 从 OSS attune 物理删除 patent 可执行能力(route + scanner + 孤儿 governor 变体 + 旧仓名注释),硬删 404 无 alias;**排除**行业 agent 声明/流/测试解耦(那是 S4b)。

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
- [Appendix A 事实表](#appendix-a--事实表实测-fileline)

---

## 1. 目标定位

**用户痛点 / 边界归位**:OSS attune 裸装即无条件注册 `/api/v1/patent/search` + `/api/v1/patent/databases`(`rust/crates/attune-server/src/lib.rs:178-179`,无 pro/tier gating),直连 USPTO PatentsView。专利结构化检索(assignee/inventor/IPC/patent_number)只服务专利代理 / 知产律师,对「任何领域个人通用用户」无价值 —— 违反 OSS 北极星(零行业绑定)。本 sprint 把 patent 可执行能力从 OSS 物理删除,能力去向 attune-pro/patent-pro(cross-repo,本仓不实现)。

**与产品 positioning 对齐**:per 项目 CLAUDE.md「OSS attune 边界规则(v0.6.0-rc.2 起)」+ `docs/oss-pro-strategy.md` v2 §4.3 —— 一个功能进 OSS 当且仅当对任何领域个人通用用户都有价值;行业(law/patent/sales/tech/medical/academic)完全在 attune-pro。本 sprint 是 v0.6.0-rc.2 边界瘦身的**延续**:瘦身已删 CaseNo/extract_case_no/CHAT_TRIGGER_KEYWORDS/4 yaml(audit §3 核对全部完成),本次清的是瘦身**之后**由 v1.0 patent connector 回灌的新债(audit V1/V2/V6)。

**与全局 / 项目规则映射**:

| 规则 | 本 spec 落点 |
|------|------------|
| 全局 §3.1 架构级设计铁律(rename/迁移类 → spec-first) | 本 spec 即 spec-first;§10 migration + §11 risk 为重头 |
| 项目「OSS attune 边界规则」 | §1/§2 判定标尺;patent 全栈完全在 attune-pro |
| 项目「Rust 商用线约定」API path 命名(旧 path alias 1 release 周期) | §5/§10 决定 patent **不走** deprecation alias(无 OSS caller) |
| 全局 §4.2.3(多文件 reset/stash conflict-marker) | §11 R7;多文件删除 commit 纪律 |
| 项目「双产品矩阵 + 边界」(技术上独立) | §6 cross-repo assumption 不读不写 pro 仓 |

**与被拆 spec 关系**:本 S4a spec **supersede** `2026-06-01-oss-boundary-realignment.md` 的 patent 部分(SS-A)。被拆 spec 的行业 agent 声明/流/测试解耦(SS-B)由独立 S4b spec 承接,本 spec 不涉及。

---

## 2. 范围边界

### ✅ v1.0.x 本 sprint(S4a)做

1. 删 `rust/crates/attune-server/src/routes/patent.rs`(整文件,147 行,实测 A.1)。
2. 删 `routes/mod.rs:28 pub mod patent;`(模块声明,实测 A.1)。
3. 解除路由注册 `lib.rs:178-179`(2 条 `.route(...)`,**实测确认在 lib.rs 而非 router/mod**,A.1)。
4. 删 `rust/crates/attune-core/src/scanner_patent.rs`(整文件,含内联 `#[cfg(test)] mod tests` 5 个 test,实测 A.2)。
5. 删 `attune-core/src/lib.rs:156 pub mod scanner_patent;`(模块声明,实测 A.2)。
6. **删 `TaskKind::PatentScanner` 孤儿 governor 变体 + 其 wiring + 测试**(实测 A.3,新发现,见下「关键决策」):
   - `resource_governor/profiles.rs:26` enum variant `PatentScanner`
   - `:45` `Self::PatentScanner => "patent_scanner"` Display arm
   - `:158-159` budget 路由 `(p, PatentScanner) => p.budget_for(FileScanner)`
   - `:320-327` 内联测试 `fn patent_scanner_inherits_file_scanner`
   - `:359-362` 内联表驱动测试 3 行 `PatentScanner` 期望
   - **(F1, G1 architect 必修)** `:384` 计数断言 `assert_eq!(cases.len(), 30, "must cover 3 profiles × 10 kinds")` → 改 `27` + 注释 `9 kinds`;测试名 `all_30_combinations_snapshot`→`all_27_combinations_snapshot` + `:336/:337` doc 注释 `30→27` / `10→9 task kinds`。**删 :360-362 三行后不同步改此断言/名 → `cargo test` runtime RED**(cases.len 30→27)。
   - `tests/governor_integration.rs:152` `TaskKind::PatentScanner,`(测试数组元素;其 `:163 assert_eq!(snap.len(), kinds.len())` 自洽,kinds.len 随之 9,无第二处计数耦合 — G1 architect N3 已核)
7. 清 V6 `npu-vault` 旧仓名 cruft 注释:`scanner_patent.rs:1`(随整文件删自动消失)、`patent.rs:1`(随整文件删自动消失)。**核实结论:V6 注释全部位于待删的两个整文件顶部,无独立残留**(A.4)。
8. `/api/v1/patent/*` 硬删 → axum 默认 404,**无 alias**(extension 0 调用 + 无 OSS client,A.1)。
9. 同步删 `rust/README.md` / `rust/README.zh.md` 中 USPTO patent search feature 文档行(doc 漂移,防 §7.2 Gate1 fail;A.4 列待核实行号)。

**关键决策(S4a-specific,S4 architect「scanner_patent 自包含」claim 的补正)**:`TaskKind::PatentScanner` 是**孤儿枚举变体** —— 实测全仓仅 `profiles.rs`(自定义 + budget + 2 内联测试)+ `governor_integration.rs:152` 引用,**真实 patent route 走 `spawn_blocking` 直接执行网络查询,从不经过 resource_governor 调度**(`patent.rs:90-103` 实测无 `TaskKind`/governor 调用)。因此该变体是 dead label。归入本 sprint 删除,因为它属「patent 全栈硬删」语义,且留下一个名为 `patent_scanner` 的 budget 档是 named cruft。但它**触碰通用 `resource_governor` 子系统**(非 patent 自包含),故 §11 R4 单列风险并在 §9 T7 验证 governor 其余变体不漂移。

### ❌ 本 sprint 不做(写死,划给 S4b)

1. 行业 agent 声明(`agents.registry.toml` 18 paid agent)—— **全部 S4b**。
2. legal_defamation 流(`agent_flows.toml`)—— **全部 S4b**。
3. `case_metadata.rs`(plaintiff/defendant/case_no 案件法律模型)—— **全部 S4b**。
4. agent 子系统测试解耦(defamation_extractor telemetry/golden key 重锚)—— **全部 S4b**。
5. CI grep gate 行业关键词守卫(patent/名誉/诽谤/原告/被告)—— **S4b**(涉及 agent 词,本 sprint 仅做 patent 局部 grep 验证 §8 命令 1,不落 CI 守卫)。

### 推迟到 v.next / 灰区保留(本 sprint 不动)

1. `corpus_domain="patent"` 数据标签(`ingest/connector.rs:72` / `store/dirs.rs:19` / `search.rs:141` cross-domain penalty)—— **保留**。实测确认它只是字符串 domain label(枚举值 legal/tech/medical/patent/general),无害,无 patent 模块 import,无 schema。去留属灰区,划 S4b/推迟。
2. `corpus_domain` 行业硬编码值改可扩展注册 —— 独立 spec。
3. patent-pro 在 attune-pro 仓的接收实现 —— cross-repo,attune-pro 自己的 spec/plan 承接(§6/§10/§11 assumption)。

---

## 3. 架构数据流

### Before(当前 OSS,违规)

```
 Chrome ext / Web UI / CLI ──► /api/v1/* router (lib.rs)
                          │   ├─ /api/v1/patent/search    ◄── V1 违规 (lib.rs:178)
                          │   ├─ /api/v1/patent/databases ◄── V1 违规 (lib.rs:179)
                          │   └─ ... 通用 endpoints
                          │        │
                          │   routes/patent.rs ──► attune_core::scanner_patent ──► USPTO PatentsView (外网)
                          │     (V1, patent.rs:12)      (V2, scanner_patent.rs:16 USPTO_BASE)
                          │        │
                          │   spawn_blocking → search_patents() → ingest_patent_records()
                          │        │                                     │
                          │        │                              store.insert_item(.., "patent", ..) ── 通用 items 表
                          │        ▼
                          │   resource_governor::TaskKind::PatentScanner ◄── 孤儿变体 (profiles.rs:26)
                          │     (实测: patent route 从不调用此变体, dead label)
```

**关键事实**:patent 入库走**通用** `store.insert_item(dek, .., content, url, "patent", None, None)`(`scanner_patent.rs:163-188`),`"patent"` 仅是 `source` 字段字面量,**无 patent 专属表 / 无专属 schema / 无 migration DDL**。已入库 patent item 是普通 item。

### After(本 sprint 后)

```
 Chrome ext / Web UI / CLI ──► /api/v1/* router (lib.rs)
                          │   └─ 仅通用 endpoints
                          │      (patent route 已删 → /api/v1/patent/* = axum 默认 404 unknown-route)
                          │
                          │  routes/patent.rs        ── 已删 (整文件)
                          │  attune_core::scanner_patent ── 已删 (整文件)
                          │  TaskKind::PatentScanner  ── 已删 (governor 变体 + wiring + 测试)
                          │
                          │  vault 内 source="patent" 旧 item ── 保留为普通 item, 可 search/RAG 引用 (§10)

   ┌──── cross-repo assumption (本 sprint 不操作, §10 协调点 R2) ────┐
   │  attune-pro/patent-pro: 经 PluginHub 接收 patent route+scanner   │
   │  运行时注册同形 /api/v1/patent/* (与 OSS 删除形态同构)           │
   └─────────────────────────────────────────────────────────────────┘
```

**DB / cache / 状态机**:无 DB schema 变更、无 migration DDL、无新增表、无状态机。摘要缓存按 `chunk_hash`,与 `source` 来源无关 → cache 无影响。

---

## 4. 模块边界

| crate | 文件 | 改动 | 实测引用 |
|-------|------|------|---------|
| attune-server | `src/routes/patent.rs` | **删整文件**(147 行) | A.1 |
| attune-server | `src/routes/mod.rs:28` | 删 `pub mod patent;` | A.1 |
| attune-server | `src/lib.rs:178-179` | 删 2 条 `.route(...)` | A.1 |
| attune-core | `src/scanner_patent.rs` | **删整文件**(含内联 5 test) | A.2 |
| attune-core | `src/lib.rs:156` | 删 `pub mod scanner_patent;` | A.2 |
| attune-core | `src/resource_governor/profiles.rs` | 删 enum variant + Display arm + budget 路由 + 2 内联测试(6 处) | A.3 |
| attune-core | `tests/governor_integration.rs:152` | 删 `TaskKind::PatentScanner,` 数组元素 | A.3 |
| (doc) | `rust/README.md` / `rust/README.zh.md` | 删 patent feature 行 | A.4 |

**跨仓边界(硬约束)**:attune-pro 接收 patent = 外部依赖。本仓**不读不写** attune-pro 私有仓。patent「落到 attune-pro 哪里」仅在 §6/§10/§11 标 assumption + 协调点。

**改动文件总数 ≤ 8**(单 worktree 单 PR 即可,不构成必须拆的大体量)。**无 second-order 模块受影响**:scanner_patent 自建独立 `reqwest::blocking::Client`(`:193`),不共享;删除不影响其他 blocking client(R6)。

---

## 5. API 契约

### 被删 endpoint 清单(typed schema,删除前形态)

```yaml
removed_endpoints:
  - method: POST
    path: /api/v1/patent/search          # lib.rs:178, routes/patent.rs:40 search()
    request:                              # routes/patent.rs:19-34 PatentSearchRequest
      q:           string                 # 必填, trim 后非空, ≤500 bytes (MAX_QUERY_BYTES)
      limit:       integer = 10           # clamp(1, 20) (MAX_LIMIT)
      database:    string  = "uspto"      # 仅 "uspto" 合法, 否则 400
      ipc_filter:  string | null          # 可选 IPC 大类 (如 "G06F")
      auto_ingest: boolean = false        # true 时入库 (需 vault unlock 拿 DEK)
    response_200:                         # patent.rs:113-129 serde_json::Value
      database:    string
      keywords:    string
      total_found: integer
      count:       integer
      ingested:    integer
      results:     PatentRecord[]         # {patent_number,title,abstract,grant_date,assignees,inventors,ipc_classes,source_url}

  - method: GET
    path: /api/v1/patent/databases        # lib.rs:179, routes/patent.rs:133 databases()
    response_200:                         # patent.rs:134-145
      databases: [ { id:"uspto", name:"USPTO PatentsView", description:string,
                     coverage:string, auth_required:false, rate_limit:string } ]
```

### 删除后 404 行为决策

```yaml
removal_decision:
  strategy: hard-delete-404            # 无 deprecation alias
  rationale:
    - grep 确认 OSS 内**无任何 client** 调用 /api/v1/patent
      (extension/ 0 命中; rust 内仅 lib.rs 注册 + README 文档; scanner_patent.rs USPTO_BASE 是外网 URL 非 OSS endpoint; A.1)
    - 项目「Rust 商用线约定」alias 宽限是为「OSS 内部已有 caller 需时间迁移」, 此处无 caller, alias 纯增维护面
    - patent 是行业能力, 按 OSS 北极星本就不该对通用用户暴露 → 无「平滑迁移老用户」诉求
  after_removal:
    - 任意请求 /api/v1/patent/*  →  axum 默认 404 Not Found (unknown route, 无 patent handler)
    - 正确去处: 安装 attune-pro/patent-pro 后由 pro 注册同形 endpoint (cross-repo, §10 协调点)
  error_code: unknown-route (HTTP 404)   # axum fallback, 非自定义错误码
```

**无新增 / 修改 endpoint**;本 sprint 纯删除 API surface。

---

## 6. 扩展点 / 插件接口

**patent 能力如何在 Pro 重新接入(cross-repo assumption,本仓不验证不实现)**:

- **assumption A1**:attune-pro/patent-pro 在自己的 plugin pack 内提供 patent route + scanner,经 PluginHub 签名分发,运行时由 plugin loader 注册 `/api/v1/patent/*`(与 OSS 删除的形态同构,保证 Pro 用户能力不丢)。**本仓不验证、不实现**;列为 §11 R2 协调点。
- **现有扩展机制支撑(OSS 侧已具备,不属本 sprint 改动)**:
  - `routes/agents.rs::run_agent`(`lib.rs:173`)—— 前端触发 plugin agent binary 的通道。
  - `marketplace::install_plugin`(`lib.rs:176`)+ `plugin_registry` 动态 merge —— 「paid plugin 经插件市场安装 + 运行时注册」的运行时通道已存在。
- **配置覆盖位置**:patent-pro plugin.yaml 的 route/scanner 声明在 pro 仓 plugin pack 内,OSS 侧无需任何 hook 点改动(plugin loader 已是通用 merge 机制)。
- **`TaskKind` 扩展**(本 sprint 删 PatentScanner 后):若 pro plugin 需要资源治理档,应由 plugin 侧声明自己的 task profile,而非在 OSS 通用 `resource_governor` enum 硬编码行业变体 —— §11 标记为后续可扩展点(non-blocking)。

**跨仓边界硬约束(重申)**:patent → attune-pro/patent-pro = **外部前置依赖**。本 sprint 仅声明 assumption,不读不写 pro 仓。

---

## 7. 错误 + 边界 case

| 错误码 (kebab) / 状态 | 触发 | 行为 |
|---------------------|------|------|
| `unknown-route` (HTTP 404) | 删除后请求 `/api/v1/patent/search` 或 `/databases` | axum 默认 404,无 patent handler;**非 panic、非 500** |
| (无错误,正常检索) | vault 内 `source="patent"` 旧 item 被通用 search 命中 | 普通 item 返回,不依赖 scanner;不报错(§10 数据不丢) |
| (编译期) | 删 scanner_patent / patent.rs / PatentScanner 后 `cargo build` | 必须 0 dangling ref(实测 A.1-A.3 已枚举全部引用点,删全即绿) |

**边界 case 矩阵**:

- **空 / 超长 query**:N/A(endpoint 已删,无输入路径)。删除前的 `q` trim 空 → 400、`>500 bytes` → 400 校验逻辑随文件删除消失,无需保留。
- **已入库 patent items(无 scanner)**:vault 中 `source="patent"` 旧 item 删 scanner 后仍可 search/被 RAG 引用(普通 item),无 500、无 dangling。这是 §9 T4 重点回归。
- **governor 其余变体不漂移**:删 `PatentScanner` 后 `FileScanner`/`WebDavSync`/`BrowserSearch`/`AiAnnotator` budget 不变 —— §9 T7 验证。
- **doc 漂移**:README patent 行未删 → §7.2 Gate1 fail(R8);§2 已列入改动。

**graceful degradation**:删除后 OSS 对 patent 请求的「降级」即 404(无可执行能力),正确恢复路径是安装 patent-pro(§10)。无中间态崩溃风险。

---

## 8. 成本契约

| 维度 | 估算 | 归属 |
|------|------|------|
| **磁盘** | **净减** ~510 LOC 源码(patent.rs 147L + scanner_patent.rs ~380L,含内联测试) + governor ~12L(enum/arm/budget/2 test)。二进制略减(删 USPTO reqwest blocking 路径)。无新增磁盘。 | 🆓 零成本 |
| **token** | **0**。纯删除 + 文档改 + 测试删,无 LLM 调用、无 embedding 重算。 | 🆓 零成本 |
| **wall-clock** | 实施估 ~30-45 分钟(8 文件删/改 + `cargo build` + `cargo test --workspace` + clippy + grep gate)。**诚实计:无 wall-clock 实证,按客观计数 8 文件 / 1 PR / ~510 LOC 删除 + 1 轮回归。** | ⚡ 本地编译 |
| **本地算力** | 仅 `cargo build --release` + `cargo test --workspace` 的编译/测试 CPU,秒-分钟级。无 GPU/NPU/LLM。 | ⚡ 本地算力 |

**审计命令(用户可一行运行)**:

```bash
# 命令 1: patent 可执行残留 (应 0 命中 — corpus_domain/plugin-id 命中需人工区分, 见下)
grep -rn "scanner_patent\|routes::patent\|/api/v1/patent\|PatentQuery\|search_patents\|PatentScanner\|USPTO" \
  rust/ --include="*.rs" --include="*.toml" | grep -v reports/
# 预期: 仅可能剩 corpus_domain="patent" 数据标签(§2 灰区保留)+ patent_pro plugin-id(S4b), 无 route/scanner/governor/USPTO 命中

# 命令 2: 删后编译 + 回归 + lint 三连
cd rust && cargo build --release && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings

# 命令 3: README doc 漂移检查 (应仅剩"由 attune-pro 提供"措辞, 无 USPTO patent search feature 行)
grep -ni "patent\|uspto" rust/README.md rust/README.zh.md
```

---

## 9. 测试矩阵 (6 类下限 per §6.1)

| 类型 | # | 输入描述 | 期望输出 | 通过判据 (pass/fail) |
|------|---|---------|---------|---------------------|
| **happy path** | T1 通用 endpoint 不受影响 | 删 patent 后启动 server,调 `GET /api/v1/search?q=...` | 200 正常返回 | 通用功能 0 回归;状态码 == 200 |
| **happy path** | T2 删后干净编译 | `cargo build --release` | 编译通过,无 orphan 引用 | 0 build error(scanner_patent / patent / PatentScanner 全部引用点已删,A.1-A.3) |
| **edge case** | T3 已入库 patent item 仍可检索 | vault 含 `source="patent"` 旧 item,以其关键词调通用 search | 命中返回,无 scanner 依赖 | search 结果含该 item,无 500、无 dangling(数据不丢,§10) |
| **edge case** | T4 空 patent vault | 无任何 patent item 的 vault,调通用 search | 正常空/非 patent 结果 | 不因删 scanner 报错 |
| **error case** | T5 删后请求 patent endpoint(search) | `POST /api/v1/patent/search {"q":"neural network"}` | 404 unknown-route | 状态码 == 404,非 500/panic |
| **error case** | T6 删后请求 patent endpoint(databases) | `GET /api/v1/patent/databases` | 404 unknown-route | 状态码 == 404 |
| **adversarial** | T7 governor 其余变体不漂移 | 删 `PatentScanner` 后跑 `resource_governor::profiles` 全部测试 | `FileScanner`/`WebDavSync`/`BrowserSearch`/`AiAnnotator` budget 期望全绿 | profiles.rs 剩余测试 0 fail;无变体被误删(防 §2.6 改通用子系统时漂移) |
| **concurrent / 多并发** | T8 多 server 并发启动(无 patent route) | 多实例同时 `build_router` / 加载 | 各自构建只读 router,无竞争 | 并发启动无 panic/data race(router 构建无 patent 共享态) |
| **resource / 资源** | T9 二进制 / 编译资源 + lint | 删后 `cargo clippy --workspace --all-targets -- -D warnings` | clippy 干净 | 0 warning;无 dead_code/unused_import 残留(删 governor 变体后 Display arm/budget 全删,无 unreachable) |
| **回归(重头)** | T10 全量 workspace | `cargo test --workspace` | 全绿(含删 governor_integration.rs:152 后的 governor 集成测试) | 0 fail;新 `#[ignore]` 不突增(§7.2 Gate2) |
| **回归(grep gate)** | T11 patent 可执行残留 0 命中 | §8 审计命令 1 | 仅剩 corpus_domain/plugin-id 命中(人工区分),无 route/scanner/governor/USPTO | grep 无 `scanner_patent`/`routes::patent`/`/api/v1/patent` mount/`PatentScanner`/`USPTO` 命中 |
| **回归(doc 漂移)** | T12 README patent 行已删 | §8 审计命令 3 | 无 doc drift(§7.2 Gate1) | README grep patent == 仅「pro 提供」措辞或 0 |

**通过判据汇总**:T2/T10 编译+回归全绿 + T5/T6 硬删 404 + T3 数据不丢 + T7 governor 不漂移 + T11 grep gate 0 patent 可执行残留 + T12 doc 一致。任一 fail → 不 merge。

**multi-seed**:N/A(纯删除,deterministic,无 LLM/无高方差指标)。

---

## 10. 向后兼容

### SemVer 策略

- patent endpoint **硬删除 = breaking change**(移除公开 API surface)→ 严格按 SemVer 应触发 minor bump(0.x/1.0.x 阶段 minor 表示 breaking 可接受)。归入下一可用 v1.0.x minor(按 merge 顺序拿号,§7.1.7)。
- RELEASE.md「Breaking」节必列:`/api/v1/patent/*` removed from OSS。

### schema versioning

- **无 DB schema 变更、无数据迁移 DDL**。patent items 走通用 `items` 表 + `source` 字段(`scanner_patent.rs:163-188` `store.insert_item(.., "patent", ..)`),删 scanner 不改表结构。
- **已入库 patent items**:以普通 item 形态留在 vault(content + embedding 已生成),删 route/scanner **不删用户已有数据**。

### 老 client 行为

```yaml
client_impact:
  - client: OSS 内部 (extension / web UI / CLI)
    calls_patent: false          # grep 确认 0 调用 (Appendix A.1)
    impact: 无
  - client: 直接 curl /api/v1/patent (理论外部脚本)
    after: 404 unknown-route
    migration: 安装 attune-pro/patent-pro 恢复同形 endpoint
  - data: vault 内已入库 patent items
    after: 保留为普通 item, 可 search / RAG 引用, 不丢失
```

### migration path — worked old → new 示例

```yaml
old:   # OSS v1.0.x-1 (删除前): OSS 裸装即可调
  request:   POST http://localhost:18900/api/v1/patent/search  {"q":"neural network","database":"uspto"}
  served_by: OSS attune routes/patent.rs → attune_core::scanner_patent → USPTO PatentsView
  response:  200 {"database":"USPTO","keywords":"neural network","total_found":N,"results":[...]}

new:   # 本 sprint 后: OSS 不再提供
  request:     POST http://localhost:18900/api/v1/patent/search  {"q":"neural network"}
  oss_response: 404 Not Found (unknown-route)
  to_restore:
    step1: 安装 attune-pro/patent-pro plugin pack (经 PluginHub / marketplace install_plugin)
    step2: pro plugin 运行时注册同形 /api/v1/patent/search (cross-repo assumption A1)
    step3: 同请求由 pro plugin 服务 → 能力恢复, 数据仍在用户本地 vault (源 source="patent" 旧 item 不丢)
```

### 跨仓 sequencing(关键:本 sprint **不阻塞**)

- **OSS 删除独立可做**:patent 物理移除是 OSS 仓内自包含改动(A.1-A.3 确认无外部依赖),不需要等 attune-pro/patent-pro 就绪。
- **能力在 Pro 重建是 Pro 仓的事**:patent-pro 接收 = attune-pro 仓自己的 spec/plan 承接,本仓不验证不实现。
- **前置依赖性质**:patent-pro 是否已接住 = cross-repo **协调点**(本仓无法验证,§11 R2),但它是「Pro 用户能力是否暂时真空」的运维/发布问题,**不是 OSS 删除的技术阻塞**。RELEASE.md「Known Limitations」标明「patent 能力需 attune-pro/patent-pro,发布状态见协调点」即可。

### RELEASE.md 写入要点

- Highlights:OSS 边界归位 — patent 全栈迁出。
- Breaking:`/api/v1/patent/*` removed from OSS。
- Migration:上面 worked example + 跨仓 sequencing(安装 patent-pro 恢复)。
- Known Limitations:patent 能力需 attune-pro/patent-pro(cross-repo,发布状态见协调点)。

---

## 11. 风险登记

| # | 风险 | 概率 | 影响 | 缓解 |
|---|------|------|------|------|
| **R1** | 删可执行 patent 能力 = breaking change,外部脚本/未知 client 直 curl `/api/v1/patent` 收 404 | Low | Med | grep 确认 OSS 内 0 caller(A.1);RELEASE.md Breaking+Migration 明示;硬删合理(无 OSS client,§5 决策) |
| **R2** | **跨仓能力暂时性 gap(assumption)**:OSS 删了 patent 但 attune-pro/patent-pro **尚未接住** → Pro 用户能力真空 | Med | High | 本仓不可验证 pro 状态(硬约束);列为 RELEASE.md「Known Limitations」+ merge 前可选人工确认协调点;**非 OSS 删除技术阻塞**(§10 sequencing) |
| **R3** | **dangling ref 遗漏**:删 patent.rs/scanner_patent.rs 后仍有引用点未删 → `cargo build` 红 | Low | Med | A.1-A.3 已**实测枚举全部引用点**(route 注册 2 + mod 声明 2 + governor 6 + 集成测试 1);§9 T2/T10 编译+回归 gate;§8 审计命令 1 grep 验证 |
| **R4** | **改通用 governor 子系统漂移**:删 `PatentScanner` 误删/误改 `FileScanner`/`BrowserSearch` 等通用变体的 budget | Med | Med | §9 T7 专测 governor 其余变体不漂移;profiles.rs 改动**精确限定** 6 处(A.3 行号);删后跑 `resource_governor::profiles` 全部测试 |
| **R5** | **删测试遗漏**:scanner_patent.rs 内联 5 test + profiles.rs 2 test + governor_integration.rs:152 未一并处理 → 编译红或测试引用悬空 | Med | Med | A.2/A.3 已列全部测试位置;scanner_patent 内联 test 随整文件删自动消失;profiles.rs 2 内联 test + governor_integration.rs 数组元素需手删;§9 T10 回归验证 |
| **R6** | **二进制行为变化**:删 scanner_patent 移除 USPTO reqwest blocking 路径,意外影响其他 blocking client | Low | Low | scanner_patent 自建独立 `reqwest::blocking::Client`(`:193`),无共享;A.2 确认无其他模块引用其 client |
| **R7** | **多文件删除 conflict-marker**(§4.2.3):reset/stash/cherry-pick 重排残留 `<<<<<<<` 提进 commit | Med | Med | 每次 reset/stash 后强制 `grep -n "^<<<<<<<\|^=======$\|^>>>>>>>"` + `wc -l` + `git diff` 三步验证(§4.2.3);优先 cherry-pick 少用 stash+reset;8 文件改动逐一 `git diff` 核 |
| **R8** | **doc 漂移**:删 route 但 README.md/README.zh.md patent feature 行未同步 → §7.2 Gate1 fail | Med | Low | §2/§4 列入 README 改动;§8 审计命令 3 含 .md;双语 README 同步(§1.1.3) |
| **R9** | **clippy dead_code/unreachable**:删 governor 变体后 Display arm/budget match 残留 unreachable 或漏删 import | Low | Low | §9 T9 clippy `-D warnings` gate;A.3 列全 6 处确保 enum/arm/budget/test 同步删,无半删 |
| **R10** | **grep gate 误判**:审计命令 1 把 `corpus_domain="patent"` 数据标签 / `patent_pro` plugin-id 误判为残留 | Med | Low | §8 命令 1 注释明示需人工区分;corpus_domain/plugin-id 是 §2 灰区保留 + S4b 范围,非 patent 可执行残留;判据只针对 route/scanner/governor/USPTO 命中 |

---

## Appendix A — 事实表(实测 file:line)

### A.1 patent route 注册 + caller 核实

| 事实 | 证据 (实测) |
|------|------|
| patent 2 route **在 lib.rs 注册**(非 router/mod.rs) | `rust/crates/attune-server/src/lib.rs:178` `.route("/api/v1/patent/search", post(routes::patent::search))`;`:179` `.route("/api/v1/patent/databases", get(routes::patent::databases))` |
| 模块声明 | `rust/crates/attune-server/src/routes/mod.rs:28 pub mod patent;` |
| patent.rs 是 scanner_patent 唯一消费者 | `routes/patent.rs:12` `use attune_core::scanner_patent::{ingest_patent_records, search_patents, PatentDatabase, PatentQuery}`;`:71 PatentQuery`;`:93 search_patents`;`:97 ingest_patent_records`。全仓 grep 这些符号仅 `patent.rs` + `scanner_patent.rs` 自身,**无第三方** |
| patent route 走 spawn_blocking,**不经 governor** | `routes/patent.rs:90-103` `tokio::task::spawn_blocking(...)`,无 `TaskKind`/`resource_governor` 调用 |
| Chrome extension **无** patent 调用 | per S4 architect 实测确认 extension 0 命中(本 sprint grounding 复用) |
| patent.rs 文件长度 | 147 行(实测 Read 全文) |

### A.2 scanner_patent 模块

| 事实 | 证据 (实测) |
|------|------|
| 模块声明 | `rust/crates/attune-core/src/lib.rs:156 pub mod scanner_patent;` |
| 文件位置(在 **attune-core** 非 attune-server) | `rust/crates/attune-core/src/scanner_patent.rs` |
| USPTO 直连 | `scanner_patent.rs:16 const USPTO_BASE = "https://search.patentsview.org/api/v1/patent/"`;`:192 fn search_uspto`;`:193` 独立 `reqwest::blocking::Client`(无共享) |
| 入库走通用 store(无 patent 专属 schema) | `scanner_patent.rs:154 search_patents`;`:163-188 ingest_patent_records` → `store.insert_item(.., "patent", None, None)`(`source` 字段字面量) |
| 内联测试 | `scanner_patent.rs:302 #[cfg(test)] mod tests`,内含 `:306,:329,:349,:354,:366` 5 个 `#[test]` —— 随整文件删自动消失 |
| 旧仓名注释 cruft (V6) | `scanner_patent.rs:1 // npu-vault/crates/vault-core/src/scanner_patent.rs`;`patent.rs:1 // npu-vault/crates/vault-server/src/routes/patent.rs` —— 均在待删整文件顶部 |

### A.3 TaskKind::PatentScanner 孤儿变体(S4a 新发现,补正 S4「自包含」claim)

| 事实 | 证据 (实测) |
|------|------|
| enum variant 定义 | `rust/crates/attune-core/src/resource_governor/profiles.rs:26 PatentScanner,` |
| Display arm | `profiles.rs:45 Self::PatentScanner => "patent_scanner",` |
| budget 路由 | `profiles.rs:158-159 (p, PatentScanner) => p.budget_for(FileScanner),`(与 FileScanner 同档) |
| 内联测试 1 | `profiles.rs:320-327 fn patent_scanner_inherits_file_scanner` |
| 内联测试 2(表驱动) | `profiles.rs:359-362` 3 行 `(Profile::*, TaskKind::PatentScanner, ...)` 期望 |
| **(F1) 计数断言耦合** | `profiles.rs:384 assert_eq!(cases.len(), 30, "...3 profiles × 10 kinds")` + 测试名 `all_30_combinations_snapshot`(:339) + `:336/:337` doc 注释 — 删 3 行后必同步 30→27 / 改名 / 改注释,否则 runtime RED。G1 architect F1 补正,本表原漏(G13) |
| 集成测试引用 | `rust/crates/attune-core/tests/governor_integration.rs:152 TaskKind::PatentScanner,`(数组元素) |
| **dead label 判定** | 全仓 `PatentScanner`/`patent_scanner` 仅上述 2 文件命中;真实 patent route(patent.rs)走 spawn_blocking 不调 governor → 该变体从不被 dispatch,是孤儿。删除属「patent 全栈硬删」语义,但触碰通用 governor 子系统 → R4 |

### A.4 V6 cruft / doc 漂移 / 灰区保留(out-of-scope 确认)

| 项 | 证据 (实测) | 本 sprint 处理 |
|----|------|--------------|
| V6 旧仓名注释全部位于待删整文件顶部 | `scanner_patent.rs:1` + `patent.rs:1`,无独立残留行 | 随整文件删,无需单独处理 |
| README patent feature 行 | `rust/README.md` / `rust/README.zh.md`(待实施时 grep 精确行号,§8 命令 3 验证) | 删/改为「由 attune-pro 提供」措辞 |
| `corpus_domain="patent"` 数据标签(灰区保留) | `ingest/connector.rs:72`(注释「领域分类 legal/tech/medical/patent/general」对应 `items.corpus_domain`);`store/dirs.rs:19`(`bind_directory_with_domain`);`search.rs:140-143`(cross-domain penalty 命中 'patent') | **保留**(§2,字符串 domain label,无 patent 模块 import) |
| `patent_pro` plugin-id(S4b 范围) | `tests/generic_plugins_test.rs:74 id: patent_pro`;`:103,:125,:193`(plugin-id 匹配测试) | **不动**(S4b) |

### A.5 Pre-Create Gate(§1.1.7)

| 问 | 答 |
|----|------|
| 重复检查 | `grep -rli "patent" docs/superpowers/specs/` 仅命中被拆的 `2026-06-01-oss-boundary-realignment.md`(本 spec supersede 其 patent 部分);无其他 patent 迁移同主题 spec |
| 生命周期 | 长期 SSOT 设计 spec → `docs/superpowers/specs/`(白名单正确位置) |
| 命名 | kebab-case `2026-06-01-oss-patent-migration.md`,无版本号绑定 |

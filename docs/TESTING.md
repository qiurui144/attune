# Attune Testing Guide

> **目标**：产品级测试，可复现、可追溯、覆盖用户真实场景。
> **能力清单 SSOT**：18 条产品能力清单见 [`../README.md`](../README.md)（`FEATURES.md` 已于 86e3833 合并进 README，不再单独维护，避免同主题双副本 per §3.2）。本文档是**测试方案 SSOT**，按测试层 → 覆盖哪些能力反向组织，能力 ID（`F-{nn}-{TOPIC}`）映射见本文 §1.3。
> **当前版本基准**：v1.1.0（ACP — Agent Control Plane）。统计数字均为本机真跑取数（见 §1.2）。

**非目标**：
- 随机生成测试数据（行为不可复现）
- 只有 unit test（缺真实用户场景覆盖）
- 只追"测试数量"不看"质量指标"

---

## 1. 测试金字塔 — 5 层主轴

attune 采用工业经典 5 层分类（Unit / Integration / System / E2E / Smoke），叠加 attune 特化的 3 类质量门（见第 2 章）。

```
                ┌─────────────┐
                │   E2E       │  Playwright Chrome — 真实浏览器交互
                └─────────────┘
              ┌─────────────────┐
              │   System        │  二进制起 + HTTP 黑盒，无浏览器
              └─────────────────┘
            ┌─────────────────────┐
            │   Integration       │  跨模块（含 SQLite/Tantivy）+ 真实语料注入
            └─────────────────────┘
          ┌─────────────────────────┐
          │   Unit                  │  纯逻辑、边界、错误路径
          └─────────────────────────┘

           ╔══════════════════════════╗
           ║   Smoke (release gate)   ║   5 分钟二进制健康检查
           ╚══════════════════════════╝

         ┌────────────────────────────────────────┐
         │  跨层质量门: Corpus / Performance /     │
         │  Quality Regression (见第 2 章)         │
         └────────────────────────────────────────┘
```

### 1.1 五层定义与边界

| 层 | 边界 | 数据源 | 回归耗时 | 覆盖 | 现有命令 |
|----|------|-------|---------|------|---------|
| **Unit** | 单函数 / 单模块 `#[cfg(test)] mod tests` | 内存 fixture | < 30 s | 纯算法、错误分支、derive 默认 | `cargo test --lib` |
| **Integration** | 跨模块（多个 crate 协作）+ 持久化（SQLite/Tantivy）+ 可选真实语料 | mock store 或本地 fixture | < 2 min | 跨模块协作、数据库 round-trip | `cargo test --test '*'` |
| **System** | 整个 server 起来 + HTTP client 黑盒 | 真实二进制 + 临时 vault | < 5 min | 整链路用户场景，不开浏览器 | `cargo test --test system_*` (B.3 后) |
| **E2E** | 真实浏览器（Chromium MCP / Playwright） | 启动的 server + Playwright fixture | 5-10 min | 用户可见 UI 路径 | `tests/e2e_rust/` (C.2 后) |
| **Smoke** | 二进制活性检测，**部署后 / release 前**跑 | 临时 vault | < 5 min | 二进制 spawn + 关键 API 200 | `bash scripts/smoke-test.sh` |

**关键边界**：

- **Unit vs Integration**：Unit 不允许跨模块、不允许真实 SQLite。如果你 `use sqlx::SqlitePool` 或 `Vault::setup`，那是 Integration。
- **Integration vs System**：Integration 走 Rust API（直接调函数）。System 必须经 HTTP（reqwest 走 `/api/v1/*`）。
- **System vs E2E**：System 不开浏览器。E2E 必须真实 Chrome 实例。
- **Smoke 不属于金字塔回归**：Smoke 不在每个 PR 跑（CI 上的 Unit/Integration 才是 PR gate）；它是 release 前手动 + 部署后冒烟用。

**视角划分（白盒 / 灰盒 / 黑盒，per §6.1）**：5 层同时是测试视角的映射 —

| 视角 | 含义 | 对应层 |
|------|------|--------|
| **白盒** | 基于源码内部逻辑，知道分支走向逐个覆盖 | Unit（`#[cfg(test)]`，每个 branch 一 case） |
| **灰盒** | 知 Rust API 契约、不依赖具体实现 | Integration（直接调函数 + 持久化 round-trip） |
| **黑盒** | 完全用户视角，不知实现 | System（HTTP `/api/v1/*`）+ E2E（真 Chrome）+ Smoke |

ACP / Agent Flow 质量门（§2.4）同样三视角并存：governor 单元（白）→ wire 集成（灰）→ governed-chat 端到端（黑）。

### 1.2 现有覆盖统计（v1.1.0，2026-05-30 本机真跑）

> 数字来源：`cargo test -p attune-core --lib`（本机 develop @ v1.1.0，4.14s）+ `ls tests/*.rs | wc -l` + `grep -rn '#[ignore'`。**不照抄历史数字**。

| 层 | 文件位置 | 已有套件 | 真跑结果 |
|----|---------|---------|----------|
| Unit | 各 crate `src/**/tests` 内联 | attune-core lib + attune-server lib + headless | **attune-core lib 1499 passed / 0 failed / 1 ignored**（4.14s） |
| Integration | `crates/attune-core/tests/`（**53** 套件）、`crates/attune-server/tests/`（**34** 套件）、`rust/tests/` | **87 集成套件** | 抽样全绿（ACP wire 子集见 §2.4） |
| System | `rust/tests/corpus_integration_test.rs`、`server_test.rs` | 2+ 套件 | 部分 |
| E2E | `tests/e2e_rust/`（C.2 后） + AMD 笔电 deb 真机（§1.4） | 真机线已建 | release 前手动 |
| Smoke | `scripts/smoke-test.sh` / `smoke-test-cli.sh` | 2 脚本 | 5+ 项检查 |

**v1.1.0 关键统计**（真跑）：
- attune-core lib **1499 passed / 0 failed / 1 ignored**（pass rate 100%，符合 §7.2 Gate 2 ≥1.00 deterministic）。
- 集成套件 **87 个**（core 53 + server 34），非历史大纲所写「19 / 20 套件」。
- `#[ignore]` 总数 **76**（grep src+tests）：主要是 real-LLM gate（需 ollama）/ 大语料 corpus / 慢测（form_factor Argon2id 250s 等）拆走 nightly 通道，**非「用 ignore 绕过失败」**（1 ignored in core lib = 极低）。
- 历史数字（535 / 587 / 630 / 3 ignore）已全部过时作废，以本节为准。

### 1.3 能力 ↔ 测试层覆盖矩阵

能力清单 SSOT 现已并入 [`../README.md`](../README.md)（FEATURES.md 已合并）。下表为 18 条能力 ID（`F-{nn}-{TOPIC}`）与本测试方案的内联映射，本文档不再外链 FEATURES.md。

**18 条能力 ID 速查**（v1.1.0；新增 ACP 见 §2.4，能力体系仍以 F-10-GOVERNOR 为根）：

| ID | 能力 | ID | 能力 |
|----|------|----|------|
| F-01-VAULT | 三因子加密 vault + 状态机 + 跨设备迁移 | F-10-GOVERNOR | H1 资源 governor（3 profile + 节流 + 顶栏暂停）→ v1.1.0 ACP 扩展 |
| F-02-RAG | 混合检索（BM25+vector+RRF）+ J1 path-prefix chunker + 两阶段层级检索 | F-11-PLUGINS | 插件框架（plugin.yaml + EntityExtractor trait + 市场开关）|
| F-03-CHAT | RAG chat + B1 引用面包屑 + 会话持久化 + 跨会话续接 | F-12-PROJECT | Project/Case 通用层 + 跨证据推荐 |
| F-04-READER | Reader 模态 + 5 用户批注标签 + 4 角度 AI 批注 + 批注加权 RAG | F-13-WORKFLOW | Workflow 引擎 + Intent Router 自然语言路由 |
| F-05-COMPRESS | 上下文压缩 + 摘要缓存（70-85% 云端 token 节省）| F-14-ENTITIES | 通用实体抽取（Person/Money/Date/Org）|
| F-06-WEBSEARCH | 浏览器自动化网络搜索 + 30d 加密缓存 | F-15-MCP | Python stdio shim 包 REST 供 MCP 客户端 |
| F-07-EVOLUTION | episodic 记忆固化 + SkillEvolver 失败信号扩展 | F-16-DISTRIBUTION | Tauri 2 桌面（Win MSI/NSIS、Linux deb/AppImage）+ NAS HTTPS + 硬件 profile |
| F-08-BROWSEEXT | Chrome 扩展 G1/G2/G5：通用浏览捕获 + 自动书签 + 隐私面板 | F-17-PRIVACY | 三级隐私（L0 chunk 隔离 / L1 PII 占位 / L3）+ 跨域防御 |
| F-09-FORMFACTOR | FormFactor split（Laptop/K3Appliance/Server/Unknown）+ LLM 默认路径 | F-18-QUALITY | K2 Parse golden set（CI 门）+ RAGAS 风格 benchmark harness |

简版覆盖概览（v1.1.0 状态，新能力 ACP/Agent Flow 见 §2.4）：

| 测试层 | 完整覆盖 | 部分覆盖 | 空白 |
|--------|---------|---------|------|
| Unit | F-01 ~ F-04, F-07, F-09, F-10, F-12 ~ F-14, F-16, F-18 | F-05, F-06, F-08, F-11, F-17 | F-15 |
| Integration | F-01, F-02, F-04, F-05, F-07, F-10, F-12 ~ F-14, F-16, F-18 | F-03, F-06, F-08 | F-09, F-11, F-15, F-17 |
| System | F-01, F-02 (corpus), F-16, F-18 | F-03, F-04, F-06 | F-05, F-07 ~ F-15, F-17 |
| E2E | F-08 (扩展自有) | F-01, F-03, F-04 | F-02 ~ F-07, F-09 ~ F-18 |
| Smoke | F-01, F-16 | F-02, F-09 | F-03 ~ F-08, F-10 ~ F-15, F-17, F-18 |

**驱动当前任务定义**：B.1 / B.2 / B.3 / C.1 / C.3 都从这张表的"空白" + "部分"格倒推。

### 1.4 真机 E2E (Linux deb)

第 5 层 (System E2E) 含一条独立子线: **AMD 笔电 deb 路径真机验收** (per [[feedback_linux_deb_only_testing]]).

- 主机: 192.168.100.201 (Ryzen 7 8845H + Radeon 780M + XDNA NPU + Ubuntu 25.10)
- 部署: `cargo tauri build --bundles deb` → scp → `dpkg -i` (不走 cargo run)
- 测试驱动: 本机 Playwright MCP Chrome + SSH -L 18900 tunnel
- 验收脚本: `rust/crates/attune-server/tests/amd_laptop_e2e_smoke.rs` (默认 #[ignore], 跑前设 `ATTUNE_E2E_HOST` + `ATTUNE_E2E_TOKEN`)
- 报告: 真机验收截图归 `docs/screenshots/<release>-verification/`；bug 修复记录入 RELEASE.md 对应版本节（不单独维护 e2e-test-report.md per §3.2）
- 跑 trigger: 每次 release-candidate 重 deb 后必跑一次 (覆盖 wizard 5 步 + 6 tab + RAG chat + law-pro 证据链)

---

## 2. 跨层质量门 — attune 特化

5 层是"测试边界"分类。但 attune 作为 RAG 产品还需要"质量边界"分类：

```
┌───────────────────────────────────────────────────────────────┐
│  跨层质量门（每个都跨 Unit/Integration/System 多层执行）         │
├───────────────────────────────────────────────────────────────┤
│  ① Corpus Integration   真实 GitHub 知识库注入 + 检索          │
│  ② Performance Bench    criterion benchmarks，跨版本对比性能   │
│  ③ Quality Regression   golden set precision@K，回归 5% 报警   │
└───────────────────────────────────────────────────────────────┘
```

### 2.1 Corpus Integration（真实语料）

**核心原则**：语料**版本固化**（tag 或 commit SHA），保证任何时间跑出来的结果可比。

#### 测试数据分级（R16）

| Tier | 大小 | 是否入仓 | 何时跑 | 内容 |
|------|------|---------|--------|------|
| **Tier 0：内嵌 fixtures** | < 100 KB | ✅ 跟代码走 | 单元测试 / golden | `rust/crates/attune-core/tests/fixtures/` 5 篇手写 MD（中/英/代码/法律/学术） |
| **Tier 1：小语料** | < 100 MB | ❌ 下载 | 默认集成测试（pre-PR） | rust-book + cs-notes（共 ~160 MB） |
| **Tier 2：大语料** | > 1 GB | ❌ 下载 sparse | nightly / pre-release | technical-books（sparse-checkout 5 子目录） |

**判定规则**：
- 测试运行时间 < 10 s → 必须用 Tier 0
- 验证检索/分词质量 → Tier 1（rust-book 英文 + cs-notes 中文）
- 验证大规模索引/查询 throughput → Tier 2

CI 默认只跑 Tier 0 + Tier 1；Tier 2 用 `cargo test --test '*' -- --ignored --include-ignored` 手动触发。

**Tier 0 多样性现状（R17 audit 2026-05-01）**：5 篇 fixture 覆盖中/英/代码/法律/学术/新闻/技术博客，4 主类齐全。已知 gap：`rust/tests/fixtures/edge_cases/` 目录创建但未填充（空文档 / 10 MB / 非 UTF-8 / 全 emoji / 恶意 HTML），为 **v0.7+ 待补**。

#### 语料库清单

| Corpus | 来源 | 固化版本 | 内容 | 用途 |
|--------|------|---------|------|------|
| **A: rust-book** | github.com/rust-lang/book | tag `1.75.0` (commit `f1e5e4b`) | 500+ 篇 Markdown，Rust 代码块密集 | chunking + 英文 embedding + 搜索相关度 |
| **B: cs-notes** | github.com/CyC2018/CS-Notes | commit `c47a2a7` | 400+ 篇中文算法笔记 | tantivy-jieba 中文分词 + 中英混合 |
| **C: openai-cookbook** | github.com/openai/openai-cookbook | tag `2025-12-01` | Markdown + Jupyter notebook | Notebook 解析 + token-dense 内容 |
| **D: pdl** (可选) | github.com/openlawlibrary/pdl | TBD | 法律类长文档 | law 插件 + 长文档分段 |
| **E: edge cases** | `rust/tests/fixtures/edge_cases/` | 跟代码走 | 空文档/10MB/非 UTF-8/全 emoji/恶意 HTML | 容错与压力 |

**GitConnector 测试语料（2026-05-31）**：GitConnector（`Settings → 从 Git 仓库导入`）复用上表
**A: rust-book**（tag `1.75.0`）+ **B: cs-notes**（commit `c47a2a7`）做真平台仓验证（手动/nightly：
clone → glob → 入库 → BM25/向量可检索 + tantivy-jieba 中文分词）。CI 默认走**本地 bare-repo
fixture**（`crates/attune-core/tests/git_connector.rs`，git2 程序化建仓，无网络、确定性），覆盖
happy/edge（空仓/全二进制/subdir/超长路径）/error（无效 URL/404/ref）/adversarial（SSRF 全表 +
path traversal）/资源耗尽（限额）/i18n（中文 .md）。SSRF + 错误码契约端到端见
`crates/attune-server/tests/git_route_subprocess.rs`。

#### 运行

```bash
# 首次：下载并固化语料
./scripts/download-corpora.sh

# 跑 corpus 集成测试（默认 #[ignore]，需手动触发）
cd rust && cargo test --test corpus_integration -- --ignored
```

### 2.2 Performance Bench（性能基准）

| ID | 测试 | 指标 | 阈值 | v0.6.3 baseline (i9-14900K) | 实装位置 |
|----|------|------|------|--------------------------|---------|
| P-001 | Corpus A 全量注入吞吐 | docs/s | > 20 | TBD (依赖 embedding) | tests/corpus_integration_test.rs |
| P-002 | 单次向量检索（10k chunks） | p95 latency | < 100 ms | TBD | TBD (v0.7+) |
| P-003 | RAG Chat 端到端（本地 LLM） | p95 total | < 3 s | TBD (依赖 LLM) | TBD (v0.7+) |
| P-004 | 并发 10 个查询 | p99 | < 500 ms | TBD | TBD (v0.7+) |
| P-005 | Tantivy 索引写入吞吐 | chunks/s | > 500 | TBD (依赖 index) | TBD (v0.7+) |
| **P-006** | **chunker::chunk (sliding window)** | **docs/s** | **> 50** | **2,535** ✅ | **tests/perf_chunker_bench.rs** |
| **P-007** | **chunker::chunk** | **MB/s** | — | **26.63** ✅ | 同上 |
| **P-008** | **chunker::extract_sections** | **docs/s** | **> 50** | **37,615** ✅ | 同上 |
| **P-009** | **chunker::extract_sections_with_path** (J1) | **docs/s** | **> 50** | **38,116** ✅ | 同上 |

#### 跑 perf bench

```bash
# Phase 1 (实装) — chunker hot path
cd rust && cargo test -p attune-core --test perf_chunker_bench --release -- --ignored --nocapture

# Phase 2+ (规划)
# cargo bench  # 加 criterion dev-dep 后启用
```

#### 解读 baseline 数字

- **chunker 不是瓶颈**：26.63 MB/s 意味着 100 MB corpus chunk 完成只需 ~3.8s。生产 ingest 时间几乎全在 embedding (Ollama bge-m3 ~5-50 docs/s on GPU) + 索引写入。
- **extract_sections 比 chunk 快 14x**（345k vs 24k sections-or-chunks/s）：因为 sections 走 Markdown heading 边界，比滑动窗口少状态。
- **J1 path-prefix 和 plain extract_sections 同 throughput**（38k vs 37k docs/s）：ancestor stack 维护几乎零成本。

#### 阈值断言（防退化）

每个 perf test 内置 `assert!(throughput > N)` — N 是当前 baseline 50% 防回归。CI 跑 `--ignored` 时自动检测。重大优化 PR 提升 baseline + 同步更新阈值。

### 2.3 Quality Regression（质量回归）

#### 2.3.1 K2 Parse Golden Set（CI 门控）

**位置**：`rust/crates/attune-core/tests/fixtures/parse_corpus/`

- `manifest.yaml` 描述每 fixture 的 expected：`title_contains` / `min_text_chars` / `must_contain_phrases` / `section_count_min` / `section_paths_must_include`
- 5 篇 markdown fixture（baseline）

**Harness**：`crates/attune-core/tests/parse_golden_set_regression.rs`

**回归门**：
- baseline 5 篇：`min_pass_rate = 1.0`（必须全过）
- 扩 200 篇：降到 `0.95`（per Readwise Reader 范例）

```bash
cargo test -p attune-core --test parse_golden_set_regression
```

#### 2.3.2 RAGAS 风格 benchmark harness

跑三赛道检索质量评估（法律 legal-track / 英文 rust-book / 中文 cs-notes）：

```bash
# bench-orchestrator.sh 自身完成完整 pipeline（起 headless server → vault setup →
# bind corpus → 等索引 → 跑 queries.json → 报告数字），无需额外 eval 脚本
bash scripts/bench-orchestrator.sh all
```

> 注（2026-05-30 修正）：历史大纲引用的 `scripts/run-final-eval.py` **已不存在**，eval 逻辑已内聚进 `bench-orchestrator.sh`（步骤 6 直接报告指标）。相关辅助：`scripts/run-benchmark-corpus.sh`、`scripts/gen-latest-json.sh`。

输出指标：Hit@10 / MRR / Recall@10。

v0.6.0 GA baseline：`0.80/0.50` (legal) · `1.00/1.00` (rust) · `1.00/1.00` (cs-notes)。

#### 2.3.3 Golden Set precision@K（手维护）

`rust/tests/golden/queries.json`：

```json
[
  {
    "query": "rust 的所有权机制怎么理解",
    "expected_docs": ["rust-book/ch04-00", "rust-book/ch04-01", "rust-book/ch04-02"],
    "min_precision_at_3": 0.66
  }
]
```

> 注（2026-05-30 修正）：历史大纲引用的 `cargo run --bin quality-eval` **bin 不存在**（`attune-server/src/bin/` 仅 `headless.rs`）。precision@K 质量回归走 `bench-orchestrator.sh`（上 §2.3.2）+ `parse_golden_set_regression`（§2.3.1）两条已存在的真实入口；本节 golden queries.json 作为 corpus 集成测试的判据数据使用。

### 2.4 ACP / Agent Flow 质量门（v1.1.0 新增）

v1.1.0 引入 **ACP — Agent Control Plane**（Cost Governor + Agent registry / flow / feedback / scheduler + governed-chat wiring）。这套 capability 的测试门，三视角并存：

| 测试 | 文件 | 视角 | 场景 × 输入 × 期望 | 真跑（2026-05-30） |
|------|------|------|----------------------|---------------------|
| Cost Governor 单元 | `attune-core/src/governor.rs` 内联 | 白盒 | profile 切换 / per-task throttle / output cap / CoT budget / Budget 耗尽 → 拒绝 | 计入 core lib 1499 全绿 |
| Agent telemetry | `attune-core/src/agent_telemetry.rs` 内联 | 白盒 | (agent×model) 失败率累计 / >30% 触发提示 | 同上 |
| Agent flow / scheduler | `attune-core/src/agents/{flow,flow_runner,registry,scheduler}*` 内联 | 白盒 | registry 注册 / flow DAG 推进 / scheduler 调度顺序 / feedback 回写 | 同上 |
| ACP-4 governor wire | `attune-server/tests/acp4_governor_wire_test.rs` | 灰盒 | governor island 接入 chat：cache/usage 透传 + output cap 生效 | **2 passed / 0 failed**（0.09s） |
| ACP-5 chat-flow wire | `attune-server/tests/acp5_chat_flow_wire_test.rs` | 灰盒 | 自主流转：registry→flow→feedback→scheduler 串联进 governed-chat | **3 passed / 0 failed**（0.02s） |
| governor 集成 | `attune-core/tests/governor_integration.rs` | 灰盒 | 端到端预算约束（CI mock 隔离，per CLAUDE.md timing 隔离） | 存在（编译通过） |
| Agent gate orchestrator | `attune-core/tests/agent_gate_orchestrator.rs` | 灰盒 | agent 编排 + 注入对抗（prompt-inj）| 存在 |

**判据**：deterministic agent gate pass rate = 1.00；LLM-judgement agent real-LLM gate F1 ≥ 0.85（走 nightly `#[ignore]` lane，需 ollama，per CLAUDE.md「Agent 验证铁律」）。ACP-1~7 内联门计入 core lib 1499 全绿。

> 关联 spec：`docs/superpowers/specs/2026-05-25-* acp*`（ACP 设计 + 自主流转深化）。新增 agent / 改 governor 时本节矩阵随同更新（per §3.1）。

### 2.5 6 类下限测试矩阵（§6.1 八类下限对照）

代码层 8 类全覆盖；本节把已有测试组织成显式矩阵行（修复历史大纲「测试存在但无矩阵行」的 gap）：

| 类别 | 下限 | 证据文件 | 视角 |
|------|------|---------|------|
| happy path | 各能力主路径 | 全套件主路径 + corpus 集成 | 灰/黑 |
| edge case | 空 / 超长 / 边界 | `golden/*/error/e01-empty-input.yaml`、boundary `#[test]` | 白 |
| error case | 非法输入 / 服务挂 | golden `expected_error`、`oss_agent_real_llm_gate.rs` | 白/灰 |
| **adversarial** | SQLi/XSS/prompt-inj/path-traversal | §3.1 安全表 S-001~S-007、`agent_gate_orchestrator.rs`、`pii_chat_path_redact_test.rs` | 黑 |
| **多并发** | race / N user | `concurrent_stress_test.rs`、`vault::tests::concurrent_lock_unlock_no_race_via_mutex` | 灰 |
| **资源耗尽** | OOM / 盘满 / 大文件 | `oom_behavior_test.rs`、`stress_large_scale_test.rs`、S-003 大文件 DoS | 灰 |
| **i18n** | 中英 / 繁简 / unicode / 非 UTF-8 | `entities_test.rs`、`golden/*/e03-non-utf8-like.yaml`、tantivy-jieba 中文分词套件 | 白/灰 |
| **降级** | LLM 不可用 / network slow | `MockLlmProvider` / `MockEmbeddingProvider` 遍布、`rag_quality_benchmark.rs` | 灰 |

跑法（多并发 / 资源耗尽 / 降级 示例）：

```bash
cd rust
cargo test -p attune-core --test concurrent_stress_test
cargo test -p attune-core --test oom_behavior_test
cargo test -p attune-core --test stress_large_scale_test
# 降级路径默认走 mock，遍布各集成套件（无需额外命令）
```

---

## 3. 安全 + 跨平台测试

### 3.1 安全测试

| ID | 测试 | 预期 | 覆盖能力 |
|----|------|------|---------|
| S-001 | SQL 注入（搜索 query） | 参数化查询，无执行 | F-02-RAG |
| S-002 | XSS 注入（ingest markdown） | 存储时剥离或转义 | F-04-READER |
| S-003 | 大文件 DoS | 强制 size limit，拒绝超限 | F-16-DISTRIBUTION |
| S-004 | 弱口令 | argon2 派生 + 速率限制 | F-01-VAULT |
| S-005 | Session token 伪造 | HMAC 校验 + nonce 递增 | F-01-VAULT |
| S-006 | 无授权访问 | 所有 vault API 返回 403 | F-01-VAULT |
| S-007 | API key 泄露（GET /settings） | redact_api_key 必须生效 | F-09-FORMFACTOR / 配置 |

### 3.2 跨平台测试矩阵（CI）

| OS | 架构 | 编译 | Unit | Integration |
|----|------|------|------|-------------|
| Linux | x86_64 | ✅ | ✅ | ✅ |
| Linux | aarch64 | ✅ | 交叉编译跳过 | - |
| Windows | x86_64 | CI | CI | CI |
| macOS | x86_64 / arm64 | 手动 | 手动 | 手动 |

按 CLAUDE.md 平台优先级：Windows P0 → Linux P1 → macOS 暂不投入资源。

---

## 4. 添加新测试的规范

**每个新 feature 必须配套**：

1. **至少 1 个 Unit test**（贴着实现，覆盖边界）
2. **至少 1 个 Integration test**（跨模块协作）
3. **如果影响用户可见行为** → 加 System 或 E2E 场景
4. **如果涉及算法质量** → 加 Quality Regression entry
5. **如果暴露新 HTTP 端点** → 加 Smoke 一条 curl

**命名规范**（与 §1.3 能力 ID 同步）：

- Unit: `<module>::tests::<scenario>_<expected>` 如 `vault::tests::unlock_with_wrong_password_fails`
- Integration: `tests/<feature_id>_integration.rs` 如 `tests/persona_plugin_integration.rs` (B.2)
- System: `tests/system_<flow>_test.rs` 如 `tests/system_wizard_full_flow_test.rs` (B.3)
- E2E: `tests/e2e_rust/<flow>.spec.ts` 如 `tests/e2e_rust/wizard.spec.ts` (C.3)

**永远不要**：

- 用 `rand` 生成测试数据（不可复现）
- 用 "any integer" / "any string" 这种空洞断言
- 跳过 `cargo test` 直接 commit
- 让 `#[ignore]` 测试永远没跑（至少每周 CI 跑一次）

**始终应当**：

- fixture 文件放 `tests/fixtures/`，版本跟代码走
- 外部语料用 tag/commit 锁定，**绝不**用 `main` 分支
- golden set 质量阈值变动要 PR 评审（不能静默降阈值）
- 性能测试加 baseline 文件，回归时 CI 阻塞

---

## 5. 运行测试 — 命令速查

### 5.1 日常开发循环（< 30s）

```bash
cd rust
cargo test --lib                    # Unit（attune-core lib 1499 tests @ v1.1.0）
cargo test -p attune-core --lib    # 仅 attune-core unit（1499 passed / 4.14s）
cargo test platform::tests          # 仅某模块测试
```

### 5.2 提交前完整跑（< 5min）

```bash
# 必跑 4 层（Unit + Integration + Smoke + Quality）
bash scripts/test-pyramid.sh
```

### 5.3 含真实语料（< 10 min）

```bash
bash scripts/test-pyramid.sh --with-corpus
```

### 5.4 含浏览器 E2E（< 15 min）

```bash
bash scripts/test-pyramid.sh --with-e2e
```

### 5.5 全部跑（release 前）

```bash
bash scripts/test-pyramid.sh --all
```

### 5.6 单项跑

```bash
# Smoke（5 分钟二进制冒烟）
bash scripts/smoke-test.sh        # server 冒烟
bash scripts/smoke-test-cli.sh    # CLI 冒烟

# Quality regression（parse golden set，CI 门）
cd rust && cargo test -p attune-core --test parse_golden_set_regression

# RAGAS benchmark（三赛道检索质量，自含 eval pipeline）
bash scripts/bench-orchestrator.sh all

# ACP / Agent flow wire（v1.1.0）
cd rust && cargo test -p attune-server --test acp4_governor_wire_test
cd rust && cargo test -p attune-server --test acp5_chat_flow_wire_test

# Performance bench（chunker hot path）
cd rust && cargo test -p attune-core --test perf_chunker_bench --release -- --ignored --nocapture
```

### 5.7 CI 验证 (release 前)

```bash
cargo audit                                    # 0 vulnerabilities
cargo clippy --all-targets --all-features      # 0 errors (警告需 review)
cargo fmt --all -- --check                     # 0 diffs (新代码必须)
cargo test                                     # 全绿
cargo build --release                          # 编译产物 OK
```

---

## 6. CI 流水线

实装于 `.github/workflows/ci.yml`（rust-test job 已含 `[ubuntu-latest, windows-latest]` matrix，per M6 ✅）+ `rust-release.yml`（多平台 release artifact，build-only 不跑 test）。下方为参考骨架（PR gate = unit + integration；corpus / quality regression 走 schedule）：

```yaml
on: [push, pull_request]

jobs:
  unit-and-integration:
    runs-on: ${{ matrix.os }}
    strategy:
      matrix:
        os: [ubuntu-latest, windows-latest]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cd rust && cargo test
      - run: cd rust && cargo build --release  # 编译验证

  smoke:
    runs-on: ubuntu-latest
    needs: unit-and-integration
    steps:
      - run: bash scripts/smoke-test.sh

  corpus-integration:
    runs-on: ubuntu-latest
    needs: smoke
    steps:
      - run: ./scripts/download-corpora.sh
      - run: cd rust && cargo test --test corpus_integration -- --ignored

  quality-regression:
    runs-on: ubuntu-latest
    needs: corpus-integration
    if: github.event_name == 'schedule'  # 周级，不是每 PR
    steps:
      - run: bash scripts/bench-orchestrator.sh all   # 自含 eval pipeline，报告 Hit@10/MRR/Recall
      # precision 降 > 5% 自动开 issue
```

---

## 7. 成熟度路线

| 阶段 | 里程碑 | 状态 |
|------|--------|------|
| M1 | Unit + Integration 基线 | ✅ — v1.1.0 实测 attune-core lib **1499** + **87** 集成套件 |
| M2 | Corpus A/B 真实语料接入 + Quality regression baseline | ✅ v0.6.0 GA |
| M3 | System 测试 (wizard 完整链路) | ✅ — `system_wizard_full_flow_test` 等已落（慢测走 nightly） |
| M4 | E2E Playwright + 真机 deb 验收 | ✅ — AMD 笔电 deb 真机线已建（§1.4） |
| M5 | Smoke 升级覆盖新能力 | ✅ — server + CLI 双冒烟 |
| M6 | CI 矩阵跨 Linux + Windows 全绿 | ✅ — `ci.yml` rust-test matrix [ubuntu, windows]（R18 落地） |
| M6.1 | 慢测试拆 nightly 通道 | ✅ — 76 `#[ignore]`（real-LLM gate / corpus / Argon2id 慢测），PR CI 跑 fast lane |
| M6.2 | Flakiness baseline | ✅ R20 实测 — 多 runs 0 失败；runtime 变异 < 2× 属系统噪声 |
| M7 | Performance baseline + 跨版本对比 | 🟡 — chunker hot path baseline 在（§2.2 P-006~P-009）；检索/RAG p95 待补 |
| M8 | 发版前强制 M1-M6 全绿 | ✅ — RC 四节门（per CLAUDE.md §7.2） |
| **M9** | **ACP — Agent Control Plane 测试体系（v1.1.0）** | ✅ — Cost Governor + agent flow/scheduler 内联门计入 1499 全绿 + ACP-4/5 wire 集成（§2.4） |

### 7.1 版本 trace（v0.6 → v1.1.0）

| 版本 | 测试体系增量 |
|------|------------|
| v0.6.0/0.6.1 GA | 5 层金字塔建立 + corpus A/B + parse golden set CI 门 + FormFactor 测试 |
| v0.6.3 | PII redact 全 chat 路径接入 + chunker perf baseline + 安全表 S-001~S-007 |
| v0.7 | Email/WebDAV/RSS 采集源测试矩阵 + 多层记忆 L0~L3 + SourceConnector 抽象 + 两级侧边栏（见各专节）|
| v0.8 | law-pro 6 agent 闭环 backfill + agent_golden_gate 6 类下限门（attune-pro）|
| v0.9.0/0.9.5 | 4 新 law-pro agent + E2E + Playwright + perf baseline 横切 |
| v1.0.0 GA | CI 3 fail 修齐 + 多平台 install pkg + 真机 deb 验收线 |
| v1.0.x（11 minor）| 升级策略 / DSAR / observability / security / 性能 scale / DR / billing 等缺口闭环 |
| **v1.1.0** | **ACP — Agent Control Plane**：Cost Governor（cache/usage island + output cap + CoT budget）+ agent registry/flow/feedback/scheduler 自主流转 + governed-chat wiring（§2.4）|

---

## 附录 D：4h × 40 轮深度审视发现登记（R15-R40，2026-05-01）

> 这是一次系统性"代码 + 测试 + 产品质量"全面体检的产出，作为 v0.7+ backlog 锚点。

### 测试质量（R15-R20，已落 M6.1/M6.2）

| Round | 维度 | 发现 |
|-------|------|------|
| R15 | vault 并发 | Vault `!Sync` (rusqlite RefCell) 必须外层 `Mutex<Vault>`；2 新 concurrent test 已加 |
| R16 | 测试数据分级 | Tier 0/1/2 表已加 |
| R17 | golden corpus | 4 主类齐全；edge_cases/ 空待 v0.7+ |
| R18 | CI 矩阵 | ci.yml 仅 ubuntu-latest，Windows test 缺 |
| R19 | P95 runtime | 慢测试 ~6 min；建议拆 nightly |
| R20 | flakiness | 0 失败 in 30 runs |

### 代码质量（R21-R30）

| Round | 维度 | 发现 | 行动 |
|-------|------|------|------|
| R21 | unsafe 审计 | 仅 1 处 (state.rs:145 `set_var`) — 极佳 | 保持 |
| R22 | clippy::pedantic | ~500 风格 warning，**0 correctness** | v0.8+ 风格清理 |
| R23 | dead_code | 6 项（QueueWorker / is_allowed / 等） | v0.7+ 接入或删除 |
| R24 | error chain | 单一 `VaultError` thiserror 一致 | ✅ 保持 |
| R25 | lock map | Store mutex 写读串行化 — 已知 contention | v0.7+ 拆 read pool |
| R26 | alloc hot path | chunker 26.63 MB/s 证实非瓶颈 | ✅ |
| R27 | Send+Sync | 0 unsafe impl，全自动推导 | ✅ |
| R28 | log 风格 | tracing×92 + log×65 双栈混用 | v0.7+ 统一 tracing |
| R29 | async timeout | HTTP 客户端全带 timeout | ✅ |
| R30 | panic-free | 生产 unwrap < 10/file，865 总数大部分在 #[cfg(test)] | ✅ |

### 产品质量（R31-R40）

| Round | 维度 | 实测 | 行动 |
|-------|------|------|------|
| R31 | startup time | ~18 ms cold start | ✅ |
| R32 | binary size | **47 MB stripped / 59 MB unstripped**（与 README "~30 MB" 不符） | ✅ 已修文档 |
| R33 | memory peak | governor 已实装 cpu_pct_max + Budget | ✅ |
| R34 | CPU caps | 同上 governor 路径 | ✅ |
| R35 | shutdown graceful | ❌ 无 ctrl_c/with_graceful_shutdown，SIGTERM 直接 kill | **v0.7+ P0** |
| R36 | config validation | settings.rs 无 validate() | v0.7+ 加白名单 |
| R37 | log persistence | 仅 stdout，无文件 rotation | v0.7+ tracing_appender |
| R38 | backup/restore | ❌ 无 vault export/import CLI | v0.7+ 加 |
| R39 | doc consistency | "~30 MB" → 47 MB stripped | ✅ 已修 README + CLAUDE.md |
| R40 | synthesis | 本表 | — |

### v0.7+ 产品级 P0 backlog（来自 R15-R40）— 2026-05-01 全部修复

| # | 项 | Round | 落地 |
|---|----|-------|------|
| 1 | **graceful shutdown** | R35 | ✅ `lib.rs` SIGINT/SIGTERM oneshot + axum_server::Handle.graceful_shutdown(30s) — 实测 `exit=0` |
| 2 | **CI Windows job** | R18 | ✅ `ci.yml` rust-test 加 matrix [ubuntu-latest, windows-latest] |
| 3 | **慢测试 #[ignore]** | R19 | ✅ 6 tests 加 `#[ignore]` (form_factor×4 + wizard×1 + vault_setup×1)；attune-server 测试时间 6 min → 18s（20× 加速）+ nightly schedule |
| 4 | **vault backup/restore CLI** | R38 | ✅ `attune vault-export <dest>` + `attune vault-import <src> [--force]`，含 vault 状态守卫 |
| 5 | **log file rotation** | R37 | ✅ `tracing_appender::rolling::daily` → `data_dir/logs/attune-server.YYYY-MM-DD`，stdout 同步 |
| 6 | **dead_code 接入** | R23 | ✅ 抽 `attune_core::queue::embed_and_index_batch` 共享函数；server `start_queue_worker` 与 core `QueueWorker::process_embed_batch` 共用一份 batch logic，消除 ~50 行重复 |

---

## Email IMAP 采集源 + SourceConnector 抽象测试矩阵（v0.7）

Email IMAP 采集源与 `SourceConnector` 统一抽象（`ingest/connector.rs`）的测试覆盖。
实现计划：`docs/ingest/email-implementation-plan.md`。

| 层 | 文件 | 覆盖 |
|----|------|------|
| Unit — connector | `ingest/connector.rs` 内联 | `SourceKind::as_str` 稳定字符串；`RawDocument` 字段构造；`SourceConnector` trait 驱动 sink 回调 |
| Unit — email parse | `ingest/email.rs` 内联 | `html_to_text` 剥 HTML 标签；style/script block 过滤 |
| Unit — pipeline enum | `ingest/pipeline.rs` 内联 | `IngestOutcome` derive 特征（Debug/Clone/PartialEq/Eq）四 variant 全覆盖 |
| Integration — email | `tests/ingest_email_test.rs` | `parse_email_bytes`（plain/HTML/attachment/invalid）；`EmailConnector` mock fetcher；UID 增量游标；attachment RawDocument 独立产出；正文 RawDocument 可过 `ingest_document` |
| Integration — pipeline | `tests/ingest_pipeline_test.rs` | `ingest_document` 四态（Inserted/Duplicate/Updated/Skipped）；domain/tags 透传；corpus_domain 前缀；`ingest_document_replacing` + 第三方 hash 防护；`ingest_document_with_profile` 命名 profile；raw.title 优先于 parser title |
| Manual | `python/tests/MANUAL_TEST_CHECKLIST.md` § "Email IMAP 采集源" | 添加 IMAP 账号、手动同步、UID 游标增量、附件索引 — 需真实 IMAP 账号，不进 CI |

跑法：

```bash
cd rust
cargo test -p attune-core --lib ingest                      # unit tests
cargo test -p attune-core --test ingest_email_test          # email integration
cargo test -p attune-core --test ingest_pipeline_test       # pipeline integration
```

## 两级侧边栏导航测试矩阵（v0.7）

两级侧边栏（`rust/crates/attune-server/ui/src/layout/Sidebar.tsx`：主级 PRIMARY_NAV 常驻 +
次级 MORE_NAV 折叠组）当前无 UI 自动化测试层（E2E Playwright 层在 C.2 后才建立，
`ui/package.json` 只有 `build`/`typecheck`，无 vitest/jest）。

| 层 | 文件 | 覆盖 |
|----|------|------|
| Unit | — | TypeScript 类型检查：`npm run typecheck`（`tsc --noEmit`） |
| E2E | `tests/e2e_rust/`（C.2 规划后） | 待建立 Playwright 层后补充导航交互测试 |
| Manual | `python/tests/MANUAL_TEST_CHECKLIST.md` § "两级侧边栏导航" | 主级常驻可见、折叠模式图标、"更多"展开/折叠、激活指示器、活跃视图自动展开、Settings 位置 |

跑法：

```bash
cd rust/crates/attune-server/ui
npm run typecheck   # TypeScript 类型检查（覆盖 Sidebar.tsx props/signal 类型）
```

人工验收在 `python/tests/MANUAL_TEST_CHECKLIST.md` 维护，每次 release 前必须人工跑一遍。

## RSS / Atom 采集源测试矩阵（v0.7，2026-05-20）

第三采集源 RSS（继 Email/WebDAV 之后）的测试覆盖。实现 `ingest/rss.rs` +
`store/rss_feeds.rs` + `ingest_rss.rs`（server-layer sync）+ `start_rss_sync_worker`
（周期 worker）+ `routes/rss.rs`（5 个 REST endpoint）。

| 层 | 文件 | 覆盖 |
|----|------|------|
| Unit — connector | `ingest/rss.rs` 内联 | RSS 2.0 + Atom 解析；HTML body 剥标签；entry dedup（last_entry_guid 命中即 break）；304 路径不 emit；200 last_response 透出 ETag/Last-Modified；垃圾 XML 拒绝 |
| Unit — store CRUD | `tests/rss_feeds_test.rs` | add/get/list/delete 全流程；URL 加密落盘 + 解密回明文；明文 URL 绝不在 BLOB 里；update_etag_lastmod / touch_polled_at / update_last_entry / update_feed_settings 幂等性 |
| Integration — connector | `tests/ingest_rss_test.rs` | 端到端 first-poll → 全 emit；conditional-GET 透传 ETag；dedup invariant（cursor 推进后二次 poll → 0 新条目）；fetch Err 传播；空 entry 跳过；RawDocument 真正过 ingest_document |
| Manual | `python/tests/MANUAL_TEST_CHECKLIST.md` § "RSS 订阅采集源"（待补） | 添加真实 LWN / GitHub releases RSS；poll-now；周期 worker 到期触发；304 路径；删除订阅；禁用订阅 |

跑法：

```bash
cd rust
cargo test -p attune-core --lib ingest::rss          # 8 个内联 connector unit test
cargo test -p attune-core --test rss_feeds_test      # 10 个 store CRUD test
cargo test -p attune-core --test ingest_rss_test     # 8 个端到端 integration test
```

全部 release 模式 <1s 完成；无 `#[ignore]`，进 CI。

mailing-list 备注：开源项目邮件列表（LWN / lkml.org / kernel newbies 等）多数发布
web RSS 镜像 —— 用 RSS 订阅这条路径即可。订阅了真 IMAP 邮件列表的用户走
EmailConnector（INBOX 文件夹），不在 RSS 这里重复支持。

## 多层记忆测试矩阵（2026-05-18）

多层记忆系统（L0 raw → L1 chunk summary → L2 episodic → L3 semantic + tier-aware
assembler）的测试覆盖。设计稿 `docs/superpowers/plans/2026-05-18-multilayer-memory.md`。

| 层 | 文件 | 覆盖 |
|----|------|------|
| Unit — store | `store/memory_vectors.rs`、`store/memories.rs` 内联 | memory_vectors CRUD + 级联删除；insert_semantic_memory topic_key 幂等；mark_memory_superseded；demote_cold_memories |
| Unit — retrieval | `memory/retrieval.rs` 内联 | MemoryVectorIndex upsert/search/维度防护；search_memories 相关性排序、时间过滤、冷记忆排除 |
| Unit — semantic | `memory/semantic.rs` 内联 | hdbscan 主题聚类；topic_key 跨重跑幂等；subset 主题 supersede |
| Unit — assembler | `memory/assembler.rs` 内联 | classify_query_shape（recall/overview/precise）；coverage gate；assembler-off == L0；compact_history 缓存命中 |
| Integration | `tests/multilayer_memory_integration.rs` | 完整 L0→L1→L2→L3 生命周期；recall/overview/precise 路由；冷降级；assembler on/off 等价 |
| Benchmark | `tests/memory_token_reduction_benchmark.rs` | §5.3 验收指标 — 注入 token 数 assembler on vs off。实测 recall+overview 子集 median 降幅 78.7%，precise 子集 0% |

跑法：

```bash
cd rust
cargo test -p attune-core memory                              # 全部 unit
cargo test -p attune-core --test multilayer_memory_integration
cargo test -p attune-core --test memory_token_reduction_benchmark -- --nocapture
```

benchmark 走确定性 MockEmbeddingProvider，无 LLM / 无网络，进 CI（<1s）。

---

## 附录 A：人工验收清单

某些 UX / 集成场景无法自动化（需要真实 Chrome 实例 / 真实 USB / 真实账号登录等），这些用 [`python/tests/MANUAL_TEST_CHECKLIST.md`](../python/tests/MANUAL_TEST_CHECKLIST.md) 维护勾选式步骤（含 v0.7 Memory Moat 验收节）。

每次 release 前，必须人工跑一遍清单。

## 附录 B：语料下载脚本

`scripts/download-corpora.sh`（规划 / 已实装部分）：

```bash
#!/bin/bash
# 下载并固化测试语料库到 rust/tests/corpora/
# 每个语料用 git clone --depth 1 -b <tag> 锁版本

mkdir -p rust/tests/corpora
cd rust/tests/corpora

# Corpus A
[ -d rust-book ] || git clone --depth 1 -b 1.75.0 https://github.com/rust-lang/book rust-book

# Corpus B
[ -d cs-notes ] || git clone --depth 1 https://github.com/CyC2018/CS-Notes cs-notes
cd cs-notes && git checkout c47a2a7 && cd ..

# Corpus C
# Corpus D / E 类似
```

## 附录 C：能力 ID 与本文档的关系

> 历史上 FEATURES.md 是能力清单 SSOT、与本文互为反向索引；该文件已于 86e3833 合并进 README（避免同主题双副本 per §3.2）。能力 ID（`F-{nn}-{TOPIC}`）映射现内联于本文 §1.3。

- **能力清单**（"产品有哪些能力"）→ [`../README.md`](../README.md)
- **测试方案**（本文，"这层测试在测什么"）→ 按测试层组织，§1.3 给能力 ID ↔ 层映射

新增/修改测试时，**必须**：
1. 在测试 case 注释里 cite 能力 ID（如 `// covers F-09-FORMFACTOR`）
2. 如果填补了缺口，在 §1.3 矩阵、§2.4 ACP 矩阵、§2.5 6 类下限矩阵 和 §7 成熟度路线相应更新

# Attune Testing Guide

> **目标**：产品级测试，可复现、可追溯、覆盖用户真实场景。
> **配套**：[`FEATURES.md`](FEATURES.md) — 18 条能力清单，每条配测试覆盖映射。本文档反向组织（按测试层 → 覆盖哪些能力）。
> **双语**：[English](TESTING.md) (本文) / 中文术语在每段并列。

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

### 1.2 现有覆盖统计（v0.6.1 GA）

| 层 | 文件位置 | 已有套件 | 已有 case |
|----|---------|---------|----------|
| Unit | 各 crate `src/**/tests` 内联 | 535 attune-core + 5 attune-server lib + 3 headless | 543 |
| Integration | `crates/attune-core/tests/`、`crates/attune-server/tests/`、`rust/tests/` | 20 套件 | 87 |
| System | `rust/tests/corpus_integration_test.rs`、`server_test.rs` | 2 套件 | 部分 |
| E2E | `tests/e2e_rust/`（C.2 后） | ❌ 0 | 0 |
| Smoke | `scripts/smoke-test.sh` | 1 脚本 | 5 项检查 |

**v0.6.1 总测试**：630 passed / 0 failed / 3 ignored 跨 19 套件（含 8 个新增 form_factor 测试）。

### 1.3 能力 ↔ 测试层覆盖矩阵

完整矩阵见 [`FEATURES.md` §4](FEATURES.md#4-capability-↔-test-layer-coverage-map)。每条能力用 ID 引用（`F-{nn}-{TOPIC}`）。

简版概览（v0.6.1 GA 状态）：

| 测试层 | 完整覆盖 | 部分覆盖 | 空白 |
|--------|---------|---------|------|
| Unit | F-01 ~ F-04, F-07, F-09, F-10, F-12 ~ F-14, F-16, F-18 | F-05, F-06, F-08, F-11, F-17 | F-15 |
| Integration | F-01, F-02, F-04, F-05, F-07, F-10, F-12 ~ F-14, F-16, F-18 | F-03, F-06, F-08 | F-09, F-11, F-15, F-17 |
| System | F-01, F-02 (corpus), F-16, F-18 | F-03, F-04, F-06 | F-05, F-07 ~ F-15, F-17 |
| E2E | F-08 (扩展自有) | F-01, F-03, F-04 | F-02 ~ F-07, F-09 ~ F-18 |
| Smoke | F-01, F-16 | F-02, F-09 | F-03 ~ F-08, F-10 ~ F-15, F-17, F-18 |

**驱动当前任务定义**：B.1 / B.2 / B.3 / C.1 / C.3 都从这张表的"空白" + "部分"格倒推。

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

#### 语料库清单

| Corpus | 来源 | 固化版本 | 内容 | 用途 |
|--------|------|---------|------|------|
| **A: rust-book** | github.com/rust-lang/book | tag `1.75.0` (commit `f1e5e4b`) | 500+ 篇 Markdown，Rust 代码块密集 | chunking + 英文 embedding + 搜索相关度 |
| **B: cs-notes** | github.com/CyC2018/CS-Notes | commit `c47a2a7` | 400+ 篇中文算法笔记 | tantivy-jieba 中文分词 + 中英混合 |
| **C: openai-cookbook** | github.com/openai/openai-cookbook | tag `2025-12-01` | Markdown + Jupyter notebook | Notebook 解析 + token-dense 内容 |
| **D: pdl** (可选) | github.com/openlawlibrary/pdl | TBD | 法律类长文档 | law 插件 + 长文档分段 |
| **E: edge cases** | `rust/tests/fixtures/edge_cases/` | 跟代码走 | 空文档/10MB/非 UTF-8/全 emoji/恶意 HTML | 容错与压力 |

#### 运行

```bash
# 首次：下载并固化语料
./scripts/download-corpora.sh

# 跑 corpus 集成测试（默认 #[ignore]，需手动触发）
cd rust && cargo test --test corpus_integration -- --ignored
```

### 2.2 Performance Bench（性能基准）

| ID | 测试 | 指标 | 阈值 |
|----|------|------|------|
| P-001 | Corpus A 全量注入吞吐 | docs/s | > 20 |
| P-002 | 单次向量检索（10k chunks） | p95 latency | < 100 ms |
| P-003 | RAG Chat 端到端（本地 LLM） | p95 total | < 3 s |
| P-004 | 并发 10 个查询 | p99 | < 500 ms |
| P-005 | Tantivy 索引写入吞吐 | chunks/s | > 500 |

```bash
cd rust && cargo bench
# 报告：target/criterion/report/index.html
# 历史对比：每个 release commit 的 bench 快照
```

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

跑三赛道检索质量评估（法律 lawcontrol / 英文 rust-book / 中文 cs-notes）：

```bash
bash scripts/bench-orchestrator.sh all
python3 scripts/run-final-eval.py
```

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

```bash
cd rust && cargo run --release --bin quality-eval
# 输出：[OK] / [REGRESSION]，下降 > 5% 视为回归
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

**命名规范**（与 FEATURES.md ID 同步）：

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
cargo test --lib                    # Unit 535+ tests
cargo test -p attune-core --lib    # 仅 attune-core unit
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
bash scripts/smoke-test.sh

# Quality regression（precision@K）
cd rust && cargo run --release --bin quality-eval

# RAGAS benchmark
bash scripts/bench-orchestrator.sh all && python3 scripts/run-final-eval.py

# Performance bench
cd rust && cargo bench
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

`.github/workflows/test.yml`（规划，部分已实装）：

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
      - run: bash scripts/bench-orchestrator.sh all
      - run: python3 scripts/run-final-eval.py
      # precision 降 > 5% 自动开 issue
```

---

## 7. 成熟度路线

| 阶段 | 里程碑 | 状态 |
|------|--------|------|
| M1 | Unit 535+ + Integration 87+ | ✅ v0.6.1 |
| M2 | Corpus A/B 真实语料接入 + Quality regression baseline | ✅ v0.6.0 GA |
| M3 | System 测试 (wizard 完整链路) | 🚧 B.3 推进中 |
| M4 | E2E Playwright + 3 golden flow | 🚧 C.2 + C.3 推进中 |
| M5 | Smoke 升级覆盖 v0.6.1 新能力 | 🚧 C.1 推进中 |
| M6 | CI 矩阵跨 Linux + Windows 全绿 | 🟡 部分 (Linux ✅，Windows 待) |
| M7 | Performance baseline + 跨版本对比 | ❌ 待做 |
| M8 | 发版前强制 M1-M5 全绿 | ❌ 待做 |

---

## 附录 A：人工验收清单

某些 UX / 集成场景无法自动化（需要真实 Chrome 实例 / 真实 USB / 真实账号登录等），这些用 [`tests/MANUAL_TEST_CHECKLIST.md`](../tests/MANUAL_TEST_CHECKLIST.md) 维护勾选式步骤。

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

## 附录 C：与 FEATURES.md 的关系

- **FEATURES.md** 主轴：能力 → 测试覆盖（"我的能力被哪些测试覆盖？"）
- **TESTING.md**（本文）主轴：测试层 → 覆盖哪些能力（"这层测试在测什么？"）
- 两者共享同一份能力 ID（`F-{nn}-{TOPIC}`），互为反向索引

新增/修改测试时，**必须**：
1. 在测试 case 注释里 cite 能力 ID（如 `// covers F-09-FORMFACTOR`）
2. 在 FEATURES.md 对应能力的"测试覆盖"段更新覆盖项
3. 如果填补了缺口，在 §1.3 矩阵和 §7 成熟度路线相应更新

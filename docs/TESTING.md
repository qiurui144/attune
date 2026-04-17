# Attune Testing Guide

**目标**：产品级测试，可复现、可追溯、覆盖用户真实场景。

**非目标**：
- 随机生成测试数据（行为不可复现）
- 只有 unit test（缺真实用户场景覆盖）
- 只追"测试数量"不看"质量指标"

---

## 1. 测试金字塔

```
           ┌─────────────────┐
           │  E2E (Chrome)   │  浏览器交互、跨 API 流程
           └─────────────────┘
         ┌─────────────────────┐
         │  Integration + 语料 │  真实 GitHub 知识库注入 + 检索质量
         └─────────────────────┘
       ┌─────────────────────────┐
       │  Unit（237 existing）   │  纯逻辑、边界、错误路径
       └─────────────────────────┘

      + Performance (criterion benchmarks)
      + Quality Regression (golden-set precision)
```

每一层的职责：

| 层 | 数据源 | 回归耗时 | 覆盖 |
|----|-------|---------|------|
| Unit | 内存 fixture | < 30s | 纯算法、错误分支 |
| Integration | 本地 mock store | < 2 min | 跨模块协作 |
| **Corpus Integration** | **Pinned GitHub repo** | **2-5 min** | **真实用户场景** |
| E2E | 真实二进制 + Playwright | 5-10 min | 用户可见路径 |
| Performance | Pinned corpus | 跨版本对比 | 性能回归 |
| Quality Regression | Golden set (每季度更新) | 5-10 min | 搜索/Chat 质量不降 |

---

## 2. 测试语料库（Test Corpora）

**核心原则**：语料 **版本固化**（tag 或 commit SHA），保证任何时间跑出来的结果可比。

### 2.1 Corpus A：`rust-lang/book`（技术类英文）

- **来源**：https://github.com/rust-lang/book
- **固化版本**：tag `1.75.0`（commit `f1e5e4b`）
- **内容**：500+ 篇 Markdown、Rust 代码块密集
- **测试用途**：
  - chunking（章节边界 + 代码块保留）
  - 英文 embedding
  - tech 插件分类（应识别为"rust / systems programming"）
  - 搜索相关度（给定查询 → 期望文档在 top-5）

### 2.2 Corpus B：`CyC2018/CS-Notes`（技术类中文）

- **来源**：https://github.com/CyC2018/CS-Notes
- **固化版本**：commit `c47a2a7`
- **内容**：400+ 篇中文算法笔记 + 面试题
- **测试用途**：
  - tantivy-jieba 中文分词
  - 中英混合查询
  - 中文 embedding 质量
  - tech 插件对中文的兼容

### 2.3 Corpus C：`openai/openai-cookbook`（AI/多模态）

- **来源**：https://github.com/openai/openai-cookbook
- **固化版本**：tag `2025-12-01`
- **内容**：Markdown + Jupyter notebook（.ipynb）混合
- **测试用途**：
  - Notebook 解析（如实现）
  - 代码与说明文字的分块
  - embedding 对 token-dense 内容的鲁棒性

### 2.4 Corpus D：`openlawlibrary/pdl`（法律类，可选）

- **来源**：https://github.com/openlawlibrary/pdl
- **固化版本**：commit TBD
- **用途**：law 插件、长文档分段

### 2.5 Corpus E：合成边界用例（Synthetic Edge Cases）

维护在 `rust/tests/fixtures/edge_cases/` 下：

- 空文档
- 10 MB 纯文本（压力测试 chunker）
- 非 UTF-8 字节序列（容错）
- 嵌入二进制（容错）
- 全 emoji 文档（tokenization 边界）
- 超长单行（超过 chunk 窗口 5x）
- Markdown + 恶意 HTML（XSS 防御）

---

## 3. 测试矩阵（Test Matrix）

### 3.1 功能测试

| ID | 测试 | 语料 | 预期 | 当前状态 |
|----|------|------|------|----------|
| F-001 | 注入 Corpus A 500 个文档 | A | 全部入库，无失败 | 待实现 |
| F-002 | 注入 Corpus B 中文分词正确 | B | jieba tokens 至少 30k | 待实现 |
| F-003 | 注入后搜索 "rust ownership" | A | rust-book 中 ownership 相关章节 top-3 | 待实现 |
| F-004 | 注入后搜索 "动态规划" | B | CS-Notes 中 DP 章节 top-3 | 待实现 |
| F-005 | tech 插件分类 | A | Rust 文档被标记 lang=rust | 待实现 |
| F-006 | 边界：10MB 文档 ingest | E | 不 panic、正常分块 | ✅ chunker tests |
| F-007 | 边界：非 UTF-8 | E | 容错处理（lossy 转换） | 已有部分 |
| F-008 | 浏览器搜索 fallback | Web | 本地无结果 → 浏览器搜索 | 本次新增 Rust 单元测试 |
| F-009 | 技能进化触发 | 合成 | 10 次失败信号后扩展词入库 | ✅ skill_evolution tests |

### 3.2 性能测试（criterion.rs benchmarks）

| ID | 测试 | 指标 | 阈值 |
|----|------|------|------|
| P-001 | Corpus A 全量注入 | throughput (docs/s) | > 20 docs/s |
| P-002 | 单次向量检索（10k chunks） | p95 latency | < 100 ms |
| P-003 | RAG Chat 端到端 | p95 total | < 3 s（本地 LLM） |
| P-004 | 并发 10 个查询 | p99 | < 500 ms |
| P-005 | Tantivy 索引写入吞吐 | chunks/s | > 500 chunks/s |

### 3.3 质量回归（Golden Set）

维护一组"标准 QA 对"作为质量基线，在 `rust/tests/golden/queries.json`：

```json
[
  {
    "query": "rust 的所有权机制怎么理解",
    "expected_docs": ["rust-book/ch04-00", "rust-book/ch04-01", "rust-book/ch04-02"],
    "min_precision_at_3": 0.66
  },
  {
    "query": "什么是动态规划",
    "expected_docs": ["cs-notes/dp-intro", "cs-notes/dp-examples"],
    "min_precision_at_3": 0.50
  }
]
```

每次变更后跑 `cargo run --bin quality-eval` 对比当前 precision 与基线，下降 > 5% 视为回归需要人工审查。

### 3.4 安全测试

| ID | 测试 | 预期 |
|----|------|------|
| S-001 | SQL 注入（搜索查询） | 参数化查询，无执行 |
| S-002 | XSS 注入（ingest markdown） | 存储时剥离或转义 |
| S-003 | 大文件 DoS | 强制 size limit，拒绝超限 |
| S-004 | 密码弱口令 | argon2 派生，速率限制 |
| S-005 | 会话 token 伪造 | HMAC 验证，nonce 递增 |
| S-006 | 无授权访问 | 所有 vault API 返回 403 |

### 3.5 跨平台测试（CI 矩阵）

| OS | 架构 | 编译 | Unit | Integration |
|----|------|------|------|-------------|
| Linux | x86_64 | ✅ | ✅ | ✅ |
| Linux | aarch64 | ✅ | 交叉编译跳过 | - |
| Windows | x86_64 | CI | CI | CI |
| macOS | x86_64 / arm64 | 手动 | 手动 | 手动 |

---

## 4. 运行测试

### 4.1 日常开发循环

```bash
# 跑所有单元 + integration 测试（已有 237 个）
cd rust && cargo test

# 跑特定 crate
cargo test -p attune-core

# 跑带集成测试标签的
cargo test --test server_test
```

### 4.2 Corpus-based 集成测试

```bash
# 首次：下载语料（版本固化）
./scripts/download-corpora.sh

# 跑 corpus 测试（默认 #[ignore]，需手动触发）
cd rust && cargo test --test corpus_integration -- --ignored
```

### 4.3 性能基准

```bash
cd rust && cargo bench
# 结果保存在 target/criterion/report/index.html
# 对比历史：git log 里每个 release commit 的 bench 快照
```

### 4.4 质量回归

```bash
# 先装 Ollama 和拉 bge-m3 模型
ollama pull bge-m3

# 跑质量评估
cd rust && cargo run --release --bin quality-eval

# 输出：
# [OK] query=rust_ownership  precision@3=1.00 (baseline=0.66)
# [REGRESSION] query=DP       precision@3=0.33 (baseline=0.50, -34%)
```

### 4.5 E2E（Playwright）

```bash
# 启动服务
cd rust && cargo run --release --bin attune-server &

# 另一个 shell 跑 Playwright
cd tests/e2e && npm test
```

---

## 5. CI 流水线

`.github/workflows/test.yml`（规划）：

```yaml
on: [push, pull_request]

jobs:
  unit:
    runs-on: ${{ matrix.os }}
    strategy:
      matrix:
        os: [ubuntu-latest, windows-latest, macos-latest]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cd rust && cargo test

  corpus-integration:
    runs-on: ubuntu-latest
    needs: unit
    steps:
      - uses: actions/checkout@v4
      - run: ./scripts/download-corpora.sh
      - run: cd rust && cargo test --test corpus_integration -- --ignored

  quality-regression:
    runs-on: ubuntu-latest
    needs: corpus-integration
    # 周级调度，不是每 PR 跑
    if: github.event_name == 'schedule'
    steps:
      - run: ./scripts/eval-quality.sh
      - run: # 比对 baseline，precision 降 > 5% 发 issue
```

---

## 6. 添加新测试的规范

**每个新 feature 必须配套**：

1. **至少 1 个 unit test**（贴着实现，覆盖边界）
2. **至少 1 个 integration test**（跨模块协作）
3. **如果影响用户可见行为**：加 corpus-based 或 E2E 场景
4. **如果涉及算法质量**：加 golden-set entry

**永远不要**：

- 用 `rand` 生成测试数据（结果不可复现）
- 用 "any integer" / "any string" 这种空洞断言
- 跳过 `cargo test` 直接 commit
- 让 `#[ignore]` 测试永远没跑（至少每周 CI 跑一次）

**始终应当**：

- fixture 文件放 `tests/fixtures/` 下，版本跟代码走
- 外部语料用 tag/commit 锁定，不用 `main` 分支
- golden set 质量指标变动要 PR 评审（不能静默降阈值）
- 性能测试加 baseline 文件，回归时 CI 阻塞

---

## 7. 成熟度路线

| 阶段 | 里程碑 | 状态 |
|------|--------|------|
| M1 | Unit 237 + 基础 integration | ✅ 当前 |
| M2 | Corpus A/B 两个真实语料接入 | 🚧 本文档启动 |
| M3 | Performance benchmark baseline | 待做 |
| M4 | Golden-set 质量回归 + CI 告警 | 待做 |
| M5 | E2E Playwright + 跨平台 CI 矩阵 | 待做 |
| M6 | 发版前强制 M1-M5 全绿 | 待做 |

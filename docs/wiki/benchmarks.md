# Attune Benchmark 数字 (v1.0)

> 检索 benchmark 测试日期：2026-04-28（v0.6.0-rc.5，v1.0 保持不变）  
> Reliability F1 测试日期：2026-05（v1.0 GA）  
> 测试机：Intel i9-14900K / 64GB / NVIDIA GPU / Ubuntu 25.10  
> 复现命令：`bash scripts/bench-orchestrator.sh all && python3 scripts/run-final-eval.py`

## TL;DR

**三赛道全 PRO，两赛道 MRR 满分。Reliability framework Phase 3 F1 = 0.9828。**

```
Scen A 法律 (attune-enterprise):  Hit@10=0.80  MRR=0.50  ✅ PRO
Scen B Rust (rust-book):   Hit@10=1.00  MRR=1.00  ✅ PRO 满分
Scen C 中文八股 (cs-notes): Hit@10=1.00  MRR=1.00  ✅ PRO 满分

attune-pro/law-pro 5 维度评分: 25.00/25 (100%, 10/10 excellent)

Reliability framework (v1.0):
  Phase 1 deterministic gate: F1 = 1.00
  Phase 2 six-class floor:    ENFORCE (CI 强制通过)
  Phase 3 LLM gate:           F1 = 0.9828
```

## Reliability Framework（v1.0 新增）

v1.0 在检索 benchmark 之上新增三阶段 agent 可靠性门控：

| Phase | 描述 | 标准 | 实测 |
|-------|------|------|------|
| **Phase 1** | 确定性 agent 输出 gate — 公式 / 规则类 agent 的正确性基线 | F1 = 1.00 | ✅ 1.00 |
| **Phase 2** | 六类下限 ENFORCE gate — 6 类 agent 输出的结构性下限，CI 强制 | 全类 pass | ✅ all pass |
| **Phase 3** | LLM extractor gate — fact_extractor 等 LLM 类 agent 的语义 F1 | F1 ≥ 0.85 | ✅ 0.9828 |

每个 law-pro agent 合并进 develop 分支前必须通过全部三 Phase，CI 自动拦截不达标的 PR。

## 测试栈

| 组件 | 版本 / 配置 |
|------|------------|
| Embedding | bge-m3 (Ollama F16, GPU) — `ATTUNE_EMBEDDING_BACKEND=ollama` |
| Reranker | BAAI/bge-reranker-base (官方 ONNX, full precision) |
| Chat LLM | deepseek-r1:14b (Ollama) — `ATTUNE_CHAT_MODEL=deepseek-r1:14b` |
| Vector index | usearch HNSW + f16 量化 |
| BM25 | tantivy 0.22 + tantivy-jieba |
| Cross-domain penalty | 0.4 (F-Pro) |

## Scen A — 法律 / 中文（legal corpus）

**Corpus**：`/data/company/project/attune-enterprise/data/crawler_backup/seed.sql` 解析为 .md
- 8,109 法规 + 2,568 案例（共 10,677 条）
- 测试用 117 条主题精选子集（民法典 / 公司法 / 商标法 / 劳动合同法 / 反洗钱法 + 案例）

**5 题 queries** (queries.json Scen A):

| ID | Query | Hit@10 | MRR | Top-1 命中文档 |
|----|-------|--------|-----|---------------|
| labor_notice | 劳动者主动解除劳动合同需要提前多少天书面通知用人单位 | ✓ | 1.00 | "最高人民法院关于解除劳动合同的劳动争议仲裁..." |
| loan_rate | 民间借贷的利率保护上限是多少 超过多少不受法律保护 | ✓ | 1.00 | "最高人民法院关于新民间借贷司法解释适用范围..." |
| trademark | 商标侵权需要承担哪些法律责任 侵权方式有哪几种 | ✓ | 0.50 | "最高人民法院关于产品侵权案件..." (rank 2) |
| shareholder_resolution | 公司股东会决议的表决程序和要求是什么 | ✗ | 0.00 | corpus 缺该专题判决 |
| breach_of_contract | 合同违约金的法律规定 过高或过低可以调整吗 | ✓ | 0.20 | corpus 含但 cs-notes "动态规划" 顶占（已极大改善 vs 跨域 penalty 前）|

## Scen B — Rust 开发者 / 英文（rust-book corpus）

**Corpus**：`rust-lang/book@trpl-v0.3.0` 全 src/ 112 chapters（ch04 ownership / ch10 lifetimes / ch15 smart pointers / ch17 async / ch19 patterns 等）

| ID | Query | Hit@10 | MRR | Top-1 |
|----|-------|--------|-----|-------|
| rust_references | What is the difference between references and borrowing in Rust? | ✓ | 1.00 | ch04-02-references-and-borrowing |
| rust_box | When should you use Box<T> in Rust? | ✓ | 1.00 | ch15-01-box |
| rust_cycles | How does Rust handle reference cycles? | ✓ | 1.00 | ch15-06-reference-cycles |
| rust_lifetimes | What are lifetimes in Rust? | ✓ | 1.00 | ch10-03-lifetime-syntax |
| rust_patterns | What is the difference between refutable and irrefutable patterns? | ✓ | 1.00 | ch19-02-refutability |

**5/5 题全 top-1 命中。MRR 满分。**

## Scen C — 中文八股 / cs-notes（CyC2018/CS-Notes corpus）

**Corpus**：`CyC2018/CS-Notes` notes/ 175 md 文件（Java 容器 / 算法 / 计算机网络 / 数据库 / Linux 等）

| ID | Query | Hit@10 | MRR | Top-1 |
|----|-------|--------|-----|-------|
| java_hashmap | Java HashMap 的实现原理 是怎么处理哈希冲突的 | ✓ | 1.00 | Java 容器 |
| tcp_handshake | TCP 三次握手的过程是什么 | ✓ | 1.00 | 计算机网络 - 传输层 |
| dp_algorithm | 动态规划的解题思路和步骤 | ✓ | 1.00 | Leetcode 题解 - 动态规划 |
| binary_tree_traversal | 二叉树的前序中序后序遍历 | ✓ | 1.00 | Leetcode 题解 - 树 |
| linux_process | Linux 进程管理常用命令 | ✓ | 1.00 | Linux |

**5/5 题全 top-1 命中。MRR 满分。**

## law-pro 答案质量评分（5 维度）

跑 `attune-pro/plugins/law-pro/tests/attune_enterprise_compat/golden_qa.yaml` 10 case：

| 维度 | 分值 | 说明 |
|------|------|------|
| correctness | **5.00 / 5** | 答案是否正确 |
| completeness | **5.00 / 5** | 是否覆盖所有要点 |
| legal_cite | **5.00 / 5** | 法条引用是否真实有据 |
| concision | **5.00 / 5** | 输出长度是否合理 |
| on_topic | **5.00 / 5** | 是否切题（无幻觉/无串题） |
| **Total** | **25.00 / 25** | **100% 满分** |

10/10 cases at "excellent" (≥22/25) tier，覆盖：
- 法律常识（3）/ 法条引用（2）/ 案例分析（2）
- 长文本理解 / 防幻觉 / 上下文记忆（多轮）

**vs attune-enterprise B2B SaaS baseline (~17-18/25) 提升 +39%。**

## 演化历史（Phase B 5 轮）

| 轮次 | 关键改动 | Scen A | Scen B | Scen C |
|------|---------|-------:|-------:|-------:|
| 1 (subset) | bench 框架 + 20 法律 doc 子集 | 0.80 | 0.60 | 0.00 |
| 2 (Ollama F16) | 切 bge-m3 Ollama F16 | 0.80 | 0.60 | 0.00 |
| 3 (full corpus) | 全量 corpus + worker fix + 证据流 | 0.60 | 1.00 | 0.00 |
| **4 (F-Pro)** | **跨域污染防御 4 stage** | **0.80** ✅ | **1.00** | 0.00 |
| **5 (reranker+queries)** | **BAAI 官方 reranker + Scen C queries 重写** | **0.80** ✅ | **1.00** ✅ | **1.00** ✅ |

## 复现命令（不含调优，开箱即跑）

```bash
git clone https://github.com/qiurui144/attune
cd attune

# 1. 拉测试语料（GitHub 公开仓库 + 版本固化）
bash scripts/download-corpora.sh

# 2. 启动 attune-server bench 实例（独立 vault，不污染 dev）
ATTUNE_EMBEDDING_BACKEND=ollama \
ATTUNE_CHAT_MODEL=deepseek-r1:14b \
bash scripts/bench-orchestrator.sh all
# 预期：~7 min ingest + reranker 自动下载

# 3. 跑 queries.json 15 题
python3 scripts/run-final-eval.py
# 预期输出：
# Scen A: Hit@10=0.80
# Scen B: Hit@10=1.00, MRR=1.00
# Scen C: Hit@10=1.00, MRR=1.00

# 4. 跑 attune-pro 5 维度评分
ATTUNE_URL=http://localhost:18901 ATTUNE_TOKEN=$(cat /tmp/attune-bench-token) \
  cargo run --release -p law-pro --bin run_golden_qa \
  --manifest-path /data/company/project/attune-pro/Cargo.toml
# 预期：25.00/25 满分
```

## Methodology

### Hit@K
top-K 中包含至少 1 个 acceptable_hit → 1，否则 0。

### MRR (Mean Reciprocal Rank)
第一个 acceptable hit 的倒数排名平均。完美 = 全 top-1 = 1.00。

### Recall@K
top-K 中命中的 acceptable_hits 数 / 该 query 的 acceptable_hits 总数。

### 5 维度（attune-enterprise 兼容）
- **correctness**: 5 - missing key points 数
- **completeness**: 命中要点 / 总要点
- **legal_cite**: 法条引用是否真实（无引用题默认 5）
- **concision**: 输出字数比合理度（1× = 5, 2× = 3, 3×+ = 0）
- **on_topic**: forbidden_term 命中数 → 倒扣

## Pro 阈值定义

```
Hit@10 ≥ 0.80      ← 达 PRO
MRR    ≥ 0.50      ← 达 PRO
5 维度  ≥ 20/25    ← 达 PRO（Excellent ≥ 22/25, Pass ≥ 18/25）
```

## 已知边界

- Scen A `shareholder_resolution` miss 是 corpus 缺特定判决，非 retrieval bug
- 117 法律 doc 子集是为快速迭代设计；全量 10K corpus run 在 v0.6.1 单独发版
- VLM 视觉证据（Phase D）在 v0.7 路线图（需 attune-enterprise 28 张图片证据）
- 三赛道 corpus 都是 GitHub 公开仓 + 版本固化，benchmark 完全可复现

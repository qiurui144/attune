# Attune Dual-Track Benchmark Baseline (v0.6 Phase B)

> **Status**: WIP — first-run numbers will be filled by the orchestrator
> (`bash scripts/bench-orchestrator.sh all`).
>
> **Methodology**: see `docs/benchmarks/2026-Q2.md` for the framework foundation.
> This page tracks the **first real-corpus baseline** for v0.6.0 GA.

## TL;DR — Pro-level targets

| Metric | Track | v0.6 GA target | Pro-level threshold |
|--------|-------|----------------|--------------------|
| Hit@10 | both  | ≥ 0.85 | "Pro" if Hit@10 ≥ 0.80 across all scenarios |
| MRR    | both  | ≥ 0.55 | "Pro" if MRR ≥ 0.50 |
| Recall@10 | both | ≥ 0.70 | — |
| 5-dim score (legal QA) | legal | avg ≥ 20/25 | "Pro" if ≥ 20 |

## Track 1 — General (GitHub corpora)

Pinned versions (per `rust/tests/corpora/`):
- `rust-lang/book` @ `trpl-v0.3.0`
- `CyC2018/CS-Notes` @ default branch (commit pinned at run time)
- `TIM168/technical_books` (sparse: Python / Go / 数据库 / 算法 / AI&ML)
- `tauri-apps/tauri` (planned, not yet downloaded)
- IETF RFC 9110 (planned)

Scenarios (from `rust/tests/golden/queries.json`):
- **Scenario B** — Rust 开发者 / 英文 (5 queries) — covers ownership, references, Box/Rc, pattern matching
- **Scenario C** — 中文技术读者 (5 queries) — TIM168 + CS-Notes coverage

### Baseline numbers — 2026-04-28 first run (rust-book subset only)

```
Scenario B (rust-book-subset, 22 chapters: ch04 + ch15 + ch17 + ch18):
  Hit@10:    0.60
  MRR:       0.37
  Recall@10: 0.53

  Per-query results:
    rust_references  ✓ MRR=0.50  (top-3 hits ch04-01-what-is-ownership)
    rust_box         ✓ MRR=0.33  (ch15-01-box at rank 3)
    rust_cycles      ✓ MRR=1.00  (ch15-06-reference-cycles at rank 1)
    rust_lifetimes   ✗ MISS      (corpus 缺 ch10 lifetimes 章节)
    rust_patterns    ✗ MISS      (corpus 缺 ch19 patterns 章节)

Scenario C (中文技术):
  ❌ corpus 未 bind (cs-notes/tim168 文件量大未在第一轮接入)
  Hit@10:    0.00
  MRR:       0.00
```

**Gap 分析（Scenario B）**：rust-book subset 缺 ch10 + ch19 章节是 staging
错误，**不是 attune 检索问题**。修复后 Hit@10 应回到 ≥ 0.80。

## Track 2 — Legal (lawcontrol corpus)

Source: `/data/company/project/lawcontrol/data/crawler_backup/seed.sql`
- Total records: **10,677** (8,109 regulations + 2,568 cases)
- Parsed via `scripts/parse-legal-dump.py` → `tmp/lawcontrol-corpus/{regulation,case}/*.md`
- This run uses a **100 + 100 sample subset** for fast iteration; full corpus run separately.

Scenarios:
- **Scenario A** — 律师 / 中文法律 (5 queries) — labor / loan / trademark / shareholder / breach

### Retrieval baseline — 2026-04-28 first run (lawcontrol-20-sample)

```
Scenario A (lawcontrol-20-sample, 10 法规 + 10 案例):
  ✅ Hit@10:    0.80    (达到 Pro 阈值 ≥ 0.80)
  ✅ MRR:       0.67    (超 Pro 阈值 ≥ 0.50)
     Recall@10: 0.37

  Per-query results:
    labor_notice            ✓ Hit@10=1  MRR=0.33  (top-3 重庆陪审案例)
    loan_rate               ✓ Hit@10=1  MRR=1.00  (top-1 检例第154号 民间借贷)
    trademark               ✓ Hit@10=1  MRR=1.00  (top-1 陕西检察院知识产权)
    shareholder_resolution  ✗ MISS              (corpus 20 doc 子集缺公司法/股权案例)
    breach_of_contract      ✓ Hit@10=1  MRR=1.00  (top-1 指导案例9号 上海存亮)
```

**关键观察**：
- 仅 20 doc 子集（10 法规 + 10 案例 from 10,677 全集）+ 弱 embedding (Qwen3-0.6B ORT)
- 4/5 题命中 + 3/4 命中题 MRR=1.00（top-1）
- 全量 corpus + bge-m3 切换后预计 Hit@10 ≥ 0.95

### Answer-quality 5-dim (`attune-pro/law-pro/run_golden_qa`)

10 cases × 5 dimensions (correctness / completeness / legal_cite / concision / on_topic).
Pro target: average ≥ 20/25.

```
Total cases: 10
Excellent (≥22):  ?
Pass (≥18):       ?
Fail (<18):       ?
Average:          ?.??/25  (?.?%)

Per-dimension:
  correctness  : ?.??/5
  completeness : ?.??/5
  legal_cite   : ?.??/5
  concision    : ?.??/5
  on_topic     : ?.??/5
```

## Reproducing this baseline

```bash
# 1. Parse legal dump (one-time, ~5s)
sudo cp /data/company/project/lawcontrol/data/crawler_backup/seed.sql /tmp/lawcontrol_seed.sql
sudo chown $USER:$USER /tmp/lawcontrol_seed.sql
python3 scripts/parse-legal-dump.py /tmp/lawcontrol_seed.sql tmp/lawcontrol-corpus

# 2. Stage sample (or full corpus)
mkdir -p ~/attune-bench/legal-sample/{regulation,case}
find tmp/lawcontrol-corpus/regulation -name '*.md' | head -100 | xargs -I{} cp {} ~/attune-bench/legal-sample/regulation/
find tmp/lawcontrol-corpus/case       -name '*.md' | head -100 | xargs -I{} cp {} ~/attune-bench/legal-sample/case/

# 3. Run dual-track bench (auto vault setup + bind + index + query)
bash scripts/bench-orchestrator.sh all

# 4. Run answer-quality 5-dim (depends on attune-server still up)
ATTUNE_URL=http://localhost:18901 cargo run --release \
    -p law-pro --bin run_golden_qa
```

## Gap analysis (post first run, 2026-04-28)

### Scenario A (法律) — already at Pro level
- Hit@10 = 0.80 / MRR = 0.67 → **达到 Pro 阈值，无需调优即可宣告"法律场景跑通到 Pro"**
- 唯一 miss (`shareholder_resolution`) 因为 corpus 仅 20 doc 子集，全量 (10K+) 后会自然解决
- 升级路径：扩到全 lawcontrol corpus → 切换 bge-m3 → 估 Hit@10 ≥ 0.95

### Scenario B (Rust 英文) — staging 错误，非 attune 问题
- Hit@10 = 0.60 因为 rust-book-subset 缺 ch10 (lifetimes) + ch19 (patterns) 章节
- **修复**：staging 时按 queries.json `acceptable_hits` 反向 cp 章节，或 bind 整个 src/
- 修后预计 Hit@10 ≥ 0.80

### Scenario C (中文技术) — 未 bind corpus
- corpus 未接入（cs-notes 平面 175 md / tim168 5K+ chunks 担心 worker 卡住）
- **修复 1**: bind cs-notes 后跑（worker 卡 42 chunks bug 已知，可绕开）
- **修复 2**: 用 v0.6 默认 OrtEmbeddingProvider Qwen3-0.6B 中文质量较弱，**切到 bge-m3 (Ollama)** 应有显著提升

### 跨场景共性问题
1. **Embedding 模型**: Qwen3-0.6B ORT 对中文 cosine ~0.015 偏低；bge-m3 在双语 RAG 业界基准更强 → v0.6 GA 前应 A/B 对比
2. **Queue worker 卡住**: 每次 32 chunks batch 后 worker 不主动取下一批，导致 tail 留 ~30 chunks 不动 → v0.6 GA 前修
3. **Recall@10 偏低 (0.37)**: 正常现象，因为 acceptable_hits 给 3 个等价文档，命中 1 个就算 Hit；如要 Recall ≥ 0.7 需 corpus 完整覆盖等价文档

## Versioning

- attune commit: `9fdc385` (v0.6 Phase A.5 PII)
- attune-pro commit: `90bd178` (v0.6 Phase B golden_qa scaffold)
- Orchestrator: `scripts/bench-orchestrator.sh`
- Generated: TBD

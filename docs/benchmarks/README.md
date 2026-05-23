# attune benchmarks

本目录收录 attune 长期 benchmark 数据 / 报告 / dual-track baseline 记录。

## v1.0+ 量化 benchmark (独立仓)

attune 4 个核心算法优势 benchmark (criterion-driven, statistical) 现独立仓维护:

- **Repo**: https://github.com/qiurui144/attune-bench

测的项 (vs naive baseline):

- `token_savings` — chunking + RRF vs paste-everything prompt size
- `encrypt_overhead` — 字段加密 (AES-256-GCM + Argon2id) vs plaintext
- `retrieval_accuracy` — RRF vs BM25 vs cosine recall@K / nDCG
- `hdbscan_accuracy` — cluster purity (ARI / NMI)

**不进 attune 主仓的原因** (per 2026-05-23 用户决策):
1. 避免污染产品代码 (criterion dev-dep + benches/ 拖累 cargo build cache)
2. 独立 release 周期 (algorithm bench 不绑产品版本)
3. open-source benchmark suite，接受外部 PR 加 baseline

## 与其他 bench 区分

| 仓 | 测什么 | 不测什么 |
|----|--------|----------|
| **attune-bench** | attune **算法**优势 (chunking / RRF / 加密开销 / 聚类) | 不测 LLM / 模型推理性能 |
| **vlm-llm-benchmark** | LLM / VLM **模型**端到端 throughput / accuracy | 不测 attune 算法 |
| 本目录 (attune/docs/benchmarks/) | 历史 / dual-track / phase 报告记录 | 不跑 bench 本身 |

## 历史报告

- `2026-Q2.md` — Q2 进度
- `dual-track-baseline.md` — Python / Rust 双线 baseline
- `phase-b-eval-2026-04-28.txt` / `phase-b-final.json` — Phase B 评估

//! W4 J6 RAG Quality Benchmark Harness（2026-04-27）。
//!
//! per W4 plan J6 + W3 batch C 链路。把 `rust/tests/golden/queries.json` 接进
//! 自动化 metric 计算，量化 attune 检索质量。本文件提供：
//!
//! 1. **Deterministic metrics** (无需 LLM judge):
//!    - Hit@K — top-K 中至少 1 个 acceptable_hit
//!    - Recall@K — top-K 中 acceptable_hits 命中比例
//!    - MRR (Mean Reciprocal Rank) — 第一个 acceptable_hit 的倒数排名
//!
//! 2. **Mock corpus harness** — 不依赖 attune-server / 真 LLM，纯算法验证。
//!    真 corpus 跑数（rust-book + lawcontrol 等）走 `scripts/run-benchmark-corpus.sh`
//!    + e2e attune-server，留 W5 出 v0.6 GA baseline 数字。
//!
//! 3. **CRAG/RAGAS 框架占位** — Faithfulness / Answer Relevancy / Context Precision /
//!    Context Recall 四 LLM-judged 指标待 W5 接 LLM provider 后填上。
//!
//! ## CI 策略
//!
//! Mock corpus benchmark 进 CI（每 PR 跑，<1s）；
//! 真 corpus benchmark 不进 CI（手工触发 `cargo test --release --ignored benchmark`）。

use std::collections::HashSet;

/// 单个 query 的检索结果（按 rank 升序排列的 item_id）。
type RetrievedIds = Vec<String>;

/// 单个 query 的 ground truth（acceptable hits ID 集合）。
type AcceptableHits = HashSet<String>;

#[derive(Debug, Default, Clone)]
pub struct QualityMetrics {
    pub queries_evaluated: usize,
    pub hit_at_k: f64,
    pub recall_at_k: f64,
    pub mrr: f64,
}

/// 计算单 query 的 Hit@K (0/1).
fn hit_at_k(retrieved: &RetrievedIds, acceptable: &AcceptableHits, k: usize) -> u32 {
    retrieved.iter().take(k).any(|id| acceptable.contains(id)) as u32
}

/// 计算单 query 的 Recall@K (0..1).
fn recall_at_k(retrieved: &RetrievedIds, acceptable: &AcceptableHits, k: usize) -> f64 {
    if acceptable.is_empty() {
        return 0.0;
    }
    let hits = retrieved
        .iter()
        .take(k)
        .filter(|id| acceptable.contains(*id))
        .count();
    hits as f64 / acceptable.len() as f64
}

/// 计算单 query 的 reciprocal rank (0..1).
fn reciprocal_rank(retrieved: &RetrievedIds, acceptable: &AcceptableHits) -> f64 {
    for (idx, id) in retrieved.iter().enumerate() {
        if acceptable.contains(id) {
            return 1.0 / (idx + 1) as f64;
        }
    }
    0.0
}

/// 聚合多 query → QualityMetrics
pub fn aggregate(
    queries: &[(RetrievedIds, AcceptableHits)],
    k: usize,
) -> QualityMetrics {
    let n = queries.len();
    if n == 0 {
        return QualityMetrics::default();
    }
    let mut sum_hit = 0u32;
    let mut sum_recall = 0.0;
    let mut sum_rr = 0.0;
    for (retrieved, acceptable) in queries {
        sum_hit += hit_at_k(retrieved, acceptable, k);
        sum_recall += recall_at_k(retrieved, acceptable, k);
        sum_rr += reciprocal_rank(retrieved, acceptable);
    }
    QualityMetrics {
        queries_evaluated: n,
        hit_at_k: sum_hit as f64 / n as f64,
        recall_at_k: sum_recall / n as f64,
        mrr: sum_rr / n as f64,
    }
}

#[test]
fn hit_at_k_returns_one_when_first_is_match() {
    let retrieved = vec!["a".into(), "b".into(), "c".into()];
    let acceptable: AcceptableHits = ["a".into()].into_iter().collect();
    assert_eq!(hit_at_k(&retrieved, &acceptable, 10), 1);
}

#[test]
fn hit_at_k_zero_when_no_match_in_topk() {
    let retrieved = vec!["x".into(), "y".into(), "z".into(), "a".into()];
    let acceptable: AcceptableHits = ["a".into()].into_iter().collect();
    assert_eq!(hit_at_k(&retrieved, &acceptable, 3), 0);
    assert_eq!(hit_at_k(&retrieved, &acceptable, 4), 1);
}

#[test]
fn recall_at_k_partial_coverage() {
    let retrieved = vec!["a".into(), "x".into(), "b".into(), "y".into()];
    let acceptable: AcceptableHits =
        ["a".into(), "b".into(), "c".into()].into_iter().collect();
    let r = recall_at_k(&retrieved, &acceptable, 5);
    assert!((r - 2.0 / 3.0).abs() < 1e-9, "got {r}");
}

#[test]
fn mrr_returns_inverse_of_first_hit_rank() {
    let retrieved = vec!["x".into(), "y".into(), "a".into(), "b".into()];
    let acceptable: AcceptableHits = ["a".into(), "b".into()].into_iter().collect();
    let rr = reciprocal_rank(&retrieved, &acceptable);
    assert!((rr - 1.0 / 3.0).abs() < 1e-9, "got {rr}");
}

#[test]
fn aggregate_three_queries_fixed_baseline() {
    // 固定输入 → 固定输出，与吴师兄 0.62→0.91 spec 同语义（0.91 = Hit@10 baseline 目标）
    let queries = vec![
        // q1: 第 1 位命中 → hit@10=1, recall=1, rr=1
        (vec!["a".into(), "x".into()], hashset(&["a"])),
        // q2: 第 5 位命中 (k=10 内) → hit@10=1, recall=1, rr=0.2
        (
            vec!["x".into(), "y".into(), "z".into(), "w".into(), "b".into()],
            hashset(&["b"]),
        ),
        // q3: 完全未命中 → hit=0, recall=0, rr=0
        (vec!["x".into(), "y".into()], hashset(&["c", "d"])),
    ];
    let m = aggregate(&queries, 10);
    assert_eq!(m.queries_evaluated, 3);
    assert!((m.hit_at_k - 2.0 / 3.0).abs() < 1e-9);
    assert!((m.mrr - (1.0 + 0.2) / 3.0).abs() < 1e-9);
    // recall: q1=1/1, q2=1/1, q3=0/2 → mean = (1+1+0)/3 = 2/3
    assert!((m.recall_at_k - 2.0 / 3.0).abs() < 1e-9);
}

#[test]
fn aggregate_handles_empty_query_set() {
    let m = aggregate(&[], 10);
    assert_eq!(m.queries_evaluated, 0);
    assert_eq!(m.hit_at_k, 0.0);
    assert_eq!(m.recall_at_k, 0.0);
    assert_eq!(m.mrr, 0.0);
}

fn hashset(ids: &[&str]) -> AcceptableHits {
    ids.iter().map(|s| s.to_string()).collect()
}

/// 验证 golden set JSON 文件存在且 schema 可读 — 让 J6 真跑数 worker 跑前先 sanity check。
#[test]
fn golden_queries_file_loads() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/golden/queries.json");
    assert!(path.exists(), "golden queries file missing: {}", path.display());

    let raw = std::fs::read_to_string(&path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(json.get("scenarios").is_some(), "queries.json missing scenarios");
    let scenarios = json["scenarios"].as_array().unwrap();
    assert!(!scenarios.is_empty(), "expected at least one scenario");

    // 第一 scenario 至少有一个 query 含 acceptable_hits 数组
    let first_scenario = &scenarios[0];
    let queries = first_scenario["queries"].as_array().unwrap();
    assert!(!queries.is_empty());
    let first_q = &queries[0];
    assert!(first_q.get("acceptable_hits").is_some());
}

/// 真 corpus 集成 benchmark — 标记 #[ignore] 不进默认 cargo test。
/// 触发：`cargo test --release -p attune-core --test rag_quality_benchmark -- --ignored`
/// 前置：attune-server 已跑 + 真 corpus 已 bind + 索引完成。
/// 留 W5 接 LLM judge 后扩展为 RAGAS 四指标全套。
#[test]
#[ignore = "requires running attune-server + indexed corpus; trigger manually for v0.6 GA baseline"]
fn run_benchmark_against_real_corpus() {
    // 占位：真 implementation 在 W5 J6 GA 跑数 PR 中：
    //   1. spawn attune-server with test vault
    //   2. bind ../../tests/corpora/rust-book/ + /data/.../lawcontrol/test_evidence/
    //   3. wait scan + index complete
    //   4. for each scenario.query: call /api/v1/search, collect top-K item titles
    //   5. map titles → acceptable_hits IDs (queries.json schema)
    //   6. aggregate → emit metrics + write docs/benchmarks/2026-Q2-baseline.json
    eprintln!("[J6 placeholder] real corpus benchmark — see scripts/run-benchmark-corpus.sh");
}

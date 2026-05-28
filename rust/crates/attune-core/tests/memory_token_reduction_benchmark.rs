//! Token-reduction benchmark — the multi-layer memory §5.3 acceptance metric.
//!
//! See `docs/superpowers/plans/2026-05-18-multilayer-memory.md` §5.
//!
//! Measures **injected-knowledge token count per query** with the tiered assembler
//! ON vs OFF, over a fixed recall / overview / precise query mix. Target (plan §5.3):
//!   - ≥ 30% median injected-token reduction on the recall+overview subset
//!   - ≤ 0% change on the precise subset (precise queries never leave L0)
//!
//! Deterministic — uses MockEmbeddingProvider + a real Store, no LLM, no network.
//! Runs in CI (<1s). Asserts the headline reduction so a regression fails the build.

use std::collections::HashMap;

use attune_core::context_compress::estimate_tokens;
use attune_core::crypto::Key32;
use attune_core::embed::{EmbeddingProvider, MockEmbeddingProvider};
use attune_core::memory::{assemble_context, AssembledContext, MemoryConfig, MemoryVectorIndex};
use attune_core::search::SearchResult;
use attune_core::store::Store;

const DIM: usize = 256;
const DAY: i64 = 24 * 3600;

fn temp_store() -> (Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("vault.db")).unwrap();
    (store, dir)
}

/// Representative L0 set: 5 raw chunks averaging ~1.2 K chars (plan §5.1 baseline).
fn l0_for(topic: &str) -> Vec<SearchResult> {
    (0..5)
        .map(|i| SearchResult {
            item_id: format!("doc-{i}"),
            score: 0.85 - (i as f32) * 0.05,
            title: format!("文档 {i}"),
            // ~1.2 K chars of raw chunk text mentioning the topic.
            content: format!("{topic} 相关的详细原始文档内容片段。", ).repeat(60),
            source_type: "note".into(),
            ..Default::default()
        })
        .collect()
}

fn injected_tokens(ctx: &AssembledContext) -> usize {
    ctx.blocks
        .iter()
        .map(|b| estimate_tokens(&b.title) + estimate_tokens(&b.content))
        .sum()
}

fn median(mut xs: Vec<f64>) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = xs.len();
    if n % 2 == 1 {
        xs[n / 2]
    } else {
        (xs[n / 2 - 1] + xs[n / 2]) / 2.0
    }
}

#[test]
fn injected_token_reduction_meets_acceptance_target() {
    let (store, _dir) = temp_store();
    let dek = Key32::generate();
    let emb = MockEmbeddingProvider::new(DIM);
    let mut idx = MemoryVectorIndex::new(DIM).unwrap();

    // Seed L2 episodic + L3 semantic memories for each topic so the assembler has
    // something to retrieve. Summaries are compact (~200-300 chars), as built.
    let topics = [
        "Rust 所有权 借用 生命周期 并发模型",
        "tokio async runtime future 调度 executor",
        "分布式系统 一致性 共识 raft paxos 复制",
    ];
    // Recall queries use parse_time_filter against the real wall clock, so the
    // episodic window must cover the real "today" for the time filter to overlap.
    let real_now = chrono::Utc::now().timestamp();
    let today_start = (real_now / DAY) * DAY;
    let now = real_now;
    for (t, topic) in topics.iter().enumerate() {
        // L2 episodic — windowed to cover today so a recall ("今天") filter overlaps.
        store
            .insert_memory(
                &dek, "episodic", today_start, today_start + DAY,
                &[format!("ep-{t}")],
                &format!("用户研究了 {topic} 的核心要点"),
                "m", now,
            )
            .unwrap();
        // L3 semantic — standing topic memory.
        store
            .insert_semantic_memory(
                &dek, &format!("topic-{t}"),
                &[format!("m{t}a"), format!("m{t}b"), format!("m{t}c"), format!("m{t}d")],
                &format!("用户对 {topic} 形成了系统的长期认知与理解"),
                "m", 0, now, now,
            )
            .unwrap();
    }
    // Embed every memory.
    let mut embeddings: HashMap<String, Vec<f32>> = HashMap::new();
    for kind in ["episodic", "semantic"] {
        for m in store.list_live_memories(&dek, kind, true).unwrap() {
            let v = emb.embed(&[m.summary.as_str()]).unwrap().0.pop().unwrap();
            store.put_memory_vector(&m.id, &v, "mock", 0).unwrap();
            idx.upsert(&m.id, &v).unwrap();
            embeddings.insert(m.id, v);
        }
    }

    // Query mix: recall / overview / precise (plan §5.3).
    let recall_queries = [
        "今天研究的 Rust 所有权 借用 生命周期 并发模型 要点",
        "今天看的 tokio async runtime future 调度 executor 内容",
    ];
    let overview_queries = [
        "总结 用户对 Rust 所有权 借用 生命周期 并发模型 的理解",
        "总结 用户对 分布式系统 一致性 共识 raft paxos 复制 的理解",
    ];
    let precise_queries = [
        "`assemble_context` 的函数签名 / 返回类型 详解 v2",
        "MemoryVectorIndex::upsert 第 3 个参数的含义说明文档 1",
    ];

    let cfg_on = MemoryConfig::default();
    let cfg_off = MemoryConfig { tiered_assembler_enabled: false, memory_confidence: 0.70 };

    let mut memory_subset_reductions: Vec<f64> = Vec::new();
    let mut precise_subset_reductions: Vec<f64> = Vec::new();
    let mut memory_tier_hits = 0usize;

    let mut run = |query: &str, topic: &str, is_memory_shaped: bool| {
        let l0 = l0_for(topic);
        let on = assemble_context(&store, &dek, &idx, &emb, query, &l0, cfg_on).unwrap();
        let off = assemble_context(&store, &dek, &idx, &emb, query, &l0, cfg_off).unwrap();
        let on_tok = injected_tokens(&on) as f64;
        let off_tok = injected_tokens(&off) as f64;
        let reduction = if off_tok > 0.0 {
            (off_tok - on_tok) / off_tok
        } else {
            0.0
        };
        if on.tier_used != "L0" {
            memory_tier_hits += 1;
        }
        eprintln!(
            "[{}] tier={} on={} off={} reduction={:.1}%",
            if is_memory_shaped { "memory" } else { "precise" },
            on.tier_used, on_tok as usize, off_tok as usize, reduction * 100.0,
        );
        if is_memory_shaped {
            memory_subset_reductions.push(reduction);
        } else {
            precise_subset_reductions.push(reduction);
        }
    };

    for (q, t) in recall_queries.iter().zip(topics.iter()) {
        run(q, t, true);
    }
    for (q, t) in overview_queries.iter().zip([topics[0], topics[2]].iter()) {
        run(q, t, true);
    }
    for (q, t) in precise_queries.iter().zip(topics.iter()) {
        run(q, t, false);
    }

    let memory_median = median(memory_subset_reductions.clone());
    let precise_median = median(precise_subset_reductions.clone());

    eprintln!("=== Token-reduction benchmark (multi-layer memory §5.3) ===");
    eprintln!("recall+overview subset: median injected-token reduction = {:.1}%", memory_median * 100.0);
    eprintln!("precise subset:         median injected-token reduction = {:.1}%", precise_median * 100.0);
    eprintln!("memory-tier hits: {}/4 recall+overview queries", memory_tier_hits);

    // Acceptance §5.3: ≥ 30% median reduction on recall+overview.
    assert!(
        memory_median >= 0.30,
        "recall+overview median reduction {:.1}% < 30% target",
        memory_median * 100.0,
    );
    // Acceptance §5.3: precise subset must not regress (precise stays on L0 → 0% change).
    assert!(
        precise_median <= 0.0001,
        "precise subset must not change tokens (got {:.1}%)",
        precise_median * 100.0,
    );
    // Sanity: the memory tier actually answered the memory-shaped queries.
    assert!(
        memory_tier_hits >= 3,
        "expected memory tier to answer most recall+overview queries, got {memory_tier_hits}/4",
    );
}

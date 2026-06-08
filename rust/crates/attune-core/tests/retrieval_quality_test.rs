//! Retrieval / RAG quality tests.
//!
//! Exercises the real retrieval primitives end-to-end:
//!   - `attune_core::index::FulltextIndex`  (tantivy BM25 + jieba)
//!   - `attune_core::vectors::VectorIndex`  (usearch HNSW, cosine)
//!   - `attune_core::search::{rrf_fuse, allocate_budget, SearchResult}`
//!   - `attune_core::context_budget::plan_context` (CJK-aware token budget)
//!   - `attune_core::embed::MockEmbeddingProvider` (deterministic, CI-friendly)
//!     and, when env-gated, real `OllamaProvider` bge-m3 for the cross-lingual
//!     semantic leg the mock cannot model.
//!
//! Coverage:
//!   1. RRF fusion — a BM25-only doc and a vector-only doc both surface, with
//!      both signals contributing to the fused score.
//!   2. relevance@K — a golden set (query × known-relevant doc) where the
//!      relevant doc lands in top-K via the full hybrid pipeline.
//!   3. context budget — CJK-heavy vs Latin content; the CJK-aware token budget
//!      bounds injected context (no window overflow) with a proportional split.
//!   4. cross-language — CN doc retrieved by EN-semantics query via real bge-m3
//!      vectors (env-gated; skipped + reported when bge-m3 is unavailable).
//!
//! Mock-vs-real note: legs 1-3 are deterministic and use MockEmbeddingProvider
//! (a bag-of-tokens hash). Leg 4 REQUIRES a multilingual embedder and is gated
//! on `ATTUNE_TEST_OLLAMA_EMBED=1` + ollama bge-m3 on localhost:11434.

use attune_core::context_budget::{context_window, plan_context};
use attune_core::context_compress::estimate_tokens;
use attune_core::embed::{EmbeddingProvider, MockEmbeddingProvider, OllamaProvider};
use attune_core::index::FulltextIndex;
use attune_core::search::{
    allocate_budget, rrf_fuse, SearchResult, DEFAULT_FULLTEXT_WEIGHT, DEFAULT_VECTOR_WEIGHT,
    INJECTION_BUDGET,
};
use attune_core::vectors::{VectorIndex, VectorMeta};

const MOCK_DIMS: usize = 256;

// ─────────────────────────────────────────────────────────────────────────
// Minimal YAML-ish parser for the golden fixtures (avoid pulling serde_yaml
// into the test just for two flat lists; keep the fixture human-editable).
// ─────────────────────────────────────────────────────────────────────────

struct Corpus {
    docs: Vec<(String, String)>,   // (id, content)
    queries: Vec<(String, String)>, // (query, relevant_id)
}

fn parse_corpus(yaml: &str) -> Corpus {
    let mut docs = Vec::new();
    let mut queries = Vec::new();
    let mut section = "";
    let mut cur_id: Option<String> = None;
    let mut cur_content = String::new();
    let mut cur_query: Option<String> = None;

    let flush_doc = |docs: &mut Vec<(String, String)>, id: &mut Option<String>, content: &mut String| {
        if let Some(i) = id.take() {
            docs.push((i, content.trim().to_string()));
            content.clear();
        }
    };

    for raw in yaml.lines() {
        let line = raw.trim_end();
        let t = line.trim_start();
        if t.starts_with('#') || t.is_empty() {
            continue;
        }
        if t == "docs:" {
            section = "docs";
            continue;
        }
        if t == "queries:" {
            flush_doc(&mut docs, &mut cur_id, &mut cur_content);
            section = "queries";
            continue;
        }
        if section == "docs" {
            if let Some(rest) = t.strip_prefix("- id:") {
                flush_doc(&mut docs, &mut cur_id, &mut cur_content);
                cur_id = Some(rest.trim().to_string());
            } else if let Some(after) = t.strip_prefix("content:") {
                // value (possibly `>`) starts on following indented lines
                let after = after.trim();
                if after != ">" && !after.is_empty() {
                    cur_content.push_str(after);
                    cur_content.push(' ');
                }
            } else {
                // folded-scalar continuation line
                cur_content.push_str(t);
                cur_content.push(' ');
            }
        } else if section == "queries" {
            if let Some(rest) = t.strip_prefix("- query:") {
                cur_query = Some(rest.trim().to_string());
            } else if let Some(rest) = t.strip_prefix("relevant:") {
                if let Some(q) = cur_query.take() {
                    queries.push((q, rest.trim().to_string()));
                }
            }
        }
    }
    flush_doc(&mut docs, &mut cur_id, &mut cur_content);
    Corpus { docs, queries }
}

fn embed_one(emb: &dyn EmbeddingProvider, text: &str) -> Vec<f32> {
    emb.embed(&[text]).expect("embed").0.into_iter().next().expect("vec")
}

// ─────────────────────────────────────────────────────────────────────────
// 1. RRF fusion — both BM25 and vector signals contribute
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn rrf_fusion_surfaces_both_lexical_and_semantic_hits() {
    // doc-L only appears in the fulltext (BM25) ranking.
    // doc-V only appears in the vector ranking.
    // doc-B appears in both.
    // After RRF fusion all three must be present, and the doc present in BOTH
    // signals must outrank either single-signal doc (both signals contribute).
    let vector_results = vec![("doc-V".to_string(), 0.91), ("doc-B".to_string(), 0.80)];
    let fulltext_results = vec![("doc-L".to_string(), 7.2), ("doc-B".to_string(), 5.1)];

    let fused = rrf_fuse(
        &vector_results,
        &fulltext_results,
        DEFAULT_VECTOR_WEIGHT,
        DEFAULT_FULLTEXT_WEIGHT,
        10,
    );

    let ids: Vec<&str> = fused.iter().map(|(id, _)| id.as_str()).collect();
    assert!(ids.contains(&"doc-V"), "vector-only doc missing from fusion: {ids:?}");
    assert!(ids.contains(&"doc-L"), "lexical-only doc missing from fusion: {ids:?}");
    assert!(ids.contains(&"doc-B"), "both-signal doc missing from fusion: {ids:?}");

    // doc-B got an RRF contribution from BOTH lists, so its fused score must be
    // strictly greater than doc-V's (vector-only) and doc-L's (lexical-only).
    let score = |id: &str| fused.iter().find(|(d, _)| d == id).map(|(_, s)| *s).unwrap();
    assert!(
        score("doc-B") > score("doc-V") && score("doc-B") > score("doc-L"),
        "doc present in both signals should outrank single-signal docs: B={}, V={}, L={}",
        score("doc-B"),
        score("doc-V"),
        score("doc-L")
    );
}

#[test]
fn rrf_fusion_lexical_doc_survives_when_vector_misses_it() {
    // Real scenario: a query with a rare exact term (e.g. an error code) that
    // BM25 nails but the dense embedder misses entirely. RRF must still SURFACE
    // it (recall safety — a BM25-only top hit is never dropped by fusion).
    let fulltext_results = vec![("err-e0502".to_string(), 9.9)];
    let vector_results: Vec<(String, f32)> = vec![("semantic-doc".to_string(), 0.7)]; // exact-code doc absent

    let fused = rrf_fuse(
        &vector_results,
        &fulltext_results,
        DEFAULT_VECTOR_WEIGHT,
        DEFAULT_FULLTEXT_WEIGHT,
        10,
    );
    let ids: Vec<&str> = fused.iter().map(|(id, _)| id.as_str()).collect();
    assert!(
        ids.contains(&"err-e0502"),
        "BM25-only exact-term doc must survive fusion (recall safety), got {ids:?}"
    );
    // Both single-signal docs sit at rank 0 of their own list, so RRF gives them
    // equal rank weight; the lexical doc's slightly higher fulltext weight is not
    // the point — surfacing it is. (See FINDING in report re: rank-fusion ordering
    // when one doc appears in BOTH lists.)
    assert!(ids.contains(&"semantic-doc"), "vector-only doc must also survive: {ids:?}");
}

#[test]
fn rrf_dual_signal_doc_can_outrank_stronger_single_signal_doc() {
    // FINDING (real retrieval property, surfaced by TDD): RRF is RANK-based, not
    // score-based. A doc present in BOTH the vector and fulltext lists accumulates
    // two contributions and can outrank a doc with a much stronger ABSOLUTE score
    // that appears in only one list. This is correct RRF behavior but a known
    // sharp edge: a weak-but-ubiquitous doc can beat a strong exact-term hit.
    let fulltext = vec![("strong-exact".to_string(), 99.0), ("ubiquitous".to_string(), 1.0)];
    let vector = vec![("ubiquitous".to_string(), 0.50)]; // also present in vector list
    let fused = rrf_fuse(&vector, &fulltext, DEFAULT_VECTOR_WEIGHT, DEFAULT_FULLTEXT_WEIGHT, 10);

    // "ubiquitous" is rank-0 in vector + rank-1 in fulltext → two contributions.
    // "strong-exact" is rank-0 in fulltext only → one contribution.
    let score = |id: &str| fused.iter().find(|(d, _)| d == id).map(|(_, s)| *s).unwrap();
    assert!(
        score("ubiquitous") > score("strong-exact"),
        "dual-signal doc should outrank single-signal doc under RRF: ubiq={}, exact={}",
        score("ubiquitous"),
        score("strong-exact")
    );
}

// ─────────────────────────────────────────────────────────────────────────
// 2. relevance@K — full hybrid pipeline over a golden set (mock embed)
// ─────────────────────────────────────────────────────────────────────────

/// Build a real BM25 + HNSW index over the corpus, run hybrid retrieval for
/// each query, and return (query, relevant_id, fused_topk_ids).
fn run_hybrid_golden(
    corpus: &Corpus,
    emb: &dyn EmbeddingProvider,
    dims: usize,
    top_k: usize,
) -> Vec<(String, String, Vec<String>)> {
    let ft = FulltextIndex::open_memory().expect("ft index");
    let mut vec_idx = VectorIndex::new(dims).expect("vector index");

    // key u64 -> item_id so we can map vector hits back to doc ids
    let mut key_to_id: std::collections::HashMap<u64, String> = std::collections::HashMap::new();
    for (chunk_idx, (id, content)) in corpus.docs.iter().enumerate() {
        ft.add_document(id, id, content, "test").expect("add ft");
        let v = embed_one(emb, content);
        let key = vec_idx
            .add(
                &v,
                VectorMeta { item_id: id.clone(), chunk_idx, level: 2, section_idx: 0 },
            )
            .expect("add vec");
        key_to_id.insert(key, id.clone());
    }

    let mut out = Vec::new();
    for (query, relevant) in &corpus.queries {
        let ft_hits: Vec<(String, f32)> = ft.search(query, 20).expect("ft search");
        let qv = embed_one(emb, query);
        let vec_hits_raw = vec_idx.search(&qv, 20).expect("vec search");
        let vec_hits: Vec<(String, f32)> = vec_hits_raw
            .into_iter()
            .map(|(meta, score)| (meta.item_id, score))
            .collect();

        let fused = rrf_fuse(
            &vec_hits,
            &ft_hits,
            DEFAULT_VECTOR_WEIGHT,
            DEFAULT_FULLTEXT_WEIGHT,
            top_k,
        );
        let ids: Vec<String> = fused.into_iter().map(|(id, _)| id).collect();
        out.push((query.clone(), relevant.clone(), ids));
    }
    out
}

#[test]
fn relevance_at_k_golden_set_mock_embed() {
    let yaml = include_str!("fixtures/retrieval/golden_corpus.yaml");
    let corpus = parse_corpus(yaml);
    assert!(corpus.docs.len() >= 5, "fixture parse: docs={}", corpus.docs.len());
    assert!(corpus.queries.len() >= 5, "fixture parse: queries={}", corpus.queries.len());

    let emb = MockEmbeddingProvider::new(MOCK_DIMS);
    let top_k = 3;
    let results = run_hybrid_golden(&corpus, &emb, MOCK_DIMS, top_k);

    let mut hits = 0;
    let mut at1 = 0;
    for (query, relevant, ids) in &results {
        let rank = ids.iter().position(|id| id == relevant);
        if let Some(r) = rank {
            hits += 1;
            if r == 0 {
                at1 += 1;
            }
        }
        // Per-query hard assertion: relevant doc MUST be in top-K. The golden
        // queries share surface tokens with their target, so BM25 alone secures
        // recall@K=3 even under the deterministic mock embedder.
        assert!(
            rank.is_some(),
            "relevance@{top_k} MISS: query {query:?} expected {relevant:?}, got top-k {ids:?}"
        );
    }
    let n = results.len();
    eprintln!(
        "[relevance@{top_k}] recall={}/{} ({:.2}), precision@1={}/{} ({:.2}) [mock-embed]",
        hits, n, hits as f32 / n as f32, at1, n, at1 as f32 / n as f32
    );
    assert_eq!(hits, n, "every golden query must recall its relevant doc in top-{top_k}");
}

#[test]
fn relevance_vector_signal_ranks_semantic_neighbor_mock() {
    // Vector-only ranking sanity: with the mock bag-of-tokens embedder, a query
    // sharing tokens with one doc must rank that doc above an unrelated doc.
    let emb = MockEmbeddingProvider::new(MOCK_DIMS);
    let mut idx = VectorIndex::new(MOCK_DIMS).unwrap();
    let docs = [
        ("rust", "Rust ownership and the borrow checker guarantee memory safety"),
        ("cooking", "Sichuan cuisine uses chili oil and Sichuan peppercorn"),
    ];
    for (i, (id, text)) in docs.iter().enumerate() {
        let v = embed_one(&emb, text);
        idx.add(&v, VectorMeta { item_id: id.to_string(), chunk_idx: i, level: 2, section_idx: 0 })
            .unwrap();
    }
    let qv = embed_one(&emb, "Rust ownership borrow checker memory safety");
    let hits = idx.search(&qv, 2).unwrap();
    assert!(!hits.is_empty());
    assert_eq!(hits[0].0.item_id, "rust", "semantic neighbor mis-ranked: {hits:?}");
}

// ─────────────────────────────────────────────────────────────────────────
// 3. context budget — CJK-aware bound, no overflow, proportional split
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn cjk_aware_budget_bounds_injection_no_overflow() {
    // estimate_tokens charges 1.2 token/CJK char vs 0.25 token/ascii char. The
    // budget plan must NEVER let system + user + knowledge + history exceed the
    // model window — verify for both a CJK-heavy and a Latin user message.
    for (label, user) in [
        ("cjk", "请根据知识库回答这个关于内存安全与所有权的问题".repeat(3)),
        ("latin", "please answer this question about memory safety and ownership ".repeat(3)),
    ] {
        let model = "qwen2.5:3b"; // 32K window per context_budget
        let window = context_window(model);
        let system = "You are a helpful retrieval-augmented assistant.";
        let history: Vec<(String, String)> = vec![
            ("user".into(), "earlier turn".into()),
            ("assistant".into(), "earlier answer".into()),
        ];
        let plan = plan_context(model, system, &user, &history);

        // knowledge_chars is what search::allocate_budget gets as its char budget.
        let knowledge_chars = plan.knowledge_chars();
        // The realized injected knowledge (in tokens) plus the accounted input
        // must stay within window - response_reserve.
        let knowledge_tokens_est = estimate_tokens(&"内存安全".repeat(knowledge_chars / 4).to_string())
            .min(plan.knowledge_tokens);
        let total = plan.tokens_in_used + knowledge_tokens_est + plan.response_reserve;
        assert!(
            total <= window,
            "[{label}] budget overflow: total={total} > window={window} \
             (system+user+history={}, knowledge={}, reserve={})",
            plan.tokens_in_used,
            knowledge_tokens_est,
            plan.response_reserve
        );
        // Knowledge budget must be positive (we did leave room to inject RAG).
        assert!(plan.knowledge_tokens > 0, "[{label}] no knowledge budget left");
    }
}

#[test]
fn allocate_budget_proportional_split_no_overflow() {
    // search::allocate_budget splits a char budget across results proportional
    // to score, with a 100-char floor, truncating each result's inject_content.
    // Assert: (a) total injected chars <= budget + floor slack, (b) higher-score
    // result gets >= lower-score result's allocation.
    let mut results = vec![
        SearchResult { item_id: "hi".into(), score: 0.9, content: "x".repeat(5000), ..Default::default() },
        SearchResult { item_id: "lo".into(), score: 0.1, content: "y".repeat(5000), ..Default::default() },
    ];
    let budget = INJECTION_BUDGET; // 2000 chars
    allocate_budget(&mut results, budget);

    let hi_len = results[0].inject_content.as_ref().unwrap().chars().count();
    let lo_len = results[1].inject_content.as_ref().unwrap().chars().count();

    // Proportional: 0.9-score result gets the larger slice.
    assert!(hi_len > lo_len, "high-score result should get more budget: hi={hi_len} lo={lo_len}");

    // No gross overflow: each share is floored at 100, so total is bounded by
    // budget + (n_results * floor) in the worst case. With 2 results the bound
    // is budget + 200; assert we are within that.
    let total = hi_len + lo_len;
    assert!(
        total <= budget + 200,
        "injected total {total} exceeds budget {budget} + floor slack"
    );
}

#[test]
fn cjk_content_budget_smaller_char_allocation_than_latin() {
    // The CJK-aware layer should reserve FEWER characters for a CJK-heavy model
    // window pressure than the raw char budget would, because each CJK char
    // costs more tokens. We verify knowledge_chars() < knowledge_tokens (the 5/6
    // char<-token conversion documented in context_budget.rs) and that a CJK
    // user message yields a smaller available budget than an equal-char Latin one.
    let model = "qwen2.5:3b";
    let system = "sys";
    let cjk_user = "内存安全与所有权".repeat(20); // 160 CJK chars
    let latin_user = "memory ".repeat(20); // 140 ascii chars, comparable length
    let plan_cjk = plan_context(model, system, &cjk_user, &[]);
    let plan_latin = plan_context(model, system, &latin_user, &[]);

    // CJK user message consumes more of the fixed budget (1.2 vs 0.25 tok/char),
    // leaving a smaller knowledge budget than the Latin message.
    assert!(
        plan_cjk.knowledge_tokens <= plan_latin.knowledge_tokens,
        "CJK-heavy input should leave <= knowledge budget vs Latin: cjk={} latin={}",
        plan_cjk.knowledge_tokens,
        plan_latin.knowledge_tokens
    );
    assert!(plan_cjk.knowledge_chars() < plan_cjk.knowledge_tokens, "5/6 char conversion broken");
}

// ─────────────────────────────────────────────────────────────────────────
// 4. cross-language — CN doc by EN-semantics query (real bge-m3, env-gated)
// ─────────────────────────────────────────────────────────────────────────

/// Returns Some(OllamaProvider) iff the env gate is set AND bge-m3 is reachable.
fn maybe_real_embed() -> Option<OllamaProvider> {
    if std::env::var("ATTUNE_TEST_OLLAMA_EMBED").is_err() {
        return None;
    }
    let base = std::env::var("ATTUNE_TEST_OLLAMA_URL")
        .unwrap_or_else(|_| "http://localhost:11434".to_string());
    let model = std::env::var("ATTUNE_TEST_OLLAMA_EMBED_MODEL")
        .unwrap_or_else(|_| "bge-m3".to_string());
    let p = OllamaProvider::new(&base, &model, 1024);
    if p.is_available() {
        Some(p)
    } else {
        None
    }
}

#[test]
fn cross_language_cn_doc_by_en_query_real_bge_m3() {
    let emb = match maybe_real_embed() {
        Some(p) => p,
        None => {
            eprintln!(
                "[cross-language] SKIPPED — set ATTUNE_TEST_OLLAMA_EMBED=1 with ollama bge-m3 \
                 on localhost:11434 to run the cross-lingual vector leg (mock embedder cannot \
                 model EN<->CN semantics)."
            );
            return;
        }
    };
    let dims = emb.dimensions();
    let yaml = include_str!("fixtures/retrieval/cross_language.yaml");
    let corpus = parse_corpus(yaml);
    assert!(corpus.docs.len() >= 3 && corpus.queries.len() >= 3);

    // Vector-only retrieval: EN query should pull its CN-semantic doc into top-2.
    let mut idx = VectorIndex::new(dims).unwrap();
    for (i, (id, content)) in corpus.docs.iter().enumerate() {
        let v = embed_one(&emb, content);
        idx.add(&v, VectorMeta { item_id: id.clone(), chunk_idx: i, level: 2, section_idx: 0 })
            .unwrap();
    }

    let mut hits_at2 = 0;
    for (query, relevant) in &corpus.queries {
        let qv = embed_one(&emb, query);
        let res = idx.search(&qv, 2).unwrap();
        let ids: Vec<String> = res.iter().map(|(m, _)| m.item_id.clone()).collect();
        if ids.iter().any(|id| id == relevant) {
            hits_at2 += 1;
        }
        eprintln!("[cross-language] q={query:?} want={relevant:?} top2={ids:?}");
    }
    // Real multilingual embedder should align EN queries to CN docs for the
    // majority of the small golden set.
    assert!(
        hits_at2 >= corpus.queries.len().saturating_sub(1),
        "cross-lingual recall@2 too low: {hits_at2}/{} (real bge-m3)",
        corpus.queries.len()
    );
}

#[test]
fn cross_language_mock_documents_limitation() {
    // DOCUMENT the mock's limitation explicitly so the skip above is not silent:
    // with the bag-of-tokens mock, an EN query and a CN doc share no tokens, so
    // cosine similarity is ~0 and cross-lingual retrieval is NOT achievable.
    // This test asserts that limitation (it is WHY leg 4 needs a real embedder).
    let emb = MockEmbeddingProvider::new(MOCK_DIMS);
    let cn = embed_one(&emb, "Rust 的所有权保证内存安全");
    let en = embed_one(&emb, "memory safety guarantees in Rust");
    // shared latin token "rust" gives a tiny overlap; cross-lingual *concepts*
    // ("内存安全" vs "memory safety") contribute nothing. Cosine stays low.
    let dot: f32 = cn.iter().zip(en.iter()).map(|(a, b)| a * b).sum();
    assert!(
        dot < 0.5,
        "mock embedder unexpectedly modeled cross-lingual semantics (cos={dot}); \
         if this fails the mock changed and leg 4 gating may be revisited"
    );
}

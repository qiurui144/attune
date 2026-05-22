//! Tier-aware context assembler — the single decision point that replaces the
//! "always inject L0 raw" assumption.
//!
//! Plan §3. A cheap heuristic classifies the query shape (zero LLM, microseconds),
//! the assembler then prefers a compact memory tier (L2/L3) for recall/overview
//! questions and falls back to today's L0+L1 path for precise ones — or whenever
//! the memory tier's hits are too weak to trust (the coverage gate guarantees no
//! regression).

use crate::context_compress::estimate_tokens;
use crate::crypto::Key32;
use crate::embed::EmbeddingProvider;
use crate::error::Result;
use crate::memory::retrieval::{search_memories, MemoryHit, MemoryVectorIndex};
use crate::search::{parse_time_filter, parse_time_filter_with_now, SearchResult, TimeFilter};
use crate::store::Store;

/// Heuristic query shape — sets the *preferred* tier; the assembler still verifies
/// coverage before committing to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryShape {
    /// Time words present → prefer L2 episodic, scoped to the window.
    Recall,
    /// Broad-intent / short topic question → prefer L3 semantic.
    Overview,
    /// Default — precise lookup, exact quote, code, numbers → L0 raw (unchanged).
    Precise,
}

/// Runtime knobs for the tiered assembler (mirrors the `memory.*` settings keys).
#[derive(Debug, Clone, Copy)]
pub struct MemoryConfig {
    /// Master switch — `false` reproduces today's L0+L1 behavior exactly.
    pub tiered_assembler_enabled: bool,
    /// Memory-tier hits below this cosine score do not displace L0 (coverage gate).
    pub memory_confidence: f32,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            tiered_assembler_enabled: true,
            memory_confidence: 0.70,
        }
    }
}

/// Overview-intent markers — broad "what do I know / summarize my" verbs.
const OVERVIEW_MARKERS: &[&str] = &[
    "总结", "回顾", "学了什么", "了解多少", "了解什么", "知道什么", "掌握了什么",
    "overview", "summarize my", "what do i know", "what have i", "recap",
];

/// One assembled context block destined for the RAG system prompt.
#[derive(Debug, Clone)]
pub struct ContextBlock {
    pub title: String,
    pub content: String,
    pub score: f32,
    /// Which layer this block came from: "L0" / "L1" / "L2" / "L3".
    pub tier: &'static str,
    /// item_id for L0/L1 anchor blocks (deep-link / citation); empty for memory tiers.
    pub item_id: String,
}

/// Result of tier-aware assembly.
#[derive(Debug, Clone)]
pub struct AssembledContext {
    pub blocks: Vec<ContextBlock>,
    /// The dominant tier that answered: "L0" (precise / fallback) or "L2"/"L3" (memory).
    pub tier_used: &'static str,
    /// Estimated injected-knowledge token count — drives the UI cost chip.
    pub est_tokens: usize,
    pub shape: QueryShape,
}

/// Count "content words" — CJK chars + whitespace-delimited latin runs.
fn content_word_count(query: &str) -> usize {
    let mut count = 0usize;
    let mut in_latin = false;
    for ch in query.chars() {
        if ('\u{4e00}'..='\u{9fff}').contains(&ch) {
            count += 1;
            in_latin = false;
        } else if ch.is_alphanumeric() {
            if !in_latin {
                count += 1;
                in_latin = true;
            }
        } else {
            in_latin = false;
        }
    }
    count
}

/// Classify a query's shape using only cheap existing signals — no model.
pub fn classify_query_shape(query: &str) -> QueryShape {
    classify_query_shape_with_now(query, chrono::Utc::now().timestamp())
}

/// Testable variant — injects `now` so time-word classification is deterministic.
pub fn classify_query_shape_with_now(query: &str, now_unix: i64) -> QueryShape {
    let q = query.to_lowercase();

    // Overview markers take precedence — "总结一下上周" is overview-shaped even with
    // a time word, because it wants a synthesized standing view.
    if OVERVIEW_MARKERS.iter().any(|m| q.contains(m)) {
        return QueryShape::Overview;
    }
    // Time words → recall.
    if parse_time_filter_with_now(query, now_unix).is_some() {
        return QueryShape::Recall;
    }
    // Short, broad topic question with no precise markers → overview.
    if content_word_count(query) <= 6 && !has_precise_marker(&q) {
        return QueryShape::Overview;
    }
    QueryShape::Precise
}

/// Markers that signal a precise lookup — code identifiers, quotes, numbers, paths.
fn has_precise_marker(q: &str) -> bool {
    q.contains('`')
        || q.contains('"')
        || q.contains('“')
        || q.contains("()")
        || q.contains("::")
        || q.contains('/')
        || q.contains('.')
        || q.contains('_')
        || q.chars().any(|c| c.is_ascii_digit())
}

/// Build the time filter for a recall query (`None` for non-recall shapes).
fn recall_time_filter(query: &str, shape: QueryShape) -> Option<TimeFilter> {
    if shape == QueryShape::Recall {
        parse_time_filter(query)
    } else {
        None
    }
}

/// Sum estimated tokens across blocks.
fn sum_tokens(blocks: &[ContextBlock]) -> usize {
    blocks
        .iter()
        .map(|b| estimate_tokens(&b.title) + estimate_tokens(&b.content))
        .sum()
}

/// Convert an L0 `SearchResult` to a context block (uses `inject_content` if the
/// caller already budget-sliced it, else the raw `content`).
fn l0_block(r: &SearchResult) -> ContextBlock {
    let content = r
        .inject_content
        .clone()
        .unwrap_or_else(|| r.content.clone());
    ContextBlock {
        title: r.title.clone(),
        content,
        score: r.score,
        tier: "L0",
        item_id: r.item_id.clone(),
    }
}

/// Convert a memory hit to a context block.
fn memory_block(hit: &MemoryHit, tier: &'static str) -> ContextBlock {
    let title = if tier == "L3" {
        "知识主题记忆".to_string()
    } else {
        "情景记忆".to_string()
    };
    ContextBlock {
        title,
        content: hit.memory.summary.clone(),
        score: hit.score,
        tier,
        item_id: String::new(),
    }
}

/// Assemble context for a chat call, choosing the cheapest tier that covers the query.
///
/// `l0_results` are the caller's existing `search_with_context` hits (already
/// retrieved). The assembler decides whether to displace them with compact memory
/// summaries. When `config.tiered_assembler_enabled` is false, or the query is
/// `Precise`, or memory hits are below `memory_confidence`, it returns the L0 path
/// unchanged — guaranteeing no regression.
#[allow(clippy::too_many_arguments)]
pub fn assemble_context(
    store: &Store,
    dek: &Key32,
    memory_index: &MemoryVectorIndex,
    embedder: &dyn EmbeddingProvider,
    query: &str,
    l0_results: &[SearchResult],
    config: MemoryConfig,
) -> Result<AssembledContext> {
    let l0_blocks: Vec<ContextBlock> = l0_results.iter().map(l0_block).collect();

    // Escape hatch — assembler off reproduces today's behavior exactly.
    if !config.tiered_assembler_enabled {
        let est = sum_tokens(&l0_blocks);
        return Ok(AssembledContext {
            blocks: l0_blocks,
            tier_used: "L0",
            est_tokens: est,
            shape: QueryShape::Precise,
        });
    }

    let shape = classify_query_shape(query);

    // Precise queries never leave L0 — no memory lookup at all.
    if shape == QueryShape::Precise {
        let est = sum_tokens(&l0_blocks);
        return Ok(AssembledContext {
            blocks: l0_blocks,
            tier_used: "L0",
            est_tokens: est,
            shape,
        });
    }

    // Recall → episodic (L2); Overview → semantic (L3).
    let (kind, tier): (&str, &'static str) = match shape {
        QueryShape::Recall => ("episodic", "L2"),
        QueryShape::Overview => ("semantic", "L3"),
        QueryShape::Precise => unreachable!(),
    };
    let time_filter = recall_time_filter(query, shape);

    let candidates = search_memories(
        store,
        dek,
        memory_index,
        embedder,
        query,
        kind,
        time_filter,
        3,
    )
    .unwrap_or_default();

    let best = candidates.first().map(|h| h.score).unwrap_or(0.0);

    // Coverage gate: memory tier must hit confidently to displace L0.
    if candidates.is_empty() || best < config.memory_confidence {
        let est = sum_tokens(&l0_blocks);
        return Ok(AssembledContext {
            blocks: l0_blocks,
            tier_used: "L0",
            est_tokens: est,
            shape,
        });
    }

    // Memory tier answers: inject up to 3 compact summaries + 1 L0 anchor chunk
    // (preserves a precise citation / deep-link target — plan §3.2).
    let mut blocks: Vec<ContextBlock> = candidates
        .iter()
        .take(3)
        .map(|h| memory_block(h, tier))
        .collect();
    if let Some(anchor) = l0_blocks.into_iter().next() {
        blocks.push(anchor);
    }
    let est = sum_tokens(&blocks);
    Ok(AssembledContext {
        blocks,
        tier_used: tier,
        est_tokens: est,
        shape,
    })
}

/// Roll the oldest dropped conversation turns into one cached summary turn.
///
/// Plan §3.3 — when `context_budget::plan_context` reports `history_dropped > 0`,
/// instead of silently discarding the oldest turns (information loss), summarize
/// them once. The summary is cached in `chunk_summaries` keyed by
/// `sha256(dropped turns)` with a synthetic `item_id = "conv:<session_id>"`, so a
/// long session pays one summarization the first time it overflows and every
/// subsequent call is a cache hit.
///
/// `dropped` is the oldest `(role, content)` turns being trimmed. Returns the
/// rolling summary, or `None` if there is nothing to compact / the LLM is down.
/// The summarization is cost tier 3 but amortized (one call per overflow point).
pub fn compact_history(
    store: &Store,
    dek: &Key32,
    llm: &dyn crate::llm::LlmProvider,
    session_id: &str,
    dropped: &[(String, String)],
) -> Option<String> {
    if dropped.is_empty() {
        return None;
    }
    let joined = dropped
        .iter()
        .map(|(role, content)| format!("{role}: {content}"))
        .collect::<Vec<_>>()
        .join("\n\n");
    let hash = crate::context_compress::chunk_hash(&joined);
    let item_id = format!("conv:{session_id}");

    // Cache hit — the rolling summary for this exact dropped span already exists.
    if let Ok(Some(cached)) = store.get_chunk_summary(dek, &hash, "economical") {
        return Some(cached);
    }
    // Miss — summarize once (tier 3) and cache.
    let summary = crate::context_compress::generate_summary(
        llm,
        &joined,
        crate::context_compress::ContextStrategy::Economical,
    )
    .ok()?;
    let _ = store.put_chunk_summary(
        dek,
        &hash,
        "economical",
        &item_id,
        llm.model_name(),
        &summary,
        joined.chars().count(),
    );
    Some(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::MockEmbeddingProvider;

    const FIXED_NOW: i64 = 1_714_000_000; // 2024-04-24

    fn shape(q: &str) -> QueryShape {
        classify_query_shape_with_now(q, FIXED_NOW)
    }

    #[test]
    fn classify_recall_on_time_words() {
        assert_eq!(shape("上周学习的 rust 笔记内容是什么"), QueryShape::Recall);
        assert_eq!(shape("what did i read 3 days ago about tokio"), QueryShape::Recall);
    }

    #[test]
    fn classify_overview_on_broad_markers() {
        assert_eq!(shape("总结一下我对 rust 的理解"), QueryShape::Overview);
        assert_eq!(shape("what do i know about distributed systems"), QueryShape::Overview);
    }

    #[test]
    fn classify_overview_on_short_topic() {
        assert_eq!(shape("机器学习"), QueryShape::Overview);
    }

    #[test]
    fn classify_precise_on_code_and_specifics() {
        assert_eq!(shape("the signature of `compress_chunk`"), QueryShape::Precise);
        assert_eq!(shape("MemoryVectorIndex::upsert 返回值类型"), QueryShape::Precise);
        assert_eq!(shape("HTTP 错误码 404 的含义和处理方式详解"), QueryShape::Precise);
    }

    #[test]
    fn overview_marker_beats_time_word() {
        // "总结" + "上周" → overview wins (wants a synthesized standing view).
        assert_eq!(shape("总结上周做了什么"), QueryShape::Overview);
    }

    fn make_l0(n: usize) -> Vec<SearchResult> {
        (0..n)
            .map(|i| SearchResult {
                item_id: format!("item-{i}"),
                score: 0.9 - (i as f32) * 0.1,
                title: format!("Doc {i}"),
                content: "原始 chunk 文本 ".repeat(40),
                source_type: "note".into(),
                ..Default::default()
            })
            .collect()
    }

    #[allow(dead_code)]
    fn seed_episodic(store: &Store, dek: &Key32, hash: &str, summary: &str, win: i64) -> String {
        store
            .insert_memory(dek, "episodic", win, win + 86400, &[hash.into()], summary, "m", win)
            .unwrap();
        store
            .list_recent_memories(dek, 1000)
            .unwrap()
            .into_iter()
            .find(|m| m.source_chunk_hashes == vec![hash])
            .unwrap()
            .id
    }

    #[test]
    fn assembler_off_passes_l0_through() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let idx = MemoryVectorIndex::new(64).unwrap();
        let emb = MockEmbeddingProvider::new(64);
        let l0 = make_l0(3);
        let cfg = MemoryConfig { tiered_assembler_enabled: false, memory_confidence: 0.70 };
        let out = assemble_context(&store, &dek, &idx, &emb, "总结一下", &l0, cfg).unwrap();
        assert_eq!(out.tier_used, "L0");
        assert_eq!(out.blocks.len(), 3);
        assert!(out.blocks.iter().all(|b| b.tier == "L0"));
    }

    #[test]
    fn precise_query_never_leaves_l0() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let idx = MemoryVectorIndex::new(64).unwrap();
        let emb = MockEmbeddingProvider::new(64);
        let l0 = make_l0(5);
        let out = assemble_context(
            &store, &dek, &idx, &emb,
            "`compress_chunk` 的函数签名是什么", &l0, MemoryConfig::default(),
        )
        .unwrap();
        assert_eq!(out.tier_used, "L0");
        assert_eq!(out.blocks.len(), 5);
    }

    #[test]
    fn weak_memory_hit_falls_back_to_l0() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        // Empty memory index → no candidates → coverage gate fails → L0 fallback.
        let idx = MemoryVectorIndex::new(128).unwrap();
        let emb = MockEmbeddingProvider::new(128);
        let l0 = make_l0(4);
        let out = assemble_context(
            &store, &dek, &idx, &emb, "总结我学了什么", &l0, MemoryConfig::default(),
        )
        .unwrap();
        assert_eq!(out.tier_used, "L0", "no memory coverage must fall back to L0");
        assert_eq!(out.blocks.len(), 4);
    }

    #[test]
    fn strong_memory_hit_displaces_l0_with_anchor() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let emb = MockEmbeddingProvider::new(256);
        let mut idx = MemoryVectorIndex::new(256).unwrap();

        // Seed an L3 semantic memory about a topic. The mock embedder shares tokens,
        // so the overview query embeds close to this memory → coverage gate passes.
        let topic_text = "用户对 Rust 所有权 借用 生命周期 形成了系统理解";
        let (id, _) = store
            .insert_semantic_memory(
                &dek, "topic-rust", &["m1".into(), "m2".into(), "m3".into(), "m4".into()],
                topic_text, "m", 0, 1000, 1000,
            )
            .unwrap();
        let v = emb.embed(&[topic_text]).unwrap().pop().unwrap();
        idx.upsert(&id, &v).unwrap();

        let l0 = make_l0(5);
        // Overview-marker query ("总结") → Overview shape → L3 semantic lookup.
        let out = assemble_context(
            &store, &dek, &idx, &emb,
            "总结 用户对 Rust 所有权 借用 生命周期 形成了系统理解",
            &l0, MemoryConfig::default(),
        )
        .unwrap();
        assert_eq!(out.shape, QueryShape::Overview);
        assert_eq!(out.tier_used, "L3");
        assert!(out.blocks.iter().any(|b| b.tier == "L3"));
        assert!(out.blocks.iter().any(|b| b.tier == "L0"), "must keep an L0 anchor");
        // token count strictly smaller than dumping all 5 raw chunks
        let l0_only: usize = l0
            .iter()
            .map(|r| estimate_tokens(&r.title) + estimate_tokens(&r.content))
            .sum();
        assert!(out.est_tokens < l0_only, "memory tier must inject fewer tokens");
    }

    #[test]
    fn recall_query_routes_to_l2_episodic() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let emb = MockEmbeddingProvider::new(256);
        let mut idx = MemoryVectorIndex::new(256).unwrap();

        // Episodic memory whose window covers "now-ish" so a "今天" filter overlaps.
        let now = chrono::Utc::now().timestamp();
        let day = 86400;
        let win_start = (now / day) * day;
        let summary = "用户研究了 tokio async runtime 与 future 调度";
        store
            .insert_memory(&dek, "episodic", win_start, win_start + day, &["e1".into()], summary, "m", now)
            .unwrap();
        let id = store
            .list_recent_memories(&dek, 10)
            .unwrap()
            .into_iter()
            .find(|m| m.source_chunk_hashes == vec!["e1"])
            .unwrap()
            .id;
        let v = emb.embed(&[summary]).unwrap().pop().unwrap();
        idx.upsert(&id, &v).unwrap();

        let l0 = make_l0(5);
        let out = assemble_context(
            &store, &dek, &idx, &emb,
            "今天研究的 tokio async runtime 与 future 调度内容",
            &l0, MemoryConfig::default(),
        )
        .unwrap();
        assert_eq!(out.shape, QueryShape::Recall);
        assert_eq!(out.tier_used, "L2");
        assert!(out.blocks.iter().any(|b| b.tier == "L2"));
    }

    #[test]
    fn compact_history_caches_rolling_summary() {
        use crate::llm::MockLlmProvider;
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let llm = MockLlmProvider::new("test-model");
        // 两次调用都要有 response；第二次应命中缓存不消耗
        llm.push_response("用户先问了 Rust 所有权，再问了借用规则。");
        llm.push_response("SHOULD-NOT-BE-USED");

        let dropped = vec![
            ("user".to_string(), "Rust 的所有权是什么".to_string()),
            ("assistant".to_string(), "所有权是 Rust 的核心内存模型……".to_string()),
            ("user".to_string(), "那借用规则呢".to_string()),
        ];
        let s1 = compact_history(&store, &dek, &llm, "sess-1", &dropped).unwrap();
        assert!(!s1.is_empty());
        // 第二次同样的 dropped → 缓存命中，返回与首次相同（不会用第二个 mock response）
        let s2 = compact_history(&store, &dek, &llm, "sess-1", &dropped).unwrap();
        assert_eq!(s1, s2, "second call must be a cache hit");
        assert_eq!(store.chunk_summary_count().unwrap(), 1);
    }

    #[test]
    fn compact_history_empty_returns_none() {
        use crate::llm::MockLlmProvider;
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let llm = MockLlmProvider::new("m");
        assert!(compact_history(&store, &dek, &llm, "s", &[]).is_none());
    }
}

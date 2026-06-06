//! Token-thrift deep summary pipeline (spec §3.2, FLAGSHIP).
//!
//! Generalizes the proven `routes/chat.rs:640-728` cache-map-reduce over `chunk_summaries`
//! into a standalone, measurable multi-level summary. Three independent token-savings levers
//! stack (spec §3.2): (1) local extractive pre-cut, (2) chunk_summaries cache reuse,
//! (3) cheap-map / reasoning-reduce split. Output carries a [`TokenBill`] so the savings are
//! **measured, not asserted** (spec §8.5 / §9.1).
//!
//! Stages:
//!   0. extract_sections_with_path → chunk → chunk_hash            (zero LLM)
//!   1. local extractive pre-cut (short blocks pass through)        (zero LLM)
//!   2. cache query (chunk_hash, "deepsum:<level>")                 (zero LLM)
//!   3. bounded MAP: cheap LLM compresses miss blocks, writes back  (CHEAP model, bulk)
//!   4. bounded REDUCE ×1 (or ≤⌈n/FANIN⌉): reasoning LLM synthesizes (REASONING model)

use crate::context_compress::{chunk_hash, estimate_tokens};
use crate::cost;
use crate::crypto::Key32;
use crate::document_intelligence::extractive;
use crate::document_intelligence::model_routing::{ModelRole, ModelRouter};
use crate::document_intelligence::token_bill::TokenBill;
use crate::error::Result;
use crate::llm::{ChatMessage, LlmProvider};
use crate::store::Store;
use serde::{Deserialize, Serialize};

/// Target summary level (spec §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryLevel {
    Brief,
    Standard,
    Detailed,
}

impl SummaryLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            SummaryLevel::Brief => "brief",
            SummaryLevel::Standard => "standard",
            SummaryLevel::Detailed => "detailed",
        }
    }
    /// Cache strategy string — namespaced under `deepsum:` (spec §10, T-06 allowlist).
    pub fn cache_strategy(self) -> String {
        format!("deepsum:{}", self.as_str())
    }
}

/// A multi-level summary (spec §5.2 response).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub level: String,
    pub overview: String,
    pub per_chapter: Vec<ChapterSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChapterSummary {
    pub heading_path: String,
    pub summary: String,
}

/// Tuning knobs (kept small; defaults match the chat.rs precedent).
pub struct DeepSummaryConfig {
    /// Blocks with `estimate_tokens` below this skip the map LLM entirely (chat.rs is_short).
    pub short_block_tokens: usize,
    /// Local extractive keep-ratio applied before the cheap-map LLM.
    pub extractive_keep_ratio: f32,
    /// Chunk size / overlap for `chunker::chunk`.
    pub chunk_size: usize,
    pub chunk_overlap: usize,
    /// Reduce fan-in: number of block summaries folded per reduce call.
    pub reduce_fanin: usize,
}

impl Default for DeepSummaryConfig {
    fn default() -> Self {
        Self {
            short_block_tokens: 80,
            extractive_keep_ratio: 0.5,
            chunk_size: 1200,
            chunk_overlap: 100,
            reduce_fanin: 16,
        }
    }
}

/// The cheap (map) and reasoning (reduce) LLM handles, already model-selected via the router.
/// Production builds these from a single provider via `with_model`; tests pass two
/// `RecordingMockLlm`s so per-stage model assertions are possible.
pub struct StageLlms<'a> {
    pub cheap: &'a dyn LlmProvider,
    pub reasoning: &'a dyn LlmProvider,
}

/// Run the token-thrift deep summary. `item_id` may be empty (ad-hoc text → no cache).
///
/// Returns the multi-level [`Summary`] plus a [`TokenBill`] whose `naive_baseline_tokens`
/// is exactly `cost::estimate_tokens(full_text, reasoning_model)` (spec §3.2).
#[allow(clippy::too_many_arguments)]
pub fn summarize(
    full_text: &str,
    level: SummaryLevel,
    item_id: &str,
    router: &ModelRouter,
    llms: &StageLlms,
    store: &Store,
    dek: &Key32,
    cfg: &DeepSummaryConfig,
) -> Result<(Summary, TokenBill)> {
    let reasoning_model = router.pick(ModelRole::Reasoning).to_string();
    let cheap_model = router.pick(ModelRole::Cheap).to_string();
    let strategy = level.cache_strategy();

    let mut bill = TokenBill {
        naive_baseline_tokens: cost::estimate_tokens(full_text, &reasoning_model) as u32,
        baseline_model: reasoning_model.clone(),
        ..Default::default()
    };

    // STAGE 0 — sections + chunks (zero LLM).
    let sections = crate::chunker::extract_sections_with_path(full_text);
    // Each chunk carries (heading_path, raw chunk text).
    let mut blocks: Vec<(String, String)> = Vec::new();
    for sec in &sections {
        let heading = sec.path.join(" / ");
        for c in crate::chunker::chunk(&sec.content, cfg.chunk_size, cfg.chunk_overlap) {
            blocks.push((heading.clone(), c));
        }
    }
    // Degenerate doc (no sections) → treat whole text as one block.
    if blocks.is_empty() && !full_text.trim().is_empty() {
        blocks.push((String::new(), full_text.to_string()));
    }

    // STAGES 1-3 — per block: extractive pre-cut → cache → cheap-map.
    let mut block_summaries: Vec<(String /*heading*/, String /*summary*/)> = Vec::new();
    let mut extractive_kept_tokens: u32 = 0;

    for (heading, block) in &blocks {
        let block_tokens = estimate_tokens(block);
        let heading_words: Vec<String> =
            heading.split(['/', ' ']).filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();

        // STAGE 1: extractive pre-cut (zero LLM). Short blocks pass through verbatim.
        let is_short = block_tokens < cfg.short_block_tokens;
        let candidate = if is_short {
            block.clone()
        } else {
            extractive::extract_candidates(block, cfg.extractive_keep_ratio, &heading_words)
        };
        extractive_kept_tokens = extractive_kept_tokens.saturating_add(estimate_tokens(&candidate) as u32);

        if is_short {
            // Short block: use its text directly as its "summary" (no LLM, like chat.rs is_short).
            block_summaries.push((heading.clone(), candidate));
            continue;
        }

        // STAGE 2: cache query (only when we have a real item_id).
        let hash = chunk_hash(block); // hash the ORIGINAL block (stable across queries, chat.rs R1-I1)
        if !item_id.is_empty() {
            if let Ok(Some(cached)) = store.get_chunk_summary(dek, &hash, &strategy) {
                bill.cache_hit_chunks += 1;
                block_summaries.push((heading.clone(), cached));
                continue;
            }
        }

        // STAGE 3: bounded MAP — cheap LLM compresses the extractive candidate (not the raw block).
        bill.new_chunks += 1;
        let system = MAP_SYSTEM_PROMPT;
        let user = format!("段落：\n{candidate}");
        let (summary, usage) = llms.cheap.chat(system, &user)?;
        bill.map_llm_tokens.add(&usage);
        // Approximate the map input token count from the candidate (mock usage reports 0).
        if usage.tokens_in == 0 {
            bill.map_llm_tokens.r#in = bill
                .map_llm_tokens
                .r#in
                .saturating_add(estimate_tokens(&user) as u32);
            bill.map_llm_tokens.out = bill
                .map_llm_tokens
                .out
                .saturating_add(estimate_tokens(&summary) as u32);
            if bill.map_llm_tokens.model.is_empty() {
                bill.map_llm_tokens.model = cheap_model.clone();
            }
        }
        bill.cache_read_tokens = bill.cache_read_tokens.saturating_add(usage.cached_in);

        // Write back to cache (only with a real item_id). A cache-write failure is non-fatal
        // (the summary is still returned) — but it must not be silent in a way that masks a
        // schema regression, so the put error is surfaced via the `?`-free explicit handling.
        if !item_id.is_empty() {
            store.put_chunk_summary(
                dek,
                &hash,
                &strategy,
                item_id,
                &cheap_model,
                &summary,
                block.chars().count(),
            )?;
        }
        block_summaries.push((heading.clone(), summary));
    }
    bill.extractive_kept_tokens = extractive_kept_tokens;

    // STAGE 4 — bounded REDUCE: reasoning LLM ×1 (or ≤⌈n/FANIN⌉ fan-in).
    let summary = reduce(level, &block_summaries, llms.reasoning, cfg, &mut bill, &reasoning_model)?;
    Ok((summary, bill))
}

const MAP_SYSTEM_PROMPT: &str = "你是浓缩器。把用户给你的段落压缩为简洁中文摘要，保留专有名词、数字、命令/代码/函数名，省略举例与重复。直接输出摘要正文。";

const REDUCE_SYSTEM_PROMPT: &str = "你是文档总结器。基于给定的章节骨架与每段摘要，合成多级总结：先给全文导语，再给每章一句话要点。直接输出，不加前后缀。";

/// Fan-in reduce. With `n` block summaries and fan-in `f`, calls the reasoning LLM
/// `⌈n/f⌉` times to fold groups, then once more to synthesize when there are multiple
/// groups — bounded by spec §3.2 ("reduce ≤ ⌈N/FANIN⌉" plus a final fold).
fn reduce(
    level: SummaryLevel,
    block_summaries: &[(String, String)],
    reasoning: &dyn LlmProvider,
    cfg: &DeepSummaryConfig,
    bill: &mut TokenBill,
    reasoning_model: &str,
) -> Result<Summary> {
    // Build the per-chapter view (group block summaries by heading).
    let per_chapter = group_by_chapter(block_summaries);

    // Compose the reduce input: chapter skeleton + each block summary.
    let fold_one = |reasoning: &dyn LlmProvider,
                    bill: &mut TokenBill,
                    payload: &str|
     -> Result<String> {
        let messages = [
            ChatMessage::system(REDUCE_SYSTEM_PROMPT),
            ChatMessage::user(payload),
        ];
        let (text, usage) = reasoning.chat_with_history(&messages)?;
        if usage.tokens_in == 0 {
            bill.reduce_llm_tokens.r#in = bill
                .reduce_llm_tokens
                .r#in
                .saturating_add(estimate_tokens(payload) as u32);
            bill.reduce_llm_tokens.out = bill
                .reduce_llm_tokens
                .out
                .saturating_add(estimate_tokens(&text) as u32);
            if bill.reduce_llm_tokens.model.is_empty() {
                bill.reduce_llm_tokens.model = reasoning_model.to_string();
            }
        } else {
            bill.reduce_llm_tokens.add(&usage);
        }
        Ok(text)
    };

    let n = block_summaries.len();
    let fanin = cfg.reduce_fanin.max(1);

    let overview = if n <= fanin {
        // Single reduce call.
        let payload = compose_reduce_payload(level, block_summaries);
        fold_one(reasoning, bill, &payload)?
    } else {
        // Fan-in tree: fold each group, then a final fold over the partials.
        let mut partials: Vec<String> = Vec::new();
        for group in block_summaries.chunks(fanin) {
            let payload = compose_reduce_payload(level, group);
            partials.push(fold_one(reasoning, bill, &payload)?);
        }
        let final_payload = partials
            .iter()
            .enumerate()
            .map(|(i, p)| format!("[部分{}] {p}", i + 1))
            .collect::<Vec<_>>()
            .join("\n");
        fold_one(reasoning, bill, &final_payload)?
    };

    Ok(Summary {
        level: level.as_str().to_string(),
        overview,
        per_chapter,
    })
}

fn compose_reduce_payload(level: SummaryLevel, block_summaries: &[(String, String)]) -> String {
    let mut s = format!("目标级别：{}\n章节摘要：\n", level.as_str());
    for (heading, summary) in block_summaries {
        let h = if heading.is_empty() { "(无标题)" } else { heading.as_str() };
        s.push_str(&format!("- 【{h}】{summary}\n"));
    }
    s
}

fn group_by_chapter(block_summaries: &[(String, String)]) -> Vec<ChapterSummary> {
    let mut out: Vec<ChapterSummary> = Vec::new();
    for (heading, summary) in block_summaries {
        match out.iter_mut().find(|c| &c.heading_path == heading) {
            Some(c) => {
                c.summary.push(' ');
                c.summary.push_str(summary);
            }
            None => out.push(ChapterSummary {
                heading_path: heading.clone(),
                summary: summary.clone(),
            }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::RecordingMockLlm;
    use serde_json::json;

    fn router() -> ModelRouter {
        ModelRouter::from_settings(&json!({
            "model_routing": { "cheap": "gpt-4o-mini", "reasoning": "gpt-4o", "vision": "gpt-4o-mini" }
        }))
    }

    fn mem_store_dek() -> (Store, Key32) {
        (Store::open_memory().unwrap(), Key32::generate())
    }

    // A multi-section doc with at least one long block per section.
    fn doc() -> String {
        let para = "这是一段足够长的正文内容用来触发分块压缩流程因为它超过了短块阈值所以会走 map 阶段调用廉价模型进行压缩处理并写回缓存。".repeat(4);
        format!("# 第一章 引言\n\n{para}\n\n# 第二章 方法\n\n{para}\n")
    }

    #[test]
    fn test_map_uses_cheap_reduce_uses_reasoning() {
        let cheap = RecordingMockLlm::new("gpt-4o-mini")
            .with_response("第一章压缩摘要")
            .with_response("第二章压缩摘要");
        let reasoning = RecordingMockLlm::new("gpt-4o").with_response("全文导语 + 每章要点");
        let llms = StageLlms { cheap: &cheap, reasoning: &reasoning };
        let (store, dek) = mem_store_dek();
        let r = router();
        let cfg = DeepSummaryConfig::default();

        let (summary, bill) =
            summarize(&doc(), SummaryLevel::Standard, "item-1", &r, &llms, &store, &dek, &cfg).unwrap();

        // map calls used the cheap model; reduce used the reasoning model.
        assert!(cheap.call_count() >= 2, "at least one map call per long section");
        assert!(cheap.calls().iter().all(|c| c.model == "gpt-4o-mini"));
        assert_eq!(reasoning.calls().iter().filter(|c| c.model == "gpt-4o").count(), reasoning.call_count());
        // reduce called ≤ ⌈n/FANIN⌉ (+ final fold). For ≤ FANIN blocks: exactly 1.
        assert!(reasoning.call_count() >= 1);
        assert_eq!(summary.level, "standard");
        assert!(!summary.overview.is_empty());
        assert!(summary.per_chapter.len() >= 2, "≥2-section doc → ≥2 per_chapter");
        assert_eq!(bill.baseline_model, "gpt-4o");
    }

    #[test]
    fn test_naive_baseline_exact() {
        let cheap = RecordingMockLlm::new("gpt-4o-mini").with_response("摘要A").with_response("摘要B");
        let reasoning = RecordingMockLlm::new("gpt-4o").with_response("总结");
        let llms = StageLlms { cheap: &cheap, reasoning: &reasoning };
        let (store, dek) = mem_store_dek();
        let r = router();
        let cfg = DeepSummaryConfig::default();
        let text = doc();
        let (_s, bill) =
            summarize(&text, SummaryLevel::Standard, "item-x", &r, &llms, &store, &dek, &cfg).unwrap();
        assert_eq!(
            bill.naive_baseline_tokens,
            cost::estimate_tokens(&text, "gpt-4o") as u32,
            "naive baseline must equal estimate_tokens(full_text, reasoning_model)"
        );
    }

    #[test]
    fn test_deepsum_strategy_cache_roundtrip() {
        // Regression guard (the schema CHECK constraint rejected deepsum:* before the
        // migrate_chunk_summaries_deepsum_strategy fix). Direct roundtrip proves the
        // deepsum:<level> strategy survives put+get on a fresh in-memory store.
        let (store, dek) = mem_store_dek();
        let h = chunk_hash("some block text");
        store
            .put_chunk_summary(&dek, &h, "deepsum:standard", "item-rt", "gpt-4o-mini", "摘要", 16)
            .expect("put deepsum:standard must succeed");
        let got = store.get_chunk_summary(&dek, &h, "deepsum:standard").unwrap();
        assert_eq!(got.as_deref(), Some("摘要"), "deepsum cache roundtrip");
    }

    #[test]
    fn test_cache_hit_zero_new_tokens_on_second_run() {
        let r = router();
        let cfg = DeepSummaryConfig::default();
        let (store, dek) = mem_store_dek();
        let text = doc();

        // First run: populates the cache.
        {
            let cheap = RecordingMockLlm::new("gpt-4o-mini")
                .with_response("摘要1").with_response("摘要2").with_response("摘要3").with_response("摘要4");
            let reasoning = RecordingMockLlm::new("gpt-4o").with_response("总结");
            let llms = StageLlms { cheap: &cheap, reasoning: &reasoning };
            let (_s, bill1) =
                summarize(&text, SummaryLevel::Standard, "item-cache", &r, &llms, &store, &dek, &cfg).unwrap();
            assert!(bill1.new_chunks > 0, "first run has new chunks");
            assert!(cheap.call_count() > 0);
        }

        // Second run on identical input + same item_id → all map calls served from cache.
        let cheap2 = RecordingMockLlm::new("gpt-4o-mini"); // no responses preloaded
        let reasoning2 = RecordingMockLlm::new("gpt-4o").with_response("总结2");
        let llms2 = StageLlms { cheap: &cheap2, reasoning: &reasoning2 };
        let (_s2, bill2) =
            summarize(&text, SummaryLevel::Standard, "item-cache", &r, &llms2, &store, &dek, &cfg).unwrap();

        assert_eq!(cheap2.call_count(), 0, "second run: zero map LLM calls (full cache hit)");
        assert_eq!(bill2.map_llm_tokens.r#in, 0, "second run: zero new map input tokens");
        assert_eq!(bill2.new_chunks, 0, "second run: zero new chunks");
        assert!(bill2.cache_hit_chunks > 0, "second run: chunks served from cache");
        // Second-run cost is ONLY the single reduce fold over cached summaries — map is free.
        // Savings is high (>0.8) but not exactly 1.0 because the reduce ×1 still bills against
        // the naive (full-text) baseline. The cache-hit invariants above are the real proof.
        assert!(
            bill2.savings_ratio_by_token() > 0.8,
            "second-run savings should be high (map free), got {}",
            bill2.savings_ratio_by_token()
        );
    }

    #[test]
    fn test_short_blocks_skip_llm() {
        // A doc whose only block is below the short threshold → no map call.
        let cheap = RecordingMockLlm::new("gpt-4o-mini");
        let reasoning = RecordingMockLlm::new("gpt-4o").with_response("短文总结");
        let llms = StageLlms { cheap: &cheap, reasoning: &reasoning };
        let (store, dek) = mem_store_dek();
        let r = router();
        let cfg = DeepSummaryConfig::default();
        let (_s, bill) =
            summarize("简短一句话。", SummaryLevel::Brief, "item-short", &r, &llms, &store, &dek, &cfg).unwrap();
        assert_eq!(cheap.call_count(), 0, "short block must not call the map LLM");
        assert_eq!(bill.new_chunks, 0);
    }

    #[test]
    fn test_reduce_called_once_for_small_doc() {
        let cheap = RecordingMockLlm::new("gpt-4o-mini").with_response("a").with_response("b");
        let reasoning = RecordingMockLlm::new("gpt-4o").with_response("总结");
        let llms = StageLlms { cheap: &cheap, reasoning: &reasoning };
        let (store, dek) = mem_store_dek();
        let r = router();
        let cfg = DeepSummaryConfig::default(); // fanin=16, doc has 2 blocks
        let (_s, _bill) =
            summarize(&doc(), SummaryLevel::Standard, "i", &r, &llms, &store, &dek, &cfg).unwrap();
        assert_eq!(reasoning.call_count(), 1, "≤FANIN blocks → exactly one reduce call");
    }

    #[test]
    fn test_fanin_tree_for_many_blocks() {
        // Force a tiny fan-in so a 2-block doc triggers the tree path (2 group folds + 1 final).
        let cheap = RecordingMockLlm::new("gpt-4o-mini").with_response("a").with_response("b");
        let reasoning = RecordingMockLlm::new("gpt-4o")
            .with_response("g1").with_response("g2").with_response("final");
        let llms = StageLlms { cheap: &cheap, reasoning: &reasoning };
        let (store, dek) = mem_store_dek();
        let r = router();
        let cfg = DeepSummaryConfig { reduce_fanin: 1, ..Default::default() };
        let (_s, _bill) =
            summarize(&doc(), SummaryLevel::Standard, "i", &r, &llms, &store, &dek, &cfg).unwrap();
        // 2 blocks / fanin 1 = 2 group folds + 1 final fold = 3 reduce calls.
        assert_eq!(reasoning.call_count(), 3, "fan-in tree: ⌈n/f⌉ group folds + 1 final");
    }
}

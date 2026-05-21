//! self_evolving_skill_agent — per-query expansion learner (SkillClaw style).
//!
//! ## Why a new agent on top of the existing `skill_evolution::run_evolution_cycle`?
//!
//! The legacy cycle is **topic-keyed** — clusters all failed queries by LLM-extracted
//! topic and writes them to `app_settings.search.learned_expansions`. That is great
//! for thematic grouping but has two limitations the v0.7 self-learning loop needs to
//! overcome:
//!
//! - **Cost binding** — the legacy cycle is LLM-only; if the user disables LLM, nothing learns. The new agent ships a zero-cost heuristic path so even LLM-off vaults benefit from per-user expansion.
//! - **Provenance and granularity** — the legacy cycle drops the source query; we cannot say "the user searched X 4 times and now we expand X → [a, b]". The new `skill_expansions` table keeps per-query rows with `generated_by` + `confidence`, so the UI can render learned vocabulary honestly and the user can delete individual rows.
//!
//! Both paths coexist. `expand_query_with_table` (the new search-side hook)
//! tries the exact `query_pattern` row first, then falls back to the legacy
//! topic-keyed `learned_expansions` blob.
//!
//! ## Cost contract
//!
//! Per `attune/CLAUDE.md` "Cost & Trigger Contract":
//!
//! - Layer 1 (CPU/ms): `Heuristic` path. Always allowed.
//! - Layer 3 (LLM): `Llm` path. **Only** when `cfg.enable_llm = true` AND the
//!   caller supplies an `LlmProvider`. Never opportunistically.
//!
//! The agent runs in the same 3-phase shape as `skill_evolution`:
//!
//! ```text
//! prepare()  — vault lock, fetch signals
//! generate() — no lock, pure CPU (Heuristic) or LLM call (Llm)
//! apply()    — vault lock, upsert rows + mark signals processed
//! ```
//!
//! ## Agent verification doctrine (`attune/CLAUDE.md` §"Agent 验证铁律")
//!
//! - **Deterministic on the heuristic path** → required pass rate 1.00 on goldens
//! - Ground truth in tests is computed independently (stoplist + token splitter
//!   reimplemented in the test harness), **never** via `agent.run()`
//! - ≥10 real golden cases (`tests/golden/skill_evolution/*.yaml`)
//! - ≥3 error cases (`error/*.yaml` subdir)
//! - ≥3 proptest invariants (idempotent / bounded / monotone)
//! - ≥5 boundary `#[test]`
//! - ≥1 integration test against a tempfile-backed `Store`
//! - ENFORCE mode: 0 violations

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::llm::LlmProvider;
use crate::store::{ExpansionSource, SkillExpansionRow, SkillSignal, Store};

// ── Public types ─────────────────────────────────────────────────────────────

/// How an expansion was generated. Mirrors [`ExpansionSource`] for the public
/// agent surface; we keep both because the store layer is provenance-agnostic
/// (it accepts strings) while the agent should communicate in typed enums.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GeneratedBy {
    /// Default — cheapest path, matches `SkillAgentConfig::default`'s
    /// `enable_llm = false`.
    #[default]
    Heuristic,
    Llm,
}

impl GeneratedBy {
    pub fn as_str(self) -> &'static str {
        match self {
            GeneratedBy::Heuristic => "heuristic",
            GeneratedBy::Llm => "llm",
        }
    }
    pub fn to_source(self) -> ExpansionSource {
        match self {
            GeneratedBy::Heuristic => ExpansionSource::Heuristic,
            GeneratedBy::Llm => ExpansionSource::Llm,
        }
    }
}

/// One generated row — what the agent *would* persist. The agent does not
/// modify the user's original query expression; expansions are appended at
/// search time by `expand_query_with_table`.
#[derive(Debug, Clone, PartialEq)]
pub struct EvolutionRecord {
    pub query_pattern: String,
    pub expansions: Vec<String>,
    pub generated_by: GeneratedBy,
    pub confidence: f32,
}

/// Aggregated outcome of one agent run cycle.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct EvolutionRunStats {
    /// Total `search_miss` signals examined.
    pub signals_considered: usize,
    /// Distinct query_patterns that crossed `min_signal_count`.
    pub patterns_above_threshold: usize,
    /// Rows written (insert or upsert). May be < `patterns_above_threshold` if
    /// the agent decided an existing row was already strong enough.
    pub rows_written: usize,
    /// Generator path used. `Heuristic` is the default; flipped to `Llm` only
    /// when the LLM path was taken for at least one record.
    pub used_path: GeneratedBy,
}

/// Agent runtime configuration.
#[derive(Debug, Clone)]
pub struct SkillAgentConfig {
    /// Only consider signals created within the last N days. 0 = unlimited.
    pub window_days: u32,
    /// Query must appear ≥ this many times in the window before the agent
    /// considers expanding it. Defense against one-off typos.
    pub min_signal_count: u32,
    /// Maximum signals to scan per cycle (defence: very large unprocessed
    /// queue should not blow up LLM prompt). Mirrors
    /// `skill_evolution::MAX_SIGNALS_PER_CYCLE`.
    pub max_signals_per_cycle: usize,
    /// If true and an `LlmProvider` is given, the agent uses the LLM path.
    /// If false, the agent runs heuristic regardless of whether an LLM is
    /// available — required for the "零成本档" tier.
    pub enable_llm: bool,
}

impl Default for SkillAgentConfig {
    fn default() -> Self {
        Self {
            window_days: 14,
            min_signal_count: 3,
            max_signals_per_cycle: 50,
            enable_llm: false,
        }
    }
}

/// The agent itself — a zero-sized struct; all state lives in the `Store`.
/// Same shape as [`crate::memory::consolidation_agent::MemoryConsolidationAgent`].
pub struct SelfEvolvingSkillAgent;

impl SelfEvolvingSkillAgent {
    pub fn id() -> &'static str {
        "self_evolving_skill_agent"
    }
    pub fn description() -> &'static str {
        "Per-query expansion learner (heuristic by default, LLM opt-in). \
         Reads skill_signals → writes skill_expansions; never edits user queries."
    }
}

// ── 3-phase API (vault-lock-discipline aware) ────────────────────────────────

/// Phase 1: pull unprocessed `search_miss` signals within the window, bucket
/// them by canonical query_pattern, and drop patterns below
/// `cfg.min_signal_count`. **Vault lock required.**
///
/// Returns:
/// - `Ok(None)` if no qualifying patterns (the worker should idle this cycle).
/// - `Ok(Some(buckets))` map of `query_pattern → Vec<signal_id>` for Phase 3.
///
/// `buckets` is intentionally a stable ordering (BTreeMap) so two prepare()
/// calls on the same data produce the same bucket list — required for
/// idempotency proptest.
pub fn prepare_run(
    store: &Store,
    cfg: &SkillAgentConfig,
    now_secs: i64,
) -> Result<Option<Vec<QueryBucket>>> {
    let signals = store.get_unprocessed_signals(cfg.max_signals_per_cycle)?;
    let buckets = group_signals_by_pattern(&signals, cfg, now_secs);
    if buckets.is_empty() {
        Ok(None)
    } else {
        Ok(Some(buckets))
    }
}

/// A grouped bundle of signals for one query_pattern — emitted by `prepare_run`
/// and consumed by `generate_records` / `apply_records`.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryBucket {
    pub query_pattern: String,
    pub occurrences: u32,
    pub signal_ids: Vec<i64>,
}

/// Phase 2: turn buckets into [`EvolutionRecord`]s. **No vault lock held**.
///
/// Strategy:
/// - Always compute the heuristic record (zero cost).
/// - If `cfg.enable_llm` AND `llm.is_some()`, ALSO compute the LLM record and
///   the LLM record takes precedence on apply (LLM > heuristic).
/// - If LLM call fails, fall back to the heuristic record (never drop the
///   bucket — a partial result is better than none).
///
/// The function never reads or writes the `Store`. Pure function over inputs.
pub fn generate_records(
    buckets: &[QueryBucket],
    llm: Option<&dyn LlmProvider>,
    cfg: &SkillAgentConfig,
) -> Vec<EvolutionRecord> {
    let mut out: Vec<EvolutionRecord> = Vec::with_capacity(buckets.len());
    let llm_enabled = cfg.enable_llm && llm.is_some();

    for bucket in buckets {
        let heuristic = heuristic_expansion(&bucket.query_pattern, buckets);
        let mut record_for_bucket: Option<EvolutionRecord> = None;

        if llm_enabled {
            if let Some(provider) = llm {
                match llm_expansion(provider, &bucket.query_pattern) {
                    Ok(terms) if !terms.is_empty() => {
                        record_for_bucket = Some(EvolutionRecord {
                            query_pattern: bucket.query_pattern.clone(),
                            expansions: terms,
                            generated_by: GeneratedBy::Llm,
                            confidence: GeneratedBy::Llm.to_source().default_confidence(),
                        });
                    }
                    _ => {
                        // LLM failure → fall through to heuristic.
                    }
                }
            }
        }

        let record = record_for_bucket.unwrap_or_else(|| EvolutionRecord {
            query_pattern: bucket.query_pattern.clone(),
            expansions: heuristic,
            generated_by: GeneratedBy::Heuristic,
            confidence: GeneratedBy::Heuristic.to_source().default_confidence(),
        });

        // Skip rows with empty expansions — nothing to learn.
        if !record.expansions.is_empty() {
            out.push(record);
        }
    }
    out
}

/// Phase 3: upsert records into `skill_expansions` and mark the consumed
/// signals as processed. **Vault lock required.**
///
/// Returns aggregated stats. Idempotent — re-running with the same records
/// against the same data writes no new rows (the per-source guard in
/// `upsert_skill_expansion` prevents downgrade/duplication).
pub fn apply_records(
    store: &Store,
    buckets: &[QueryBucket],
    records: &[EvolutionRecord],
) -> Result<EvolutionRunStats> {
    let mut stats = EvolutionRunStats {
        signals_considered: buckets.iter().map(|b| b.signal_ids.len()).sum(),
        patterns_above_threshold: buckets.len(),
        rows_written: 0,
        used_path: GeneratedBy::Heuristic,
    };

    for r in records {
        let written = store.upsert_skill_expansion(
            &r.query_pattern,
            &r.expansions,
            r.generated_by.to_source(),
            r.confidence,
        )?;
        if written {
            stats.rows_written += 1;
        }
        if r.generated_by == GeneratedBy::Llm {
            stats.used_path = GeneratedBy::Llm;
        }
    }

    // Mark all consumed signals processed — even if their row was skipped (the
    // signal has already been "seen"; otherwise the next cycle re-reads them
    // and we burn CPU re-running heuristic on stuck patterns).
    let ids: Vec<i64> = buckets.iter().flat_map(|b| b.signal_ids.iter().copied()).collect();
    if !ids.is_empty() {
        store.mark_signals_processed(&ids)?;
    }
    Ok(stats)
}

/// Single-call convenience wrapper. Acquires no extra locks beyond what
/// `prepare_run` / `apply_records` need — caller must hold the vault lock
/// across the whole call. For the production worker, prefer the 3-phase API
/// so the LLM call happens with the lock released (see `state.rs`
/// `start_skill_evolver` for the pattern).
pub fn run_cycle(
    store: &Store,
    llm: Option<&dyn LlmProvider>,
    cfg: &SkillAgentConfig,
    now_secs: i64,
) -> Result<EvolutionRunStats> {
    let Some(buckets) = prepare_run(store, cfg, now_secs)? else {
        return Ok(EvolutionRunStats::default());
    };
    let records = generate_records(&buckets, llm, cfg);
    apply_records(store, &buckets, &records)
}

// ── Heuristic expansion (zero-cost path) ─────────────────────────────────────

/// Curated multilingual stopword list — small, hand-picked from frequent
/// CJK function words + English particles. Anything in this list is *never*
/// emitted as an expansion candidate even if it co-occurs with the query.
const STOPWORDS: &[&str] = &[
    // English
    "the", "a", "an", "of", "to", "for", "in", "on", "with", "and", "or",
    "is", "are", "be", "by", "at", "as", "it", "this", "that", "how", "what",
    "why", "when", "where", "who", "which", "from", "do", "does", "did",
    // Chinese function words / very common high-frequency words
    "的", "了", "是", "在", "和", "与", "或", "为", "我", "你", "他", "她",
    "它", "这", "那", "如何", "怎么", "什么", "怎样", "为什么", "请", "把",
    "被", "有", "无", "对", "从", "到", "及", "等",
];

fn is_stopword(token: &str) -> bool {
    let lower = token.to_lowercase();
    STOPWORDS.contains(&lower.as_str())
}

/// Tokenize a query for the heuristic — emits both ASCII words (split on
/// non-alphanumeric) AND CJK character bigrams (since Chinese has no word
/// boundaries and a bigram is the cheapest "word-ish" unit without pulling
/// in tantivy-jieba). The split lets one pattern share signals with another
/// that has overlapping tokens.
pub fn tokenize_for_heuristic(query: &str) -> Vec<String> {
    let mut toks: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cjk_buf: Vec<char> = Vec::new();

    let push_ascii = |buf: &mut String, out: &mut Vec<String>| {
        let s = buf.trim();
        if !s.is_empty() {
            let lowered = s.to_lowercase();
            if !is_stopword(&lowered) && lowered.chars().count() >= 2 {
                out.push(lowered);
            }
        }
        buf.clear();
    };

    let flush_cjk = |buf: &mut Vec<char>, out: &mut Vec<String>| {
        // Emit char bigrams: ABC → AB, BC. Single char gets emitted as-is
        // (a one-char query like "钱" still deserves a token).
        if buf.len() == 1 {
            let t: String = buf.iter().collect();
            if !is_stopword(&t) {
                out.push(t);
            }
        } else {
            for i in 0..buf.len().saturating_sub(1) {
                let bigram: String = buf[i..i + 2].iter().collect();
                if !is_stopword(&bigram) {
                    out.push(bigram);
                }
            }
        }
        buf.clear();
    };

    for c in query.chars() {
        if is_cjk(c) {
            if !cur.is_empty() {
                push_ascii(&mut cur, &mut toks);
            }
            cjk_buf.push(c);
        } else if c.is_alphanumeric() {
            if !cjk_buf.is_empty() {
                flush_cjk(&mut cjk_buf, &mut toks);
            }
            cur.push(c);
        } else {
            if !cur.is_empty() {
                push_ascii(&mut cur, &mut toks);
            }
            if !cjk_buf.is_empty() {
                flush_cjk(&mut cjk_buf, &mut toks);
            }
        }
    }
    if !cur.is_empty() {
        push_ascii(&mut cur, &mut toks);
    }
    if !cjk_buf.is_empty() {
        flush_cjk(&mut cjk_buf, &mut toks);
    }

    // Dedup while keeping order.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    toks.retain(|t| seen.insert(t.clone()));
    toks
}

fn is_cjk(c: char) -> bool {
    matches!(
        c as u32,
        0x4E00..=0x9FFF      // CJK Unified Ideographs
            | 0x3400..=0x4DBF // CJK Extension A
            | 0x3000..=0x303F // CJK Symbols and Punctuation (excluded by is_alphanumeric anyway)
            | 0x3040..=0x30FF // Hiragana / Katakana
    )
}

/// Pure-function heuristic — given the *target* pattern and the *other*
/// buckets in this cycle, find tokens that co-occur in other failed queries
/// but are NOT in the target itself. Those are candidate expansion terms
/// (other words the user *also* searched alongside this concept).
///
/// Bounded by [`crate::store::MAX_EXPANSIONS_PER_PATTERN`].
pub fn heuristic_expansion(target: &str, all_buckets: &[QueryBucket]) -> Vec<String> {
    let target_tokens: BTreeSet<String> =
        tokenize_for_heuristic(target).into_iter().collect();
    if target_tokens.is_empty() {
        return Vec::new();
    }

    let mut co_occurrence: HashMap<String, u32> = HashMap::new();
    for bucket in all_buckets {
        if bucket.query_pattern == target {
            continue;
        }
        let other_tokens: BTreeSet<String> =
            tokenize_for_heuristic(&bucket.query_pattern).into_iter().collect();
        // Only count "other" buckets that share at least one token with the
        // target — co-occurrence in a *related* failed query.
        if other_tokens.intersection(&target_tokens).next().is_none() {
            continue;
        }
        for tok in other_tokens.difference(&target_tokens) {
            *co_occurrence.entry(tok.clone()).or_insert(0) += bucket.occurrences;
        }
    }

    // Sort by score desc, tie-break lexicographic asc (deterministic).
    let mut ranked: Vec<(String, u32)> = co_occurrence.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    ranked
        .into_iter()
        .take(crate::store::MAX_EXPANSIONS_PER_PATTERN)
        .map(|(t, _)| t)
        .collect()
}

// ── LLM expansion (opt-in, layer 💰) ────────────────────────────────────────

fn llm_expansion(llm: &dyn LlmProvider, query_pattern: &str) -> Result<Vec<String>> {
    let prompt = format!(
        r#"User searched the local knowledge base for "{query_pattern}" but got zero results.
Provide up to 5 short related search terms (synonyms / related concepts / common abbreviations)
that the user might also want to try. Return STRICT JSON only, no prose:

{{
  "terms": ["term1", "term2", "term3"]
}}

Constraints:
- Each term ≤ 30 characters
- Each term is a keyword phrase, NOT a sentence
- Do NOT include the original query text itself
- 5 terms maximum"#,
    );
    let messages = vec![crate::llm::ChatMessage::user(&prompt)];
    let raw = llm.chat_with_history(&messages).map_err(|e| {
        crate::error::VaultError::LlmUnavailable(format!("skill agent LLM call: {e}"))
    })?;
    Ok(parse_llm_terms(&raw, query_pattern))
}

/// Extract `terms: [...]` from an LLM response, tolerant of ```json fences```
/// and trailing prose.
pub(crate) fn parse_llm_terms(raw: &str, query_pattern: &str) -> Vec<String> {
    let json_str = strip_fences(raw);
    let value: serde_json::Value = match serde_json::from_str(&json_str) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let arr = match value.get("terms").and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return Vec::new(),
    };

    let target_lower = query_pattern.to_lowercase();
    let mut out: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for v in arr {
        let Some(s) = v.as_str() else { continue };
        let s = s.trim();
        if s.is_empty() || s.len() > 60 {
            continue;
        }
        // Don't echo the user's own query.
        if s.to_lowercase() == target_lower {
            continue;
        }
        let key = s.to_lowercase();
        if seen.insert(key) {
            out.push(s.to_string());
        }
        if out.len() >= 5 {
            break;
        }
    }
    out
}

fn strip_fences(raw: &str) -> String {
    if let Some(start) = raw.find("```json") {
        let after = &raw[start + 7..];
        if let Some(end) = after.find("```") {
            return after[..end].trim().to_string();
        }
    }
    if let Some(start) = raw.find("```") {
        let after = &raw[start + 3..];
        if let Some(end) = after.find("```") {
            return after[..end].trim().to_string();
        }
    }
    if let Some(start) = raw.find('{') {
        if let Some(end) = raw.rfind('}') {
            if end > start {
                return raw[start..=end].to_string();
            }
        }
    }
    raw.trim().to_string()
}

// ── Signal bucketing ─────────────────────────────────────────────────────────

/// Canonicalize a query for use as a `query_pattern` key — lowercase + trim.
/// Whitespace inside is preserved (so "rust async" ≠ "rustasync"); a fancier
/// normalizer is out of scope (the legacy `expand_query` already does
/// `.contains()` on the lowercased query so trimming+lowercasing is enough).
fn canonical_pattern(q: &str) -> String {
    q.trim().to_lowercase()
}

/// Bucket raw signals → patterns with occurrence counts and signal ids.
/// Filters out short / empty queries (`min_query_chars = 2`) so a stray "a"
/// search never makes it into the table.
pub fn group_signals_by_pattern(
    signals: &[SkillSignal],
    cfg: &SkillAgentConfig,
    now_secs: i64,
) -> Vec<QueryBucket> {
    let mut by_pattern: HashMap<String, (u32, Vec<i64>)> = HashMap::new();
    let cutoff_secs = if cfg.window_days == 0 {
        i64::MIN
    } else {
        now_secs - (cfg.window_days as i64) * 86_400
    };

    for s in signals {
        if s.query.trim().chars().count() < 2 {
            continue;
        }
        // skill_signals.created_at is a TEXT ISO timestamp; we can't reliably
        // window-filter without parsing. The store-level `get_unprocessed_signals`
        // is already ASC-ordered and bounded by limit, so the windowing is a
        // soft floor — if the parse fails we keep the signal (safer to over-
        // count than to silently drop).
        if let Ok(t) = chrono::DateTime::parse_from_rfc3339(&s.created_at) {
            if t.timestamp() < cutoff_secs {
                continue;
            }
        } else if let Ok(t) = chrono::NaiveDateTime::parse_from_str(
            &s.created_at,
            "%Y-%m-%d %H:%M:%S",
        ) {
            if t.and_utc().timestamp() < cutoff_secs {
                continue;
            }
        }
        let key = canonical_pattern(&s.query);
        let entry = by_pattern.entry(key).or_insert_with(|| (0, Vec::new()));
        entry.0 += 1;
        entry.1.push(s.id);
    }

    // Stable order by (occurrences desc, pattern asc) — required for tests.
    let mut buckets: Vec<QueryBucket> = by_pattern
        .into_iter()
        .filter(|(_, (count, _))| *count >= cfg.min_signal_count)
        .map(|(pattern, (count, ids))| QueryBucket {
            query_pattern: pattern,
            occurrences: count,
            signal_ids: ids,
        })
        .collect();
    buckets.sort_by(|a, b| {
        b.occurrences
            .cmp(&a.occurrences)
            .then_with(|| a.query_pattern.cmp(&b.query_pattern))
    });
    buckets
}

// ── Search-side expansion hook ───────────────────────────────────────────────

/// Search-time expansion: try exact `skill_expansions` row first, fall back to
/// the legacy topic-keyed blob (`expand_query`). Returns the original query if
/// no expansion fires.
///
/// This is the function the chat / search route calls *instead of*
/// `expand_query` once the new table is live. We keep the legacy function
/// for backward compatibility.
pub fn expand_query_with_table(
    store: &Store,
    query: &str,
    legacy_settings: &serde_json::Value,
) -> String {
    let canonical = canonical_pattern(query);
    if let Ok(Some(row)) = store.get_skill_expansion(&canonical) {
        return apply_expansion_row(query, &row);
    }
    // Fall back to legacy topic-keyed expansion.
    super::expand_query(query, legacy_settings)
}

fn apply_expansion_row(original_query: &str, row: &SkillExpansionRow) -> String {
    let lower = original_query.to_lowercase();
    let mut extras: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for t in &row.expansions {
        let lt = t.to_lowercase();
        if lower.contains(&lt) {
            continue;
        }
        if seen.insert(lt.clone()) {
            extras.push(t.clone());
        }
    }
    if extras.is_empty() {
        original_query.to_string()
    } else {
        format!("{} {}", original_query, extras.join(" "))
    }
}

// ── Unit tests (≥5 boundary, in-module) ──────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(id: i64, query: &str) -> SkillSignal {
        SkillSignal {
            id,
            query: query.to_string(),
            knowledge_count: 0,
            web_used: false,
            // ISO 8601 — group_signals_by_pattern accepts both RFC3339 and
            // SQLite's "YYYY-MM-DD HH:MM:SS"; we hand it the SQLite shape.
            created_at: "2026-05-19 12:00:00".to_string(),
        }
    }

    // ── B1: empty signals → no buckets, no crash. ─────────────────────────
    #[test]
    fn boundary_empty_signals_no_buckets() {
        let cfg = SkillAgentConfig::default();
        let buckets = group_signals_by_pattern(&[], &cfg, 1_764_547_200);
        assert!(buckets.is_empty());
    }

    // ── B2: min_signal_count gate excludes one-offs. ──────────────────────
    #[test]
    fn boundary_min_signal_count_gate() {
        let signals = vec![
            sig(1, "rust ownership"),
            sig(2, "rust ownership"),
            // 1 occurrence only, below default min=3
            sig(3, "another query"),
        ];
        let cfg = SkillAgentConfig {
            window_days: 0,
            min_signal_count: 3,
            ..Default::default()
        };
        let buckets = group_signals_by_pattern(&signals, &cfg, 1_764_547_200);
        // Both queries are below threshold (count 2 and 1) → empty.
        assert!(buckets.is_empty());
    }

    // ── B3: identical queries are bucketed case-insensitively. ────────────
    #[test]
    fn boundary_case_insensitive_bucketing() {
        let signals = vec![
            sig(1, "Rust Ownership"),
            sig(2, "rust ownership"),
            sig(3, "RUST OWNERSHIP"),
        ];
        let cfg = SkillAgentConfig {
            window_days: 0,
            min_signal_count: 3,
            ..Default::default()
        };
        let buckets = group_signals_by_pattern(&signals, &cfg, 1_764_547_200);
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].query_pattern, "rust ownership");
        assert_eq!(buckets[0].occurrences, 3);
        assert_eq!(buckets[0].signal_ids.len(), 3);
    }

    // ── B4: heuristic gives empty expansion when no co-occurring buckets. ─
    #[test]
    fn boundary_heuristic_isolated_bucket() {
        let only = vec![QueryBucket {
            query_pattern: "rust ownership".into(),
            occurrences: 5,
            signal_ids: vec![1, 2, 3, 4, 5],
        }];
        let expansion = heuristic_expansion("rust ownership", &only);
        assert!(expansion.is_empty(), "no co-occurring tokens → empty");
    }

    // ── B5: heuristic respects MAX_EXPANSIONS_PER_PATTERN cap. ───────────
    #[test]
    fn boundary_heuristic_respects_max() {
        // Build 20 co-occurring buckets, each contributing a distinct token.
        let mut all = vec![QueryBucket {
            query_pattern: "rust ownership".into(),
            occurrences: 5,
            signal_ids: vec![1],
        }];
        for i in 0..20 {
            all.push(QueryBucket {
                query_pattern: format!("rust extraword{i:02}"),
                occurrences: (20 - i) as u32,
                signal_ids: vec![100 + i as i64],
            });
        }
        let exp = heuristic_expansion("rust ownership", &all);
        assert!(
            exp.len() <= crate::store::MAX_EXPANSIONS_PER_PATTERN,
            "len {} > cap {}",
            exp.len(),
            crate::store::MAX_EXPANSIONS_PER_PATTERN
        );
        // Should be sorted by frequency desc — the highest-occurrence bucket
        // contributes the first tokens.
        assert!(exp.iter().any(|t| t == "extraword00"),
            "highest-occurrence token should be present: {:?}", exp);
    }

    // ── tokenizer correctness ────────────────────────────────────────────

    #[test]
    fn tokenize_ascii_and_cjk() {
        let toks = tokenize_for_heuristic("Rust 所有权 borrow checker");
        assert!(toks.contains(&"rust".to_string()), "{:?}", toks);
        assert!(toks.contains(&"borrow".to_string()), "{:?}", toks);
        assert!(toks.contains(&"checker".to_string()), "{:?}", toks);
        // CJK bigram for 所有权 should produce at least 所有 + 有权.
        assert!(toks.iter().any(|t| t == "所有"), "{:?}", toks);
        assert!(toks.iter().any(|t| t == "有权"), "{:?}", toks);
    }

    #[test]
    fn tokenize_strips_stopwords() {
        let toks = tokenize_for_heuristic("the rust of ownership");
        assert!(!toks.contains(&"the".to_string()), "{:?}", toks);
        assert!(!toks.contains(&"of".to_string()), "{:?}", toks);
        assert!(toks.contains(&"rust".to_string()));
        assert!(toks.contains(&"ownership".to_string()));
    }

    #[test]
    fn parse_llm_terms_respects_cap_and_dedup() {
        let resp = r#"```json
{"terms": ["foo", "bar", "foo", "baz", "qux", "quux", "extra"]}
```"#;
        let out = parse_llm_terms(resp, "target");
        // Dedup + cap=5
        assert_eq!(out.len(), 5);
        assert!(out.iter().all(|t| t != "foo" || out.iter().filter(|x| **x == "foo").count() == 1));
    }

    #[test]
    fn parse_llm_terms_drops_echo() {
        let resp = r#"{"terms": ["target", "alt1", "alt2"]}"#;
        let out = parse_llm_terms(resp, "target");
        assert_eq!(out, vec!["alt1", "alt2"]);
    }

    #[test]
    fn parse_llm_terms_invalid_returns_empty() {
        assert!(parse_llm_terms("sorry I can't", "x").is_empty());
    }

    #[test]
    fn apply_records_writes_rows() {
        let store = Store::open_memory().unwrap();
        // Seed real signals so mark_signals_processed has something to flip.
        for _ in 0..5 {
            store.record_skill_signal("rust ownership", 0, false).unwrap();
        }
        let real_signals = store.get_unprocessed_signals(50).unwrap();
        let buckets = vec![QueryBucket {
            query_pattern: "rust ownership".into(),
            occurrences: 5,
            signal_ids: real_signals.iter().map(|s| s.id).collect(),
        }];
        let records = vec![EvolutionRecord {
            query_pattern: "rust ownership".into(),
            expansions: vec!["borrow".into(), "lifetime".into()],
            generated_by: GeneratedBy::Heuristic,
            confidence: 0.4,
        }];
        let stats = apply_records(&store, &buckets, &records).unwrap();
        assert_eq!(stats.rows_written, 1);
        let row = store.get_skill_expansion("rust ownership").unwrap().unwrap();
        assert_eq!(row.expansions, vec!["borrow".to_string(), "lifetime".to_string()]);
        // Signals marked processed.
        assert_eq!(store.count_unprocessed_signals().unwrap(), 0);
    }

    #[test]
    fn run_cycle_heuristic_smoke() {
        let store = Store::open_memory().unwrap();
        // 3 "rust ownership" + 3 "rust borrow" failed searches (each crosses min=3)
        for _ in 0..3 {
            store.record_skill_signal("rust ownership", 0, false).unwrap();
            store.record_skill_signal("rust borrow", 0, false).unwrap();
        }
        let cfg = SkillAgentConfig {
            window_days: 0,
            min_signal_count: 3,
            ..Default::default()
        };
        let stats = run_cycle(&store, None, &cfg, 1_764_547_200).unwrap();
        assert!(stats.rows_written >= 1, "should learn at least one pattern: {stats:?}");
        // Heuristic path used (no LLM provider supplied).
        assert_eq!(stats.used_path, GeneratedBy::Heuristic);
    }

    #[test]
    fn expand_query_with_table_prefers_exact_row() {
        let store = Store::open_memory().unwrap();
        store
            .upsert_skill_expansion(
                "rust ownership",
                &["borrow".into(), "lifetime".into()],
                ExpansionSource::Heuristic,
                0.4,
            )
            .unwrap();
        let legacy = serde_json::json!({
            "search": {
                "learned_expansions": {
                    "rust": ["cargo", "tokio"]
                }
            }
        });
        let expanded = expand_query_with_table(&store, "Rust Ownership", &legacy);
        assert!(expanded.contains("borrow"), "{}", expanded);
        assert!(expanded.contains("lifetime"), "{}", expanded);
        // Legacy "cargo" must NOT also fire — table hit takes precedence.
        assert!(!expanded.contains("cargo"), "{}", expanded);
    }

    #[test]
    fn expand_query_with_table_falls_back_to_legacy() {
        let store = Store::open_memory().unwrap();
        let legacy = serde_json::json!({
            "search": {
                "learned_expansions": {
                    "rust": ["cargo", "tokio"]
                }
            }
        });
        let expanded = expand_query_with_table(&store, "rust async runtime", &legacy);
        assert!(expanded.contains("cargo") || expanded.contains("tokio"), "{}", expanded);
    }
}

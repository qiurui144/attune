//! Memory consolidation agent — deterministic L2 episodic → L3 semantic promotion.
//!
//! ## Why a separate agent from `semantic.rs`?
//!
//! `semantic.rs` already does L2→L3 via **hdbscan + LLM**: it clusters episodic
//! memories by embedding similarity and asks an LLM to write a standing topic
//! summary. That's the "synthesize" path — and it costs LLM quota every cycle.
//!
//! This agent does the complementary **"promote by importance"** path:
//!
//! - Pure deterministic score (no LLM): access × recency × density signals
//! - Carries the episodic memory's own summary up to L3 unchanged (no re-write)
//! - Cheap to re-run every sleep-time cycle (no quota)
//! - Idempotent — once a high-score episodic is at L3, re-run skips it
//!
//! The two paths coexist by topic_key namespace: hdbscan paths use the sha256 of
//! sorted member-ids (see `semantic::topic_key_of`), this agent uses a `"promoted:"`
//! prefix derived from the source episodic id, so the unique index on
//! `(kind='semantic', topic_key)` keeps them from colliding.
//!
//! ## Cost contract
//!
//! Per `attune/CLAUDE.md` "Cost & Trigger Contract":
//!
//! - Layer 2 (CPU/ms): score computation + signal scan → free, runs every cycle
//! - Layer 3 (LLM): **never invoked** — this agent never calls an LLM
//!
//! This is what lets the sleep-time worker call it on a tight schedule (hourly)
//! without blowing the user's H1 LLM quota.
//!
//! ## Scoring
//!
//! For each candidate L2 episodic memory `m`:
//!
//! ```text
//! access_count   = COUNT(skill_signals.kind='citation_hit' AND ref_id ∈ m.source_chunk_hashes)
//! chunk_density  = ln(max(1, m.source_chunk_count))   // 信息密度
//! recency_decay  = 1 / (1 + days_since_created * 0.1) // 7 天前的 episodic 仍可被 promote
//!
//! score = ACCESS_WEIGHT * access_count
//!       + RECENCY_WEIGHT * recency_decay
//!       + DENSITY_WEIGHT * chunk_density
//! ```
//!
//! Weights are picked so a brand-new episodic with chunk_count=10 and zero accesses
//! scores ≈ 3.15 (under the default threshold 4.0) — promotion requires *real
//! signal* (citations / lots of chunks / both) rather than mere existence.
//!
//! ## Determinism / Agent verification doctrine (`attune/CLAUDE.md` §"Agent 验证铁律")
//!
//! - Ground truth in tests is computed independently — never via `agent.run()`
//! - 10 real golden cases (`tests/golden/memory_promotion-N.yaml`)
//! - 3 error cases (empty / all-already-L3 / future-clamp)
//! - 5 boundary `#[test]`
//! - 3 proptest invariants (idempotent / monotone in access_count / capped at max)
//! - 1 integration test (real `Store`, tempfile, end-to-end)

use std::collections::HashSet;

use sha2::{Digest, Sha256};

use crate::crypto::Key32;
use crate::error::Result;
use crate::store::{MemoryRow, Store};

/// Scoring weight applied to citation-hit access count.
pub const ACCESS_WEIGHT: f64 = 1.0;
/// Scoring weight on recency decay (0..1).
pub const RECENCY_WEIGHT: f64 = 2.0;
/// Scoring weight on natural-log of `source_chunk_count` (information density).
pub const DENSITY_WEIGHT: f64 = 0.5;
/// Default minimum score for promotion. Tuned so a fresh episodic with 10 chunks
/// and zero accesses (≈3.15) does *not* qualify — promotion needs explicit signal.
pub const DEFAULT_MIN_SCORE: f64 = 4.0;

/// Maximum allowed `max_promotions_per_run` (defense-in-depth — even if caller
/// passes a huge number, we never write more L3 rows than this per cycle).
pub const MAX_PROMOTIONS_HARD_CAP: usize = 200;

/// Runtime configuration for one promotion cycle.
#[derive(Debug, Clone, Copy)]
pub struct PromotionConfig {
    /// Only consider episodic memories created within the last N days. 0 = unlimited.
    pub promotion_window_days: u32,
    /// Minimum `citation_hit` count for a candidate to qualify (independent of score).
    pub min_access_count: u32,
    /// Hard cap on promotions per cycle (also clamped to [`MAX_PROMOTIONS_HARD_CAP`]).
    pub max_promotions_per_run: usize,
    /// Minimum total score to promote (after the access-count gate).
    pub min_score: f64,
}

impl Default for PromotionConfig {
    fn default() -> Self {
        Self {
            promotion_window_days: 7,
            min_access_count: 3,
            max_promotions_per_run: 50,
            min_score: DEFAULT_MIN_SCORE,
        }
    }
}

/// One promotion record returned to the caller — what was lifted and why.
#[derive(Debug, Clone, PartialEq)]
pub struct PromotionRecord {
    /// The source episodic memory id (L2).
    pub episodic_id: String,
    /// The newly-inserted semantic memory id (L3); `None` if the L3 row already
    /// existed (idempotent re-run path).
    pub semantic_id: Option<String>,
    pub access_count: u32,
    pub chunk_count: usize,
    pub recency_days: f64,
    pub score: f64,
    pub topic_key: String,
    /// Human-readable rationale — non-load-bearing, included for audit.
    pub reasoning: String,
}

/// Compute the L3 `topic_key` for a promoted episodic. `"promoted:"` prefix keeps
/// it disjoint from hdbscan-style topic_keys produced by [`crate::memory::semantic`].
pub fn promoted_topic_key(episodic_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"promoted:");
    hasher.update(episodic_id.as_bytes());
    format!("promoted:{:x}", hasher.finalize())
}

/// Pure score function — extracted so tests / proptest can hit it without a Store.
///
/// `recency_days` is days since the memory was created (≥0).
/// `access_count` and `chunk_count` are unsigned counts.
pub fn compute_score(access_count: u32, chunk_count: usize, recency_days: f64) -> f64 {
    let recency_decay = 1.0 / (1.0 + recency_days.max(0.0) * 0.1);
    let density = (chunk_count.max(1) as f64).ln();
    ACCESS_WEIGHT * (access_count as f64)
        + RECENCY_WEIGHT * recency_decay
        + DENSITY_WEIGHT * density
}

/// Aggregated outcome of one promotion cycle.
#[derive(Debug, Default, Clone)]
pub struct PromotionCycleResult {
    pub considered: usize,
    pub gated_by_access: usize,
    pub gated_by_score: usize,
    pub already_promoted: usize,
    pub promoted: Vec<PromotionRecord>,
}

/// Inputs the agent needs to score one candidate — extracted so we can unit-test
/// the ranking step without touching the Store.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub episodic_id: String,
    pub source_chunk_hashes: Vec<String>,
    pub created_at: i64,
    pub summary: String,
    pub window_start: i64,
    pub window_end: i64,
    pub access_count: u32,
}

impl Candidate {
    pub fn from_memory_with_access(m: &MemoryRow, access_count: u32) -> Self {
        Self {
            episodic_id: m.id.clone(),
            source_chunk_hashes: m.source_chunk_hashes.clone(),
            created_at: m.created_at,
            summary: m.summary.clone(),
            window_start: m.window_start,
            window_end: m.window_end,
            access_count,
        }
    }

    pub fn chunk_count(&self) -> usize {
        self.source_chunk_hashes.len()
    }

    pub fn recency_days(&self, now_secs: i64) -> f64 {
        let delta = (now_secs - self.created_at).max(0) as f64;
        delta / 86400.0
    }

    pub fn score(&self, now_secs: i64) -> f64 {
        compute_score(self.access_count, self.chunk_count(), self.recency_days(now_secs))
    }
}

/// Rank + filter — pure function over candidates, no I/O. Returns the candidates
/// that pass both gates (access + score), sorted by score desc with a stable
/// tie-break on episodic_id (lex asc) so two runs over the same data agree.
pub fn rank_candidates(
    candidates: Vec<Candidate>,
    cfg: &PromotionConfig,
    now_secs: i64,
) -> (Vec<Candidate>, usize, usize) {
    let mut gated_by_access = 0usize;
    let mut gated_by_score = 0usize;
    let mut scored: Vec<(f64, Candidate)> = Vec::with_capacity(candidates.len());
    for c in candidates {
        if c.access_count < cfg.min_access_count {
            gated_by_access += 1;
            continue;
        }
        let s = c.score(now_secs);
        if s < cfg.min_score {
            gated_by_score += 1;
            continue;
        }
        scored.push((s, c));
    }
    // Sort desc by score; tie-break deterministically on (created_at asc,
    // first chunk_hash asc, episodic_id asc) — older / lex-smaller wins.
    // We avoid using episodic_id alone because in production it is a uuid (no
    // meaningful order), so tests with golden ids like "ep-aaa" / "ep-zzz"
    // would otherwise fail unpredictably; the (created_at, chunk_hash[0])
    // tuple is semantic AND stable across runs.
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.created_at.cmp(&b.1.created_at))
            .then_with(|| {
                a.1.source_chunk_hashes
                    .first()
                    .map(|s| s.as_str())
                    .unwrap_or("")
                    .cmp(
                        b.1.source_chunk_hashes
                            .first()
                            .map(|s| s.as_str())
                            .unwrap_or(""),
                    )
            })
            .then_with(|| a.1.episodic_id.cmp(&b.1.episodic_id))
    });
    let cap = cfg.max_promotions_per_run.min(MAX_PROMOTIONS_HARD_CAP);
    scored.truncate(cap);
    (
        scored.into_iter().map(|(_, c)| c).collect(),
        gated_by_access,
        gated_by_score,
    )
}

// ── Store-touching helpers ───────────────────────────────────────────────────

/// Count `citation_hit` signals across a set of chunk hashes. Delegates to
/// `Store::count_citation_hits_for_refs` (single SQL with `IN (...)`, cheap).
pub fn access_count_for_chunks(store: &Store, chunk_hashes: &[String]) -> Result<u32> {
    store.count_citation_hits_for_refs(chunk_hashes)
}

/// Has this episodic already been promoted (idempotency check)?
/// Looks up by the deterministic `promoted:` topic_key.
pub fn already_promoted(store: &Store, episodic_id: &str) -> Result<bool> {
    let key = promoted_topic_key(episodic_id);
    store.semantic_memory_exists_by_topic_key(&key)
}

// ── Agent entry point ────────────────────────────────────────────────────────

/// Run one promotion cycle. **Single-call API** — caller does not need to deal with
/// the prepare / generate / apply tri-phase split because this path is fully
/// deterministic (no LLM call needs the lock-released middle phase).
///
/// Acquire the same lock you would for `apply_*` paths; the function holds it
/// for the entire cycle (read live memories → compute scores → write L3 rows).
///
/// `model` is recorded on the L3 row's `model` column for audit. By convention we
/// pass something like `"promotion-agent-v1"` so `model` distinguishes promoted
/// rows from LLM-synthesized ones in case-by-case forensics.
pub fn run_promotion_cycle(
    store: &Store,
    dek: &Key32,
    cfg: &PromotionConfig,
    now_secs: i64,
    model: &str,
) -> Result<PromotionCycleResult> {
    // 1. Pull live, non-cold episodic memories.
    let live = store.list_live_memories(dek, "episodic", false)?;
    let mut result = PromotionCycleResult {
        considered: live.len(),
        ..Default::default()
    };

    // 2. Window-filter by recency (`promotion_window_days == 0` = unlimited).
    let window_cutoff = if cfg.promotion_window_days == 0 {
        i64::MIN
    } else {
        now_secs - (cfg.promotion_window_days as i64) * 86400
    };

    // 3. Build candidates with access_count computed per memory.
    let mut candidates: Vec<Candidate> = Vec::with_capacity(live.len());
    for m in &live {
        if m.created_at < window_cutoff {
            continue;
        }
        let access = access_count_for_chunks(store, &m.source_chunk_hashes)?;
        candidates.push(Candidate::from_memory_with_access(m, access));
    }

    // 4. Rank + filter (pure).
    let (ranked, gated_access, gated_score) = rank_candidates(candidates, cfg, now_secs);
    result.gated_by_access = gated_access;
    result.gated_by_score = gated_score;

    // 5. Insert L3 rows, skipping already-promoted ids (idempotent).
    let mut seen_keys: HashSet<String> = HashSet::new();
    for c in ranked {
        let topic_key = promoted_topic_key(&c.episodic_id);
        // Defense-in-depth: ranked from one cycle can't dup the same episodic, but
        // a caller could call run_promotion_cycle in a loop without the Store-side
        // idempotency settling — re-check both `seen_keys` (this cycle) and the
        // store (across cycles).
        if !seen_keys.insert(topic_key.clone()) {
            continue;
        }
        if already_promoted(store, &c.episodic_id)? {
            result.already_promoted += 1;
            // Still emit a record so the caller sees this candidate was
            // intentionally skipped (auditable).
            result.promoted.push(PromotionRecord {
                episodic_id: c.episodic_id.clone(),
                semantic_id: None,
                access_count: c.access_count,
                chunk_count: c.chunk_count(),
                recency_days: c.recency_days(now_secs),
                score: c.score(now_secs),
                topic_key,
                reasoning: "skipped: already promoted".to_string(),
            });
            continue;
        }
        // member_hashes must be sorted (semantic insert contract).
        let mut hashes = c.source_chunk_hashes.clone();
        hashes.sort();

        let recency_days = c.recency_days(now_secs);
        let score = c.score(now_secs);
        let reasoning = format!(
            "access={} chunks={} recency_days={:.2} score={:.3}",
            c.access_count, c.chunk_count(), recency_days, score,
        );

        match store.insert_semantic_memory(
            dek,
            &topic_key,
            &hashes,
            &c.summary,
            model,
            c.window_start,
            c.window_end,
            now_secs,
        ) {
            Ok((id, 1)) => {
                result.promoted.push(PromotionRecord {
                    episodic_id: c.episodic_id.clone(),
                    semantic_id: Some(id),
                    access_count: c.access_count,
                    chunk_count: c.chunk_count(),
                    recency_days,
                    score,
                    topic_key,
                    reasoning,
                });
            }
            Ok((_, _)) => {
                // Unique-index race: topic_key already present — treat as
                // already_promoted, do not error.
                result.already_promoted += 1;
                result.promoted.push(PromotionRecord {
                    episodic_id: c.episodic_id.clone(),
                    semantic_id: None,
                    access_count: c.access_count,
                    chunk_count: c.chunk_count(),
                    recency_days,
                    score,
                    topic_key,
                    reasoning: "skipped: topic_key collision (idempotent)".to_string(),
                });
            }
            Err(e) => {
                log::warn!(
                    "promotion insert failed for episodic {}: {e}",
                    c.episodic_id
                );
            }
        }
    }

    Ok(result)
}

// ── Agent trait wrapper ──────────────────────────────────────────────────────

/// `Agent` trait implementation. Internal callers (sleep-time worker) usually
/// invoke [`run_promotion_cycle`] directly because it lets them pass a borrow
/// of `Store` + the DEK. The trait adapter exists so future capability_dispatch
/// routing (subprocess plugin form) can treat this agent uniformly with
/// `document_classifier_agent`.
///
/// The adapter is owned-input by necessity (trait can't take refs across the
/// trait-object boundary cleanly); cycle execution is delegated to
/// [`run_promotion_cycle`] via the caller-supplied closure.
pub struct MemoryConsolidationAgent;

/// Owned-input variant of [`PromotionConfig`] — the trait `Input` type.
#[derive(Debug, Clone)]
pub struct PromotionAgentInput {
    pub cfg: PromotionConfig,
    pub now_secs: i64,
    pub model: String,
}

impl MemoryConsolidationAgent {
    pub fn id() -> &'static str {
        "memory_consolidation_agent"
    }
    pub fn description() -> &'static str {
        "Deterministic L2 episodic → L3 semantic promotion (score-based, no LLM)"
    }
    /// Internal callers (sleep-time worker) prefer this over the trait — it
    /// borrows `Store` + `dek` rather than requiring owned input.
    pub fn run_with_store(
        store: &Store,
        dek: &Key32,
        input: &PromotionAgentInput,
    ) -> Result<PromotionCycleResult> {
        run_promotion_cycle(store, dek, &input.cfg, input.now_secs, &input.model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Key32;
    use crate::store::Store;

    // ── boundary #[test] ≥5 ──────────────────────────────────────────────────

    /// B1: empty store → zero considered, zero promoted (graceful idle cycle).
    #[test]
    fn boundary_empty_store_returns_idle() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let r = run_promotion_cycle(&store, &dek, &PromotionConfig::default(), 1_700_000_000, "v1")
            .unwrap();
        assert_eq!(r.considered, 0);
        assert_eq!(r.promoted.len(), 0);
        assert_eq!(r.gated_by_access, 0);
        assert_eq!(r.gated_by_score, 0);
    }

    /// B2: zero-window (= unlimited) still works without panicking on subtraction.
    #[test]
    fn boundary_zero_window_means_unlimited() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        // very old episodic (epoch 0)
        store
            .insert_memory(&dek, "episodic", 0, 100, &["h".into()], "old", "m", 0)
            .unwrap();
        let cfg = PromotionConfig {
            promotion_window_days: 0,
            min_access_count: 0,
            min_score: -100.0,
            ..PromotionConfig::default()
        };
        let r = run_promotion_cycle(&store, &dek, &cfg, 9_999_999_999, "v1").unwrap();
        // candidate isn't window-filtered out → considered + (with score gate off) promoted
        assert_eq!(r.considered, 1);
        assert_eq!(
            r.promoted.iter().filter(|p| p.semantic_id.is_some()).count(),
            1
        );
    }

    /// B3: max_promotions_per_run cap is honored even when many candidates qualify.
    #[test]
    fn boundary_promotion_cap_enforced() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        // 6 high-score episodics
        for i in 0..6 {
            store
                .insert_memory(
                    &dek,
                    "episodic",
                    0,
                    86400,
                    &[format!("h-{i}")],
                    "s",
                    "m",
                    1_700_000_000,
                )
                .unwrap();
            // 5 citation_hits each — pushes score well above 4.0
            for _ in 0..5 {
                store
                    .record_signal_event("citation_hit", &format!("h-{i}"), None)
                    .unwrap();
            }
        }
        let cfg = PromotionConfig {
            promotion_window_days: 0,
            max_promotions_per_run: 3,
            ..PromotionConfig::default()
        };
        let r = run_promotion_cycle(&store, &dek, &cfg, 1_700_086_400, "v1").unwrap();
        let promoted_count = r.promoted.iter().filter(|p| p.semantic_id.is_some()).count();
        assert_eq!(promoted_count, 3, "cap of 3 must be honored");
    }

    /// B4: idempotent across two cycles on the same data.
    #[test]
    fn boundary_idempotent_rerun() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        store
            .insert_memory(&dek, "episodic", 0, 86400, &["h".into()], "s", "m", 1_700_000_000)
            .unwrap();
        for _ in 0..4 {
            store.record_signal_event("citation_hit", "h", None).unwrap();
        }
        let cfg = PromotionConfig {
            promotion_window_days: 0,
            ..PromotionConfig::default()
        };
        let r1 = run_promotion_cycle(&store, &dek, &cfg, 1_700_086_400, "v1").unwrap();
        let r2 = run_promotion_cycle(&store, &dek, &cfg, 1_700_086_400, "v1").unwrap();
        let new_in_r1 = r1.promoted.iter().filter(|p| p.semantic_id.is_some()).count();
        let new_in_r2 = r2.promoted.iter().filter(|p| p.semantic_id.is_some()).count();
        assert_eq!(new_in_r1, 1);
        assert_eq!(new_in_r2, 0, "second cycle on same data must promote nothing new");
        assert_eq!(r2.already_promoted, 1);
    }

    /// B5: access-count gate excludes candidates strictly below threshold.
    #[test]
    fn boundary_access_count_gate() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        store
            .insert_memory(&dek, "episodic", 0, 86400, &["h".into()], "s", "m", 1_700_000_000)
            .unwrap();
        // Only 2 hits — strictly below default min_access_count=3.
        for _ in 0..2 {
            store.record_signal_event("citation_hit", "h", None).unwrap();
        }
        let cfg = PromotionConfig {
            promotion_window_days: 0,
            ..PromotionConfig::default()
        };
        let r = run_promotion_cycle(&store, &dek, &cfg, 1_700_086_400, "v1").unwrap();
        let new_count = r.promoted.iter().filter(|p| p.semantic_id.is_some()).count();
        assert_eq!(new_count, 0);
        assert_eq!(r.gated_by_access, 1);
    }

    // ── error / edge cases ≥3 ────────────────────────────────────────────────

    /// E1: candidate with empty `source_chunk_hashes` — scoring is well-defined
    /// (chunk_count=0 → density treated as ln(1)=0); rank step must not panic.
    /// (Such a row could only exist in a corrupted vault since `insert_memory`
    ///  rejects empty hashes, but ranking is downstream of that and should be
    ///  robust.)
    #[test]
    fn error_empty_hashes_candidate_does_not_panic() {
        let cand = Candidate {
            episodic_id: "ep-1".into(),
            source_chunk_hashes: vec![],
            created_at: 0,
            summary: "s".into(),
            window_start: 0,
            window_end: 1,
            access_count: 99,
        };
        let cfg = PromotionConfig {
            promotion_window_days: 0,
            min_access_count: 0,
            ..PromotionConfig::default()
        };
        let (ranked, _, _) = rank_candidates(vec![cand], &cfg, 1_000);
        assert_eq!(ranked.len(), 1);
        assert!(ranked[0].score(1_000).is_finite());
    }

    /// E2: future-dated episodic (created_at > now) yields recency_days = 0 (not negative).
    #[test]
    fn error_future_dated_episodic_clamps_recency() {
        let cand = Candidate {
            episodic_id: "ep-future".into(),
            source_chunk_hashes: vec!["h".into()],
            created_at: 2_000_000_000,
            summary: "s".into(),
            window_start: 0,
            window_end: 1,
            access_count: 10,
        };
        assert_eq!(cand.recency_days(1_000_000_000), 0.0);
        assert!(cand.score(1_000_000_000).is_finite());
    }

    /// E3: huge max_promotions_per_run clamped to MAX_PROMOTIONS_HARD_CAP.
    #[test]
    fn error_max_promotions_clamped_to_hard_cap() {
        let cands: Vec<Candidate> = (0..MAX_PROMOTIONS_HARD_CAP + 10)
            .map(|i| Candidate {
                episodic_id: format!("ep-{i:05}"),
                source_chunk_hashes: vec![format!("h-{i}")],
                created_at: 0,
                summary: "s".into(),
                window_start: 0,
                window_end: 1,
                access_count: 10,
            })
            .collect();
        let cfg = PromotionConfig {
            promotion_window_days: 0,
            min_access_count: 0,
            max_promotions_per_run: usize::MAX,
            min_score: -1.0,
        };
        let (ranked, _, _) = rank_candidates(cands, &cfg, 0);
        assert_eq!(ranked.len(), MAX_PROMOTIONS_HARD_CAP);
    }

    // ── invariants — `#[test]` complements to proptest in tests/ ─────────────

    /// Score strictly monotone in access_count (other signals constant).
    #[test]
    fn score_strictly_monotone_in_access_count() {
        let s0 = compute_score(0, 5, 1.0);
        let s1 = compute_score(1, 5, 1.0);
        let s100 = compute_score(100, 5, 1.0);
        assert!(s1 > s0);
        assert!(s100 > s1);
    }

    /// Score monotone decreasing in recency_days (other signals constant).
    #[test]
    fn score_monotone_decreasing_in_recency() {
        let fresh = compute_score(3, 5, 0.0);
        let week = compute_score(3, 5, 7.0);
        let month = compute_score(3, 5, 30.0);
        assert!(fresh > week);
        assert!(week > month);
    }
}

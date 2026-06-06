//! Token bill + savings computation (spec §5.2 / §8.4).
//!
//! The bill makes the token-savings **visible**: it carries the naive baseline (full text
//! into the reasoning model in one shot) alongside the actual billable map+reduce tokens,
//! so the UI can render a naive-vs-actual bar and the §9.1 measurement harness can assert a
//! distribution. Savings is reported primarily **by token** (model-agnostic, immune to
//! pricing drift, spec §8.5) and secondarily by USD (cheap/reasoning split makes USD savings
//! even larger).
//!
//! SECURITY (G1 panel flag, spec §11 / CLAUDE.md §1.4): every field here is a **count or a
//! USD amount** — there is NO field holding an api_key / gateway token / credential. The word
//! "token" throughout means *count*, never *secret*. T-08 asserts the sentinel gateway token
//! never appears in a serialized bill.

use crate::cost;
use crate::usage::TokenUsage;
use serde::{Deserialize, Serialize};

/// Per-model token leg (map uses Cheap, reduce uses Reasoning).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelLeg {
    /// Billable input tokens for this leg.
    pub r#in: u32,
    /// Billable output tokens for this leg.
    pub out: u32,
    /// Logical model name this leg was routed to (count metadata, NOT a secret).
    pub model: String,
}

impl ModelLeg {
    /// Accumulate a vendor usage report into this leg. The leg's `model` is set from the
    /// usage report's model (first non-empty wins; subsequent calls must match).
    pub fn add(&mut self, u: &TokenUsage) {
        self.r#in = self.r#in.saturating_add(u.tokens_in);
        self.out = self.out.saturating_add(u.tokens_out);
        if self.model.is_empty() {
            self.model = u.model.clone();
        }
    }

    /// USD for this leg via per-model pricing (`None` if model not priced).
    pub fn usd(&self) -> Option<f64> {
        cost::estimate_cost_usd(self.r#in as usize, self.out as usize, &self.model)
    }
}

/// The token bill for one deep-summary / compare / chapter operation (spec §5.2).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TokenBill {
    /// Naive baseline: full text → reasoning model in one shot (`cost::estimate_tokens`).
    pub naive_baseline_tokens: u32,
    /// Tokens kept after the local extractive pre-cut (stage 1; 0 LLM cost).
    pub extractive_kept_tokens: u32,
    /// Map leg (cheap LLM, bulk block compression).
    pub map_llm_tokens: ModelLeg,
    /// Reduce leg (reasoning LLM, final synthesis ×1).
    pub reduce_llm_tokens: ModelLeg,
    /// Vendor prompt-cache read tokens (cheap; counted, billed at cache rate).
    pub cache_read_tokens: u32,
    /// Number of chunks served from the chunk_summaries cache (0 new LLM tokens).
    pub cache_hit_chunks: u32,
    /// Number of chunks that required a new map LLM call.
    pub new_chunks: u32,
    /// Reasoning model name used for the naive baseline estimate (for USD).
    pub baseline_model: String,
    /// Which pipeline path produced this bill (spec §3.2): `"map-reduce"` (multi-stage) or
    /// `"single-call"` (short-doc STAGE -1 bypass, spec §9.1 / §11 R2). Empty default keeps the
    /// serialized shape additive for old clients (back-compat, spec §10). NOT a secret.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
}

impl TokenBill {
    /// Total **actual billable** LLM tokens = map(in+out) + reduce(in+out).
    /// Cache-read tokens are excluded from the "billable" total used for the headline
    /// savings ratio (they are near-free); they are reported separately for observability.
    pub fn actual_billable_tokens(&self) -> u32 {
        self.map_llm_tokens
            .r#in
            .saturating_add(self.map_llm_tokens.out)
            .saturating_add(self.reduce_llm_tokens.r#in)
            .saturating_add(self.reduce_llm_tokens.out)
    }

    /// Headline savings by **token count** (spec §8.5 primary metric):
    /// `1 − actual_billable / naive_baseline`. Returns 1.0 when there is no actual billable
    /// LLM cost (100% cache hit / all-short). Returns 0.0 when there is no baseline (empty).
    pub fn savings_ratio_by_token(&self) -> f64 {
        if self.naive_baseline_tokens == 0 {
            return 0.0;
        }
        let actual = self.actual_billable_tokens() as f64;
        let naive = self.naive_baseline_tokens as f64;
        (1.0 - actual / naive).clamp(0.0, 1.0)
    }

    /// Savings by **USD**: `1 − actual_usd / naive_usd`. `None` if either side cannot be
    /// priced. Larger than the token ratio because map runs on the cheap model.
    pub fn savings_ratio_by_usd(&self) -> Option<f64> {
        let naive_usd = cost::estimate_cost_usd(
            self.naive_baseline_tokens as usize,
            0,
            &self.baseline_model,
        )?;
        if naive_usd <= 0.0 {
            return None;
        }
        let map_usd = self.map_llm_tokens.usd().unwrap_or(0.0);
        let reduce_usd = self.reduce_llm_tokens.usd().unwrap_or(0.0);
        let actual_usd = map_usd + reduce_usd;
        Some((1.0 - actual_usd / naive_usd).clamp(0.0, 1.0))
    }

    /// Actual billable USD (map + reduce).
    pub fn actual_billable_usd(&self) -> Option<f64> {
        let m = self.map_llm_tokens.usd()?;
        let r = self.reduce_llm_tokens.usd()?;
        Some(m + r)
    }

    /// Naive billable USD (full text → reasoning model).
    pub fn naive_billable_usd(&self) -> Option<f64> {
        cost::estimate_cost_usd(self.naive_baseline_tokens as usize, 0, &self.baseline_model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::TokenUsage;

    fn usage(model: &str, t_in: u32, t_out: u32) -> TokenUsage {
        TokenUsage {
            tokens_in: t_in,
            tokens_out: t_out,
            cached_in: 0,
            model: model.to_string(),
            provider: "mock".into(),
        }
    }

    #[test]
    fn test_savings_ratio_token_math() {
        let mut bill = TokenBill {
            naive_baseline_tokens: 100_000,
            baseline_model: "gpt-4o".into(),
            ..Default::default()
        };
        bill.map_llm_tokens.add(&usage("gpt-4o-mini", 30_000, 5_000));
        bill.reduce_llm_tokens.add(&usage("gpt-4o", 8_000, 2_000));
        // actual = 30k+5k+8k+2k = 45k ; ratio = 1 - 45k/100k = 0.55
        assert_eq!(bill.actual_billable_tokens(), 45_000);
        assert!((bill.savings_ratio_by_token() - 0.55).abs() < 1e-9);
    }

    #[test]
    fn test_usd_uses_lookup_pricing() {
        let mut bill = TokenBill {
            naive_baseline_tokens: 100_000,
            baseline_model: "gpt-4o".into(),
            ..Default::default()
        };
        bill.map_llm_tokens.add(&usage("gpt-4o-mini", 40_000, 10_000));
        bill.reduce_llm_tokens.add(&usage("gpt-4o", 10_000, 2_000));
        // map USD/token must be cheaper than reduce USD/token (cheap vs reasoning split).
        let map_usd = bill.map_llm_tokens.usd().unwrap();
        let reduce_usd = bill.reduce_llm_tokens.usd().unwrap();
        let map_per_tok = map_usd / (bill.map_llm_tokens.r#in + bill.map_llm_tokens.out) as f64;
        let reduce_per_tok =
            reduce_usd / (bill.reduce_llm_tokens.r#in + bill.reduce_llm_tokens.out) as f64;
        assert!(
            map_per_tok < reduce_per_tok,
            "cheap map USD/tok {map_per_tok} must be < reasoning reduce USD/tok {reduce_per_tok}"
        );
        // USD savings should exceed token savings (cheap model leverage).
        let token_savings = bill.savings_ratio_by_token();
        let usd_savings = bill.savings_ratio_by_usd().unwrap();
        assert!(
            usd_savings > token_savings,
            "USD savings {usd_savings} should exceed token savings {token_savings}"
        );
    }

    #[test]
    fn test_cache_read_counted_separately() {
        let mut bill = TokenBill {
            naive_baseline_tokens: 50_000,
            baseline_model: "gpt-4o".into(),
            cache_read_tokens: 12_000,
            cache_hit_chunks: 8,
            new_chunks: 4,
            ..Default::default()
        };
        bill.map_llm_tokens.add(&usage("gpt-4o-mini", 5_000, 1_000));
        // cache_read is reported but NOT in the billable total used for headline ratio
        assert_eq!(bill.actual_billable_tokens(), 6_000);
        assert_eq!(bill.cache_read_tokens, 12_000);
        assert_eq!(bill.cache_hit_chunks, 8);
    }

    #[test]
    fn test_zero_actual_gives_ratio_one() {
        // 100% cache hit / all-short → no actual billable LLM tokens → savings ~1.0
        let bill = TokenBill {
            naive_baseline_tokens: 80_000,
            baseline_model: "gpt-4o".into(),
            cache_hit_chunks: 40,
            new_chunks: 0,
            ..Default::default()
        };
        assert_eq!(bill.actual_billable_tokens(), 0);
        assert!((bill.savings_ratio_by_token() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_zero_baseline_gives_ratio_zero() {
        let bill = TokenBill::default();
        assert_eq!(bill.savings_ratio_by_token(), 0.0);
    }

    #[test]
    fn test_no_secret_field_only_counts() {
        // Structural guard for the G1 security flag: serialize a bill and assert the JSON
        // contains only count/usd/model fields, never an api_key-shaped field.
        let mut bill = TokenBill {
            naive_baseline_tokens: 1000,
            baseline_model: "gpt-4o".into(),
            ..Default::default()
        };
        bill.map_llm_tokens.add(&usage("gpt-4o-mini", 100, 20));
        let json = serde_json::to_string(&bill).unwrap();
        assert!(!json.contains("apiKey"));
        assert!(!json.contains("api_key"));
        assert!(!json.contains("secret"));
        assert!(!json.contains("Bearer"));
        // sentinel gateway token (T-08 uses this) must not be representable here
        assert!(!json.contains("test-gateway-token-not-real"));
    }
}

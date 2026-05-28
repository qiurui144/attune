//! Standard usage types — every LLM / Embed / Rerank / OCR / ASR / VLM call
//! records a [`UsageEvent`].
//!
//! Spec: `docs/superpowers/specs/2026-05-28-cache-context-token-standard-api.md` §5.1
//!
//! Wire format: serde camelCase for UI / REST surface; SQLite column names are
//! snake_case in the DB layer (see `attune-core::store::usage`).

use serde::{Deserialize, Serialize};

/// Per-call vendor usage report. Every LLM / Embed / Rerank / OCR / ASR / VLM
/// provider returns one of these alongside its primary payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    /// Prompt / input tokens billed by the vendor.
    pub tokens_in: u32,
    /// Completion / output tokens returned by the vendor.
    pub tokens_out: u32,
    /// Vendor-side prompt cache hits (Anthropic prompt-cache /
    /// OpenAI prompt-cache). 0 when the provider does not report this.
    pub cached_in: u32,
    /// Concrete model identifier (e.g. `"gemini-1.5-flash"`, `"qwen2.5:3b"`).
    pub model: String,
    /// Provider identifier — one of `ollama` / `openai` / `gemini` /
    /// `cloud_gateway` / `k3_local` / `mock` / vendor-specific names.
    pub provider: String,
}

impl TokenUsage {
    /// Empty placeholder — useful when a provider does not report usage and
    /// the recorder needs to fall back to heuristic estimation.
    pub fn empty(provider: &str, model: &str) -> Self {
        Self {
            tokens_in: 0,
            tokens_out: 0,
            cached_in: 0,
            provider: provider.into(),
            model: model.into(),
        }
    }
}

/// L1 / L2 cache lookup outcome. Vendor-side prompt cache is reported
/// separately via [`TokenUsage::cached_in`] and is not represented here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheOutcome {
    /// L1 (memory LRU) or L2 (SQLite encrypted) hit.
    Hit,
    /// Cache miss — upstream call performed.
    Miss,
    /// Cache deliberately skipped (user disabled, nocache hint, vault locked).
    Bypass,
}

/// Failure classification for [`CallOutcome::Fail`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// JSON / structured-output parse failure.
    Parse,
    /// Grounding check failed (LLM hallucinated / cited missing source).
    Grounding,
    /// Network or upstream timeout.
    Timeout,
    /// Quota / rate-limit exhaustion.
    Quota,
    /// Network transport error (DNS, TCP, TLS).
    Network,
    /// Schema validation failed against expected format.
    SchemaInvalid,
    /// Catch-all bucket.
    Other,
}

/// Disposition of one call attempt. Internally-tagged via the `kind` field so
/// the JSON shape is `{"kind":"ok"}` / `{"kind":"retry","attempt":2}` /
/// `{"kind":"fail","errorKind":"timeout"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CallOutcome {
    /// Call succeeded on the first attempt.
    Ok,
    /// Call succeeded after one or more retries.
    Retry {
        /// Zero-based retry attempt count (0 = succeeded on first retry).
        attempt: u8,
    },
    /// Call ultimately failed.
    #[serde(rename_all = "camelCase")]
    Fail {
        /// Failure classification.
        error_kind: ErrorKind,
    },
}

/// What kind of work the recorded event represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageKind {
    /// Free-form chat / completion turn.
    LlmChat,
    /// Structured-output extraction call.
    LlmExtract,
    /// Embedding vector generation.
    Embed,
    /// Cross-encoder reranking.
    Rerank,
    /// OCR (PP-OCRv5, K3, etc.).
    Ocr,
    /// ASR (whisper.cpp, K3).
    Asr,
    /// Vision-language model (image caption / VQA).
    Vlm,
}

/// One record persisted to the `usage_events` table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageEvent {
    /// Unix epoch milliseconds.
    pub ts_ms: i64,
    /// Kind of work performed.
    pub kind: UsageKind,
    /// Vendor usage report. Flattened into the JSON wire format so consumers
    /// see `{ "tokensIn": ..., "tokensOut": ..., "cachedIn": ..., "model": ...,
    /// "provider": ... }` at the top level of the event.
    #[serde(flatten)]
    pub usage: TokenUsage,
    /// Estimated USD cost; `None` when pricing is unknown.
    pub cost_usd: Option<f64>,
    /// L1 / L2 cache outcome.
    pub cache: CacheOutcome,
    /// Disposition of this call attempt.
    pub outcome: CallOutcome,
    /// End-to-end latency in milliseconds.
    pub latency_ms: u32,
    /// Calling agent id (`None` = direct chat / non-agent path).
    pub agent_id: Option<String>,
    /// BLAKE3 16-hex prefix of the user query. `None` unless
    /// `settings.usage.log_queries = true`.
    pub query_hash: Option<String>,
}

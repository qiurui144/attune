//! ACP-3 — Agent×Model Failure Telemetry (§4.5-F, implemented from zero).
//!
//! Per spec `docs/superpowers/specs/2026-05-29-ai-agents-governance-orchestration.md`
//! §3 (ACP-3 data flow) + §5.2 (AgentCallRecord) + §7 (telemetry write never blocks)
//! + §9 (test matrix).
//!
//! ## Why this exists (B audit `2026-05-29-quality-gating-telemetry-audit.md`)
//!
//! The global CLAUDE.md §4.5-F mandate — "per (agent × model) failure rate; > 30%
//! → UI 提示切高 tier" — had **zero implementation anywhere in the codebase**.
//! (`telemetry.rs` is the v1.0.6 *privacy* event log — a different concern; this
//! module deliberately does not touch or reuse its name/types.)
//!
//! This module is the agent-reliability observability layer. It does NOT add a new
//! table: it records onto the existing `usage_events` table (A1 / ACP-1 cache+usage
//! island), which already carries `agent_id`, `model`, `outcome`, `error_kind`,
//! `retry`, `latency_ms`, and token columns. A telemetry-shaped [`AgentCallRecord`]
//! maps onto that row, and the failure-rate roll-up is a query over it.
//!
//! Per §7 the write path is best-effort: a failed telemetry insert is logged and
//! swallowed (`let _ = ...`) — it must never block the agent's main result.
//!
//! This module is **observe-only** (§2.3): it never changes an agent's
//! computation. ACP-4's `FeedbackController` (auto escalate / disable) consumes
//! these rates in a later slice; here we only record + roll up + classify.

use serde::{Deserialize, Serialize};

use crate::usage::types::{CallOutcome, ErrorKind, TokenUsage, UsageEvent, UsageKind};

/// Telemetry-shaped disposition of one agent call (spec §5.2). This is the
/// agent-reliability view; it maps onto the persisted [`CallOutcome`] +
/// [`ErrorKind`] for storage (see [`AgentCallRecord::to_usage_event`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentOutcome {
    /// Call succeeded (possibly after retries).
    Ok,
    /// Structured-output / JSON parse failure.
    ParseErr,
    /// Grounding check failed (hallucination / missing-source citation).
    GroundingErr,
    /// Network / upstream timeout.
    Timeout,
    /// Quota / rate-limit exhaustion.
    RateLimit,
}

impl AgentOutcome {
    /// Is this a failure (anything other than [`AgentOutcome::Ok`])?
    pub fn is_failure(self) -> bool {
        !matches!(self, AgentOutcome::Ok)
    }

    /// Map onto the persisted (`CallOutcome`, `Option<ErrorKind>`) pair used by
    /// the `usage_events` table.
    fn to_call_outcome(self, retry_count: u8) -> CallOutcome {
        match self {
            AgentOutcome::Ok => {
                if retry_count == 0 {
                    CallOutcome::Ok
                } else {
                    CallOutcome::Retry { attempt: retry_count }
                }
            }
            AgentOutcome::ParseErr => CallOutcome::Fail {
                error_kind: ErrorKind::Parse,
            },
            AgentOutcome::GroundingErr => CallOutcome::Fail {
                error_kind: ErrorKind::Grounding,
            },
            AgentOutcome::Timeout => CallOutcome::Fail {
                error_kind: ErrorKind::Timeout,
            },
            AgentOutcome::RateLimit => CallOutcome::Fail {
                error_kind: ErrorKind::Quota,
            },
        }
    }

    /// Reconstruct the telemetry view from a persisted (`CallOutcome`) row. Used
    /// by the failure-rate roll-up reading back from `usage_events`.
    pub fn from_call_outcome(outcome: CallOutcome) -> Self {
        match outcome {
            CallOutcome::Ok | CallOutcome::Retry { .. } => AgentOutcome::Ok,
            CallOutcome::Fail { error_kind } => match error_kind {
                ErrorKind::Parse | ErrorKind::SchemaInvalid => AgentOutcome::ParseErr,
                ErrorKind::Grounding => AgentOutcome::GroundingErr,
                ErrorKind::Timeout | ErrorKind::Network => AgentOutcome::Timeout,
                ErrorKind::Quota => AgentOutcome::RateLimit,
                // Catch-all failures classify as parse (most common JSON failure).
                ErrorKind::Other => AgentOutcome::ParseErr,
            },
        }
    }
}

/// One agent call record (spec §5.2). Telemetry-shaped — converted to a
/// [`UsageEvent`] for persistence onto the `usage_events` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCallRecord {
    /// The agent that made the call (must be a registered agent id; the call
    /// site enforces this via the ACP-1 registry shadow-agent guard).
    pub agent_id: String,
    /// Concrete model used (`"qwen2.5:3b"` / `"gpt-4o-mini"` / ...).
    pub model: String,
    /// Telemetry disposition.
    pub outcome: AgentOutcome,
    /// Retry attempts before this disposition (0 = first try).
    pub retry_count: u8,
    /// End-to-end latency in milliseconds.
    pub latency_ms: u32,
    /// Vendor token usage (reuses the A1 [`TokenUsage`]).
    pub tokens: TokenUsage,
}

impl AgentCallRecord {
    /// Convert to a persistable [`UsageEvent`]. The provider/model live inside
    /// `tokens`; `agent_id` tags the row; `outcome` + `retry_count` fold into
    /// the persisted `CallOutcome`.
    pub fn to_usage_event(&self, ts_ms: i64) -> UsageEvent {
        UsageEvent {
            ts_ms,
            kind: UsageKind::LlmExtract,
            usage: self.tokens.clone(),
            cost_usd: None,
            cache: crate::usage::types::CacheOutcome::Miss,
            outcome: self.outcome.to_call_outcome(self.retry_count),
            latency_ms: self.latency_ms,
            agent_id: Some(self.agent_id.clone()),
            query_hash: None,
        }
    }
}

/// Per-(agent × model) failure-rate roll-up over a time window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentModelHealth {
    /// Agent id.
    pub agent_id: String,
    /// Model identifier.
    pub model: String,
    /// Total calls recorded for this (agent, model).
    pub total_calls: u64,
    /// Calls that failed (any non-Ok outcome).
    pub failures: u64,
    /// `failures / total_calls`, `0.0` when `total_calls == 0`.
    pub failure_rate: f64,
}

/// The §4.5-F red line: a per-(agent × model) failure rate strictly above this
/// fraction triggers the "switch to a higher tier" UI hint.
pub const FAILURE_RATE_ALERT_THRESHOLD: f64 = 0.30;

impl AgentModelHealth {
    /// Construct from raw counts. `failure_rate` is `0.0` for zero calls (never
    /// divides by zero — §9 boundary).
    pub fn new(agent_id: String, model: String, total_calls: u64, failures: u64) -> Self {
        let failure_rate = if total_calls > 0 {
            failures as f64 / total_calls as f64
        } else {
            0.0
        };
        Self {
            agent_id,
            model,
            total_calls,
            failures,
            failure_rate,
        }
    }

    /// True when the failure rate is strictly above the §4.5-F alert threshold —
    /// the UI should suggest switching to a higher model tier.
    pub fn should_suggest_higher_tier(&self) -> bool {
        self.failure_rate > FAILURE_RATE_ALERT_THRESHOLD
    }
}

/// Render the `attune agent health` dashboard (§5.5) from a per-(agent × model)
/// roll-up. Worst-first (the store query already orders by failure_rate desc);
/// rows above the §4.5-F threshold are flagged with a "⚠ switch to higher tier"
/// hint. Empty input yields a friendly "no agent calls recorded" line.
pub fn render_health(rows: &[AgentModelHealth]) -> String {
    let mut out = String::new();
    out.push_str("Agent×Model Health (failure telemetry, §4.5-F)\n");
    out.push_str(&"=".repeat(60));
    out.push('\n');
    if rows.is_empty() {
        out.push_str("  (no agent calls recorded in this window)\n");
        return out;
    }
    out.push_str(&format!(
        "  {:<24} {:<14} {:>6} {:>6} {:>8}\n",
        "agent", "model", "calls", "fail", "rate"
    ));
    for h in rows {
        let flag = if h.should_suggest_higher_tier() {
            "  ⚠ switch to higher tier"
        } else {
            ""
        };
        out.push_str(&format!(
            "  {:<24} {:<14} {:>6} {:>6} {:>7.1}%{}\n",
            h.agent_id,
            h.model,
            h.total_calls,
            h.failures,
            h.failure_rate * 100.0,
            flag,
        ));
    }
    out
}

#[cfg(test)]
mod tests;

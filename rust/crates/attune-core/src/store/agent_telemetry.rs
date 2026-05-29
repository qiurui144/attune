//! ACP-3 — agent×model failure-telemetry store extension (§4.5-F).
//!
//! Spec: `docs/superpowers/specs/2026-05-29-ai-agents-governance-orchestration.md`
//! §3 (ACP-3) + §5.2 (AgentCallRecord) + §7 (write never blocks).
//!
//! Records an [`AgentCallRecord`] onto the existing `usage_events` table (no new
//! table — reuses the A1 / ACP-1 usage island), and rolls up per-(agent × model)
//! failure rate. `failures` = rows whose persisted `outcome = 'fail'` (an
//! ultimately-failed call); `retry`/`ok` rows count toward total but not failures.

use crate::agent_telemetry::{AgentCallRecord, AgentModelHealth};
use crate::error::Result;
use crate::store::Store;

impl Store {
    /// Record one agent call (ACP-3). Persists onto `usage_events` with the
    /// `agent_id` tag. Returns the insert result; callers on the hot path should
    /// prefer [`Store::record_agent_call_best_effort`] (§7 never-block).
    pub fn record_agent_call(&self, rec: &AgentCallRecord, ts_ms: i64) -> Result<()> {
        let event = rec.to_usage_event(ts_ms);
        self.record_usage(&event)
    }

    /// Best-effort record (§7): a telemetry write failure is logged and
    /// swallowed — it must never block the agent's main result. `ts_ms` is
    /// supplied by the caller (the governor passes its own measured timestamp).
    pub fn record_agent_call_best_effort(&self, rec: &AgentCallRecord, ts_ms: i64) {
        if let Err(e) = self.record_agent_call(rec, ts_ms) {
            log::warn!(
                "agent telemetry record dropped (agent={} model={}): {e}",
                rec.agent_id,
                rec.model
            );
        }
    }

    /// Roll up per-(agent × model) failure rate over the inclusive time window
    /// `[from_ms, to_ms]`. Only agent-tagged rows (`agent_id IS NOT NULL`) are
    /// counted — direct-chat usage is excluded. Rows are ordered by failure_rate
    /// desc then agent_id for a stable, worst-first dashboard.
    pub fn agent_model_health(&self, from_ms: i64, to_ms: i64) -> Result<Vec<AgentModelHealth>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT agent_id, model,
                    count(*) AS total,
                    coalesce(sum(CASE outcome WHEN 'fail' THEN 1 ELSE 0 END), 0) AS failures
             FROM usage_events
             WHERE agent_id IS NOT NULL AND ts_ms BETWEEN ?1 AND ?2
             GROUP BY agent_id, model
             ORDER BY (CAST(coalesce(sum(CASE outcome WHEN 'fail' THEN 1 ELSE 0 END),0) AS REAL)
                       / count(*)) DESC, agent_id ASC",
        )?;
        let rows = stmt
            .query_map([from_ms, to_ms], |r| {
                let agent_id: String = r.get(0)?;
                let model: String = r.get(1)?;
                let total: i64 = r.get(2)?;
                let failures: i64 = r.get(3)?;
                Ok(AgentModelHealth::new(
                    agent_id,
                    model,
                    total.max(0) as u64,
                    failures.max(0) as u64,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

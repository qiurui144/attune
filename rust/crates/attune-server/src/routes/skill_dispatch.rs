//! Subprocess-backed [`AgentDispatcher`] — the server's implementation of the skill-runtime's
//! plugin-agent dispatch capability (CAP-4b). It lets a declarative skill chain a **pro plugin's
//! own agent** (law `legal_drafter`, patent OA-response, tech debt, medical de-id) as a step by
//! routing the agent id to that plugin's binary subprocess, reusing the exact mechanism the
//! `/agents/{id}/run` route already uses (`agent_runner::run_agent_subprocess`).
//!
//! ## Security boundary (spec §7 / CLAUDE.md §1.4 + Agent 验证铁律)
//! Every dispatch is gated, in order:
//! 1. **Agent must be declared by an installed plugin.** `list_agents()` only yields agents from
//!    plugins that passed the scan-time signature/trust gate (`scan_with_trust`); an id not in
//!    that set is rejected (`agent-not-found`) — no arbitrary binary can be invoked.
//! 2. **Entitlement gate (T10).** The owning plugin's license must be entitled to run
//!    (free/active/trial/paid-grace/degraded); trial-expired / revoked rejects. Same gate as the
//!    HTTP dispatch route, so a skill can't bypass licensing.
//! 3. **`library`-runtime agents are not directly dispatchable** (called internally by other
//!    agents) — rejected, mirroring the HTTP route.
//! 4. **Timeout + resource bound.** The subprocess is killed past [`AGENT_RUN_TIMEOUT`]; the
//!    plugin's `resources` (cpu/token caps) apply via the agent binary's own SDK; the skill-level
//!    token cap (`MAX_TOTAL_TOKENS`) additionally bounds the whole chain.
//! 5. **LLM env only.** Just the `LLM_*` vars are forwarded (never the full parent env), through
//!    the same `llm_env_from_settings` the HTTP route uses — so a pro agent reaches the configured
//!    cloud LLM (member gateway / BYOK) without env leakage.

use std::sync::Arc;
use std::time::Duration;

use attune_core::plugin_registry::PluginRegistry;
use attune_core::skill_runtime::{AgentDispatcher, DispatchOutput};
use serde_json::Value;

use crate::state::SharedState;

/// Per-agent subprocess timeout (parity with the `/agents/{id}/run` route). LLM deliverable
/// agents (legal_drafter, oa_response) take longer than the calculators, but a cloud LLM call
/// chain is still bounded well under this; the cap prevents a hung binary from stalling a worker.
const AGENT_RUN_TIMEOUT: Duration = Duration::from_secs(90);

/// A dispatcher that runs a plugin agent as a subprocess. Owns cloned, `Send`-safe handles so it
/// can be moved into a `spawn_blocking` closure alongside the (blocking) skill run.
pub struct SubprocessAgentDispatcher {
    registry: Arc<PluginRegistry>,
    /// Entitlement cache (T10 gate); shares the `Arc<RwLock>` inner state via cheap clone — no
    /// vault/network on dispatch, just an O(1) keyed lookup.
    entitlement_cache: attune_core::entitlement::EntitlementCache,
    /// The `LLM_*` env forwarded to each agent subprocess (resolved once from settings).
    llm_env: Vec<(String, String)>,
    /// Plugins root (`plugin-install` layout: `<root>/<plugin_id>`).
    plugins_root: std::path::PathBuf,
}

impl SubprocessAgentDispatcher {
    /// Build the dispatcher from server state. Resolves the LLM env from `app_settings` once
    /// (so the blocking skill run needs no further vault access). Returns `None` if the plugins
    /// root can't be resolved (no plugin agent can run then — the runner degrades gracefully).
    pub fn from_state(state: &SharedState) -> Option<Self> {
        let plugins_root = PluginRegistry::default_plugins_dir().ok()?;
        let llm_env = resolve_llm_env(state);
        Some(SubprocessAgentDispatcher {
            registry: state.plugin_registry.clone(),
            entitlement_cache: state.entitlement_cache.clone(),
            llm_env,
            plugins_root,
        })
    }

    /// Map an agent id to its owning plugin id + runtime, verifying it is a declared agent of an
    /// installed (trust-allowed) plugin. `None` ⇒ not a dispatchable plugin agent.
    fn resolve_owning_plugin(&self, agent_id: &str) -> Option<(String, String)> {
        self.registry
            .list_agents()
            .iter()
            .find(|(_, a)| a.id == agent_id)
            .map(|(pid, a)| (pid.to_string(), a.runtime.clone()))
    }
}

impl AgentDispatcher for SubprocessAgentDispatcher {
    fn dispatch(&self, agent_id: &str, input: &Value) -> Result<DispatchOutput, String> {
        if agent_id.len() > 128 {
            return Err("agent id too long".to_string());
        }
        // (1) declared by an installed plugin?
        let (plugin_id, runtime) = self
            .resolve_owning_plugin(agent_id)
            .ok_or_else(|| format!("agent '{agent_id}' not found in any installed plugin"))?;

        // (2) entitlement gate (T10) — trial-expired / revoked blocks the run.
        let now = chrono::Utc::now();
        if let attune_core::entitlement::EntitlementDecision::Reject(code) =
            self.entitlement_cache.is_entitled(&plugin_id, &now)
        {
            return Err(format!("plugin '{plugin_id}' not entitled: {code}"));
        }

        // (3) library agents are not directly dispatchable.
        if runtime == "library" {
            return Err(format!(
                "agent '{agent_id}' is runtime=library and is invoked internally, not dispatchable"
            ));
        }

        let plugin_dir = self.plugins_root.join(&plugin_id);
        let stdin_json =
            serde_json::to_string(input).map_err(|e| format!("serialize agent input: {e}"))?;

        // (4)/(5) subprocess with timeout + LLM-env-only forwarding.
        let result = attune_core::agent_runner::run_agent_subprocess(
            &self.registry,
            agent_id,
            &plugin_dir,
            &stdin_json,
            self.llm_env.clone(),
            AGENT_RUN_TIMEOUT,
        )
        .map_err(|e| format!("agent run: {e}"))?;

        if result.timed_out {
            return Err(format!("agent '{agent_id}' timed out (>{}s)", AGENT_RUN_TIMEOUT.as_secs()));
        }
        match result.exit_code {
            // 0 = success, 2 = business red line (still a valid envelope with red_lines_violated).
            0 | 2 => {
                let envelope: Value = serde_json::from_str(&result.stdout).map_err(|e| {
                    format!("agent '{agent_id}' stdout not JSON: {e}")
                })?;
                let llm_tokens = extract_llm_tokens(&envelope);
                Ok(DispatchOutput { envelope, llm_tokens })
            }
            3 => Err(format!("agent '{agent_id}' rejected input: {}", result.stderr.trim())),
            4 => Err(format!("agent '{agent_id}' has no LLM configured (exit 4)")),
            other => Err(format!("agent '{agent_id}' exit {other}: {}", result.stderr.trim())),
        }
    }
}

/// Read the agent's reported LLM token usage from its `AgentOutput` envelope. Pro agents put it
/// at `computation.cost_used.llm_tokens` (per the agent SDK `AgentOutput<T>` where `T` carries a
/// `cost_used`). Best-effort: 0 if absent.
fn extract_llm_tokens(envelope: &Value) -> u32 {
    envelope
        .get("computation")
        .and_then(|c| c.get("cost_used"))
        .and_then(|c| c.get("llm_tokens"))
        .and_then(|v| v.as_u64())
        .map(|n| n.min(u32::MAX as u64) as u32)
        .unwrap_or(0)
}

/// Resolve the `LLM_*` env from `app_settings` (same shape + bare-prefix names as the HTTP
/// `/agents/{id}/run` route's `llm_env_from_settings`).
fn resolve_llm_env(state: &SharedState) -> Vec<(String, String)> {
    let settings: Value = state
        .vault
        .lock()
        .ok()
        .and_then(|vault| {
            vault
                .store()
                .get_meta("app_settings")
                .ok()
                .flatten()
                .and_then(|data| serde_json::from_slice(&data).ok())
        })
        .unwrap_or_else(|| serde_json::json!({}));
    crate::routes::agents::llm_env_from_settings(&settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_llm_tokens_from_cost_used() {
        let env = json!({ "computation": { "cost_used": { "llm_tokens": 3500 } } });
        assert_eq!(extract_llm_tokens(&env), 3500);
    }

    #[test]
    fn extract_llm_tokens_absent_is_zero() {
        assert_eq!(extract_llm_tokens(&json!({ "computation": {} })), 0);
        assert_eq!(extract_llm_tokens(&json!({})), 0);
    }

    #[test]
    fn extract_llm_tokens_clamps_overflow() {
        let env = json!({ "computation": { "cost_used": { "llm_tokens": 9_999_999_999_u64 } } });
        assert_eq!(extract_llm_tokens(&env), u32::MAX);
    }
}

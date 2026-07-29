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
//! 2. **Entitlement gate (T10).** Pro/paid/trial plugins must have a local entitlement row and
//!    the owning plugin's license must be entitled to run (active/trial/paid-grace);
//!    degraded / trial-expired / revoked rejects. Same gate as the HTTP dispatch route, so a
//!    skill can't bypass licensing.
//! 3. **`library`-runtime agents are not directly dispatchable** (called internally by other
//!    agents) — rejected, mirroring the HTTP route.
//! 4. **Timeout + resource bound.** The subprocess is killed past [`AGENT_RUN_TIMEOUT`]; the
//!    plugin's `resources` (cpu/token caps) apply via the agent binary's own SDK; the skill-level
//!    token cap (`MAX_TOTAL_TOKENS`) additionally bounds the whole chain.
//! 5. **Local LLM env only.** The child starts with an empty environment and receives a complete
//!    `LLM_*` override only through the same local-destination policy as `/agents/{id}/run`.
//!    Cloud, named-host, malformed, and key-only settings fail closed before the subprocess.

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
    /// The `LLM_*` env forwarded to each agent subprocess (resolved once from settings). A policy
    /// error is retained so non-agent skills remain usable, but any attempted plugin dispatch is
    /// rejected before the child is spawned.
    llm_env: Result<Vec<(String, String)>, String>,
    /// Plugins root (`plugin-install` layout: `<root>/<plugin_id>`).
    plugins_root: std::path::PathBuf,
}

impl SubprocessAgentDispatcher {
    /// Build the dispatcher from server state. Resolves the LLM env from `app_settings` once
    /// (so the blocking skill run needs no further vault access). Returns `None` if the plugins
    /// root can't be resolved (no plugin agent can run then — the runner degrades gracefully).
    pub fn from_state(state: &SharedState) -> Option<Self> {
        let plugins_root = PluginRegistry::default_plugins_dir().ok()?;
        let llm_env = resolve_llm_env(state).map_err(|error| match error {
            crate::error::AppError::Detailed { body, .. } => body
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("agent-llm-policy-rejected")
                .to_string(),
            other => other.to_string(),
        });
        Some(SubprocessAgentDispatcher {
            registry: crate::routes::plugins::current_plugin_registry(state),
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

        // (2) entitlement gate (T10) — copied pro plugin dirs without entitlement rows are
        // rejected, and degraded / trial-expired / revoked blocks the run.
        if crate::routes::agents::plugin_requires_entitlement(&self.registry, &plugin_id) {
            let tier = self.entitlement_cache.tier(&plugin_id);
            if tier
                .as_deref()
                .map(|t| t.trim().eq_ignore_ascii_case("free"))
                .unwrap_or(true)
            {
                return Err(format!(
                    "plugin '{plugin_id}' not entitled: plugin-entitlement-required"
                ));
            }
        }
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

        // (4)/(5) subprocess with timeout + isolated, local-only LLM env. Resolve this before
        // touching the binary so a cloud/uncertain configuration cannot degrade into inherited
        // process credentials.
        let llm_env = self.llm_env.as_ref().map_err(Clone::clone)?.clone();
        let result = attune_core::agent_runner::run_agent_subprocess(
            &self.registry,
            agent_id,
            &plugin_dir,
            &stdin_json,
            llm_env,
            AGENT_RUN_TIMEOUT,
        )
        .map_err(|e| format!("agent run: {e}"))?;

        if result.timed_out {
            return Err(format!(
                "agent '{agent_id}' timed out (>{}s)",
                AGENT_RUN_TIMEOUT.as_secs()
            ));
        }
        match result.exit_code {
            // 0 = success, 2 = business red line (still a valid envelope with red_lines_violated).
            0 | 2 => {
                let envelope: Value = serde_json::from_str(&result.stdout)
                    .map_err(|e| format!("agent '{agent_id}' stdout not JSON: {e}"))?;
                let llm_tokens = extract_llm_tokens(&envelope);
                Ok(DispatchOutput {
                    envelope,
                    llm_tokens,
                })
            }
            3 => Err(format!(
                "agent '{agent_id}' rejected input: {}",
                result.stderr.trim()
            )),
            4 => Err(format!("agent '{agent_id}' has no LLM configured (exit 4)")),
            other => Err(format!(
                "agent '{agent_id}' exit {other}: {}",
                result.stderr.trim()
            )),
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

/// Resolve the exact same isolated, local-only environment used by the direct
/// HTTP agent route. Keeping this thin prevents skill chains from acquiring a
/// second, weaker outbound policy.
fn resolve_llm_env(state: &SharedState) -> crate::error::AppResult<Vec<(String, String)>> {
    crate::routes::agents::load_isolated_local_agent_llm_env(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state_with_settings(settings: Value) -> SharedState {
        let tmp = tempfile::tempdir().expect("tmp");
        let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("vault");
        vault.setup("P@ss-skill-agent-egress").expect("setup");
        crate::settings_store::persist_settings(&vault, settings).expect("persist settings");
        Arc::new(crate::state::AppState::new(vault, false))
    }

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

    #[test]
    fn skill_chain_cloud_llm_fails_closed_and_never_forwards_the_key() {
        let state = state_with_settings(json!({
            "llm": {
                "provider": "openai_compat",
                "endpoint": "https://api.openai.com/v1",
                "model": "gpt-test",
                "api_key": "sk-must-not-reach-plugin"
            }
        }));

        let error = resolve_llm_env(&state).expect_err("cloud skill agent must fail closed");
        match error {
            crate::error::AppError::Detailed { status, body } => {
                assert_eq!(status, axum::http::StatusCode::FORBIDDEN);
                assert_eq!(body["code"], "cloud-agent-proxy-required");
                assert!(
                    !body.to_string().contains("sk-must-not-reach-plugin"),
                    "policy errors must not echo the encrypted credential"
                );
            }
            other => panic!("expected cloud-agent policy error, got {other:?}"),
        }
    }

    #[test]
    fn skill_chain_compute_only_env_has_complete_empty_llm_overrides() {
        let state = state_with_settings(json!({}));
        let env = resolve_llm_env(&state).expect("compute-only skill agent env");
        for key in ["LLM_PROVIDER", "LLM_ENDPOINT", "LLM_MODEL", "LLM_API_KEY"] {
            assert_eq!(
                env.iter()
                    .find(|(name, _)| name == key)
                    .map(|(_, value)| value.as_str()),
                Some(""),
                "{key} must override any inherited process value"
            );
        }
    }

    #[test]
    fn skill_chain_loads_a_stored_key_only_for_a_proven_local_endpoint() {
        let state = state_with_settings(json!({
            "llm": {
                "provider": "openai_compat",
                "endpoint": "http://127.0.0.1:8090/v1",
                "model": "local-model",
                "api_key": "local-only-token"
            }
        }));
        let env = resolve_llm_env(&state).expect("local endpoint should be dispatchable");

        assert!(env
            .iter()
            .any(|(key, value)| key == "LLM_ENDPOINT" && value == "http://127.0.0.1:8090/v1"));
        assert!(env
            .iter()
            .any(|(key, value)| key == "LLM_API_KEY" && value == "local-only-token"));
    }

    #[test]
    fn skill_dispatch_policy_error_precedes_subprocess_resolution() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let plugin_dir = tmp.path().join("local-helper");
        std::fs::create_dir_all(&plugin_dir).expect("mkdir plugin");
        std::fs::write(
            plugin_dir.join("plugin.yaml"),
            "id: local-helper\nname: Local Helper\ntype: skill\nversion: \"1.0.0\"\npricing:\n  tier: free\nagents:\n  - id: chained_helper\n    runtime: rust_binary\n    binary: bin/must-not-run\n",
        )
        .expect("write plugin.yaml");
        let (registry, warnings) = PluginRegistry::scan(tmp.path()).expect("scan plugin");
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

        let dispatcher = SubprocessAgentDispatcher {
            registry: Arc::new(registry),
            entitlement_cache: attune_core::entitlement::EntitlementCache::new(),
            llm_env: Err("cloud-agent-proxy-required".to_string()),
            plugins_root: tmp.path().to_path_buf(),
        };
        let error = dispatcher
            .dispatch("chained_helper", &json!({"raw": "private"}))
            .expect_err("policy error must stop dispatch");

        assert_eq!(error, "cloud-agent-proxy-required");
        assert!(
            !error.contains("binary not found"),
            "the subprocess resolver must not run after a policy failure"
        );
    }

    #[test]
    fn dispatch_blocks_pro_plugin_without_entitlement_before_subprocess() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let plugin_dir = tmp.path().join("law-pro");
        std::fs::create_dir_all(&plugin_dir).expect("mkdir plugin");
        std::fs::write(
            plugin_dir.join("plugin.yaml"),
            "id: law-pro\nname: Law Pro\ntype: industry\nversion: \"1.0.0\"\nagents:\n  - id: legal_drafter\n    runtime: rust_binary\n    binary: bin/missing\n",
        )
        .expect("write plugin.yaml");
        let (registry, warnings) = PluginRegistry::scan(tmp.path()).expect("scan pro plugin");
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

        let dispatcher = SubprocessAgentDispatcher {
            registry: Arc::new(registry),
            entitlement_cache: attune_core::entitlement::EntitlementCache::new(),
            llm_env: Ok(Vec::new()),
            plugins_root: tmp.path().to_path_buf(),
        };
        let err = dispatcher
            .dispatch("legal_drafter", &json!({"facts": "x"}))
            .unwrap_err();

        assert!(err.contains("plugin-entitlement-required"));
    }
}

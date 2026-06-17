//! Per-stage vetted-model routing (spec §8.2).
//!
//! The routing DECISION is **client-side**: attune picks a logical model name per
//! operation via a [`ModelRole`] → model-name map, then calls the gateway with that
//! name (gateway only bills + forwards). Roles are sub-grouped by task family — NOT a
//! blanket "tier=most-expensive". The gateway/new-api group + GroupRatio + model_mapping
//! handle the upstream routing; attune only chooses the logical name.
//!
//! Falls back gracefully: a settings blob with no `model_routing` block maps every role
//! to the current `default_model` (degenerate-but-usable). BYOK/Ollama users get their
//! local model mapped to every role (weak-model degrade, spec §4.5.E / §11 R1).

use crate::error::{Result, VaultError};
use serde_json::Value;

/// The three model families a document-intelligence stage can route to.
///
/// - `Cheap`     — bulk map / per-block semantic verdict (gpt-4o-mini class). Largest token
///   volume → highest cost leverage; the task (extractive rewrite / 4-class verdict) is low
///   difficulty so a cheap model meets the quality floor.
/// - `Reasoning` — final reduce ×1 / diff summary ×1 / chapter Q&A (gpt-4o / sonnet class).
///   Higher reasoning need, but called few times so absolute cost stays bounded.
/// - `Vision`    — scanned / image documents (VLM; gpt-4o-mini-vision / qwen-vl class).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelRole {
    Cheap,
    Reasoning,
    Vision,
}

impl ModelRole {
    /// Stable settings key for this role inside the `model_routing` block.
    pub fn settings_key(self) -> &'static str {
        match self {
            ModelRole::Cheap => "cheap",
            ModelRole::Reasoning => "reasoning",
            ModelRole::Vision => "vision",
        }
    }

    /// All roles a fully-configured router must map (used by startup validation).
    pub fn all() -> [ModelRole; 3] {
        [ModelRole::Cheap, ModelRole::Reasoning, ModelRole::Vision]
    }
}

/// Resolves a [`ModelRole`] to a concrete logical model name.
///
/// Built from the vault `app_settings` JSON blob. Construction never fails; the explicit
/// [`Self::validate`] call is what surfaces a missing role (so startup can decide whether
/// to hard-fail or accept the degenerate fallback).
#[derive(Debug, Clone)]
pub struct ModelRouter {
    cheap: String,
    reasoning: String,
    vision: String,
    /// True when every role came from an explicit `model_routing` entry (not the fallback).
    fully_configured: bool,
}

impl ModelRouter {
    /// Build from the `app_settings` JSON value.
    ///
    /// Reads `model_routing.{cheap,reasoning,vision}`. Any role missing from that block
    /// falls back to `default_model` (resolved from `model_routing.default` → `llm.model`).
    /// A blob with no `model_routing` block AND no resolvable default yields an all-empty
    /// router whose [`Self::validate`] errors — callers should pass a real default.
    pub fn from_settings(settings: &Value) -> Self {
        let routing = settings.get("model_routing");
        let default_model = Self::resolve_default(settings);

        let pick = |role: ModelRole| -> (String, bool) {
            let explicit = routing
                .and_then(|r| r.get(role.settings_key()))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            match explicit {
                Some(m) => (m, true),
                None => (default_model.clone(), false),
            }
        };

        let (cheap, c_ok) = pick(ModelRole::Cheap);
        let (reasoning, r_ok) = pick(ModelRole::Reasoning);
        let (vision, v_ok) = pick(ModelRole::Vision);

        ModelRouter {
            cheap,
            reasoning,
            vision,
            fully_configured: c_ok && r_ok && v_ok,
        }
    }

    /// Degenerate router: every role maps to the same model (BYOK/Ollama fallback,
    /// or settings with no routing block). Always passes [`Self::validate`] when
    /// `model` is non-empty.
    pub fn all_same(model: &str) -> Self {
        ModelRouter {
            cheap: model.to_string(),
            reasoning: model.to_string(),
            vision: model.to_string(),
            fully_configured: false,
        }
    }

    fn resolve_default(settings: &Value) -> String {
        settings
            .get("model_routing")
            .and_then(|r| r.get("default"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                settings
                    .get("llm")
                    .and_then(|l| l.get("model"))
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or("")
            .to_string()
    }

    /// Pick the logical model name for a stage's role.
    pub fn pick(&self, role: ModelRole) -> &str {
        match role {
            ModelRole::Cheap => &self.cheap,
            ModelRole::Reasoning => &self.reasoning,
            ModelRole::Vision => &self.vision,
        }
    }

    /// True iff every role came from an explicit `model_routing` entry.
    pub fn is_fully_configured(&self) -> bool {
        self.fully_configured
    }

    /// Startup validation: every role must resolve to a non-empty model name, else
    /// `model-route-unconfigured` (spec §7 / error code). Run once at boot; the runtime
    /// route handler maps the same error to HTTP 500 as a backstop.
    pub fn validate(&self) -> Result<()> {
        for role in ModelRole::all() {
            if self.pick(role).is_empty() {
                return Err(VaultError::InvalidInput(format!(
                    "model-route-unconfigured: role {:?} has no model mapping",
                    role
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_role_maps_to_configured_model() {
        let settings = json!({
            "model_routing": {
                "cheap": "gpt-4o-mini",
                "reasoning": "gpt-4o",
                "vision": "gpt-4o-mini"
            }
        });
        let r = ModelRouter::from_settings(&settings);
        assert_eq!(r.pick(ModelRole::Cheap), "gpt-4o-mini");
        assert_eq!(r.pick(ModelRole::Reasoning), "gpt-4o");
        assert_eq!(r.pick(ModelRole::Vision), "gpt-4o-mini");
        assert!(r.is_fully_configured());
        r.validate().unwrap();
    }

    #[test]
    fn test_missing_block_falls_back_to_default() {
        // no model_routing block → every role = llm.model
        let settings = json!({ "llm": { "model": "qwen2.5:3b" } });
        let r = ModelRouter::from_settings(&settings);
        assert_eq!(r.pick(ModelRole::Cheap), "qwen2.5:3b");
        assert_eq!(r.pick(ModelRole::Reasoning), "qwen2.5:3b");
        assert_eq!(r.pick(ModelRole::Vision), "qwen2.5:3b");
        assert!(!r.is_fully_configured(), "fallback is not fully-configured");
        r.validate().unwrap(); // degenerate but usable
    }

    #[test]
    fn test_partial_block_fills_gaps_from_default() {
        // cheap explicit, reasoning/vision fall back to default
        let settings = json!({
            "llm": { "model": "gpt-4o" },
            "model_routing": { "cheap": "gpt-4o-mini" }
        });
        let r = ModelRouter::from_settings(&settings);
        assert_eq!(r.pick(ModelRole::Cheap), "gpt-4o-mini");
        assert_eq!(r.pick(ModelRole::Reasoning), "gpt-4o");
        assert_eq!(r.pick(ModelRole::Vision), "gpt-4o");
    }

    #[test]
    fn test_unconfigured_role_errors() {
        // empty settings → no default resolvable → all-empty → validate errors
        let settings = json!({});
        let r = ModelRouter::from_settings(&settings);
        let err = r.validate().unwrap_err();
        assert!(
            format!("{err:?}").contains("model-route-unconfigured"),
            "expected model-route-unconfigured, got {err:?}"
        );
    }

    #[test]
    fn test_byok_local_fallback_to_ollama_model() {
        // BYOK/Ollama: user has no routing block, just a local model → all_same
        let r = ModelRouter::all_same("llama3.2:3b");
        assert_eq!(r.pick(ModelRole::Cheap), "llama3.2:3b");
        assert_eq!(r.pick(ModelRole::Reasoning), "llama3.2:3b");
        assert_eq!(r.pick(ModelRole::Vision), "llama3.2:3b");
        r.validate().unwrap();
    }

    #[test]
    fn test_default_key_overrides_llm_model() {
        let settings = json!({
            "llm": { "model": "gpt-4o" },
            "model_routing": { "default": "qwen-turbo" }
        });
        let r = ModelRouter::from_settings(&settings);
        assert_eq!(r.pick(ModelRole::Cheap), "qwen-turbo");
        assert_eq!(r.pick(ModelRole::Reasoning), "qwen-turbo");
    }

    #[test]
    fn test_reasoning_distinct_from_cheap_by_default_config() {
        // proves sub-grouping (not blanket-tier): with the recommended config the two differ
        let settings = json!({
            "model_routing": { "cheap": "gpt-4o-mini", "reasoning": "gpt-4o", "vision": "gpt-4o-mini" }
        });
        let r = ModelRouter::from_settings(&settings);
        assert_ne!(
            r.pick(ModelRole::Cheap),
            r.pick(ModelRole::Reasoning),
            "cheap and reasoning must be distinct in the recommended config"
        );
    }
}

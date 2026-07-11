use serde_json::Value;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub(crate) const SUBMIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const DEFAULT_PROFILE_CACHE_TTL_MS: u32 = 60_000;
const DEFAULT_PROFILE_PROBE_TIMEOUT_MS: u32 = 500;
static RUNTIME_PROFILE_CACHE: OnceLock<Mutex<attune_core::edge_cloud::RuntimeProfileCache>> =
    OnceLock::new();

const SCHEDULER_NATIVE_PROVIDERS: &[&str] =
    &["local_scheduler", "edge_scheduler", "scheduler_native"];

pub(crate) fn provider_is_scheduler_native(provider: &str) -> bool {
    let normalized = provider.trim().to_ascii_lowercase();
    SCHEDULER_NATIVE_PROVIDERS
        .iter()
        .any(|known| normalized == *known)
}

pub(crate) fn settings_provider_is_scheduler_native(settings: &Value, section: &str) -> bool {
    settings
        .get(section)
        .and_then(|v| v.get("provider"))
        .and_then(|v| v.as_str())
        .map(provider_is_scheduler_native)
        .unwrap_or(false)
}

pub(crate) fn native_kb_enabled(
    settings: &Value,
    hardware: &attune_core::platform::HardwareProfile,
) -> bool {
    hardware.form_factor.prefers_local_llm()
        || settings_provider_is_scheduler_native(settings, "embedding")
        || settings_provider_is_scheduler_native(settings, "llm")
        || env_bool_any(
            &[
                "ATTUNE_SCHEDULER_NATIVE_KB",
                "ATTUNE_LOCAL_SCHEDULER_NATIVE_KB",
            ],
            false,
        )
}

pub(crate) fn base_from_settings(settings: &Value) -> String {
    let configured = settings
        .get("llm")
        .and_then(|llm| llm.get("endpoint"))
        .and_then(|v| v.as_str())
        .or_else(|| {
            settings
                .get("embedding")
                .and_then(|embedding| embedding.get("endpoint"))
                .and_then(|v| v.as_str())
        })
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let base = configured.unwrap_or(attune_core::edge_cloud::capacity::DEFAULT_SCHEDULER_BASE);
    attune_core::edge_cloud::capacity::normalize_scheduler_base(base)
}

pub(crate) fn base_from_state(state: &crate::state::SharedState) -> String {
    let settings = state
        .vault
        .lock()
        .ok()
        .and_then(|vault| vault.store().get_meta("app_settings").ok().flatten())
        .and_then(|data| serde_json::from_slice::<Value>(&data).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    base_from_settings(&settings)
}

pub(crate) fn ingest_options_from_state(
    state: &crate::state::SharedState,
    profile: Option<&str>,
) -> attune_core::ingest::IngestOptions {
    let base = base_from_state(state);
    attune_core::ingest::IngestOptions::with_profile(profile)
        .with_scheduler_base(Some(&base))
        .with_scheduler_timeout_ms(env_u32_any(
            &[
                "ATTUNE_SCHEDULER_PARSE_TIMEOUT_MS",
                "ATTUNE_LOCAL_SCHEDULER_PARSE_TIMEOUT_MS",
            ],
            120_000,
        ) as u64)
}

pub(crate) fn runtime_profiles_for_base(base: &str) -> attune_core::edge_cloud::RuntimeProfileSet {
    let ttl = Duration::from_millis(env_u32_any(
        &[
            "ATTUNE_SCHEDULER_PROFILE_CACHE_TTL_MS",
            "ATTUNE_LOCAL_SCHEDULER_PROFILE_CACHE_TTL_MS",
        ],
        DEFAULT_PROFILE_CACHE_TTL_MS,
    ) as u64);
    let timeout = Duration::from_millis(env_u32_any(
        &[
            "ATTUNE_SCHEDULER_PROFILE_PROBE_TIMEOUT_MS",
            "ATTUNE_LOCAL_SCHEDULER_PROFILE_PROBE_TIMEOUT_MS",
        ],
        DEFAULT_PROFILE_PROBE_TIMEOUT_MS,
    ) as u64);
    let cache = RUNTIME_PROFILE_CACHE
        .get_or_init(|| Mutex::new(attune_core::edge_cloud::RuntimeProfileCache::new(ttl)));
    let client = attune_core::edge_cloud::LocalSchedulerClient::with_base(base, timeout);
    cache
        .lock()
        .map(|mut guard| {
            guard.set_ttl(ttl);
            guard.get_or_refresh(&client, base, Instant::now())
        })
        .unwrap_or_else(|_| {
            attune_core::edge_cloud::RuntimeProfileResolver::static_local_scheduler_profile(base)
        })
}

pub(crate) fn env_u32_any(keys: &[&str], default: u32) -> u32 {
    keys.iter()
        .find_map(|key| {
            std::env::var(key)
                .ok()
                .and_then(|v| v.trim().parse::<u32>().ok())
                .filter(|v| *v > 0)
        })
        .unwrap_or(default)
}

pub(crate) fn env_bool_any(keys: &[&str], default: bool) -> bool {
    keys.iter()
        .find_map(|key| {
            std::env::var(key).ok().map(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
        })
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduler_native_provider_names_are_generic() {
        assert!(provider_is_scheduler_native("local_scheduler"));
        assert!(provider_is_scheduler_native("edge_scheduler"));
        assert!(provider_is_scheduler_native("scheduler_native"));
        assert!(!provider_is_scheduler_native("openai_compat"));
    }

    #[test]
    fn base_from_settings_prefers_llm_then_embedding_and_strips_v1() {
        let settings = serde_json::json!({
            "llm": { "endpoint": "http://127.0.0.1:8090/v1/" },
            "embedding": { "endpoint": "http://127.0.0.1:8091" }
        });
        assert_eq!(base_from_settings(&settings), "http://127.0.0.1:8090");

        let settings = serde_json::json!({
            "embedding": { "endpoint": "http://127.0.0.1:8091/v1" }
        });
        assert_eq!(base_from_settings(&settings), "http://127.0.0.1:8091");
    }
}

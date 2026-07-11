use axum::http::StatusCode;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SchedulerDegradationPolicy {
    /// The caller cannot fabricate a useful partial result. Return a structured
    /// delay/failure response instead of pretending the task succeeded.
    HonestFailure,
    /// The caller may return a reduced result, but only with explicit degraded
    /// metadata and warnings in the payload.
    ExplicitDegradedResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SchedulerFailureView {
    pub status: StatusCode,
    pub code: &'static str,
    pub scheduler_error: &'static str,
    pub retryable: bool,
    pub may_degrade: bool,
}

pub(crate) fn classify_scheduler_failure(
    error: &attune_core::error::VaultError,
    policy: SchedulerDegradationPolicy,
) -> SchedulerFailureView {
    use attune_core::edge_cloud::SchedulerErrorKind;

    let kind = attune_core::edge_cloud::classify_scheduler_error(error);
    let mut view = match kind {
        Some(SchedulerErrorKind::Busy) => SchedulerFailureView {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "local-scheduler-busy",
            scheduler_error: "busy",
            retryable: true,
            may_degrade: false,
        },
        Some(SchedulerErrorKind::Oversize) => SchedulerFailureView {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            code: "local-scheduler-oversize",
            scheduler_error: "oversize",
            retryable: false,
            may_degrade: false,
        },
        Some(SchedulerErrorKind::RateLimited) => SchedulerFailureView {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: "local-scheduler-rate-limited",
            scheduler_error: "rate-limited",
            retryable: true,
            may_degrade: false,
        },
        Some(SchedulerErrorKind::Unavailable | SchedulerErrorKind::Transport) => {
            SchedulerFailureView {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "local-scheduler-unavailable",
                scheduler_error: kind.map(|k| k.as_str()).unwrap_or("unavailable"),
                retryable: true,
                may_degrade: false,
            }
        }
        Some(SchedulerErrorKind::Delayed) => SchedulerFailureView {
            status: StatusCode::GATEWAY_TIMEOUT,
            code: "local-scheduler-delayed",
            scheduler_error: "delayed",
            retryable: true,
            may_degrade: false,
        },
        Some(SchedulerErrorKind::Cancelled) => SchedulerFailureView {
            status: StatusCode::CONFLICT,
            code: "local-scheduler-cancelled",
            scheduler_error: "cancelled",
            retryable: false,
            may_degrade: false,
        },
        Some(SchedulerErrorKind::Expired) => SchedulerFailureView {
            status: StatusCode::GONE,
            code: "local-scheduler-expired",
            scheduler_error: "expired",
            retryable: false,
            may_degrade: false,
        },
        Some(SchedulerErrorKind::JobFailed) => SchedulerFailureView {
            status: StatusCode::BAD_GATEWAY,
            code: "local-scheduler-job-failed",
            scheduler_error: "job-failed",
            retryable: false,
            may_degrade: false,
        },
        Some(SchedulerErrorKind::InvalidJson) => SchedulerFailureView {
            status: StatusCode::BAD_GATEWAY,
            code: "local-scheduler-invalid-response",
            scheduler_error: "invalid-json",
            retryable: false,
            may_degrade: false,
        },
        Some(SchedulerErrorKind::Http(status)) if (500..600).contains(&status) => {
            SchedulerFailureView {
                status: StatusCode::BAD_GATEWAY,
                code: "local-scheduler-upstream-error",
                scheduler_error: "http-error",
                retryable: true,
                may_degrade: false,
            }
        }
        Some(SchedulerErrorKind::Http(_)) => SchedulerFailureView {
            status: StatusCode::BAD_REQUEST,
            code: "local-scheduler-request-rejected",
            scheduler_error: "http-error",
            retryable: false,
            may_degrade: false,
        },
        None => SchedulerFailureView {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "local-scheduler-submit-failed",
            scheduler_error: "unknown",
            retryable: true,
            may_degrade: false,
        },
    };

    if policy == SchedulerDegradationPolicy::ExplicitDegradedResult {
        view.may_degrade = matches!(
            kind,
            Some(SchedulerErrorKind::InvalidJson | SchedulerErrorKind::JobFailed)
        );
    }
    view
}

pub(crate) fn scheduler_failure_body(
    error: &attune_core::error::VaultError,
    policy: SchedulerDegradationPolicy,
    human_error: &'static str,
) -> (StatusCode, serde_json::Value) {
    let view = classify_scheduler_failure(error, policy);
    (
        view.status,
        serde_json::json!({
            "error": human_error,
            "code": view.code,
            "scheduler_error": view.scheduler_error,
            "retryable": view.retryable,
            "may_degrade": view.may_degrade,
            "detail": error.to_string(),
        }),
    )
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

    #[test]
    fn scheduler_failure_policy_distinguishes_delay_failure_and_degrade() {
        let delayed = attune_core::error::VaultError::LlmUnavailable(
            "local scheduler job job_abc timed out".to_string(),
        );
        let delayed_view = classify_scheduler_failure(
            &delayed,
            SchedulerDegradationPolicy::ExplicitDegradedResult,
        );
        assert_eq!(delayed_view.code, "local-scheduler-delayed");
        assert!(delayed_view.retryable);
        assert!(!delayed_view.may_degrade);

        let oversize = attune_core::error::VaultError::LlmUnavailable(
            "local scheduler /kb/tasks/kb.query.ask returned 422 Unprocessable Entity: too large"
                .to_string(),
        );
        let oversize_view = classify_scheduler_failure(
            &oversize,
            SchedulerDegradationPolicy::ExplicitDegradedResult,
        );
        assert_eq!(oversize_view.code, "local-scheduler-oversize");
        assert!(!oversize_view.may_degrade);

        let failed = attune_core::error::VaultError::LlmUnavailable(
            "local scheduler job job_abc failed: worker crashed".to_string(),
        );
        let strict_view =
            classify_scheduler_failure(&failed, SchedulerDegradationPolicy::HonestFailure);
        let degrade_view =
            classify_scheduler_failure(&failed, SchedulerDegradationPolicy::ExplicitDegradedResult);
        assert!(!strict_view.may_degrade);
        assert!(degrade_view.may_degrade);
    }
}

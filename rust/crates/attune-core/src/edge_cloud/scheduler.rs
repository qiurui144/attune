//! Typed client for the local scheduler API.
//!
//! This is intentionally a thin transport/contract layer. Attune owns privacy,
//! retrieval policy, SRAS selection, and cloud spill decisions; local-scheduler
//! remains the source of truth for model state, resource capacity, and
//! sync/async admission limits.

use crate::edge_cloud::capacity::{
    normalize_scheduler_base, DEFAULT_PROBE_TIMEOUT, DEFAULT_SCHEDULER_BASE,
};
use crate::error::{Result, VaultError};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

/// Blocking client for scheduler control/contract APIs.
pub struct LocalSchedulerClient {
    base_url: String,
    client: reqwest::blocking::Client,
}

impl LocalSchedulerClient {
    pub fn new() -> Self {
        Self::with_base(DEFAULT_SCHEDULER_BASE, DEFAULT_PROBE_TIMEOUT)
    }

    pub fn with_base(base_url: &str, timeout: Duration) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout)
            .build()
            .unwrap_or_default();
        LocalSchedulerClient {
            base_url: normalize_scheduler_base(base_url),
            client,
        }
    }

    pub fn benchmark_contract(&self) -> Result<SchedulerBenchmarkContract> {
        self.get_json("/benchmark/contract")
    }

    pub fn models(&self) -> Result<SchedulerModels> {
        self.get_json("/models")
    }

    pub fn model(&self, name: &str) -> Result<SchedulerModelStatus> {
        validate_path_segment("model", name)?;
        self.get_json(&format!("/models/{name}"))
    }

    pub fn capacity(&self) -> Result<SchedulerCapacitySnapshot> {
        self.get_json("/capacity")
    }

    pub fn job(&self, job_id: &str) -> Result<SchedulerJobStatus> {
        validate_path_segment("job_id", job_id)?;
        self.get_json(&format!("/jobs/{job_id}"))
    }

    pub fn cancel_job(&self, job_id: &str) -> Result<SchedulerJobStatus> {
        validate_path_segment("job_id", job_id)?;
        self.post_empty(&format!("/jobs/{job_id}:cancel"))
    }

    pub fn submit_kb_task<B: Serialize>(
        &self,
        task: &str,
        body: &B,
        explicit_async: bool,
    ) -> Result<SchedulerKbTaskResponse> {
        validate_path_segment("task", task)?;
        let suffix = if explicit_async { ":async" } else { "" };
        self.post_json(&format!("/kb/tasks/{task}{suffix}"), body)
    }

    fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .client
            .get(&url)
            .send()
            .map_err(|e| scheduler_transport_error(path, e))?;
        parse_response(path, resp)
    }

    fn post_empty<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .client
            .post(&url)
            .send()
            .map_err(|e| scheduler_transport_error(path, e))?;
        parse_response(path, resp)
    }

    fn post_json<T: DeserializeOwned, B: Serialize>(&self, path: &str, body: &B) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .client
            .post(&url)
            .json(body)
            .send()
            .map_err(|e| scheduler_transport_error(path, e))?;
        parse_response(path, resp)
    }
}

impl Default for LocalSchedulerClient {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SchedulerBenchmarkContract {
    #[serde(default)]
    pub contract_version: String,
    #[serde(default)]
    pub schema_versions: BTreeMap<String, String>,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub queue_order: Vec<String>,
    #[serde(default)]
    pub request_fields: Vec<String>,
    #[serde(default)]
    pub endpoints: serde_json::Value,
    #[serde(default)]
    pub application_api: serde_json::Value,
    #[serde(default)]
    pub runtime_tasks: Vec<SchedulerRuntimeTaskSpec>,
    #[serde(default)]
    pub runtime_policy: serde_json::Value,
    #[serde(default)]
    pub service_classes: Vec<SchedulerServiceClassSpec>,
    #[serde(default)]
    pub models: Vec<SchedulerContractModel>,
    #[serde(default)]
    pub async_jobs: SchedulerAsyncJobs,
    #[serde(default)]
    pub metrics: Vec<String>,
    #[serde(default)]
    pub errors: serde_json::Value,
    #[serde(default)]
    pub prompt_cache: serde_json::Value,
    #[serde(default)]
    pub refusal_policy: serde_json::Value,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SchedulerContractModel {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub primary_device: String,
    #[serde(default)]
    pub fallback_devices: Vec<String>,
    #[serde(default)]
    pub resource_key: String,
    #[serde(default)]
    pub worker_kind: String,
    #[serde(default)]
    pub exclusive: bool,
    #[serde(default)]
    pub queue_capacity: u32,
    #[serde(default)]
    pub worker_threads: u32,
    #[serde(default)]
    pub dram_gb: f64,
    #[serde(default)]
    pub service_class: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub estimated_runtime_ms: u32,
    #[serde(default)]
    pub deadline_ms: u32,
    #[serde(default)]
    pub sync_allowed: bool,
    #[serde(default)]
    pub async_required_above_ms: u32,
    #[serde(default)]
    pub max_context_tokens_sync: u32,
    #[serde(default)]
    pub max_context_tokens_async: u32,
    #[serde(default)]
    pub max_output_tokens_sync: u32,
    #[serde(default)]
    pub max_output_tokens_async: u32,
    #[serde(default)]
    pub quality_profile: serde_json::Value,
    #[serde(default)]
    pub backend_profile: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SchedulerRuntimeTaskSpec {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub stage: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub service_class: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub async_only: bool,
    #[serde(default)]
    pub avoid_cold_start: bool,
    #[serde(default)]
    pub timeout_ms: u32,
    #[serde(default)]
    pub deadline_ms: u32,
    #[serde(default)]
    pub context_tokens: u32,
    #[serde(default)]
    pub max_output_tokens: u32,
    #[serde(default)]
    pub ttl_ms: u32,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SchedulerServiceClassSpec {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub infer_allowed: bool,
    #[serde(default)]
    pub sync_allowed: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SchedulerAsyncJobs {
    #[serde(default)]
    pub max_active_jobs: u32,
    #[serde(default)]
    pub active: u32,
    #[serde(default)]
    pub done_retained: u32,
    #[serde(default)]
    pub canceled_retained: u32,
    #[serde(default)]
    pub expired_retained: u32,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SchedulerModels {
    #[serde(default)]
    pub models: Vec<SchedulerModelStatus>,
    #[serde(default)]
    pub revision: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SchedulerModelStatus {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub lifecycle: String,
    #[serde(default)]
    pub dispatchable: String,
    #[serde(default)]
    pub devices: serde_json::Value,
    #[serde(default)]
    pub exclusive: bool,
    #[serde(default)]
    pub queue_depth: i32,
    #[serde(default)]
    pub queue_capacity: i32,
    #[serde(default)]
    pub last_latency_ms: Option<f64>,
    #[serde(default)]
    pub p50_latency_ms: Option<f64>,
    #[serde(default)]
    pub p99_latency_ms: Option<f64>,
    #[serde(default)]
    pub last_success_ts: Option<String>,
    #[serde(default)]
    pub state_revision: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SchedulerCapacitySnapshot {
    #[serde(default)]
    pub clusters: BTreeMap<String, SchedulerClusterCapacity>,
    #[serde(default)]
    pub dram_used_gb: f64,
    #[serde(default)]
    pub dram_reserved_gb: f64,
    #[serde(default)]
    pub dram_total_gb: f64,
    #[serde(default)]
    pub memory: SchedulerMemorySnapshot,
    #[serde(default)]
    pub active_models: i32,
    #[serde(default)]
    pub revision: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SchedulerClusterCapacity {
    #[serde(default)]
    pub cores_total: i32,
    #[serde(default)]
    pub cores_used: i32,
    #[serde(default)]
    pub busy_ratio: f64,
    #[serde(default)]
    pub ep_active: bool,
    #[serde(default)]
    pub quarantined: bool,
    #[serde(default)]
    pub ep_session_holder: Option<String>,
    #[serde(default)]
    pub running_models: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SchedulerMemorySnapshot {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub governor_enabled: Option<bool>,
    #[serde(default)]
    pub available_gb: Option<f64>,
    #[serde(default)]
    pub used_gb: Option<f64>,
    #[serde(default)]
    pub soft_min_available_gb: Option<f64>,
    #[serde(default)]
    pub hard_min_available_gb: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct SchedulerJobStatus {
    #[serde(default)]
    pub job_id: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub task: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub service_class: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub eta_ms: Option<u32>,
    #[serde(default)]
    pub scheduled_as: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub phase: Option<String>,
    #[serde(default)]
    pub cancel_mode: Option<String>,
    #[serde(default)]
    pub cancel_requested: Option<bool>,
    #[serde(default)]
    pub ttl_ms: Option<u32>,
    #[serde(default)]
    pub outputs: serde_json::Value,
    #[serde(default)]
    pub device_used: Option<String>,
    #[serde(default)]
    pub latency_ms: Option<f64>,
    #[serde(default)]
    pub queue_wait_ms: Option<f64>,
    #[serde(default)]
    pub startup_state: Option<String>,
    #[serde(default)]
    pub startup_wait_ms: Option<f64>,
    #[serde(default)]
    pub cold_start_wait_ms: Option<f64>,
    #[serde(default)]
    pub worker_pid: Option<i32>,
    #[serde(default, deserialize_with = "deserialize_optional_error_string")]
    pub error: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub prompt_cache_key: Option<String>,
    #[serde(default)]
    pub cache_hit: Option<bool>,
    #[serde(default)]
    pub prompt_cache: serde_json::Value,
    #[serde(default)]
    pub prompt_cache_policy: Option<String>,
    #[serde(default)]
    pub refusal_policy: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct SchedulerKbTaskResponse {
    #[serde(default)]
    pub scheduled_as: String,
    #[serde(default)]
    pub job_id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub task: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub service_class: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub eta_ms: Option<u32>,
    #[serde(default)]
    pub outputs: serde_json::Value,
    #[serde(default)]
    pub device_used: Option<String>,
    #[serde(default)]
    pub latency_ms: Option<f64>,
    #[serde(default)]
    pub queue_wait_ms: Option<f64>,
    #[serde(default)]
    pub startup_state: Option<String>,
    #[serde(default)]
    pub startup_wait_ms: Option<f64>,
    #[serde(default)]
    pub cold_start_wait_ms: Option<f64>,
    #[serde(default)]
    pub worker_pid: Option<i32>,
    #[serde(default, deserialize_with = "deserialize_optional_error_string")]
    pub error: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub prompt_cache_key: Option<String>,
    #[serde(default)]
    pub cache_hit: Option<bool>,
    #[serde(default)]
    pub prompt_cache: serde_json::Value,
    #[serde(default)]
    pub prompt_cache_policy: Option<String>,
    #[serde(default)]
    pub refusal_policy: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerErrorKind {
    Busy,
    Oversize,
    RateLimited,
    Unavailable,
    Delayed,
    Cancelled,
    Expired,
    JobFailed,
    Http(u16),
    Transport,
    InvalidJson,
}

impl SchedulerErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SchedulerErrorKind::Busy => "busy",
            SchedulerErrorKind::Oversize => "oversize",
            SchedulerErrorKind::RateLimited => "rate-limited",
            SchedulerErrorKind::Unavailable => "unavailable",
            SchedulerErrorKind::Delayed => "delayed",
            SchedulerErrorKind::Cancelled => "cancelled",
            SchedulerErrorKind::Expired => "expired",
            SchedulerErrorKind::JobFailed => "job-failed",
            SchedulerErrorKind::Http(_) => "http-error",
            SchedulerErrorKind::Transport => "transport",
            SchedulerErrorKind::InvalidJson => "invalid-json",
        }
    }

    pub fn http_status(self) -> Option<u16> {
        match self {
            SchedulerErrorKind::Busy => Some(409),
            SchedulerErrorKind::Oversize => Some(422),
            SchedulerErrorKind::RateLimited => Some(429),
            SchedulerErrorKind::Unavailable => Some(503),
            SchedulerErrorKind::Delayed => Some(504),
            SchedulerErrorKind::Cancelled => Some(409),
            SchedulerErrorKind::Expired => Some(410),
            SchedulerErrorKind::JobFailed => Some(502),
            SchedulerErrorKind::Http(status) => Some(status),
            SchedulerErrorKind::Transport | SchedulerErrorKind::InvalidJson => None,
        }
    }

    pub fn retryable(self) -> bool {
        matches!(
            self,
            SchedulerErrorKind::Busy
                | SchedulerErrorKind::RateLimited
                | SchedulerErrorKind::Unavailable
                | SchedulerErrorKind::Delayed
                | SchedulerErrorKind::Transport
        )
    }
}

pub fn classify_scheduler_error(err: &VaultError) -> Option<SchedulerErrorKind> {
    let VaultError::LlmUnavailable(message) = err else {
        return None;
    };
    if !message.starts_with("local scheduler ") {
        return None;
    }
    if message.contains(" request failed:") {
        return Some(SchedulerErrorKind::Transport);
    }
    if message.contains(" invalid json:") {
        return Some(SchedulerErrorKind::InvalidJson);
    }
    if message.contains(" job ") && message.contains(" timed out") {
        return Some(SchedulerErrorKind::Delayed);
    }
    if message.contains(" job cancelled") || message.contains(" job canceled") {
        return Some(SchedulerErrorKind::Cancelled);
    }
    if message.contains(" job ") && message.contains(" expired:") {
        return Some(SchedulerErrorKind::Expired);
    }
    if message.contains(" job ") && message.contains(" failed:") {
        return Some(SchedulerErrorKind::JobFailed);
    }
    let status = parse_scheduler_status(message)?;
    Some(match status {
        409 => SchedulerErrorKind::Busy,
        422 => SchedulerErrorKind::Oversize,
        429 => SchedulerErrorKind::RateLimited,
        503 => SchedulerErrorKind::Unavailable,
        other => SchedulerErrorKind::Http(other),
    })
}

fn parse_response<T: DeserializeOwned>(path: &str, resp: reqwest::blocking::Response) -> Result<T> {
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(VaultError::LlmUnavailable(format!(
            "local scheduler {path} returned {status}: {}",
            truncate_body(&body)
        )));
    }
    resp.json::<T>().map_err(|e| {
        VaultError::LlmUnavailable(format!("local scheduler {path} invalid json: {e}"))
    })
}

fn parse_scheduler_status(message: &str) -> Option<u16> {
    message
        .split(" returned ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|status| status.parse::<u16>().ok())
}

fn scheduler_transport_error(path: &str, e: reqwest::Error) -> VaultError {
    VaultError::LlmUnavailable(format!("local scheduler {path} request failed: {e}"))
}

fn truncate_body(body: &str) -> String {
    const MAX: usize = 256;
    if body.chars().count() <= MAX {
        body.to_string()
    } else {
        let prefix: String = body.chars().take(MAX).collect();
        format!("{prefix}...")
    }
}

fn validate_path_segment(name: &str, value: &str) -> Result<()> {
    let valid = !value.is_empty()
        && !value.contains("..")
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'));
    if valid {
        Ok(())
    } else {
        Err(VaultError::InvalidInput(format!(
            "invalid local scheduler {name} path segment"
        )))
    }
}

fn deserialize_optional_error_string<'de, D>(deserializer: D) -> std::result::Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(error_value_to_string))
}

fn error_value_to_string(value: serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => non_empty_string(s),
        serde_json::Value::Object(map) => {
            for key in ["message", "error", "detail", "reason", "code"] {
                if let Some(s) = map.get(key).and_then(|v| v.as_str()) {
                    if let Some(s) = non_empty_string(s.to_string()) {
                        return Some(s);
                    }
                }
            }
            non_empty_string(serde_json::Value::Object(map).to_string())
        }
        other => non_empty_string(other.to_string()),
    }
}

fn non_empty_string(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_path_segments() {
        assert!(validate_path_segment("task", "kb.query.ask").is_ok());
        assert!(validate_path_segment("task", "../x").is_err());
        assert!(validate_path_segment("task", "kb..query").is_err());
        assert!(validate_path_segment("job", "job_abc-123").is_ok());
        assert!(validate_path_segment("job", "job/abc").is_err());
    }

    #[test]
    fn classifies_scheduler_http_and_transport_errors() {
        let busy = VaultError::LlmUnavailable(
            "local scheduler /kb/tasks/kb.query.ask returned 409 Conflict: busy".to_string(),
        );
        assert_eq!(
            classify_scheduler_error(&busy),
            Some(SchedulerErrorKind::Busy)
        );
        assert_eq!(
            classify_scheduler_error(&busy).unwrap().http_status(),
            Some(409)
        );
        assert!(classify_scheduler_error(&busy).unwrap().retryable());

        let oversize = VaultError::LlmUnavailable(
            "local scheduler /kb/tasks/kb.query.ask returned 422 Unprocessable Entity: too large"
                .to_string(),
        );
        assert_eq!(
            classify_scheduler_error(&oversize),
            Some(SchedulerErrorKind::Oversize)
        );
        assert!(!classify_scheduler_error(&oversize).unwrap().retryable());

        let transport = VaultError::LlmUnavailable(
            "local scheduler /capacity request failed: timed out".to_string(),
        );
        assert_eq!(
            classify_scheduler_error(&transport),
            Some(SchedulerErrorKind::Transport)
        );

        let cloud = VaultError::LlmUnavailable("openai HTTP 429: quota".to_string());
        assert_eq!(classify_scheduler_error(&cloud), None);
    }

    #[test]
    fn classifies_scheduler_job_terminal_and_delay_errors() {
        let delayed =
            VaultError::LlmUnavailable("local scheduler job job_abc timed out".to_string());
        assert_eq!(
            classify_scheduler_error(&delayed),
            Some(SchedulerErrorKind::Delayed)
        );
        assert!(classify_scheduler_error(&delayed).unwrap().retryable());

        let cancelled = VaultError::LlmUnavailable("local scheduler job cancelled".to_string());
        assert_eq!(
            classify_scheduler_error(&cancelled),
            Some(SchedulerErrorKind::Cancelled)
        );

        let expired = VaultError::LlmUnavailable(
            "local scheduler job job_abc expired: ttl exceeded".to_string(),
        );
        assert_eq!(
            classify_scheduler_error(&expired),
            Some(SchedulerErrorKind::Expired)
        );

        let failed = VaultError::LlmUnavailable(
            "local scheduler job job_abc failed: model crashed".to_string(),
        );
        assert_eq!(
            classify_scheduler_error(&failed),
            Some(SchedulerErrorKind::JobFailed)
        );
    }
}

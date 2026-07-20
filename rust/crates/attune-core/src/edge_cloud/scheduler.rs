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
use std::io::Read;
use std::time::Duration;

const DEFAULT_MAX_SCHEDULER_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// Blocking client for scheduler control/contract APIs.
pub struct LocalSchedulerClient {
    base_url: String,
    client: reqwest::blocking::Client,
    max_response_bytes: usize,
}

impl LocalSchedulerClient {
    pub fn new() -> Self {
        Self::with_base(DEFAULT_SCHEDULER_BASE, DEFAULT_PROBE_TIMEOUT)
    }

    pub fn with_base(base_url: &str, timeout: Duration) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout)
            .no_proxy()
            // A local scheduler must never redirect a prompt/document-bearing
            // request to another origin after the destination check above.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default();
        LocalSchedulerClient {
            base_url: normalize_scheduler_base(base_url),
            client,
            max_response_bytes: DEFAULT_MAX_SCHEDULER_RESPONSE_BYTES,
        }
    }

    /// Bound the bytes retained before JSON decoding. Scheduler responses are
    /// untrusted local IPC; semantic validators run only after this transport
    /// guard has prevented an oversized body from being allocated in full.
    pub fn with_max_response_bytes(mut self, max_response_bytes: usize) -> Self {
        self.max_response_bytes = max_response_bytes.max(1);
        self
    }

    /// Rebuild this client against the same normalized scheduler origin with a
    /// tighter per-request timeout. Polling callers use this to ensure no job
    /// status request can outlive their remaining total deadline.
    pub fn with_timeout(&self, timeout: Duration) -> Self {
        Self::with_base(&self.base_url, timeout).with_max_response_bytes(self.max_response_bytes)
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
        let path = format!("/jobs/{job_id}");
        let url = self.request_url(&path)?;
        let resp = self
            .client
            .get(&url)
            .send()
            .map_err(|e| scheduler_transport_error(&path, e))?;
        let http_status = resp.status().as_u16();
        let mut response: SchedulerJobStatus =
            parse_response(&path, resp, self.max_response_bytes)?;
        response.http_status = Some(http_status);
        Ok(response)
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
        let path = format!("/kb/tasks/{task}{suffix}");
        let url = self.request_url(&path)?;
        let resp = self
            .client
            .post(&url)
            .json(body)
            .send()
            .map_err(|e| scheduler_transport_error(&path, e))?;
        let http_status = resp.status().as_u16();
        let mut response: SchedulerKbTaskResponse =
            parse_response(&path, resp, self.max_response_bytes)?;
        response.http_status = Some(http_status);
        Ok(response)
    }

    fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = self.request_url(path)?;
        let resp = self
            .client
            .get(&url)
            .send()
            .map_err(|e| scheduler_transport_error(path, e))?;
        parse_response(path, resp, self.max_response_bytes)
    }

    fn post_empty<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = self.request_url(path)?;
        let resp = self
            .client
            .post(&url)
            .send()
            .map_err(|e| scheduler_transport_error(path, e))?;
        parse_response(path, resp, self.max_response_bytes)
    }

    fn request_url(&self, path: &str) -> Result<String> {
        crate::net::destination::join_local_scheduler_url(&self.base_url, path).ok_or_else(|| {
            VaultError::LlmUnavailable(
                "local scheduler endpoint must use an unambiguous localhost, loopback, or private IP URL"
                    .to_string(),
            )
        })
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
    /// Transport status captured by [`LocalSchedulerClient`].
    #[serde(skip)]
    pub http_status: Option<u16>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schema_version: String,
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
    #[serde(
        default,
        deserialize_with = "deserialize_optional_error_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub error_code: Option<String>,
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

/// Normalized lifecycle state for scheduler jobs.
///
/// The scheduler contract uses `queued`, `running`, `cancel_requested`, `done`,
/// `error`, `canceled`, and `expired`, while older deployments and adapters may
/// return common aliases. Unknown values remain waiting so callers preserve the
/// existing conservative behavior of polling until their local deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerJobState {
    Succeeded,
    Failed,
    Waiting,
}

impl SchedulerJobStatus {
    pub fn normalized_state(&self) -> SchedulerJobState {
        if self
            .error
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            return SchedulerJobState::Failed;
        }
        SchedulerJobState::from_status(&self.status)
    }

    pub fn failure_detail(&self) -> Option<&str> {
        self.error
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                self.detail
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
            })
            .or_else(|| {
                self.reason
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
            })
    }
}

impl SchedulerJobState {
    pub fn from_status(status: &str) -> Self {
        match status.trim().to_ascii_lowercase().as_str() {
            "done" | "completed" | "complete" | "ok" | "success" | "succeeded" => Self::Succeeded,
            "error" | "failed" | "failure" | "canceled" | "cancelled" | "expired" | "timeout"
            | "timed_out" | "timed-out" => Self::Failed,
            _ => Self::Waiting,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct SchedulerKbTaskResponse {
    /// Transport status captured by [`LocalSchedulerClient`]. It is not part of
    /// the scheduler JSON contract, but `202 Accepted` itself proves that the
    /// result is asynchronous and therefore must carry a trackable job id.
    #[serde(skip)]
    pub http_status: Option<u16>,
    #[serde(default)]
    pub schema_version: String,
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
    #[serde(default, deserialize_with = "deserialize_optional_error_string")]
    pub error_code: Option<String>,
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

impl SchedulerKbTaskResponse {
    pub fn normalized_state(&self) -> SchedulerJobState {
        if self
            .error
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            return SchedulerJobState::Failed;
        }
        if self.http_status == Some(202) {
            return SchedulerJobState::Waiting;
        }
        self.status
            .as_deref()
            .map(SchedulerJobState::from_status)
            .unwrap_or(SchedulerJobState::Waiting)
    }

    pub fn failure_detail(&self) -> Option<&str> {
        self.error
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                self.detail
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
            })
            .or_else(|| {
                self.reason
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
            })
    }

    /// Whether this response represents work that must be followed through the
    /// async job API. Every non-empty status normalized as waiting needs a
    /// `job_id`, including statuses introduced by newer schedulers. A missing
    /// status remains the legacy synchronous-response shape.
    pub fn requires_job_id(&self) -> bool {
        let scheduled_as = self.scheduled_as.trim();
        // Only a missing/blank mode is the legacy shape. `sync` is the sole
        // explicit synchronous mode; every unknown non-empty future mode must
        // fail closed behind a trackable job id.
        if !scheduled_as.is_empty() && !scheduled_as.eq_ignore_ascii_case("sync") {
            return true;
        }
        self.status
            .as_deref()
            .map(str::trim)
            .filter(|status| !status.is_empty())
            .is_some_and(|status| {
                SchedulerJobState::from_status(status) == SchedulerJobState::Waiting
            })
    }

    pub fn missing_required_job_id(&self) -> bool {
        self.requires_job_id()
            && self
                .job_id
                .as_deref()
                .map(str::trim)
                .filter(|job_id| !job_id.is_empty())
                .is_none()
    }

    /// Validate the scheduler's 2xx submission payload. HTTP success is only
    /// transport success: explicit async calls and async/future response modes
    /// require a job id, while failed/error-bearing bodies are never accepted
    /// as completed work.
    pub fn validate_submission(&self, explicit_async: bool, label: &str) -> Result<()> {
        if let Some(status) = self
            .http_status
            .filter(|status| *status != 200 && *status != 202)
        {
            return Err(VaultError::LlmUnavailable(format!(
                "{label} returned unsupported successful HTTP status {status}"
            )));
        }
        if self.normalized_state() == SchedulerJobState::Failed {
            return Err(VaultError::LlmUnavailable(format!(
                "{label} failed: {}",
                self.failure_detail()
                    .or(self.status.as_deref())
                    .unwrap_or("unknown error")
            )));
        }
        let missing_job_id = self
            .job_id
            .as_deref()
            .map(str::trim)
            .filter(|job_id| !job_id.is_empty())
            .is_none();
        if missing_job_id
            && (explicit_async || self.http_status == Some(202) || self.requires_job_id())
        {
            return Err(VaultError::LlmUnavailable(format!(
                "{label} returned async/pending status without job_id"
            )));
        }
        Ok(())
    }
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
    if message.contains("/jobs/")
        && (message.contains("worker_error")
            || message.contains("\"status\":\"error\"")
            || message.contains("\"status\":\"failed\""))
    {
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

fn parse_response<T: DeserializeOwned>(
    path: &str,
    mut resp: reqwest::blocking::Response,
    max_response_bytes: usize,
) -> Result<T> {
    let status = resp.status();
    if resp
        .content_length()
        .is_some_and(|length| length > max_response_bytes as u64)
    {
        return Err(oversized_response_error(path, status, max_response_bytes));
    }
    let mut bytes = Vec::with_capacity(
        resp.content_length()
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or(0)
            .min(max_response_bytes)
            .min(64 * 1024),
    );
    resp.by_ref()
        .take(max_response_bytes.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            VaultError::LlmUnavailable(format!(
                "local scheduler {path} request failed: response read failed: {error}"
            ))
        })?;
    if bytes.len() > max_response_bytes {
        return Err(oversized_response_error(path, status, max_response_bytes));
    }
    if !status.is_success() {
        let body = String::from_utf8_lossy(&bytes);
        return Err(VaultError::LlmUnavailable(format!(
            "local scheduler {path} returned {status}: {}",
            truncate_body(&body)
        )));
    }
    serde_json::from_slice::<T>(&bytes).map_err(|e| {
        VaultError::LlmUnavailable(format!("local scheduler {path} invalid json: {e}"))
    })
}

fn oversized_response_error(
    path: &str,
    status: reqwest::StatusCode,
    max_response_bytes: usize,
) -> VaultError {
    if status.is_success() {
        VaultError::LlmUnavailable(format!(
            "local scheduler {path} invalid json: response body exceeds {max_response_bytes} bytes"
        ))
    } else {
        VaultError::LlmUnavailable(format!(
            "local scheduler {path} returned {status}: response body exceeds {max_response_bytes} bytes"
        ))
    }
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

pub(crate) fn validate_path_segment(name: &str, value: &str) -> Result<()> {
    let valid = !value.is_empty()
        // Only a complete dot-segment can change URL path semantics. Embedded
        // dots such as `tts..job` are safe because slash/backslash are not in
        // the allowlist below and are valid Scheduler identifiers.
        && value != "."
        && value != ".."
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

fn deserialize_optional_error_string<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error>
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
    fn normalizes_scheduler_job_status_aliases() {
        let cases = [
            ("done", SchedulerJobState::Succeeded),
            ("completed", SchedulerJobState::Succeeded),
            ("complete", SchedulerJobState::Succeeded),
            ("ok", SchedulerJobState::Succeeded),
            ("success", SchedulerJobState::Succeeded),
            ("succeeded", SchedulerJobState::Succeeded),
            (" DoNe ", SchedulerJobState::Succeeded),
            ("error", SchedulerJobState::Failed),
            ("failed", SchedulerJobState::Failed),
            ("failure", SchedulerJobState::Failed),
            ("canceled", SchedulerJobState::Failed),
            ("cancelled", SchedulerJobState::Failed),
            ("expired", SchedulerJobState::Failed),
            ("timeout", SchedulerJobState::Failed),
            ("timed_out", SchedulerJobState::Failed),
            ("timed-out", SchedulerJobState::Failed),
            (" FaIlEd ", SchedulerJobState::Failed),
            ("queued", SchedulerJobState::Waiting),
            ("running", SchedulerJobState::Waiting),
            ("cancel_requested", SchedulerJobState::Waiting),
            ("pending", SchedulerJobState::Waiting),
            ("accepted", SchedulerJobState::Waiting),
            ("", SchedulerJobState::Waiting),
            ("future_scheduler_state", SchedulerJobState::Waiting),
        ];

        for (status, expected) in cases {
            assert_eq!(
                SchedulerJobState::from_status(status),
                expected,
                "unexpected normalized state for {status:?}"
            );
        }
    }

    #[test]
    fn task_response_exposes_normalized_state_and_failure_detail() {
        let response = SchedulerKbTaskResponse {
            status: Some("FAILED".to_string()),
            error: Some("  ".to_string()),
            detail: Some("model unavailable".to_string()),
            reason: Some("fallback reason".to_string()),
            ..SchedulerKbTaskResponse::default()
        };
        assert_eq!(response.normalized_state(), SchedulerJobState::Failed);
        assert_eq!(response.failure_detail(), Some("model unavailable"));
    }

    #[test]
    fn completed_job_with_error_payload_fails_closed() {
        let job = SchedulerJobStatus {
            status: "done".to_string(),
            error: Some("OOM".to_string()),
            outputs: serde_json::json!({"text": "must not be accepted"}),
            ..SchedulerJobStatus::default()
        };
        assert_eq!(job.normalized_state(), SchedulerJobState::Failed);
        assert_eq!(job.failure_detail(), Some("OOM"));
    }

    #[test]
    fn async_or_pending_task_response_requires_job_id() {
        for response in [
            SchedulerKbTaskResponse {
                scheduled_as: "async".to_string(),
                ..SchedulerKbTaskResponse::default()
            },
            SchedulerKbTaskResponse {
                status: Some("queued".to_string()),
                ..SchedulerKbTaskResponse::default()
            },
            SchedulerKbTaskResponse {
                status: Some("RUNNING".to_string()),
                ..SchedulerKbTaskResponse::default()
            },
            SchedulerKbTaskResponse {
                status: Some("async".to_string()),
                job_id: Some("   ".to_string()),
                ..SchedulerKbTaskResponse::default()
            },
            SchedulerKbTaskResponse {
                status: Some("future_scheduler_state".to_string()),
                ..SchedulerKbTaskResponse::default()
            },
            SchedulerKbTaskResponse {
                scheduled_as: "deferred".to_string(),
                status: Some("done".to_string()),
                ..SchedulerKbTaskResponse::default()
            },
        ] {
            assert!(response.requires_job_id(), "response={response:?}");
            assert!(response.missing_required_job_id(), "response={response:?}");
        }

        let completed = SchedulerKbTaskResponse {
            status: Some("done".to_string()),
            ..SchedulerKbTaskResponse::default()
        };
        assert!(!completed.requires_job_id());
        assert!(!completed.missing_required_job_id());

        let ok = SchedulerKbTaskResponse {
            status: Some(" OK ".to_string()),
            ..SchedulerKbTaskResponse::default()
        };
        assert_eq!(ok.normalized_state(), SchedulerJobState::Succeeded);
        assert!(!ok.requires_job_id());

        for legacy_sync in [
            SchedulerKbTaskResponse::default(),
            SchedulerKbTaskResponse {
                status: Some("   ".to_string()),
                ..SchedulerKbTaskResponse::default()
            },
        ] {
            assert!(!legacy_sync.requires_job_id(), "response={legacy_sync:?}");
            assert!(
                !legacy_sync.missing_required_job_id(),
                "response={legacy_sync:?}"
            );
        }

        let queued = SchedulerKbTaskResponse {
            status: Some("queued".to_string()),
            job_id: Some("job_123".to_string()),
            ..SchedulerKbTaskResponse::default()
        };
        assert!(!queued.missing_required_job_id());
    }

    #[test]
    fn submission_validation_uses_request_mode_and_error_body() {
        let legacy = SchedulerKbTaskResponse::default();
        assert!(legacy.validate_submission(false, "task").is_ok());
        assert!(legacy
            .validate_submission(true, "task")
            .expect_err("explicit async request requires job id")
            .to_string()
            .contains("without job_id"));

        let accepted_without_job = SchedulerKbTaskResponse {
            http_status: Some(202),
            ..SchedulerKbTaskResponse::default()
        };
        assert!(accepted_without_job
            .validate_submission(false, "task")
            .expect_err("HTTP 202 is asynchronous even when the body is legacy-shaped")
            .to_string()
            .contains("without job_id"));
        let accepted_with_job = SchedulerKbTaskResponse {
            http_status: Some(202),
            job_id: Some("job-202".to_string()),
            status: Some("done".to_string()),
            ..SchedulerKbTaskResponse::default()
        };
        assert!(accepted_with_job.validate_submission(false, "task").is_ok());
        assert_eq!(
            accepted_with_job.normalized_state(),
            SchedulerJobState::Waiting,
            "HTTP 202 must be polled even if its JSON body says done"
        );

        let unsupported_success = SchedulerKbTaskResponse {
            http_status: Some(201),
            ..SchedulerKbTaskResponse::default()
        };
        assert!(unsupported_success
            .validate_submission(false, "task")
            .expect_err("the scheduler contract only permits 200 or 202")
            .to_string()
            .contains("unsupported successful HTTP status 201"));

        let error_only = SchedulerKbTaskResponse {
            error: Some("OOM".to_string()),
            ..SchedulerKbTaskResponse::default()
        };
        assert_eq!(error_only.normalized_state(), SchedulerJobState::Failed);
        assert!(error_only
            .validate_submission(false, "task")
            .expect_err("2xx error payload must fail")
            .to_string()
            .contains("OOM"));
    }

    #[test]
    fn rejects_unsafe_path_segments() {
        assert!(validate_path_segment("task", "kb.query.ask").is_ok());
        assert!(validate_path_segment("task", "../x").is_err());
        assert!(validate_path_segment("task", ".").is_err());
        assert!(validate_path_segment("task", "..").is_err());
        assert!(
            validate_path_segment("job", "tts..job").is_ok(),
            "embedded double dots are safe when slash/backslash are forbidden"
        );
        assert!(validate_path_segment("job", "job_abc-123").is_ok());
        assert!(validate_path_segment("job", "job/abc").is_err());
    }

    #[test]
    fn unsafe_scheduler_destinations_are_blocked_before_transport() {
        for endpoint in [
            "https://scheduler.example.test:8090",
            "http://user@127.0.0.1:8090",
            "http://127.0.0.1:8090/admin?target=/models",
            "http://127.0.0.1:8090/#fragment",
            "http://0.0.0.0:8090",
            "http://169.254.169.254:80/latest",
            "http://[fe80::2]:8090",
        ] {
            let client = LocalSchedulerClient::with_base(endpoint, Duration::from_millis(10));
            let error = client
                .models()
                .expect_err("unsafe scheduler URL must be rejected before transport");
            assert!(
                error.to_string().contains("must use an unambiguous"),
                "endpoint={endpoint}, error={error}"
            );
        }
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

        let worker_error = VaultError::LlmUnavailable(
            "local scheduler /jobs/job_abc returned 500 Internal Server Error: {\"detail\":\"worker_error: type must be number\",\"status\":\"error\"}"
                .to_string(),
        );
        assert_eq!(
            classify_scheduler_error(&worker_error),
            Some(SchedulerErrorKind::JobFailed)
        );
    }
}

//! Runtime profiles resolved from local scheduler contracts.
//!
//! `LocalSchedulerClient` is the transport layer. This module is the product-facing
//! planning layer: it turns scheduler DTOs into stable model/task profiles that
//! ContextAdmission, SRAS, and future Windows schedulers can consume.

use super::capacity::{CapacityState, DEFAULT_SCHEDULER_BASE};
use super::scheduler::{
    SchedulerBenchmarkContract, SchedulerCapacitySnapshot, SchedulerContractModel,
    SchedulerModelStatus, SchedulerModels, SchedulerRuntimeTaskSpec,
};
use serde_json::json;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeProviderKind {
    LocalScheduler,
    StaticLocalScheduler,
    Cloud,
}

impl RuntimeProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RuntimeProviderKind::LocalScheduler => "local-scheduler",
            RuntimeProviderKind::StaticLocalScheduler => "static-local-scheduler",
            RuntimeProviderKind::Cloud => "cloud",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelRuntimeProfile {
    pub model_id: String,
    pub provider_kind: RuntimeProviderKind,
    pub endpoint: String,
    pub primary_device: String,
    pub resource_key: String,
    pub worker_kind: String,
    pub service_class: String,
    pub quality_profile: serde_json::Value,
    pub backend_profile: serde_json::Value,
    pub estimated_runtime_ms: u32,
    pub deadline_ms: u32,
    pub sync_allowed: bool,
    pub tested_sync_input_tokens: u32,
    pub tested_async_input_tokens: u32,
    pub recommended_output_tokens: u32,
    pub async_required_above_ms: u32,
    pub max_context_tokens_sync: u32,
    pub max_context_tokens_async: u32,
    pub max_output_tokens_sync: u32,
    pub max_output_tokens_async: u32,
    pub queue_depth: u32,
    pub queue_capacity: u32,
    pub state: CapacityState,
    pub lifecycle: String,
    pub dispatchable: String,
    pub memory_status: String,
    pub dram_available_gb: Option<f64>,
    pub active_models: u32,
    pub revision: u64,
}

impl ModelRuntimeProfile {
    pub fn supports_sync_input_tokens(&self, input_tokens: u32, output_tokens: u32) -> bool {
        self.sync_allowed
            && cap_allows(self.sync_context_cap(), input_tokens)
            && cap_allows(self.sync_output_cap(), output_tokens)
    }

    pub fn supports_async_input_tokens(&self, input_tokens: u32, output_tokens: u32) -> bool {
        cap_allows(self.async_context_cap(), input_tokens)
            && cap_allows(self.async_output_cap(), output_tokens)
    }

    pub fn recommended_sync_output_tokens(&self) -> u32 {
        if self.recommended_output_tokens > 0 {
            self.recommended_output_tokens
        } else if self.max_output_tokens_sync > 0 {
            self.max_output_tokens_sync
        } else if self.max_output_tokens_async > 0 {
            self.max_output_tokens_async.min(256)
        } else {
            256
        }
    }

    pub fn is_ready(&self) -> bool {
        matches!(
            self.state,
            CapacityState::ReadyFast | CapacityState::ReadySlow
        )
    }

    pub fn sync_context_cap(&self) -> u32 {
        prefer_smaller_non_zero(self.tested_sync_input_tokens, self.max_context_tokens_sync)
    }

    pub fn async_context_cap(&self) -> u32 {
        prefer_smaller_non_zero(
            self.tested_async_input_tokens,
            self.max_context_tokens_async,
        )
    }

    pub fn sync_output_cap(&self) -> u32 {
        prefer_smaller_non_zero(self.recommended_output_tokens, self.max_output_tokens_sync)
    }

    pub fn async_output_cap(&self) -> u32 {
        self.max_output_tokens_async
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeTaskProfile {
    pub task_name: String,
    pub stage: String,
    pub model_id: String,
    pub service_class: String,
    pub async_only: bool,
    pub avoid_cold_start: bool,
    pub timeout_ms: u32,
    pub deadline_ms: u32,
    pub context_tokens: u32,
    pub max_output_tokens: u32,
    pub ttl_ms: u32,
}

impl RuntimeTaskProfile {
    pub fn should_submit_async(&self) -> bool {
        self.async_only || self.avoid_cold_start
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeProfileSet {
    pub provider_kind: RuntimeProviderKind,
    pub endpoint: String,
    pub revision: u64,
    pub memory_status: String,
    pub dram_available_gb: Option<f64>,
    pub models: BTreeMap<String, ModelRuntimeProfile>,
    pub tasks: BTreeMap<String, RuntimeTaskProfile>,
}

impl RuntimeProfileSet {
    pub fn model(&self, model_id: &str) -> Option<&ModelRuntimeProfile> {
        self.models.get(model_id)
    }

    pub fn task(&self, task_name: &str) -> Option<&RuntimeTaskProfile> {
        self.tasks.get(task_name)
    }

    pub fn task_model(&self, task_name: &str) -> Option<&ModelRuntimeProfile> {
        let task = self.task(task_name)?;
        self.model(&task.model_id)
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty() && self.tasks.is_empty()
    }
}

pub struct RuntimeProfileResolver;

impl RuntimeProfileResolver {
    pub fn from_scheduler(
        contract: &SchedulerBenchmarkContract,
        models: &SchedulerModels,
        capacity: &SchedulerCapacitySnapshot,
        endpoint: &str,
    ) -> RuntimeProfileSet {
        let endpoint = normalize_endpoint(endpoint);
        let revision = contract
            .revision
            .max(models.revision)
            .max(capacity.revision);
        let memory_status = memory_status(capacity);
        let dram_available_gb = dram_available_gb(capacity);
        let status_by_name: BTreeMap<&str, &SchedulerModelStatus> =
            models.models.iter().map(|m| (m.name.as_str(), m)).collect();

        let model_profiles = contract
            .models
            .iter()
            .map(|m| {
                let status = status_by_name.get(m.name.as_str()).copied();
                let profile = profile_from_contract_model(
                    m,
                    status,
                    RuntimeProviderKind::LocalScheduler,
                    &endpoint,
                    &memory_status,
                    dram_available_gb,
                    capacity.active_models.max(0) as u32,
                    revision,
                );
                (profile.model_id.clone(), profile)
            })
            .collect();

        let tasks = contract
            .runtime_tasks
            .iter()
            .map(|t| {
                let profile = task_profile_from_contract(t);
                (profile.task_name.clone(), profile)
            })
            .collect();

        RuntimeProfileSet {
            provider_kind: RuntimeProviderKind::LocalScheduler,
            endpoint,
            revision,
            memory_status,
            dram_available_gb,
            models: model_profiles,
            tasks,
        }
    }

    pub fn static_local_scheduler_profile(endpoint: &str) -> RuntimeProfileSet {
        let endpoint = normalize_endpoint(endpoint);
        let memory_status = "unknown".to_string();
        let dram_available_gb = None;
        let revision = 0;
        let models = static_local_scheduler_models()
            .into_iter()
            .map(|m| {
                let profile = profile_from_contract_model(
                    &m,
                    None,
                    RuntimeProviderKind::StaticLocalScheduler,
                    &endpoint,
                    &memory_status,
                    dram_available_gb,
                    0,
                    revision,
                );
                (profile.model_id.clone(), profile)
            })
            .collect();
        let tasks = static_local_scheduler_tasks()
            .into_iter()
            .map(|t| {
                let profile = task_profile_from_contract(&t);
                (profile.task_name.clone(), profile)
            })
            .collect();

        RuntimeProfileSet {
            provider_kind: RuntimeProviderKind::StaticLocalScheduler,
            endpoint,
            revision,
            memory_status,
            dram_available_gb,
            models,
            tasks,
        }
    }
}

fn profile_from_contract_model(
    model: &SchedulerContractModel,
    status: Option<&SchedulerModelStatus>,
    provider_kind: RuntimeProviderKind,
    endpoint: &str,
    memory_status: &str,
    dram_available_gb: Option<f64>,
    active_models: u32,
    revision: u64,
) -> ModelRuntimeProfile {
    let queue_depth = status.map(|s| s.queue_depth.max(0) as u32).unwrap_or(0);
    let status_queue_capacity = status
        .map(|s| s.queue_capacity.max(0) as u32)
        .filter(|c| *c > 0);
    let state = status
        .map(|s| CapacityState::parse(&s.state))
        .unwrap_or(CapacityState::Unknown);
    ModelRuntimeProfile {
        model_id: model.name.clone(),
        provider_kind,
        endpoint: endpoint.to_string(),
        primary_device: model.primary_device.clone(),
        resource_key: model.resource_key.clone(),
        worker_kind: model.worker_kind.clone(),
        service_class: model.service_class.clone(),
        quality_profile: model.quality_profile.clone(),
        backend_profile: model.backend_profile.clone(),
        estimated_runtime_ms: model.estimated_runtime_ms,
        deadline_ms: model.deadline_ms,
        sync_allowed: model.sync_allowed,
        tested_sync_input_tokens: calibrated_sync_input_cap(model),
        tested_async_input_tokens: calibrated_async_input_cap(model),
        recommended_output_tokens: calibrated_output_cap(model),
        async_required_above_ms: model.async_required_above_ms,
        max_context_tokens_sync: model.max_context_tokens_sync,
        max_context_tokens_async: model.max_context_tokens_async,
        max_output_tokens_sync: model.max_output_tokens_sync,
        max_output_tokens_async: model.max_output_tokens_async,
        queue_depth,
        queue_capacity: status_queue_capacity.unwrap_or(model.queue_capacity),
        state,
        lifecycle: status
            .map(|s| s.lifecycle.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "UNKNOWN".to_string()),
        dispatchable: status
            .map(|s| s.dispatchable.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "UNKNOWN".to_string()),
        memory_status: memory_status.to_string(),
        dram_available_gb,
        active_models,
        revision,
    }
}

fn task_profile_from_contract(task: &SchedulerRuntimeTaskSpec) -> RuntimeTaskProfile {
    RuntimeTaskProfile {
        task_name: task.name.clone(),
        stage: task.stage.clone(),
        model_id: task.model.clone(),
        service_class: task.service_class.clone(),
        async_only: task.async_only,
        avoid_cold_start: task.avoid_cold_start,
        timeout_ms: task.timeout_ms,
        deadline_ms: task.deadline_ms,
        context_tokens: task.context_tokens,
        max_output_tokens: task.max_output_tokens,
        ttl_ms: task.ttl_ms,
    }
}

fn cap_allows(cap: u32, requested: u32) -> bool {
    cap == 0 || requested <= cap
}

fn prefer_smaller_non_zero(a: u32, b: u32) -> u32 {
    match (a, b) {
        (0, 0) => 0,
        (0, b) => b,
        (a, 0) => a,
        (a, b) => a.min(b),
    }
}

fn calibrated_sync_input_cap(model: &SchedulerContractModel) -> u32 {
    match model.name.as_str() {
        // Local scheduler benchmark evidence: 30B can be resident, but interactive sync
        // should stay around 1K; 3K+ belongs to async/evidence reduction.
        "llm-chat" => prefer_smaller_non_zero(1024, model.max_context_tokens_sync),
        _ => model.max_context_tokens_sync,
    }
}

fn calibrated_async_input_cap(model: &SchedulerContractModel) -> u32 {
    match model.name.as_str() {
        // Keep async bounded even if a future scheduler advertises larger hard
        // caps. Long documents should use evidence packets/map-reduce.
        "llm-chat" => prefer_smaller_non_zero(8192, model.max_context_tokens_async),
        _ => model.max_context_tokens_async,
    }
}

fn calibrated_output_cap(model: &SchedulerContractModel) -> u32 {
    match model.name.as_str() {
        "llm-summary" => prefer_smaller_non_zero(128, model.max_output_tokens_sync),
        "llm-chat" | "vlm" => prefer_smaller_non_zero(256, model.max_output_tokens_sync),
        _ => model.max_output_tokens_sync,
    }
}

fn memory_status(capacity: &SchedulerCapacitySnapshot) -> String {
    if capacity.memory.status.is_empty() {
        "unknown".to_string()
    } else {
        capacity.memory.status.clone()
    }
}

fn dram_available_gb(capacity: &SchedulerCapacitySnapshot) -> Option<f64> {
    capacity.memory.available_gb.or_else(|| {
        let available = capacity.dram_total_gb - capacity.dram_used_gb;
        (available.is_finite() && available > 0.0).then_some(available)
    })
}

fn normalize_endpoint(endpoint: &str) -> String {
    let trimmed = endpoint.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        DEFAULT_SCHEDULER_BASE.to_string()
    } else {
        trimmed.to_string()
    }
}

fn static_local_scheduler_models() -> Vec<SchedulerContractModel> {
    vec![
        static_model(
            "embedding-int8",
            "A100",
            "A100-SVC",
            "llama_http",
            32,
            700,
            "realtime_retrieval",
            94,
            25,
            2000,
            true,
            0,
            2048,
            2048,
            0,
            0,
            json!({
                "runtime": "spacemit-llama-cpp-runtime",
                "api": "/v1/embeddings",
                "a100_path": "ime2",
                "quantization": "Q4_K_M"
            }),
        ),
        static_model(
            "reranker-int8",
            "A100",
            "A100-SVC",
            "llama_http",
            16,
            800,
            "realtime_retrieval",
            94,
            145,
            2000,
            true,
            0,
            2048,
            2048,
            0,
            0,
            json!({
                "runtime": "spacemit-llama-cpp-runtime",
                "api": "/v1/rerank",
                "a100_path": "ime2",
                "quantization": "Q4_0"
            }),
        ),
        static_model(
            "llm-summary",
            "A100",
            "A100-SVC",
            "llama_http",
            32,
            2500,
            "realtime_answer",
            96,
            15000,
            20000,
            true,
            0,
            4096,
            8192,
            256,
            1024,
            chat_backend_profile(),
        ),
        static_model(
            "llm-chat",
            "A100",
            "A100-SVC",
            "llama_http",
            8,
            18000,
            "realtime_answer",
            96,
            1100,
            10000,
            true,
            20000,
            4096,
            8192,
            256,
            1024,
            chat_backend_profile(),
        ),
        static_model(
            "vlm",
            "A100",
            "A100-SVC",
            "llama_http",
            4,
            5500,
            "realtime_vlm_compact",
            65,
            11000,
            20000,
            true,
            0,
            2048,
            4096,
            256,
            1024,
            json!({
                "runtime": "spacemit-llama-cpp-runtime",
                "api": "/v1/chat/completions",
                "media_backend": "smt",
                "a100_path": "ime2+smt",
                "requires_no_think": true
            }),
        ),
    ]
}

#[allow(clippy::too_many_arguments)]
fn static_model(
    name: &str,
    primary_device: &str,
    resource_key: &str,
    worker_kind: &str,
    queue_capacity: u32,
    dram_mb: u32,
    service_class: &str,
    priority: i32,
    estimated_runtime_ms: u32,
    deadline_ms: u32,
    sync_allowed: bool,
    async_required_above_ms: u32,
    max_context_tokens_sync: u32,
    max_context_tokens_async: u32,
    max_output_tokens_sync: u32,
    max_output_tokens_async: u32,
    backend_profile: serde_json::Value,
) -> SchedulerContractModel {
    SchedulerContractModel {
        name: name.to_string(),
        primary_device: primary_device.to_string(),
        fallback_devices: Vec::new(),
        resource_key: resource_key.to_string(),
        worker_kind: worker_kind.to_string(),
        exclusive: true,
        queue_capacity,
        worker_threads: 1,
        dram_gb: dram_mb as f64 / 1000.0,
        service_class: service_class.to_string(),
        priority,
        estimated_runtime_ms,
        deadline_ms,
        sync_allowed,
        async_required_above_ms,
        max_context_tokens_sync,
        max_context_tokens_async,
        max_output_tokens_sync,
        max_output_tokens_async,
        quality_profile: serde_json::Value::Null,
        backend_profile,
    }
}

fn chat_backend_profile() -> serde_json::Value {
    json!({
        "runtime": "spacemit-llama-cpp-runtime",
        "api": "/v1/chat/completions",
        "a100_path": "ime2",
        "requires_no_think": true
    })
}

fn static_local_scheduler_tasks() -> Vec<SchedulerRuntimeTaskSpec> {
    vec![
        static_task(
            "kb.query.embed",
            "query_retrieval",
            "embedding-int8",
            "realtime_retrieval",
            true,
            true,
            60000,
            2000,
            512,
            0,
            900000,
        ),
        static_task(
            "kb.query.rerank",
            "query_retrieval",
            "reranker-int8",
            "realtime_retrieval",
            true,
            true,
            60000,
            2000,
            1024,
            0,
            900000,
        ),
        static_task(
            "kb.query.ask",
            "query_rag_flow",
            "llm-summary",
            "realtime_answer",
            true,
            true,
            120000,
            15000,
            4096,
            128,
            900000,
        ),
        static_task(
            "kb.query.answer",
            "query_answer",
            "llm-summary",
            "realtime_answer",
            true,
            true,
            120000,
            20000,
            4096,
            128,
            900000,
        ),
        static_task(
            "kb.query.vlm_extract",
            "query_vlm",
            "vlm",
            "realtime_vlm_compact",
            false,
            true,
            120000,
            20000,
            2048,
            256,
            0,
        ),
        static_task(
            "kb.document.summary",
            "ingest_summary",
            "llm-summary",
            "user_async",
            true,
            true,
            300000,
            0,
            4096,
            512,
            900000,
        ),
    ]
}

#[allow(clippy::too_many_arguments)]
fn static_task(
    name: &str,
    stage: &str,
    model: &str,
    service_class: &str,
    async_only: bool,
    avoid_cold_start: bool,
    timeout_ms: u32,
    deadline_ms: u32,
    context_tokens: u32,
    max_output_tokens: u32,
    ttl_ms: u32,
) -> SchedulerRuntimeTaskSpec {
    SchedulerRuntimeTaskSpec {
        name: name.to_string(),
        stage: stage.to_string(),
        model: model.to_string(),
        service_class: service_class.to_string(),
        description: String::new(),
        async_only,
        avoid_cold_start,
        timeout_ms,
        deadline_ms,
        context_tokens,
        max_output_tokens,
        ttl_ms,
    }
}

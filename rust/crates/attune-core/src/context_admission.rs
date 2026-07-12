//! Final-prompt admission before any LLM/VLM call.
//!
//! This layer is deliberately stricter than nominal model context windows. A
//! 1M cloud window or a 4K local scheduler hard cap does not mean the product
//! should stuff whole documents into a single request. Callers should run this
//! after retrieval, compression, redaction, and final message assembly.
//! By default, final input admission is capped at 65,536 estimated tokens even
//! if a provider advertises a larger context window.

use crate::context_compress::estimate_tokens;
use crate::edge_cloud::{ModelRuntimeProfile, RuntimeProviderKind, RuntimeTaskProfile};
use crate::llm::ChatMessage;

pub const CONTEXT_ADMISSION_MAX_INPUT_TOKENS_ENV: &str =
    "ATTUNE_CONTEXT_ADMISSION_MAX_INPUT_TOKENS";
pub const DEFAULT_CONTEXT_ADMISSION_MAX_INPUT_TOKENS: u32 = 65_536;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionLatencyClass {
    Interactive,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionReason {
    FitsSync,
    TaskAsyncOnly,
    BackgroundLatencyClass,
    ModelSyncDisabled,
    ContextTooLargeForSync,
    OutputTooLargeForSync,
    ContextTooLargeForLocalAsync,
    OutputTooLargeForLocalAsync,
    ContextTooLargeForProvider,
    OutputTooLargeForProvider,
    EmptyMessages,
}

impl AdmissionReason {
    pub fn as_str(self) -> &'static str {
        match self {
            AdmissionReason::FitsSync => "fits-sync",
            AdmissionReason::TaskAsyncOnly => "task-async-only",
            AdmissionReason::BackgroundLatencyClass => "background-latency-class",
            AdmissionReason::ModelSyncDisabled => "model-sync-disabled",
            AdmissionReason::ContextTooLargeForSync => "context-too-large-for-sync",
            AdmissionReason::OutputTooLargeForSync => "output-too-large-for-sync",
            AdmissionReason::ContextTooLargeForLocalAsync => "context-too-large-for-local-async",
            AdmissionReason::OutputTooLargeForLocalAsync => "output-too-large-for-local-async",
            AdmissionReason::ContextTooLargeForProvider => "context-too-large-for-provider",
            AdmissionReason::OutputTooLargeForProvider => "output-too-large-for-provider",
            AdmissionReason::EmptyMessages => "empty-messages",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ContextAdmissionRequest<'a> {
    pub messages: &'a [ChatMessage],
    pub runtime: &'a ModelRuntimeProfile,
    pub task: Option<&'a RuntimeTaskProfile>,
    pub desired_output_tokens: Option<u32>,
    pub latency_class: AdmissionLatencyClass,
}

impl<'a> ContextAdmissionRequest<'a> {
    pub fn interactive(messages: &'a [ChatMessage], runtime: &'a ModelRuntimeProfile) -> Self {
        ContextAdmissionRequest {
            messages,
            runtime,
            task: None,
            desired_output_tokens: None,
            latency_class: AdmissionLatencyClass::Interactive,
        }
    }

    pub fn with_task(mut self, task: &'a RuntimeTaskProfile) -> Self {
        self.task = Some(task);
        self
    }

    pub fn with_desired_output_tokens(mut self, tokens: u32) -> Self {
        self.desired_output_tokens = Some(tokens);
        self
    }

    pub fn background(mut self) -> Self {
        self.latency_class = AdmissionLatencyClass::Background;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextAdmissionDecision {
    AdmitSync(AdmittedContext),
    SubmitAsync(AsyncContext),
    UseCloudIfAllowed(CloudFallbackContext),
    Reject(AdmissionRejection),
}

impl ContextAdmissionDecision {
    pub fn reason(&self) -> AdmissionReason {
        match self {
            ContextAdmissionDecision::AdmitSync(ctx) => ctx.reason,
            ContextAdmissionDecision::SubmitAsync(ctx) => ctx.reason,
            ContextAdmissionDecision::UseCloudIfAllowed(ctx) => ctx.reason,
            ContextAdmissionDecision::Reject(ctx) => ctx.reason,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmittedContext {
    pub model_id: String,
    pub service_class: String,
    pub estimated_input_tokens: u32,
    pub max_output_tokens: u32,
    pub context_tokens: u32,
    pub reason: AdmissionReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsyncContext {
    pub model_id: String,
    pub service_class: String,
    pub estimated_input_tokens: u32,
    pub max_output_tokens: u32,
    pub context_tokens: u32,
    pub ttl_ms: Option<u32>,
    pub reason: AdmissionReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudFallbackContext {
    pub model_id: String,
    pub estimated_input_tokens: u32,
    pub max_output_tokens: u32,
    pub reason: AdmissionReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionRejection {
    pub model_id: String,
    pub estimated_input_tokens: u32,
    pub max_output_tokens: u32,
    pub reason: AdmissionReason,
}

pub fn admit_context(req: ContextAdmissionRequest<'_>) -> ContextAdmissionDecision {
    if req.messages.is_empty() {
        return ContextAdmissionDecision::Reject(AdmissionRejection {
            model_id: req.runtime.model_id.clone(),
            estimated_input_tokens: 0,
            max_output_tokens: requested_output_tokens(req),
            reason: AdmissionReason::EmptyMessages,
        });
    }

    let estimated_input_tokens = estimate_chat_tokens(req.messages);
    let max_output_tokens = requested_output_tokens(req);

    let async_context_cap = effective_context_cap(req.runtime.async_context_cap());
    if !cap_allows(async_context_cap, estimated_input_tokens) {
        return async_overflow(req, estimated_input_tokens, max_output_tokens);
    }
    if !cap_allows(req.runtime.async_output_cap(), max_output_tokens) {
        return output_overflow(req, estimated_input_tokens, max_output_tokens);
    }

    if let Some(reason) = sync_blocker(req, estimated_input_tokens, max_output_tokens) {
        return ContextAdmissionDecision::SubmitAsync(AsyncContext {
            model_id: req.runtime.model_id.clone(),
            service_class: service_class(req).to_string(),
            estimated_input_tokens,
            max_output_tokens,
            context_tokens: estimated_input_tokens,
            ttl_ms: req.task.and_then(|t| (t.ttl_ms > 0).then_some(t.ttl_ms)),
            reason,
        });
    }

    ContextAdmissionDecision::AdmitSync(AdmittedContext {
        model_id: req.runtime.model_id.clone(),
        service_class: service_class(req).to_string(),
        estimated_input_tokens,
        max_output_tokens,
        context_tokens: estimated_input_tokens,
        reason: AdmissionReason::FitsSync,
    })
}

pub fn estimate_chat_tokens(messages: &[ChatMessage]) -> u32 {
    let total = messages
        .iter()
        .map(|m| estimate_tokens(&m.role) + estimate_tokens(&m.content) + 8)
        .sum::<usize>()
        + 4;
    total.min(u32::MAX as usize) as u32
}

fn sync_blocker(
    req: ContextAdmissionRequest<'_>,
    estimated_input_tokens: u32,
    max_output_tokens: u32,
) -> Option<AdmissionReason> {
    if req.latency_class == AdmissionLatencyClass::Background {
        return Some(AdmissionReason::BackgroundLatencyClass);
    }
    if req.task.is_some_and(|t| t.async_only) {
        return Some(AdmissionReason::TaskAsyncOnly);
    }
    if !req.runtime.sync_allowed {
        return Some(AdmissionReason::ModelSyncDisabled);
    }
    let sync_context_cap = effective_context_cap(req.runtime.sync_context_cap());
    if !cap_allows(sync_context_cap, estimated_input_tokens) {
        return Some(AdmissionReason::ContextTooLargeForSync);
    }
    if !cap_allows(req.runtime.sync_output_cap(), max_output_tokens) {
        return Some(AdmissionReason::OutputTooLargeForSync);
    }
    None
}

fn requested_output_tokens(req: ContextAdmissionRequest<'_>) -> u32 {
    if let Some(tokens) = req.desired_output_tokens {
        return tokens;
    }
    if let Some(task) = req.task {
        if task.max_output_tokens > 0 {
            return task.max_output_tokens;
        }
    }
    req.runtime.recommended_sync_output_tokens()
}

fn service_class(req: ContextAdmissionRequest<'_>) -> &str {
    req.task
        .map(|t| t.service_class.as_str())
        .unwrap_or(req.runtime.service_class.as_str())
}

fn async_overflow(
    req: ContextAdmissionRequest<'_>,
    estimated_input_tokens: u32,
    max_output_tokens: u32,
) -> ContextAdmissionDecision {
    let reason = AdmissionReason::ContextTooLargeForLocalAsync;
    if req.runtime.provider_kind == RuntimeProviderKind::Cloud {
        ContextAdmissionDecision::Reject(AdmissionRejection {
            model_id: req.runtime.model_id.clone(),
            estimated_input_tokens,
            max_output_tokens,
            reason: AdmissionReason::ContextTooLargeForProvider,
        })
    } else {
        ContextAdmissionDecision::UseCloudIfAllowed(CloudFallbackContext {
            model_id: req.runtime.model_id.clone(),
            estimated_input_tokens,
            max_output_tokens,
            reason,
        })
    }
}

fn output_overflow(
    req: ContextAdmissionRequest<'_>,
    estimated_input_tokens: u32,
    max_output_tokens: u32,
) -> ContextAdmissionDecision {
    let reason = AdmissionReason::OutputTooLargeForLocalAsync;
    if req.runtime.provider_kind == RuntimeProviderKind::Cloud {
        ContextAdmissionDecision::Reject(AdmissionRejection {
            model_id: req.runtime.model_id.clone(),
            estimated_input_tokens,
            max_output_tokens,
            reason: AdmissionReason::OutputTooLargeForProvider,
        })
    } else {
        ContextAdmissionDecision::UseCloudIfAllowed(CloudFallbackContext {
            model_id: req.runtime.model_id.clone(),
            estimated_input_tokens,
            max_output_tokens,
            reason,
        })
    }
}

fn cap_allows(cap: u32, requested: u32) -> bool {
    cap == 0 || requested <= cap
}

fn effective_context_cap(runtime_cap: u32) -> u32 {
    prefer_smaller_non_zero(runtime_cap, product_context_cap())
}

fn product_context_cap() -> u32 {
    std::env::var(CONTEXT_ADMISSION_MAX_INPUT_TOKENS_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<u32>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_CONTEXT_ADMISSION_MAX_INPUT_TOKENS)
}

fn prefer_smaller_non_zero(a: u32, b: u32) -> u32 {
    match (a, b) {
        (0, 0) => 0,
        (0, b) => b,
        (a, 0) => a,
        (a, b) => a.min(b),
    }
}

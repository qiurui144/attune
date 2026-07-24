//! ContextAdmission tests for local scheduler/local and cloud runtime profiles.

use attune_core::context_admission::{
    admit_context, AdmissionReason, ContextAdmissionDecision, ContextAdmissionRequest,
    CONTEXT_ADMISSION_MAX_INPUT_TOKENS_ENV,
};
use attune_core::edge_cloud::{
    CapacityState, ModelRuntimeProfile, RuntimeProfileResolver, RuntimeProviderKind,
};
use attune_core::llm::ChatMessage;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn edge_scheduler_30b_small_prompt_admits_sync_with_product_output_cap() {
    let profiles = RuntimeProfileResolver::static_local_scheduler_profile("");
    let chat = profiles.model("llm-chat").unwrap();
    let messages = vec![ChatMessage::user("short operational question")];

    let decision = admit_context(ContextAdmissionRequest::interactive(&messages, chat));

    match decision {
        ContextAdmissionDecision::AdmitSync(ctx) => {
            assert_eq!(ctx.model_id, "llm-chat");
            assert_eq!(ctx.max_output_tokens, 256);
            assert_eq!(ctx.reason, AdmissionReason::FitsSync);
        }
        other => panic!("expected sync admission, got {other:?}"),
    }
}

#[test]
fn edge_scheduler_30b_over_product_sync_cap_routes_async_before_scheduler() {
    let profiles = RuntimeProfileResolver::static_local_scheduler_profile("");
    let chat = profiles.model("llm-chat").unwrap();
    assert_eq!(chat.sync_context_cap(), 1024);
    let messages = vec![ChatMessage::user(&"长".repeat(1500))];

    let decision = admit_context(ContextAdmissionRequest::interactive(&messages, chat));

    match decision {
        ContextAdmissionDecision::SubmitAsync(ctx) => {
            assert_eq!(ctx.reason, AdmissionReason::ContextTooLargeForSync);
            assert!(ctx.estimated_input_tokens > chat.sync_context_cap());
            assert!(ctx.estimated_input_tokens <= chat.async_context_cap());
        }
        other => panic!("expected local async admission, got {other:?}"),
    }
}

#[test]
fn edge_scheduler_30b_over_async_cap_asks_caller_to_try_cloud_if_allowed() {
    let profiles = RuntimeProfileResolver::static_local_scheduler_profile("");
    let chat = profiles.model("llm-chat").unwrap();
    let messages = vec![ChatMessage::user(&"长".repeat(8000))];

    let decision = admit_context(ContextAdmissionRequest::interactive(&messages, chat));

    match decision {
        ContextAdmissionDecision::UseCloudIfAllowed(ctx) => {
            assert_eq!(ctx.reason, AdmissionReason::ContextTooLargeForLocalAsync);
            assert!(ctx.estimated_input_tokens > chat.async_context_cap());
        }
        other => panic!("expected cloud fallback decision, got {other:?}"),
    }
}

#[test]
fn kb_query_ask_interactive_small_prompt_admits_sync_and_uses_task_output_cap() {
    let profiles = RuntimeProfileResolver::static_local_scheduler_profile("");
    let summary = profiles.model("llm-summary").unwrap();
    let ask = profiles.task("kb.query.ask").unwrap();
    let messages = vec![ChatMessage::user("question with compact cited evidence")];

    let decision =
        admit_context(ContextAdmissionRequest::interactive(&messages, summary).with_task(ask));

    match decision {
        ContextAdmissionDecision::AdmitSync(ctx) => {
            assert_eq!(ctx.reason, AdmissionReason::FitsSync);
            assert_eq!(ctx.max_output_tokens, 128);
            assert_eq!(ctx.service_class, "realtime_answer");
        }
        other => panic!("expected sync task admission, got {other:?}"),
    }
}

#[test]
fn kb_query_ask_interactive_large_prompt_still_routes_async() {
    let profiles = RuntimeProfileResolver::static_local_scheduler_profile("");
    let summary = profiles.model("llm-summary").unwrap();
    let ask = profiles.task("kb.query.ask").unwrap();
    let messages = vec![ChatMessage::user(&"长".repeat(5000))];

    let decision =
        admit_context(ContextAdmissionRequest::interactive(&messages, summary).with_task(ask));

    match decision {
        ContextAdmissionDecision::SubmitAsync(ctx) => {
            assert_eq!(ctx.reason, AdmissionReason::ContextTooLargeForSync);
            assert_eq!(ctx.max_output_tokens, 128);
            assert_eq!(ctx.ttl_ms, Some(900000));
        }
        other => panic!("expected async task admission for large prompt, got {other:?}"),
    }
}

#[test]
fn cloud_profile_still_enforces_final_context_cap() {
    let cloud = cloud_profile("gpt-4o-mini", 128000, 4096);
    let admitted = vec![ChatMessage::user(&"长".repeat(50000))];
    let too_large = vec![ChatMessage::user(&"长".repeat(120000))];

    let decision = admit_context(ContextAdmissionRequest::interactive(&admitted, &cloud));
    assert!(
        matches!(decision, ContextAdmissionDecision::AdmitSync(_)),
        "cloud 128K should admit bounded final prompt, got {decision:?}"
    );

    let decision = admit_context(ContextAdmissionRequest::interactive(&too_large, &cloud));
    match decision {
        ContextAdmissionDecision::Reject(ctx) => {
            assert_eq!(ctx.reason, AdmissionReason::ContextTooLargeForProvider);
        }
        other => panic!("expected provider cap rejection, got {other:?}"),
    }
}

#[test]
fn cloud_1m_window_still_respects_product_final_prompt_cap() {
    let _lock = ENV_LOCK.lock().unwrap();
    let _guard = EnvGuard::unset(CONTEXT_ADMISSION_MAX_INPUT_TOKENS_ENV);
    let cloud = cloud_profile("gemini-2.5-flash", 1_000_000, 8192);
    let messages = vec![ChatMessage::user(&"长".repeat(70_000))];

    let decision = admit_context(ContextAdmissionRequest::interactive(&messages, &cloud));

    match decision {
        ContextAdmissionDecision::Reject(ctx) => {
            assert_eq!(ctx.reason, AdmissionReason::ContextTooLargeForProvider);
            assert!(ctx.estimated_input_tokens > 65_536);
            assert!(ctx.estimated_input_tokens < cloud.async_context_cap());
        }
        other => panic!("expected product cap rejection before 1M provider window, got {other:?}"),
    }
}

#[test]
fn product_final_prompt_cap_can_be_raised_for_explicit_eval_runs() {
    let _lock = ENV_LOCK.lock().unwrap();
    let _guard = EnvGuard::set(CONTEXT_ADMISSION_MAX_INPUT_TOKENS_ENV, "120000");
    let cloud = cloud_profile("gemini-2.5-flash", 1_000_000, 8192);
    let messages = vec![ChatMessage::user(&"长".repeat(70_000))];

    let decision = admit_context(ContextAdmissionRequest::interactive(&messages, &cloud));

    assert!(
        matches!(decision, ContextAdmissionDecision::AdmitSync(_)),
        "explicit eval cap should allow the bounded 1M-window cloud prompt, got {decision:?}"
    );
}

#[test]
fn empty_messages_reject() {
    let profiles = RuntimeProfileResolver::static_local_scheduler_profile("");
    let chat = profiles.model("llm-chat").unwrap();

    let decision = admit_context(ContextAdmissionRequest::interactive(&[], chat));

    assert!(matches!(
        decision,
        ContextAdmissionDecision::Reject(ref ctx) if ctx.reason == AdmissionReason::EmptyMessages
    ));
}

struct EnvGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvGuard {
    fn unset(key: &'static str) -> Self {
        let previous = std::env::var(key).ok();
        std::env::remove_var(key);
        EnvGuard { key, previous }
    }

    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        std::env::set_var(key, value);
        EnvGuard { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.previous.as_ref() {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

fn cloud_profile(model_id: &str, context_cap: u32, output_cap: u32) -> ModelRuntimeProfile {
    ModelRuntimeProfile {
        model_id: model_id.to_string(),
        model_class: None,
        preferred_size: None,
        fallback_sizes: Vec::new(),
        sync_sla_ms: None,
        provider_kind: RuntimeProviderKind::Cloud,
        endpoint: "https://api.example.invalid".to_string(),
        primary_device: "cloud".to_string(),
        resource_key: "cloud".to_string(),
        worker_kind: "api".to_string(),
        service_class: "realtime_answer".to_string(),
        quality_profile: serde_json::Value::Null,
        backend_profile: serde_json::Value::Null,
        estimated_runtime_ms: 1000,
        deadline_ms: 30000,
        sync_allowed: true,
        tested_sync_input_tokens: context_cap,
        tested_async_input_tokens: context_cap,
        recommended_output_tokens: output_cap,
        async_required_above_ms: 0,
        max_context_tokens_sync: context_cap,
        max_context_tokens_async: context_cap,
        max_output_tokens_sync: output_cap,
        max_output_tokens_async: output_cap,
        queue_depth: 0,
        queue_capacity: 0,
        state: CapacityState::ReadyFast,
        lifecycle: "READY".to_string(),
        dispatchable: "FREE".to_string(),
        memory_status: "ok".to_string(),
        dram_available_gb: None,
        active_models: 0,
        revision: 0,
    }
}

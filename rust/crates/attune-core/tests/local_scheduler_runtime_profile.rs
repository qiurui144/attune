//! RuntimeProfile resolver/cache tests for the local scheduler boundary.

use attune_core::edge_cloud::{
    CapacityState, RuntimeProfileCache, RuntimeProfileResolver, RuntimeProviderKind,
    SchedulerBenchmarkContract, SchedulerCapacitySnapshot, SchedulerModels,
};
use attune_core::error::VaultError;
use std::cell::Cell;
use std::time::{Duration, Instant};

const CONTRACT_JSON: &str = include_str!("fixtures/local_scheduler/benchmark_contract.json");
const MODELS_JSON: &str = include_str!("fixtures/local_scheduler/models.json");
const CAPACITY_JSON: &str = include_str!("fixtures/local_scheduler/capacity.json");

fn fixture_profile(endpoint: &str) -> attune_core::edge_cloud::RuntimeProfileSet {
    let contract: SchedulerBenchmarkContract = serde_json::from_str(CONTRACT_JSON).unwrap();
    let models: SchedulerModels = serde_json::from_str(MODELS_JSON).unwrap();
    let capacity: SchedulerCapacitySnapshot = serde_json::from_str(CAPACITY_JSON).unwrap();
    RuntimeProfileResolver::from_scheduler(&contract, &models, &capacity, endpoint)
}

#[test]
fn resolves_profiles_from_scheduler_contract_models_and_capacity() {
    let set = fixture_profile("http://127.0.0.1:8090/");

    assert_eq!(set.provider_kind, RuntimeProviderKind::LocalScheduler);
    assert_eq!(set.revision, 4827);
    assert_eq!(set.memory_status, "ok");
    assert_eq!(set.dram_available_gb, Some(23.5));

    let summary = set.model("llm-summary").unwrap();
    assert_eq!(summary.state, CapacityState::ReadyFast);
    assert_eq!(summary.max_context_tokens_sync, 4096);
    assert_eq!(summary.max_context_tokens_async, 8192);
    assert_eq!(summary.max_output_tokens_sync, 256);
    assert_eq!(summary.recommended_output_tokens, 128);
    assert!(summary.supports_sync_input_tokens(4096, 128));
    assert!(!summary.supports_sync_input_tokens(4097, 128));
    assert_eq!(
        summary.backend_profile["requires_no_think"].as_bool(),
        Some(true)
    );

    let chat = set.model("llm-chat").unwrap();
    assert_eq!(chat.state, CapacityState::ReadySlow);
    assert_eq!(chat.queue_depth, 2);
    assert_eq!(chat.tested_sync_input_tokens, 1024);
    assert!(!chat.supports_sync_input_tokens(1025, 256));

    let vlm = set.model("vlm").unwrap();
    assert_eq!(vlm.state, CapacityState::Queued);
    assert_eq!(vlm.queue_depth, 2);
    assert_eq!(
        vlm.quality_profile["benchmark"].as_str(),
        Some("vlm_document_extraction")
    );

    let ask = set.task("kb.query.ask").unwrap();
    assert_eq!(ask.model_id, "llm-summary");
    assert_eq!(ask.context_tokens, 4096);
    assert_eq!(ask.max_output_tokens, 128);
    assert!(ask.should_submit_async());
    assert_eq!(
        set.task_model("kb.query.ask").unwrap().model_id,
        "llm-summary"
    );
}

#[test]
fn runtime_profile_cache_uses_ttl_then_refreshes() {
    let now = Instant::now();
    let calls = Cell::new(0);
    let mut cache = RuntimeProfileCache::new(Duration::from_secs(10));

    let first = cache.get_or_refresh_with("http://127.0.0.1:8090/", now, || {
        calls.set(calls.get() + 1);
        Ok(fixture_profile("http://127.0.0.1:8090/"))
    });
    let second = cache.get_or_refresh_with(
        "http://127.0.0.1:8090",
        now + Duration::from_secs(5),
        || {
            calls.set(calls.get() + 1);
            Ok(fixture_profile("http://127.0.0.1:8090/"))
        },
    );
    let third = cache.get_or_refresh_with(
        "http://127.0.0.1:8090",
        now + Duration::from_secs(11),
        || {
            calls.set(calls.get() + 1);
            Ok(fixture_profile("http://127.0.0.1:8090/"))
        },
    );

    assert_eq!(calls.get(), 2);
    assert_eq!(first.revision, second.revision);
    assert_eq!(third.provider_kind, RuntimeProviderKind::LocalScheduler);
}

#[test]
fn runtime_profile_cache_falls_back_to_stale_then_static() {
    let now = Instant::now();
    let mut cache = RuntimeProfileCache::new(Duration::from_millis(1));
    let first = cache.get_or_refresh_with("http://127.0.0.1:8090", now, || {
        Ok(fixture_profile("http://127.0.0.1:8090"))
    });
    let stale = cache.get_or_refresh_with(
        "http://127.0.0.1:8090",
        now + Duration::from_secs(1),
        || Err(VaultError::LlmUnavailable("scheduler down".into())),
    );
    assert_eq!(stale.provider_kind, RuntimeProviderKind::LocalScheduler);
    assert_eq!(stale.revision, first.revision);

    let static_fallback = cache.get_or_refresh_with(
        "http://127.0.0.1:19090",
        now + Duration::from_secs(2),
        || Err(VaultError::LlmUnavailable("scheduler down".into())),
    );
    assert_eq!(
        static_fallback.provider_kind,
        RuntimeProviderKind::StaticLocalScheduler
    );
    assert_eq!(static_fallback.endpoint, "http://127.0.0.1:19090");
}

#[test]
fn static_local_scheduler_profile_profile_contains_required_models_and_task_defaults() {
    let set = RuntimeProfileResolver::static_local_scheduler_profile("");

    assert_eq!(set.provider_kind, RuntimeProviderKind::StaticLocalScheduler);
    assert_eq!(set.endpoint, "http://127.0.0.1:8090");
    for model in [
        "embedding-int8",
        "reranker-int8",
        "llm-summary",
        "llm-chat",
        "vlm",
    ] {
        assert!(set.model(model).is_some(), "{model} must exist");
    }

    let chat = set.model("llm-chat").unwrap();
    assert_eq!(chat.max_context_tokens_sync, 4096);
    assert_eq!(chat.max_context_tokens_async, 8192);
    assert_eq!(chat.tested_sync_input_tokens, 1024);
    assert_eq!(chat.max_output_tokens_sync, 256);
    assert_eq!(chat.max_output_tokens_async, 1024);
    assert!(chat.supports_sync_input_tokens(1024, 256));
    assert!(!chat.supports_sync_input_tokens(1025, 256));
    assert!(chat.supports_async_input_tokens(8192, 1024));
    assert!(!chat.supports_async_input_tokens(8193, 1024));

    let ask = set.task("kb.query.ask").unwrap();
    assert_eq!(ask.model_id, "llm-summary");
    assert_eq!(ask.context_tokens, 4096);
    assert_eq!(ask.max_output_tokens, 128);
    assert_eq!(ask.ttl_ms, 900000);
}

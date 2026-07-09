//! RuntimeProfile resolver tests for the local scheduler pilot.

use attune_core::edge_cloud::{
    CapacityState, SchedulerBenchmarkContract, SchedulerCapacitySnapshot, SchedulerModels,
    RuntimeProfileResolver, RuntimeProviderKind,
};

#[test]
fn resolves_profiles_from_scheduler_contract_models_and_capacity() {
    let contract: SchedulerBenchmarkContract = serde_json::from_str(
        r#"{
          "contract_version": "local-scheduler-stress-v1",
          "revision": 10,
          "runtime_tasks": [
            {
              "name": "kb.query.ask",
              "stage": "query_rag_flow",
              "model": "llm-summary",
              "service_class": "realtime_answer",
              "async_only": true,
              "avoid_cold_start": true,
              "timeout_ms": 120000,
              "deadline_ms": 15000,
              "context_tokens": 4096,
              "max_output_tokens": 128,
              "ttl_ms": 900000
            }
          ],
          "models": [
            {
              "name": "llm-summary",
              "primary_device": "A100",
              "resource_key": "A100-SVC",
              "worker_kind": "llama_http",
              "queue_capacity": 32,
              "dram_gb": 2.5,
              "service_class": "realtime_answer",
              "priority": 96,
              "estimated_runtime_ms": 15000,
              "deadline_ms": 20000,
              "sync_allowed": true,
              "max_context_tokens_sync": 4096,
              "max_context_tokens_async": 8192,
              "max_output_tokens_sync": 256,
              "max_output_tokens_async": 1024,
              "backend_profile": {"runtime": "spacemit-llama-cpp-runtime", "requires_no_think": true}
            },
            {
              "name": "vlm",
              "primary_device": "A100",
              "resource_key": "A100-SVC",
              "worker_kind": "llama_http",
              "queue_capacity": 4,
              "dram_gb": 5.5,
              "service_class": "realtime_vlm_compact",
              "priority": 65,
              "estimated_runtime_ms": 11000,
              "deadline_ms": 20000,
              "sync_allowed": true,
              "max_context_tokens_sync": 2048,
              "max_context_tokens_async": 4096,
              "max_output_tokens_sync": 256,
              "max_output_tokens_async": 1024,
              "quality_profile": {
                "benchmark": "vlm_document_extraction",
                "field_accuracy": 1.0
              },
              "backend_profile": {"runtime": "spacemit-llama-cpp-runtime", "media_backend": "smt"}
            }
          ]
        }"#,
    )
    .unwrap();
    let models: SchedulerModels = serde_json::from_str(
        r#"{
          "models": [
            {
              "name": "llm-summary",
              "state": "READY_FAST",
              "lifecycle": "READY",
              "dispatchable": "FREE",
              "queue_depth": 0,
              "queue_capacity": 32,
              "state_revision": 11
            },
            {
              "name": "vlm",
              "state": "QUEUED",
              "lifecycle": "READY",
              "dispatchable": "BUSY",
              "queue_depth": 2,
              "queue_capacity": 4,
              "state_revision": 11
            }
          ],
          "revision": 11
        }"#,
    )
    .unwrap();
    let capacity: SchedulerCapacitySnapshot = serde_json::from_str(
        r#"{
          "dram_used_gb": 8.5,
          "dram_reserved_gb": 7.0,
          "dram_total_gb": 32.0,
          "memory": {"status": "ok", "available_gb": 21.5},
          "active_models": 2,
          "revision": 12
        }"#,
    )
    .unwrap();

    let set =
        RuntimeProfileResolver::from_scheduler(&contract, &models, &capacity, "http://127.0.0.1:8090/");

    assert_eq!(set.provider_kind, RuntimeProviderKind::LocalScheduler);
    assert_eq!(set.revision, 12);
    assert_eq!(set.memory_status, "ok");
    assert_eq!(set.dram_available_gb, Some(21.5));

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

//! Fixture tests for the actual local-scheduler contract surface.

use attune_core::edge_cloud::{
    SchedulerBenchmarkContract, SchedulerCapacitySnapshot, SchedulerJobStatus, SchedulerKbTaskResponse, SchedulerModels,
};

#[test]
fn parses_benchmark_contract_context_caps_and_runtime_tasks() {
    let contract: SchedulerBenchmarkContract = serde_json::from_str(
        r#"{
          "contract_version": "local-scheduler-stress-v1",
          "revision": 4827,
          "queue_order": ["resource_key", "priority_desc", "deadline_ms_asc"],
          "request_fields": ["inputs", "timeout_ms", "context_tokens", "max_output_tokens"],
          "endpoints": {
            "kb_task": "POST /kb/tasks/{task}",
            "job_status": "GET /jobs/{id}"
          },
          "runtime_tasks": [
            {
              "name": "kb.query.ask",
              "stage": "query_rag_flow",
              "model": "llm-summary",
              "service_class": "realtime_answer",
              "description": "Application-facing bounded KB answer flow",
              "async_only": true,
              "avoid_cold_start": true,
              "timeout_ms": 120000,
              "deadline_ms": 15000,
              "context_tokens": 4096,
              "max_output_tokens": 128,
              "ttl_ms": 900000
            }
          ],
          "service_classes": [
            {"name": "long_context", "priority": 30, "infer_allowed": true, "sync_allowed": false}
          ],
          "models": [
            {
              "name": "llm-chat",
              "primary_device": "A100",
              "fallback_devices": [],
              "resource_key": "A100-SVC",
              "worker_kind": "llama_http",
              "exclusive": true,
              "queue_capacity": 4,
              "worker_threads": 1,
              "dram_gb": 18.0,
              "service_class": "realtime_answer",
              "priority": 80,
              "estimated_runtime_ms": 1100,
              "deadline_ms": 30000,
              "sync_allowed": true,
              "async_required_above_ms": 20000,
              "max_context_tokens_sync": 4096,
              "max_context_tokens_async": 8192,
              "max_output_tokens_sync": 256,
              "max_output_tokens_async": 1024,
              "quality_profile": "qwen3-30b-a3b-q4_0",
              "backend_profile": {"runtime": "spacemit-llama-cpp-runtime", "requires_no_think": true}
            }
          ],
          "async_jobs": {
            "max_active_jobs": 256,
            "active": 2,
            "done_retained": 4,
            "canceled_retained": 1,
            "expired_retained": 0
          },
          "metrics": ["local_scheduler_context_tokens", "local_scheduler_async_jobs"],
          "errors": {"async_required": 409},
          "notes": ["cold-start samples must be bucketed by startup_state"]
        }"#,
    )
    .unwrap();

    assert_eq!(contract.contract_version, "local-scheduler-stress-v1");
    assert_eq!(contract.revision, 4827);
    assert_eq!(contract.models[0].name, "llm-chat");
    assert_eq!(contract.models[0].max_context_tokens_sync, 4096);
    assert_eq!(contract.models[0].max_context_tokens_async, 8192);
    assert_eq!(contract.models[0].max_output_tokens_async, 1024);
    assert_eq!(
        contract.models[0].backend_profile["requires_no_think"].as_bool(),
        Some(true)
    );
    assert_eq!(contract.runtime_tasks[0].name, "kb.query.ask");
    assert!(contract.runtime_tasks[0].async_only);
    assert_eq!(contract.runtime_tasks[0].context_tokens, 4096);
    assert_eq!(contract.service_classes[0].name, "long_context");
    assert_eq!(contract.service_classes[0].sync_allowed, Some(false));
    assert_eq!(contract.async_jobs.max_active_jobs, 256);
}

#[test]
fn parses_models_capacity_job_and_kb_task_shapes() {
    let models: SchedulerModels = serde_json::from_str(
        r#"{
          "models": [
            {
              "name": "llm-chat",
              "state": "READY_SLOW",
              "lifecycle": "READY",
              "dispatchable": "BUSY",
              "devices": {"A100": {"role": "primary", "status": "busy", "in_flight": 1}},
              "exclusive": true,
              "queue_depth": 2,
              "queue_capacity": 4,
              "last_latency_ms": 980.0,
              "p50_latency_ms": 1000.0,
              "p99_latency_ms": 1800.0,
              "last_success_ts": "2026-04-21T12:34:56Z",
              "state_revision": 4827
            }
          ],
          "revision": 4827
        }"#,
    )
    .unwrap();
    assert_eq!(models.models[0].state, "READY_SLOW");
    assert_eq!(models.models[0].queue_depth, 2);
    assert_eq!(models.models[0].p50_latency_ms, Some(1000.0));

    let capacity: SchedulerCapacitySnapshot = serde_json::from_str(
        r#"{
          "clusters": {
            "A100": {
              "cores_total": 8,
              "cores_used": 1,
              "busy_ratio": 0.125,
              "ep_active": true,
              "ep_session_holder": "llm-chat",
              "quarantined": false,
              "running_models": ["llm-chat"]
            }
          },
          "dram_used_gb": 7.0,
          "dram_reserved_gb": 7.0,
          "dram_total_gb": 32.0,
          "memory": {
            "status": "ok",
            "governor_enabled": true,
            "available_gb": 23.5,
            "soft_min_available_gb": 6.0,
            "hard_min_available_gb": 2.0
          },
          "active_models": 1,
          "revision": 4827
        }"#,
    )
    .unwrap();
    assert_eq!(capacity.memory.status, "ok");
    assert_eq!(capacity.memory.available_gb, Some(23.5));
    assert!(capacity.clusters["A100"].ep_active);

    let job: SchedulerJobStatus = serde_json::from_str(
        r#"{
          "job_id": "job_abc",
          "model": "llm-summary",
          "task": "kb.query.ask",
          "source": "kb_task",
          "service_class": "realtime_answer",
          "reason": "task_async_only",
          "scheduled_as": "async",
          "status": "done",
          "outputs": {"choices": [{"message": {"content": "answer"}}]},
          "device_used": "A100",
          "latency_ms": 1200.0,
          "queue_wait_ms": 20.0,
          "startup_state": "hot_resident",
          "startup_wait_ms": 0.0,
          "cold_start_wait_ms": 0.0,
          "worker_pid": 12847
        }"#,
    )
    .unwrap();
    assert_eq!(job.status, "done");
    assert_eq!(
        job.outputs["choices"][0]["message"]["content"].as_str(),
        Some("answer")
    );

    let task: SchedulerKbTaskResponse = serde_json::from_str(
        r#"{
          "job_id": "job_def",
          "status": "queued",
          "task": "kb.query.ask",
          "model": "llm-summary",
          "service_class": "realtime_answer",
          "scheduled_as": "async",
          "reason": "task_async_only",
          "eta_ms": 1500
        }"#,
    )
    .unwrap();
    assert_eq!(task.scheduled_as, "async");
    assert_eq!(task.job_id.as_deref(), Some("job_def"));
    assert_eq!(task.eta_ms, Some(1500));
}

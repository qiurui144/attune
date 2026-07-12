//! Fixture tests for the local scheduler contract surface Attune consumes.

use attune_core::edge_cloud::{
    LocalSchedulerClient, RuntimeProfileResolver, SchedulerBenchmarkContract,
    SchedulerCapacitySnapshot, SchedulerJobStatus, SchedulerKbTaskResponse, SchedulerModels,
};
use std::time::Duration;

const CONTRACT_JSON: &str = include_str!("fixtures/local_scheduler/benchmark_contract.json");
const MODELS_JSON: &str = include_str!("fixtures/local_scheduler/models.json");
const CAPACITY_JSON: &str = include_str!("fixtures/local_scheduler/capacity.json");
const JOB_DONE_JSON: &str = include_str!("fixtures/local_scheduler/job_done.json");
const JOB_FAILED_STRUCTURED_JSON: &str =
    include_str!("fixtures/local_scheduler/job_failed_structured_error.json");
const KB_TASK_ASYNC_JSON: &str = include_str!("fixtures/local_scheduler/kb_task_async.json");

#[test]
fn parses_benchmark_contract_context_caps_and_runtime_tasks() {
    let contract: SchedulerBenchmarkContract = serde_json::from_str(CONTRACT_JSON).unwrap();

    assert_eq!(contract.contract_version, "edge-scheduler-contract-v2");
    assert_eq!(
        contract.schema_versions["benchmark_contract"],
        "benchmark_contract.v2"
    );
    assert_eq!(contract.schema_versions["jobs"], "jobs.v2");
    assert_eq!(contract.revision, 4827);
    assert!(contract
        .request_fields
        .iter()
        .any(|field| field == "context_tokens"));
    assert!(contract
        .runtime_tasks
        .iter()
        .any(|task| task.name == "kb.query.ask"));
    assert!(contract
        .runtime_tasks
        .iter()
        .any(|task| task.name == "kb.query.vlm_extract"));
    assert_eq!(
        contract.application_api["kb_task"].as_str(),
        Some("POST /kb/tasks/{task}")
    );
    assert_eq!(
        contract.prompt_cache["policy"].as_str(),
        Some("prefix_stable_v1")
    );
    assert_eq!(
        contract.refusal_policy["name"].as_str(),
        Some("scheduler_refusal_v1")
    );

    let chat = contract
        .models
        .iter()
        .find(|model| model.name == "llm-chat")
        .expect("llm-chat model");
    assert_eq!(chat.max_context_tokens_sync, 4096);
    assert_eq!(chat.max_context_tokens_async, 8192);
    assert_eq!(chat.max_output_tokens_async, 1024);
    assert_eq!(
        chat.backend_profile["requires_no_think"].as_bool(),
        Some(true)
    );

    let ask = contract
        .runtime_tasks
        .iter()
        .find(|task| task.name == "kb.query.ask")
        .expect("ask task");
    assert!(ask.async_only);
    assert_eq!(ask.context_tokens, 4096);
    assert_eq!(ask.max_output_tokens, 128);

    let long_context = contract
        .service_classes
        .iter()
        .find(|class| class.name == "long_context")
        .expect("long_context service class");
    assert_eq!(long_context.sync_allowed, Some(false));
    assert_eq!(contract.async_jobs.max_active_jobs, 256);
}

#[test]
fn parses_models_capacity_job_and_kb_task_shapes() {
    let models: SchedulerModels = serde_json::from_str(MODELS_JSON).unwrap();
    let chat = models
        .models
        .iter()
        .find(|model| model.name == "llm-chat")
        .expect("llm-chat model status");
    assert_eq!(chat.state, "READY_SLOW");
    assert_eq!(chat.queue_depth, 2);
    assert_eq!(chat.p50_latency_ms, Some(1000.0));

    let capacity: SchedulerCapacitySnapshot = serde_json::from_str(CAPACITY_JSON).unwrap();
    assert_eq!(capacity.memory.status, "ok");
    assert_eq!(capacity.memory.available_gb, Some(23.5));
    assert!(capacity.clusters["cpu.vector"].ep_active);

    let job: SchedulerJobStatus = serde_json::from_str(JOB_DONE_JSON).unwrap();
    assert_eq!(job.status, "done");
    assert_eq!(
        job.outputs["choices"][0]["message"]["content"].as_str(),
        Some("answer")
    );
    assert_eq!(job.device_used.as_deref(), Some("cpu.vector"));
    assert_eq!(job.prompt_cache_key.as_deref(), Some("kb.query.ask:stable-prefix:abc123"));
    assert_eq!(job.cache_hit, Some(true));
    assert_eq!(job.prompt_cache_policy.as_deref(), Some("prefix_stable_v1"));
    assert_eq!(job.refusal_policy.as_deref(), Some("scheduler_refusal_v1"));

    let failed: SchedulerJobStatus = serde_json::from_str(JOB_FAILED_STRUCTURED_JSON).unwrap();
    assert_eq!(failed.status, "failed");
    assert_eq!(
        failed.error.as_deref(),
        Some("no grounded evidence was available for this answer")
    );

    let task: SchedulerKbTaskResponse = serde_json::from_str(KB_TASK_ASYNC_JSON).unwrap();
    assert_eq!(task.scheduled_as, "async");
    assert_eq!(task.job_id.as_deref(), Some("job_def"));
    assert_eq!(task.eta_ms, Some(1500));
}

#[test]
fn live_scheduler_contract_parses_and_resolves_profiles_when_configured() {
    let Some(base) = live_scheduler_base() else {
        eprintln!("skipping live scheduler contract test; set ATTUNE_E2E_LOCAL_SCHEDULER");
        return;
    };
    let client = LocalSchedulerClient::with_base(&base, Duration::from_secs(5));

    let contract = client
        .benchmark_contract()
        .expect("live scheduler /benchmark/contract must parse");
    assert!(
        contract
            .runtime_tasks
            .iter()
            .any(|task| task.name == "kb.query.ask"),
        "live scheduler must expose kb.query.ask runtime task"
    );
    assert!(
        contract.models.iter().any(|model| {
            model.max_context_tokens_sync > 0 || model.max_context_tokens_async > 0
        }),
        "live scheduler contract should expose context caps for at least one model"
    );

    let models = client.models().expect("live scheduler /models must parse");
    assert!(
        models.models.iter().any(|model| !model.name.is_empty()),
        "live scheduler /models should include named models"
    );

    let capacity = client
        .capacity()
        .expect("live scheduler /capacity must parse");
    assert!(
        !capacity.memory.status.is_empty() || capacity.dram_total_gb > 0.0,
        "live scheduler capacity should expose memory status or DRAM totals"
    );

    let profiles = RuntimeProfileResolver::from_scheduler(&contract, &models, &capacity, &base);
    assert!(
        profiles.task("kb.query.ask").is_some(),
        "runtime profile resolver should retain kb.query.ask"
    );
    assert!(
        profiles.task_model("kb.query.ask").is_some(),
        "kb.query.ask should resolve to a model profile"
    );
    assert!(
        profiles.models.values().any(|profile| profile.is_ready()),
        "live scheduler should have at least one READY model"
    );
}

fn live_scheduler_base() -> Option<String> {
    ["ATTUNE_E2E_LOCAL_SCHEDULER", "ATTUNE_LOCAL_SCHEDULER_BASE"]
        .iter()
        .find_map(|key| {
            std::env::var(key)
                .ok()
                .map(|value| value.trim().trim_end_matches('/').to_string())
                .filter(|value| !value.is_empty())
        })
}

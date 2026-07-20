//! HTTP contract tests for scheduler-backed text-to-speech.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use attune_server::test_support::spawn_eval_server_with_cloud_llm;
use axum::body::Body;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use sha2::{Digest, Sha256};

#[tokio::test]
async fn tts_rejects_empty_text_before_contacting_scheduler() {
    let server = spawn_eval_server_with_cloud_llm().await;
    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/tts/synthesize", server.url()))
        .json(&serde_json::json!({ "text": "   " }))
        .send()
        .await
        .expect("POST /tts/synthesize");

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = response.json().await.expect("JSON error body");
    assert_eq!(body["code"], "invalid-tts-request");
}

#[derive(Clone)]
struct MockSchedulerState {
    submitted: Arc<Mutex<Option<serde_json::Value>>>,
    cancel_seen: Arc<AtomicBool>,
    outputs: serde_json::Value,
    models: serde_json::Value,
    runtime_tasks: serde_json::Value,
    submit_failure: Option<serde_json::Value>,
    submit_http_status: StatusCode,
    submit_response: Option<serde_json::Value>,
    job_response: Option<serde_json::Value>,
    job_http_status: StatusCode,
    job_raw_body: Option<String>,
    models_failure: Option<serde_json::Value>,
}

struct MockSchedulerConfig {
    outputs: serde_json::Value,
    models: serde_json::Value,
    runtime_tasks: serde_json::Value,
    submit_failure: Option<serde_json::Value>,
    submit_http_status: StatusCode,
    submit_response: Option<serde_json::Value>,
    job_response: Option<serde_json::Value>,
    job_http_status: StatusCode,
    job_raw_body: Option<String>,
    models_failure: Option<serde_json::Value>,
}

impl MockSchedulerConfig {
    fn new(outputs: serde_json::Value) -> Self {
        Self {
            outputs,
            models: serde_json::json!([]),
            runtime_tasks: serde_json::json!([{
                "name": "kb.speech.synthesize",
                "stage": "tts",
                "model": "tts-default",
                "async_only": true
            }]),
            submit_failure: None,
            submit_http_status: StatusCode::ACCEPTED,
            submit_response: None,
            job_response: None,
            job_http_status: StatusCode::OK,
            job_raw_body: None,
            models_failure: None,
        }
    }
}

async fn mock_submit(
    State(state): State<MockSchedulerState>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    *state.submitted.lock().expect("submission lock") = Some(body);
    if let Some(error) = state.submit_failure {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(error)).into_response();
    }
    let response = state.submit_response.unwrap_or_else(|| {
        serde_json::json!({
            "schema_version": "kb_task.v1",
            "scheduled_as": "async",
            "job_id": "tts-job-1",
            "status": "queued",
            "task": "kb.speech.synthesize",
            "model": "tts-default"
        })
    });
    (state.submit_http_status, Json(response)).into_response()
}

async fn mock_job(State(state): State<MockSchedulerState>) -> Response {
    if let Some(raw) = state.job_raw_body {
        return Response::builder()
            .status(state.job_http_status)
            .header("content-type", "application/json")
            .body(Body::from(raw))
            .expect("raw job response");
    }
    let response = state.job_response.unwrap_or_else(|| {
        serde_json::json!({
            "schema_version": "job_status.v2",
            "job_id": "tts-job-1",
            "task": "kb.speech.synthesize",
            "model": "tts-default",
            "scheduled_as": "async",
            "status": "done",
            "phase": "done",
            "outputs": state.outputs
        })
    });
    (state.job_http_status, Json(response)).into_response()
}

async fn mock_cancel(State(state): State<MockSchedulerState>) -> Json<serde_json::Value> {
    state.cancel_seen.store(true, Ordering::SeqCst);
    Json(serde_json::json!({
        "schema_version": "job_status.v2",
        "job_id": "tts-job-1",
        "task": "kb.speech.synthesize",
        "model": "tts-default",
        "scheduled_as": "async",
        "status": "canceled",
        "phase": "done"
    }))
}

async fn mock_benchmark_contract(
    State(state): State<MockSchedulerState>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "contract_version": "test",
        "runtime_tasks": state.runtime_tasks
    }))
}

async fn mock_models(State(state): State<MockSchedulerState>) -> Response {
    if let Some(error) = state.models_failure {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response();
    }
    Json(serde_json::json!({"models": state.models})).into_response()
}

fn pcm16_mono_wav(sample_rate_hz: u32, samples: &[i16]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&sample_rate_hz.to_le_bytes());
    wav.extend_from_slice(&(sample_rate_hz * 2).to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    wav
}

fn valid_speech_audio() -> serde_json::Value {
    let wav = pcm16_mono_wav(16_000, &vec![500_i16; 1_600]);
    serde_json::json!({
        "schema_version": "speech_audio.v1",
        "task": "kb.speech.synthesize",
        "status": "ok",
        "language": "zh-CN",
        "voice": "default",
        "engine": "tts-default",
        "degraded": false,
        "audio": {
            "encoding": "base64",
            "data": BASE64_STANDARD.encode(&wav),
            "mime_type": "audio/wav",
            "format": "wav",
            "sample_rate_hz": 16_000,
            "channels": 1,
            "sample_format": "pcm_s16le",
            "duration_ms": 100,
            "byte_length": wav.len(),
            "sha256": hex::encode(Sha256::digest(&wav))
        }
    })
}

fn valid_submit_response() -> serde_json::Value {
    serde_json::json!({
        "schema_version": "kb_task.v1",
        "scheduled_as": "async",
        "job_id": "tts-job-1",
        "status": "queued",
        "task": "kb.speech.synthesize",
        "model": "tts-default"
    })
}

fn valid_job_response(outputs: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "schema_version": "job_status.v2",
        "job_id": "tts-job-1",
        "task": "kb.speech.synthesize",
        "model": "tts-default",
        "scheduled_as": "async",
        "status": "done",
        "phase": "done",
        "outputs": outputs
    })
}

async fn spawn_scheduler_custom(
    config: MockSchedulerConfig,
) -> (
    String,
    Arc<Mutex<Option<serde_json::Value>>>,
    Arc<AtomicBool>,
    tokio::task::JoinHandle<()>,
) {
    let submitted = Arc::new(Mutex::new(None));
    let cancel_seen = Arc::new(AtomicBool::new(false));
    let state = MockSchedulerState {
        submitted: Arc::clone(&submitted),
        cancel_seen: Arc::clone(&cancel_seen),
        outputs: config.outputs,
        models: config.models,
        runtime_tasks: config.runtime_tasks,
        submit_failure: config.submit_failure,
        submit_http_status: config.submit_http_status,
        submit_response: config.submit_response,
        job_response: config.job_response,
        job_http_status: config.job_http_status,
        job_raw_body: config.job_raw_body,
        models_failure: config.models_failure,
    };
    let router = Router::new()
        .route("/kb/tasks/kb.speech.synthesize:async", post(mock_submit))
        .route("/jobs/tts-job-1", get(mock_job))
        .route("/jobs/tts-job-1:cancel", post(mock_cancel))
        .route("/jobs/tts..job", get(mock_job))
        .route("/jobs/tts..job:cancel", post(mock_cancel))
        .route("/benchmark/contract", get(mock_benchmark_contract))
        .route("/models", get(mock_models))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind scheduler");
    let addr = listener.local_addr().expect("scheduler address");
    let handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("serve scheduler");
    });
    (format!("http://{addr}"), submitted, cancel_seen, handle)
}

async fn spawn_scheduler(
    outputs: serde_json::Value,
) -> (
    String,
    Arc<Mutex<Option<serde_json::Value>>>,
    tokio::task::JoinHandle<()>,
) {
    spawn_scheduler_config(outputs, serde_json::json!([]), None).await
}

async fn spawn_scheduler_config(
    outputs: serde_json::Value,
    models: serde_json::Value,
    submit_failure: Option<serde_json::Value>,
) -> (
    String,
    Arc<Mutex<Option<serde_json::Value>>>,
    tokio::task::JoinHandle<()>,
) {
    let mut config = MockSchedulerConfig::new(outputs);
    config.models = models;
    config.submit_failure = submit_failure;
    let (base, submitted, _cancel_seen, handle) = spawn_scheduler_custom(config).await;
    (base, submitted, handle)
}

async fn spawn_attune(
    scheduler_base: &str,
) -> (String, tokio::task::JoinHandle<()>, tempfile::TempDir) {
    let (base, handle, tmp, _state) = spawn_attune_with_state(scheduler_base).await;
    (base, handle, tmp)
}

async fn spawn_attune_with_state(
    scheduler_base: &str,
) -> (
    String,
    tokio::task::JoinHandle<()>,
    tempfile::TempDir,
    Arc<attune_server::state::AppState>,
) {
    let (base, handle, tmp, state, _token) = spawn_attune_with_config(scheduler_base, false).await;
    (base, handle, tmp, state)
}

async fn spawn_attune_with_config(
    scheduler_base: &str,
    require_auth: bool,
) -> (
    String,
    tokio::task::JoinHandle<()>,
    tempfile::TempDir,
    Arc<attune_server::state::AppState>,
    String,
) {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("open vault");
    vault.setup("tts-test-password").expect("setup vault");
    let token = vault.unlock("tts-test-password").expect("unlock vault");
    let settings = serde_json::json!({
        "embedding": {
            "provider": "local_scheduler",
            "endpoint": scheduler_base
        },
        "tts": {
            "enabled": true,
            "provider": "local_scheduler",
            "task": "kb.speech.synthesize",
            "voice": "auto",
            "language": "auto",
            "speed": 1.0,
            "format": "wav"
        }
    });
    vault
        .store()
        .set_meta(
            "app_settings",
            &serde_json::to_vec(&settings).expect("settings JSON"),
        )
        .expect("persist settings");
    let state = Arc::new(attune_server::state::AppState::new(vault, require_auth));
    let router = attune_server::build_router(Arc::clone(&state));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind attune");
    let addr = listener.local_addr().expect("attune address");
    let handle = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve attune");
    });
    (format!("http://{addr}"), handle, tmp, state, token)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tts_fails_fast_when_vault_is_busy_without_contacting_scheduler() {
    let (scheduler_base, submitted, scheduler_handle) = spawn_scheduler(valid_speech_audio()).await;
    let (attune_base, attune_handle, _tmp, state) = spawn_attune_with_state(&scheduler_base).await;
    let vault_guard = state
        .vault
        .lock()
        .expect("hold vault lock like a slow scan");

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        reqwest::Client::new()
            .post(format!("{attune_base}/api/v1/tts/synthesize"))
            .json(&serde_json::json!({"text": "do not wait for OCR"}))
            .send(),
    )
    .await
    .expect("TTS must not wait for the slow scan to release the vault mutex")
    .expect("POST TTS while vault is busy");

    assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    let raw_body = response.text().await.expect("busy JSON error body");
    assert!(!raw_body.contains("tts-test-password"));
    let body: serde_json::Value = serde_json::from_str(&raw_body).expect("busy JSON error");
    assert_eq!(
        body["error"],
        "text-to-speech settings are temporarily busy"
    );
    assert_eq!(body["code"], "tts-settings-busy");
    assert_eq!(body["retryable"], true);
    assert_eq!(body["may_degrade"], false);
    assert_eq!(body["degradation_policy"], "honest_failure");
    assert!(body.get("detail").is_none());
    assert!(
        submitted.lock().expect("submission lock").is_none(),
        "vault contention must fail before Scheduler submission"
    );

    drop(vault_guard);
    let recovered = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        reqwest::Client::new()
            .post(format!("{attune_base}/api/v1/tts/synthesize"))
            .json(&serde_json::json!({"text": "retry after OCR"}))
            .send(),
    )
    .await
    .expect("TTS retry must resume after the scan releases the vault mutex")
    .expect("POST recovered TTS");
    assert_eq!(recovered.status(), reqwest::StatusCode::OK);
    assert!(
        submitted.lock().expect("submission lock").is_some(),
        "the recovered request must reach Scheduler"
    );

    attune_handle.abort();
    scheduler_handle.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tts_fails_fast_when_vault_is_busy_with_auth_enabled() {
    let (scheduler_base, submitted, scheduler_handle) = spawn_scheduler(valid_speech_audio()).await;
    let (attune_base, attune_handle, _tmp, state, token) =
        spawn_attune_with_config(&scheduler_base, true).await;
    let vault_guard = state
        .vault
        .lock()
        .expect("hold vault lock like a slow scan");

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        reqwest::Client::new()
            .post(format!("{attune_base}/api/v1/tts/synthesize"))
            .bearer_auth(token)
            .json(&serde_json::json!({"text": "authenticated TTS must not wait"}))
            .send(),
    )
    .await
    .expect("authenticated TTS must not wait for the vault mutex")
    .expect("POST authenticated TTS while vault is busy");

    assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    let body: serde_json::Value = response.json().await.expect("busy JSON error");
    assert_eq!(body["code"], "tts-settings-busy");
    assert_eq!(body["retryable"], true);
    assert_eq!(body["degradation_policy"], "honest_failure");
    assert!(submitted.lock().expect("submission lock").is_none());

    drop(vault_guard);
    attune_handle.abort();
    scheduler_handle.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tts_fails_fast_before_logged_in_member_reconciliation_can_wait_on_vault() {
    let (scheduler_base, submitted, scheduler_handle) = spawn_scheduler(valid_speech_audio()).await;
    let (attune_base, attune_handle, _tmp, state) = spawn_attune_with_state(&scheduler_base).await;
    *state.member_state.lock().expect("member state lock") =
        attune_core::member_session::MemberState::Free {
            account_id: "tts-lock-regression".to_string(),
        };
    let vault_guard = state
        .vault
        .lock()
        .expect("hold vault lock like a slow scan");

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        reqwest::Client::new()
            .post(format!("{attune_base}/api/v1/tts/synthesize"))
            .json(&serde_json::json!({"text": "member TTS must not wait"}))
            .send(),
    )
    .await
    .expect("TTS must not wait in account reconciliation for the vault mutex")
    .expect("POST member TTS while vault is busy");

    assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    let body: serde_json::Value = response.json().await.expect("busy JSON error");
    assert_eq!(body["code"], "tts-settings-busy");
    assert_eq!(body["retryable"], true);
    assert_eq!(body["degradation_policy"], "honest_failure");
    assert!(submitted.lock().expect("submission lock").is_none());

    drop(vault_guard);
    attune_handle.abort();
    scheduler_handle.abort();
}

#[tokio::test]
async fn tts_submits_exact_async_contract_and_returns_validated_wav() {
    let sample_rate_hz = 16_000;
    let samples = vec![500_i16; 1_600];
    let wav = pcm16_mono_wav(sample_rate_hz, &samples);
    let digest = hex::encode(Sha256::digest(&wav));
    let outputs = serde_json::json!({
        "schema_version": "speech_audio.v1",
        "task": "kb.speech.synthesize",
        "status": "ok",
        "language": "zh-CN",
        "voice": "studio.zh",
        "engine": "tts-default",
        "degraded": false,
        "audio": {
            "encoding": "base64",
            "data": BASE64_STANDARD.encode(&wav),
            "mime_type": "audio/wav",
            "format": "wav",
            "sample_rate_hz": sample_rate_hz,
            "channels": 1,
            "sample_format": "pcm_s16le",
            "duration_ms": 100,
            "byte_length": wav.len(),
            "sha256": digest
        }
    });
    let (scheduler_base, submitted, scheduler_handle) = spawn_scheduler(outputs).await;
    let (attune_base, attune_handle, _tmp) = spawn_attune(&scheduler_base).await;

    let response = reqwest::Client::new()
        .post(format!("{attune_base}/api/v1/tts/synthesize"))
        .json(&serde_json::json!({
            "text": "你好，Attune",
            "voice": "default",
            "language": "zh-CN",
            "speed": 1.0,
            "output_format": "wav"
        }))
        .send()
        .await
        .expect("POST TTS");

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        response.headers()[reqwest::header::CONTENT_TYPE],
        "audio/wav"
    );
    assert_eq!(
        response.headers()[reqwest::header::CACHE_CONTROL],
        "no-store"
    );
    assert_eq!(
        response.headers()[reqwest::header::CONTENT_LENGTH],
        wav.len().to_string()
    );
    assert_eq!(response.bytes().await.expect("WAV body").as_ref(), wav);
    assert_eq!(
        submitted.lock().expect("submission lock").clone(),
        Some(serde_json::json!({
            "text": "你好，Attune",
            "voice": "default",
            "language": "zh-CN",
            "speed": 1.0,
            "output_format": "wav"
        }))
    );

    attune_handle.abort();
    scheduler_handle.abort();
}

#[tokio::test]
async fn tts_rejects_wav_above_scheduler_short_audio_budget() {
    let sample_rate_hz = 16_000;
    // Canonical WAV maximum is 44-byte header + 720_000 PCM bytes.
    let wav = pcm16_mono_wav(sample_rate_hz, &vec![500_i16; 360_001]);
    assert_eq!(wav.len(), 720_046);
    let outputs = serde_json::json!({
        "schema_version": "speech_audio.v1",
        "task": "kb.speech.synthesize",
        "status": "ok",
        "language": "zh-CN",
        "voice": "default",
        "engine": "tts-default",
        "degraded": false,
        "audio": {
            "encoding": "base64",
            "data": BASE64_STANDARD.encode(&wav),
            "mime_type": "audio/wav",
            "format": "wav",
            "sample_rate_hz": sample_rate_hz,
            "channels": 1,
            "sample_format": "pcm_s16le",
            "duration_ms": 22_500,
            "byte_length": wav.len(),
            "sha256": hex::encode(Sha256::digest(&wav))
        }
    });
    let (scheduler_base, _submitted, scheduler_handle) = spawn_scheduler(outputs).await;
    let (attune_base, attune_handle, _tmp) = spawn_attune(&scheduler_base).await;

    let response = reqwest::Client::new()
        .post(format!("{attune_base}/api/v1/tts/synthesize"))
        .json(&serde_json::json!({"text": "short request"}))
        .send()
        .await
        .expect("POST TTS");

    assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);
    let body: serde_json::Value = response.json().await.expect("JSON error");
    assert_eq!(body["code"], "invalid-tts-output");
    attune_handle.abort();
    scheduler_handle.abort();
}

#[tokio::test]
async fn tts_rejects_duration_above_15_seconds_even_below_byte_budget() {
    let sample_rate_hz = 8_000;
    let samples = vec![500_i16; 120_004];
    let wav = pcm16_mono_wav(sample_rate_hz, &samples);
    assert!(wav.len() < 720_044);
    let outputs = serde_json::json!({
        "schema_version": "speech_audio.v1",
        "task": "kb.speech.synthesize",
        "status": "ok",
        "language": "zh-CN",
        "voice": "default",
        "engine": "tts-default",
        "degraded": false,
        "audio": {
            "encoding": "base64",
            "data": BASE64_STANDARD.encode(&wav),
            "mime_type": "audio/wav",
            "format": "wav",
            "sample_rate_hz": sample_rate_hz,
            "channels": 1,
            "sample_format": "pcm_s16le",
            "duration_ms": 15_001,
            "byte_length": wav.len(),
            "sha256": hex::encode(Sha256::digest(&wav))
        }
    });
    let (scheduler_base, _submitted, scheduler_handle) = spawn_scheduler(outputs).await;
    let (attune_base, attune_handle, _tmp) = spawn_attune(&scheduler_base).await;

    let response = reqwest::Client::new()
        .post(format!("{attune_base}/api/v1/tts/synthesize"))
        .json(&serde_json::json!({"text": "short request"}))
        .send()
        .await
        .expect("POST TTS");

    assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);
    let body: serde_json::Value = response.json().await.expect("JSON error");
    assert_eq!(body["code"], "invalid-tts-output");
    attune_handle.abort();
    scheduler_handle.abort();
}

#[tokio::test]
async fn ai_stack_and_capability_registry_report_scheduler_tts_ready() {
    let (scheduler_base, _submitted, scheduler_handle) = spawn_scheduler_config(
        serde_json::json!({}),
        serde_json::json!([{
            "name": "tts-default",
            "state": "READY_FAST",
            "lifecycle": "READY",
            "dispatchable": "FREE"
        }]),
        None,
    )
    .await;
    let (attune_base, attune_handle, _tmp) = spawn_attune(&scheduler_base).await;
    let client = reqwest::Client::new();

    let stack: serde_json::Value = client
        .get(format!("{attune_base}/api/v1/ai_stack"))
        .send()
        .await
        .expect("GET ai_stack")
        .error_for_status()
        .expect("ai_stack success")
        .json()
        .await
        .expect("ai_stack JSON");
    assert_eq!(stack["tts"]["available"], true);
    assert_eq!(stack["tts"]["task"], "kb.speech.synthesize");
    assert_eq!(stack["tts"]["engine"], "scheduler:tts-default");

    let capabilities: serde_json::Value = client
        .get(format!("{attune_base}/api/v1/diagnostics/capabilities"))
        .send()
        .await
        .expect("GET capabilities")
        .error_for_status()
        .expect("capabilities success")
        .json()
        .await
        .expect("capabilities JSON");
    let tts = capabilities
        .as_array()
        .expect("capability array")
        .iter()
        .find(|capability| capability["id"] == "tts")
        .expect("TTS capability");
    assert_eq!(tts["health"], "ok");
    assert_eq!(tts["enabled"], true);

    attune_handle.abort();
    scheduler_handle.abort();
}

#[tokio::test]
async fn ai_stack_reports_tts_registered_but_unavailable_without_model() {
    let (scheduler_base, _submitted, scheduler_handle) =
        spawn_scheduler(serde_json::json!({})).await;
    let (attune_base, attune_handle, _tmp) = spawn_attune(&scheduler_base).await;
    let client = reqwest::Client::new();

    let stack: serde_json::Value = client
        .get(format!("{attune_base}/api/v1/ai_stack"))
        .send()
        .await
        .expect("GET ai_stack")
        .error_for_status()
        .expect("ai_stack success")
        .json()
        .await
        .expect("ai_stack JSON");
    assert_eq!(stack["tts"]["registered"], true);
    assert_eq!(stack["tts"]["model"], "tts-default");
    assert_eq!(stack["tts"]["available"], false);

    let capabilities: serde_json::Value = client
        .get(format!("{attune_base}/api/v1/diagnostics/capabilities"))
        .send()
        .await
        .expect("GET capabilities")
        .json()
        .await
        .expect("capabilities JSON");
    let tts = capabilities
        .as_array()
        .expect("capability array")
        .iter()
        .find(|capability| capability["id"] == "tts")
        .expect("TTS capability");
    assert_eq!(tts["health"], "unavailable");
    assert_eq!(tts["enabled"], false);

    attune_handle.abort();
    scheduler_handle.abort();
}

#[tokio::test]
async fn tts_rejects_more_than_128_unicode_scalars_before_scheduler_submit() {
    let (scheduler_base, submitted, scheduler_handle) =
        spawn_scheduler(serde_json::json!({})).await;
    let (attune_base, attune_handle, _tmp) = spawn_attune(&scheduler_base).await;
    let response = reqwest::Client::new()
        .post(format!("{attune_base}/api/v1/tts/synthesize"))
        .json(&serde_json::json!({"text": "字".repeat(129)}))
        .send()
        .await
        .expect("POST TTS");

    assert_eq!(response.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);
    assert!(submitted.lock().expect("submission lock").is_none());
    attune_handle.abort();
    scheduler_handle.abort();
}

#[tokio::test]
async fn tts_rejects_control_characters_even_at_trim_boundaries() {
    let (scheduler_base, submitted, scheduler_handle) =
        spawn_scheduler(serde_json::json!({})).await;
    let (attune_base, attune_handle, _tmp) = spawn_attune(&scheduler_base).await;
    let response = reqwest::Client::new()
        .post(format!("{attune_base}/api/v1/tts/synthesize"))
        .json(&serde_json::json!({"text": "\nhello"}))
        .send()
        .await
        .expect("POST TTS");

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    assert!(submitted.lock().expect("submission lock").is_none());
    attune_handle.abort();
    scheduler_handle.abort();
}

#[tokio::test]
async fn tts_rejects_non_lowercase_sha256() {
    let wav = pcm16_mono_wav(16_000, &vec![500_i16; 1_600]);
    let outputs = serde_json::json!({
        "schema_version": "speech_audio.v1",
        "task": "kb.speech.synthesize",
        "status": "ok",
        "language": "zh-CN",
        "voice": "default",
        "engine": "tts-default",
        "degraded": false,
        "audio": {
            "encoding": "base64",
            "data": BASE64_STANDARD.encode(&wav),
            "mime_type": "audio/wav",
            "format": "wav",
            "sample_rate_hz": 16_000,
            "channels": 1,
            "sample_format": "pcm_s16le",
            "duration_ms": 100,
            "byte_length": wav.len(),
            "sha256": hex::encode(Sha256::digest(&wav)).to_ascii_uppercase()
        }
    });
    let (scheduler_base, _submitted, scheduler_handle) = spawn_scheduler(outputs).await;
    let (attune_base, attune_handle, _tmp) = spawn_attune(&scheduler_base).await;
    let response = reqwest::Client::new()
        .post(format!("{attune_base}/api/v1/tts/synthesize"))
        .json(&serde_json::json!({"text": "sha"}))
        .send()
        .await
        .expect("POST TTS");
    assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);
    attune_handle.abort();
    scheduler_handle.abort();
}

#[tokio::test]
async fn tts_preserves_scheduler_no_model_503_as_honest_failure() {
    const SENTINEL: &str = "/private/models/tts-secret.onnx tensor_x_sentinel";
    let (scheduler_base, _submitted, scheduler_handle) = spawn_scheduler_config(
        serde_json::json!({}),
        serde_json::json!([]),
        Some(serde_json::json!({
            "error": SENTINEL,
            "code": "model_unavailable"
        })),
    )
    .await;
    let (attune_base, attune_handle, _tmp) = spawn_attune(&scheduler_base).await;
    let response = reqwest::Client::new()
        .post(format!("{attune_base}/api/v1/tts/synthesize"))
        .json(&serde_json::json!({"text": "no model"}))
        .send()
        .await
        .expect("POST TTS");

    assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    let raw_body = response.text().await.expect("JSON error body");
    assert!(!raw_body.contains(SENTINEL));
    let body: serde_json::Value = serde_json::from_str(&raw_body).expect("JSON error");
    assert_eq!(body["code"], "local-scheduler-unavailable");
    assert_eq!(body["error"], "text-to-speech scheduler task failed");
    assert!(body.get("detail").is_none());
    assert_eq!(body["degradation_policy"], "honest_failure");
    assert_eq!(body["may_degrade"], false);
    attune_handle.abort();
    scheduler_handle.abort();
}

#[tokio::test]
async fn tts_rejects_explicit_null_optional_fields_without_submitting() {
    let (scheduler_base, submitted, scheduler_handle) = spawn_scheduler(valid_speech_audio()).await;
    let (attune_base, attune_handle, _tmp) = spawn_attune(&scheduler_base).await;
    let client = reqwest::Client::new();

    for field in ["voice", "language", "speed", "output_format"] {
        *submitted.lock().expect("submission lock") = None;
        let mut body = serde_json::json!({"text": "null boundary"});
        body[field] = serde_json::Value::Null;
        let response = client
            .post(format!("{attune_base}/api/v1/tts/synthesize"))
            .json(&body)
            .send()
            .await
            .expect("POST TTS null field");
        assert_eq!(
            response.status(),
            reqwest::StatusCode::BAD_REQUEST,
            "explicit null {field} must be rejected"
        );
        assert!(
            submitted.lock().expect("submission lock").is_none(),
            "explicit null {field} must not reach Scheduler"
        );
    }

    attune_handle.abort();
    scheduler_handle.abort();
}

#[tokio::test]
async fn tts_json_rejection_is_fixed_and_does_not_reflect_input_values() {
    const SENTINEL: &str = "/private/models/tts-secret.onnx tensor_x_sentinel";
    let (scheduler_base, submitted, scheduler_handle) = spawn_scheduler(valid_speech_audio()).await;
    let (attune_base, attune_handle, _tmp) = spawn_attune(&scheduler_base).await;
    let response = reqwest::Client::new()
        .post(format!("{attune_base}/api/v1/tts/synthesize"))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(format!(r#"{{"text":"hello","speed":"{SENTINEL}"}}"#))
        .send()
        .await
        .expect("POST invalid TTS JSON");

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let raw = response.text().await.expect("invalid JSON response body");
    assert!(!raw.contains(SENTINEL));
    let body: serde_json::Value = serde_json::from_str(&raw).expect("invalid JSON error body");
    assert_eq!(body["error"], "invalid TTS JSON request");
    assert_eq!(body["code"], "invalid-tts-request");
    assert!(submitted.lock().expect("submission lock").is_none());

    attune_handle.abort();
    scheduler_handle.abort();
}

#[tokio::test]
async fn tts_accepts_safe_job_id_with_embedded_double_dots() {
    let mut submit = valid_submit_response();
    submit["job_id"] = serde_json::json!("tts..job");
    let mut job = valid_job_response(valid_speech_audio());
    job["job_id"] = serde_json::json!("tts..job");
    let mut config = MockSchedulerConfig::new(valid_speech_audio());
    config.submit_response = Some(submit);
    config.job_response = Some(job);
    let (scheduler_base, _submitted, _cancel_seen, scheduler_handle) =
        spawn_scheduler_custom(config).await;
    let (attune_base, attune_handle, _tmp) = spawn_attune(&scheduler_base).await;
    let response = reqwest::Client::new()
        .post(format!("{attune_base}/api/v1/tts/synthesize"))
        .json(&serde_json::json!({"text": "safe job id"}))
        .send()
        .await
        .expect("POST double-dot job-id TTS");

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    attune_handle.abort();
    scheduler_handle.abort();
}

#[tokio::test]
async fn tts_trims_only_outer_ascii_spaces_and_preserves_unicode_whitespace() {
    let (scheduler_base, submitted, scheduler_handle) = spawn_scheduler(valid_speech_audio()).await;
    let (attune_base, attune_handle, _tmp) = spawn_attune(&scheduler_base).await;
    let client = reqwest::Client::new();

    let response = client
        .post(format!("{attune_base}/api/v1/tts/synthesize"))
        .json(&serde_json::json!({"text": "  \u{00a0}hello\u{2003}  "}))
        .send()
        .await
        .expect("POST unicode whitespace TTS");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        submitted
            .lock()
            .expect("submission lock")
            .as_ref()
            .expect("submitted request")["text"],
        serde_json::json!("\u{00a0}hello\u{2003}")
    );

    *submitted.lock().expect("submission lock") = None;
    let response = client
        .post(format!("{attune_base}/api/v1/tts/synthesize"))
        .json(&serde_json::json!({"text": "\u{00a0}"}))
        .send()
        .await
        .expect("POST NBSP-only TTS");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        submitted
            .lock()
            .expect("submission lock")
            .as_ref()
            .expect("submitted request")["text"],
        serde_json::json!("\u{00a0}")
    );

    attune_handle.abort();
    scheduler_handle.abort();
}

#[tokio::test]
async fn tts_rejects_and_cancels_invalid_async_submit_lineage() {
    let valid = valid_submit_response();
    let mut cases = Vec::new();
    for (field, invalid) in [
        (
            "schema_version",
            serde_json::json!("kb_task.sentinel-/private/model"),
        ),
        ("status", serde_json::json!("running")),
        ("scheduled_as", serde_json::json!("sync")),
        ("task", serde_json::json!("kb.query.ask")),
        ("model", serde_json::json!("secret-model-path")),
    ] {
        let mut response = valid.clone();
        response[field] = invalid;
        cases.push((field, response, true));
    }
    let mut unsafe_job = valid;
    unsafe_job["job_id"] = serde_json::json!("../unsafe-job");
    cases.push(("job_id", unsafe_job, false));

    for (field, submit_response, expect_cancel) in cases {
        let mut config = MockSchedulerConfig::new(valid_speech_audio());
        config.submit_response = Some(submit_response);
        let (scheduler_base, _submitted, cancel_seen, scheduler_handle) =
            spawn_scheduler_custom(config).await;
        let (attune_base, attune_handle, _tmp) = spawn_attune(&scheduler_base).await;
        let response = reqwest::Client::new()
            .post(format!("{attune_base}/api/v1/tts/synthesize"))
            .json(&serde_json::json!({"text": "strict submit"}))
            .send()
            .await
            .expect("POST strict submit TTS");
        assert_eq!(
            response.status(),
            reqwest::StatusCode::BAD_GATEWAY,
            "invalid submit {field} must fail closed"
        );
        let body = response.text().await.expect("strict submit error body");
        assert!(!body.contains("sentinel"));
        assert!(!body.contains("secret-model-path"));
        assert_eq!(cancel_seen.load(Ordering::SeqCst), expect_cancel);
        attune_handle.abort();
        scheduler_handle.abort();
    }

    let mut config = MockSchedulerConfig::new(valid_speech_audio());
    config.submit_http_status = StatusCode::OK;
    let (scheduler_base, _submitted, cancel_seen, scheduler_handle) =
        spawn_scheduler_custom(config).await;
    let (attune_base, attune_handle, _tmp) = spawn_attune(&scheduler_base).await;
    let response = reqwest::Client::new()
        .post(format!("{attune_base}/api/v1/tts/synthesize"))
        .json(&serde_json::json!({"text": "strict submit HTTP"}))
        .send()
        .await
        .expect("POST strict submit HTTP TTS");
    assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);
    assert!(cancel_seen.load(Ordering::SeqCst));
    attune_handle.abort();
    scheduler_handle.abort();
}

#[tokio::test]
async fn tts_rejects_and_cancels_invalid_poll_lineage_and_phase_matrix() {
    let valid = valid_job_response(valid_speech_audio());
    let mut cases = Vec::new();
    for (field, invalid) in [
        ("schema_version", serde_json::json!("job_status.sentinel")),
        ("job_id", serde_json::json!("different-job")),
        ("task", serde_json::json!("kb.query.ask")),
        ("model", serde_json::json!("different-model")),
        ("scheduled_as", serde_json::json!("sync")),
        ("phase", serde_json::json!("worker_infer")),
        ("status", serde_json::json!("future-status")),
    ] {
        let mut response = valid.clone();
        response[field] = invalid;
        cases.push((field, response));
    }

    for (field, job_response) in cases {
        let mut config = MockSchedulerConfig::new(valid_speech_audio());
        config.job_response = Some(job_response);
        let (scheduler_base, _submitted, cancel_seen, scheduler_handle) =
            spawn_scheduler_custom(config).await;
        let (attune_base, attune_handle, _tmp) = spawn_attune(&scheduler_base).await;
        let response = reqwest::Client::new()
            .post(format!("{attune_base}/api/v1/tts/synthesize"))
            .json(&serde_json::json!({"text": "strict poll"}))
            .send()
            .await
            .expect("POST strict poll TTS");
        assert_eq!(
            response.status(),
            reqwest::StatusCode::BAD_GATEWAY,
            "invalid poll {field} must fail closed"
        );
        assert!(
            cancel_seen.load(Ordering::SeqCst),
            "invalid poll {field} must cancel the original job"
        );
        attune_handle.abort();
        scheduler_handle.abort();
    }
}

#[tokio::test]
async fn tts_cancels_job_after_poll_5xx_or_invalid_json_without_leaking_details() {
    const SENTINEL: &str = "/private/worker/tts.onnx tensor_internal_sentinel";
    for invalid_json in [false, true] {
        let mut config = MockSchedulerConfig::new(valid_speech_audio());
        if invalid_json {
            config.job_raw_body = Some(format!(
                "{{\"schema_version\":\"job_status.v2\",\"detail\":\"{SENTINEL}\""
            ));
        } else {
            config.job_http_status = StatusCode::INTERNAL_SERVER_ERROR;
            config.job_response = Some(serde_json::json!({"error": SENTINEL}));
        }
        let (scheduler_base, _submitted, cancel_seen, scheduler_handle) =
            spawn_scheduler_custom(config).await;
        let (attune_base, attune_handle, _tmp) = spawn_attune(&scheduler_base).await;
        let response = reqwest::Client::new()
            .post(format!("{attune_base}/api/v1/tts/synthesize"))
            .json(&serde_json::json!({"text": "poll failure"}))
            .send()
            .await
            .expect("POST poll failure TTS");
        assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);
        let body = response.text().await.expect("poll failure body");
        assert!(!body.contains(SENTINEL));
        assert!(!body.contains("detail"));
        assert!(cancel_seen.load(Ordering::SeqCst));
        attune_handle.abort();
        scheduler_handle.abort();
    }
}

#[tokio::test]
async fn tts_rejects_oversized_scheduler_body_before_json_decode_and_cancels() {
    const SENTINEL: &str = "oversized-private-scheduler-body-sentinel";
    let mut config = MockSchedulerConfig::new(valid_speech_audio());
    config.job_raw_body = Some(format!(
        "{{\"detail\":\"{SENTINEL}\",\"padding\":\"{}\"}}",
        "x".repeat(2 * 1024 * 1024)
    ));
    let (scheduler_base, _submitted, cancel_seen, scheduler_handle) =
        spawn_scheduler_custom(config).await;
    let (attune_base, attune_handle, _tmp) = spawn_attune(&scheduler_base).await;
    let response = reqwest::Client::new()
        .post(format!("{attune_base}/api/v1/tts/synthesize"))
        .json(&serde_json::json!({"text": "bounded response"}))
        .send()
        .await
        .expect("POST oversized scheduler response");
    assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);
    let body = response.text().await.expect("oversized response body");
    assert!(!body.contains(SENTINEL));
    assert!(cancel_seen.load(Ordering::SeqCst));
    attune_handle.abort();
    scheduler_handle.abort();
}

#[tokio::test]
async fn tts_maps_structured_terminal_statuses_without_string_guessing() {
    for (terminal, expected_status, expected_code) in [
        (
            "error",
            reqwest::StatusCode::BAD_GATEWAY,
            "local-scheduler-job-failed",
        ),
        (
            "canceled",
            reqwest::StatusCode::CONFLICT,
            "local-scheduler-cancelled",
        ),
        (
            "expired",
            reqwest::StatusCode::GONE,
            "local-scheduler-expired",
        ),
    ] {
        let mut job = valid_job_response(serde_json::json!({}));
        job["status"] = serde_json::json!(terminal);
        job["error"] = serde_json::json!("/private/model/tensor-terminal-sentinel");
        let mut config = MockSchedulerConfig::new(serde_json::json!({}));
        config.job_response = Some(job);
        let (scheduler_base, _submitted, _cancel_seen, scheduler_handle) =
            spawn_scheduler_custom(config).await;
        let (attune_base, attune_handle, _tmp) = spawn_attune(&scheduler_base).await;
        let response = reqwest::Client::new()
            .post(format!("{attune_base}/api/v1/tts/synthesize"))
            .json(&serde_json::json!({"text": "terminal status"}))
            .send()
            .await
            .expect("POST terminal TTS");
        assert_eq!(response.status(), expected_status, "terminal {terminal}");
        let raw = response.text().await.expect("terminal error body");
        assert!(!raw.contains("tensor-terminal-sentinel"));
        let body: serde_json::Value = serde_json::from_str(&raw).expect("terminal JSON");
        assert_eq!(body["code"], expected_code, "terminal {terminal}");
        assert!(body.get("detail").is_none());
        attune_handle.abort();
        scheduler_handle.abort();
    }
}

#[tokio::test]
async fn invalid_speech_audio_error_is_fixed_and_does_not_reflect_schema_sentinel() {
    const SENTINEL: &str = "/private/model/path tensor_output_sentinel";
    let mut outputs = valid_speech_audio();
    outputs["tensor"] = serde_json::json!(SENTINEL);
    let (scheduler_base, _submitted, scheduler_handle) = spawn_scheduler(outputs).await;
    let (attune_base, attune_handle, _tmp) = spawn_attune(&scheduler_base).await;
    let response = reqwest::Client::new()
        .post(format!("{attune_base}/api/v1/tts/synthesize"))
        .json(&serde_json::json!({"text": "invalid output"}))
        .send()
        .await
        .expect("POST invalid output TTS");
    assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);
    let raw = response.text().await.expect("invalid output body");
    assert!(!raw.contains(SENTINEL));
    assert!(!raw.contains("unknown field"));
    let body: serde_json::Value = serde_json::from_str(&raw).expect("invalid output JSON");
    assert_eq!(body["error"], "invalid scheduler TTS output");
    assert!(body.get("detail").is_none());
    attune_handle.abort();
    scheduler_handle.abort();
}

#[tokio::test]
async fn ai_stack_scheduler_probe_error_is_stable_and_sanitized() {
    const SENTINEL: &str = "/private/models/tts.onnx tensor_probe_sentinel";
    let mut config = MockSchedulerConfig::new(serde_json::json!({}));
    config.models_failure = Some(serde_json::json!({"error": SENTINEL}));
    let (scheduler_base, _submitted, _cancel_seen, scheduler_handle) =
        spawn_scheduler_custom(config).await;
    let (attune_base, attune_handle, _tmp) = spawn_attune(&scheduler_base).await;
    let response = reqwest::Client::new()
        .get(format!("{attune_base}/api/v1/ai_stack"))
        .send()
        .await
        .expect("GET ai_stack");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let raw = response.text().await.expect("ai_stack body");
    assert!(!raw.contains(SENTINEL));
    let body: serde_json::Value = serde_json::from_str(&raw).expect("ai_stack JSON");
    assert_eq!(
        body["scheduler"]["error"],
        "local scheduler model inventory unavailable"
    );

    let readiness = reqwest::Client::new()
        .get(format!(
            "{attune_base}/api/v1/local-scheduler/readiness?model=tts-default"
        ))
        .send()
        .await
        .expect("GET local scheduler readiness")
        .text()
        .await
        .expect("readiness body");
    assert!(!readiness.contains(SENTINEL));
    let readiness: serde_json::Value = serde_json::from_str(&readiness).expect("readiness JSON");
    assert_eq!(
        readiness["scheduler"]["error"],
        "local scheduler model inventory unavailable"
    );

    let capabilities = reqwest::Client::new()
        .get(format!("{attune_base}/api/v1/diagnostics/capabilities"))
        .send()
        .await
        .expect("GET diagnostics capabilities")
        .text()
        .await
        .expect("capabilities body");
    assert!(!capabilities.contains(SENTINEL));
    attune_handle.abort();
    scheduler_handle.abort();
}

#[tokio::test]
async fn ai_stack_requires_exact_tts_task_model_binding_and_async_only() {
    let ready_model = serde_json::json!([{
        "name": "tts-default",
        "state": "READY_FAST",
        "lifecycle": "READY",
        "dispatchable": "FREE"
    }]);
    for task in [
        serde_json::json!({
            "name": "kb.speech.synthesize",
            "stage": "tts",
            "model": "different-model",
            "async_only": true
        }),
        serde_json::json!({
            "name": "kb.speech.synthesize",
            "stage": "tts",
            "model": "tts-default",
            "async_only": false
        }),
        serde_json::json!({
            "name": "kb.speech.synthesize",
            "stage": "tts",
            "async_only": true
        }),
    ] {
        let mut config = MockSchedulerConfig::new(valid_speech_audio());
        config.models = ready_model.clone();
        config.runtime_tasks = serde_json::json!([task]);
        let (scheduler_base, _submitted, _cancel_seen, scheduler_handle) =
            spawn_scheduler_custom(config).await;
        let (attune_base, attune_handle, _tmp) = spawn_attune(&scheduler_base).await;
        let synthesize = reqwest::Client::new()
            .post(format!("{attune_base}/api/v1/tts/synthesize"))
            .json(&serde_json::json!({"text": "readiness must remain strict"}))
            .send()
            .await
            .expect("POST successful TTS with misbound runtime task");
        assert_eq!(synthesize.status(), reqwest::StatusCode::OK);
        let capabilities: serde_json::Value = reqwest::Client::new()
            .get(format!("{attune_base}/api/v1/diagnostics/capabilities"))
            .send()
            .await
            .expect("GET capabilities after successful TTS")
            .json()
            .await
            .expect("capabilities JSON");
        let tts = capabilities
            .as_array()
            .expect("capability array")
            .iter()
            .find(|capability| capability["id"] == "tts")
            .expect("TTS capability");
        assert_eq!(tts["health"], "unavailable");
        assert_eq!(tts["enabled"], false);

        let stack: serde_json::Value = reqwest::Client::new()
            .get(format!("{attune_base}/api/v1/ai_stack"))
            .send()
            .await
            .expect("GET ai_stack")
            .json()
            .await
            .expect("ai_stack JSON");
        assert_eq!(stack["tts"]["registered"], true);
        assert_eq!(stack["tts"]["available"], false);
        attune_handle.abort();
        scheduler_handle.abort();
    }
}

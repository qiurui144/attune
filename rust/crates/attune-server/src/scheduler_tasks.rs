use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use attune_core::edge_cloud::scheduler::{
    LocalSchedulerClient, SchedulerErrorKind, SchedulerJobState, SchedulerJobStatus,
    SchedulerKbTaskResponse,
};
use attune_core::error::{Result as CoreResult, VaultError};
use serde::Serialize;
use serde_json::Value;

use crate::state::SharedState;

const JOB_POLL_INTERVAL: Duration = Duration::from_millis(500);
const JOB_CANCEL_RESERVE: Duration = Duration::from_millis(250);

/// Bridges cancellation of the async request future into the Scheduler polling
/// loop running on Tokio's blocking pool. Dropping a `spawn_blocking` join
/// handle does not stop its closure, so the closure must observe an explicit
/// signal and cancel any trackable Scheduler job itself.
struct BlockingCancellationSignal {
    cancelled: Arc<AtomicBool>,
    armed: bool,
}

impl BlockingCancellationSignal {
    fn new() -> (Self, Arc<AtomicBool>) {
        let cancelled = Arc::new(AtomicBool::new(false));
        (
            Self {
                cancelled: Arc::clone(&cancelled),
                armed: true,
            },
            cancelled,
        )
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for BlockingCancellationSignal {
    fn drop(&mut self) {
        if self.armed {
            self.cancelled.store(true, Ordering::Release);
        }
    }
}

#[derive(Debug)]
pub(crate) struct TtsSchedulerTaskError {
    kind: SchedulerErrorKind,
    detail: String,
}

impl TtsSchedulerTaskError {
    fn from_source(error: VaultError) -> Self {
        let kind = attune_core::edge_cloud::classify_scheduler_error(&error)
            .unwrap_or(SchedulerErrorKind::Unavailable);
        Self {
            kind,
            detail: error.to_string(),
        }
    }

    fn contract(detail: impl Into<String>) -> Self {
        Self {
            kind: SchedulerErrorKind::InvalidJson,
            detail: detail.into(),
        }
    }

    fn terminal(kind: SchedulerErrorKind, job: &SchedulerJobStatus) -> Self {
        Self {
            kind,
            detail: format!(
                "local scheduler TTS job {} {}: {}",
                job.job_id,
                job.status,
                job.failure_detail().unwrap_or("no failure detail")
            ),
        }
    }

    pub(crate) fn kind(&self) -> SchedulerErrorKind {
        self.kind
    }
}

impl std::fmt::Display for TtsSchedulerTaskError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for TtsSchedulerTaskError {}

/// Strict Scheduler-only TTS path. This deliberately does not share the
/// compatibility aliases accepted by generic KB tasks: speech synthesis is
/// async-only and must preserve one exact task/model/job lineage.
pub(crate) async fn submit_tts_task_final<B>(
    state: &SharedState,
    body: B,
    poll_timeout: Duration,
) -> Result<Value, TtsSchedulerTaskError>
where
    B: Serialize + Send + 'static,
{
    let scheduler_base = crate::local_scheduler::base_from_state(state);
    let (mut cancellation_guard, cancellation) = BlockingCancellationSignal::new();
    let result = tokio::task::spawn_blocking(move || {
        submit_tts_task_final_blocking(&scheduler_base, &body, poll_timeout, || {
            cancellation.load(Ordering::Acquire)
        })
    })
    .await;
    cancellation_guard.disarm();
    result.map_err(|error| TtsSchedulerTaskError {
        kind: SchedulerErrorKind::Unavailable,
        detail: format!("local scheduler TTS task join failed: {error}"),
    })?
}

fn submit_tts_task_final_blocking<B, F>(
    scheduler_base: &str,
    body: &B,
    poll_timeout: Duration,
    should_cancel: F,
) -> Result<Value, TtsSchedulerTaskError>
where
    B: Serialize,
    F: Fn() -> bool,
{
    let request_timeout = poll_timeout.min(crate::local_scheduler::SUBMIT_TIMEOUT);
    // A valid speech_audio.v1 result is <=1 MiB. Leave bounded space for the
    // job_status.v2 envelope, while rejecting an oversized local response
    // before reqwest/serde can retain and decode it in full.
    let client = LocalSchedulerClient::with_base(scheduler_base, request_timeout)
        .with_max_response_bytes(crate::routes::tts::MAX_PUBLIC_OUTPUT_BYTES + 64 * 1024);
    let response = client
        .submit_kb_task(crate::routes::tts::TTS_TASK, body, true)
        .map_err(TtsSchedulerTaskError::from_source)?;
    strict_tts_final_outputs(&client, response, poll_timeout, should_cancel)
}

fn strict_tts_final_outputs<F>(
    client: &LocalSchedulerClient,
    response: SchedulerKbTaskResponse,
    poll_timeout: Duration,
    should_cancel: F,
) -> Result<Value, TtsSchedulerTaskError>
where
    F: Fn() -> bool,
{
    let cancel_candidate = response
        .job_id
        .as_deref()
        .filter(|job_id| valid_tts_job_id(job_id))
        .map(str::to_string);
    let job_id = match validate_tts_submission(&response) {
        Ok(job_id) => job_id.to_string(),
        Err(detail) => {
            if let Some(job_id) = cancel_candidate.as_deref() {
                best_effort_tts_cancel(client, job_id, JOB_CANCEL_RESERVE);
            }
            return Err(TtsSchedulerTaskError::contract(detail));
        }
    };

    let deadline = Instant::now() + poll_timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining <= JOB_CANCEL_RESERVE {
            best_effort_tts_cancel(client, &job_id, remaining);
            return Err(TtsSchedulerTaskError {
                kind: SchedulerErrorKind::Delayed,
                detail: format!("local scheduler TTS job {job_id} timed out"),
            });
        }
        if should_cancel() {
            best_effort_tts_cancel(client, &job_id, remaining);
            return Err(TtsSchedulerTaskError {
                kind: SchedulerErrorKind::Cancelled,
                detail: format!("local scheduler TTS job {job_id} cancelled"),
            });
        }

        let request_timeout = remaining
            .saturating_sub(JOB_CANCEL_RESERVE)
            .min(crate::local_scheduler::SUBMIT_TIMEOUT);
        let poll_client = client.with_timeout(request_timeout);
        let job = match poll_client.job(&job_id) {
            Ok(job) => job,
            Err(error) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                best_effort_tts_cancel(client, &job_id, remaining);
                return Err(TtsSchedulerTaskError::from_source(error));
            }
        };
        if let Err(detail) = validate_tts_job(&job, &job_id) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            best_effort_tts_cancel(client, &job_id, remaining);
            return Err(TtsSchedulerTaskError::contract(detail));
        }

        match job.status.as_str() {
            "done" => return Ok(job.outputs),
            "error" => {
                return Err(TtsSchedulerTaskError::terminal(
                    SchedulerErrorKind::JobFailed,
                    &job,
                ));
            }
            "canceled" => {
                return Err(TtsSchedulerTaskError::terminal(
                    SchedulerErrorKind::Cancelled,
                    &job,
                ));
            }
            "expired" => {
                return Err(TtsSchedulerTaskError::terminal(
                    SchedulerErrorKind::Expired,
                    &job,
                ));
            }
            "queued" | "running" | "cancel_requested" => {}
            _ => unreachable!("validated TTS job status matrix"),
        }

        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining <= JOB_CANCEL_RESERVE {
            best_effort_tts_cancel(client, &job_id, remaining);
            return Err(TtsSchedulerTaskError {
                kind: SchedulerErrorKind::Delayed,
                detail: format!("local scheduler TTS job {job_id} timed out"),
            });
        }
        std::thread::sleep(JOB_POLL_INTERVAL.min(remaining.saturating_sub(JOB_CANCEL_RESERVE)));
    }
}

fn validate_tts_submission(response: &SchedulerKbTaskResponse) -> Result<&str, String> {
    if response.http_status != Some(202) {
        return Err("TTS async submit must return HTTP 202".to_string());
    }
    if response.schema_version != "kb_task.v1"
        || response.scheduled_as != "async"
        || response.status.as_deref() != Some("queued")
        || response.task != crate::routes::tts::TTS_TASK
        || response.model != crate::routes::tts::TTS_ENGINE
        || has_scheduler_error(response.error.as_deref())
        || has_scheduler_error(response.error_code.as_deref())
    {
        return Err("TTS async submit lineage is invalid".to_string());
    }
    response
        .job_id
        .as_deref()
        .filter(|job_id| valid_tts_job_id(job_id))
        .ok_or_else(|| "TTS async submit job_id is missing or unsafe".to_string())
}

fn validate_tts_job(job: &SchedulerJobStatus, expected_job_id: &str) -> Result<(), String> {
    if job.http_status != Some(200)
        || job.schema_version != "job_status.v2"
        || job.job_id != expected_job_id
        || job.task.as_deref() != Some(crate::routes::tts::TTS_TASK)
        || job.model != crate::routes::tts::TTS_ENGINE
        || job.scheduled_as.as_deref() != Some("async")
    {
        return Err("TTS job lineage is invalid".to_string());
    }

    let phase_valid = matches!(
        (job.status.as_str(), job.phase.as_deref()),
        ("queued", Some("not_started" | "scheduler_queue"))
            | ("running", Some("worker_infer"))
            | ("cancel_requested", Some("scheduler_queue" | "worker_infer"))
            | ("done" | "error" | "canceled" | "expired", Some("done"))
    );
    if !phase_valid {
        return Err("TTS job status/phase matrix is invalid".to_string());
    }
    if job.status == "done"
        && (has_scheduler_error(job.error.as_deref())
            || has_scheduler_error(job.error_code.as_deref()))
    {
        return Err("completed TTS job contains an error".to_string());
    }
    Ok(())
}

fn has_scheduler_error(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.trim().is_empty())
}

fn valid_tts_job_id(job_id: &str) -> bool {
    let bytes = job_id.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'_' | b'.' | b'-'))
}

fn best_effort_tts_cancel(client: &LocalSchedulerClient, job_id: &str, available: Duration) {
    if available.is_zero() {
        return;
    }
    let cancel_client = client.with_timeout(available.min(JOB_CANCEL_RESERVE));
    let _ = cancel_client.cancel_job(job_id);
}

pub(crate) async fn submit_kb_task_final<B>(
    state: &SharedState,
    task: &'static str,
    body: B,
    explicit_async: bool,
    poll_timeout: Duration,
) -> CoreResult<Value>
where
    B: Serialize + Send + 'static,
{
    let scheduler_base = crate::local_scheduler::base_from_state(state);
    tokio::task::spawn_blocking(move || {
        submit_kb_task_final_blocking(
            &scheduler_base,
            task,
            &body,
            explicit_async,
            poll_timeout,
            || false,
        )
    })
    .await
    .map_err(|e| VaultError::LlmUnavailable(format!("local scheduler task join failed: {e}")))?
}

pub(crate) fn submit_kb_task_final_blocking<B, F>(
    scheduler_base: &str,
    task: &str,
    body: &B,
    explicit_async: bool,
    poll_timeout: Duration,
    should_cancel: F,
) -> CoreResult<Value>
where
    B: Serialize,
    F: Fn() -> bool,
{
    // The total job budget must not become the timeout of every individual HTTP
    // request; otherwise one stalled status probe can overrun the poll deadline.
    let request_timeout = poll_timeout.min(crate::local_scheduler::SUBMIT_TIMEOUT);
    let client = LocalSchedulerClient::with_base(scheduler_base, request_timeout);
    let mut response = client.submit_kb_task(task, body, explicit_async)?;

    // Cold-start retry: scheduler may return "async/pending" without a job_id
    // when the target model worker is not yet loaded. Wait up to poll_timeout
    // for the cold-start to complete, then retry the task submission.
    if response.status.as_deref() == Some("async/pending")
        && response.job_id.as_deref().map(|s| s.trim().is_empty()).unwrap_or(true)
    {
        let retry_deadline = std::time::Instant::now() + poll_timeout;
        let retry_request_timeout = Duration::from_secs(30);
        let retry_client = LocalSchedulerClient::with_base(scheduler_base, retry_request_timeout);
        while std::time::Instant::now() < retry_deadline {
            std::thread::sleep(Duration::from_secs(10));
            match retry_client.submit_kb_task(task, body, explicit_async) {
                Ok(r) => {
                    let has_job = r.job_id.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false);
                    response = r;
                    if has_job {
                        break;
                    }
                }
                Err(_) => {
                    // Transient errors during cold-start are expected; continue retrying
                    continue;
                }
            }
        }
    }

    final_outputs(
        &client,
        response,
        explicit_async,
        poll_timeout,
        should_cancel,
    )
}

fn final_outputs<F>(
    client: &LocalSchedulerClient,
    response: SchedulerKbTaskResponse,
    explicit_async: bool,
    poll_timeout: Duration,
    should_cancel: F,
) -> CoreResult<Value>
where
    F: Fn() -> bool,
{
    response.validate_submission(
        explicit_async,
        &format!("local scheduler task {}", response.task),
    )?;
    match response.normalized_state() {
        SchedulerJobState::Succeeded => return Ok(response.outputs),
        SchedulerJobState::Failed => {
            return Err(VaultError::LlmUnavailable(format!(
                "local scheduler task {} failed: {}",
                response.task,
                response
                    .failure_detail()
                    .or(response.status.as_deref())
                    .unwrap_or("unknown error")
            )));
        }
        SchedulerJobState::Waiting => {}
    }
    if let Some(job_id) = response.job_id {
        let deadline = Instant::now() + poll_timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining <= JOB_CANCEL_RESERVE {
                return timeout_after_best_effort_cancel(client, &job_id, remaining);
            }
            if should_cancel() {
                let cancel_client =
                    client.with_timeout(remaining.min(crate::local_scheduler::SUBMIT_TIMEOUT));
                let _ = cancel_client.cancel_job(&job_id);
                return Err(VaultError::LlmUnavailable(
                    "local scheduler job cancelled".to_string(),
                ));
            }
            let request_timeout = remaining
                .saturating_sub(JOB_CANCEL_RESERVE)
                .min(crate::local_scheduler::SUBMIT_TIMEOUT);
            let poll_client = client.with_timeout(request_timeout);
            let job = match poll_client.job(&job_id) {
                Ok(job) => job,
                Err(error) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining <= JOB_CANCEL_RESERVE {
                        return timeout_after_best_effort_cancel(client, &job_id, remaining);
                    }
                    return Err(error);
                }
            };
            match job.normalized_state() {
                SchedulerJobState::Succeeded => return Ok(job.outputs),
                SchedulerJobState::Failed => {
                    let detail = job.failure_detail().unwrap_or("local scheduler job failed");
                    return Err(VaultError::LlmUnavailable(format!(
                        "local scheduler job {job_id} {}: {detail}",
                        job.status
                    )));
                }
                SchedulerJobState::Waiting => {}
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining <= JOB_CANCEL_RESERVE {
                return timeout_after_best_effort_cancel(client, &job_id, remaining);
            }
            std::thread::sleep(JOB_POLL_INTERVAL.min(remaining.saturating_sub(JOB_CANCEL_RESERVE)));
        }
    }

    Ok(response.outputs)
}

fn timeout_after_best_effort_cancel(
    client: &LocalSchedulerClient,
    job_id: &str,
    remaining: Duration,
) -> CoreResult<Value> {
    if !remaining.is_zero() {
        let cancel_client = client.with_timeout(remaining.min(JOB_CANCEL_RESERVE));
        let _ = cancel_client.cancel_job(job_id);
    }
    Err(VaultError::LlmUnavailable(format!(
        "local scheduler job {job_id} timed out"
    )))
}

pub(crate) fn output_text(outputs: &Value) -> Option<String> {
    for pointer in [
        "/text",
        "/full_text",
        "/transcript",
        "/answer",
        "/content",
        "/response",
        "/summary",
        "/output",
        "/result/text",
        "/outputs/text",
        "/outputs/full_text",
        "/outputs/transcript",
        "/choices/0/message/content",
        "/choices/0/text",
        "/outputs/choices/0/message/content",
        "/outputs/choices/0/text",
    ] {
        if let Some(text) = outputs.pointer(pointer).and_then(|v| v.as_str()) {
            let text = text.trim();
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }
    if let Some(s) = outputs.as_str().map(str::trim).filter(|s| !s.is_empty()) {
        return Some(s.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    fn accept_test_request(listener: &TcpListener, deadline: Instant) -> TcpStream {
        loop {
            match listener.accept() {
                Ok((stream, _)) => return stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "timed out waiting for mock Scheduler request"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("accept mock Scheduler request: {error}"),
            }
        }
    }

    fn read_test_request(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set mock Scheduler read timeout");
        let mut request = Vec::new();
        let mut chunk = [0_u8; 4096];
        loop {
            let length = stream
                .read(&mut chunk)
                .expect("read mock Scheduler request");
            assert!(length > 0, "mock Scheduler request ended before headers");
            request.extend_from_slice(&chunk[..length]);

            let Some(header_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
                continue;
            };
            let body_start = header_end + 4;
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().expect("valid content-length"))
                })
                .unwrap_or(0);
            if request.len() >= body_start + content_length {
                return String::from_utf8(request).expect("mock Scheduler request is UTF-8");
            }
        }
    }

    fn write_test_json(stream: &mut TcpStream, status: &str, body: &str) {
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("write mock Scheduler response");
    }

    fn test_tts_state(scheduler_base: &str) -> (SharedState, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("TTS test tempdir");
        let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("open test vault");
        vault
            .setup("tts-cancel-test-password")
            .expect("setup vault");
        vault
            .unlock("tts-cancel-test-password")
            .expect("unlock vault");
        let settings = serde_json::json!({
            "tts": {
                "enabled": true,
                "provider": "local_scheduler",
                "task": crate::routes::tts::TTS_TASK,
                "endpoint": scheduler_base
            },
            "embedding": {
                "provider": "local_scheduler",
                "endpoint": scheduler_base
            }
        });
        vault
            .store()
            .set_meta(
                "app_settings",
                &serde_json::to_vec(&settings).expect("serialize TTS test settings"),
            )
            .expect("persist TTS test settings");
        (Arc::new(crate::state::AppState::new(vault, false)), tmp)
    }

    #[test]
    fn output_text_accepts_common_shapes() {
        assert_eq!(
            output_text(&serde_json::json!({"outputs": {"full_text": " hello "}})).as_deref(),
            Some("hello")
        );
        assert_eq!(
            output_text(&serde_json::json!({"result": {"text": "ok"}})).as_deref(),
            Some("ok")
        );
        assert_eq!(
            output_text(&serde_json::json!({"summary": " summary "})).as_deref(),
            Some("summary")
        );
        assert_eq!(
            output_text(&serde_json::json!({
                "choices": [{"message": {"content": "answer"}}]
            }))
            .as_deref(),
            Some("answer")
        );
    }

    #[test]
    fn blocking_cancellation_signal_only_fires_when_armed() {
        let (guard, cancelled) = BlockingCancellationSignal::new();
        assert!(!cancelled.load(Ordering::Acquire));
        drop(guard);
        assert!(cancelled.load(Ordering::Acquire));

        let (mut guard, cancelled) = BlockingCancellationSignal::new();
        guard.disarm();
        drop(guard);
        assert!(!cancelled.load(Ordering::Acquire));
    }

    #[test]
    fn synchronous_scheduler_failure_is_not_treated_as_empty_success() {
        let client =
            LocalSchedulerClient::with_base("http://127.0.0.1:8090", Duration::from_millis(10));
        let response = SchedulerKbTaskResponse {
            task: "kb.query.ask".to_string(),
            status: Some("error".to_string()),
            error: Some("model unavailable".to_string()),
            outputs: serde_json::json!({}),
            ..SchedulerKbTaskResponse::default()
        };
        let err = final_outputs(&client, response, false, Duration::from_millis(10), || {
            false
        })
        .expect_err("terminal submit failure must be returned");
        assert!(err.to_string().contains("model unavailable"));
    }

    #[test]
    fn waiting_or_unknown_scheduler_response_without_job_id_is_rejected() {
        let client =
            LocalSchedulerClient::with_base("http://127.0.0.1:8090", Duration::from_millis(10));
        for response in [
            SchedulerKbTaskResponse {
                task: "kb.query.ask".to_string(),
                status: Some("queued".to_string()),
                outputs: serde_json::json!({}),
                ..SchedulerKbTaskResponse::default()
            },
            SchedulerKbTaskResponse {
                task: "kb.query.ask".to_string(),
                status: Some("future_scheduler_state".to_string()),
                outputs: serde_json::json!({}),
                ..SchedulerKbTaskResponse::default()
            },
            SchedulerKbTaskResponse {
                task: "kb.query.ask".to_string(),
                scheduled_as: "async".to_string(),
                status: Some("done".to_string()),
                outputs: serde_json::json!({"answer": "must not escape validation"}),
                ..SchedulerKbTaskResponse::default()
            },
        ] {
            let err = final_outputs(&client, response, false, Duration::from_millis(10), || {
                false
            })
            .expect_err(
                "async/waiting/unknown response without job_id must not be treated as success",
            );
            assert!(err.to_string().contains("without job_id"));
        }
    }

    #[test]
    fn explicit_async_and_error_only_submissions_fail_closed() {
        let client =
            LocalSchedulerClient::with_base("http://127.0.0.1:8090", Duration::from_millis(10));
        let legacy_shape = SchedulerKbTaskResponse {
            task: "kb.query.ask".to_string(),
            outputs: serde_json::json!({"answer": "not trackable"}),
            ..SchedulerKbTaskResponse::default()
        };
        assert!(final_outputs(
            &client,
            legacy_shape,
            true,
            Duration::from_millis(10),
            || false,
        )
        .expect_err("the :async request itself requires a job id")
        .to_string()
        .contains("without job_id"));

        let error_only = SchedulerKbTaskResponse {
            task: "kb.query.ask".to_string(),
            error: Some("OOM".to_string()),
            ..SchedulerKbTaskResponse::default()
        };
        assert!(final_outputs(
            &client,
            error_only,
            false,
            Duration::from_millis(10),
            || false,
        )
        .expect_err("a 2xx error body is not success")
        .to_string()
        .contains("OOM"));
    }

    #[test]
    fn poll_timeout_best_effort_cancels_trackable_job() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock scheduler");
        let address = listener.local_addr().expect("mock scheduler address");
        let cancel_seen = Arc::new(AtomicBool::new(false));
        let cancel_seen_server = Arc::clone(&cancel_seen);
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept cancel request");
            let mut request = [0_u8; 2048];
            let length = stream.read(&mut request).expect("read cancel request");
            let request = String::from_utf8_lossy(&request[..length]);
            cancel_seen_server.store(
                request.starts_with("POST /jobs/timeout-job:cancel "),
                Ordering::SeqCst,
            );
            let body = r#"{"job_id":"timeout-job","status":"canceled"}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write cancel response");
        });
        let client = LocalSchedulerClient::with_base(
            &format!("http://{address}"),
            Duration::from_millis(100),
        );
        let response = SchedulerKbTaskResponse {
            scheduled_as: "async".to_string(),
            job_id: Some("timeout-job".to_string()),
            status: Some("queued".to_string()),
            task: "kb.speech.synthesize".to_string(),
            ..SchedulerKbTaskResponse::default()
        };

        let error = final_outputs(&client, response, true, Duration::from_millis(50), || false)
            .expect_err("poll timeout must fail");
        server.join().expect("mock scheduler thread");

        assert!(error.to_string().contains("timed out"));
        assert!(
            cancel_seen.load(Ordering::SeqCst),
            "timeout must issue best-effort scheduler cancellation"
        );
    }

    #[test]
    fn tts_request_cancellation_best_effort_cancels_trackable_job() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock scheduler");
        let address = listener.local_addr().expect("mock scheduler address");
        let cancel_seen = Arc::new(AtomicBool::new(false));
        let cancel_seen_server = Arc::clone(&cancel_seen);
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept cancel request");
            let mut request = [0_u8; 2048];
            let length = stream.read(&mut request).expect("read cancel request");
            cancel_seen_server.store(
                String::from_utf8_lossy(&request[..length])
                    .starts_with("POST /jobs/tts-cancel-job:cancel "),
                Ordering::SeqCst,
            );
            let body = r#"{"job_id":"tts-cancel-job","status":"canceled"}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write cancel response");
        });
        let client = LocalSchedulerClient::with_base(
            &format!("http://{address}"),
            Duration::from_millis(250),
        );
        let response = SchedulerKbTaskResponse {
            http_status: Some(202),
            schema_version: "kb_task.v1".to_string(),
            scheduled_as: "async".to_string(),
            job_id: Some("tts-cancel-job".to_string()),
            status: Some("queued".to_string()),
            task: crate::routes::tts::TTS_TASK.to_string(),
            model: crate::routes::tts::TTS_ENGINE.to_string(),
            ..SchedulerKbTaskResponse::default()
        };

        let error = strict_tts_final_outputs(&client, response, Duration::from_secs(1), || true)
            .expect_err("request cancellation must fail the local wait");
        server.join().expect("mock scheduler thread");

        assert_eq!(error.kind(), SchedulerErrorKind::Cancelled);
        assert!(cancel_seen.load(Ordering::SeqCst));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tts_future_abort_after_accepted_submit_cancels_scheduler_job() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock Scheduler");
        listener
            .set_nonblocking(true)
            .expect("set mock Scheduler nonblocking");
        let address = listener.local_addr().expect("mock Scheduler address");
        let (poll_seen_tx, poll_seen_rx) = tokio::sync::oneshot::channel();
        let (cancel_seen_tx, cancel_seen_rx) = tokio::sync::oneshot::channel();
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);

            let mut submit = accept_test_request(&listener, deadline);
            let request = read_test_request(&mut submit);
            assert!(request.starts_with("POST /kb/tasks/kb.speech.synthesize:async "));
            write_test_json(
                &mut submit,
                "202 Accepted",
                r#"{"schema_version":"kb_task.v1","scheduled_as":"async","job_id":"tts-abort-job","status":"queued","task":"kb.speech.synthesize","model":"tts-default"}"#,
            );
            drop(submit);

            // Seeing a poll proves the blocking task consumed and validated the
            // 202 response before the upper async future is aborted.
            let mut poll = accept_test_request(&listener, deadline);
            let request = read_test_request(&mut poll);
            assert!(request.starts_with("GET /jobs/tts-abort-job "));
            write_test_json(
                &mut poll,
                "200 OK",
                r#"{"schema_version":"job_status.v2","scheduled_as":"async","job_id":"tts-abort-job","status":"queued","phase":"scheduler_queue","task":"kb.speech.synthesize","model":"tts-default","outputs":{}}"#,
            );
            let _ = poll_seen_tx.send(());
            drop(poll);

            let mut cancel = accept_test_request(&listener, deadline);
            let request = read_test_request(&mut cancel);
            assert!(request.starts_with("POST /jobs/tts-abort-job:cancel "));
            write_test_json(
                &mut cancel,
                "200 OK",
                r#"{"job_id":"tts-abort-job","status":"canceled"}"#,
            );
            let _ = cancel_seen_tx.send(());
        });

        let (state, _tmp) = test_tts_state(&format!("http://{address}"));
        let request_task = tokio::spawn(async move {
            submit_tts_task_final(
                &state,
                serde_json::json!({
                    "text": "abort after accepted submit",
                    "voice": "auto",
                    "language": "auto",
                    "speed": 1.0,
                    "output_format": "wav"
                }),
                Duration::from_secs(5),
            )
            .await
        });

        tokio::time::timeout(Duration::from_secs(3), poll_seen_rx)
            .await
            .expect("TTS task did not poll after accepted submit")
            .expect("mock Scheduler stopped before reporting the poll");
        request_task.abort();
        let join_error = request_task
            .await
            .expect_err("aborted upper TTS future must not complete normally");
        assert!(join_error.is_cancelled());

        tokio::time::timeout(Duration::from_secs(3), cancel_seen_rx)
            .await
            .expect("aborted TTS future did not cancel the Scheduler job")
            .expect("mock Scheduler stopped before reporting cancellation");
        server.join().expect("mock Scheduler thread");
    }

    #[test]
    fn tts_strict_poll_timeout_cancels_scheduler_job() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock Scheduler");
        listener
            .set_nonblocking(true)
            .expect("set mock Scheduler nonblocking");
        let address = listener.local_addr().expect("mock Scheduler address");
        let poll_seen = Arc::new(AtomicBool::new(false));
        let poll_seen_server = Arc::clone(&poll_seen);
        let cancel_seen = Arc::new(AtomicBool::new(false));
        let cancel_seen_server = Arc::clone(&cancel_seen);
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);

            let mut submit = accept_test_request(&listener, deadline);
            let request = read_test_request(&mut submit);
            assert!(request.starts_with("POST /kb/tasks/kb.speech.synthesize:async "));
            write_test_json(
                &mut submit,
                "202 Accepted",
                r#"{"schema_version":"kb_task.v1","scheduled_as":"async","job_id":"tts-timeout-job","status":"queued","task":"kb.speech.synthesize","model":"tts-default"}"#,
            );
            drop(submit);

            loop {
                let mut stream = accept_test_request(&listener, deadline);
                let request = read_test_request(&mut stream);
                if request.starts_with("GET /jobs/tts-timeout-job ") {
                    poll_seen_server.store(true, Ordering::SeqCst);
                    write_test_json(
                        &mut stream,
                        "200 OK",
                        r#"{"schema_version":"job_status.v2","scheduled_as":"async","job_id":"tts-timeout-job","status":"queued","phase":"scheduler_queue","task":"kb.speech.synthesize","model":"tts-default","outputs":{}}"#,
                    );
                    continue;
                }
                cancel_seen_server.store(
                    request.starts_with("POST /jobs/tts-timeout-job:cancel "),
                    Ordering::SeqCst,
                );
                write_test_json(
                    &mut stream,
                    "200 OK",
                    r#"{"job_id":"tts-timeout-job","status":"canceled"}"#,
                );
                break;
            }
        });

        let error = submit_tts_task_final_blocking(
            &format!("http://{address}"),
            &serde_json::json!({
                "text": "strict timeout",
                "voice": "auto",
                "language": "auto",
                "speed": 1.0,
                "output_format": "wav"
            }),
            Duration::from_millis(900),
            || false,
        )
        .expect_err("strict TTS polling timeout must fail");
        server.join().expect("mock Scheduler thread");

        assert_eq!(error.kind(), SchedulerErrorKind::Delayed);
        assert!(poll_seen.load(Ordering::SeqCst));
        assert!(cancel_seen.load(Ordering::SeqCst));
    }

    #[test]
    fn tts_poll_transport_failure_best_effort_cancels_original_job() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock scheduler");
        listener
            .set_nonblocking(true)
            .expect("nonblocking mock scheduler");
        let address = listener.local_addr().expect("mock scheduler address");
        let cancel_seen = Arc::new(AtomicBool::new(false));
        let cancel_seen_server = Arc::clone(&cancel_seen);
        let server = std::thread::spawn(move || {
            let accept = || {
                let deadline = Instant::now() + Duration::from_secs(3);
                loop {
                    match listener.accept() {
                        Ok((stream, _)) => return stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline, "timed out waiting for request");
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("accept mock scheduler request: {error}"),
                    }
                }
            };

            let mut submit = accept();
            let mut request = [0_u8; 4096];
            let length = submit.read(&mut request).expect("read submit request");
            assert!(String::from_utf8_lossy(&request[..length])
                .starts_with("POST /kb/tasks/kb.speech.synthesize:async "));
            let body = r#"{"schema_version":"kb_task.v1","scheduled_as":"async","job_id":"tts-job-1","status":"queued","task":"kb.speech.synthesize","model":"tts-default"}"#;
            write!(
                submit,
                "HTTP/1.1 202 Accepted\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write submit response");
            drop(submit);

            let mut poll = accept();
            let length = poll.read(&mut request).expect("read poll request");
            assert!(String::from_utf8_lossy(&request[..length]).starts_with("GET /jobs/tts-job-1 "));
            drop(poll);

            let mut cancel = accept();
            let length = cancel.read(&mut request).expect("read cancel request");
            cancel_seen_server.store(
                String::from_utf8_lossy(&request[..length])
                    .starts_with("POST /jobs/tts-job-1:cancel "),
                Ordering::SeqCst,
            );
            let body = r#"{"job_id":"tts-job-1","status":"canceled"}"#;
            write!(
                cancel,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write cancel response");
        });

        let error = submit_tts_task_final_blocking(
            &format!("http://{address}"),
            &serde_json::json!({
                "text": "transport",
                "voice": "auto",
                "language": "auto",
                "speed": 1.0,
                "output_format": "wav"
            }),
            Duration::from_secs(2),
            || false,
        )
        .expect_err("poll transport failure must fail");
        server.join().expect("mock scheduler thread");

        assert_eq!(error.kind(), SchedulerErrorKind::Transport);
        assert!(cancel_seen.load(Ordering::SeqCst));
    }
}

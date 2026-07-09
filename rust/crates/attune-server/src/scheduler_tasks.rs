use std::time::{Duration, Instant};

use attune_core::edge_cloud::scheduler::{
    LocalSchedulerClient, SchedulerJobStatus, SchedulerKbTaskResponse,
};
use attune_core::error::{Result as CoreResult, VaultError};
use serde::Serialize;
use serde_json::Value;

use crate::state::SharedState;

const JOB_POLL_INTERVAL: Duration = Duration::from_millis(500);

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
    let client = LocalSchedulerClient::with_base(scheduler_base, poll_timeout);
    let response = client.submit_kb_task(task, body, explicit_async)?;
    final_outputs(&client, response, poll_timeout, should_cancel)
}

fn final_outputs<F>(
    client: &LocalSchedulerClient,
    response: SchedulerKbTaskResponse,
    poll_timeout: Duration,
    should_cancel: F,
) -> CoreResult<Value>
where
    F: Fn() -> bool,
{
    if let Some(job_id) = response.job_id {
        let deadline = Instant::now() + poll_timeout;
        loop {
            if should_cancel() {
                let _ = client.cancel_job(&job_id);
                return Err(VaultError::LlmUnavailable(
                    "local scheduler job cancelled".to_string(),
                ));
            }
            if Instant::now() >= deadline {
                return Err(VaultError::LlmUnavailable(format!(
                    "local scheduler job {job_id} timed out"
                )));
            }
            let job = client.job(&job_id)?;
            if scheduler_job_done(&job) {
                return Ok(job.outputs);
            }
            if scheduler_job_failed(&job) {
                let detail = job
                    .error
                    .or(job.detail)
                    .unwrap_or_else(|| "local scheduler job failed".to_string());
                return Err(VaultError::LlmUnavailable(format!(
                    "local scheduler job {job_id} {}: {detail}",
                    job.status
                )));
            }
            std::thread::sleep(JOB_POLL_INTERVAL);
        }
    }

    if response.status.as_deref().is_some_and(is_terminal_error_status) {
        return Err(VaultError::LlmUnavailable(format!(
            "local scheduler task {} failed: {}",
            response.task,
            response.status.unwrap_or_default()
        )));
    }
    Ok(response.outputs)
}

fn scheduler_job_done(job: &SchedulerJobStatus) -> bool {
    matches!(job.status.to_ascii_lowercase().as_str(), "done" | "success")
}

fn scheduler_job_failed(job: &SchedulerJobStatus) -> bool {
    is_terminal_error_status(&job.status)
}

fn is_terminal_error_status(status: &str) -> bool {
    matches!(
        status.to_ascii_lowercase().as_str(),
        "error" | "failed" | "cancelled" | "canceled" | "expired" | "timeout"
    )
}

pub(crate) fn output_text(outputs: &Value) -> Option<String> {
    for pointer in [
        "/text",
        "/full_text",
        "/transcript",
        "/answer",
        "/content",
        "/result/text",
        "/outputs/text",
        "/outputs/full_text",
        "/outputs/transcript",
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
    }
}

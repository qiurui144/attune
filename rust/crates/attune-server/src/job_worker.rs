//! G5: durable job worker wiring — per-kind handlers + the background drain loop.
//!
//! Replaces office.rs's inline `tokio::task::spawn_blocking` per request: jobs are
//! enqueued to the durable `job_queue` table and drained here, so they survive a
//! restart (recover_on_boot requeues idempotent kinds) and respect deadlines.
//! Spec: docs/superpowers/specs/2026-06-22-durable-job-queue.md §4/§6.

use crate::state::AppState;
use attune_core::job_handler::{JobControl, JobHandler, JobHandlerRegistry};
use attune_core::office_job_queue::JobKind;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// Max executions per job before the worker parks it (`max-attempts`).
const JOB_MAX_ATTEMPTS: i64 = 5;
/// done/failed/cancelled rows older than this are TTL-purged (spec §8).
const JOB_TTL_DAYS: i64 = 30;
/// Base exponential-backoff window for an auto-retried failed job (spec §7).
/// attempt N waits base * 2^(N-1), capped at 1h inside auto_retry_failed_jobs.
/// 30s base → 30s, 1m, 2m, 4m … for a transient nightly-batch failure.
const JOB_RETRY_BASE_BACKOFF_MS: i64 = 30_000;

fn join_text_parts<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    let mut parts = parts.into_iter();
    let Some(first) = parts.next() else {
        return String::new();
    };

    let mut out = String::from(first);
    for part in parts {
        out.push(' ');
        out.push_str(part);
    }
    out
}

/// ASR handler — submits the task to local scheduler. Payload:
/// {"file_path": "...", "diarization": bool, "scheduler_base": "..."}.
/// at_least_once: re-transcribing the same file after a crash is idempotent.
pub struct AsrJobHandler;

impl JobHandler for AsrJobHandler {
    fn kind(&self) -> JobKind {
        JobKind::Asr
    }

    fn initial_stage_json(&self) -> Option<&'static str> {
        Some("{\"stage\":\"transcribing\"}")
    }

    fn run(&self, payload_json: &str, ctl: &dyn JobControl) -> Result<String, (String, String)> {
        let v: serde_json::Value = serde_json::from_str(payload_json)
            .map_err(|e| ("bad-payload".to_string(), e.to_string()))?;
        let file_path = v["file_path"]
            .as_str()
            .ok_or_else(|| ("bad-payload".to_string(), "missing file_path".to_string()))?;
        let diarization = v["diarization"].as_bool().unwrap_or(false);
        let scheduler_base = v["scheduler_base"]
            .as_str()
            .unwrap_or(attune_core::edge_cloud::capacity::DEFAULT_SCHEDULER_BASE)
            .to_string();

        // Source file may have been deleted between enqueue and run (spec §7).
        if !std::path::Path::new(file_path).exists() {
            return Err((
                "source-missing".to_string(),
                format!("audio file not found: {file_path}"),
            ));
        }

        if ctl.is_cancelled() {
            return Err((
                "cancelled".to_string(),
                "cancelled before start".to_string(),
            ));
        }

        let outputs = crate::scheduler_tasks::submit_kb_task_final_blocking(
            &scheduler_base,
            "kb.meeting.asr_frontend",
            &serde_json::json!({
                "file_path": file_path,
                "language": v["language"].as_str().unwrap_or("auto"),
                "model": v["model"].as_str().unwrap_or("small"),
                "diarization": diarization,
            }),
            true,
            Duration::from_secs(60 * 60),
            || ctl.is_cancelled(),
        )
        .map_err(|e| {
            let view = crate::local_scheduler::classify_scheduler_failure(
                &e,
                crate::local_scheduler::SchedulerDegradationPolicy::HonestFailure,
            );
            (view.code.to_string(), e.to_string())
        })?;

        let result = scheduler_asr_result(outputs, diarization);
        Ok(result.to_string())
    }
}

fn scheduler_asr_result(outputs: serde_json::Value, diarization: bool) -> serde_json::Value {
    let segments = asr_segments_from_outputs(&outputs);
    let full_text = if segments.is_empty() {
        crate::scheduler_tasks::output_text(&outputs).unwrap_or_default()
    } else {
        join_text_parts(
            segments
                .iter()
                .filter_map(|s| s.get("text").and_then(|v| v.as_str())),
        )
    };
    let duration_sec = outputs
        .get("duration_sec")
        .and_then(|v| v.as_f64())
        .or_else(|| {
            segments
                .last()
                .and_then(|s| s.get("end_sec").and_then(|v| v.as_f64()))
        })
        .unwrap_or(0.0);
    let speakers = aggregate_speakers(&segments);
    serde_json::json!({
        "model": outputs.get("model").and_then(|v| v.as_str()).unwrap_or("scheduler:kb.meeting.asr_frontend"),
        "language_detected": outputs
            .get("language_detected")
            .or_else(|| outputs.get("language"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown"),
        "duration_sec": duration_sec,
        "segments": if segments.is_empty() && !full_text.is_empty() {
            vec![serde_json::json!({
                "start_sec": 0.0,
                "end_sec": duration_sec,
                "text": full_text,
                "speaker": serde_json::Value::Null,
            })]
        } else {
            segments
        },
        "speakers": speakers,
        "full_text": full_text,
        "diarization_used": diarization,
        "raw_scheduler_output": outputs,
    })
}

fn asr_segments_from_outputs(outputs: &serde_json::Value) -> Vec<serde_json::Value> {
    for pointer in [
        "/segments",
        "/outputs/segments",
        "/result/segments",
        "/data/segments",
    ] {
        if let Some(items) = outputs.pointer(pointer).and_then(|v| v.as_array()) {
            let segments: Vec<_> = items.iter().filter_map(normalize_asr_segment).collect();
            if !segments.is_empty() {
                return segments;
            }
        }
    }
    Vec::new()
}

fn normalize_asr_segment(value: &serde_json::Value) -> Option<serde_json::Value> {
    let text = value
        .get("text")
        .or_else(|| value.get("content"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let start_sec = value
        .get("start_sec")
        .or_else(|| value.get("start"))
        .and_then(|v| v.as_f64())
        .or_else(|| {
            value
                .get("start_ms")
                .and_then(|v| v.as_f64())
                .map(|v| v / 1000.0)
        })
        .unwrap_or(0.0);
    let end_sec = value
        .get("end_sec")
        .or_else(|| value.get("end"))
        .and_then(|v| v.as_f64())
        .or_else(|| {
            value
                .get("end_ms")
                .and_then(|v| v.as_f64())
                .map(|v| v / 1000.0)
        })
        .unwrap_or(start_sec);
    Some(serde_json::json!({
        "start_sec": start_sec,
        "end_sec": end_sec,
        "text": text,
        "speaker": value.get("speaker").cloned().unwrap_or(serde_json::Value::Null),
    }))
}

fn aggregate_speakers(segments: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let mut speakers_agg: std::collections::BTreeMap<String, (f64, u64)> =
        std::collections::BTreeMap::new();
    for segment in segments {
        let Some(speaker) = segment.get("speaker").and_then(|v| v.as_str()) else {
            continue;
        };
        let start = segment
            .get("start_sec")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let end = segment
            .get("end_sec")
            .and_then(|v| v.as_f64())
            .unwrap_or(start);
        let entry = speakers_agg.entry(speaker.to_string()).or_insert((0.0, 0));
        entry.0 += (end - start).max(0.0);
        entry.1 += 1;
    }
    speakers_agg
        .into_iter()
        .map(|(id, (total, count))| {
            serde_json::json!({
                "id": id,
                "total_sec": total,
                "segment_count": count,
            })
        })
        .collect()
}

/// Build the production handler registry. New kinds register here (spec §6).
pub fn build_registry() -> JobHandlerRegistry {
    let mut reg = JobHandlerRegistry::new();
    reg.register(Arc::new(AsrJobHandler));
    reg
}

/// Spawn the background job worker: per tick, sweep timeouts + TTL-purge, then
/// drain queued jobs **serially** (one at a time — preserves the office "信号量
/// 门控防资源踩踏" semantic: never two ASR jobs at once). Handlers
/// are blocking → each job runs inside `spawn_blocking`.
pub fn start_job_worker(state: Arc<AppState>) {
    if state
        .job_worker_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        tracing::debug!("G5: job worker already running, skipping");
        return;
    }
    let Some(store) = state.job_store() else {
        state.job_worker_running.store(false, Ordering::SeqCst);
        tracing::warn!("G5: job worker not started — job store unavailable");
        return;
    };
    let registry = Arc::new(build_registry());

    tokio::spawn(async move {
        tracing::info!("G5: durable job worker started");
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(500));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let now = chrono::Utc::now().timestamp_millis();
            {
                let s = store.lock().unwrap_or_else(|e| e.into_inner());
                let _ = s.sweep_timeouts(now);
                // Auto-retry transient failures with exponential backoff before the
                // TTL purge sees them — an unattended local-scheduler nightly batch retries on its
                // own (spec §7) instead of waiting for an operator requeue.
                let _ = s.auto_retry_failed_jobs(now, JOB_MAX_ATTEMPTS, JOB_RETRY_BASE_BACKOFF_MS);
                let _ = s.purge_terminal_jobs(now, JOB_TTL_DAYS);
            }
            // Drain serially until the queue is empty for this tick. run_one_job
            // blocks on the handler (scheduler job polling) → spawn_blocking so the
            // tokio worker thread is not starved.
            loop {
                let store_c = store.clone();
                let registry_c = registry.clone();
                let ran = tokio::task::spawn_blocking(move || {
                    attune_core::job_handler::run_one_job(&store_c, &registry_c, JOB_MAX_ATTEMPTS)
                })
                .await
                .unwrap_or(None);
                if ran.is_none() {
                    break;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::join_text_parts;

    #[test]
    fn join_text_parts_matches_slice_join_semantics() {
        let parts = ["alpha", " beta ", "", "gamma"];

        assert_eq!(join_text_parts(parts), parts.join(" "));
        assert_eq!(join_text_parts(Vec::<&str>::new()), "");
    }
}

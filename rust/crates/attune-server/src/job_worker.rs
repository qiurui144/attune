//! G5: durable job worker wiring — per-kind handlers + the background drain loop.
//!
//! Replaces office.rs's inline `tokio::task::spawn_blocking` per request: jobs are
//! enqueued to the durable `job_queue` table and drained here, so they survive a
//! restart (recover_on_boot requeues idempotent kinds) and respect deadlines.
//! Spec: docs/superpowers/specs/2026-06-10-k3-g5-durable-job-queue.md §4/§6.

use crate::state::AppState;
use attune_core::job_handler::{JobControl, JobHandler, JobHandlerRegistry};
use attune_core::office_job_queue::JobKind;
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// Max executions per job before the worker parks it (`max-attempts`).
const JOB_MAX_ATTEMPTS: i64 = 5;
/// done/failed/cancelled rows older than this are TTL-purged (spec §8).
const JOB_TTL_DAYS: i64 = 30;

/// ASR handler — runs whisper via subprocess, same pipeline the old inline
/// office.rs spawn used. Payload: {"file_path": "...", "diarization": bool}.
/// at_least_once: re-transcribing the same file after a crash is idempotent.
///
/// **Cancellation limitation (documented, RELEASE.md Known Limitations):** the
/// core of `run` is a single uninterruptible `transcribe_with_diarization`
/// subprocess call. We honor cancellation at the boundaries we *can* — before
/// backend detection and before the subprocess starts — but once whisper is
/// running we cannot stop it mid-file; cancel flips DB state immediately and the
/// late result is dropped by the running-guard. True mid-subprocess kill is a
/// follow-up (needs a child-process handle + SIGTERM path).
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

        // Source file may have been deleted between enqueue and run (spec §7).
        if !std::path::Path::new(file_path).exists() {
            return Err((
                "source-missing".to_string(),
                format!("audio file not found: {file_path}"),
            ));
        }

        // Honor cancellation at the boundaries we can (pre-detection, pre-subprocess) —
        // the cheapest savings for a job cancelled while still queued behind a slow one.
        if ctl.is_cancelled() {
            return Err(("cancelled".to_string(), "cancelled before start".to_string()));
        }

        let backend = attune_core::asr::detect_asr_backend().ok_or_else(|| {
            (
                "asr-engine-failed".to_string(),
                "ASR backend not available (whisper-cli not installed)".to_string(),
            )
        })?;
        let diar_backend = if diarization {
            attune_core::asr::detect_diarization_backend()
        } else {
            None
        };

        // Last pre-subprocess cancel checkpoint. Beyond this the whisper call is
        // uninterruptible (see struct doc).
        if ctl.is_cancelled() {
            return Err(("cancelled".to_string(), "cancelled before transcribe".to_string()));
        }

        let (segments, _full_text_legacy) = attune_core::asr::transcribe_with_diarization(
            &backend,
            std::path::Path::new(file_path),
            diar_backend.as_ref(),
        )
        .map_err(|e| ("asr-engine-failed".to_string(), e.to_string()))?;

        // Aggregate speakers (TranscriptSegment uses ms granularity → sec for response).
        let mut speakers_agg: std::collections::BTreeMap<String, (f64, u64)> =
            std::collections::BTreeMap::new();
        for s in &segments {
            let key = s.speaker.clone().unwrap_or_else(|| "SPEAKER_UNK".into());
            let dur = (s.end_ms as f64 - s.start_ms as f64).max(0.0) / 1000.0;
            let entry = speakers_agg.entry(key).or_insert((0.0, 0));
            entry.0 += dur;
            entry.1 += 1;
        }
        let duration_sec = segments
            .last()
            .map(|s| s.end_ms as f64 / 1000.0)
            .unwrap_or(0.0);

        let result = serde_json::json!({
            "model": backend.model_name,
            "language_detected": backend.language,
            "duration_sec": duration_sec,
            "segments": segments.iter().map(|s| serde_json::json!({
                "start_sec": s.start_ms as f64 / 1000.0,
                "end_sec": s.end_ms as f64 / 1000.0,
                "text": s.text,
                "speaker": s.speaker,
            })).collect::<Vec<_>>(),
            "speakers": speakers_agg.iter().map(|(id, (total, count))| serde_json::json!({
                "id": id,
                "total_sec": total,
                "segment_count": count,
            })).collect::<Vec<_>>(),
            "full_text": segments.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" "),
            "diarization_used": diar_backend.is_some(),
        });
        Ok(result.to_string())
    }
}

/// Build the production handler registry. New kinds register here (spec §6).
pub fn build_registry() -> JobHandlerRegistry {
    let mut reg = JobHandlerRegistry::new();
    reg.register(Arc::new(AsrJobHandler));
    reg
}

/// Spawn the background job worker: per tick, sweep timeouts + TTL-purge, then
/// drain queued jobs **serially** (one at a time — preserves the office "信号量
/// 门控防资源踩踏" semantic: never two whisper subprocesses at once). Handlers
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
                let _ = s.purge_terminal_jobs(now, JOB_TTL_DAYS);
            }
            // Drain serially until the queue is empty for this tick. run_one_job
            // blocks on the handler (whisper subprocess) → spawn_blocking so the
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

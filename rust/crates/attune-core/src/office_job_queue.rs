//! Office helper async job queue — in-memory state machine.
//!
//! 个人助手语义：不限并发，FIFO 排队，信号量门控防止资源踩踏。
//! 服务重启后所有 in-flight job 标 cancelled（不持久化，per spec §1）。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStage {
    Queued,
    LoadingModel,
    Transcribing,
    Diarizing,
    Postprocess,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobError {
    pub message: String,
    /// kebab-case stable code (per CLAUDE.md error contract)
    pub code: String,
}

/// G5: durable job kinds. Each kind maps to a [`crate::job_handler::JobHandler`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    Asr,
    Ocr,
    Agent,
    IngestBatch,
}

impl JobKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            JobKind::Asr => "asr",
            JobKind::Ocr => "ocr",
            JobKind::Agent => "agent",
            JobKind::IngestBatch => "ingest_batch",
        }
    }

    pub fn from_str_kind(s: &str) -> Option<Self> {
        match s {
            "asr" => Some(JobKind::Asr),
            "ocr" => Some(JobKind::Ocr),
            "agent" => Some(JobKind::Agent),
            "ingest_batch" => Some(JobKind::IngestBatch),
            _ => None,
        }
    }

    /// Default delivery contract per kind. Drives boot recovery:
    /// at_least_once → Running requeued to Queued; at_most_once → Running marked Failed.
    pub fn default_delivery(&self) -> DeliveryContract {
        match self {
            // Re-running yields the same artifact (content_hash dedupes writes).
            JobKind::Asr | JobKind::Ocr | JobKind::IngestBatch => DeliveryContract::AtLeastOnce,
            // LLM spend / external side effects are not safely repeatable by default.
            JobKind::Agent => DeliveryContract::AtMostOnce,
        }
    }
}

/// G5 risk mitigation: how a kind tolerates re-execution after a crash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryContract {
    /// Safe to re-run (idempotent handler). Boot recovery requeues Running→Queued.
    AtLeastOnce,
    /// Not safe to re-run. Boot recovery marks an interrupted Running job Failed
    /// (code `interrupted-no-retry`) instead of requeueing.
    AtMostOnce,
}

/// Durable, DB-facing job record (mirror of a `job_queue` row).
/// Replaces the in-memory `Job` (which used `Instant`, not persistable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: String,
    pub kind: JobKind,
    pub state: JobState,
    /// kind-specific stage as JSON, e.g. {"stage":"transcribing"} for ASR.
    pub stage_json: Option<String>,
    pub progress: f32,
    pub priority: i64,
    pub payload_json: String,
    pub result_json: Option<String>,
    pub error: Option<JobError>,
    pub warnings: Vec<String>,
    pub attempts: i64,
    pub created_ms: i64,
    pub started_ms: Option<i64>,
    pub finished_ms: Option<i64>,
    pub deadline_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: String,
    pub state: JobState,
    pub stage: JobStage,
    pub progress: f32,
    pub created_at: Instant,
    pub started_at: Option<Instant>,
    pub elapsed_ms: u64,
    pub eta_ms: Option<u64>,
    pub result_json: Option<String>,
    pub error: Option<JobError>,
    pub warnings: Vec<String>,
}

impl Job {
    pub fn new(id: String) -> Self {
        Self {
            id,
            state: JobState::Queued,
            stage: JobStage::Queued,
            progress: 0.0,
            created_at: Instant::now(),
            started_at: None,
            elapsed_ms: 0,
            eta_ms: None,
            result_json: None,
            error: None,
            warnings: vec![],
        }
    }
}

/// 内存 job registry — 不持久化（server 重启即 reset）。
///
/// FIFO 排队语义：`queue_position(id)` 返回该 job 之前还有多少个 Queued 状态的 job。
pub struct JobRegistry {
    jobs: Mutex<HashMap<String, Job>>,
}

impl JobRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            jobs: Mutex::new(HashMap::new()),
        })
    }

    pub fn insert(&self, job: Job) {
        let mut g = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        g.insert(job.id.clone(), job);
    }

    pub fn get(&self, id: &str) -> Option<Job> {
        let g = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        g.get(id).cloned()
    }

    /// 原子更新 — closure 接收 mut Job ref。`true` = job 存在并被更新。
    pub fn update<F: FnOnce(&mut Job)>(&self, id: &str, f: F) -> bool {
        let mut g = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(j) = g.get_mut(id) {
            f(j);
            true
        } else {
            false
        }
    }

    /// 在 FIFO 队列中该 job 之前还有多少个 Queued。
    /// 不存在 → 0。已 Running/Done 等 → 0（队列里只看 Queued）。
    pub fn queue_position(&self, id: &str) -> usize {
        let g = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        let Some(target) = g.get(id) else {
            return 0;
        };
        if target.state != JobState::Queued {
            return 0;
        }
        let target_created = target.created_at;
        g.values()
            .filter(|j| j.state == JobState::Queued && j.created_at < target_created)
            .count()
    }

    /// 服务重启或紧急停机时调用 — 把所有 Queued/Running 标 Cancelled + 加 warning。
    pub fn cancel_all_running(&self) {
        let mut g = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        for j in g.values_mut() {
            if matches!(j.state, JobState::Running | JobState::Queued) {
                j.state = JobState::Cancelled;
                j.warnings.push("server restarted, please resubmit".into());
            }
        }
    }

    /// 当前 in-flight (Queued + Running) 数量 — 给 metrics 用。
    pub fn in_flight_count(&self) -> usize {
        let g = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        g.values()
            .filter(|j| matches!(j.state, JobState::Queued | JobState::Running))
            .count()
    }
}

impl Default for JobRegistry {
    fn default() -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;
    use std::time::Duration;

    #[test]
    fn insert_get_roundtrip() {
        let r = JobRegistry::new();
        r.insert(Job::new("j1".into()));
        let j = r.get("j1").unwrap();
        assert_eq!(j.state, JobState::Queued);
        assert_eq!(j.stage, JobStage::Queued);
        assert_eq!(j.progress, 0.0);
    }

    #[test]
    fn get_unknown_returns_none() {
        let r = JobRegistry::new();
        assert!(r.get("nope").is_none());
    }

    #[test]
    fn update_changes_state() {
        let r = JobRegistry::new();
        r.insert(Job::new("j1".into()));
        assert!(r.update("j1", |j| j.state = JobState::Running));
        assert_eq!(r.get("j1").unwrap().state, JobState::Running);
    }

    #[test]
    fn update_unknown_returns_false() {
        let r = JobRegistry::new();
        assert!(!r.update("nope", |j| j.state = JobState::Running));
    }

    #[test]
    fn cancel_all_running_marks_them_with_warning() {
        let r = JobRegistry::new();
        let mut j = Job::new("j1".into());
        j.state = JobState::Running;
        r.insert(j);
        r.cancel_all_running();
        let got = r.get("j1").unwrap();
        assert_eq!(got.state, JobState::Cancelled);
        assert!(!got.warnings.is_empty());
        assert!(got.warnings[0].contains("server restarted"));
    }

    #[test]
    fn cancel_all_running_skips_done_and_failed() {
        let r = JobRegistry::new();
        let mut d = Job::new("done".into());
        d.state = JobState::Done;
        r.insert(d);
        let mut f = Job::new("failed".into());
        f.state = JobState::Failed;
        r.insert(f);
        r.cancel_all_running();
        assert_eq!(r.get("done").unwrap().state, JobState::Done);
        assert_eq!(r.get("failed").unwrap().state, JobState::Failed);
    }

    #[test]
    fn queue_position_fifo() {
        let r = JobRegistry::new();
        r.insert(Job::new("j1".into()));
        sleep(Duration::from_millis(2));
        r.insert(Job::new("j2".into()));
        sleep(Duration::from_millis(2));
        r.insert(Job::new("j3".into()));
        assert_eq!(r.queue_position("j1"), 0);
        assert_eq!(r.queue_position("j2"), 1);
        assert_eq!(r.queue_position("j3"), 2);
    }

    #[test]
    fn queue_position_zero_for_running() {
        let r = JobRegistry::new();
        let mut j = Job::new("j1".into());
        j.state = JobState::Running;
        r.insert(j);
        assert_eq!(r.queue_position("j1"), 0);
    }

    #[test]
    fn queue_position_zero_for_unknown() {
        let r = JobRegistry::new();
        assert_eq!(r.queue_position("nope"), 0);
    }

    #[test]
    fn in_flight_count_tracks_queued_and_running() {
        let r = JobRegistry::new();
        r.insert(Job::new("q".into()));
        let mut running = Job::new("r".into());
        running.state = JobState::Running;
        r.insert(running);
        let mut done = Job::new("d".into());
        done.state = JobState::Done;
        r.insert(done);
        assert_eq!(r.in_flight_count(), 2);
    }

    #[test]
    fn job_kind_str_roundtrip() {
        assert_eq!(JobKind::Asr.as_str(), "asr");
        assert_eq!(JobKind::IngestBatch.as_str(), "ingest_batch");
        assert_eq!(JobKind::from_str_kind("agent"), Some(JobKind::Agent));
        assert_eq!(JobKind::from_str_kind("nope"), None);
    }

    #[test]
    fn delivery_contract_default_per_kind() {
        // ASR is idempotent (re-transcribe same file = same result) → at_least_once.
        assert_eq!(JobKind::Asr.default_delivery(), DeliveryContract::AtLeastOnce);
        // agent may have side effects (LLM spend, write item) → at_most_once unless deduped.
        assert_eq!(JobKind::Agent.default_delivery(), DeliveryContract::AtMostOnce);
    }

    #[test]
    fn job_record_serde_state_snake_case() {
        // JobState serde must stay snake_case (DB stores 'queued'/'running'/...).
        assert_eq!(serde_json::to_string(&JobState::Queued).unwrap(), "\"queued\"");
        assert_eq!(serde_json::to_string(&JobState::Running).unwrap(), "\"running\"");
    }

    #[test]
    fn job_error_serde_roundtrip() {
        let e = JobError { message: "boom".into(), code: "asr-engine-failed".into() };
        let s = serde_json::to_string(&e).unwrap();
        let d: JobError = serde_json::from_str(&s).unwrap();
        assert_eq!(d.message, "boom");
        assert_eq!(d.code, "asr-engine-failed");
    }
}

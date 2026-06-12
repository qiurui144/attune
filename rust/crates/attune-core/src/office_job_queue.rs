//! Office helper job types — shared enums + the durable [`JobRecord`].
//!
//! G5 (2026-06-11): the in-memory `JobRegistry` state machine that lived here was
//! replaced by the durable SQLite job queue (`crate::store::job_queue` +
//! `crate::job_handler`). Jobs now survive restart: `Store::recover_on_boot`
//! requeues interrupted idempotent kinds instead of mass-cancelling.
//! Spec: docs/superpowers/specs/2026-06-10-k3-g5-durable-job-queue.md

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
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

#[cfg(test)]
mod tests {
    use super::*;
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

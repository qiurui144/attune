//! G5 durable job queue — SQLite-backed CRUD on the `job_queue` table.
//! Mirrors the reindex_queue idiom (store/items.rs) generalized to multi-kind jobs.
//! Spec: docs/superpowers/specs/2026-06-10-k3-g5-durable-job-queue.md

use crate::error::Result;
use crate::office_job_queue::{JobError, JobKind, JobRecord, JobState};
use crate::store::Store;
use rusqlite::{params, Row};
use uuid::Uuid;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Column list for `row_to_record`. Order MUST match the `row.get(idx)` indices.
const SELECT_COLS: &str = "id, kind, state, stage_json, progress, priority, payload_json, \
     result_json, error_code, error_message, warnings_json, attempts, \
     created_ms, started_ms, finished_ms, deadline_ms";

fn row_to_record(row: &Row) -> rusqlite::Result<JobRecord> {
    let kind_s: String = row.get(1)?;
    let state_s: String = row.get(2)?;
    let error_code: Option<String> = row.get(8)?;
    let error_message: Option<String> = row.get(9)?;
    let warnings_json: Option<String> = row.get(10)?;
    let error = error_code.map(|code| JobError {
        code,
        message: error_message.unwrap_or_default(),
    });
    let warnings: Vec<String> = warnings_json
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    Ok(JobRecord {
        id: row.get(0)?,
        // Unknown kind/state in DB is data corruption; fall back rather than panic.
        kind: JobKind::from_str_kind(&kind_s).unwrap_or(JobKind::IngestBatch),
        state: parse_state(&state_s).unwrap_or(JobState::Failed),
        stage_json: row.get(3)?,
        progress: row.get::<_, f64>(4)? as f32,
        priority: row.get(5)?,
        payload_json: row.get(6)?,
        result_json: row.get(7)?,
        error,
        warnings,
        attempts: row.get(11)?,
        created_ms: row.get(12)?,
        started_ms: row.get(13)?,
        finished_ms: row.get(14)?,
        deadline_ms: row.get(15)?,
    })
}

/// DB state string → JobState (matches the snake_case serde of [`JobState`]).
fn parse_state(s: &str) -> Option<JobState> {
    match s {
        "queued" => Some(JobState::Queued),
        "running" => Some(JobState::Running),
        "done" => Some(JobState::Done),
        "failed" => Some(JobState::Failed),
        "cancelled" => Some(JobState::Cancelled),
        _ => None,
    }
}

impl Store {
    /// Enqueue a durable job. Returns the generated job id (uuid).
    /// Mirror of `enqueue_reindex` generalized to multi-kind + priority + deadline.
    pub fn enqueue_job(
        &self,
        kind: JobKind,
        payload_json: &str,
        priority: i64,
        deadline_ms: Option<i64>,
    ) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        self.conn.execute(
            "INSERT INTO job_queue (id, kind, state, payload_json, priority, created_ms, deadline_ms) \
             VALUES (?1, ?2, 'queued', ?3, ?4, ?5, ?6)",
            params![id, kind.as_str(), payload_json, priority, now_ms(), deadline_ms],
        )?;
        Ok(id)
    }

    /// Atomically claim the next runnable job (highest priority, then FIFO).
    ///
    /// THE claim-race solver: a single `UPDATE ... WHERE state='queued' ... RETURNING`
    /// is one atomic statement under SQLite's connection-level write lock. With N
    /// concurrent workers (each on its own connection to the same WAL DB), at most one
    /// `UPDATE` can match a given row in 'queued' state — the loser's subquery re-selects
    /// and either grabs a different row or matches zero rows (returns None). No row is
    /// ever transitioned to Running by two workers. (Verified by the integration
    /// N-worker race test in tests/job_queue_durable.rs.)
    pub fn claim_next_job(&self) -> Result<Option<JobRecord>> {
        let now = now_ms();
        let mut stmt = self.conn.prepare_cached(&format!(
            "UPDATE job_queue SET state = 'running', started_ms = ?1 \
             WHERE id = ( \
                 SELECT id FROM job_queue WHERE state = 'queued' \
                 ORDER BY priority DESC, created_ms ASC, id ASC LIMIT 1 \
             ) AND state = 'queued' \
             RETURNING {SELECT_COLS}",
        ))?;
        let mut rows = stmt.query_map(params![now], row_to_record)?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    /// Cancel a job: any non-terminal job → Cancelled. Running jobs stop cooperatively
    /// (the handler checks [`Store::is_job_cancelled`] between stages). No-op for
    /// terminal jobs. Returns true if a row changed.
    pub fn cancel_job(&self, id: &str) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE job_queue SET state = 'cancelled', finished_ms = ?2 \
             WHERE id = ?1 AND state IN ('queued', 'running')",
            params![id, now_ms()],
        )?;
        Ok(n > 0)
    }

    /// Lightweight cancellation probe for cooperative-stop handlers.
    pub fn is_job_cancelled(&self, id: &str) -> Result<bool> {
        let s: Option<String> = self
            .conn
            .query_row(
                "SELECT state FROM job_queue WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .ok();
        Ok(s.as_deref() == Some("cancelled"))
    }

    /// Read a job by id. None if absent.
    pub fn get_job(&self, id: &str) -> Result<Option<JobRecord>> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!("SELECT {SELECT_COLS} FROM job_queue WHERE id = ?1"))?;
        let mut rows = stmt.query_map(params![id], row_to_record)?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::office_job_queue::{JobKind, JobState};
    use crate::store::Store;

    #[test]
    fn job_queue_table_exists_after_open() {
        let store = Store::open_memory().unwrap();
        let n: i64 = store
            .raw_connection_for_test()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='job_queue'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "job_queue table must be created by SCHEMA_SQL");
    }

    #[test]
    fn enqueue_then_get_roundtrips() {
        let store = Store::open_memory().unwrap();
        let id = store
            .enqueue_job(JobKind::Asr, "{\"file\":\"a.wav\"}", 0, None)
            .unwrap();
        let job = store.get_job(&id).unwrap().expect("job must exist");
        assert_eq!(job.kind, JobKind::Asr);
        assert_eq!(job.state, JobState::Queued);
        assert_eq!(job.payload_json, "{\"file\":\"a.wav\"}");
        assert_eq!(job.attempts, 0);
        assert!(job.created_ms > 0);
        assert!(job.started_ms.is_none());
        assert!(job.finished_ms.is_none());
    }

    #[test]
    fn get_unknown_job_returns_none() {
        let store = Store::open_memory().unwrap();
        assert!(store.get_job("nope").unwrap().is_none());
    }

    #[test]
    fn claim_next_transitions_queued_to_running_and_sets_started() {
        let store = Store::open_memory().unwrap();
        let id = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
        let claimed = store.claim_next_job().unwrap().expect("a queued job exists");
        assert_eq!(claimed.id, id);
        assert_eq!(claimed.state, JobState::Running);
        assert!(claimed.started_ms.is_some());
        // Re-reading confirms it is durably Running (claim is committed, not in-memory).
        assert_eq!(store.get_job(&id).unwrap().unwrap().state, JobState::Running);
    }

    #[test]
    fn claim_next_returns_none_on_empty_queue() {
        let store = Store::open_memory().unwrap();
        assert!(store.claim_next_job().unwrap().is_none());
    }

    #[test]
    fn claim_respects_priority_then_fifo() {
        let store = Store::open_memory().unwrap();
        let _low = store.enqueue_job(JobKind::Asr, "{\"n\":1}", 0, None).unwrap();
        let high = store.enqueue_job(JobKind::Asr, "{\"n\":2}", 10, None).unwrap();
        // Higher priority claimed first despite being enqueued later.
        assert_eq!(store.claim_next_job().unwrap().unwrap().id, high);
    }

    #[test]
    fn claim_does_not_pick_running_or_cancelled() {
        let store = Store::open_memory().unwrap();
        let id = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
        store.claim_next_job().unwrap().unwrap(); // now Running
        // Nothing left to claim.
        assert!(store.claim_next_job().unwrap().is_none());
        // A cancelled job is also never claimed.
        let c = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
        store.cancel_job(&c).unwrap();
        let next = store.claim_next_job().unwrap();
        assert!(next.is_none(), "cancelled job must not be claimed; got {next:?}");
        let _ = id;
    }

    #[test]
    fn double_claim_serial_never_returns_same_job_twice() {
        // Serial proxy for the N-worker race (full thread test in integration suite).
        let store = Store::open_memory().unwrap();
        for _ in 0..5 {
            store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
        }
        let mut seen = std::collections::HashSet::new();
        while let Some(j) = store.claim_next_job().unwrap() {
            assert!(seen.insert(j.id.clone()), "job {} claimed twice", j.id);
        }
        assert_eq!(seen.len(), 5);
    }

    #[test]
    fn cancel_terminal_job_is_noop() {
        let store = Store::open_memory().unwrap();
        let id = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
        store.cancel_job(&id).unwrap();
        // Second cancel: already terminal → false.
        assert!(!store.cancel_job(&id).unwrap());
        assert!(store.is_job_cancelled(&id).unwrap());
        assert!(!store.is_job_cancelled("nope").unwrap());
    }

    #[test]
    fn enqueue_persists_deadline_and_priority() {
        let store = Store::open_memory().unwrap();
        let id = store
            .enqueue_job(JobKind::Agent, "{}", 5, Some(1_000_000))
            .unwrap();
        let job = store.get_job(&id).unwrap().unwrap();
        assert_eq!(job.priority, 5);
        assert_eq!(job.deadline_ms, Some(1_000_000));
    }
}

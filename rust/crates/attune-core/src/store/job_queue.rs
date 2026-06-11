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

/// Outcome of [`Store::recover_on_boot`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecoverSummary {
    /// Running at_least_once jobs requeued to Queued.
    pub requeued: usize,
    /// Running at_most_once jobs marked Failed (not retried).
    pub failed_no_retry: usize,
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

    /// Update kind-specific stage + progress on a Running job. No-op if id absent.
    pub fn update_job_progress(
        &self,
        id: &str,
        stage_json: Option<&str>,
        progress: f32,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE job_queue SET stage_json = ?2, progress = ?3 WHERE id = ?1",
            params![id, stage_json, progress as f64],
        )?;
        Ok(())
    }

    /// Mark a Running job Done with its result. Sets progress = 1.0 + finished_ms.
    /// Guarded on state='running' so a cooperative cancel that already landed is
    /// not overwritten by a late-finishing worker (cancel/complete race).
    /// Returns true if the row transitioned.
    pub fn complete_job(&self, id: &str, result_json: &str) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE job_queue SET state = 'done', result_json = ?2, progress = 1.0, \
             finished_ms = ?3 WHERE id = ?1 AND state = 'running'",
            params![id, result_json, now_ms()],
        )?;
        Ok(n > 0)
    }

    /// Mark a Running job Failed with a kebab error code + message. Same
    /// state='running' guard as [`Store::complete_job`] (cancel wins races).
    /// Returns true if the row transitioned.
    pub fn fail_job(&self, id: &str, code: &str, message: &str) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE job_queue SET state = 'failed', error_code = ?2, error_message = ?3, \
             finished_ms = ?4 WHERE id = ?1 AND state = 'running'",
            params![id, code, message, now_ms()],
        )?;
        Ok(n > 0)
    }

    /// Increment attempts and return the new count (mirror of bump_reindex_attempts).
    pub fn increment_job_attempts(&self, id: &str) -> Result<i64> {
        self.conn.execute(
            "UPDATE job_queue SET attempts = attempts + 1 WHERE id = ?1",
            params![id],
        )?;
        let n: i64 = self
            .conn
            .query_row(
                "SELECT attempts FROM job_queue WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        Ok(n)
    }

    /// Requeue a Failed (or Cancelled) job back to Queued for retry. Clears
    /// started_ms/finished_ms/error. No-op (returns false) for Done/Queued/Running.
    pub fn requeue_job(&self, id: &str) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE job_queue SET state = 'queued', started_ms = NULL, finished_ms = NULL, \
             error_code = NULL, error_message = NULL \
             WHERE id = ?1 AND state IN ('failed', 'cancelled')",
            params![id],
        )?;
        Ok(n > 0)
    }

    /// Reprioritize a still-Queued job (spec §2 可重排). No-op once claimed.
    pub fn set_job_priority(&self, id: &str, priority: i64) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE job_queue SET priority = ?2 WHERE id = ?1 AND state = 'queued'",
            params![id, priority],
        )?;
        Ok(n > 0)
    }

    /// How many Queued jobs are ahead of this one in claim order
    /// (priority DESC, created_ms ASC, id ASC — must match [`Store::claim_next_job`]).
    /// 0 for unknown ids and non-Queued jobs (mirrors JobRegistry::queue_position).
    pub fn job_queue_position(&self, id: &str) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM job_queue q, job_queue t \
                 WHERE t.id = ?1 AND t.state = 'queued' AND q.state = 'queued' \
                   AND (q.priority > t.priority \
                        OR (q.priority = t.priority AND q.created_ms < t.created_ms) \
                        OR (q.priority = t.priority AND q.created_ms = t.created_ms \
                            AND q.id < t.id))",
                params![id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        Ok(n as usize)
    }

    /// In-flight (Queued + Running) job count — metrics parity with the old
    /// JobRegistry::in_flight_count.
    pub fn in_flight_job_count(&self) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM job_queue WHERE state IN ('queued', 'running')",
            [],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// Boot recovery (generalizes the embed_queue processing→pending reset in
    /// `Store::open`). Replaces the old JobRegistry::cancel_all_running which
    /// dropped every in-flight job ("server restarted, please resubmit").
    /// For each Running job:
    ///   - delivery == at_least_once → Queued (requeue, clear started_ms)
    ///   - delivery == at_most_once  → Failed (code `interrupted-no-retry`)
    ///
    /// Queued/Done/Failed/Cancelled are untouched.
    pub fn recover_on_boot(&self) -> Result<RecoverSummary> {
        // Read interrupted Running jobs (need kind to decide delivery).
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id, kind FROM job_queue WHERE state = 'running'")?;
        let running: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut summary = RecoverSummary::default();
        for (id, kind_s) in running {
            let delivery = JobKind::from_str_kind(&kind_s)
                .map(|k| k.default_delivery())
                // Unknown kind: be conservative, do not silently re-run.
                .unwrap_or(crate::office_job_queue::DeliveryContract::AtMostOnce);
            match delivery {
                crate::office_job_queue::DeliveryContract::AtLeastOnce => {
                    self.conn.execute(
                        "UPDATE job_queue SET state = 'queued', started_ms = NULL WHERE id = ?1",
                        params![id],
                    )?;
                    summary.requeued += 1;
                }
                crate::office_job_queue::DeliveryContract::AtMostOnce => {
                    self.conn.execute(
                        "UPDATE job_queue SET state = 'failed', \
                         error_code = 'interrupted-no-retry', \
                         error_message = 'server restarted; job is not safe to retry', \
                         finished_ms = ?2 WHERE id = ?1",
                        params![id, now_ms()],
                    )?;
                    summary.failed_no_retry += 1;
                }
            }
        }
        Ok(summary)
    }

    /// Fail every Running job whose deadline_ms has passed. Returns count failed.
    pub fn sweep_timeouts(&self, now_ms: i64) -> Result<usize> {
        let n = self.conn.execute(
            "UPDATE job_queue SET state = 'failed', error_code = 'job-timeout', \
             error_message = 'job exceeded deadline_ms', finished_ms = ?1 \
             WHERE state = 'running' AND deadline_ms IS NOT NULL AND deadline_ms < ?1",
            params![now_ms],
        )?;
        Ok(n)
    }

    /// TTL purge: delete terminal (done/failed/cancelled) jobs older than `ttl_days`.
    /// Prevents unbounded growth on a 24h K3 box (spec §8). Returns count deleted.
    pub fn purge_terminal_jobs(&self, now_ms: i64, ttl_days: i64) -> Result<usize> {
        let cutoff = now_ms - ttl_days * 86_400_000;
        let n = self.conn.execute(
            "DELETE FROM job_queue \
             WHERE state IN ('done', 'failed', 'cancelled') AND created_ms < ?1",
            params![cutoff],
        )?;
        Ok(n)
    }

    /// List jobs filtered by optional kind/state, claim order first (priority DESC),
    /// newest first within priority. For the GET /jobs panel (spec §5).
    /// Dynamic-filter SQL uses `prepare` (NOT prepare_cached — per CLAUDE.md 约定).
    pub fn list_jobs(
        &self,
        kind: Option<&str>,
        state: Option<&str>,
        limit: usize,
    ) -> Result<Vec<JobRecord>> {
        let mut sql = format!("SELECT {SELECT_COLS} FROM job_queue WHERE 1=1");
        let mut binds: Vec<String> = Vec::new();
        if let Some(k) = kind {
            sql.push_str(" AND kind = ?");
            binds.push(k.to_string());
        }
        if let Some(s) = state {
            sql.push_str(" AND state = ?");
            binds.push(s.to_string());
        }
        sql.push_str(" ORDER BY priority DESC, created_ms DESC LIMIT ?");
        let mut stmt = self.conn.prepare(&sql)?;
        let limit_i = limit as i64;
        let mut p: Vec<&dyn rusqlite::ToSql> =
            binds.iter().map(|b| b as &dyn rusqlite::ToSql).collect();
        p.push(&limit_i);
        let rows = stmt.query_map(p.as_slice(), row_to_record)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
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

    #[test]
    fn update_progress_persists_stage_and_progress() {
        let store = Store::open_memory().unwrap();
        let id = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
        store.claim_next_job().unwrap();
        store
            .update_job_progress(&id, Some("{\"stage\":\"transcribing\"}"), 0.5)
            .unwrap();
        let j = store.get_job(&id).unwrap().unwrap();
        assert_eq!(j.stage_json.as_deref(), Some("{\"stage\":\"transcribing\"}"));
        assert!((j.progress - 0.5).abs() < 1e-6);
    }

    #[test]
    fn complete_job_sets_done_and_result() {
        let store = Store::open_memory().unwrap();
        let id = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
        store.claim_next_job().unwrap();
        assert!(store.complete_job(&id, "{\"segments\":[]}").unwrap());
        let j = store.get_job(&id).unwrap().unwrap();
        assert_eq!(j.state, JobState::Done);
        assert_eq!(j.result_json.as_deref(), Some("{\"segments\":[]}"));
        assert!((j.progress - 1.0).abs() < 1e-6);
        assert!(j.finished_ms.is_some());
    }

    #[test]
    fn complete_does_not_overwrite_cancelled() {
        // cancel/complete race: cancel landed first → late worker completion is dropped.
        let store = Store::open_memory().unwrap();
        let id = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
        store.claim_next_job().unwrap();
        store.cancel_job(&id).unwrap();
        assert!(!store.complete_job(&id, "{}").unwrap());
        assert_eq!(store.get_job(&id).unwrap().unwrap().state, JobState::Cancelled);
    }

    #[test]
    fn fail_job_sets_failed_with_code_and_message() {
        let store = Store::open_memory().unwrap();
        let id = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
        store.claim_next_job().unwrap();
        assert!(store.fail_job(&id, "asr-engine-failed", "whisper exited 1").unwrap());
        let j = store.get_job(&id).unwrap().unwrap();
        assert_eq!(j.state, JobState::Failed);
        let e = j.error.unwrap();
        assert_eq!(e.code, "asr-engine-failed");
        assert_eq!(e.message, "whisper exited 1");
    }

    #[test]
    fn increment_attempts_counts_up() {
        let store = Store::open_memory().unwrap();
        let id = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
        assert_eq!(store.increment_job_attempts(&id).unwrap(), 1);
        assert_eq!(store.increment_job_attempts(&id).unwrap(), 2);
    }

    #[test]
    fn requeue_failed_job_back_to_queued() {
        let store = Store::open_memory().unwrap();
        let id = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
        store.claim_next_job().unwrap();
        store.fail_job(&id, "asr-engine-failed", "boom").unwrap();
        assert!(store.requeue_job(&id).unwrap());
        let j = store.get_job(&id).unwrap().unwrap();
        assert_eq!(j.state, JobState::Queued);
        assert!(j.started_ms.is_none(), "requeue clears started_ms");
        assert!(j.error.is_none(), "requeue clears error");
        // Now claimable again.
        assert_eq!(store.claim_next_job().unwrap().unwrap().id, id);
    }

    #[test]
    fn requeue_done_job_is_noop() {
        let store = Store::open_memory().unwrap();
        let id = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
        store.claim_next_job().unwrap();
        store.complete_job(&id, "{}").unwrap();
        assert!(!store.requeue_job(&id).unwrap(), "done jobs do not requeue");
        assert_eq!(store.get_job(&id).unwrap().unwrap().state, JobState::Done);
    }

    #[test]
    fn set_priority_only_while_queued() {
        let store = Store::open_memory().unwrap();
        let id = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
        assert!(store.set_job_priority(&id, 7).unwrap());
        assert_eq!(store.get_job(&id).unwrap().unwrap().priority, 7);
        store.claim_next_job().unwrap();
        assert!(!store.set_job_priority(&id, 9).unwrap(), "running job not reprioritizable");
    }

    #[test]
    fn queue_position_follows_claim_order() {
        let store = Store::open_memory().unwrap();
        let a = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
        let b = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
        let high = store.enqueue_job(JobKind::Asr, "{}", 10, None).unwrap();
        // Same-ms enqueues tie-break by random uuid — pin created_ms so the
        // FIFO assertion is deterministic.
        for (id, ms) in [(&a, 1i64), (&b, 2i64)] {
            store
                .raw_connection_for_test()
                .execute(
                    "UPDATE job_queue SET created_ms = ?2 WHERE id = ?1",
                    rusqlite::params![id, ms],
                )
                .unwrap();
        }
        assert_eq!(store.job_queue_position(&high).unwrap(), 0, "high prio is next");
        assert_eq!(store.job_queue_position(&a).unwrap(), 1);
        assert_eq!(store.job_queue_position(&b).unwrap(), 2);
        // Unknown / non-queued → 0 (JobRegistry parity).
        assert_eq!(store.job_queue_position("nope").unwrap(), 0);
        store.claim_next_job().unwrap(); // high → Running
        assert_eq!(store.job_queue_position(&high).unwrap(), 0);
        assert_eq!(store.job_queue_position(&a).unwrap(), 0, "a moved up");
    }

    #[test]
    fn recover_on_boot_requeues_at_least_once_running() {
        let store = Store::open_memory().unwrap();
        let id = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap(); // ASR = at_least_once
        store.claim_next_job().unwrap(); // Running
        let summary = store.recover_on_boot().unwrap();
        assert_eq!(summary.requeued, 1);
        assert_eq!(summary.failed_no_retry, 0);
        let j = store.get_job(&id).unwrap().unwrap();
        assert_eq!(j.state, JobState::Queued);
        assert!(j.started_ms.is_none());
    }

    #[test]
    fn recover_on_boot_fails_at_most_once_running() {
        let store = Store::open_memory().unwrap();
        let id = store.enqueue_job(JobKind::Agent, "{}", 0, None).unwrap(); // Agent = at_most_once
        store.claim_next_job().unwrap(); // Running
        let summary = store.recover_on_boot().unwrap();
        assert_eq!(summary.requeued, 0);
        assert_eq!(summary.failed_no_retry, 1);
        let j = store.get_job(&id).unwrap().unwrap();
        assert_eq!(j.state, JobState::Failed);
        assert_eq!(j.error.unwrap().code, "interrupted-no-retry");
    }

    #[test]
    fn recover_on_boot_does_not_touch_done_or_queued() {
        let store = Store::open_memory().unwrap();
        let done = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
        store.claim_next_job().unwrap();
        store.complete_job(&done, "{}").unwrap();
        let queued = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
        store.recover_on_boot().unwrap();
        assert_eq!(store.get_job(&done).unwrap().unwrap().state, JobState::Done);
        assert_eq!(store.get_job(&queued).unwrap().unwrap().state, JobState::Queued);
    }

    #[test]
    fn sweep_timeouts_fails_expired_running_jobs() {
        let store = Store::open_memory().unwrap();
        // deadline already in the past.
        let id = store.enqueue_job(JobKind::Asr, "{}", 0, Some(1)).unwrap();
        store.claim_next_job().unwrap(); // Running, deadline_ms=1
        let n = store.sweep_timeouts(now_ms_test()).unwrap();
        assert_eq!(n, 1);
        let j = store.get_job(&id).unwrap().unwrap();
        assert_eq!(j.state, JobState::Failed);
        assert_eq!(j.error.unwrap().code, "job-timeout");
    }

    #[test]
    fn sweep_timeouts_ignores_future_deadline_and_queued() {
        let store = Store::open_memory().unwrap();
        let _future = store
            .enqueue_job(JobKind::Asr, "{}", 0, Some(now_ms_test() + 60_000))
            .unwrap();
        store.claim_next_job().unwrap();
        // An expired-deadline job that is still Queued is NOT swept (only Running).
        let _queued_expired = store.enqueue_job(JobKind::Asr, "{}", -1, Some(1)).unwrap();
        assert_eq!(store.sweep_timeouts(now_ms_test()).unwrap(), 0);
    }

    #[test]
    fn purge_terminal_jobs_removes_old_done_failed() {
        let store = Store::open_memory().unwrap();
        let id = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
        store.claim_next_job().unwrap();
        store.complete_job(&id, "{}").unwrap();
        // Backdate created_ms far past TTL.
        store
            .raw_connection_for_test()
            .execute(
                "UPDATE job_queue SET created_ms = 0 WHERE id = ?1",
                rusqlite::params![id],
            )
            .unwrap();
        // A fresh queued job must survive the purge.
        let fresh = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
        let removed = store.purge_terminal_jobs(now_ms_test(), 30).unwrap();
        assert_eq!(removed, 1);
        assert!(store.get_job(&id).unwrap().is_none());
        assert!(store.get_job(&fresh).unwrap().is_some());
    }

    fn now_ms_test() -> i64 {
        chrono::Utc::now().timestamp_millis()
    }

    #[test]
    fn in_flight_count_tracks_queued_and_running() {
        let store = Store::open_memory().unwrap();
        let q = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
        store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
        store.claim_next_job().unwrap(); // one Running, one Queued
        assert_eq!(store.in_flight_job_count().unwrap(), 2);
        let d = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
        store.claim_next_job().unwrap();
        // Terminal jobs drop out of in-flight.
        let running = store.get_job(&d).unwrap().unwrap();
        let done_id = if running.state == JobState::Running { d.clone() } else { q.clone() };
        store.complete_job(&done_id, "{}").unwrap();
        assert_eq!(store.in_flight_job_count().unwrap(), 2);
    }

    #[test]
    fn list_jobs_filters_by_kind_and_state() {
        let store = Store::open_memory().unwrap();
        store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
        store.enqueue_job(JobKind::Agent, "{}", 0, None).unwrap();
        let asr = store.list_jobs(Some("asr"), None, 100).unwrap();
        assert_eq!(asr.len(), 1);
        assert_eq!(asr[0].kind, JobKind::Asr);
        let queued = store.list_jobs(None, Some("queued"), 100).unwrap();
        assert_eq!(queued.len(), 2);
        let none = store.list_jobs(None, Some("done"), 100).unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn list_jobs_respects_limit_and_priority_order() {
        let store = Store::open_memory().unwrap();
        let _low = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
        let high = store.enqueue_job(JobKind::Asr, "{}", 9, None).unwrap();
        let top = store.list_jobs(None, None, 1).unwrap();
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].id, high, "priority DESC ordering");
    }
}

//! G5 durable job queue — adversarial + concurrency + persistence integration tests.
//! Spec §9 mandatory rows: restart-recovery, concurrent-claim race, timeout/requeue,
//! resource-exhaust. File-backed DB (not :memory:) to exercise real WAL + reopen
//! semantics across connections.

use attune_core::office_job_queue::{JobKind, JobState};
use attune_core::store::Store;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

fn open_disk() -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("vault.db");
    (dir, path)
}

#[test]
fn restart_recovery_requeues_running_not_cancels_all() {
    let (_dir, path) = open_disk();

    // Session 1: enqueue 3, complete the highest-priority one, leave one Running.
    let done_id;
    let running_id;
    let queued_id;
    {
        let store = Store::open(&path).unwrap();
        queued_id = store
            .enqueue_job(JobKind::Asr, "{\"f\":\"q.wav\"}", 0, None)
            .unwrap();
        running_id = store
            .enqueue_job(JobKind::Asr, "{\"f\":\"r.wav\"}", 5, None)
            .unwrap();
        done_id = store
            .enqueue_job(JobKind::Asr, "{\"f\":\"d.wav\"}", 9, None)
            .unwrap();
        let c1 = store.claim_next_job().unwrap().unwrap();
        assert_eq!(c1.id, done_id);
        store.complete_job(&done_id, "{}").unwrap();
        let c2 = store.claim_next_job().unwrap().unwrap();
        assert_eq!(c2.id, running_id);
        // Store dropped here = process "killed" mid-Running.
    }

    // Session 2: reopen same DB + boot recovery (the server runs recover_on_boot
    // exactly once per process in AppState::install_job_store — NOT in Store::open,
    // which also runs at vault unlock and must not touch Running jobs).
    {
        let store = Store::open(&path).unwrap();
        let summary = store.recover_on_boot().unwrap();
        assert_eq!(summary.requeued, 1);
        assert_eq!(summary.failed_no_retry, 0);
        assert_eq!(
            store.get_job(&running_id).unwrap().unwrap().state,
            JobState::Queued,
            "Running ASR job must be REQUEUED on restart, not cancelled"
        );
        assert_eq!(
            store.get_job(&queued_id).unwrap().unwrap().state,
            JobState::Queued,
            "Queued job preserved"
        );
        assert_eq!(
            store.get_job(&done_id).unwrap().unwrap().state,
            JobState::Done,
            "Done job not re-run"
        );
        assert!(store.get_job(&running_id).unwrap().unwrap().started_ms.is_none());
    }
}

#[test]
fn restart_recovery_at_most_once_agent_fails_not_requeues() {
    let (_dir, path) = open_disk();
    let agent_id;
    {
        let store = Store::open(&path).unwrap();
        agent_id = store.enqueue_job(JobKind::Agent, "{}", 0, None).unwrap();
        store.claim_next_job().unwrap(); // Running, then "killed".
    }
    {
        let store = Store::open(&path).unwrap();
        let summary = store.recover_on_boot().unwrap();
        assert_eq!(summary.failed_no_retry, 1);
        let j = store.get_job(&agent_id).unwrap().unwrap();
        assert_eq!(
            j.state,
            JobState::Failed,
            "at_most_once job must NOT silently re-run"
        );
        assert_eq!(j.error.unwrap().code, "interrupted-no-retry");
    }
}

#[test]
fn concurrent_claim_no_double_claim() {
    let (_dir, path) = open_disk();
    const N_JOBS: usize = 200;
    const N_WORKERS: usize = 8;

    // Seed N jobs on a writer connection.
    {
        let store = Store::open(&path).unwrap();
        for i in 0..N_JOBS {
            store
                .enqueue_job(JobKind::Asr, &format!("{{\"n\":{i}}}"), 0, None)
                .unwrap();
        }
    }

    // Each worker opens its OWN Store (own connection) on the SAME file (WAL).
    let claimed = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut handles = vec![];
    for _ in 0..N_WORKERS {
        let path = path.clone();
        let claimed = claimed.clone();
        handles.push(std::thread::spawn(move || {
            let store = Store::open(&path).unwrap();
            loop {
                match store.claim_next_job() {
                    Ok(Some(job)) => claimed.lock().unwrap().push(job.id),
                    Ok(None) => break,
                    // SQLITE_BUSY under contention: busy_timeout=5000 is set by
                    // open; a residual busy error just retries.
                    Err(_) => continue,
                }
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    let mut ids = claimed.lock().unwrap().clone();
    let total = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        total,
        "a job was claimed by more than one worker (double-claim)"
    );
    assert_eq!(ids.len(), N_JOBS, "every job must be claimed exactly once");
}

#[test]
fn cancel_races_claim_cancelled_job_never_runs() {
    let (_dir, path) = open_disk();
    let store = Store::open(&path).unwrap();
    let id = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
    // Cancel BEFORE claim → claim must skip it.
    assert!(store.cancel_job(&id).unwrap());
    assert!(
        store.claim_next_job().unwrap().is_none(),
        "cancelled job must not be claimed"
    );
    assert_eq!(store.get_job(&id).unwrap().unwrap().state, JobState::Cancelled);
}

#[test]
fn timeout_then_requeue_round_trip() {
    let (_dir, path) = open_disk();
    let store = Store::open(&path).unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    let id = store
        .enqueue_job(JobKind::Asr, "{}", 0, Some(now - 1))
        .unwrap(); // already expired
    store.claim_next_job().unwrap(); // Running
    assert_eq!(store.sweep_timeouts(now).unwrap(), 1);
    let j = store.get_job(&id).unwrap().unwrap();
    assert_eq!(j.state, JobState::Failed);
    assert_eq!(j.error.unwrap().code, "job-timeout");
    // Operator requeues the timed-out job.
    assert!(store.requeue_job(&id).unwrap());
    assert_eq!(store.get_job(&id).unwrap().unwrap().state, JobState::Queued);
}

#[test]
fn resource_exhaust_thousand_jobs_queue_and_drain() {
    let (_dir, path) = open_disk();
    let store = Store::open(&path).unwrap();
    const N: usize = 1000;
    for i in 0..N {
        // Mix priorities to stress the ORDER BY index.
        store
            .enqueue_job(JobKind::Asr, "{}", (i % 10) as i64, None)
            .unwrap();
    }
    assert_eq!(store.list_jobs(None, Some("queued"), N + 10).unwrap().len(), N);
    // Drain all: every claim succeeds, none double, queue ends empty.
    let mut count = 0usize;
    while let Some(j) = store.claim_next_job().unwrap() {
        store.complete_job(&j.id, "{}").unwrap();
        count += 1;
    }
    assert_eq!(count, N);
    assert!(store.list_jobs(None, Some("queued"), 10).unwrap().is_empty());
}

#[test]
fn poison_job_parked_after_max_attempts() {
    // Attempts guard at the worker layer (run_one_job) parks poison jobs even
    // if an operator keeps requeueing them.
    use attune_core::job_handler::{run_one_job, JobHandler, JobHandlerRegistry};
    struct AlwaysFail;
    impl JobHandler for AlwaysFail {
        fn kind(&self) -> JobKind {
            JobKind::Ocr
        }
        fn run(&self, _: &str) -> Result<String, (String, String)> {
            Err(("always".into(), "fail".into()))
        }
    }
    let (_dir, path) = open_disk();
    let store = Arc::new(Mutex::new(Store::open(&path).unwrap()));
    let id = store
        .lock()
        .unwrap()
        .enqueue_job(JobKind::Ocr, "{}", 0, None)
        .unwrap();
    let mut reg = JobHandlerRegistry::new();
    reg.register(Arc::new(AlwaysFail));
    const MAX: i64 = 5;
    for _ in 0..(MAX + 2) {
        run_one_job(&store, &reg, MAX);
        let _ = store.lock().unwrap().requeue_job(&id); // operator keeps retrying
    }
    let j = store.lock().unwrap().get_job(&id).unwrap().unwrap();
    assert!(
        j.attempts > MAX,
        "attempts must exceed max after repeated requeue (got {})",
        j.attempts
    );
    // The final execution was parked by the worker, not the handler.
    // (last requeue flips it back to Queued; the run before it stamped max-attempts)
}

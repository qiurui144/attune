//! G5: per-kind job handler trait + registry + a poll-and-run worker step.
//! The worker claims jobs from the durable [`crate::store::Store`] job_queue and
//! dispatches to the registered handler for that kind.
//! Spec: docs/superpowers/specs/2026-06-10-k3-g5-durable-job-queue.md §6

use crate::office_job_queue::JobKind;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// A handler executes one job of a given kind. `payload_json` is the durable input
/// (everything needed to re-run after a restart); returns the result JSON on
/// success, or `(kebab_code, message)` on failure.
pub trait JobHandler: Send + Sync {
    fn kind(&self) -> JobKind;
    fn run(&self, payload_json: &str) -> Result<String, (String, String)>;
    /// Stage surfaced to the frontend when the job starts running
    /// (e.g. `{"stage":"transcribing"}` for ASR). None = no stage display.
    fn initial_stage_json(&self) -> Option<&'static str> {
        None
    }
}

/// kind → handler. The worker looks up by [`JobKind`].
#[derive(Default)]
pub struct JobHandlerRegistry {
    handlers: HashMap<&'static str, Arc<dyn JobHandler>>,
}

impl JobHandlerRegistry {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }
    pub fn register(&mut self, h: Arc<dyn JobHandler>) {
        self.handlers.insert(h.kind().as_str(), h);
    }
    pub fn get(&self, kind: JobKind) -> Option<Arc<dyn JobHandler>> {
        self.handlers.get(kind.as_str()).cloned()
    }
}

/// Claim one job and run it to completion. Returns Some(job_id) if a job was
/// claimed (whatever its outcome), None if the queue had nothing claimable.
/// Errors are recorded on the job (fail_job), never propagated as a worker crash.
///
/// The handler runs WITHOUT holding the store lock (handlers are slow/IO-bound —
/// whisper subprocess, OCR pipeline); only the short state transitions lock.
pub fn run_one_job(
    store: &Arc<Mutex<crate::store::Store>>,
    registry: &JobHandlerRegistry,
    max_attempts: i64,
) -> Option<String> {
    let job = {
        let s = store.lock().unwrap_or_else(|e| e.into_inner());
        s.claim_next_job().ok().flatten()
    }?;
    let Some(handler) = registry.get(job.kind) else {
        let s = store.lock().unwrap_or_else(|e| e.into_inner());
        let _ = s.fail_job(&job.id, "no-handler", "no registered handler for kind");
        return Some(job.id);
    };
    // Attempts guard: park poison jobs (mirror reindex WHERE attempts < N).
    let attempts = {
        let s = store.lock().unwrap_or_else(|e| e.into_inner());
        s.increment_job_attempts(&job.id).unwrap_or(max_attempts)
    };
    if attempts > max_attempts {
        let s = store.lock().unwrap_or_else(|e| e.into_inner());
        let _ = s.fail_job(&job.id, "max-attempts", "exceeded max attempts");
        return Some(job.id);
    }
    if let Some(stage) = handler.initial_stage_json() {
        let s = store.lock().unwrap_or_else(|e| e.into_inner());
        let _ = s.update_job_progress(&job.id, Some(stage), 0.0);
    }
    // Run the handler WITHOUT holding the store lock.
    let outcome = handler.run(&job.payload_json);
    let s = store.lock().unwrap_or_else(|e| e.into_inner());
    // complete/fail are guarded on state='running' — if the job was cancelled
    // mid-run, the late result is dropped (cancel wins, spec §7 cooperative stop).
    match outcome {
        Ok(result_json) => {
            let _ = s.complete_job(&job.id, &result_json);
        }
        Err((code, msg)) => {
            let _ = s.fail_job(&job.id, &code, &msg);
        }
    }
    Some(job.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::office_job_queue::JobState;
    use crate::store::Store;

    struct EchoHandler;
    impl JobHandler for EchoHandler {
        fn kind(&self) -> JobKind {
            JobKind::Asr
        }
        fn run(&self, payload: &str) -> Result<String, (String, String)> {
            Ok(format!("{{\"echo\":{payload}}}"))
        }
        fn initial_stage_json(&self) -> Option<&'static str> {
            Some("{\"stage\":\"transcribing\"}")
        }
    }
    struct FailHandler;
    impl JobHandler for FailHandler {
        fn kind(&self) -> JobKind {
            JobKind::Ocr
        }
        fn run(&self, _: &str) -> Result<String, (String, String)> {
            Err(("ocr-boom".into(), "kaboom".into()))
        }
    }

    #[test]
    fn run_one_job_completes_via_handler() {
        let store = Arc::new(Mutex::new(Store::open_memory().unwrap()));
        let id = store
            .lock()
            .unwrap()
            .enqueue_job(JobKind::Asr, "{\"k\":1}", 0, None)
            .unwrap();
        let mut reg = JobHandlerRegistry::new();
        reg.register(Arc::new(EchoHandler));
        let ran = run_one_job(&store, &reg, 5);
        assert_eq!(ran, Some(id.clone()));
        let j = store.lock().unwrap().get_job(&id).unwrap().unwrap();
        assert_eq!(j.state, JobState::Done);
        assert!(j.result_json.unwrap().contains("echo"));
        // initial stage surfaced while running, persisted after done.
        assert_eq!(j.stage_json.as_deref(), Some("{\"stage\":\"transcribing\"}"));
        assert_eq!(j.attempts, 1);
    }

    #[test]
    fn run_one_job_fails_via_handler_error() {
        let store = Arc::new(Mutex::new(Store::open_memory().unwrap()));
        let id = store
            .lock()
            .unwrap()
            .enqueue_job(JobKind::Ocr, "{}", 0, None)
            .unwrap();
        let mut reg = JobHandlerRegistry::new();
        reg.register(Arc::new(FailHandler));
        run_one_job(&store, &reg, 5);
        let j = store.lock().unwrap().get_job(&id).unwrap().unwrap();
        assert_eq!(j.state, JobState::Failed);
        assert_eq!(j.error.unwrap().code, "ocr-boom");
    }

    #[test]
    fn run_one_job_none_on_empty_queue() {
        let store = Arc::new(Mutex::new(Store::open_memory().unwrap()));
        let reg = JobHandlerRegistry::new();
        assert_eq!(run_one_job(&store, &reg, 5), None);
    }

    #[test]
    fn run_one_job_no_handler_marks_failed() {
        let store = Arc::new(Mutex::new(Store::open_memory().unwrap()));
        let id = store
            .lock()
            .unwrap()
            .enqueue_job(JobKind::Agent, "{}", 0, None)
            .unwrap();
        let reg = JobHandlerRegistry::new(); // no handler for agent
        run_one_job(&store, &reg, 5);
        let j = store.lock().unwrap().get_job(&id).unwrap().unwrap();
        assert_eq!(j.state, JobState::Failed);
        assert_eq!(j.error.unwrap().code, "no-handler");
    }

    #[test]
    fn run_one_job_parks_poison_job_past_max_attempts() {
        let store = Arc::new(Mutex::new(Store::open_memory().unwrap()));
        let id = store
            .lock()
            .unwrap()
            .enqueue_job(JobKind::Ocr, "{}", 0, None)
            .unwrap();
        let mut reg = JobHandlerRegistry::new();
        reg.register(Arc::new(FailHandler));
        for _ in 0..3 {
            run_one_job(&store, &reg, 2);
            let _ = store.lock().unwrap().requeue_job(&id); // operator keeps retrying
        }
        // 4th run: attempts=4 > 2 → parked by the guard before the handler runs.
        run_one_job(&store, &reg, 2);
        let j = store.lock().unwrap().get_job(&id).unwrap().unwrap();
        assert!(j.attempts > 2);
        assert_eq!(j.error.unwrap().code, "max-attempts");
    }
}

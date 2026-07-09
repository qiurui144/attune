use attune_core::job_handler::{run_one_job, JobControl, JobHandler, JobHandlerRegistry};
use attune_core::office_job_queue::{JobKind, JobState};
use attune_core::store::Store;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct SerialProbeHandler {
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
    order: Arc<Mutex<Vec<usize>>>,
}

impl JobHandler for SerialProbeHandler {
    fn kind(&self) -> JobKind {
        JobKind::Asr
    }

    fn initial_stage_json(&self) -> Option<&'static str> {
        Some("{\"stage\":\"serial-probe\"}")
    }

    fn run(&self, payload_json: &str, ctl: &dyn JobControl) -> Result<String, (String, String)> {
        if ctl.is_cancelled() {
            return Err(("cancelled".into(), "cancelled before start".into()));
        }
        let n = serde_json::from_str::<serde_json::Value>(payload_json)
            .ok()
            .and_then(|v| v["n"].as_u64())
            .unwrap_or(0) as usize;
        let now_active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(now_active, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(10));
        self.order.lock().unwrap().push(n);
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(format!("{{\"n\":{n}}}"))
    }
}

#[test]
fn durable_worker_drain_preserves_single_active_local_handler() {
    let store = Arc::new(Mutex::new(Store::open_memory().unwrap()));
    for n in 0..5 {
        store
            .lock()
            .unwrap()
            .enqueue_job(JobKind::Asr, &format!("{{\"n\":{n}}}"), 0, None)
            .unwrap();
        std::thread::sleep(Duration::from_millis(2));
    }

    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let order = Arc::new(Mutex::new(Vec::new()));
    let mut registry = JobHandlerRegistry::new();
    registry.register(Arc::new(SerialProbeHandler {
        active: active.clone(),
        max_active: max_active.clone(),
        order: order.clone(),
    }));

    let mut drained = Vec::new();
    while let Some(id) = run_one_job(&store, &registry, 5) {
        drained.push(id);
    }

    assert_eq!(drained.len(), 5);
    assert_eq!(
        max_active.load(Ordering::SeqCst),
        1,
        "ASR/office-style local handlers must drain serially on a personal machine"
    );
    assert_eq!(
        *order.lock().unwrap(),
        vec![0, 1, 2, 3, 4],
        "same-priority durable jobs preserve FIFO order"
    );
    let done = store
        .lock()
        .unwrap()
        .list_jobs(None, Some("done"), 10)
        .unwrap();
    assert_eq!(done.len(), 5);
    assert!(done.iter().all(|j| j.state == JobState::Done));
    assert!(done
        .iter()
        .all(|j| j.stage_json.as_deref() == Some("{\"stage\":\"serial-probe\"}")));
}

#[test]
fn late_handler_result_cannot_overwrite_cancelled_job() {
    let store = Store::open_memory().unwrap();
    let id = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
    let claimed = store.claim_next_job().unwrap().unwrap();
    assert_eq!(claimed.id, id);

    assert!(store.cancel_job(&id).unwrap());
    assert!(
        !store.complete_job(&id, "{\"late\":true}").unwrap(),
        "complete guard must drop late success after cancellation"
    );
    assert!(
        !store
            .fail_job(&id, "asr-engine-failed", "late failure")
            .unwrap(),
        "fail guard must drop late failure after cancellation"
    );
    let job = store.get_job(&id).unwrap().unwrap();
    assert_eq!(job.state, JobState::Cancelled);
    assert!(job.result_json.is_none());
    assert!(job.error.is_none());
}

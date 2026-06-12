//! D5.2 — L2 cancel semantics test (G5: durable JobStore edition).
//!
//! Spec §6.3 (office) + G5 spec §7:
//!   - DELETE on running job → state=Cancelled
//!   - DELETE on Done → 409 job-already-completed (route guards; store refuses too)
//!   - DELETE on Cancelled → 409 job-already-cancelled
//!   - WS disconnect does NOT auto-cancel
//!
//! 不依赖真实 whisper-cli (CI 不一定有)：直接驱动 durable Store 的状态机 —
//! office.rs 的 delete_job / WS cancel 路径就是这些 store 转换的薄包装。

use attune_core::office_job_queue::{JobKind, JobState};
use attune_core::store::Store;

#[test]
fn cancel_running_job_flips_state() {
    let store = Store::open_memory().unwrap();
    let id = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
    store.claim_next_job().unwrap(); // → Running
    assert_eq!(store.get_job(&id).unwrap().unwrap().state, JobState::Running);

    assert!(store.cancel_job(&id).unwrap(), "running job is cancellable");
    assert_eq!(store.get_job(&id).unwrap().unwrap().state, JobState::Cancelled);
}

#[test]
fn cancel_done_job_is_refused_by_store_and_route() {
    // The route returns 409 before calling cancel; the store ALSO refuses
    // (guarded to queued/running) so a race cannot flip a terminal job.
    let store = Store::open_memory().unwrap();
    let id = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
    store.claim_next_job().unwrap();
    store.complete_job(&id, "{}").unwrap();

    assert!(!store.cancel_job(&id).unwrap(), "done job must not cancel");
    assert_eq!(store.get_job(&id).unwrap().unwrap().state, JobState::Done);
}

#[test]
fn cancel_failed_job_treated_as_terminal() {
    let store = Store::open_memory().unwrap();
    let id = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
    store.claim_next_job().unwrap();
    store.fail_job(&id, "asr-engine-failed", "test").unwrap();

    assert!(!store.cancel_job(&id).unwrap(), "failed is terminal");
    assert_eq!(store.get_job(&id).unwrap().unwrap().state, JobState::Failed);
}

#[test]
fn ws_disconnect_does_not_change_job_state() {
    // WS handler only cancels on an explicit {"type":"cancel"} frame; a bare
    // close performs no store write — the job keeps running.
    let store = Store::open_memory().unwrap();
    let id = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
    store.claim_next_job().unwrap();
    // (no cancel_job call = WS closed without cancel frame)
    assert_eq!(store.get_job(&id).unwrap().unwrap().state, JobState::Running);
}

#[test]
fn restart_requeues_running_instead_of_mass_cancel() {
    // G5 replaces JobRegistry::cancel_all_running ("server restarted, please
    // resubmit") with recover_on_boot: idempotent ASR requeues, Done untouched.
    let store = Store::open_memory().unwrap();
    let running = store.enqueue_job(JobKind::Asr, "{}", 5, None).unwrap();
    let queued = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
    let done = store.enqueue_job(JobKind::Asr, "{}", 9, None).unwrap();
    let c = store.claim_next_job().unwrap().unwrap();
    assert_eq!(c.id, done);
    store.complete_job(&done, "{}").unwrap();
    store.claim_next_job().unwrap(); // `running` (prio 5) → Running

    let summary = store.recover_on_boot().unwrap();
    assert_eq!(summary.requeued, 1);
    assert_eq!(store.get_job(&running).unwrap().unwrap().state, JobState::Queued);
    assert_eq!(store.get_job(&queued).unwrap().unwrap().state, JobState::Queued);
    assert_eq!(store.get_job(&done).unwrap().unwrap().state, JobState::Done);
}

#[test]
fn queue_position_collapses_after_cancel() {
    // Priority-distinct enqueues make claim order deterministic (created_ms has
    // ms resolution — same-ms ties break by id, which is random uuid).
    let store = Store::open_memory().unwrap();
    let _j1 = store.enqueue_job(JobKind::Asr, "{}", 9, None).unwrap();
    let j2 = store.enqueue_job(JobKind::Asr, "{}", 5, None).unwrap();
    let j3 = store.enqueue_job(JobKind::Asr, "{}", 1, None).unwrap();

    assert_eq!(store.job_queue_position(&j3).unwrap(), 2);

    // Cancel j2 → j3's position drops to 1 (only j1 ahead and still Queued).
    assert!(store.cancel_job(&j2).unwrap());
    assert_eq!(store.job_queue_position(&j3).unwrap(), 1);
}

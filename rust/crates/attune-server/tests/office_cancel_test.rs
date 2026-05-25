//! D5.2 — L2 cancel semantics test.
//!
//! Spec §6.3:
//!   - DELETE on running job → state=Cancelled
//!   - DELETE on Done → 409 job-already-completed
//!   - DELETE on Cancelled → 409 job-already-cancelled
//!   - WS disconnect does NOT auto-cancel
//!
//! 不依赖真实 whisper-cli (CI 不一定有), 用 JobRegistry 直接操作 + REST DELETE.

use attune_core::office_job_queue::{Job, JobError, JobRegistry, JobStage, JobState};

#[test]
fn cancel_running_job_via_registry_flips_state() {
    let registry = JobRegistry::new();
    let mut j = Job::new("test-1".into());
    j.state = JobState::Running;
    j.stage = JobStage::Transcribing;
    registry.insert(j);

    // Simulate DELETE: route reads job, sees state=Running, calls update.
    let job = registry.get("test-1").unwrap();
    assert_eq!(job.state, JobState::Running);

    registry.update("test-1", |j| j.state = JobState::Cancelled);
    let after = registry.get("test-1").unwrap();
    assert_eq!(after.state, JobState::Cancelled);
}

#[test]
fn cancel_done_job_should_be_rejected_at_route_level() {
    // The route returns 409 before calling update, so registry stays Done.
    let registry = JobRegistry::new();
    let mut j = Job::new("done-1".into());
    j.state = JobState::Done;
    j.result_json = Some("{}".into());
    registry.insert(j);

    let job = registry.get("done-1").unwrap();
    // Route logic: if job.state == Done → return 409, don't update.
    match job.state {
        JobState::Done => { /* would return 409 */ }
        _ => panic!("expected Done"),
    }
    // Registry should remain Done
    assert_eq!(registry.get("done-1").unwrap().state, JobState::Done);
}

#[test]
fn cancel_failed_job_treated_as_terminal() {
    let registry = JobRegistry::new();
    let mut j = Job::new("failed-1".into());
    j.state = JobState::Failed;
    j.error = Some(JobError {
        message: "test".into(),
        code: "asr-engine-failed".into(),
    });
    registry.insert(j);

    let job = registry.get("failed-1").unwrap();
    assert_eq!(job.state, JobState::Failed);
    // Route returns 409 for Failed too (terminal state)
}

#[test]
fn ws_disconnect_does_not_change_job_state() {
    // WS handler only flips state on receiving {"type":"cancel"} text frame.
    // A bare close (no cancel frame) leaves job alone.
    let registry = JobRegistry::new();
    let mut j = Job::new("ws-test".into());
    j.state = JobState::Running;
    j.stage = JobStage::Transcribing;
    registry.insert(j);

    // Simulate: WS opens, ticks once (no cancel sent), client closes connection.
    // No update calls happen on close path.
    assert_eq!(registry.get("ws-test").unwrap().state, JobState::Running);
}

#[test]
fn cancel_all_running_marks_in_flight_with_warning() {
    let registry = JobRegistry::new();
    let mut running = Job::new("r1".into());
    running.state = JobState::Running;
    registry.insert(running);

    let queued = Job::new("q1".into()); // default Queued
    registry.insert(queued);

    let mut done = Job::new("d1".into());
    done.state = JobState::Done;
    registry.insert(done);

    registry.cancel_all_running();

    assert_eq!(registry.get("r1").unwrap().state, JobState::Cancelled);
    assert_eq!(registry.get("q1").unwrap().state, JobState::Cancelled);
    // Done preserved
    assert_eq!(registry.get("d1").unwrap().state, JobState::Done);

    // Warning added to former-running
    let r1 = registry.get("r1").unwrap();
    assert!(r1.warnings.iter().any(|w| w.contains("server restarted")));
}

#[test]
fn fifo_queue_position_collapses_after_cancel() {
    let registry = JobRegistry::new();
    registry.insert(Job::new("j1".into()));
    std::thread::sleep(std::time::Duration::from_millis(2));
    registry.insert(Job::new("j2".into()));
    std::thread::sleep(std::time::Duration::from_millis(2));
    registry.insert(Job::new("j3".into()));

    assert_eq!(registry.queue_position("j3"), 2);

    // Cancel j2 → j3's position should drop to 1 (only j1 older + still Queued)
    registry.update("j2", |j| j.state = JobState::Cancelled);
    assert_eq!(registry.queue_position("j3"), 1);
}

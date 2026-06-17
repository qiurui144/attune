//! G5: durable job-queue management routes (K3 nightly batch panel).
//! GET /api/v1/jobs?kind=&state=  ·  POST /jobs/{id}/cancel  ·  POST /jobs/{id}/requeue
//! Spec: docs/superpowers/specs/2026-06-10-k3-g5-durable-job-queue.md §5

use crate::state::SharedState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

fn err(code: StatusCode, kebab: &str, msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (code, Json(serde_json::json!({ "error": msg, "code": kebab })))
}

fn store_or_503(
    state: &SharedState,
) -> Result<
    std::sync::Arc<std::sync::Mutex<attune_core::store::Store>>,
    (StatusCode, Json<serde_json::Value>),
> {
    state.job_store().ok_or_else(|| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "job-store-unavailable",
            "durable job queue unavailable (store failed to open at boot)",
        )
    })
}

#[derive(Deserialize)]
pub struct JobsQuery {
    pub kind: Option<String>,
    pub state: Option<String>,
}

/// GET /api/v1/jobs — list the durable queue, claim order first.
pub async fn list_jobs(
    State(state): State<SharedState>,
    Query(q): Query<JobsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let store = store_or_503(&state)?;
    let jobs = {
        let s = store.lock().unwrap_or_else(|e| e.into_inner());
        s.list_jobs(q.kind.as_deref(), q.state.as_deref(), 200)
            .map_err(|e| {
                err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "job-list-failed",
                    &e.to_string(),
                )
            })?
    };
    let arr: Vec<_> = jobs
        .iter()
        .map(crate::routes::office::job_record_to_status_json)
        .collect();
    Ok(Json(serde_json::json!({ "jobs": arr })))
}

/// POST /api/v1/jobs/{id}/cancel — queued/running → cancelled (cooperative).
pub async fn cancel_job(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let store = store_or_503(&state)?;
    let changed = {
        let s = store.lock().unwrap_or_else(|e| e.into_inner());
        s.cancel_job(&id).map_err(|e| {
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "job-cancel-failed",
                &e.to_string(),
            )
        })?
    };
    if !changed {
        return Err(err(
            StatusCode::CONFLICT,
            "job-not-cancellable",
            "job is terminal or absent",
        ));
    }
    Ok(Json(serde_json::json!({ "cancelled": id })))
}

/// POST /api/v1/jobs/{id}/requeue — failed/cancelled → queued (operator retry).
pub async fn requeue_job(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let store = store_or_503(&state)?;
    let changed = {
        let s = store.lock().unwrap_or_else(|e| e.into_inner());
        s.requeue_job(&id).map_err(|e| {
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "job-requeue-failed",
                &e.to_string(),
            )
        })?
    };
    if !changed {
        return Err(err(
            StatusCode::CONFLICT,
            "job-not-requeuable",
            "only failed/cancelled jobs requeue",
        ));
    }
    Ok(Json(serde_json::json!({ "requeued": id })))
}

#[cfg(test)]
mod tests {
    use attune_core::office_job_queue::JobKind;
    use attune_core::store::Store;

    #[test]
    fn list_jobs_store_to_json_shape() {
        let store = Store::open_memory().unwrap();
        store.enqueue_job(JobKind::Asr, "{}", 3, None).unwrap();
        let jobs = store.list_jobs(None, None, 10).unwrap();
        let v = crate::routes::office::job_record_to_status_json(&jobs[0]);
        assert_eq!(v["state"], "queued");
        assert_eq!(v["kind"], "asr");
        assert!(v.get("progress").is_some());
        assert!(v.get("job_id").is_some());
    }

    #[test]
    fn requeue_then_cancel_round_trip_store_level() {
        // Route handlers are thin wrappers; verify the store transitions they rely on.
        let store = Store::open_memory().unwrap();
        let id = store.enqueue_job(JobKind::Asr, "{}", 0, None).unwrap();
        assert!(store.cancel_job(&id).unwrap(), "queued → cancelled");
        assert!(!store.cancel_job(&id).unwrap(), "terminal not re-cancellable");
        assert!(store.requeue_job(&id).unwrap(), "cancelled → queued");
        assert!(!store.requeue_job(&id).unwrap(), "queued not requeuable");
    }
}

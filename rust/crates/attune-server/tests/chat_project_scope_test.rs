//! C4 — chat.rs project_id intake + validation + scope wiring (chat-centric IA).
//!
//! Calls the `chat` handler directly against an unlocked in-memory state (no HTTP,
//! no real LLM). Project validation runs BEFORE the LLM-availability check, so:
//!   - a non-existent project_id → 400 `project-not-found` (Detailed error)
//!   - a valid project_id passes validation and only then hits the LLM-unavailable
//!     path (proving validation did not reject a real project)
//!   - absent project_id → loose path, also reaches LLM-unavailable (no regression
//!     in the validation layer)

use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;

use attune_server::error::AppError;
use attune_server::routes::chat::{chat, ChatRequest};
use attune_server::test_support::unlocked_state;

fn req(message: &str, project_id: Option<&str>) -> ChatRequest {
    // Build via JSON so optional #[serde(default)] fields default correctly and we
    // exercise the same deserialization old clients use (no project_id key at all).
    let mut body = serde_json::json!({ "message": message });
    if let Some(pid) = project_id {
        body["project_id"] = serde_json::Value::String(pid.to_string());
    }
    serde_json::from_value(body).expect("ChatRequest deserializes")
}

#[tokio::test]
async fn nonexistent_project_id_returns_400_project_not_found() {
    let state = unlocked_state();
    let body = req("hello", Some("does-not-exist"));
    let res = chat(State(state), HeaderMap::new(), Json(body)).await;
    match res {
        Err(AppError::Detailed { status, body }) => {
            assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
            assert_eq!(body["code"], "project-not-found", "stable kebab code");
        }
        other => panic!("expected 400 project-not-found, got {other:?}"),
    }
}

#[tokio::test]
async fn valid_project_id_passes_validation_then_hits_llm_unavailable() {
    let state = unlocked_state();
    // Seed a real project so validation accepts it.
    let pid = {
        let vault = state.vault.lock().unwrap();
        let p = vault.store().create_project("Case A", "generic").unwrap();
        p.id
    };
    let body = req("hello", Some(&pid));
    let res = chat(State(state), HeaderMap::new(), Json(body)).await;
    // No LLM is wired → the handler must get PAST validation and fail at the
    // LLM-availability gate (503), NOT at project validation (400).
    match res {
        Err(AppError::Detailed { status, .. }) => {
            assert_eq!(
                status,
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "valid project must pass validation and reach the LLM gate"
            );
        }
        other => panic!("expected 503 (LLM unavailable) after valid project, got {other:?}"),
    }
}

#[tokio::test]
async fn absent_project_id_is_loose_no_validation_rejection() {
    let state = unlocked_state();
    let body = req("hello", None);
    let res = chat(State(state), HeaderMap::new(), Json(body)).await;
    // Loose path: no project validation; must reach the LLM gate (503), proving the
    // legacy no-project_id request is not rejected by the new validation layer.
    match res {
        Err(AppError::Detailed { status, .. }) => {
            assert_eq!(status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
        }
        other => panic!("expected 503 (LLM unavailable) for loose chat, got {other:?}"),
    }
}

//! C5 + C6 — session project filter + GET /projects/:id/conversations (chat-centric IA).
//!
//! Handler-level tests against an unlocked in-memory state (no HTTP, no LLM). Seed
//! conversations directly via the store, then exercise:
//!   - C5: GET /chat/sessions?project_id=__loose__ / =<pid> / absent
//!   - C6: GET /projects/:id/conversations (project branch list; 404 if missing)

use axum::extract::{Path, Query, State};

use attune_server::routes::chat_sessions::{list_sessions, PaginationQuery};
use attune_server::routes::projects::{list_project_conversations, ConversationsQuery};
use attune_server::test_support::unlocked_state;

/// Seed: 2 loose + 1 under project p1 + 1 under project p2. Returns (p1, p2).
fn seed(state: &std::sync::Arc<attune_server::state::AppState>) -> (String, String) {
    let vault = state.vault.lock().unwrap();
    let dek = vault.dek_db().unwrap();
    let store = vault.store();
    let p1 = store.create_project("P1", "generic").unwrap();
    let p2 = store.create_project("P2", "generic").unwrap();
    store.create_conversation(&dek, "loose-a", None).unwrap();
    store.create_conversation(&dek, "loose-b", None).unwrap();
    store.create_conversation(&dek, "p1-conv", Some(&p1.id)).unwrap();
    store.create_conversation(&dek, "p2-conv", Some(&p2.id)).unwrap();
    (p1.id, p2.id)
}

fn pagination(project_id: Option<&str>) -> PaginationQuery {
    let mut body = serde_json::json!({});
    if let Some(pid) = project_id {
        body["project_id"] = serde_json::Value::String(pid.to_string());
    }
    serde_json::from_value(body).unwrap()
}

#[tokio::test]
async fn sessions_absent_filter_returns_all() {
    let state = unlocked_state();
    let _ = seed(&state);
    let res = list_sessions(State(state), Query(pagination(None))).await.unwrap();
    assert_eq!(res.0["total"], 4, "no filter → all conversations");
}

#[tokio::test]
async fn sessions_loose_sentinel_returns_loose_only() {
    let state = unlocked_state();
    let _ = seed(&state);
    let res = list_sessions(State(state), Query(pagination(Some("__loose__"))))
        .await
        .unwrap();
    assert_eq!(res.0["total"], 2, "__loose__ → only project-less conversations");
    for s in res.0["sessions"].as_array().unwrap() {
        assert!(s["project_id"].is_null(), "loose conversations have null project_id");
    }
}

#[tokio::test]
async fn sessions_project_id_returns_that_project_only() {
    let state = unlocked_state();
    let (p1, _p2) = seed(&state);
    let res = list_sessions(State(state), Query(pagination(Some(&p1))))
        .await
        .unwrap();
    assert_eq!(res.0["total"], 1, "project filter → only that project's conversations");
    assert_eq!(res.0["sessions"][0]["project_id"], p1);
    // Response carries project_id for each session (C5 contract).
    assert_eq!(res.0["sessions"][0]["title"], "p1-conv");
}

fn conv_query() -> ConversationsQuery {
    serde_json::from_value(serde_json::json!({})).unwrap()
}

#[tokio::test]
async fn project_conversations_lists_branch_only() {
    let state = unlocked_state();
    let (p1, _p2) = seed(&state);
    let res = list_project_conversations(State(state), Path(p1.clone()), Query(conv_query()))
        .await
        .unwrap();
    assert_eq!(res.0.total, 1, "project branch list excludes loose + other projects");
    assert_eq!(res.0.conversations[0].project_id.as_deref(), Some(p1.as_str()));
    assert_eq!(res.0.conversations[0].title, "p1-conv");
}

#[tokio::test]
async fn project_conversations_404_for_missing_project() {
    let state = unlocked_state();
    let _ = seed(&state);
    let res = list_project_conversations(
        State(state),
        Path("no-such-project".to_string()),
        Query(conv_query()),
    )
    .await;
    match res {
        Err(attune_server::error::AppError::NotFound(_)) => {}
        other => panic!("expected 404 NotFound for missing project, got {other:?}"),
    }
}

#[tokio::test]
async fn project_with_no_conversations_returns_empty_not_404() {
    let state = unlocked_state();
    // A real project with zero conversations → 200 + empty (not 404).
    let pid = {
        let vault = state.vault.lock().unwrap();
        vault.store().create_project("Empty", "generic").unwrap().id
    };
    let res = list_project_conversations(State(state), Path(pid), Query(conv_query()))
        .await
        .unwrap();
    assert_eq!(res.0.total, 0);
    assert!(res.0.conversations.is_empty());
}

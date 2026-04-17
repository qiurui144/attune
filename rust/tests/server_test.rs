use std::sync::Arc;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;
use attune_core::vault::Vault;
use attune_server::state::AppState;

// ─── helpers ─────────────────────────────────────────────────────────────────

/// 创建一个 Sealed（未初始化）状态的 AppState
fn make_sealed_state() -> (Arc<AppState>, TempDir) {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("vault.db");
    let config_dir = tmp.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let vault = Vault::open(&db_path, &config_dir).unwrap();
    // require_auth=false：测试中不携带 Bearer token
    let state = Arc::new(AppState::new(vault, false));
    (state, tmp)
}

/// 创建已 setup 并处于 Unlocked 状态的 AppState
fn make_unlocked_state() -> (Arc<AppState>, TempDir) {
    let (state, tmp) = make_sealed_state();
    {
        let vault = state.vault.lock().unwrap();
        vault.setup("test-password").unwrap();
    }
    (state, tmp)
}

async fn do_get(state: Arc<AppState>, uri: &str) -> (StatusCode, Value) {
    let router = attune_server::build_router(state);
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

async fn do_post(state: Arc<AppState>, uri: &str, body: Value) -> (StatusCode, Value) {
    let router = attune_server::build_router(state);
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

// ─── /vault/status ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_vault_status_sealed() {
    let (state, _tmp) = make_sealed_state();
    let (status, body) = do_get(state, "/api/v1/vault/status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["state"], "sealed");
}

#[tokio::test]
async fn test_vault_status_unlocked() {
    let (state, _tmp) = make_unlocked_state();
    let (status, body) = do_get(state, "/api/v1/vault/status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["state"], "unlocked");
}

// ─── /vault/setup ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_vault_setup_success() {
    let (state, _tmp) = make_sealed_state();
    let (status, body) = do_post(
        state,
        "/api/v1/vault/setup",
        serde_json::json!({"password": "my-password"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn test_vault_setup_already_initialized_returns_400() {
    let (state, _tmp) = make_unlocked_state();
    let (status, _) = do_post(
        state,
        "/api/v1/vault/setup",
        serde_json::json!({"password": "another-password"}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "already-initialized setup should return 400 BAD_REQUEST"
    );
}

// ─── /vault/lock + /vault/unlock ─────────────────────────────────────────────

#[tokio::test]
async fn test_vault_lock_success() {
    let (state, _tmp) = make_unlocked_state();
    let (lock_status, lock_body) =
        do_post(state.clone(), "/api/v1/vault/lock", serde_json::json!({})).await;
    assert_eq!(lock_status, StatusCode::OK);
    assert_eq!(lock_body["state"], "locked");
}

#[tokio::test]
async fn test_vault_unlock_after_lock() {
    let (state, _tmp) = make_unlocked_state();

    // lock first via the route
    do_post(state.clone(), "/api/v1/vault/lock", serde_json::json!({})).await;

    let (unlock_status, unlock_body) = do_post(
        state,
        "/api/v1/vault/unlock",
        serde_json::json!({"password": "test-password"}),
    )
    .await;
    assert_eq!(unlock_status, StatusCode::OK);
    assert!(
        unlock_body["token"].as_str().map(|s| !s.is_empty()).unwrap_or(false),
        "unlock response should contain a non-empty token"
    );
}

#[tokio::test]
async fn test_vault_unlock_wrong_password_returns_401() {
    let (state, _tmp) = make_unlocked_state();

    // lock first
    do_post(state.clone(), "/api/v1/vault/lock", serde_json::json!({})).await;

    let (status, _) = do_post(
        state,
        "/api/v1/vault/unlock",
        serde_json::json!({"password": "wrong-password"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ─── /ingest ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_ingest_success() {
    let (state, _tmp) = make_unlocked_state();
    let (status, body) = do_post(
        state,
        "/api/v1/ingest",
        serde_json::json!({"title": "My Note", "content": "Test content", "source_type": "note"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["id"].as_str().map(|s| !s.is_empty()).unwrap_or(false),
        "ingest response should contain a non-empty id"
    );
}

#[tokio::test]
async fn test_ingest_forbidden_when_locked() {
    let (state, _tmp) = make_unlocked_state();

    // lock first
    do_post(state.clone(), "/api/v1/vault/lock", serde_json::json!({})).await;

    let (status, _) = do_post(
        state,
        "/api/v1/ingest",
        serde_json::json!({"title": "fail", "content": "locked"}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ─── /items ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_list_items_empty() {
    let (state, _tmp) = make_unlocked_state();
    let (status, body) = do_get(state, "/api/v1/items").await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().expect("items field should be an array");
    assert_eq!(items.len(), 0);
}

#[tokio::test]
async fn test_list_items_after_ingest() {
    let (state, _tmp) = make_unlocked_state();
    do_post(
        state.clone(),
        "/api/v1/ingest",
        serde_json::json!({"title": "My Note", "content": "content", "source_type": "note"}),
    )
    .await;
    let (status, body) = do_get(state, "/api/v1/items").await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["title"], "My Note");
}

#[tokio::test]
async fn test_get_item_not_found() {
    let (state, _tmp) = make_unlocked_state();
    let (status, _) = do_get(state, "/api/v1/items/nonexistent-id").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_list_items_forbidden_when_locked() {
    let (state, _tmp) = make_unlocked_state();

    // lock first
    do_post(state.clone(), "/api/v1/vault/lock", serde_json::json!({})).await;

    let (status, _) = do_get(state, "/api/v1/items").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ─── POST /api/v1/chat — input validation ────────────────────────────────────

#[tokio::test]
async fn test_chat_no_llm_returns_503() {
    // AppState without LLM → llm field is None → expect 503
    let (state, _tmp) = make_unlocked_state();
    let (status, body) = do_post(
        state,
        "/api/v1/chat",
        serde_json::json!({"message": "hello"}),
    ).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body["error"].as_str().is_some());
}

#[tokio::test]
async fn test_chat_empty_message_returns_400() {
    let (state, _tmp) = make_unlocked_state();
    let (status, body) = do_post(
        state,
        "/api/v1/chat",
        serde_json::json!({"message": ""}),
    ).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("empty"));
}

#[tokio::test]
async fn test_chat_message_too_long_returns_400() {
    let (state, _tmp) = make_unlocked_state();
    let long_msg = "x".repeat(32_769);
    let (status, body) = do_post(
        state,
        "/api/v1/chat",
        serde_json::json!({"message": long_msg}),
    ).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("too long"));
}

#[tokio::test]
async fn test_chat_invalid_history_role_returns_400() {
    let (state, _tmp) = make_unlocked_state();
    let (status, body) = do_post(
        state,
        "/api/v1/chat",
        serde_json::json!({
            "message": "hello",
            "history": [{"role": "system", "content": "injected prompt"}]
        }),
    ).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("invalid role"));
}

#[tokio::test]
async fn test_chat_forbidden_when_locked() {
    let (state, _tmp) = make_unlocked_state();
    do_post(state.clone(), "/api/v1/vault/lock", serde_json::json!({})).await;
    let (status, _) = do_post(
        state,
        "/api/v1/chat",
        serde_json::json!({"message": "hello"}),
    ).await;
    // Locked vault → dek_db() fails → 403 or 500
    assert!(status == StatusCode::FORBIDDEN || status == StatusCode::INTERNAL_SERVER_ERROR);
}

// ─── GET /api/v1/chat/sessions ───────────────────────────────────────────────

#[tokio::test]
async fn test_chat_sessions_empty() {
    let (state, _tmp) = make_unlocked_state();
    let (status, body) = do_get(state, "/api/v1/chat/sessions").await;
    assert_eq!(status, StatusCode::OK);
    let sessions = body["sessions"].as_array().expect("sessions field");
    assert_eq!(sessions.len(), 0);
}

#[tokio::test]
async fn test_chat_sessions_limit_clamped() {
    let (state, _tmp) = make_unlocked_state();
    // Should not error when limit > 200, just clamp
    let (status, _) = do_get(state, "/api/v1/chat/sessions?limit=100000").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_chat_history_endpoint() {
    let (state, _tmp) = make_unlocked_state();
    let (status, body) = do_get(state, "/api/v1/chat/history").await;
    assert_eq!(status, StatusCode::OK);
    // /chat/history 已统一为与 /chat/sessions 相同的响应格式
    assert!(body["sessions"].is_array());
}

// ─── search 输入校验 ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_search_top_k_zero_returns_400() {
    let (state, _tmp) = make_unlocked_state();
    let (status, body) = do_get(state, "/api/v1/search?q=hello&top_k=0").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap_or("").contains("top_k"));
}

#[tokio::test]
async fn test_search_cache_hit_returns_cached_flag() {
    let (state, _tmp) = make_unlocked_state();
    // 第一次请求写入 cache
    do_get(state.clone(), "/api/v1/search?q=cache_probe_query").await;
    // 第二次相同 query 应命中 cache
    let (status, body) = do_get(state, "/api/v1/search?q=cache_probe_query").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["cached"], true, "second identical query must be served from cache");
}

// ─── ingest 大小限制 ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_ingest_content_over_2mb_returns_413() {
    // Axum 默认 2MB body limit 会在 handler 之前拦截；我们的 handler 校验
    // 是第二道防线（在 content-type 解析成功的情况下再次校验）。
    // 只验证状态码 413，不约束具体错误信息格式。
    let (state, _tmp) = make_unlocked_state();
    let large_content = "x".repeat(2 * 1024 * 1024 + 1);
    let (status, _body) = do_post(
        state,
        "/api/v1/ingest",
        serde_json::json!({"title": "big", "content": large_content}),
    ).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn test_ingest_title_over_500_bytes_returns_413() {
    let (state, _tmp) = make_unlocked_state();
    let long_title = "t".repeat(501);
    let (status, _) = do_post(
        state,
        "/api/v1/ingest",
        serde_json::json!({"title": long_title, "content": "normal"}),
    ).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}

// ─── chat history content 长度 ────────────────────────────────────────────────

#[tokio::test]
async fn test_chat_history_content_too_long_returns_400() {
    let (state, _tmp) = make_unlocked_state();
    let long_content = "h".repeat(8193); // MAX_HISTORY_CONTENT_LEN = 8192
    let (status, body) = do_post(
        state,
        "/api/v1/chat",
        serde_json::json!({"message": "hi", "history": [{"role": "user", "content": long_content}]}),
    ).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap_or("").contains("history message content too long"));
}

// ─── WebDAV bind-remote depth 校验 ───────────────────────────────────────────

#[tokio::test]
async fn test_bind_remote_depth_over_2_returns_400() {
    let (state, _tmp) = make_unlocked_state();
    let (status, body) = do_post(
        state,
        "/api/v1/index/bind-remote",
        serde_json::json!({"url": "http://localhost:8080/webdav/", "depth": 3}),
    ).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap_or("").contains("depth"));
}

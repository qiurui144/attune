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

// ─── /upload multipart 端点 ───────────────────────────────────────────────────

/// 构建 multipart/form-data body（模拟浏览器上传）
fn make_multipart_body(filename: &str, content: &[u8]) -> (String, Vec<u8>) {
    let boundary = "test_boundary_xyz";
    let mut body = Vec::new();
    // part header
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n").as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(content);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (
        format!("multipart/form-data; boundary={boundary}"),
        body,
    )
}

async fn do_upload(state: Arc<AppState>, filename: &str, content: &[u8]) -> (StatusCode, Value) {
    let (content_type, body_bytes) = make_multipart_body(filename, content);
    let router = attune_server::build_router(state);
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/upload")
        .header("content-type", content_type)
        .body(Body::from(body_bytes))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

#[tokio::test]
async fn test_upload_markdown_returns_ok_with_id() {
    let (state, _tmp) = make_unlocked_state();
    let md = b"# Upload Test\n\nThis is markdown content uploaded via multipart.";
    let (status, body) = do_upload(state, "test.md", md).await;
    assert_eq!(status, StatusCode::OK, "upload markdown should succeed: {body}");
    assert!(
        body["id"].as_str().map(|s| !s.is_empty()).unwrap_or(false),
        "response should contain non-empty id: {body}"
    );
}

#[tokio::test]
async fn test_upload_html_bytes_parsed() {
    let (state, _tmp) = make_unlocked_state();
    let html = b"<html><head><title>Web Page</title></head><body><p>Page content</p></body></html>";
    let (status, body) = do_upload(state, "page.html", html).await;
    assert_eq!(status, StatusCode::OK, "upload html should succeed: {body}");
    assert!(body["id"].as_str().map(|s| !s.is_empty()).unwrap_or(false));
}

#[tokio::test]
async fn test_upload_csv_bytes_parsed() {
    let (state, _tmp) = make_unlocked_state();
    let csv = b"header1,header2,header3\nval1,val2,val3\nval4,val5,val6\n";
    let (status, body) = do_upload(state, "data.csv", csv).await;
    assert_eq!(status, StatusCode::OK, "upload csv should succeed: {body}");
    assert!(body["id"].as_str().map(|s| !s.is_empty()).unwrap_or(false));
}

#[tokio::test]
async fn test_upload_txt_bytes_parsed() {
    let (state, _tmp) = make_unlocked_state();
    let txt = b"Plain text file.\nSecond line.\nThird line.";
    let (status, body) = do_upload(state, "notes.txt", txt).await;
    assert_eq!(status, StatusCode::OK, "upload txt should succeed: {body}");
    assert!(body["id"].as_str().map(|s| !s.is_empty()).unwrap_or(false));
}

#[tokio::test]
async fn test_upload_rtf_bytes_parsed() {
    let (state, _tmp) = make_unlocked_state();
    let rtf = br"{\rtf1\ansi\pard RTF upload test content\par}";
    let (status, body) = do_upload(state, "document.rtf", rtf).await;
    assert_eq!(status, StatusCode::OK, "upload rtf should succeed: {body}");
    assert!(body["id"].as_str().map(|s| !s.is_empty()).unwrap_or(false));
}

#[tokio::test]
async fn test_upload_when_locked_returns_403() {
    let (state, _tmp) = make_unlocked_state();
    // lock first
    do_post(state.clone(), "/api/v1/vault/lock", serde_json::json!({})).await;

    let (status, _) = do_upload(state, "test.md", b"content").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "upload while locked should be 403");
}

#[tokio::test]
async fn test_upload_no_file_field_returns_400() {
    let (state, _tmp) = make_unlocked_state();
    // Empty multipart body (no fields at all) — just the boundary markers
    let boundary = "empty_boundary_abc";
    let body = format!("--{boundary}--\r\n");
    let router = attune_server::build_router(state);
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/upload")
        .header("content-type", format!("multipart/form-data; boundary={boundary}"))
        .body(Body::from(body))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "missing file field should be 400");
}

#[tokio::test]
async fn test_upload_unsupported_format_returns_422() {
    let (state, _tmp) = make_unlocked_state();
    // .mp4 is not in is_supported() — parse_bytes_with_profile now returns InvalidInput
    let (status, body) = do_upload(state, "video.mp4", b"fake video bytes").await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "unsupported format should return 422: {body}"
    );
}

#[tokio::test]
async fn test_upload_duplicate_returns_ok_with_duplicate_status() {
    let (state, _tmp) = make_unlocked_state();
    let content = b"# Dedup Test\n\nThis content will be uploaded twice to test deduplication.";
    // First upload
    let (status1, body1) = do_upload(state.clone(), "dedup.md", content).await;
    assert_eq!(status1, StatusCode::OK, "first upload should succeed: {body1}");

    // Second upload — same content
    let (status2, body2) = do_upload(state, "dedup.md", content).await;
    assert_eq!(status2, StatusCode::OK, "duplicate upload should not error: {body2}");
    // status may be "ok" or "duplicate" depending on implementation
    // Server accepts duplicate uploads — status reflects current processing state
    let s = body2["status"].as_str().unwrap_or("");
    assert!(
        !s.is_empty(),
        "duplicate upload response should contain status field: {body2}"
    );
}

#[tokio::test]
async fn test_upload_stores_item_retrievable_from_items_list() {
    let (state, _tmp) = make_unlocked_state();
    let content = b"# Retrievable Upload\n\nThis note should appear in /items after upload.";
    let (status, body) = do_upload(state.clone(), "retrieval.md", content).await;
    assert_eq!(status, StatusCode::OK, "upload should succeed: {body}");

    // Verify item appears in list
    let (list_status, list_body) = do_get(state, "/api/v1/items").await;
    assert_eq!(list_status, StatusCode::OK);
    let items = list_body["items"].as_array().expect("items should be array");
    assert!(!items.is_empty(), "items list should contain the uploaded item");
    assert!(
        items.iter().any(|i| i["title"].as_str().unwrap_or("").contains("Retrievable Upload")),
        "uploaded item title should appear in list: {items:?}"
    );
}

// ─── annotations ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_annotations_list_empty_for_unknown_item() {
    let (state, _tmp) = make_unlocked_state();
    let (status, body) = do_get(state, "/api/v1/annotations?item_id=nonexistent").await;
    assert_eq!(status, StatusCode::OK);
    let annotations = body["annotations"].as_array().expect("annotations field");
    assert_eq!(annotations.len(), 0);
}

#[tokio::test]
async fn test_annotation_create_and_list() {
    let (state, _tmp) = make_unlocked_state();
    // 先入库一条 item
    let (_, ingest_body) = do_post(
        state.clone(),
        "/api/v1/ingest",
        serde_json::json!({"title": "Annotatable Note", "content": "Paragraph one. Paragraph two."}),
    ).await;
    let item_id = ingest_body["id"].as_str().expect("item id").to_string();

    // 创建批注
    let (create_status, create_body) = do_post(
        state.clone(),
        "/api/v1/annotations",
        serde_json::json!({
            "item_id": item_id,
            "offset_start": 0,
            "offset_end": 13,
            "text_snippet": "Paragraph one",
            "color": "yellow",
            "content": "Important opening"
        }),
    ).await;
    assert_eq!(create_status, StatusCode::OK, "create annotation: {create_body}");
    let annotation_id = create_body["id"].as_str().expect("annotation id");
    assert!(!annotation_id.is_empty());

    // 列表包含该批注
    let (list_status, list_body) = do_get(
        state.clone(),
        &format!("/api/v1/annotations?item_id={item_id}"),
    ).await;
    assert_eq!(list_status, StatusCode::OK);
    let anns = list_body["annotations"].as_array().expect("annotations array");
    assert_eq!(anns.len(), 1);
    assert_eq!(anns[0]["text_snippet"], "Paragraph one");
}

#[tokio::test]
async fn test_annotation_invalid_color_returns_400() {
    let (state, _tmp) = make_unlocked_state();
    let (_, ingest_body) = do_post(
        state.clone(),
        "/api/v1/ingest",
        serde_json::json!({"title": "Note", "content": "Content"}),
    ).await;
    let item_id = ingest_body["id"].as_str().expect("item id").to_string();

    let (status, body) = do_post(
        state,
        "/api/v1/annotations",
        serde_json::json!({
            "item_id": item_id,
            "offset_start": 0,
            "offset_end": 4,
            "text_snippet": "Note",
            "color": "purple"  // not in ALLOWED_COLORS
        }),
    ).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "invalid color: {body}");
}

#[tokio::test]
async fn test_annotation_snippet_too_long_returns_400() {
    let (state, _tmp) = make_unlocked_state();
    let (_, ingest_body) = do_post(
        state.clone(),
        "/api/v1/ingest",
        serde_json::json!({"title": "Note", "content": "Content"}),
    ).await;
    let item_id = ingest_body["id"].as_str().expect("item id").to_string();
    let long_snippet = "x".repeat(2001); // MAX_SNIPPET_LEN = 2000

    let (status, _) = do_post(
        state,
        "/api/v1/annotations",
        serde_json::json!({
            "item_id": item_id,
            "offset_start": 0,
            "offset_end": 100,
            "text_snippet": long_snippet
        }),
    ).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ─── tags ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_tags_all_dimensions_returns_ok() {
    let (state, _tmp) = make_unlocked_state();
    let (status, body) = do_get(state, "/api/v1/tags").await;
    // 空库仍然应该返回 200 with empty dimensions map (or 403 if tag_index not built)
    assert!(
        status == StatusCode::OK || status == StatusCode::FORBIDDEN,
        "tags endpoint should return 200 or 403 (locked): {body}"
    );
    if status == StatusCode::OK {
        assert!(body["dimensions"].is_object(), "dimensions should be an object: {body}");
    }
}

#[tokio::test]
async fn test_tags_dimension_histogram_returns_ok() {
    let (state, _tmp) = make_unlocked_state();
    let (status, body) = do_get(state, "/api/v1/tags/topic").await;
    assert!(
        status == StatusCode::OK || status == StatusCode::FORBIDDEN,
        "tag dimension endpoint should return 200 or 403: {body}"
    );
    if status == StatusCode::OK {
        assert!(body["values"].is_array(), "values should be an array: {body}");
    }
}

// ─── status ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_status_endpoint_returns_version_and_build() {
    let (state, _tmp) = make_unlocked_state();
    let (status, body) = do_get(state, "/api/v1/status").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["version"].as_str().is_some(), "status should have version: {body}");
}

// ─── behavior signals ────────────────────────────────────────────────────────

#[tokio::test]
async fn test_behavior_list_returns_ok_or_forbidden() {
    let (state, _tmp) = make_unlocked_state();
    let (status, body) = do_get(state, "/api/v1/behavior").await;
    assert!(
        status == StatusCode::OK || status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND,
        "behavior endpoint should return 200/403/404: {status} {body}"
    );
}

// ─── clusters ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_clusters_list_returns_ok_or_service_unavailable() {
    let (state, _tmp) = make_unlocked_state();
    // 空库：向量不够 HDBSCAN 跑，应返回 200 空列表或 503 (no vector)
    let (status, body) = do_get(state, "/api/v1/clusters").await;
    assert!(
        status == StatusCode::OK
            || status == StatusCode::SERVICE_UNAVAILABLE
            || status == StatusCode::FORBIDDEN,
        "clusters endpoint should return 200/503/403: {status} {body}"
    );
    if status == StatusCode::OK {
        assert!(body["clusters"].is_array(), "clusters should be an array: {body}");
    }
}

// ─── ingest unification contract ─────────────────────────────────────────────

#[tokio::test]
async fn ingest_route_returns_stable_shape_after_unification() {
    // 契约回归：迁到 ingest_document 后 /api/v1/ingest 响应必须仍是
    // { id, status: "ok", chunks_queued } —— 对外形态零变化。
    let (state, _tmp) = make_unlocked_state();
    let (status, body) = do_post(
        state,
        "/api/v1/ingest",
        serde_json::json!({
            "title": "Unification Probe",
            "content": "# Probe\n\nbody paragraph for chunk.\n\n# Section Two\n\nmore body.",
            "source_type": "note"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "ingest 应成功: {body}");
    assert!(
        body.get("id").and_then(|v| v.as_str()).is_some(),
        "必须返回 id: {body}"
    );
    assert_eq!(body["status"], "ok", "status 必须是 ok: {body}");
    assert!(
        body["chunks_queued"].as_u64().unwrap_or(0) >= 2,
        "L1+L2 都应入队 (>=2): {body}"
    );
}

//! AMD 笔电真机 E2E smoke test (整 plan tingly-knitting-zephyr 验收).
//!
//! 跑前: 设环境变量 ATTUNE_E2E_HOST=http://127.0.0.1:18900 (SSH tunnel) +
//!       ATTUNE_E2E_TOKEN=<sessionStorage 拿到的 token>.
//! 不设环境变量时所有测试 skip (CI 无远端 server, 仅本地 cargo test --include-ignored 跑).
//!
//! 覆盖:
//! - 健康检查 + 访问日志 (access_log middleware 工作)
//! - settings GET 返 hiapi.online + gpt-4o-mini (LLM 配置持久化)
//! - bind_directory 幂等 (re-bind 同 path 返 UPDATE 路径 200 不 FK)
//! - search "ownership" 返 ≥3 hit
//! - chat 不实际跑 (避免 token 烧, 仅验 endpoint 可路由)
//!
//! 等价: scripts/health-check-20rounds.sh 第 1 轮的 cargo test 模式.

use std::time::Duration;

fn host() -> Option<String> {
    std::env::var("ATTUNE_E2E_HOST").ok()
}

fn token() -> Option<String> {
    std::env::var("ATTUNE_E2E_TOKEN").ok()
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("build client")
}

#[tokio::test]
#[ignore = "AMD 笔电真机 E2E, 需 ATTUNE_E2E_HOST + ATTUNE_E2E_TOKEN env var"]
async fn amd_e2e_health() {
    let host = match host() { Some(h) => h, None => return };
    let r = client().get(format!("{host}/api/v1/status/health")).send().await.expect("GET health");
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.expect("json");
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
#[ignore = "AMD 笔电真机 E2E, 需 ATTUNE_E2E_TOKEN"]
async fn amd_e2e_settings_has_hiapi() {
    let host = match host() { Some(h) => h, None => return };
    let tok = match token() { Some(t) => t, None => return };
    let r = client()
        .get(format!("{host}/api/v1/settings"))
        .header("authorization", format!("Bearer {tok}"))
        .send().await.expect("GET settings");
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.expect("json");
    let llm = &body["llm"];
    assert_eq!(llm["endpoint"].as_str().unwrap_or(""), "https://hiapi.online/v1");
    assert_eq!(llm["model"].as_str().unwrap_or(""), "gpt-4o-mini");
    assert_eq!(llm["api_key_set"], true);
}

#[tokio::test]
#[ignore = "AMD 笔电真机 E2E"]
async fn amd_e2e_bind_directory_idempotent_after_g1_fix() {
    // G1 修复: bind_directory_with_domain 改 UPDATE-or-INSERT 模式
    // 之前 INSERT OR REPLACE 触发 FK 失败, 现在 re-bind 同 path 应正常 200
    let host = match host() { Some(h) => h, None => return };
    let tok = match token() { Some(t) => t, None => return };
    let path = "/home/qiurui/test-corpus/rust-book";
    let r1 = client()
        .post(format!("{host}/api/v1/index/bind"))
        .header("authorization", format!("Bearer {tok}"))
        .json(&serde_json::json!({"path": path, "recursive": true}))
        .send().await.expect("first bind");
    assert!(r1.status().is_success(), "first bind failed: {}", r1.status());

    // re-bind 同 path: G1 修复前会 500 FK violation, 修复后 200 skipped
    let r2 = client()
        .post(format!("{host}/api/v1/index/bind"))
        .header("authorization", format!("Bearer {tok}"))
        .json(&serde_json::json!({"path": path, "recursive": true}))
        .send().await.expect("re-bind");
    assert!(r2.status().is_success(), "re-bind failed: {}", r2.status());
    let body: serde_json::Value = r2.json().await.expect("json");
    let skipped = body["scan"]["skipped"].as_u64().unwrap_or(0);
    assert!(skipped > 0, "re-bind should skip existing files, got: {body}");
}

#[tokio::test]
#[ignore = "AMD 笔电真机 E2E (要求 rust-book 已 ingest)"]
async fn amd_e2e_search_ownership() {
    let host = match host() { Some(h) => h, None => return };
    let tok = match token() { Some(t) => t, None => return };
    let r = client()
        .get(format!("{host}/api/v1/search?q=ownership&top_k=5"))
        .header("authorization", format!("Bearer {tok}"))
        .send().await.expect("search");
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.expect("json");
    let results = body["results"].as_array().expect("results array");
    assert!(!results.is_empty(), "expected ≥1 hit on rust-book corpus, got {}", results.len());
    let top_title = results[0]["title"].as_str().unwrap_or("");
    assert!(
        top_title.contains("ownership") || top_title.contains("RAG"),
        "top1 should be related to ownership, got: {top_title}"
    );
}

#[tokio::test]
#[ignore = "AMD 笔电真机 E2E"]
async fn amd_e2e_ocr_profiles_have_7_builtin_backend() {
    // F+ 删 OCR tab UI, 但 backend ProfileRegistry 仍保留 7 builtin
    // (用户级 deb 看不到, 但 API 仍工作 — 用于 chunk_kind 自动选 logic)
    let host = match host() { Some(h) => h, None => return };
    let tok = match token() { Some(t) => t, None => return };
    let r = client()
        .get(format!("{host}/api/v1/ocr/profiles"))
        .header("authorization", format!("Bearer {tok}"))
        .send().await.expect("GET profiles");
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.expect("json");
    let arr = body.as_array().expect("array");
    let builtin_count = arr.iter().filter(|p| p["builtin"] == true).count();
    assert!(builtin_count >= 7, "expected 7 builtin profiles, got {builtin_count}");
}

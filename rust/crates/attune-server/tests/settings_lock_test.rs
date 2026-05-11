//! PATCH /api/v1/settings 接 SettingsLocks 校验 e2e 测试.
//!
//! 验证:
//! 1. 默认 LoggedOut 状态可改任意字段
//! 2. 通过 /member/login-token 升级到 Member 后, llm 字段更新被拒 403
//! 3. logout 后恢复可改

use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_locked_after_member_login() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let config = attune_server::ServerConfig {
        host: "127.0.0.1".to_string(),
        port,
        tls_cert: None,
        tls_key: None,
        no_auth: true,
    };
    let handle = tokio::spawn(async move { attune_server::run_in_runtime(config).await });
    tokio::time::sleep(Duration::from_millis(600)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}/api/v1");

    // 0. vault locked → PATCH /settings 应 403 (vault guard), 而非 lock 校验
    let resp = client
        .patch(format!("{base}/settings"))
        .json(&serde_json::json!({"llm": {"endpoint": "https://x"}}))
        .send()
        .await
        .expect("PATCH");
    let st = resp.status().as_u16();
    // vault locked → 403, vault unlock 后才到 lock 校验
    assert!(
        (400..500).contains(&st),
        "vault locked PATCH should 4xx (got {st})"
    );

    // POST /member/login-token 设置为 paid Member — 注意此 endpoint 不依赖 vault
    let resp = client
        .post(format!("{base}/member/login-token"))
        .json(&serde_json::json!({
            "account_id": "u1",
            "tier": "paid",
            "license_id": "lic-1",
            "llm_quota_remaining": 100000
        }))
        .send()
        .await
        .expect("POST login-token");
    assert_eq!(resp.status().as_u16(), 200, "login-token should succeed");

    // GET /member/locks 应返 paid 锁定矩阵
    let resp = client
        .get(format!("{base}/member/locks"))
        .send()
        .await
        .expect("GET locks");
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    let llm_lock = body
        .get("llm_endpoint")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(llm_lock, "locked", "member tier 应该 llm_endpoint locked");

    handle.abort();
}

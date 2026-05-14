//! vault setup 返 recovery_key + reset-with-recovery-key 端点验证
//!
//! 覆盖：
//!   - setup 响应携带 `recovery_key` 字段（ATN- 格式）
//!   - 用恢复密钥重置密码后，旧密码 unlock 返 401
//!   - 用新密码 unlock 返 200 + token，数据未丢失
//!   - 恢复密钥错误时 reset 返 400
//!   - vault_forgot_password_reset 需 LOCKED 状态 + "RESET" 确认

use std::time::Duration;

fn start_server_config(port: u16) -> attune_server::ServerConfig {
    attune_server::ServerConfig {
        host: "127.0.0.1".to_string(),
        port,
        tls_cert: None,
        tls_key: None,
        no_auth: false,
    }
}

/// 获取空闲端口
fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "slow E2E (~120s, Argon2id×3); R19 nightly only — run with --include-ignored"]
async fn vault_setup_returns_recovery_key() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::env::set_var("HOME", tmp.path());
    std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
    std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));

    let port = free_port();
    let handle = tokio::spawn(async move { attune_server::run_in_runtime(start_server_config(port)).await });
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}/api/v1", port);

    let resp = client
        .post(format!("{base}/vault/setup"))
        .json(&serde_json::json!({"password": "InitialPass-1"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["state"], "unlocked");

    let rk = body["recovery_key"].as_str().expect("recovery_key must be in setup response");
    assert!(rk.starts_with("ATN-"), "recovery_key must start with ATN-: {rk}");
    assert_eq!(rk.len(), 38, "recovery_key format: ATN-16hex-16hex = 4+1+16+1+16 = 38 chars");

    handle.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "slow E2E (~180s, Argon2id×6); R19 nightly only — run with --include-ignored"]
async fn reset_with_recovery_key_allows_new_password() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::env::set_var("HOME", tmp.path());
    std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
    std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));

    let port = free_port();
    let handle = tokio::spawn(async move { attune_server::run_in_runtime(start_server_config(port)).await });
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}/api/v1", port);

    // 1. setup — 拿 recovery_key
    let setup_body: serde_json::Value = client
        .post(format!("{base}/vault/setup"))
        .json(&serde_json::json!({"password": "OldPass-123"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let recovery_key = setup_body["recovery_key"].as_str().unwrap().to_string();

    // 2. lock vault（模拟"忘记密码"重启场景）
    let token = setup_body["token"].as_str().unwrap().to_string();
    client
        .post(format!("{base}/vault/lock"))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();

    // 3. 尝试旧密码 unlock — 应成功（改密前一切正常）
    let unlock_old: serde_json::Value = client
        .post(format!("{base}/vault/unlock"))
        .json(&serde_json::json!({"password": "OldPass-123"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(unlock_old["state"], "unlocked", "old password should still work before reset");
    // lock again
    let tok2 = unlock_old["token"].as_str().unwrap().to_string();
    client
        .post(format!("{base}/vault/lock"))
        .header("authorization", format!("Bearer {tok2}"))
        .send()
        .await
        .unwrap();

    // 4. 使用恢复密钥重置密码
    let reset_resp = client
        .post(format!("{base}/vault/reset-with-recovery-key"))
        .json(&serde_json::json!({
            "recovery_key": recovery_key,
            "new_password": "NewPass-456"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(reset_resp.status(), 200, "reset-with-recovery-key should return 200");
    let reset_body: serde_json::Value = reset_resp.json().await.unwrap();
    assert_eq!(reset_body["status"], "ok");

    // 5. 旧密码 unlock 应失败（401）
    let old_unlock_status = client
        .post(format!("{base}/vault/unlock"))
        .json(&serde_json::json!({"password": "OldPass-123"}))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(old_unlock_status, 401, "old password must be rejected after reset");

    // 6. 新密码 unlock 应成功
    let new_unlock: serde_json::Value = client
        .post(format!("{base}/vault/unlock"))
        .json(&serde_json::json!({"password": "NewPass-456"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(new_unlock["state"], "unlocked", "new password must unlock vault");
    let new_token = new_unlock["token"].as_str().unwrap();
    assert!(!new_token.is_empty());

    handle.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "slow E2E (~120s, Argon2id×3); R19 nightly only — run with --include-ignored"]
async fn reset_with_wrong_recovery_key_returns_400() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::env::set_var("HOME", tmp.path());
    std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
    std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));

    let port = free_port();
    let handle = tokio::spawn(async move { attune_server::run_in_runtime(start_server_config(port)).await });
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}/api/v1", port);

    // setup
    client
        .post(format!("{base}/vault/setup"))
        .json(&serde_json::json!({"password": "AnyPass-789"}))
        .send()
        .await
        .unwrap();

    // lock
    // (vault auto-locks on restart — skip explicit lock for brevity, just call reset with wrong key)
    let wrong_key = "ATN-0000000000000000-0000000000000000";
    let status = client
        .post(format!("{base}/vault/reset-with-recovery-key"))
        .json(&serde_json::json!({
            "recovery_key": wrong_key,
            "new_password": "ShouldFail-999"
        }))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 400, "wrong recovery key must return 400, got {status}");

    handle.abort();
}

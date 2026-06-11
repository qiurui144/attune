//! PATCH /api/v1/settings 接 SettingsLocks 校验 e2e 测试.
//!
//! 验证:
//! 1. 通过 /member/login-token 升级到 paid Member 后, cloud_llm 字段锁定 (locked)
//! 2. paid 会员仍可改 plugin_install (pluginhub URL) — 切到真 HttpPluginHubProvider
//!
//! C1 paywall-bypass fix: 这里用 eval harness（注入 WhitelistMemberVerifier，只认
//! `EVAL_PAID_LICENSE`），所以 paid login-token 必须带正确 license 才真升级到 Paid —— 走真
//! 验证步骤，而非旧的「非空即 Paid」客户端断言。vault-locked PATCH 的 403 路径由
//! vault_lock_endpoint_test + 19 个其它集成测试覆盖，本测试聚焦 paid-lock 矩阵。

use attune_server::test_support::{spawn_eval_server, EVAL_PAID_LICENSE};
use serde_json::json;

#[tokio::test]
async fn settings_locked_after_member_login() {
    let srv = spawn_eval_server().await;
    let base = srv.url();
    let client = reqwest::Client::new();

    // 1. POST /member/login-token 升级到 paid Member —— 必须带 verifier 认可的 license。
    //    此 endpoint 不依赖 vault；verifier (WhitelistMemberVerifier) 真校验 license。
    let resp = client
        .post(format!("{base}/api/v1/member/login-token"))
        .json(&json!({
            "account_id": "u1",
            "tier": "paid",
            "license_id": EVAL_PAID_LICENSE,
            "llm_quota_remaining": 100_000
        }))
        .send()
        .await
        .expect("POST login-token");
    assert_eq!(
        resp.status().as_u16(),
        200,
        "paid login-token with a verifier-approved license should succeed"
    );

    // 2. GET /member/locks 应返 paid 锁定矩阵
    let resp = client
        .get(format!("{base}/api/v1/member/locks"))
        .send()
        .await
        .expect("GET locks");
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    let cloud_llm_lock = body.get("cloud_llm").and_then(|v| v.as_str()).unwrap_or("");
    assert_eq!(cloud_llm_lock, "locked", "paid tier 应该 cloud_llm locked");

    // P0 regression (2026-05-20): paid 会员必须能改 plugin_install / pluginhub URL,
    // 不能被锁在 Mock provider 上 — 否则 entitled 用户拿不到真 pro plugin.
    let plugin_install_lock = body
        .get("plugin_install")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(
        plugin_install_lock, "editable",
        "paid tier 必须能配 pluginhub URL 切到真 HttpPluginHubProvider"
    );
}

/// C1 regression: a paid login-token carrying a license the verifier does NOT approve (forged /
/// not-on-account) must be REJECTED — the member stays unpaid, cloud_llm is NOT locked. Proves the
/// gate enforces real verification, not the old non-empty-string blanket claim.
#[tokio::test]
async fn forged_paid_license_does_not_upgrade_to_paid() {
    let srv = spawn_eval_server().await;
    let base = srv.url();
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/api/v1/member/login-token"))
        .json(&json!({
            "account_id": "attacker",
            "tier": "paid",
            "license_id": "forged-license-not-on-account",
            "llm_quota_remaining": 999_999
        }))
        .send()
        .await
        .expect("POST login-token");
    assert_eq!(
        resp.status().as_u16(),
        403,
        "a forged paid license must be rejected (not silently granted Paid)"
    );
    let v: serde_json::Value = resp.json().await.unwrap_or(json!({}));
    assert_eq!(v["code"], "paid-verification-failed", "stable wire code: {v}");

    // The member state must NOT be paid: cloud_llm stays editable (free/logged-out matrix).
    let locks: serde_json::Value = client
        .get(format!("{base}/api/v1/member/locks"))
        .send()
        .await
        .expect("GET locks")
        .json()
        .await
        .expect("json");
    assert_eq!(
        locks.get("cloud_llm").and_then(|v| v.as_str()).unwrap_or(""),
        "editable",
        "a rejected paid claim must NOT lock cloud_llm — the user is not Paid"
    );
}

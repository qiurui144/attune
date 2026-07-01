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

use attune_server::test_support::{enable_cloud_llm, spawn_eval_server, EVAL_PAID_LICENSE};
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

/// BYOK regression (free-tier personal token): a LoggedOut / Free user MUST be able to save a full
/// personal LLM config (endpoint + api_key + provider + model) via PATCH /settings — they have no
/// gateway, so BYOK is the ONLY way to use chat/AI. The membership cloud_llm lock applies to Paid
/// only (gateway-metered); it must NOT block Free/LoggedOut. Proves the lock matrix end-to-end at
/// the HTTP layer (unit tests in settings.rs cover check_settings_locks in isolation).
#[tokio::test]
async fn logged_out_user_can_save_full_byok_llm_config() {
    let srv = spawn_eval_server().await; // default member state = LoggedOut
    let base = srv.url();
    let client = reqwest::Client::new();

    // Set up + unlock the vault (enable_cloud_llm POSTs /vault/setup; the llm-egress toggle it
    // also flips is harmless here — we only assert the settings-lock matrix).
    enable_cloud_llm(&base).await;

    // Full BYOK config — every field the cloud_llm lock guards for Paid.
    let resp = client
        .patch(format!("{base}/api/v1/settings"))
        .json(&json!({
            "llm": {
                "provider": "openai_compat",
                "endpoint": "https://api.openai.com/v1",
                "api_key": "sk-my-own-personal-key",
                "model": "gpt-4o-mini"
            }
        }))
        .send()
        .await
        .expect("PATCH settings");
    assert_eq!(
        resp.status().as_u16(),
        200,
        "LoggedOut user must be able to save their own BYOK LLM config, got {}",
        resp.status()
    );

    // Confirm it persisted (api_key is redacted to api_key_set=true on GET).
    let got: serde_json::Value = client
        .get(format!("{base}/api/v1/settings"))
        .send()
        .await
        .expect("GET settings")
        .json()
        .await
        .expect("json");
    let llm = got.get("llm").expect("llm block");
    assert_eq!(
        llm.get("provider").and_then(|v| v.as_str()),
        Some("openai_compat")
    );
    assert_eq!(
        llm.get("endpoint").and_then(|v| v.as_str()),
        Some("https://api.openai.com/v1")
    );
    assert_eq!(
        llm.get("model").and_then(|v| v.as_str()),
        Some("gpt-4o-mini")
    );
    assert_eq!(
        llm.get("api_key_set").and_then(|v| v.as_bool()),
        Some(true),
        "api_key must be stored (and redacted on GET)"
    );
}

/// BYOK regression for an authenticated FREE member: logging in as a free tier must NOT lock
/// cloud_llm — free members still configure their own personal API key (no gateway entitlement).
#[tokio::test]
async fn free_member_can_save_full_byok_llm_config() {
    let srv = spawn_eval_server().await;
    let base = srv.url();
    let client = reqwest::Client::new();

    // Log in as a FREE member (no license required for free tier).
    let login = client
        .post(format!("{base}/api/v1/member/login-token"))
        .json(&json!({ "account_id": "free-user", "tier": "free" }))
        .send()
        .await
        .expect("POST login-token");
    assert_eq!(login.status().as_u16(), 200, "free login should succeed");

    // free tier must report cloud_llm editable.
    let locks: serde_json::Value = client
        .get(format!("{base}/api/v1/member/locks"))
        .send()
        .await
        .expect("GET locks")
        .json()
        .await
        .expect("json");
    assert_eq!(
        locks
            .get("cloud_llm")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        "editable",
        "free tier must keep cloud_llm editable for BYOK"
    );

    // Unlock vault, then save full BYOK config.
    enable_cloud_llm(&base).await;
    let resp = client
        .patch(format!("{base}/api/v1/settings"))
        .json(&json!({
            "llm": {
                "provider": "deepseek",
                "endpoint": "https://api.deepseek.com/v1",
                "api_key": "sk-free-user-own-key",
                "model": "deepseek-chat"
            }
        }))
        .send()
        .await
        .expect("PATCH settings");
    assert_eq!(
        resp.status().as_u16(),
        200,
        "free member must be able to save their own BYOK LLM config, got {}",
        resp.status()
    );
}

/// Contrast: a PAID member's BYOK attempt (own endpoint/api_key/provider) IS blocked — gateway is
/// cloud-metered. This pins the asymmetry: lock affects Paid only, never Free/LoggedOut.
#[tokio::test]
async fn paid_member_byok_llm_config_is_blocked() {
    let srv = spawn_eval_server().await;
    let base = srv.url();
    let client = reqwest::Client::new();

    let login = client
        .post(format!("{base}/api/v1/member/login-token"))
        .json(&json!({
            "account_id": "paid-user",
            "tier": "paid",
            "license_id": EVAL_PAID_LICENSE,
            "llm_quota_remaining": 100_000
        }))
        .send()
        .await
        .expect("POST login-token");
    assert_eq!(login.status().as_u16(), 200, "paid login should succeed");

    enable_cloud_llm(&base).await;
    let resp = client
        .patch(format!("{base}/api/v1/settings"))
        .json(&json!({
            "llm": {
                "provider": "openai_compat",
                "endpoint": "https://api.openai.com/v1",
                "api_key": "sk-paid-tries-to-bypass-gateway"
            }
        }))
        .send()
        .await
        .expect("PATCH settings");
    assert_eq!(
        resp.status().as_u16(),
        403,
        "paid member changing endpoint/api_key/provider must be blocked (gateway-metered)"
    );
    let v: serde_json::Value = resp.json().await.unwrap_or(json!({}));
    assert_eq!(
        v["error"], "setting_locked_by_member_tier",
        "stable lock error: {v}"
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
    assert_eq!(
        v["code"], "paid-verification-failed",
        "stable wire code: {v}"
    );

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
        locks
            .get("cloud_llm")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        "editable",
        "a rejected paid claim must NOT lock cloud_llm — the user is not Paid"
    );
}

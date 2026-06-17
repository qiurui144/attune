//! #79 — 授权码 (license_key) 激活 → gateway LLM wiring (client 端).
//!
//! 镜像 login_password 的付费分支:POST /api/v1/member/activate-license 调 cloud
//! `POST /api/v1/member/activate`,把下发的 gateway 配置 (锁定 endpoint/api_key/
//! provider) 写进 vault settings、落 entitlement (allowed_plugins)、置
//! MemberState::Paid。
//!
//! 全程走真 axum router + 真 (in-memory) vault + 一个本地 mock cloud server,
//! 不 mock 内部逻辑;断言 §6.1 happy / 边界 / 异常三类。
//!
//! **单 test fn**(同 member_auth_test):server 用 set_var 把 HOME/XDG 重定向到
//! tempdir,多 fn 在同一进程内并行竞争 set_var 不安全,故所有断言(边界 / 错误 /
//! no-gateway / full-contract)串在一个 fn 内顺序跑。

use std::sync::Arc;

/// 起一个极简 mock cloud:`POST /api/v1/member/activate` 返回固定的 activate 契约。
/// 返回 (base_url, JoinHandle)。
async fn spawn_mock_cloud(
    body: serde_json::Value,
    status: u16,
) -> (String, tokio::task::JoinHandle<()>) {
    use axum::{routing::post, Json, Router};
    let app = Router::new().route(
        "/api/v1/member/activate",
        post(move || {
            let body = body.clone();
            async move { (axum::http::StatusCode::from_u16(status).unwrap(), Json(body)) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app.into_make_service()).await;
    });
    (format!("http://{addr}"), handle)
}

async fn member_state(client: &reqwest::Client, base: &str) -> serde_json::Value {
    client
        .get(format!("{base}/api/v1/member/state"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn get_settings(client: &reqwest::Client, base: &str) -> serde_json::Value {
    client
        .get(format!("{base}/api/v1/settings"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn activate_license_full_contract() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("HOME", tmp.path());
        // HF_HUB_OFFLINE: fresh empty $HOME → empty model cache; without this
        // init_search_engines() (called in vault_setup) blocks on a hf-hub download
        // inside the async handler → "Cannot drop a runtime" panic (per fix 9936dca).
        std::env::set_var("HF_HUB_OFFLINE", "1");
        std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
        std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
    }

    // ── attune server (require_auth=false → 聚焦激活逻辑) ─────────────────────
    let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("open in-memory vault");
    let state = Arc::new(attune_server::state::AppState::new(vault, false));
    let router = attune_server::build_router(Arc::clone(&state));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let srv = tokio::spawn(async move {
        let _ = axum::serve(listener, router.into_make_service()).await;
    });
    let base = format!("http://{addr}");
    let client = reqwest::Client::new();

    // readiness via always-public /health.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if let Ok(r) = client.get(format!("{base}/health")).send().await {
            if r.status().is_success() {
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    // ── 边界: empty license_key → 400, NOT Paid (在 unlock 前先验,无副作用) ───
    let resp = client
        .post(format!("{base}/api/v1/member/activate-license"))
        .json(&serde_json::json!({ "license_key": "   " }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400, "blank license_key → 400");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "license-key-required");

    // ── 异常: cloud 4xx (无效授权码) → 502 activate-failed, NOT Paid ──────────
    let (bad_cloud, bad) = spawn_mock_cloud(serde_json::json!({"error": "invalid"}), 403).await;
    let resp = client
        .post(format!("{base}/api/v1/member/activate-license"))
        .json(&serde_json::json!({ "license_key": "ATTUNE-LIC-BAD", "cloud_url": bad_cloud }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 502, "invalid license → 502");
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["code"],
        "activate-failed"
    );
    bad.abort();

    // ── 异常: cloud unreachable → 502 ────────────────────────────────────────
    let resp = client
        .post(format!("{base}/api/v1/member/activate-license"))
        .json(&serde_json::json!({ "license_key": "ATTUNE-LIC-X", "cloud_url": "http://127.0.0.1:1" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 502, "unreachable cloud → 502");

    // after all failures: still NOT Paid (no paywall bypass).
    assert_eq!(
        member_state(&client, &base).await["is_paid"],
        false,
        "failed activation must NOT set Paid (fail-closed)"
    );

    // ── 解锁 vault,才能持久化 gateway settings + entitlements ─────────────────
    let resp = client
        .post(format!("{base}/api/v1/vault/setup"))
        .json(&serde_json::json!({ "password": "P@ss-activate-not-real" }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "vault setup: {}", resp.status());

    // ── 异常: cloud 200 但 gateway 字段缺 (manual 会员无 gateway) → 仍 Paid,但
    //    不写 gateway settings (此时 vault 刚 unlock,llm 未配置 → api_key_set 应仍 false)。
    //    必须在 full-contract 写入之前断言,否则会读到后写入的 gateway key。
    let (nogw_cloud, nogw) = spawn_mock_cloud(
        serde_json::json!({"plan": "pro", "allowed_plugins": ["law-pro"]}),
        200,
    )
    .await;
    let resp = client
        .post(format!("{base}/api/v1/member/activate-license"))
        .json(&serde_json::json!({ "license_key": "ATTUNE-LIC-NOGW", "cloud_url": nogw_cloud }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "activate w/o gateway → still 200");
    nogw.abort();
    assert_eq!(
        member_state(&client, &base).await["is_paid"],
        true,
        "Paid even without gateway fields"
    );
    assert_eq!(
        get_settings(&client, &base).await["llm"]["api_key_set"],
        false,
        "no gateway token present → must not have written one"
    );

    // ── happy path: full gateway contract → 200 Paid + gateway wired/locked ───
    let (good_cloud, good) = spawn_mock_cloud(
        serde_json::json!({
            "plan": "pro",
            "expires_at": "2026-12-31T00:00:00+00:00",
            "allowed_plugins": ["law-pro", "med-pro"],
            "gateway_token": "sk-newapi-activate-not-real",
            "gateway_url": "https://gateway.engi-stack.com/v1",
            "gateway_default_model": "deepseek-v4-flash"
        }),
        200,
    )
    .await;
    let resp = client
        .post(format!("{base}/api/v1/member/activate-license"))
        .json(&serde_json::json!({ "license_key": "ATTUNE-LIC-VALID", "cloud_url": good_cloud }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "activate must 200: {}", resp.status());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["plan"], "pro");
    assert_eq!(body["allowed_plugins"][0], "law-pro");
    good.abort();

    // MemberState::Paid.
    assert_eq!(
        member_state(&client, &base).await["is_paid"],
        true,
        "activate must set MemberState::Paid"
    );

    // cloud_llm setting LOCKED for Paid (gateway-managed key) — mirrors login_password.
    let locks: serde_json::Value = client
        .get(format!("{base}/api/v1/member/locks"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(locks["cloud_llm"], "locked", "Paid → cloud_llm locked: {locks}");

    // settings.llm carries the gateway endpoint/provider/model; GET redacts api_key
    // to null + sets api_key_set (settings.rs::redact_api_key).
    let llm = get_settings(&client, &base).await["llm"].clone();
    assert_eq!(llm["provider"], "openai_compat", "gateway provider written");
    assert_eq!(llm["endpoint"], "https://gateway.engi-stack.com/v1", "gateway endpoint written");
    assert_eq!(llm["api_key_set"], true, "gateway token written (api_key_set)");
    assert_eq!(llm["model"], "deepseek-v4-flash", "gateway default model written");

    srv.abort();
}

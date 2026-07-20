//! #79 — 授权码 (license_key) 激活 → gateway LLM wiring (client 端).
//!
//! 镜像 login_password 的付费分支:POST /api/v1/member/activate-license 调 cloud
//! `POST /api/v1/member/activate`,把下发的 gateway 配置 (锁定 endpoint/api_key/
//! provider) 写进 vault settings、落 entitlement (allowed_plugins)、置
//! MemberState::Paid。
//!
//! 全程走真 axum router + 真 (in-memory) vault + 一个本地 mock cloud server,
//! 不 mock 内部逻辑;断言 §6.1 happy / 边界 / 异常三类。**完全离线确定性**:
//! mock cloud 跑在 127.0.0.1:0,activate-license 走的 accounts_url 由**服务端
//! settings** (`cloud.accounts_url`) 决定 —— 测试通过 settings API 把它指向 mock
//! (请求体不再带 `cloud_url`:client-controlled cloud_url 是 SSRF/付费墙绕过,
//! 已被服务端丢弃,见 868e893)。因此本测试**不依赖任何 live gateway**,验的是
//! client 解析 + gateway wiring 逻辑。
//!
//! **单 test fn**(同 member_auth_test):server 用 set_var 把 HOME/XDG 重定向到
//! tempdir,多 fn 在同一进程内并行竞争 set_var 不安全,故所有断言(边界 / 错误 /
//! no-gateway / full-contract)串在一个 fn 内顺序跑。

use std::sync::Arc;

/// 起一个本地 mock cloud,服务授权码激活全链需要的两个端点:
/// - `POST /api/v1/member/activate` → 固定的 activate 契约 (status/body 可控)。
/// - `POST /api/v1/devices/activate` → 固定的设备绑定成功响应 (好路径需它成功
///   才能 200;否则 device 绑定失败会让 activate-license 返回 409/403/502)。
///
/// 返回 (base_url, JoinHandle)。
async fn spawn_mock_cloud(
    activate_body: serde_json::Value,
    activate_status: u16,
) -> (String, tokio::task::JoinHandle<()>) {
    let (url, handle, _, _) = spawn_counted_mock_cloud(activate_body, activate_status).await;
    (url, handle)
}

async fn spawn_counted_mock_cloud(
    activate_body: serde_json::Value,
    activate_status: u16,
) -> (
    String,
    tokio::task::JoinHandle<()>,
    Arc<std::sync::atomic::AtomicUsize>,
    Arc<std::sync::atomic::AtomicUsize>,
) {
    use axum::{routing::post, Json, Router};
    use std::sync::atomic::Ordering;

    let activate_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let device_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let device_body = serde_json::json!({
        "device_token": "dev-token-not-real",
        "device_id": "test-device-id",
        "plan": "pro",
        "max_activations": 3,
        "current_activations": 1
    });
    let activate_calls_for_route = Arc::clone(&activate_calls);
    let device_calls_for_route = Arc::clone(&device_calls);
    let app = Router::new()
        .route(
            "/api/v1/member/activate",
            post(move || {
                let body = activate_body.clone();
                let calls = Arc::clone(&activate_calls_for_route);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    (
                        axum::http::StatusCode::from_u16(activate_status).unwrap(),
                        Json(body),
                    )
                }
            }),
        )
        .route(
            "/api/v1/devices/activate",
            post(move || {
                let body = device_body.clone();
                let calls = Arc::clone(&device_calls_for_route);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    (axum::http::StatusCode::OK, Json(body))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app.into_make_service()).await;
    });
    (
        format!("http://{addr}"),
        handle,
        activate_calls,
        device_calls,
    )
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

/// 把服务端 `cloud.accounts_url` 设为 `url`(指向本地 mock)。activate-license 由此
/// 解析要调的 cloud,而**不是**请求体里的 cloud_url(SSRF/付费墙绕过,已丢弃)。
/// 需 vault 已解锁(settings 写 vault meta)。
async fn set_accounts_url(client: &reqwest::Client, base: &str, url: &str) {
    let resp = client
        .patch(format!("{base}/api/v1/settings"))
        .json(&serde_json::json!({ "cloud": { "accounts_url": url } }))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "set accounts_url must succeed: {}",
        resp.status()
    );
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
    let db_path = tmp.path().join("vault.db");
    let config_dir = tmp.path().join("vault-config");
    let vault = attune_core::vault::Vault::open(&db_path, &config_dir).expect("open vault");
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

    // ── 解锁 vault:settings (accounts_url) + gateway settings + entitlements 都
    //    要写 vault meta,需先解锁。 ───────────────────────────────────────────
    let resp = client
        .post(format!("{base}/api/v1/vault/setup"))
        .json(&serde_json::json!({ "password": "P@ss-activate-not-real" }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "vault setup: {}", resp.status());

    // ── 异常: cloud 4xx (无效授权码) → 502 activate-failed, NOT Paid ──────────
    // 把服务端 accounts_url 指向返回 403 的 mock(请求体不带 cloud_url,SSRF-safe)。
    let (bad_cloud, bad) = spawn_mock_cloud(serde_json::json!({"error": "invalid"}), 403).await;
    set_accounts_url(&client, &base, &bad_cloud).await;
    let resp = client
        .post(format!("{base}/api/v1/member/activate-license"))
        .json(&serde_json::json!({ "license_key": "ATTUNE-LIC-BAD" }))
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
    set_accounts_url(&client, &base, "http://127.0.0.1:1").await;
    let resp = client
        .post(format!("{base}/api/v1/member/activate-license"))
        .json(&serde_json::json!({ "license_key": "ATTUNE-LIC-X" }))
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

    // A successful HTTP response is not itself a paid entitlement. A valid
    // individual/free license must fail closed before device binding, plugin
    // download, or local Paid state is applied.
    let (free_cloud, free) = spawn_mock_cloud(
        serde_json::json!({
            "plan": "individual",
            "allowed_plugins": ["law-pro"],
            "gateway_token": "must-not-be-installed",
            "gateway_url": "https://gateway.engi-stack.com/v1"
        }),
        200,
    )
    .await;
    set_accounts_url(&client, &base, &free_cloud).await;
    let resp = client
        .post(format!("{base}/api/v1/member/activate-license"))
        .json(&serde_json::json!({ "license_key": "ATTUNE-LIC-FREE" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 403, "free plan must not grant Paid");
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap()["code"],
        "license-plan-not-paid"
    );
    free.abort();
    assert_eq!(
        member_state(&client, &base).await["is_paid"],
        false,
        "non-paid license response must leave member state unpaid"
    );

    // ── 异常: cloud 200 但 gateway 字段缺 (manual 会员无 gateway) → 仍 Paid,但
    //    不写 gateway settings (此时 vault 刚 unlock,llm 未配置 → api_key_set 应仍 false)。
    //    必须在 full-contract 写入之前断言,否则会读到后写入的 gateway key。
    let (nogw_cloud, nogw) = spawn_mock_cloud(
        serde_json::json!({"plan": "pro", "allowed_plugins": ["law-pro"]}),
        200,
    )
    .await;
    set_accounts_url(&client, &base, &nogw_cloud).await;
    let resp = client
        .post(format!("{base}/api/v1/member/activate-license"))
        .json(&serde_json::json!({ "license_key": "ATTUNE-LIC-NOGW" }))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "activate w/o gateway → still 200"
    );
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

    // Consent durability is the first local commit barrier. If the privacy
    // block cannot be persisted, activation must return an error without
    // clearing the existing session or replacing its active member state.
    let (consent_cloud, consent_server) = spawn_mock_cloud(
        serde_json::json!({"plan": "pro", "allowed_plugins": []}),
        200,
    )
    .await;
    set_accounts_url(&client, &base, &consent_cloud).await;
    let before_consent_failure = member_state(&client, &base).await;
    // The previous license-code activation has no account cookie. A failed
    // replacement must preserve that active runtime and must not manufacture
    // or retire unrelated durable session state. Account-cookie switching is
    // covered separately by the epoch/transaction regression tests.
    assert!(attune_core::cloud_session::load_cloud_session()
        .expect("load session precondition")
        .is_none());
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let raw = vault
            .store()
            .get_meta(attune_core::llm_settings::SETTINGS_META_KEY)
            .unwrap()
            .unwrap();
        let mut settings: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        settings["privacy"] = serde_json::json!([]);
        vault
            .store()
            .set_meta(
                attune_core::llm_settings::SETTINGS_META_KEY,
                &serde_json::to_vec(&settings).unwrap(),
            )
            .unwrap();
    }
    let resp = client
        .post(format!("{base}/api/v1/member/activate-license"))
        .json(&serde_json::json!({ "license_key": "ATTUNE-LIC-CONSENT-FAIL" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 500);
    assert_eq!(
        member_state(&client, &base).await,
        before_consent_failure,
        "consent failure must occur before old member runtime is cleared"
    );
    assert!(attune_core::cloud_session::load_cloud_session()
        .expect("load cloud session after failed activation")
        .is_none());
    consent_server.abort();
    // Repair the deliberately malformed privacy block for the remaining happy
    // path. The previous successful activation had already opted in.
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let raw = vault
            .store()
            .get_meta(attune_core::llm_settings::SETTINGS_META_KEY)
            .unwrap()
            .unwrap();
        let mut settings: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        settings["privacy"] = serde_json::json!({
            "llm": false,
            "cloud_saas": true,
            "webdav": false,
            "web_search": false,
            "telemetry": false,
            "privacy_tour_seen": false
        });
        vault
            .store()
            .set_meta(
                attune_core::llm_settings::SETTINGS_META_KEY,
                &serde_json::to_vec(&settings).unwrap(),
            )
            .unwrap();
    }

    // ── happy path: full gateway contract → 200 Paid + gateway wired/locked ───
    let (good_cloud, good, activate_calls, device_calls) = spawn_counted_mock_cloud(
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
    set_accounts_url(&client, &base, &good_cloud).await;
    let resp = client
        .post(format!("{base}/api/v1/member/activate-license"))
        .json(&serde_json::json!({ "license_key": "ATTUNE-LIC-VALID" }))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "activate must 200: {}",
        resp.status()
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["plan"], "pro");
    assert_eq!(body["allowed_plugins"][0], "law-pro");

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
    assert_eq!(
        locks["cloud_llm"], "locked",
        "Paid → cloud_llm locked: {locks}"
    );

    // settings.llm carries the gateway endpoint/provider/model; GET redacts api_key
    // to null + sets api_key_set (settings.rs::redact_api_key).
    let llm = get_settings(&client, &base).await["llm"].clone();
    assert_eq!(llm["provider"], "openai_compat", "gateway provider written");
    assert_eq!(
        llm["endpoint"], "https://gateway.engi-stack.com/v1",
        "gateway endpoint written"
    );
    assert_eq!(
        llm["api_key_set"], true,
        "gateway token written (api_key_set)"
    );
    assert_eq!(
        llm["model"], "deepseek-v4-flash",
        "gateway default model written"
    );

    // A license-code activation has no account cookie. Reopen the same vault
    // in a fresh AppState, unlock it, and require a second online authorization
    // + device proof before Paid/runtime state is rebuilt.
    srv.abort();
    let _ = srv.await;
    drop(state);

    let restarted_vault =
        attune_core::vault::Vault::open(&db_path, &config_dir).expect("reopen persisted vault");
    assert!(matches!(
        restarted_vault.state(),
        attune_core::vault::VaultState::Locked
    ));
    let restarted_state = Arc::new(attune_server::state::AppState::new(restarted_vault, false));
    let restarted_router = attune_server::build_router(Arc::clone(&restarted_state));
    let restarted_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let restarted_addr = restarted_listener.local_addr().unwrap();
    let restarted_server = tokio::spawn(async move {
        let _ = axum::serve(restarted_listener, restarted_router.into_make_service()).await;
    });
    let restarted_base = format!("http://{restarted_addr}");
    let unlock = client
        .post(format!("{restarted_base}/api/v1/vault/unlock"))
        .json(&serde_json::json!({ "password": "P@ss-activate-not-real" }))
        .send()
        .await
        .expect("unlock restarted vault");
    assert!(
        unlock.status().is_success(),
        "restart unlock: {}",
        unlock.status()
    );

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let restored = loop {
        let snapshot = member_state(&client, &restarted_base).await;
        if snapshot["is_paid"] == true || std::time::Instant::now() >= deadline {
            break snapshot;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    assert_eq!(
        restored["is_paid"], true,
        "encrypted activation receipt must restore only after online re-verification: {restored}"
    );
    assert!(
        activate_calls.load(std::sync::atomic::Ordering::SeqCst) >= 2,
        "restart restore must call cloud authorization again"
    );
    assert!(
        device_calls.load(std::sync::atomic::Ordering::SeqCst) >= 2,
        "restart restore must prove the device again"
    );
    assert_eq!(
        get_settings(&client, &restarted_base).await["llm"]["api_key_set"],
        true,
        "restored activation must rebuild the encrypted member gateway"
    );

    restarted_server.abort();
    good.abort();
}

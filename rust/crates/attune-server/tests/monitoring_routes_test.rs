//! 信息监控闭环 REST 集成测试 (spec §9.3) —— 真 axum 路由，黑盒用户视角。
//!
//! 两 lane：
//!  1. locked vault → 监控端点拒绝（401/403）。
//!  2. unlocked round-trip: ingest items → create watch → scan → list hits (triage 序) →
//!     trigger digest (默认零成本，断言无 LLM summary) → ask (无 LLM provider → 503) →
//!     research (无 LLM → degraded retrieval list)。这是真路由契约（状态码 / JSON shape）。

use std::sync::Arc;
use std::time::Duration;

async fn wait_for_server(base: &str) {
    let client = reqwest::Client::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let url = format!("{}/health", base);
    while std::time::Instant::now() < deadline {
        if let Ok(r) = client.get(&url).send().await {
            if r.status().is_success() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("server did not become ready");
}

#[allow(unsafe_code)]
async fn spawn(setup_vault: bool) -> (String, reqwest::Client) {
    let tmp = tempfile::TempDir::new().expect("tmp");
    unsafe {
        std::env::set_var("HOME", tmp.path());
        std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
        std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
    }
    let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("open in-memory vault");
    let state = Arc::new(attune_server::state::AppState::new(vault, false));
    let router = attune_server::build_router(Arc::clone(&state));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let base = format!("http://127.0.0.1:{}", port);
    wait_for_server(&base).await;
    let client = reqwest::Client::new();
    if setup_vault {
        let resp = client
            .post(format!("{}/api/v1/vault/setup", base))
            .json(&serde_json::json!({"password": "test-password-not-real"}))
            .send()
            .await
            .expect("vault setup");
        assert_eq!(resp.status().as_u16(), 200, "vault setup must succeed");
    }
    Box::leak(Box::new(tmp));
    (base, client)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn monitoring_endpoints_locked_vault_rejected() {
    let (base, client) = spawn(false).await;
    let acceptable = |s: u16| s == 401 || s == 403;
    let watches = format!("{}/api/v1/monitoring/watches", base);
    let cases: Vec<(reqwest::RequestBuilder, &str)> = vec![
        (client.get(&watches), "GET watches"),
        (
            client.post(&watches).json(&serde_json::json!({"label": "x", "keywords": ["RVV"]})),
            "POST watches",
        ),
        (
            client.post(format!("{}/api/v1/monitoring/scan", base)),
            "POST scan",
        ),
        (
            client
                .post(format!("{}/api/v1/monitoring/research", base))
                .json(&serde_json::json!({"topic": "t"})),
            "POST research",
        ),
    ];
    for (req, label) in cases {
        let resp = req.send().await.expect(label);
        assert!(
            acceptable(resp.status().as_u16()),
            "{label}: expected 401/403 locked, got {}",
            resp.status()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn monitoring_round_trip_unlocked() {
    let (base, client) = spawn(true).await;
    let mon = format!("{}/api/v1/monitoring", base);

    // 1. ingest 3 items (2 contain RVV).
    for (title, content) in [
        ("RVV lands", "the RVV vector extension is finalized"),
        ("unrelated", "cooking recipes and gardening tips"),
        ("more RVV", "RVV autovectorization improvements in gcc"),
    ] {
        let r = client
            .post(format!("{}/api/v1/ingest", base))
            .json(&serde_json::json!({"title": title, "content": content, "source_type": "note"}))
            .send()
            .await
            .expect("ingest");
        assert_eq!(r.status().as_u16(), 200, "ingest {title}");
    }

    // 2. empty watch list initially.
    let r = client.get(format!("{}/watches", mon)).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["watches"].as_array().unwrap().len(), 0);

    // 3. validation: empty label → 400; no criteria → 400.
    let r = client
        .post(format!("{}/watches", mon))
        .json(&serde_json::json!({"label": "", "keywords": ["RVV"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 400, "empty label rejected");
    let r = client
        .post(format!("{}/watches", mon))
        .json(&serde_json::json!({"label": "x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 400, "no criteria rejected");

    // 4. create a real watch (keyword RVV, low threshold).
    let r = client
        .post(format!("{}/watches", mon))
        .json(&serde_json::json!({
            "label": "RISC-V", "keywords": ["RVV"], "match_threshold": 0.4, "digest_period": "weekly"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200, "create watch");
    let watch: serde_json::Value = r.json().await.unwrap();
    let watch_id = watch["id"].as_str().unwrap().to_string();
    assert_eq!(watch["label"], "RISC-V");

    // 5. scan → matches the 2 RVV items.
    let r = client.post(format!("{}/scan", mon)).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let scan: serde_json::Value = r.json().await.unwrap();
    assert_eq!(scan["new_hits"].as_u64().unwrap(), 2, "2 RVV items matched");

    // 6. list hits → triage-ordered, with reasons, no LLM.
    let r = client
        .get(format!("{}/watches/{}/hits", mon, watch_id))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let hits: serde_json::Value = r.json().await.unwrap();
    let arr = hits["hits"].as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert!(!arr[0]["reasons"].as_array().unwrap().is_empty(), "explainable reasons");
    let s0 = arr[0]["score"].as_f64().unwrap();
    let s1 = arr[1]["score"].as_f64().unwrap();
    assert!(s0 >= s1, "triage: score descending");

    // 7. default digest → zero-cost, no LLM summary, free tier.
    let r = client
        .post(format!("{}/watches/{}/digest", mon, watch_id))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let digest: serde_json::Value = r.json().await.unwrap();
    assert_eq!(digest["entries"].as_u64().unwrap(), 2);
    assert!(digest["llm_summary"].is_null(), "default digest carries no LLM summary");
    assert_eq!(digest["cost_hint"]["tier"], "free", "zero-cost tier");

    // 8. cross-time dedup: after digest, hits marked digested → second digest no-hits.
    let r = client
        .post(format!("{}/watches/{}/digest", mon, watch_id))
        .send()
        .await
        .unwrap();
    let digest2: serde_json::Value = r.json().await.unwrap();
    assert!(digest2["card_id"].is_null(), "digested hits not re-pushed (cross-time dedup)");

    // 9. ask as a non-member (this harness has no member login) → 403 membership-required.
    //    tier-3 💰 gate: free / logged-out users are blocked BEFORE any token spend (GA P0-1).
    //    The LLM-unavailable / privacy-gate / redaction paths are covered with a paid login in
    //    monitoring_gates_test.rs (eval harness).
    let r = client
        .post(format!("{}/watches/{}/ask", mon, watch_id))
        .json(&serde_json::json!({"question": "what about RVV?"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 403, "non-member ask → 403 (tier-3 member gate)");
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["code"], "membership-required", "ask gate code");

    // 10. research as a non-member → 403 membership-required (tier-3, same gate).
    let r = client
        .post(format!("{}/research", mon))
        .json(&serde_json::json!({"topic": "RVV", "use_web": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 403, "non-member research → 403 (tier-3 member gate)");
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["code"], "membership-required", "research gate code");

    // 11. patch + delete.
    let r = client
        .patch(format!("{}/watches/{}", mon, watch_id))
        .json(&serde_json::json!({"enabled": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let r = client.delete(format!("{}/watches/{}", mon, watch_id)).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 200);
    // delete unknown → 404.
    let r = client.delete(format!("{}/watches/nope", mon)).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 404, "unknown watch → 404");
}

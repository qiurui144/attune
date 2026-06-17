//! Chat Session API integration tests — exercises the REAL axum routes
//! (`GET /api/v1/chat/sessions`, `GET /api/v1/chat/sessions/:id`,
//! `DELETE /api/v1/chat/sessions/:id`).
//!
//! Previously this file re-implemented the route logic at the `Store` layer
//! ("模拟 chat.rs 中 ... 的逻辑"), so the actual HTTP handlers — status codes,
//! JSON envelope shapes (`{sessions,total}` / `{session,messages}`), 404 / 204,
//! the locked-vault guard — were never asserted and could drift silently.
//!
//! Sessions are created via `POST /chat`, which needs a live LLM. To test the
//! list/get/delete routes without an LLM, we seed conversations directly into
//! the server's in-memory vault BEFORE building the router, then drive the real
//! HTTP handlers. One small Store-layer test for the cascade-delete invariant
//! is retained (a legitimate store unit, not a route re-implementation).

use std::sync::Arc;
use std::time::Duration;

use attune_core::crypto::Key32;
use attune_core::vault::Vault;

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

/// Build an unlocked in-memory vault, run `seed` against its store (to create
/// conversations), then stand up the REAL router over it. Returns `(base, client)`.
#[allow(unsafe_code)] // env isolation (AppState uses data_dir() for tantivy/vectors)
async fn spawn_with_seed<F>(seed: F) -> (String, reqwest::Client)
where
    F: FnOnce(&Vault, &Key32),
{
    let tmp = tempfile::TempDir::new().expect("tmp");
    // SAFETY: isolate $HOME per test process (device.key path). Each test has its
    // own in-memory vault so cross-test state cannot leak through the DB.
    unsafe {
        std::env::set_var("HOME", tmp.path());
        std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
        std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
    }

    let vault = Vault::open_memory(tmp.path()).expect("open in-memory vault");
    vault.setup("test-password-not-real").expect("setup leaves vault unlocked");
    let dek = vault.dek_db().expect("vault unlocked → dek available");
    seed(&vault, &dek);

    let state = Arc::new(attune_server::state::AppState::new(vault, false));
    let router = attune_server::build_router(Arc::clone(&state));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let base = format!("http://127.0.0.1:{}", port);
    wait_for_server(&base).await;
    Box::leak(Box::new(tmp));
    (base, reqwest::Client::new())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_sessions_route_returns_seeded_sessions_with_envelope() {
    let (base, client) = spawn_with_seed(|vault, dek| {
        let s1 = vault.store().create_conversation(dek, "第一个会话").unwrap();
        vault.store().append_message(dek, &s1, "user", "hello", &[]).unwrap();
        vault.store().create_conversation(dek, "第二个会话").unwrap();
    })
    .await;

    let resp = client
        .get(format!("{}/api/v1/chat/sessions", base))
        .send()
        .await
        .expect("GET sessions");
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    // Real handler envelope: {"sessions": [...], "total": N}
    assert_eq!(body["total"], 2, "both seeded sessions must be listed");
    let arr = body["sessions"].as_array().expect("sessions array");
    assert_eq!(arr.len(), 2);
    let titles: Vec<&str> = arr.iter().filter_map(|s| s["title"].as_str()).collect();
    assert!(titles.contains(&"第一个会话") && titles.contains(&"第二个会话"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_sessions_pagination_clamps_limit_via_query() {
    let (base, client) = spawn_with_seed(|vault, dek| {
        for i in 0..5 {
            vault.store().create_conversation(dek, &format!("会话{i}")).unwrap();
        }
    })
    .await;

    // limit=3 honored by the real Query<PaginationQuery> parsing.
    let resp = client
        .get(format!("{}/api/v1/chat/sessions?limit=3&offset=0", base))
        .send()
        .await
        .expect("page1");
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["sessions"].as_array().unwrap().len(), 3, "limit=3 returns 3");

    let resp = client
        .get(format!("{}/api/v1/chat/sessions?limit=3&offset=3", base))
        .send()
        .await
        .expect("page2");
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["sessions"].as_array().unwrap().len(), 2, "offset=3 returns remaining 2");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_session_route_returns_session_and_messages() {
    let seeded_id = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let captured = Arc::clone(&seeded_id);
    let (base, client) = spawn_with_seed(move |vault, dek| {
        let sid = vault.store().create_conversation(dek, "带消息的会话").unwrap();
        vault.store().append_message(dek, &sid, "user", "问题", &[]).unwrap();
        vault.store().append_message(dek, &sid, "assistant", "回答", &[]).unwrap();
        *captured.lock().unwrap() = sid;
    })
    .await;
    let sid = seeded_id.lock().unwrap().clone();

    let resp = client
        .get(format!("{}/api/v1/chat/sessions/{}", base, sid))
        .send()
        .await
        .expect("GET session");
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    // Real handler envelope: {"session": {...}, "messages": [...]}
    assert_eq!(body["session"]["id"], sid);
    let msgs = body["messages"].as_array().expect("messages array");
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0]["role"], "user");
    assert_eq!(msgs[0]["content"], "问题");
    assert_eq!(msgs[1]["role"], "assistant");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_unknown_session_route_returns_404() {
    let (base, client) = spawn_with_seed(|_v, _d| {}).await;
    let resp = client
        .get(format!("{}/api/v1/chat/sessions/no-such-id", base))
        .send()
        .await
        .expect("GET missing");
    assert_eq!(resp.status().as_u16(), 404, "unknown session id → 404 from the real handler");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_session_route_returns_204_and_removes_it() {
    let seeded_id = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let captured = Arc::clone(&seeded_id);
    let (base, client) = spawn_with_seed(move |vault, dek| {
        let sid = vault.store().create_conversation(dek, "要删除").unwrap();
        vault.store().append_message(dek, &sid, "user", "x", &[]).unwrap();
        *captured.lock().unwrap() = sid;
    })
    .await;
    let sid = seeded_id.lock().unwrap().clone();

    // DELETE → 204 No Content.
    let resp = client
        .delete(format!("{}/api/v1/chat/sessions/{}", base, sid))
        .send()
        .await
        .expect("DELETE");
    assert_eq!(resp.status().as_u16(), 204, "delete returns 204 NO_CONTENT");

    // Now GET that id → 404 (the real get handler), and list is empty.
    let resp = client
        .get(format!("{}/api/v1/chat/sessions/{}", base, sid))
        .send()
        .await
        .expect("GET after delete");
    assert_eq!(resp.status().as_u16(), 404, "deleted session must be gone");

    let resp = client
        .get(format!("{}/api/v1/chat/sessions", base))
        .send()
        .await
        .expect("list after delete");
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["total"], 0, "list is empty after delete");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(unsafe_code)] // env isolation (AppState uses data_dir() for tantivy/vectors)
async fn session_routes_locked_vault_returns_403() {
    // No setup → vault sealed/locked. dek_db() fails in the handler → 403.
    let tmp = tempfile::TempDir::new().expect("tmp");
    unsafe {
        std::env::set_var("HOME", tmp.path());
        std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
        std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
    }
    let vault = Vault::open_memory(tmp.path()).expect("vault");
    let state = Arc::new(attune_server::state::AppState::new(vault, false));
    let router = attune_server::build_router(Arc::clone(&state));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap(); });
    let base = format!("http://127.0.0.1:{}", port);
    wait_for_server(&base).await;
    Box::leak(Box::new(tmp));

    let client = reqwest::Client::new();
    let acceptable = |s: u16| s == 401 || s == 403;
    let resp = client.get(format!("{}/api/v1/chat/sessions", base)).send().await.expect("list");
    assert!(acceptable(resp.status().as_u16()), "list on locked vault: got {}", resp.status());
    let resp = client.get(format!("{}/api/v1/chat/sessions/x", base)).send().await.expect("get");
    assert!(acceptable(resp.status().as_u16()), "get on locked vault: got {}", resp.status());
}

// ── Store-layer invariant (legitimately a store unit, not a route re-impl) ───
#[cfg(test)]
mod store_invariants {
    use attune_core::crypto::Key32;
    use attune_core::store::Store;

    /// Deleting a conversation must cascade-delete its messages. This is a store
    /// invariant the delete route depends on; kept as a direct store test.
    #[test]
    fn delete_conversation_cascades_messages() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let sid = store.create_conversation(&dek, "会话").unwrap();
        store.append_message(&dek, &sid, "user", "m1", &[]).unwrap();
        store.append_message(&dek, &sid, "assistant", "m2", &[]).unwrap();
        assert_eq!(store.get_conversation_messages(&dek, &sid).unwrap().len(), 2);

        store.delete_conversation(&sid).unwrap();
        assert!(store.get_conversation_by_id(&dek, &sid).unwrap().is_none());
        assert!(store.get_conversation_messages(&dek, &sid).unwrap().is_empty(), "messages cascade-deleted");
    }
}

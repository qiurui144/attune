# Test Coverage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复 Python 测试环境并补全 store.rs + vault-server 路由层测试覆盖
**Architecture:** 三阶段：(1) 快速修复 Python 环境 (2) store.rs 单元测试补全 (3) vault-server 集成测试框架
**Tech Stack:** pytest, Rust #[cfg(test)], axum::test, tempfile, tokio::test
---

## 背景

评估发现三类测试覆盖缺口：
- **Q1**：`npu-vault/crates/vault-server/src/routes/` 下 18 个路由文件（42 个 handler）零测试
- **Q2**：Python `tests/conftest.py` 中 `from npu_webhook.app_state import state` 因 `PYTHONPATH` 缺失导致 `ModuleNotFoundError`
- **Q3**：`vault-core/src/store.rs` 中 12 个 `pub` 函数无测试：`bind_directory`、`unbind_directory`、`list_bound_directories`、`update_dir_last_scan`、`get_indexed_file`、`upsert_indexed_file`、`enqueue_embedding`、`dequeue_embeddings`、`mark_embedding_done`、`mark_embedding_failed`、`mark_task_pending`、`checkpoint`

---

## Task 1：修复 Python 测试环境（Q2）

**文件**：`pytest.ini`（项目根目录 `/data/company/project/npu-webhook/`）

- [ ] 在项目根目录创建 `pytest.ini`，内容如下：

```ini
[pytest]
pythonpath = src
testpaths = tests
```

- [ ] 验证修复：在 venv 中执行 `python3 -m pytest tests/ --collect-only`，确认所有测试用例正常收集，无 `ModuleNotFoundError`

**预期结果**：`collected N items` 正常输出，无 import 错误

---

## Task 2：store.rs 目录绑定函数测试（Q3 第一批）

**文件**：`npu-vault/crates/vault-core/src/store.rs`（在文件末尾 `#[cfg(test)]` 模块中追加）

**测试函数签名依据**：
- `bind_directory(&self, path: &str, recursive: bool, file_types: &[&str]) -> Result<String>`
- `unbind_directory(&self, dir_id: &str) -> Result<()>`
- `list_bound_directories(&self) -> Result<Vec<BoundDirRow>>`
- `update_dir_last_scan(&self, dir_id: &str) -> Result<()>`

- [ ] 在 `store.rs` 末尾追加以下测试模块（若已有 `#[cfg(test)]` 则在其中追加）：

```rust
#[cfg(test)]
mod tests_dir {
    use super::*;
    use vault_core::vault::Vault;
    use tempfile::TempDir;

    fn open_vault() -> (Vault, TempDir) {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("vault.db");
        let config_dir = tmp.path().join("config");
        let vault = Vault::open(&db_path, &config_dir).unwrap();
        vault.setup("pw").unwrap();
        (vault, tmp)
    }

    #[test]
    fn test_bind_directory_returns_id() {
        let (vault, _tmp) = open_vault();
        let id = vault.store().bind_directory("/tmp/docs", true, &["md", "txt"]).unwrap();
        assert!(!id.is_empty(), "bind_directory should return a non-empty id");
    }

    #[test]
    fn test_list_bound_directories_after_bind() {
        let (vault, _tmp) = open_vault();
        vault.store().bind_directory("/tmp/docs", true, &["md"]).unwrap();
        let dirs = vault.store().list_bound_directories().unwrap();
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0].path, "/tmp/docs");
        assert!(dirs[0].recursive);
    }

    #[test]
    fn test_bind_multiple_directories() {
        let (vault, _tmp) = open_vault();
        vault.store().bind_directory("/tmp/a", false, &["txt"]).unwrap();
        vault.store().bind_directory("/tmp/b", true, &["md", "rs"]).unwrap();
        let dirs = vault.store().list_bound_directories().unwrap();
        assert_eq!(dirs.len(), 2);
    }

    #[test]
    fn test_unbind_directory_marks_inactive() {
        let (vault, _tmp) = open_vault();
        let id = vault.store().bind_directory("/tmp/docs", true, &["md"]).unwrap();
        vault.store().unbind_directory(&id).unwrap();
        let dirs = vault.store().list_bound_directories().unwrap();
        assert_eq!(dirs.len(), 0, "unbind should mark directory inactive");
    }

    #[test]
    fn test_unbind_nonexistent_returns_not_found() {
        let (vault, _tmp) = open_vault();
        let result = vault.store().unbind_directory("nonexistent-id");
        assert!(result.is_err(), "unbind nonexistent dir_id should return NotFound error");
    }

    #[test]
    fn test_update_dir_last_scan() {
        let (vault, _tmp) = open_vault();
        let id = vault.store().bind_directory("/tmp/docs", false, &["md"]).unwrap();
        // last_scan starts as None; after update it should be Some
        vault.store().update_dir_last_scan(&id).unwrap();
        let dirs = vault.store().list_bound_directories().unwrap();
        assert_eq!(dirs.len(), 1);
        assert!(dirs[0].last_scan.is_some(), "last_scan should be set after update_dir_last_scan");
    }
}
```

- [ ] 执行 `cargo test -p vault-core tests_dir` 确认全部通过

---

## Task 3：store.rs 索引文件函数测试（Q3 第二批）

**文件**：`npu-vault/crates/vault-core/src/store.rs`（同一 `#[cfg(test)]` 模块，追加独立模块）

**测试函数签名依据**：
- `get_indexed_file(&self, path: &str) -> Result<Option<IndexedFileRow>>`
- `upsert_indexed_file(&self, dir_id: &str, path: &str, file_hash: &str, item_id: &str) -> Result<()>`

- [ ] 追加以下测试模块：

```rust
#[cfg(test)]
mod tests_indexed_files {
    use super::*;
    use vault_core::vault::Vault;
    use tempfile::TempDir;

    fn open_vault() -> (Vault, TempDir) {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("vault.db");
        let config_dir = tmp.path().join("config");
        let vault = Vault::open(&db_path, &config_dir).unwrap();
        vault.setup("pw").unwrap();
        (vault, tmp)
    }

    fn insert_test_item(vault: &Vault) -> String {
        let dek = vault.dek_db().unwrap();
        vault.store()
            .insert_item(&dek, "test title", "test content", None, "note", None, None)
            .unwrap()
    }

    #[test]
    fn test_get_indexed_file_returns_none_for_unknown_path() {
        let (vault, _tmp) = open_vault();
        let result = vault.store().get_indexed_file("/nonexistent/path.md").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_upsert_indexed_file_insert() {
        let (vault, _tmp) = open_vault();
        let dir_id = vault.store().bind_directory("/tmp/docs", false, &["md"]).unwrap();
        let item_id = insert_test_item(&vault);
        vault.store()
            .upsert_indexed_file(&dir_id, "/tmp/docs/note.md", "abc123", &item_id)
            .unwrap();
        let row = vault.store().get_indexed_file("/tmp/docs/note.md").unwrap();
        assert!(row.is_some());
        let row = row.unwrap();
        assert_eq!(row.path, "/tmp/docs/note.md");
        assert_eq!(row.file_hash, "abc123");
        assert_eq!(row.item_id, item_id);
        assert_eq!(row.dir_id, dir_id);
    }

    #[test]
    fn test_upsert_indexed_file_update_existing() {
        let (vault, _tmp) = open_vault();
        let dir_id = vault.store().bind_directory("/tmp/docs", false, &["md"]).unwrap();
        let item_id = insert_test_item(&vault);
        vault.store()
            .upsert_indexed_file(&dir_id, "/tmp/docs/note.md", "hash_v1", &item_id)
            .unwrap();
        // Upsert again with a new hash
        vault.store()
            .upsert_indexed_file(&dir_id, "/tmp/docs/note.md", "hash_v2", &item_id)
            .unwrap();
        let row = vault.store().get_indexed_file("/tmp/docs/note.md").unwrap().unwrap();
        assert_eq!(row.file_hash, "hash_v2", "upsert should update existing file_hash");
    }
}
```

- [ ] 执行 `cargo test -p vault-core tests_indexed_files` 确认全部通过

---

## Task 4：store.rs embedding 队列函数测试（Q3 第三批）

**文件**：`npu-vault/crates/vault-core/src/store.rs`（追加模块）

**测试函数签名依据**：
- `enqueue_embedding(&self, item_id: &str, chunk_idx: usize, chunk_text: &str, priority: i32, level: i32, section_idx: usize) -> Result<()>`
- `dequeue_embeddings(&self, batch_size: usize) -> Result<Vec<QueueTask>>`
- `mark_embedding_done(&self, id: i64) -> Result<()>`
- `mark_embedding_failed(&self, id: i64, max_attempts: i32) -> Result<()>`
- `mark_task_pending(&self, id: i64) -> Result<()>`
- `checkpoint(&self) -> Result<()>`

- [ ] 追加以下测试模块：

```rust
#[cfg(test)]
mod tests_embed_queue {
    use super::*;
    use vault_core::vault::Vault;
    use tempfile::TempDir;

    fn open_vault() -> (Vault, TempDir) {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("vault.db");
        let config_dir = tmp.path().join("config");
        let vault = Vault::open(&db_path, &config_dir).unwrap();
        vault.setup("pw").unwrap();
        (vault, tmp)
    }

    fn insert_test_item(vault: &Vault) -> String {
        let dek = vault.dek_db().unwrap();
        vault.store()
            .insert_item(&dek, "title", "content", None, "note", None, None)
            .unwrap()
    }

    #[test]
    fn test_enqueue_embedding_adds_to_queue() {
        let (vault, _tmp) = open_vault();
        let item_id = insert_test_item(&vault);
        vault.store()
            .enqueue_embedding(&item_id, 0, "chunk text", 1, 1, 0)
            .unwrap();
        assert_eq!(vault.store().pending_embedding_count().unwrap(), 1);
    }

    #[test]
    fn test_dequeue_embeddings_returns_tasks_and_marks_processing() {
        let (vault, _tmp) = open_vault();
        let item_id = insert_test_item(&vault);
        vault.store()
            .enqueue_embedding(&item_id, 0, "chunk A", 1, 1, 0)
            .unwrap();
        vault.store()
            .enqueue_embedding(&item_id, 1, "chunk B", 1, 2, 0)
            .unwrap();
        let tasks = vault.store().dequeue_embeddings(10).unwrap();
        assert_eq!(tasks.len(), 2);
        // After dequeue, pending count should be 0 (tasks moved to processing)
        assert_eq!(vault.store().pending_embedding_count().unwrap(), 0);
    }

    #[test]
    fn test_dequeue_respects_batch_size() {
        let (vault, _tmp) = open_vault();
        let item_id = insert_test_item(&vault);
        for i in 0..5 {
            vault.store()
                .enqueue_embedding(&item_id, i, &format!("chunk {i}"), 1, 1, 0)
                .unwrap();
        }
        let tasks = vault.store().dequeue_embeddings(3).unwrap();
        assert_eq!(tasks.len(), 3);
        // Remaining 2 still pending
        assert_eq!(vault.store().pending_embedding_count().unwrap(), 2);
    }

    #[test]
    fn test_mark_embedding_done_removes_from_active() {
        let (vault, _tmp) = open_vault();
        let item_id = insert_test_item(&vault);
        vault.store()
            .enqueue_embedding(&item_id, 0, "chunk", 1, 1, 0)
            .unwrap();
        let tasks = vault.store().dequeue_embeddings(1).unwrap();
        assert_eq!(tasks.len(), 1);
        vault.store().mark_embedding_done(tasks[0].id).unwrap();
        // Pending still 0, and done task won't re-appear in dequeue
        let re_tasks = vault.store().dequeue_embeddings(10).unwrap();
        assert_eq!(re_tasks.len(), 0, "done task should not be dequeued again");
    }

    #[test]
    fn test_mark_embedding_failed_retries_within_max_attempts() {
        let (vault, _tmp) = open_vault();
        let item_id = insert_test_item(&vault);
        vault.store()
            .enqueue_embedding(&item_id, 0, "chunk", 1, 1, 0)
            .unwrap();
        let tasks = vault.store().dequeue_embeddings(1).unwrap();
        let task_id = tasks[0].id;
        // Fail once (attempts=1 < max_attempts=3) → should go back to pending
        vault.store().mark_embedding_failed(task_id, 3).unwrap();
        assert_eq!(vault.store().pending_embedding_count().unwrap(), 1);
    }

    #[test]
    fn test_mark_embedding_failed_abandons_after_max_attempts() {
        let (vault, _tmp) = open_vault();
        let item_id = insert_test_item(&vault);
        vault.store()
            .enqueue_embedding(&item_id, 0, "chunk", 1, 1, 0)
            .unwrap();
        // Fail 3 times with max_attempts=3 to reach abandoned state
        for _ in 0..3 {
            let tasks = vault.store().dequeue_embeddings(1).unwrap();
            if tasks.is_empty() {
                // Task was abandoned, no longer pending
                break;
            }
            vault.store().mark_embedding_failed(tasks[0].id, 3).unwrap();
        }
        // After max_attempts failures, task should be abandoned (not pending)
        assert_eq!(vault.store().pending_embedding_count().unwrap(), 0);
    }

    #[test]
    fn test_mark_task_pending_restores_processing_task() {
        let (vault, _tmp) = open_vault();
        let item_id = insert_test_item(&vault);
        vault.store()
            .enqueue_embedding(&item_id, 0, "chunk", 1, 1, 0)
            .unwrap();
        let tasks = vault.store().dequeue_embeddings(1).unwrap();
        assert_eq!(vault.store().pending_embedding_count().unwrap(), 0);
        // Restore task to pending
        vault.store().mark_task_pending(tasks[0].id).unwrap();
        assert_eq!(vault.store().pending_embedding_count().unwrap(), 1);
    }

    #[test]
    fn test_checkpoint_does_not_error() {
        let (vault, _tmp) = open_vault();
        // checkpoint flushes WAL; should not error on a freshly opened store
        vault.store().checkpoint().unwrap();
    }

    #[test]
    fn test_enqueue_embedding_chunk_text_preserved() {
        let (vault, _tmp) = open_vault();
        let item_id = insert_test_item(&vault);
        let text = "这是一段中文测试文本，包含特殊字符：🔑";
        vault.store()
            .enqueue_embedding(&item_id, 0, text, 1, 1, 0)
            .unwrap();
        let tasks = vault.store().dequeue_embeddings(1).unwrap();
        assert_eq!(tasks[0].chunk_text, text, "chunk_text should be preserved through enqueue/dequeue");
    }
}
```

- [ ] 执行 `cargo test -p vault-core tests_embed_queue` 确认全部通过

---

## Task 5：vault-server 集成测试框架（Q1 基础）

**文件**：`npu-vault/crates/vault-server/src/routes/` 内的路由文件（使用 `#[cfg(test)]` 内联测试）或 `npu-vault/tests/server_test.rs`

**思路**：vault-server 依赖 `AppState`，其构建需要 `Vault`。测试中使用 `tempfile::TempDir` + `Vault::open()` 构造真实状态，通过 `axum::Router` 直接调用 handler（不启动 TCP 监听）。

**前置条件**：在 `npu-vault/Cargo.toml` 的 `[dev-dependencies]` 中追加：
```toml
vault-server = { path = "crates/vault-server" }
tokio = { version = "1", features = ["full"] }
axum = { version = "0.8", features = ["json"] }
tower = { version = "0.5", features = ["util"] }
http-body-util = "0.1"
```

- [ ] 检查 `npu-vault/Cargo.toml` `[dev-dependencies]` 是否已有上述依赖，如缺少则补充

- [ ] 在 `npu-vault/tests/` 目录新建 `server_test.rs`，内容如下：

```rust
//! vault-server 路由层集成测试
//! 直接构造 axum Router + AppState，不启动 TCP 监听

use std::sync::{Arc, Mutex};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt; // for oneshot()
use vault_core::vault::Vault;
use vault_server::state::AppState;

/// 构造一个使用临时目录的 AppState（vault 处于 Sealed 状态）
fn make_sealed_state() -> (Arc<AppState>, TempDir) {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("vault.db");
    let config_dir = tmp.path().join("config");
    let vault = Vault::open(&db_path, &config_dir).unwrap();
    let state = Arc::new(AppState::new(vault, false));
    (state, tmp)
}

/// 构造已 setup（unlocked）的 AppState
fn make_unlocked_state() -> (Arc<AppState>, TempDir) {
    let (state, tmp) = make_sealed_state();
    {
        let vault = state.vault.lock().unwrap();
        vault.setup("test-password").unwrap();
    }
    (state, tmp)
}

/// 构建完整路由表（参照 vault-server/src/main.rs 的路由注册）
fn build_router(state: Arc<AppState>) -> Router {
    vault_server::build_router(state)
}

/// 发送 JSON POST 请求的辅助函数
async fn post_json(router: &Router, uri: &str, body: serde_json::Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

/// 发送 GET 请求的辅助函数
async fn get(router: &Router, uri: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

// ─── /vault/status ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_vault_status_sealed() {
    let (state, _tmp) = make_sealed_state();
    let router = build_router(state);
    let (status, body) = get(&router, "/api/v1/vault/status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["state"], "Sealed");
}

#[tokio::test]
async fn test_vault_status_unlocked() {
    let (state, _tmp) = make_unlocked_state();
    let router = build_router(state);
    let (status, body) = get(&router, "/api/v1/vault/status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["state"], "Unlocked");
}

// ─── /vault/setup ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_vault_setup_success() {
    let (state, _tmp) = make_sealed_state();
    let router = build_router(state);
    let (status, body) = post_json(
        &router,
        "/api/v1/vault/setup",
        serde_json::json!({"password": "my-password"}),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["state"], "unlocked");
}

#[tokio::test]
async fn test_vault_setup_already_initialized_returns_error() {
    let (state, _tmp) = make_unlocked_state();
    let router = build_router(state);
    // Setup on an already-initialized vault should return 400
    let (status, _) = post_json(
        &router,
        "/api/v1/vault/setup",
        serde_json::json!({"password": "another-password"}),
    ).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ─── /vault/lock + /vault/unlock ─────────────────────────────────────────────

#[tokio::test]
async fn test_vault_lock_and_unlock() {
    let (state, _tmp) = make_unlocked_state();
    let router = build_router(state);

    // Lock
    let (lock_status, lock_body) = post_json(
        &router,
        "/api/v1/vault/lock",
        serde_json::json!({}),
    ).await;
    assert_eq!(lock_status, StatusCode::OK);
    assert_eq!(lock_body["state"], "locked");

    // Unlock with correct password
    let (unlock_status, unlock_body) = post_json(
        &router,
        "/api/v1/vault/unlock",
        serde_json::json!({"password": "test-password"}),
    ).await;
    assert_eq!(unlock_status, StatusCode::OK);
    assert!(unlock_body["token"].as_str().map(|s| !s.is_empty()).unwrap_or(false));
}

#[tokio::test]
async fn test_vault_unlock_wrong_password_returns_401() {
    let (state, _tmp) = {
        let (state, tmp) = make_unlocked_state();
        // Lock first
        {
            let vault = state.vault.lock().unwrap();
            vault.lock().unwrap();
        }
        (state, tmp)
    };
    let router = build_router(state);
    let (status, _) = post_json(
        &router,
        "/api/v1/vault/unlock",
        serde_json::json!({"password": "wrong-password"}),
    ).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
```

- [ ] 在 `npu-vault/Cargo.toml` `[[test]]` 段（如不存在则追加）注册新测试文件：

```toml
[[test]]
name = "server_test"
path = "tests/server_test.rs"
```

- [ ] 执行 `cargo test --test server_test` 确认通过（若 `build_router` 未导出则参照 Task 5 修复说明）

**Task 5 修复说明**：若 `vault_server::build_router` 未公开导出，需在 `npu-vault/crates/vault-server/src/lib.rs`（若无则新建）中添加：

```rust
pub mod state;
pub use router::build_router;
```

并在 `main.rs` 中将路由构建逻辑提取为独立函数 `pub fn build_router(state: SharedState) -> axum::Router`。

---

## Task 6：vault-server 核心业务路由测试（Q1 续）

**文件**：`npu-vault/tests/server_test.rs`（续 Task 5，追加以下测试函数）

- [ ] 在 `server_test.rs` 末尾追加以下测试：

```rust
// ─── /api/v1/ingest ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_ingest_success_when_unlocked() {
    let (state, _tmp) = make_unlocked_state();
    let router = build_router(state);
    let (status, body) = post_json(
        &router,
        "/api/v1/ingest",
        serde_json::json!({
            "title": "测试文章",
            "content": "这是一篇测试内容，用于验证 ingest 接口正常工作。",
            "source_type": "note"
        }),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert!(body["id"].as_str().map(|s| !s.is_empty()).unwrap_or(false), "should return a non-empty id");
}

#[tokio::test]
async fn test_ingest_returns_403_when_locked() {
    let (state, _tmp) = {
        let (state, tmp) = make_unlocked_state();
        {
            let vault = state.vault.lock().unwrap();
            vault.lock().unwrap();
        }
        (state, tmp)
    };
    let router = build_router(state);
    let (status, _) = post_json(
        &router,
        "/api/v1/ingest",
        serde_json::json!({
            "title": "should fail",
            "content": "locked vault"
        }),
    ).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ─── /api/v1/search ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_search_empty_vault_returns_empty_results() {
    let (state, _tmp) = make_unlocked_state();
    // 注意：search 依赖 fulltext / vector 引擎，init_search_engines 未调用时应优雅降级
    let router = build_router(state);
    let (status, body) = post_json(
        &router,
        "/api/v1/search",
        serde_json::json!({"query": "test query", "limit": 10}),
    ).await;
    // Should return 200 with empty results or 403 (not 500)
    assert!(
        status == StatusCode::OK || status == StatusCode::FORBIDDEN,
        "search on empty vault should return 200 or 403, got {status}"
    );
    if status == StatusCode::OK {
        let items = body["results"].as_array().map(|a| a.len()).unwrap_or(0);
        assert_eq!(items, 0, "empty vault should return 0 results");
    }
}

#[tokio::test]
async fn test_search_returns_403_when_locked() {
    let (state, _tmp) = {
        let (state, tmp) = make_unlocked_state();
        {
            let vault = state.vault.lock().unwrap();
            vault.lock().unwrap();
        }
        (state, tmp)
    };
    let router = build_router(state);
    let (status, _) = post_json(
        &router,
        "/api/v1/search",
        serde_json::json!({"query": "test", "limit": 5}),
    ).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ─── /api/v1/items ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_list_items_empty_vault() {
    let (state, _tmp) = make_unlocked_state();
    let router = build_router(state);
    let (status, body) = get(&router, "/api/v1/items").await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().expect("items field should be array");
    assert_eq!(items.len(), 0);
}

#[tokio::test]
async fn test_list_items_after_ingest() {
    let (state, _tmp) = make_unlocked_state();
    let router = build_router(state);

    // Ingest one item
    post_json(
        &router,
        "/api/v1/ingest",
        serde_json::json!({
            "title": "My Note",
            "content": "Some content here",
            "source_type": "note"
        }),
    ).await;

    // List should return 1 item
    let (status, body) = get(&router, "/api/v1/items").await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().expect("items field should be array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["title"], "My Note");
}

#[tokio::test]
async fn test_get_item_not_found_returns_404() {
    let (state, _tmp) = make_unlocked_state();
    let router = build_router(state);
    let (status, _) = get(&router, "/api/v1/items/nonexistent-id").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_get_item_by_id() {
    let (state, _tmp) = make_unlocked_state();
    let router = build_router(state);

    // Ingest
    let (_, ingest_body) = post_json(
        &router,
        "/api/v1/ingest",
        serde_json::json!({
            "title": "Specific Item",
            "content": "Specific content",
            "source_type": "note"
        }),
    ).await;
    let item_id = ingest_body["id"].as_str().unwrap().to_string();

    // Get by id
    let (status, body) = get(&router, &format!("/api/v1/items/{item_id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["title"], "Specific Item");
    assert_eq!(body["content"], "Specific content");
}

#[tokio::test]
async fn test_delete_item_success() {
    let (state, _tmp) = make_unlocked_state();
    let router = build_router(state);

    // Ingest
    let (_, ingest_body) = post_json(
        &router,
        "/api/v1/ingest",
        serde_json::json!({
            "title": "To Delete",
            "content": "content",
            "source_type": "note"
        }),
    ).await;
    let item_id = ingest_body["id"].as_str().unwrap().to_string();

    // Delete
    let delete_req = Request::builder()
        .method("DELETE")
        .uri(&format!("/api/v1/items/{item_id}"))
        .body(Body::empty())
        .unwrap();
    let delete_resp = router.clone().oneshot(delete_req).await.unwrap();
    assert_eq!(delete_resp.status(), StatusCode::OK);

    // Verify gone
    let (get_status, _) = get(&router, &format!("/api/v1/items/{item_id}")).await;
    assert_eq!(get_status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_list_items_returns_403_when_locked() {
    let (state, _tmp) = {
        let (state, tmp) = make_unlocked_state();
        {
            let vault = state.vault.lock().unwrap();
            vault.lock().unwrap();
        }
        (state, tmp)
    };
    let router = build_router(state);
    let (status, _) = get(&router, "/api/v1/items").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
```

- [ ] 执行 `cargo test --test server_test` 全量确认通过

---

## 完成标准

全部 6 个 Task 完成后，以下指标必须满足：

| 指标 | 目标 |
|------|------|
| Python pytest 收集 | `collected N items`，无 import 错误 |
| store.rs 测试覆盖 | 12 个 pub 函数各有至少 1 个对应测试 |
| vault-server 路由测试 | 核心路由（/vault/\*、/ingest、/search、/items）有集成测试 |
| `cargo test -p vault-core` | 全部通过 |
| `cargo test --test server_test` | 全部通过 |
| `python3 -m pytest tests/` | 全部通过 |

---

## 自检结果

1. **占位符扫描**：无 "similar to above" / "..." / TODO 占位符，所有测试函数均有完整实现
2. **12 个 store.rs 函数覆盖**：
   - `bind_directory` → Task 2: `test_bind_directory_returns_id`, `test_list_bound_directories_after_bind`, `test_bind_multiple_directories`
   - `unbind_directory` → Task 2: `test_unbind_directory_marks_inactive`, `test_unbind_nonexistent_returns_not_found`
   - `list_bound_directories` → Task 2: `test_list_bound_directories_after_bind`, `test_bind_multiple_directories`
   - `update_dir_last_scan` → Task 2: `test_update_dir_last_scan`
   - `get_indexed_file` → Task 3: `test_get_indexed_file_returns_none_for_unknown_path`, `test_upsert_indexed_file_insert`
   - `upsert_indexed_file` → Task 3: `test_upsert_indexed_file_insert`, `test_upsert_indexed_file_update_existing`
   - `enqueue_embedding` → Task 4: `test_enqueue_embedding_adds_to_queue`, `test_enqueue_embedding_chunk_text_preserved`
   - `dequeue_embeddings` → Task 4: `test_dequeue_embeddings_returns_tasks_and_marks_processing`, `test_dequeue_respects_batch_size`
   - `mark_embedding_done` → Task 4: `test_mark_embedding_done_removes_from_active`
   - `mark_embedding_failed` → Task 4: `test_mark_embedding_failed_retries_within_max_attempts`, `test_mark_embedding_failed_abandons_after_max_attempts`
   - `mark_task_pending` → Task 4: `test_mark_task_pending_restores_processing_task`
   - `checkpoint` → Task 4: `test_checkpoint_does_not_error`
3. **vault-server 编译路径**：所有 import 均基于 `vault_server::` crate，需确保 Task 5 中 `build_router` 公开导出
4. **pytest.ini 路径**：`/data/company/project/npu-webhook/pytest.ini`（项目根目录，与 `src/` 同级）

# QueueWorker 自动启动 + WebSocket 扫描进度 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复 F5（vault unlock 后 QueueWorker 未自动启动导致向量搜索不工作）和 S4（缺少 WebSocket 进度端点），让 embedding 队列在 unlock 后自动消费，并向 Web UI 实时推送队列进度。

**Architecture:** Task 1 在 `AppState` 中新增 `start_queue_worker()` 静态方法，在 `vault_setup` / `vault_unlock` 路由中与现有 `start_classify_worker` 并排调用，复用相同的 VaultState 守卫模式（vault lock 时自动停止）。Task 2 在 axum 中启用 `ws` feature，新增 `routes/ws.rs` WebSocket handler，每 2 秒向客户端推送 `{pending_embeddings, pending_classify, bound_dirs}` JSON；同时在 `store.rs` 新增 `pending_count_by_type()` 辅助方法，并在 Web UI 的状态页嵌入实时进度卡片。

**Tech Stack:** Rust 2021, axum 0.8 ws feature, tokio, serde_json, 复用现有 rusqlite/tracing

---

## File Structure Map

```
npu-vault/
├── crates/vault-core/
│   └── src/
│       └── store.rs                  [MODIFY: 新增 pending_count_by_type()]
│
├── crates/vault-server/
│   ├── Cargo.toml                    [MODIFY: axum features 加 "ws"]
│   └── src/
│       ├── state.rs                  [MODIFY: 新增 start_queue_worker()]
│       ├── main.rs                   [MODIFY: 注册 /ws/scan-progress 路由]
│       └── routes/
│           ├── mod.rs                [MODIFY: 导出 ws 子模块]
│           ├── vault.rs              [MODIFY: vault_setup / vault_unlock 调用 start_queue_worker]
│           ├── ws.rs                 [NEW: WebSocket handler]
│           └── ui.rs                 [MODIFY: 状态页嵌入实时进度卡片]
```

---

## Task 1：vault unlock 后自动启动 QueueWorker

**Files:**
- `npu-vault/crates/vault-core/src/store.rs` — 新增 `pending_count_by_type()`
- `npu-vault/crates/vault-server/src/state.rs` — 新增 `start_queue_worker()`
- `npu-vault/crates/vault-server/src/routes/vault.rs` — 调用 `start_queue_worker`

### 步骤

- [ ] **1.1 store.rs — 新增 `pending_count_by_type()`**

  在 `pending_embedding_count()` 之后添加，用于 WebSocket 分类计数（Task 2 复用）。

  ```rust
  // npu-vault/crates/vault-core/src/store.rs
  // 位置：pending_embedding_count() 之后

  /// 按 task_type 查询 pending 状态任务数量（用于进度推送）
  pub fn pending_count_by_type(&self, task_type: &str) -> Result<usize> {
      let count: i64 = self.conn.query_row(
          "SELECT COUNT(*) FROM embed_queue WHERE status = 'pending' AND task_type = ?1",
          [task_type],
          |row| row.get(0),
      )?;
      Ok(count as usize)
  }
  ```

- [ ] **1.2 state.rs — 新增 `start_queue_worker()`**

  在 `start_classify_worker()` 方法之后添加，签名与 `QueueWorker::start()` 完全匹配。

  `QueueWorker::start()` 实际签名（来自 `vault-core/src/queue.rs`）：
  ```
  pub fn start(
      &self,
      store: Arc<Mutex<Store>>,
      embedding: Arc<dyn EmbeddingProvider>,
      vectors: Arc<Mutex<VectorIndex>>,
      fulltext: Arc<Mutex<FulltextIndex>>,
  ) -> std::thread::JoinHandle<()>
  ```

  > **注意**：`AppState.vault` 持有 `Mutex<Vault>`，`Vault::store()` 返回 `&Store`（不是 `Arc<Mutex<Store>>`）。
  > 因此无法直接将 store 包进 `Arc<Mutex>`。实现绕过方式：使用与 `start_classify_worker` 完全一致的模式——直接在后台线程持有 `Arc<AppState>`，在循环内按需加锁读取状态，而非直接调用 `QueueWorker::start()`（后者用于需要独立 Arc store 的测试场景）。
  >
  > 额外守卫：若 `AppState` 中已有 worker 在运行（通过 `AtomicBool` 标志判断），跳过重复启动。

  在 `AppState` struct 中新增 `queue_worker_running` 字段：

  ```rust
  // npu-vault/crates/vault-server/src/state.rs
  // 在 use 块顶部新增
  use std::sync::atomic::{AtomicBool, Ordering};

  pub struct AppState {
      pub vault: Mutex<Vault>,
      pub fulltext: Mutex<Option<FulltextIndex>>,
      pub vectors: Mutex<Option<VectorIndex>>,
      pub embedding: Mutex<Option<Arc<dyn EmbeddingProvider>>>,
      pub llm: Mutex<Option<Arc<dyn LlmProvider>>>,
      pub tag_index: Mutex<Option<TagIndex>>,
      pub cluster_snapshot: Mutex<Option<ClusterSnapshot>>,
      pub taxonomy: Mutex<Option<Arc<Taxonomy>>>,
      pub classifier: Mutex<Option<Arc<Classifier>>>,
      pub require_auth: bool,
      /// 防止重复启动 QueueWorker 后台线程
      pub queue_worker_running: AtomicBool,
  }
  ```

  更新 `AppState::new()` 初始化：

  ```rust
  pub fn new(vault: Vault, require_auth: bool) -> Self {
      Self {
          vault: Mutex::new(vault),
          fulltext: Mutex::new(None),
          vectors: Mutex::new(None),
          embedding: Mutex::new(None),
          llm: Mutex::new(None),
          tag_index: Mutex::new(None),
          cluster_snapshot: Mutex::new(None),
          taxonomy: Mutex::new(None),
          classifier: Mutex::new(None),
          require_auth,
          queue_worker_running: AtomicBool::new(false),
      }
  }
  ```

  在 `start_classify_worker()` 之后新增 `start_queue_worker()`：

  ```rust
  /// 启动后台 embedding queue worker（在 init_search_engines 之后调用）
  /// 若 embedding provider 或 vector index 未就绪则跳过。
  /// 使用 AtomicBool 防止重复启动。
  pub fn start_queue_worker(state: std::sync::Arc<AppState>) {
      // 防止重复启动
      if state.queue_worker_running.compare_exchange(
          false, true, Ordering::SeqCst, Ordering::SeqCst
      ).is_err() {
          tracing::debug!("Queue worker already running, skipping");
          return;
      }

      std::thread::spawn(move || {
          tracing::info!("Queue worker started");
          const BATCH_SIZE: usize = 10;
          const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);
          const MAX_ATTEMPTS: i32 = 3;

          loop {
              // 检查 vault 是否仍处于 unlocked 状态
              let vault_unlocked = {
                  let vault = state.vault.lock().unwrap();
                  matches!(vault.state(), vault_core::vault::VaultState::Unlocked)
              };
              if !vault_unlocked {
                  break;
              }

              // 获取 embedding provider + vectors + fulltext 快照
              let embedding = state.embedding.lock().unwrap().clone();
              let vectors_opt = state.vectors.lock().unwrap().is_some();
              let fulltext_opt = state.fulltext.lock().unwrap().is_some();

              if embedding.is_none() || !vectors_opt || !fulltext_opt {
                  std::thread::sleep(POLL_INTERVAL);
                  continue;
              }
              let embedding = embedding.unwrap();

              if !embedding.is_available() {
                  std::thread::sleep(POLL_INTERVAL);
                  continue;
              }

              // 取一批 embed 类型任务
              let tasks_result = {
                  let vault = state.vault.lock().unwrap();
                  vault.store().dequeue_embeddings(BATCH_SIZE)
              };
              let tasks = match tasks_result {
                  Ok(t) => t,
                  Err(e) => {
                      tracing::warn!("Queue worker dequeue error: {}", e);
                      std::thread::sleep(POLL_INTERVAL);
                      continue;
                  }
              };

              if tasks.is_empty() {
                  std::thread::sleep(POLL_INTERVAL);
                  continue;
              }

              // 按 task_type 分区：embed 由本 worker 处理，其余回队
              let (embed_tasks, other_tasks): (Vec<_>, Vec<_>) =
                  tasks.into_iter().partition(|t| t.task_type == "embed");

              // 非 embed 任务（classify 等）回到 pending，由 classify worker 消费
              if !other_tasks.is_empty() {
                  let vault = state.vault.lock().unwrap();
                  for task in &other_tasks {
                      let _ = vault.store().mark_task_pending(task.id);
                  }
              }

              if embed_tasks.is_empty() {
                  continue;
              }

              // 批量 embed
              let texts: Vec<&str> = embed_tasks.iter().map(|t| t.chunk_text.as_str()).collect();
              let embeddings = match embedding.embed(&texts) {
                  Ok(e) => e,
                  Err(e) => {
                      tracing::warn!("Embedding failed: {}", e);
                      let vault = state.vault.lock().unwrap();
                      for task in &embed_tasks {
                          let _ = vault.store().mark_embedding_failed(task.id, MAX_ATTEMPTS);
                      }
                      std::thread::sleep(POLL_INTERVAL);
                      continue;
                  }
              };

              // 写入向量索引 + 全文索引 + 标记完成
              for (i, task) in embed_tasks.iter().enumerate() {
                  if i >= embeddings.len() {
                      break;
                  }
                  {
                      if let Ok(mut vecs) = state.vectors.lock() {
                          if let Some(ref mut vi) = *vecs {
                              let _ = vi.add(
                                  &embeddings[i],
                                  vault_core::vectors::VectorMeta {
                                      item_id: task.item_id.clone(),
                                      chunk_idx: task.chunk_idx as usize,
                                      level: task.level as u8,
                                      section_idx: task.section_idx as usize,
                                  },
                              );
                          }
                      }
                  }
                  if task.level == 1 {
                      if let Ok(ft_guard) = state.fulltext.lock() {
                          if let Some(ref ft) = *ft_guard {
                              let _ = ft.add_document(
                                  &task.item_id, "", &task.chunk_text, "file",
                              );
                          }
                      }
                  }
                  {
                      let vault = state.vault.lock().unwrap();
                      let _ = vault.store().mark_embedding_done(task.id);
                  }
              }

              tracing::debug!("Queue worker processed {} embed tasks", embed_tasks.len());
          }

          // Worker 退出时重置标志，允许下次 unlock 重新启动
          state.queue_worker_running.store(false, Ordering::SeqCst);
          tracing::info!("Queue worker stopped (vault locked or engines cleared)");
      });
  }
  ```

- [ ] **1.3 routes/vault.rs — 在 setup/unlock 中调用 `start_queue_worker`**

  修改 `vault_setup` 函数（在现有 `start_rescan_worker` 调用之后追加）：

  ```rust
  // npu-vault/crates/vault-server/src/routes/vault.rs
  // vault_setup 函数体，原有代码：
  //   state.init_search_engines();
  //   crate::state::AppState::start_classify_worker(state.clone());
  //   crate::state::AppState::start_rescan_worker(state.clone());
  // 修改后：
  state.init_search_engines();
  crate::state::AppState::start_classify_worker(state.clone());
  crate::state::AppState::start_rescan_worker(state.clone());
  crate::state::AppState::start_queue_worker(state.clone());
  ```

  同样修改 `vault_unlock` 函数体：

  ```rust
  // vault_unlock 函数体，原有代码：
  //   state.init_search_engines();
  //   crate::state::AppState::start_classify_worker(state.clone());
  //   crate::state::AppState::start_rescan_worker(state.clone());
  // 修改后：
  state.init_search_engines();
  crate::state::AppState::start_classify_worker(state.clone());
  crate::state::AppState::start_rescan_worker(state.clone());
  crate::state::AppState::start_queue_worker(state.clone());
  ```

- [ ] **1.4 验证编译**

  ```bash
  cd /data/company/project/npu-webhook/npu-vault
  cargo build -p vault-server 2>&1 | tail -20
  ```

- [ ] **1.5 Commit**

  ```
  feat(vault-server): auto-start QueueWorker on vault unlock (fix F5)
  ```

---

## Task 2：WebSocket `/ws/scan-progress` 端点

**Files:**
- `npu-vault/crates/vault-server/Cargo.toml` — axum 加 `ws` feature
- `npu-vault/crates/vault-server/src/routes/ws.rs` — 新建 WebSocket handler
- `npu-vault/crates/vault-server/src/routes/mod.rs` — 导出 ws 模块
- `npu-vault/crates/vault-server/src/main.rs` — 注册路由
- `npu-vault/crates/vault-server/src/routes/ui.rs` — 状态页 HTML 添加进度卡片

### 步骤

- [ ] **2.1 Cargo.toml — 启用 axum ws feature**

  ```toml
  # npu-vault/crates/vault-server/Cargo.toml
  # 修改前：
  axum = { version = "0.8", features = ["json", "multipart"] }
  # 修改后：
  axum = { version = "0.8", features = ["json", "multipart", "ws"] }
  ```

- [ ] **2.2 新建 `routes/ws.rs`**

  ```rust
  // npu-vault/crates/vault-server/src/routes/ws.rs

  use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
  use axum::extract::State;
  use axum::response::Response;
  use vault_core::vault::VaultState;

  use crate::state::SharedState;

  /// GET /ws/scan-progress
  /// 客户端连接后每 2 秒推送一次队列进度 JSON，断开时退出循环。
  pub async fn scan_progress(
      ws: WebSocketUpgrade,
      State(state): State<SharedState>,
  ) -> Response {
      ws.on_upgrade(|socket| handle_scan_progress(socket, state))
  }

  async fn handle_scan_progress(mut socket: WebSocket, state: SharedState) {
      let interval = std::time::Duration::from_secs(2);

      loop {
          // 采集进度数据（均在 spawn_blocking 外直接锁，因为 Mutex 非 async-safe 但耗时极短）
          let payload = {
              let vault_guard = state.vault.lock().unwrap();
              let vault_state = vault_guard.state();

              if !matches!(vault_state, VaultState::Unlocked) {
                  // vault 已锁定，推送空进度后关闭连接
                  serde_json::json!({
                      "vault_state": "locked",
                      "pending_embeddings": 0,
                      "pending_classify": 0,
                      "bound_dirs": 0,
                  })
              } else {
                  let pending_embed = vault_guard
                      .store()
                      .pending_count_by_type("embed")
                      .unwrap_or(0);
                  let pending_classify = vault_guard
                      .store()
                      .pending_count_by_type("classify")
                      .unwrap_or(0);
                  let bound_dirs = vault_guard
                      .store()
                      .list_bound_directories()
                      .map(|v| v.len())
                      .unwrap_or(0);
                  serde_json::json!({
                      "vault_state": "unlocked",
                      "pending_embeddings": pending_embed,
                      "pending_classify": pending_classify,
                      "bound_dirs": bound_dirs,
                  })
              }
          };

          let text = payload.to_string();

          // 发送；若对端已断开则退出
          if socket.send(Message::Text(text.into())).await.is_err() {
              break;
          }

          tokio::time::sleep(interval).await;
      }
  }
  ```

- [ ] **2.3 routes/mod.rs — 导出 ws 模块**

  在 `mod.rs` 中追加：

  ```rust
  // npu-vault/crates/vault-server/src/routes/mod.rs
  // 在现有 pub mod 列表末尾追加：
  pub mod ws;
  ```

- [ ] **2.4 main.rs — 注册路由**

  在 `Router::new()` 链中，紧接 `/api/v1/status/health` 之后添加（放在不需要 vault_guard 的一组）：

  ```rust
  // npu-vault/crates/vault-server/src/main.rs
  // 在 .route("/api/v1/status/health", ...) 之后追加：
  .route("/ws/scan-progress", get(routes::ws::scan_progress))
  ```

  > **注意**：`/ws/scan-progress` 放在 `vault_guard` 中间件之前（在路由链末尾 `.layer(...)` 之前），保证未 unlock 时也能连接并收到 `vault_state: "locked"` 消息，方便 UI 轮询。

- [ ] **2.5 ui.rs — 状态页添加实时进度卡片**

  在嵌入式 HTML 的状态标签页（`id="tab-status"` 或等效容器）内追加进度卡片 + WebSocket 脚本。

  > `ui.rs` 使用 `include_str!` 嵌入静态 HTML 或直接硬编码 HTML 字符串，需定位后修改。
  > 以下代码片段演示在状态页 `<div id="status-content">` 前追加的内容：

  HTML 片段（嵌入到 status 标签页 HTML 中）：

  ```html
  <!-- 队列实时进度卡片 -->
  <div id="scan-progress-card" style="border:1px solid #e2e8f0;border-radius:8px;padding:16px;margin-bottom:16px;">
    <h3 style="margin:0 0 12px;font-size:14px;font-weight:600;color:#374151;">后台处理进度</h3>
    <div style="display:grid;grid-template-columns:repeat(3,1fr);gap:12px;">
      <div style="text-align:center;">
        <div id="prog-embed" style="font-size:24px;font-weight:700;color:#3b82f6;">—</div>
        <div style="font-size:12px;color:#6b7280;margin-top:4px;">待 Embedding</div>
      </div>
      <div style="text-align:center;">
        <div id="prog-classify" style="font-size:24px;font-weight:700;color:#8b5cf6;">—</div>
        <div style="font-size:12px;color:#6b7280;margin-top:4px;">待分类</div>
      </div>
      <div style="text-align:center;">
        <div id="prog-dirs" style="font-size:24px;font-weight:700;color:#10b981;">—</div>
        <div style="font-size:12px;color:#6b7280;margin-top:4px;">绑定目录</div>
      </div>
    </div>
    <div id="prog-status" style="margin-top:8px;font-size:12px;color:#9ca3af;text-align:center;">连接中...</div>
  </div>
  ```

  JavaScript 片段（嵌入到同一 HTML 的 `<script>` 段）：

  ```javascript
  (function initScanProgressWs() {
    let ws = null;
    let retryTimer = null;

    function connect() {
      const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
      ws = new WebSocket(proto + '//' + location.host + '/ws/scan-progress');

      ws.onmessage = function(evt) {
        try {
          const d = JSON.parse(evt.data);
          document.getElementById('prog-embed').textContent =
            d.vault_state === 'locked' ? '—' : d.pending_embeddings;
          document.getElementById('prog-classify').textContent =
            d.vault_state === 'locked' ? '—' : d.pending_classify;
          document.getElementById('prog-dirs').textContent =
            d.vault_state === 'locked' ? '—' : d.bound_dirs;
          document.getElementById('prog-status').textContent =
            d.vault_state === 'locked'
              ? 'Vault 已锁定'
              : (d.pending_embeddings + d.pending_classify === 0 ? '空闲' : '处理中...');
        } catch(e) { /* ignore */ }
      };

      ws.onclose = function() {
        document.getElementById('prog-status').textContent = '已断开，3 秒后重连…';
        retryTimer = setTimeout(connect, 3000);
      };

      ws.onerror = function() { ws.close(); };
    }

    // 仅在状态标签页可见时启动（避免在其他页面消耗连接）
    connect();

    // 页面卸载时清理
    window.addEventListener('beforeunload', function() {
      clearTimeout(retryTimer);
      if (ws) ws.close();
    });
  })();
  ```

- [ ] **2.6 验证编译**

  ```bash
  cd /data/company/project/npu-webhook/npu-vault
  cargo build -p vault-server 2>&1 | tail -20
  ```

- [ ] **2.7 手动验证（可选）**

  启动服务后，用 `wscat` 或浏览器 DevTools 连接验证：

  ```bash
  # 安装 wscat（npm 工具，可选）
  # npx wscat -c ws://127.0.0.1:18900/ws/scan-progress
  # 期望每 2 秒收到一条 JSON，vault locked 时 vault_state="locked"
  ```

- [ ] **2.8 Commit**

  ```
  feat(vault-server): add /ws/scan-progress WebSocket endpoint + UI progress card (fix S4)
  ```

---

## 自检清单

- [ ] **占位符扫描**：计划中所有代码均为完整实现，无 `todo!()` / `unimplemented!()` / `// TODO` 占位符
- [ ] **QueueWorker 签名验证**：计划中 Task 1 的实现未直接调用 `QueueWorker::start()`（因 store 不是独立 Arc），而是在 AppState worker 线程内复现相同逻辑，与 `queue.rs` 中 `process_batch` 的分区逻辑保持一致
- [ ] **WebSocket API 正确性**：使用 `axum::extract::ws::{WebSocketUpgrade, WebSocket, Message}`，`ws.on_upgrade(|socket| ...)` 模式符合 axum 0.8 文档
- [ ] **重复启动保护**：`AtomicBool::compare_exchange` 确保同一 vault 生命周期内只有一个 worker 线程
- [ ] **lock 时优雅退出**：每次循环开头检查 `VaultState::Unlocked`，退出后重置 `AtomicBool` 为 false
- [ ] **axum ws feature**：已在 `Cargo.toml` 步骤 2.1 中明确添加，否则 `axum::extract::ws` 模块不存在

# npu-vault 版本记录

## 已发布

## Search Enhancement + Queue Worker + WebSocket (2026-04-14)

### 搜索增强

- **Reranker**：`VectorIndex::get_vector()` 取 item 均值向量，`rerank()` 以 `0.7×cosine + 0.3×rrf` 二次排序，当 `top_k <= 20` 时自动启用
- **LRU 搜索缓存**：256 条目、30s TTL，djb2 哈希键，命中时响应含 `"cached": true`；ingest 时自动清空
- **GET /api/v1/items/stale**：按 `days`（默认30）返回超期未更新条目，路由顺序在 `{id}` 之前
- **GET /api/v1/items/{id}/stats**：返回 chunk_count / embedding_pending / embedding_done 统计（无需解密内容）
- **POST /api/v1/feedback**：接收 `relevant/irrelevant/correction` 三种反馈，写入 feedback 表（含 CHECK 约束）

### Queue Worker + WebSocket

- **QueueWorker 自动启动**：vault setup/unlock 后通过 AtomicBool CAS 保证单实例启动，vault lock 后退出并重置 flag
- **WebSocket /ws/scan-progress**：每 2 秒推送 `{vault_state, pending_embeddings, pending_classify, bound_dirs}`，vault 锁定时持续推送锁定状态
- **Web UI 进度卡**：首页状态页新增实时进度显示，WebSocket 断线自动重连（clearTimeout + 3s 回退）

### 测试

- 新增约 17 个测试，总计 **156 tests**（vault-core: 144 + vault-server: 12）

---

## Phase 4 增量：搜索质量提升 + 本地推理层 (2026-04-14)

### Phase 4 增量：搜索质量提升 + 本地推理层

- `vault-core/src/infer/`: 新增本地 ONNX 推理模块（ort 2.x）
  - `OrtEmbeddingProvider`: Qwen3-Embedding-0.6B INT8，mean-pool + L2 归一化
  - `OrtRerankProvider`: bge-reranker-v2-m3 INT8，cross-encoder sigmoid 评分
  - `model_store`: hf-hub 自动下载，`~/.local/share/npu-vault/models/` 缓存
  - `provider`: EP 自动选择（CUDA > CPU，`NPU_VAULT_EP` 环境变量覆盖）
- `platform.rs`: 新增 `models_dir()`, `NpuKind`, `detect_npu()`
- `search.rs`: `SearchParams` + `SearchContext` + `search_with_context` 三阶段管道
  - 修复：向量搜索硬编码 10 的 bug
  - Chat 和 Search 路径统一使用 `search_with_context`
- `llm.rs`: 新增 `OpenAiLlmProvider`（OpenAI-compat，支持 Ollama/OpenAI/LM Studio/vLLM）
- `routes/search.rs`: 新增 `initial_k` / `intermediate_k` 可选 query 参数
- `routes/chat.rs`: 修复 500 字符截断 bug（RAG 上下文不再被强制截断）

---

## Test Coverage Expansion (2026-04-14)

### 测试覆盖补全

- **Python 测试环境修复**：创建 `pytest.ini`（`pythonpath = src`），解决 `ModuleNotFoundError`，78 个测试正常收集
- **store.rs 单元测试**（+18）：3 个新模块覆盖 `bind_directory`、`unbind_directory`、`update_dir_last_scan`、`get/upsert_indexed_file`、完整 embedding 队列生命周期（enqueue/dequeue/done/failed/pending/checkpoint）
- **vault-server 集成测试框架**（+13）：导出 `build_router` 函数，`tests/server_test.rs` 通过 axum Router 直连测试核心路由；覆盖 vault 状态、setup/lock/unlock、ingest（成功/锁定403）、items（列表/查询/404/锁定403）

### 测试

- 总计 **197 tests**（vault-core: 157 + server_test: 13 + 集成测试: 27）

---

## Security Hardening (2026-04-13)

### 安全修复

- **CORS 白名单**：将 `CorsLayer::permissive()` 替换为仅允许 `chrome-extension://`、`localhost`、`127.0.0.1` 的白名单，并启用 `allow_credentials(true)`
- **Bearer token 默认开启**：`--require-auth` 默认值改为 `true`，新增 `--no-auth` 反向 flag（仅用于本地开发，启用时打印警告）
- **device-secret + change-password 强制认证**：`/api/v1/vault/device-secret/export`、`/api/v1/vault/device-secret/import`、`/api/v1/vault/change-password` 三个端点无论 `--no-auth` 状态均强制要求 Bearer token
- **NAS 模式 TLS 警告**：绑定非 loopback 地址且无 TLS 时，启动时输出 `⚠ WARNING`
- **路径边界验证**：`bind_directory` 新增 3 层验证（绝对路径、`canonicalize()` 规范化、home 目录边界），防止绑定 `/etc`、`/proc` 等系统目录
- **Zeroizing 中间缓冲**：`derive_master_key` 中的 password+device_secret 拼接 Vec 改用 `Zeroizing<Vec<u8>>`，函数返回前自动清零敏感数据
- **Token 吊销机制**：`lock()` 调用时递增 `token_nonce`（存储于 vault_meta），`verify_session` 验证 nonce 一致性，lock 后旧 token 立即失效
- **change_password 事务保护**：4 次 `set_meta` 写入（salt + 3 个 DEK）包进单个 SQLite 事务，防止中途失败导致数据不一致

### 测试

- 新增 38 个测试，总计 **138 tests**（vault-core: 129 + vault-server: 9）

---

### v0.5.0 — 全量子系统完成 (B + C + D + E + F1 + F3 + F4)

**子系统 B — 行为画像**:
- `search_history` + `click_events` 表，查询加密存储
- `Store::log_search`, `recent_searches`, `log_click`, `popular_items`
- API: `/behavior/click`, `/behavior/history`, `/behavior/popular`

**子系统 C — Web UI MVP**:
- 8 个标签页（搜索/录入/条目/分类/聚类/远程/历史/设置）
- 设置页新增：分类队列 drain、Profile 导出/导入
- 远程标签：WebDAV 目录绑定表单
- 历史标签：搜索历史 + 热门条目

**子系统 D — 运行时插件加载**:
- `Taxonomy::load_user_plugins(config_dir)` 从 `{config_dir}/plugins/*.yaml` 加载
- `/plugins` 端点区分 `source: builtin/user`
- init_search_engines 自动加载用户插件

**子系统 E — 画像导出/导入**:
- `GET /profile/export` 导出 VaultProfile JSON（tags + clusters + histograms）
- `POST /profile/import` 导入（合并语义，跳过不存在的 item_id）
- 用于跨设备迁移分类结果

**子系统 F1 — NAS WebDAV 远程目录**:
- `scanner_webdav.rs` — PROPFIND 列表 + GET 下载 + 增量去重
- `POST /index/bind-remote` 绑定 WebDAV URL 并扫描
- reqwest blocking client，支持 Basic Auth

**子系统 F3 — 分类队列 drain**:
- `AppState::drain_classify_batch(batch_size)` 手动处理分类任务
- `POST /classify/drain` 端点（替代后台线程，回避 Vault 所有权重构）

**子系统 F4 — 索引持久化加密**:
- `crypto::save_encrypted_file / load_encrypted_file` — AES-256-GCM 文件加密通用 helpers
- `VectorIndex::save_encrypted / load_encrypted` — usearch 索引打包 + 加密（长度前缀格式）
- tantivy 继续内存重建策略（从 items.content 恢复）

**子系统 F2 — Tauri 脚手架（待激活）**:
- `crates/vault-tauri/` 目录含 README + Cargo.toml.template + main.rs.template
- 文档化激活路径和架构方案

**测试**: 120 tests (114 unit + 6 integration), +11 from v0.4.0
**二进制**: vault-server 28 MB (+1 MB)

---

### v0.4.0 — 子系统 A: AI 自动分类

**vault-core 新增 5 个模块**:
- `llm.rs` — Ollama chat client，支持 qwen2.5 / llama3.2 / phi3 自动探测
- `taxonomy.rs` — 核心 5 维 + 通用扩展 3 维 + 插件机制，YAML 定义
- `classifier.rs` — 批量 LLM 分类 pipeline，MockLlmProvider 单元测试
- `clusterer.rs` — HDBSCAN 聚类 + LLM 命名
- `tag_index.rs` — 内存反向索引，unlock 时构建

**内置插件**:
- 编程/技术 (tech): stack_layer + language_tech + design_pattern
- 法律 (law): law_branch + doc_type + jurisdiction + risk_level

**HTTP API 新增**:
- `POST /classify/{id}`, `POST /classify/rebuild`, `GET /classify/status`
- `GET /tags`, `GET /tags/{dimension}`
- `GET /clusters`, `GET /clusters/{id}`, `POST /clusters/rebuild`
- `GET /plugins`

**Web UI**:
- 新增"分类"标签页：维度选择器 + 直方图浏览 + 重分类触发
- 新增"聚类"标签页：聚类卡片网格 + 重建按钮

**Store 迁移**:
- `embed_queue` 表新增 `task_type` 列（幂等迁移）
- 新方法: `update_tags`, `get_tags_json`, `enqueue_classify`, `list_all_item_ids`, `mark_task_pending`

**硬依赖**:
- 分类功能需要 Ollama 运行 + chat 模型（qwen2.5:3b 推荐）
- 无 chat 模型时分类端点返回 503，其他功能正常

**测试**: 28 unit + 3 integration = **109 tests** (103 vault-core unit + 6 integration)

**二进制大小变化**:
- vault-server 从 26 MB 增至约 27 MB（hdbscan crate + 插件 YAML）

---

### v0.3.0 — Phase 3: NAS 模式 + Web UI + Device Secret 迁移

**TLS + NAS 模式**：
- `axum-server` + `rustls` 纯 Rust TLS 栈，无 OpenSSL 依赖
- CLI 参数 `--tls-cert` / `--tls-key` 启用 HTTPS
- CLI 参数 `--require-auth` 启用 Bearer token 认证
- `bearer_auth_guard` 中间件：远程请求需携带 `Authorization: Bearer <session_token>`
- 公共白名单：`/status/health`, `/`, `/ui/*`, `/vault/setup`, `/vault/unlock`, `/vault/status`
- 双层中间件顺序：bearer_auth_guard → vault_guard → CORS

**嵌入式 Web UI**：
- 单页 HTML + vanilla JS，`include_str!` 编译进二进制
- 四个标签页：搜索 / 录入 / 条目 / 设置
- 响应式布局，移动浏览器友好
- DOM 纯脚本操作，无 innerHTML XSS 风险
- 支持 setup / unlock / lock、搜索、录入、条目列表、Device Secret 导出

**Device Secret 导出/导入**：
- `Vault::export_device_secret()` — 返回 64 字符 hex（32 字节），仅 UNLOCKED 状态
- `Vault::import_device_secret(hex)` — 导入前校验长度，写入 0o600 权限文件
- API: `GET /vault/device-secret/export` + `POST /vault/device-secret/import`
- 多设备迁移流程：导出旧设备的 device.key → 新设备 import → 用原密码 unlock → 数据解锁

**测试**: 75 unit + 3 integration = **78 tests**（vault 模块 13 → 16，新增 `export_device_secret_requires_unlocked`, `import_device_secret_writes_file`, `import_invalid_hex_fails`）

**二进制**: vault-cli 4.1 MB + vault-server 26 MB（TLS + Web UI 增量约 17 MB）

---

### v0.2.5 — 搜索集成 + Chrome 扩展兼容

**AppState 搜索引擎生命周期**：
- `AppState` 新增 `Mutex<Option<FulltextIndex>>` / `Mutex<Option<VectorIndex>>` / `Mutex<Option<Arc<dyn EmbeddingProvider>>>`
- `init_search_engines()` 在 `vault_setup` / `vault_unlock` 后调用：创建 FulltextIndex、VectorIndex(1024)、OllamaProvider
- `clear_search_engines()` 在 `vault_lock` 前调用：全部置 None
- 修复 OllamaProvider 嵌套 tokio runtime panic：搜索路由用 `spawn_blocking` 调用

**搜索路由集成**：
- `GET /search` 真实 tantivy BM25 + usearch 向量 + RRF 融合 + SQLite 解密
- `POST /search/relevant` 同上 + `allocate_budget()` 注入预算分配，返回 `inject_content`
- 搜索结果格式对齐 Chrome 扩展 `SearchResult` 接口

**Ingest 链路补全**：
- ingest 时同步加入 tantivy 全文索引
- 两层 embedding 入队：Level 1 章节 (`extract_sections`) + Level 2 段落 (`chunk`)

**Chrome 扩展兼容**：
- 补全 `/api/v1/items/{id}` PATCH（更新 title/content）
- 补全 `/api/v1/settings` GET/PATCH（存于 vault_meta，合并语义）
- 完整 18 个 API 端点覆盖 npu-webhook Python 原型协议

**测试**: 72 unit + 3 integration = 75 tests（保持不变）

---

### v0.2.0 — Phase 2b: 文件扫描 + Embedding 队列 + Upload API

**scanner.rs 文件扫描引擎**：
- `scan_directory()` — walkdir 递归/非递归遍历，file_types 过滤
- `process_single_file()` — SHA-256 hash 比对 indexed_files，未变化跳过，新增/变更入库
- `create_watcher()` / `watch_directory()` — notify-rs 实时监听（CrossPlatform）
- 只读保证：`File::open(Read)`，永不修改源文件
- 两层入队：Level 1 章节（priority-1）+ Level 2 段落块（priority=2）

**queue.rs Embedding 队列 Worker**：
- `QueueWorker::start()` — 后台线程轮询 pending 任务，批量 embed
- `QueueWorker::process_all()` — 同步处理（测试用）
- 批次大小 10，轮询间隔 2 秒，失败重试 3 次
- 结果写入 VectorIndex（所有 level）+ FulltextIndex（仅 Level 1 章节）

**vault-server 新增路由**：
- `POST /api/v1/index/bind` — 绑定目录 + 触发全量扫描
- `DELETE /api/v1/index/unbind` — 解绑目录（软删除）
- `GET /api/v1/index/status` — 绑定目录列表 + pending embedding 数
- `POST /api/v1/upload` — multipart 文件上传（最大 20 MB）

**Store 新增方法**：
- `bind_directory` / `unbind_directory` / `list_bound_directories` / `update_dir_last_scan`
- `get_indexed_file` / `upsert_indexed_file`
- `enqueue_embedding` / `dequeue_embeddings` / `mark_embedding_done` / `mark_embedding_failed` / `pending_embedding_count`

**测试**: 72 unit + 3 integration = 75 tests

---

### v0.1.5 — Phase 2a: Axum API Server + 搜索引擎基础

**vault-core 新增 6 个模块**：
- `chunker.rs` — 滑动窗口分块 + `extract_sections` 语义章节切割（Markdown 标题 / 代码 def / 段落）
- `parser.rs` — MD / TXT / 代码解析 + `parse_bytes` 内存解析 + `file_hash` SHA-256
- `embed.rs` — `EmbeddingProvider` trait + `OllamaProvider` (reqwest HTTP) + `NoopProvider` 降级
- `index.rs` — tantivy 0.22 全文索引封装，`tantivy-jieba` 中文分词，ReloadPolicy::Manual
- `vectors.rs` — usearch HNSW + cosine + f16 量化，外部 HashMap metadata 映射
- `search.rs` — RRF 融合（k=60）+ 动态注入预算（按 score 比例 + 最低 100 字保底）

**vault-server 新 crate**：
- Axum 0.8 HTTP server，Tokio 异步运行时
- `AppState = Mutex<Vault>` 共享状态
- `vault_guard` 中间件 — UNLOCKED 检查，SEALED/LOCKED 时返回 403
- 路由模块：vault / ingest / items / search / index / upload / status
- CORS 全开放（供 Chrome 扩展跨域调用）
- clap CLI args: `--host` / `--port`

**测试**: Phase 1 的 34 unit + 新增 28 unit (chunker:6, parser:6, embed:2, index:4, vectors:5, search:5) = 62 unit + 3 integration = **65 tests**

**二进制**: vault-cli 4.1 MB + vault-server 9.0 MB（尚未含 TLS）

---

### v0.1.0 — Phase 1: 加密存储引擎

**Cargo workspace 初始化**：
- `vault-core` library crate — 核心加密和存储
- `vault-cli` binary crate — 命令行管理工具

**vault-core 5 个基础模块**：
- `error.rs` — `VaultError` 统一错误类型（13 个变体），thiserror 派生，`Result<T>` 别名
- `platform.rs` — 跨平台路径：`data_dir()` / `config_dir()` / `db_path()` / `device_secret_path()`
- `crypto.rs` — 纯密码学原语：
  - `Key32` 32 字节密钥，`ZeroizeOnDrop` Drop 时清零
  - `derive_master_key` — Argon2id (m=64MB, t=3, p=4)
  - `encrypt` / `decrypt` — AES-256-GCM，格式 `nonce(12B) ‖ ciphertext ‖ tag(16B)`
  - `encrypt_dek` / `decrypt_dek` — DEK 加解密
  - `hmac_sign` / `hmac_verify` — HMAC-SHA256
- `store.rs` — rusqlite SQLite 封装：
  - Schema: vault_meta, items, embed_queue, bound_dirs, indexed_files, sessions
  - `PRAGMA journal_mode=WAL` + `foreign_keys=ON` + `busy_timeout=5000`
  - 字段级加密 CRUD：`insert_item` 加密 content/tags，`get_item` 解密返回
  - `checkpoint()` 刷 WAL 到主 DB（供加密验证测试使用）
- `vault.rs` — 顶层编排：
  - `VaultState` enum: Sealed / Locked / Unlocked
  - `setup(password)` — 生成 device.key (0o600) + salt + 3 DEK，自动 unlocked
  - `unlock(password)` — 校验 device_secret_hash → 派生 MK → 解密 DEK → 签发 session token
  - `lock()` — `UnlockedKeys` Drop → Key32 zeroize
  - `change_password(old, new)` — 重新加密 DEK，业务数据不动
  - `verify_session(token)` — HMAC 验证 + 过期检查

**vault-cli 7 个子命令**：
- `npu-vault setup` — 首次设置主密码（`rpassword` 无回显输入 + 二次确认）
- `npu-vault unlock` — 解锁 vault
- `npu-vault lock` — 锁定 vault
- `npu-vault status` — JSON 输出状态 + 条目数 + 路径
- `npu-vault insert -t -c -s` — 插入知识条目
- `npu-vault get <id>` — 获取单条目（解密）
- `npu-vault list -l` — 列出条目摘要

**集成测试**：
- `e2e_full_lifecycle` — setup → insert → lock → unlock → verify → change_password → delete
- `e2e_content_encrypted_at_rest` — 验证 SQLite 原始字节不含明文
- `e2e_multiple_items` — 批量插入 + 分页

**测试**: 34 unit + 3 integration = 37 tests

**二进制**: vault-cli 3.8 MB（初版，仅 CLI）

---

## 路线图

### v0.6.0 — Tauri 桌面客户端 + 安装包

- Tauri 2 桌面应用（Windows/macOS/Linux 原生窗口）
- 系统托盘（tray-icon）+ 右键菜单（lock/status/quit）
- 包装 Web UI 为原生应用
- 打包：`cargo tauri build` → AppImage / MSI / DMG
- 自动更新（tauri-plugin-updater）
- 开机自启（systemd user service / Windows Service / launchd）

### v0.7.0 — Queue Worker 自动启动 + WebSocket 推送

- vault-server 启动时自动 `QueueWorker::start()`，在 unlock 后开始消费队列
- WebSocket `/ws/scan-progress` 推送扫描进度 + embedding 进度
- Web UI 实时显示后台处理状态

### v0.8.0 — 云同步（可选）

- 加密备份到任意 S3 兼容对象存储（或 WebDAV）
- 端到端加密：云端仅看到密文 blob
- 增量同步（按时间戳）

### v1.0.0 — 正式发布

- GitHub Actions CI/CD 全流水线（Linux/Windows/macOS/Android 构建矩阵）
- 安装引导页（首次启动向导）
- 完整中英双语文档
- 官网 + 下载页
- 签名证书（Windows MSI / macOS notarization）

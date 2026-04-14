# npu-vault: 加密个人知识库引擎设计文档

> 日期: 2026-03-31
> 状态: Draft
> 基于: npu-webhook Python 原型 (Phase 0-3)

## 1. 产品定位

**一句话**: 本地优先、端到端加密的个人知识库引擎。跨 Linux/Windows/Android，通过浏览器扩展和本地文件扫描自动积累知识，让云端 AI 更懂你。

**产品名**: `npu-vault`

**与 npu-webhook 的关系**: Python 原型保留在 `main` 分支用于验证实验；Rust 商用产品线在独立分支 `rust-commercial` 开发，不复用 Python 代码，但复用已验证的架构思路和 API 协议。

## 2. 技术选型

| 领域 | 选型 | 理由 |
|------|------|------|
| 语言 | Rust (2021 edition) | 跨平台编译、内存安全、产品级二进制 |
| Web 框架 | Axum + Tokio | 生态最成熟的异步 HTTP 框架 |
| 数据库 | rusqlite (SQLite) | 嵌入式、零部署、字段级加密 |
| 全文搜索 | tantivy + jieba-rs | 纯 Rust BM25 + 中文分词 |
| 向量搜索 | usearch | 轻量 (~2MB)、HNSW、f16 量化 |
| 加密 | argon2 + aes-gcm crate | 纯 Rust、审计过的密码学库 |
| Embedding | Ollama HTTP API (reqwest) | 不内嵌模型，保持引擎轻量 |
| 文件监听 | notify-rs | 跨平台 (inotify/ReadDirectoryChanges/FSEvents) |
| 文件遍历 | walkdir | 递归目录遍历 |
| 配置 | toml + serde | Rust 生态标准 |
| CLI | clap | 命令行解析 |
| 桌面 GUI (Phase 3) | Tauri v2 | Rust 后端 + Web 前端 |
| 系统托盘 (Phase 3) | tray-icon | 纯 Rust |

## 3. 整体架构

```
┌─────────────────────────────────────────────────┐
│  Chrome Extension (现有，对接新 API)              │
│  Tauri Desktop (Phase 3)                         │
│  Mobile / NAS Client (Phase 3)                   │
├─────────────────────────────────────────────────┤
│  HTTP API Layer (Axum)           [vault-server]  │
│  ├── Auth: session token (vault unlock 后签发)    │
│  ├── Ingest / Search / Upload / Items / Index    │
│  └── 兼容现有 /api/v1/* 协议                     │
├─────────────────────────────────────────────────┤
│  Core Engine (纯 Rust lib crate) [vault-core]    │
│  ├── Vault    — 密钥派生 + 状态机 + 会话管理      │
│  ├── Crypto   — AES-256-GCM 字段/文件加解密       │
│  ├── Store    — rusqlite 元数据 + 加密内容        │
│  ├── Index    — tantivy 全文 + usearch 向量       │
│  ├── Scanner  — notify-rs 文件监听 + 全量扫描     │
│  ├── Chunker  — 滑动窗口 + 语义章节切割           │
│  ├── Embed    — Ollama HTTP client (reqwest)      │
│  └── Search   — RRF 混合 + 层级检索 + 动态预算    │
├─────────────────────────────────────────────────┤
│  Platform Abstraction                            │
│  ├── paths — XDG (Linux) / AppData (Win)         │
│  └── mobile — HTTP-only 模式 (NAS 场景)          │
└─────────────────────────────────────────────────┘
```

**设计原则**:
- `vault-core` 是 library crate，server/cli/tauri 共同依赖
- Vault 状态控制一切: LOCKED 状态下只能 unlock/status，其他 API 返回 403
- API 协议向后兼容 npu-webhook，Chrome 扩展换 URL 即可切换后端
- Embedding 不自带模型，通过 HTTP 调 Ollama

## 4. Vault 加密设计

### 4.1 密钥体系

```
Master Password (用户记忆)  +  Device Secret (设备文件, 256-bit)
                │                       │
                └───────────┬───────────┘
                            ↓
                Argon2id(password + device_secret, salt)
                params: m=64MB, t=3, p=4
                → 32-byte Master Key (MK)
                            │
                    ┌───────┼────────┐
                    ↓       ↓        ↓
                  DEK_db  DEK_idx  DEK_vec
```

- **Device Secret**: 首次 setup 时 `OsRng` 生成 256-bit 随机值，存于 `{config_dir}/device.key`，文件权限 0600。迁移设备需导出此文件。
- **Salt**: 每次 setup/change_password 时生成 32-byte 随机 salt，存于 `vault_meta` 表。
- **DEK (Data Encryption Key)**: 三个独立 256-bit 密钥，用 MK 通过 AES-256-GCM 加密后存于 `vault_meta` 表。数据永远用 DEK 加密，更改密码只需重新加密 DEK，不需要重新加密全部数据。

### 4.2 Vault 状态机

```
             ┌─────────┐
  init ──→   │ SEALED  │  (首次运行，无密码)
             └────┬────┘
                  │ setup(password) → 生成 device.key + salt + DEK × 3
                  ↓
             ┌─────────┐
  lock() ──→ │ LOCKED  │  ←── timeout / 手动锁定
             └────┬────┘
                  │ unlock(password) → 派生 MK → 解密 DEK → 加载索引到内存
                  ↓
             ┌──────────┐
             │ UNLOCKED │ ──→ 所有 API 可用
             └──────────┘
                  │ change_password(old, new) → 验证 old → 新 MK → 重新加密 DEK
                  │ lock() → 清除内存中 MK/DEK/索引 → 回到 LOCKED
```

### 4.3 会话管理

- unlock 成功后签发 session token: `HMAC-SHA256(session_id + expiry, MK)`
- 默认有效期 4 小时，可配置
- API 请求通过 `Authorization: Bearer <token>` 携带
- 过期或 lock 后 token 失效，返回 403
- Chrome 扩展将 token 存入 `chrome.storage.session`（浏览器关闭即清除）

### 4.4 字段级加密策略

| 字段 | 加密 | 理由 |
|------|------|------|
| `id`, `created_at`, `updated_at`, `source_type` | 明文 | 列表/统计不需解锁 |
| `title` | 明文 | LOCKED 下可展示条目名（同 1Password） |
| `content`, `chunk_text` | AES-256-GCM (DEK_db) | 核心敏感数据 |
| `url`, `domain` | 明文 | 搜索过滤需要 |
| `tags`, `metadata` | AES-256-GCM (DEK_db) | 可能含敏感上下文 |
| tantivy 索引目录 | 文件级 AES-256-GCM (DEK_idx) | 全文索引等同明文 |
| usearch 向量文件 | 文件级 AES-256-GCM (DEK_vec) | 向量可反推原文 |

每个加密字段独立 nonce (96-bit)，存储格式: `nonce(12B) || ciphertext || tag(16B)`。

## 5. 存储层

### 5.1 SQLite Schema

```sql
-- Vault 元数据 (始终明文)
CREATE TABLE vault_meta (
    key   TEXT PRIMARY KEY,
    value BLOB NOT NULL
);
-- 存储: vault_version, salt, argon2_params,
--       encrypted_dek_db, encrypted_dek_idx, encrypted_dek_vec,
--       device_secret_hash (SHA-256, 用于快速校验 device.key 是否匹配)

-- 知识条目
CREATE TABLE items (
    id          TEXT PRIMARY KEY,
    title       TEXT NOT NULL,       -- 明文
    content     BLOB NOT NULL,       -- AES-256-GCM 密文
    url         TEXT,
    source_type TEXT NOT NULL,       -- webpage/ai_chat/file/note
    domain      TEXT,
    tags        BLOB,               -- 加密 JSON
    metadata    BLOB,               -- 加密 JSON
    created_at  TEXT NOT NULL,       -- ISO 8601
    updated_at  TEXT NOT NULL,
    is_deleted  INTEGER DEFAULT 0
);
CREATE INDEX idx_items_source ON items(source_type);
CREATE INDEX idx_items_created ON items(created_at);
CREATE INDEX idx_items_deleted ON items(is_deleted);

-- Embedding 队列
CREATE TABLE embed_queue (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id     TEXT NOT NULL REFERENCES items(id),
    chunk_idx   INTEGER NOT NULL,
    chunk_text  BLOB NOT NULL,       -- 加密
    level       INTEGER DEFAULT 2,   -- 1=章节 2=段落
    section_idx INTEGER DEFAULT 0,
    priority    INTEGER DEFAULT 2,   -- 0=最高
    status      TEXT DEFAULT 'pending',
    attempts    INTEGER DEFAULT 0,
    created_at  TEXT NOT NULL
);
CREATE INDEX idx_eq_status ON embed_queue(status, priority, created_at);
CREATE INDEX idx_eq_item ON embed_queue(item_id);

-- 目录绑定 (明文)
CREATE TABLE bound_dirs (
    id         TEXT PRIMARY KEY,
    path       TEXT UNIQUE NOT NULL,
    recursive  INTEGER DEFAULT 1,
    file_types TEXT NOT NULL,        -- JSON array
    is_active  INTEGER DEFAULT 1,
    last_scan  TEXT
);

-- 文件索引 (增量扫描用)
CREATE TABLE indexed_files (
    id         TEXT PRIMARY KEY,
    dir_id     TEXT NOT NULL REFERENCES bound_dirs(id),
    path       TEXT UNIQUE NOT NULL,
    file_hash  TEXT NOT NULL,        -- SHA-256
    item_id    TEXT REFERENCES items(id),
    indexed_at TEXT NOT NULL
);
CREATE INDEX idx_if_dir ON indexed_files(dir_id);

-- 会话
CREATE TABLE sessions (
    token      TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);
```

### 5.2 tantivy 索引

- Schema: `item_id`(STRING, STORED) + `title`(TEXT, STORED) + `content`(TEXT) + `source_type`(STRING, INDEXED)
- Tokenizer: `jieba-rs` 中文分词 + tantivy 默认英文，通过 `TextAnalyzer` chain
- 加密: 解锁时将 `tantivy/*.enc` 逐文件 AES-GCM 解密到 `tempdir`，lock 时安全删除 tempdir
- 写入: 解锁状态下 ingest/scan 产生新文档时，同步更新内存索引 + 持久化 + 重新加密

### 5.3 usearch 向量索引

- 维度: 1024 (bge-m3)
- 距离: cosine
- 量化: f16 (减半存储，精度损失 <1%)
- ID 映射: 外部 `HashMap<u64, VectorMeta>` 存储 `{ item_id, chunk_idx, level, section_idx }`
- 加密: 与 tantivy 相同策略，文件级 AES-GCM

## 6. 搜索引擎

### 6.1 搜索流程

```
query (明文)
  │
  ├──→ tantivy BM25 search → Vec<(item_id, bm25_score)>
  │
  ├──→ reqwest → Ollama /api/embeddings → Vec<f32>
  │    → usearch cosine search → Vec<(vector_id, distance)>
  │    → 通过 ID map 转换为 Vec<(item_id, chunk_idx, cosine_score)>
  │
  └──→ RRF 融合
       │  score = Σ 1/(k + rank_i) * weight_i
       │  k=60, vector_weight=0.6, fulltext_weight=0.4
       │
       ├──→ 普通搜索: top-K → 解密 content → 返回
       │
       └──→ 层级搜索 (search_relevant):
            Stage 1: 向量检索 level=1 → top-5 候选章节
            Stage 2: 向量检索 level=2, section_idx IN [...], item_id IN [...]
                     → top-K 段落精排
            Stage 3: 取命中段落的父章节作为注入内容
            → _allocate_budget(2000 chars, score-weighted)
```

### 6.2 降级策略

| Ollama 状态 | 向量搜索 | 全文搜索 | 层级检索 |
|-------------|---------|---------|---------|
| 可用 | 正常 | 正常 | 正常 |
| 不可用 | 跳过 | 正常 | 回退普通搜索 |
| 模型未下载 | 队列暂停 | 正常 | 回退普通搜索 |

## 7. 文件扫描引擎

### 7.1 全量扫描

```
bind_directory(path, recursive, file_types)
  │
  ├──→ 全量首扫 (首次绑定时)
  │    ├── walkdir 递归遍历 → 过滤 file_types
  │    ├── 并发解析 (tokio::spawn, Semaphore max=4)
  │    │   └── sha256 比对 indexed_files
  │    │       ├── 未变: skip
  │    │       └── 新增/变更: parse → chunk → encrypt → insert → enqueue
  │    ├── 进度: WebSocket /api/v1/ws/scan-progress
  │    └── 完成: update bound_dirs.last_scan
  │
  └──→ 实时增量 (扫描后持续)
       └── notify-rs 监听
           ├── Create/Modify → process_file(priority=2)
           ├── Delete → soft_delete + remove vectors
           └── Rename → update indexed_files.path
```

**只读保证**: 对源文件只做 `File::open(Read)`，永不写入/修改/移动源文件。

### 7.2 文件解析

| 类型 | 实现 | 说明 |
|------|------|------|
| `.md` `.txt` | 内置 UTF-8 | 标题: `# ` 行或首行 |
| `.pdf` | `pdf-extract` / `lopdf` | 逐页提取文本 |
| `.docx` | `docx-rs` | 段落提取 |
| `.py` `.js` `.ts` `.rs` `.go` `.java` | 内置 | 整文件作为 content |

### 7.3 分块策略

复用 npu-webhook 已验证的两层分块:
- `extract_sections()`: 按 Markdown 标题 / 代码 def|class / 1500 字段落 切割 → Level 1
- `chunk()`: 512 字滑动窗口，128 字重叠 → Level 2
- Level 1 优先级 = max(1, priority-1)，Level 2 优先级 = priority

## 8. API 设计

所有端点前缀 `/api/v1/`，兼容 npu-webhook 协议。新增 vault 相关端点。

### 8.1 Vault 端点 (任何状态可用)

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/vault/status` | 返回 vault 状态 (sealed/locked/unlocked) |
| POST | `/vault/setup` | 首次设置密码，生成 device.key + DEK |
| POST | `/vault/unlock` | 输入密码解锁，返回 session token |
| POST | `/vault/lock` | 手动锁定，清除内存密钥和索引 |
| POST | `/vault/change-password` | 更改密码 (需 old + new) |
| GET | `/vault/device-secret/export` | 导出 device secret (需当前 token) |
| POST | `/vault/device-secret/import` | 导入 device secret (迁移场景) |

### 8.2 业务端点 (仅 UNLOCKED 状态)

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/ingest` | 知识注入 (纯文本) |
| POST | `/upload` | 文件直传 (multipart) |
| GET | `/search?q=&top_k=` | 混合搜索 |
| POST | `/search/relevant` | 层级检索 + 动态预算 (注入用) |
| GET/PATCH/DELETE | `/items[/{id}]` | 知识条目 CRUD |
| POST/DELETE/GET | `/index/bind\|unbind\|status` | 目录绑定管理 |
| POST | `/index/reindex` | 触发重扫 |
| GET | `/status` | 系统状态 + 统计 |
| GET/PATCH | `/settings` | 配置管理 |
| GET | `/status/health` | 健康检查 (不需 token) |

### 8.3 WebSocket

| 路径 | 说明 |
|------|------|
| `/ws` | 通用事件推送 (embedding 进度, 扫描状态) |
| `/ws/scan-progress` | 扫描进度专用 |

## 9. Cargo Workspace 结构

```
npu-vault/
├── Cargo.toml                  # [workspace]
├── crates/
│   ├── vault-core/             # lib: 加密/存储/搜索/扫描
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs          # 公开 API
│   │       ├── crypto.rs       # Argon2id + AES-256-GCM + DEK 管理
│   │       ├── vault.rs        # 状态机 + 会话 token
│   │       ├── store.rs        # rusqlite schema + CRUD + 加解密
│   │       ├── index.rs        # tantivy 封装 + 加密索引
│   │       ├── vectors.rs      # usearch 封装 + 加密向量
│   │       ├── search.rs       # RRF 混合搜索 + 层级检索 + 动态预算
│   │       ├── scanner.rs      # walkdir 全量 + notify 增量
│   │       ├── chunker.rs      # 滑动窗口 + extract_sections
│   │       ├── parser.rs       # 文件解析 (md/txt/pdf/docx/code)
│   │       ├── embed.rs        # Ollama HTTP embedding client
│   │       ├── platform.rs     # 跨平台路径 (Linux/Win/Android)
│   │       └── error.rs        # thiserror 统一错误类型
│   │
│   ├── vault-server/           # bin: Axum HTTP API
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs         # CLI args + server bootstrap
│   │       ├── state.rs        # Arc<AppState> 共享状态
│   │       ├── middleware.rs   # vault_guard (UNLOCKED 检查)
│   │       └── routes/
│   │           ├── mod.rs
│   │           ├── vault.rs    # /vault/* 端点
│   │           ├── ingest.rs   # /ingest + /upload
│   │           ├── search.rs   # /search + /search/relevant
│   │           ├── items.rs    # /items CRUD
│   │           ├── index.rs    # /index 目录管理
│   │           ├── status.rs   # /status + /health
│   │           └── settings.rs # /settings
│   │
│   └── vault-cli/              # bin: 管理工具
│       ├── Cargo.toml
│       └── src/
│           └── main.rs         # setup/unlock/lock/scan/status
│
├── tests/                      # 集成测试
│   ├── crypto_test.rs
│   ├── vault_test.rs
│   ├── store_test.rs
│   ├── search_test.rs
│   └── api_test.rs
├── docs/
├── README.md
├── DEVELOP.md
└── RELEASE.md
```

## 10. 交付阶段

### Phase 1 — 加密存储引擎 (核心基座)

范围: `vault-core` + `vault-cli`

- crypto.rs: Argon2id 密钥派生 + AES-256-GCM 加解密 + DEK 生命周期
- vault.rs: SEALED/LOCKED/UNLOCKED 状态机 + session token
- store.rs: rusqlite schema 初始化 + 加密 CRUD
- platform.rs: 跨平台路径
- error.rs: 统一错误类型
- vault-cli: `npu-vault setup` / `npu-vault unlock` / `npu-vault lock` / `npu-vault status`
- 测试: 加密正确性 + 状态机转换 + 密码变更 + session 过期

验收标准: CLI 可完成 setup → unlock → insert encrypted item → lock → verify content unreadable → unlock → verify content readable

### Phase 2 — API Server + 文件扫描

范围: `vault-server` 核心路由 + `vault-core` 搜索/扫描模块

- vault-server: Axum bootstrap + vault_guard middleware + 路由注册
- routes/vault.rs: /vault/* 端点
- routes/ingest.rs + search.rs + items.rs + index.rs + status.rs
- scanner.rs: walkdir 全量扫描 + notify-rs 增量监听
- chunker.rs + parser.rs: 文件解析和分块
- embed.rs: Ollama HTTP client + 队列 worker
- index.rs: tantivy 全文索引 + 加密
- vectors.rs: usearch 向量索引 + 加密
- search.rs: RRF 融合 + 层级检索 + 动态预算
- WebSocket: 扫描进度推送

验收标准: Chrome 扩展可对接 Rust 后端完成 ingest → search → inject 全流程; CLI 可绑定目录并完成全量扫描

### Phase 3 — 跨平台客户端

范围: Tauri v2 桌面应用 + 移动/NAS 模式

- Tauri app: 密码解锁 UI + 系统托盘 + 设置面板
- tray-icon: 状态图标 + 右键菜单 (lock/unlock/quit)
- NAS 模式: `npu-vault serve --bind 0.0.0.0:18900 --tls-cert/--tls-key`
- Android: Tauri Mobile 或纯 HTTP thin client

### Phase 4 — Chrome 扩展对接 + 生产打磨

范围: 扩展适配 + 安装包 + 自动更新

- 扩展端增加 token 管理 (Options 页输入密码 → unlock → 保存 token)
- Settings API 对齐
- 安装包: Linux (.deb/.AppImage) + Windows (.msi) + macOS (.dmg)
- 自动更新: cargo-dist 或 tauri-updater
- CI/CD: GitHub Actions 跨平台构建

## 11. 非功能需求

| 维度 | 目标 |
|------|------|
| 启动时间 | < 500ms (不含 unlock) |
| Unlock 时间 | < 2s (Argon2id m=64MB) |
| 搜索延迟 | < 100ms (1 万条) |
| 全量扫描 | > 50 files/sec |
| 二进制体积 | < 30MB (Linux release) |
| 内存占用 | < 200MB (1 万条, 索引加载后) |
| 支持平台 | x86_64-linux, x86_64-windows, aarch64-linux, aarch64-android |

## 12. 安全考量

- Master Key 和 DEK 仅在 UNLOCKED 状态存在于内存，lock 时 `zeroize` 清零
- Session token 用 HMAC-SHA256 签名，含过期时间，不可伪造
- Device Secret 文件权限 0600 (Linux)，NTFS ACL (Windows)
- Argon2id 参数抗 GPU/ASIC 暴力破解
- 加密字段每个值独立 nonce，防止密文比对攻击
- tantivy/usearch 文件解密到 tmpdir，lock 时安全删除 (overwrite + unlink)
- 不支持多用户 — 单 vault 单用户设计，简化安全模型

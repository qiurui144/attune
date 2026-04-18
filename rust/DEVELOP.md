# attune 开发指南

## 环境搭建

```bash
# Rust 工具链 (1.75+)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup default stable

# 克隆 + 构建
cd attune
cargo build --workspace

# 运行测试
cargo test --workspace    # 120 tests 全部通过

# 格式化 + lint
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
```

## 项目结构

```
rust/
├── Cargo.toml                        # workspace manifest
├── crates/
│   ├── attune-core/                   # lib crate
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                # 公开 API re-export
│   │       ├── error.rs              # VaultError + Result<T>
│   │       ├── platform.rs           # 跨平台路径 (dirs crate)
│   │       ├── crypto.rs             # Argon2id + AES-256-GCM + HMAC
│   │       ├── store.rs              # rusqlite + 加密 CRUD
│   │       ├── vault.rs              # 状态机 + Session Token
│   │       ├── chunker.rs            # 滑动窗口 + extract_sections
│   │       ├── parser.rs             # 文件解析 (MD/TXT/代码) + SHA-256
│   │       ├── embed.rs              # Ollama HTTP client (reqwest)
│   │       ├── index.rs              # tantivy 封装 (jieba tokenizer)
│   │       ├── vectors.rs            # usearch 封装 (HNSW + f16)
│   │       ├── search.rs             # RRF 融合 + 动态预算
│   │       ├── scanner.rs            # walkdir + notify-rs
│   │       ├── scanner_webdav.rs     # WebDAV 远程目录扫描
│   │       ├── queue.rs              # Embedding 队列 Worker
│   │       ├── llm.rs                # Ollama chat client (LlmProvider trait + OllamaLlmProvider + MockLlmProvider)
│   │       ├── taxonomy.rs           # 维度定义 + 插件 YAML 加载 + prompt 构建
│   │       ├── classifier.rs         # LLM 分类 pipeline (批量 + 容错)
│   │       ├── clusterer.rs          # HDBSCAN 聚类 + LLM 命名
│   │       └── tag_index.rs          # 内存反向索引
│   │   └── assets/plugins/
│   │       ├── tech.yaml             # 编程/技术插件
│   │       └── law.yaml              # 法律插件
│   │
│   ├── attune-server/                 # bin crate
│   │   ├── Cargo.toml
│   │   ├── assets/
│   │   │   └── index.html            # 嵌入式 Web UI (include_str!)
│   │   └── src/
│   │       ├── main.rs               # Axum bootstrap + CLI args + TLS
│   │       ├── state.rs              # Arc<AppState>
│   │       ├── middleware.rs         # vault_guard + bearer_auth_guard
│   │       └── routes/
│   │           ├── mod.rs
│   │           ├── vault.rs          # /vault/* (setup/unlock/lock/device-secret)
│   │           ├── ingest.rs         # /ingest
│   │           ├── upload.rs         # /upload (multipart)
│   │           ├── items.rs          # /items CRUD
│   │           ├── search.rs         # /search + /search/relevant
│   │           ├── index.rs          # /index (bind/unbind/status)
│   │           ├── settings.rs       # /settings (GET/PATCH)
│   │           ├── status.rs         # /status + /status/health
│   │           ├── classify.rs       # /classify/*
│   │           ├── clusters.rs       # /clusters/*
│   │           ├── plugins.rs        # /plugins/*
│   │           ├── tags.rs           # /tags/*
│   │           ├── behavior.rs       # /behavior/click|history|popular
│   │           ├── profile.rs        # /profile/export|import
│   │           ├── remote.rs         # /index/bind-remote (WebDAV)
│   │           └── ui.rs             # Web UI 页面
│   │
│   ├── attune-cli/                    # bin crate
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── main.rs               # clap: setup/unlock/lock/insert/get/list/status
│   │
│   └── attune-tauri/                  # bin (脚手架，待激活)
│       ├── README.md
│       ├── Cargo.toml.template
│       └── src/main.rs.template
│
└── tests/
    └── integration_test.rs           # 端到端集成测试
```

## 分层架构

```
┌─────────────────────────────────────────────────┐
│  Chrome Extension (attune/extension)        │
│  Web UI (embedded HTML)                          │
│  Mobile browser via HTTPS                        │
├─────────────────────────────────────────────────┤
│  HTTP Layer (Axum 0.8)          [attune-server]   │
│  ├── CORS middleware                             │
│  ├── bearer_auth_guard (optional Bearer token)   │
│  ├── vault_guard (UNLOCKED 检查)                │
│  └── Routes: 20+ endpoints                       │
├─────────────────────────────────────────────────┤
│  Core Engine (Rust lib)          [attune-core]    │
│  ├── Vault    — 状态机 + DEK 管理 + session       │
│  ├── Crypto   — Argon2id + AES-256-GCM + HMAC    │
│  ├── Store    — rusqlite + 字段级加密            │
│  ├── Index    — tantivy + jieba-rs               │
│  ├── Vectors  — usearch HNSW                     │
│  ├── Search   — RRF 融合 + allocate_budget       │
│  ├── Scanner  — walkdir + notify-rs              │
│  ├── ScannerWebDav — WebDAV PROPFIND + GET 远程文件扫描     │
│  ├── Chunker  — 滑动窗口 + extract_sections      │
│  ├── Parser   — MD/TXT/代码 + SHA-256            │
│  ├── Embed    — Ollama HTTP client               │
│  └── Queue    — Embedding 队列 Worker            │
└─────────────────────────────────────────────────┘
```

## 启动序列

### attune-server lifespan

```
main()
  1. tracing_subscriber 初始化日志
  2. CLI parse (host/port/tls-cert/tls-key/require-auth)
  3. Vault::open_default() — 打开 SQLite（不解锁）
  4. AppState::new(vault, require_auth) — 创建共享状态
  5. Router 注册 20+ 路由
  6. 中间件层：bearer_auth_guard → vault_guard → CORS
  7. 根据 --tls-cert/--tls-key 选择：
     - axum_server::bind_rustls (HTTPS)
     - axum::serve (HTTP)
  8. 接受请求，每个请求：
     - CORS 检查
     - Bearer token 验证（如启用）
     - vault_guard 检查 UNLOCKED 状态
     - 路由 handler 执行
```

### vault unlock 流程

```
POST /api/v1/vault/unlock { password }
  ↓
读取 device.key → 计算 SHA-256 比对 device_secret_hash
  ↓
Argon2id(password + device_secret, salt) → MK
  ↓
用 MK 解密 vault_meta 中的 encrypted_dek_db/idx/vec → DEK
  ↓
AppState.init_search_engines():
  - FulltextIndex::open_memory()
  - VectorIndex::new(1024)
  - OllamaProvider::default()
  ↓
签发 session token: HMAC(session_id:expires, MK)
  ↓
返回 { token }
```

### vault lock 流程

```
POST /api/v1/vault/lock
  ↓
AppState.clear_search_engines():
  - FulltextIndex → None
  - VectorIndex → None
  - OllamaProvider → None
  ↓
Vault.lock() → UnlockedKeys dropped → Key32::zeroize
  ↓
所有后续 API 请求被 vault_guard 拦截 → 403
```

## 加密细节

### Master Key 派生

```rust
// crypto.rs
pub fn derive_master_key(
    password: &[u8],
    device_secret: &[u8],   // 32 bytes
    salt: &[u8],            // 32 bytes
) -> Result<Key32> {
    let input = [password, device_secret].concat();
    let params = argon2::Params::new(65536, 3, 4, Some(32))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut mk = [0u8; 32];
    argon2.hash_password_into(&input, salt, &mut mk)?;
    Ok(Key32(mk))
}
```

参数选择：
- **64 MB 内存 (m=65536 KB)** — 抗 GPU 并行攻击，普通 PC 仅消耗 1-2 秒
- **3 轮迭代 (t=3)** — 增加总计算成本
- **4 线程 (p=4)** — 利用多核但不过度占用

### AES-256-GCM 加密

```rust
// 存储格式: nonce(12B) || ciphertext || tag(16B)
pub fn encrypt(key: &Key32, plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key.as_bytes())?;
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let ciphertext = cipher.encrypt(&nonce.into(), plaintext)?;
    Ok([&nonce, &ciphertext[..]].concat())
}
```

每次加密独立随机 nonce，防止相同明文产生相同密文（关键抵抗分析攻击）。

### Session Token 签发与验证

```rust
// 格式: {session_id}:{expires_timestamp}.{hmac_hex}
fn create_session_token(mk: &Key32) -> Result<String> {
    let session_id = Uuid::new_v4().simple().to_string();
    let expires = Utc::now().timestamp() + 4 * 3600;
    let payload = format!("{session_id}:{expires}");
    let sig = hmac_sign(mk, payload.as_bytes());
    Ok(format!("{payload}.{}", hex::encode(sig)))
}
```

验证时：拆分 payload + signature，用 MK 重新 HMAC 比对，再检查过期时间。

## 搜索引擎架构

### tantivy 全文索引

```rust
// index.rs
fn build_schema() -> Schema {
    let mut builder = Schema::builder();
    let text_indexing = TextFieldIndexing::default().set_tokenizer("jieba");
    let text_opts = TextOptions::default()
        .set_indexing_options(text_indexing)
        .set_stored();
    builder.add_text_field("item_id", STRING | STORED);
    builder.add_text_field("title", text_opts.clone());
    builder.add_text_field("content", text_opts);
    builder.add_text_field("source_type", STRING | STORED);
    builder.build()
}
```

关键点：
- **`jieba` tokenizer** — 通过 `tantivy-jieba` 桥接，支持中文分词
- **STORED** — `item_id` 和 `title` 存储在索引中，无需回查 SQLite
- **ReloadPolicy::Manual** — 每次搜索前手动 reload，避免 RAM 模式下延迟

### usearch 向量索引

```rust
// vectors.rs
pub struct VectorIndex {
    index: usearch::Index,
    meta: HashMap<u64, VectorMeta>,  // u64 key → 原始 metadata
    next_key: u64,
    dims: usize,
}

impl VectorIndex {
    pub fn new(dims: usize) -> Result<Self> {
        let options = IndexOptions {
            dimensions: dims,
            metric: MetricKind::Cos,
            quantization: ScalarKind::F16,  // 半精度减半存储
            ..Default::default()
        };
        // ...
    }
}
```

关键点：
- **HNSW 图索引** — 近似最近邻，亿级向量 ms 级延迟
- **f16 量化** — 向量存储减半（4 MB/10000 → 2 MB/10000），精度损失 <1%
- **外部 HashMap metadata** — usearch 原生不存 metadata，我们在外部映射 u64 key → `{item_id, chunk_idx, level, section_idx}`

### RRF 融合

```rust
// search.rs
pub fn rrf_fuse(
    vector_results: &[(String, f32)],
    fulltext_results: &[(String, f32)],
    vector_weight: f32,    // 0.6
    fulltext_weight: f32,  // 0.4
    top_k: usize,
) -> Vec<(String, f32)> {
    let mut scores: HashMap<String, f32> = HashMap::new();
    for (rank, (id, _)) in vector_results.iter().enumerate() {
        *scores.entry(id.clone()).or_default()
            += vector_weight / (60.0 + rank as f32 + 1.0);
    }
    for (rank, (id, _)) in fulltext_results.iter().enumerate() {
        *scores.entry(id.clone()).or_default()
            += fulltext_weight / (60.0 + rank as f32 + 1.0);
    }
    // sort by score descending, take top_k
}
```

RRF (Reciprocal Rank Fusion) 是 2009 年 TREC 论文提出的排名融合算法，无需归一化分数，对异构排名系统鲁棒。`k=60` 是论文推荐值。

### 动态注入预算

```rust
pub fn allocate_budget(results: &mut [SearchResult], budget: usize) {
    let total_score: f32 = results.iter().map(|r| r.score).sum();
    for r in results.iter_mut() {
        let share = r.score / total_score;
        let alloc = (budget as f32 * share).max(100.0) as usize;
        r.inject_content = Some(r.content.chars().take(alloc).collect());
    }
}
```

按 RRF 分数比例分配 2000 字预算，最低 100 字保底。取代固定截断（300 字）。

## 文件扫描流程

```
POST /api/v1/index/bind { path, recursive, file_types }
  ↓
Vault.store().bind_directory(path, recursive, file_types)
  → INSERT INTO bound_dirs → 返回 dir_id
  ↓
scanner::scan_directory(store, dek, dir_id, path, recursive, file_types)
  ↓
WalkDir(path, recursive) → 过滤 file_types
  ↓
对每个文件 process_single_file():
  1. parser::file_hash(path) → SHA-256
  2. store.get_indexed_file(path) → 比对 hash
     - 未变: Skipped
     - 变更: delete_item(旧 item_id) → 继续
  3. parser::parse_file(path) → (title, content)
  4. store.insert_item(dek, title, content, source_type="file")
  5. chunker::extract_sections(content) → Vec<(section_idx, text)>
  6. 为每个 section enqueue_embedding(level=1)
  7. 为每个 section chunk() → enqueue_embedding(level=2)
  8. store.upsert_indexed_file(dir_id, path, hash, item_id)
  ↓
store.update_dir_last_scan(dir_id)
  ↓
返回 ScanResult { total, new, updated, skipped, errors }
```

**只读保证**：`std::fs::File::open(Read)` 打开，永不写入源文件。

**增量检测**：SHA-256 hash 比对，未变化直接跳过。

**两层入队**：Level 1 (章节) 和 Level 2 (512 字段落块) 分别入队，向量索引时 metadata 区分。

## Embedding 队列 Worker

```rust
// queue.rs
pub fn start(store, embedding, vectors, fulltext) -> JoinHandle<()> {
    thread::spawn(move || {
        while running {
            match process_batch(...) {
                Ok(0) => thread::sleep(2s),  // no tasks
                Ok(n) => { /* processed n */ },
                Err(_) => thread::sleep(2s),
            }
        }
    })
}

fn process_batch() -> Result<usize> {
    let tasks = store.dequeue_embeddings(10)?;  // pending → processing
    let texts = tasks.iter().map(|t| t.chunk_text.as_str()).collect();
    let embeddings = embedding.embed(&texts)?;
    for (i, task) in tasks.iter().enumerate() {
        vectors.add(&embeddings[i], VectorMeta { ... })?;
        if task.level == 1 {
            fulltext.add_document(&task.item_id, "", &task.chunk_text, "file")?;
        }
        store.mark_embedding_done(task.id)?;
    }
    Ok(tasks.len())
}
```

**当前状态**：Worker 结构完整，`process_all()` 可同步处理（测试用），后台 `start()` 尚未在 server 启动时自动启动（Phase 4 补全）。

## 数据库 Schema

```sql
-- Vault 元数据（明文，始终可读）
CREATE TABLE vault_meta (
    key TEXT PRIMARY KEY,
    value BLOB NOT NULL
);
-- 存储: salt, argon2_params, encrypted_dek_db, encrypted_dek_idx,
--       encrypted_dek_vec, device_secret_hash, vault_version, app_settings

-- 知识条目（字段级加密）
CREATE TABLE items (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,           -- 明文
    content BLOB NOT NULL,         -- AES-256-GCM 密文
    url TEXT,
    source_type TEXT NOT NULL,
    domain TEXT,
    tags BLOB,                     -- 加密 JSON
    metadata BLOB,                 -- 加密 JSON
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    is_deleted INTEGER DEFAULT 0
);

-- Embedding 队列（明文 chunk_text，仅运行时短暂存在）
CREATE TABLE embed_queue (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id TEXT NOT NULL REFERENCES items(id),
    chunk_idx INTEGER NOT NULL,
    chunk_text BLOB NOT NULL,
    level INTEGER DEFAULT 2,       -- 1=章节, 2=段落
    section_idx INTEGER DEFAULT 0,
    priority INTEGER DEFAULT 2,
    status TEXT DEFAULT 'pending', -- pending/processing/done/abandoned
    attempts INTEGER DEFAULT 0,
    created_at TEXT NOT NULL
);

-- 目录绑定
CREATE TABLE bound_dirs (
    id TEXT PRIMARY KEY,
    path TEXT UNIQUE NOT NULL,
    recursive INTEGER DEFAULT 1,
    file_types TEXT NOT NULL,      -- JSON array
    is_active INTEGER DEFAULT 1,
    last_scan TEXT
);

-- 文件索引（增量扫描用）
CREATE TABLE indexed_files (
    id TEXT PRIMARY KEY,
    dir_id TEXT NOT NULL REFERENCES bound_dirs(id),
    path TEXT UNIQUE NOT NULL,
    file_hash TEXT NOT NULL,       -- SHA-256 hex
    item_id TEXT REFERENCES items(id),
    indexed_at TEXT NOT NULL
);

-- 会话（预留，当前 session 由 HMAC 验证，不落盘）
CREATE TABLE sessions (
    token TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);

-- 批注（Batch A.1/A.2）
-- content 加密（个人思考）；snippet 明文（文档更新后可用于恢复定位）
CREATE TABLE annotations (
    id TEXT PRIMARY KEY,
    item_id TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    offset_start INTEGER NOT NULL,   -- UTF-16 code unit 索引（与 JS String.length 对齐）
    offset_end INTEGER NOT NULL,
    text_snippet TEXT NOT NULL,      -- 明文，用于 fallback 定位
    label TEXT,
    color TEXT NOT NULL DEFAULT 'yellow',
    content BLOB,                     -- AES-256-GCM 加密
    source TEXT NOT NULL DEFAULT 'user' CHECK(source IN ('user', 'ai')),
    created_at TEXT,
    updated_at TEXT
);

-- Chunk 摘要缓存（Batch B.1）
-- 首次摘要 💰 LLM 成本；命中后 🆓 永久复用
CREATE TABLE chunk_summaries (
    chunk_hash TEXT NOT NULL,          -- sha256(chunk_text) hex
    strategy TEXT NOT NULL CHECK(strategy IN ('economical','accurate')),
    item_id TEXT NOT NULL,             -- 冗余存，用于 soft-delete 级联
    model TEXT NOT NULL,
    summary BLOB NOT NULL,             -- AES-256-GCM 加密
    orig_chars INTEGER NOT NULL,
    created_at TEXT,
    PRIMARY KEY (chunk_hash, strategy)
);
```

`PRAGMA journal_mode=WAL` + `PRAGMA foreign_keys=ON` + `PRAGMA busy_timeout=5000`。

**软删除级联**：`delete_item` 把 `items.is_deleted` 置 1 后**显式**`DELETE FROM annotations WHERE item_id = ?` 和 `DELETE FROM chunk_summaries WHERE item_id = ?`。items 是软删除，FK `ON DELETE CASCADE` 永不触发，必须应用层级联。

## 批注 + AI 分析（Batch A）

### 数据模型

见上表 `annotations`。关键设计：

- **字符偏移 + snippet 双锚点**：`offset_start` / `offset_end` 是首选；文档内容更新导致 offset 失配时，可用 `text_snippet` 字符串搜索恢复（Batch A.1 实现 primary，恢复逻辑在未来批）
- **UTF-16 code unit 索引**：与前端 JS `String.length` 对齐。后端 Rust 用 `char.len_utf16()` 累积
- **source = 状态而非分类**：user（默认）/ ai。用户手动 PATCH 不传 `source` 时强制回到 `user`（人类介入抹掉 AI 标记）
- **5 个预设 user 标签 + 4 个 AI 角度**：user 用 ⭐重点 / 📍待深入 / 🤔存疑 / ❓不懂 / 🗑过时；AI 用 ⭐ 要点 / 🤔 疑点 / ⚠️ 风险 / 🕰 过时

### AI 批注生成（`attune_core::ai_annotator`）

LLM 返回 JSON `{"findings":[{"snippet":"...", "reason":"..."}]}`，backend 在原文里做**三阶段 snippet 定位**：

1. Phase 1 **verbatim**：`content.find(snippet)`
2. Phase 2 **relaxed**：归一化空白 + 全角半角标点 → 再搜
3. Phase 3 **prefix-anchor**：snippet ≥20 字时用前 10 字作 anchor 定位，段落边界（`\n\n`）截断防越界

解析容错：提取 `{` 到 `}` 区间 → 整体解析失败时栈扫描 `{...}` pairs 逐个尝试（对抗 LLM JSON 截断）。字段 alias 兼容：`snippet` / `snpshot` / `text` / `quote` 都接收。

## 上下文压缩 + Token Chip（Batch B）

### 流水线（chat route 内）

```
RAG 检索 → allocate_budget
         ↓
批注加权（annotation_weight）
  · 🆓 零成本：仅 DB 读 + 算数
  · label 精确白名单匹配（子串匹配有 "非过时" footgun，已修）
  · 🗑/🕰 过时 → Drop
  · ⭐/要点/风险 → ×1.5
  · 🤔/📍/❓/疑点 → ×1.2
  · 多批注取 MAX 不累乘
         ↓
上下文压缩（context_compress）
  · 三阶段锁释放：
    Phase 1（锁）: 查 chunk_summaries cache
    Phase 2（无锁）: 对 miss 调 LLM 生成摘要（可能 15s/chunk）
    Phase 3（锁）: 批量写回新生成的摘要
  · hash 源用全量 content（非 inject_content）—— 后者被 allocate_budget
    按分数截断，每次查询长度不同 → hash 变 → 缓存永不命中
         ↓
Chat LLM 调用 + 返响应
  · 响应含 weight_stats + compression_stats 供 UI token chip 展开
```

### Strategy

- `raw` — 原文透传，纯本地模式推荐（免费）
- `economical`（默认）— ~150 字摘要；云端模式节省 70-85% token
- `accurate` — ~300 字摘要 + 原文前 100 字，长文场景

### Token 估算（Rust + JS 双侧镜像）

CJK 按 **1.2 tok/char**、ASCII 按 **0.25 tok/char**。这是 BPE 实测校正值 —— 旧估算 0.75/CJK 会让云端账单比 chip 显示多 2× 的惊吓。

### 成本 / 触发契约

项目级最高优先原则（见 CLAUDE.md "成本感知与触发契约"）：

| 层级 | 资源 | 触发策略 | 本批次的例子 |
|------|------|---------|-------------|
| 🆓 零成本 | CPU / 毫秒 | 随便跑 | 批注加权、cache 命中、OCR tesseract |
| ⚡ 本地算力 | GPU / 秒 | 建库阶段后台跑 | embedding、首次摘要、classify |
| 💰 时间 / 金钱 | LLM / 秒到分钟 | **必须用户显式触发**，永不后台偷跑 | Chat、AI 批注分析、云端 API |

## 测试策略

### 单元测试

每个模块在底部 `#[cfg(test)] mod tests`，使用 `tempfile::TempDir` 隔离：

```rust
fn test_vault() -> (Vault, TempDir) {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("vault.db");
    let config_dir = tmp.path().join("config");
    let vault = Vault::open(&db_path, &config_dir).unwrap();
    (vault, tmp)
}
```

### 集成测试（tests/integration_test.rs）

- `e2e_full_lifecycle` — setup → insert → lock → unlock → verify → change_password → delete
- `e2e_content_encrypted_at_rest` — 验证 SQLite 文件原始字节不含明文（需 `vault.checkpoint()` 刷 WAL）
- `e2e_multiple_items` — 批量插入 10 条，分页查询

> **安全警告：NAS 远程访问必须启用 TLS**
>
> 绑定非 loopback 地址（如 `--host 0.0.0.0`）时，**必须**同时指定 `--tls-cert` 和 `--tls-key`，
> 否则 Bearer token 和加密数据在传输层明文暴露。
>
> ```bash
> # 正确的 NAS 模式启动命令
> attune-server --host 0.0.0.0 --port 18900 \
>   --tls-cert /path/to/cert.pem \
>   --tls-key  /path/to/key.pem
> ```
>
> 服务器在非安全配置下启动时会在日志中打印 `⚠ WARNING` 提醒。

### Smoke test（手动）

```bash
# 启动服务
cargo run --bin attune-server -- --port 18900 &

# 基础链路
curl -s -X POST localhost:18900/api/v1/vault/setup -H "Content-Type: application/json" -d '{"password":"test"}'
curl -s -X POST localhost:18900/api/v1/ingest -H "Content-Type: application/json" -d '{"title":"Test","content":"Hello"}'
curl -s "localhost:18900/api/v1/search?q=Hello"
curl -s localhost:18900/api/v1/status

# Web UI
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:18900/
```

## 代码规范

- **rustfmt**: `cargo fmt --all` 强制执行
- **clippy**: `cargo clippy --workspace -- -D warnings` 零警告
- **错误处理**: 所有 public 函数返回 `Result<T, VaultError>`
- **密钥处理**: 32 字节密钥必须用 `Key32` 包装（自动 `ZeroizeOnDrop`）
- **加密数据**: BLOB 列，不得用 TEXT 存储密文
- **测试隔离**: 所有持久化测试使用 `tempfile::TempDir`
- **中文**: 代码中允许中文注释/文档；tantivy 使用 `jieba` tokenizer 支持中文搜索

## Cargo workspace 关键依赖

```toml
# attune-core
argon2 = "0.5"                    # Argon2id 密钥派生
aes-gcm = "0.10"                  # AES-256-GCM 加密
zeroize = { version = "1", features = ["derive"] }
rusqlite = { version = "0.32", features = ["bundled"] }
tantivy = "0.22"                  # 全文搜索
tantivy-jieba = "0.11"            # 中文分词
usearch = "2"                     # 向量索引
walkdir = "2"                     # 目录遍历
notify = "8"                      # 文件监听
reqwest = { version = "0.12", features = ["json"] }  # Ollama HTTP

# attune-server
axum = { version = "0.8", features = ["json", "multipart"] }
tower-http = { version = "0.6", features = ["cors"] }
axum-server = { version = "0.7", features = ["tls-rustls"] }
rustls = "0.23"

# attune-cli
clap = { version = "4", features = ["derive"] }
rpassword = "7"
```

## 跨平台编译

```bash
# Linux x86_64 (default)
cargo build --release

# Windows x86_64 (from Linux)
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu

# Android aarch64 (需要 cargo-ndk)
cargo install cargo-ndk
cargo ndk -t aarch64-linux-android build --release
```

注意：usearch 含 C++ 代码，交叉编译 Windows/Android 需要对应工具链。

## 常见问题

**Q: unlock 后搜索返回空？**
A: 需要 Ollama 服务在 `http://localhost:11434` 运行，并已 `ollama pull bge-m3`。否则向量搜索降级，只有全文搜索。刚 ingest 的数据需要等 Queue Worker 处理完才会出现在搜索结果中。

**Q: 为何 vault.db 里能看到标题明文？**
A: 设计决策。标题明文允许 LOCKED 状态展示条目列表（无需解锁即可浏览条目名称）。内容和 tags 始终加密。参考 README 的字段级加密策略表。

**Q: 改密码会丢数据吗？**
A: 不会。改密码只重新加密 3 个 DEK（共 96 字节），业务数据（用 DEK 加密）不动。

**Q: Device Secret 和密码的关系？**
A: Argon2id 的输入是 `password ‖ device_secret`，两者缺一不可。密码泄露但 device.key 不在手中时数据仍安全。迁移设备时必须同时带走 vault.db 和 device.key。

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
cargo test --workspace    # 237+ tests 全部通过 (attune-core 210 + attune-server 27)

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
│   │       ├── parser.rs             # 多格式解析 (MD/TXT/PDF/DOCX/HTML/EPUB/XLSX/PPTX/RTF/CSV/图片OCR/音频ASR/代码) + SHA-256 + is_supported()
│   │       ├── embed.rs              # Ollama HTTP client (reqwest)
│   │       ├── index.rs              # tantivy 封装 (jieba tokenizer)
│   │       ├── vectors.rs            # usearch 封装 (HNSW + f16)
│   │       ├── search.rs             # RRF 融合 + 动态预算
│   │       ├── scanner.rs            # 本地目录扫描（LocalFolderConnector 封装）
│   │       ├── scanner_webdav.rs     # WebDAV 扫描（WebDavConnector + ETag 增量 + 加密凭据）
│   │       ├── queue.rs              # Embedding 队列 Worker
│   │       ├── llm.rs                # Ollama chat client (LlmProvider trait + OllamaLlmProvider + MockLlmProvider)
│       ├── taxonomy.rs           # 维度定义 + 插件 YAML 加载 + prompt 构建
│       ├── classifier.rs         # LLM 分类 pipeline (批量 + 容错)
│       ├── clusterer.rs          # HDBSCAN 聚类 + LLM 命名
│       ├── tag_index.rs          # 内存反向索引
│       ├── ocr.rs                # OCR provider trait + PP-OCRv5 + OcrProfile (7 内置场景)
│       ├── asr.rs                # ASR backend trait + whisper.cpp subprocess
│       ├── pii.rs                # PII 检测/脱敏（邮箱/电话/身份证号）
│       ├── ai_annotator.rs       # AI 批注生成 (AiAngle: insight/question/summary/critique)
│       ├── context_compress.rs   # 上下文压缩（超长 history 裁剪）
│       ├── memory_consolidation.rs # episodic memory 聚合 (1d 时间窗口)
│       ├── resource_governor.rs  # 协作式调度 (CPU/RAM/IO 节流)
│       ├── intent_router.rs      # 意图路由（chat/search/project 分流）
│       ├── skill_evolution.rs    # 失败信号 → LLM 扩展词 → 静默生效（SkillClaw 模式）
│       ├── agents/               # 网络搜索自动化 (chromiumoxide)
│       └── workflow/             # 工作流引擎框架
│   └── assets/plugins/
│       └── (行业 yaml 已移至 attune-pro — v0.6.0-rc.2 OSS 边界)
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
│   │           ├── annotations.rs    # /annotations CRUD + AI 批注触发
│   │           ├── projects.rs       # /projects CRUD + 项目关联
│   │           ├── auto_bookmarks.rs # /auto-bookmarks (行为驱动推荐)
│   │           ├── ai_stack.rs       # /ai-stack (LLM 提供商配置)
│   │           ├── member.rs         # /member (会员状态/配额)
│   │           ├── audit.rs          # /audit-log (操作日志)
│   │           ├── privacy.rs        # /privacy (PII 扫描/脱敏)
│   │           ├── ocr_profiles.rs   # /ocr/profiles CRUD
│   │           └── ui.rs             # Web UI 页面 (8 标签页 + Settings 模态 + Reader 模态)
│   │
│   ├── attune-cli/                    # bin crate
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── main.rs               # clap: setup/unlock/lock/insert/get/list/status
│   │
│   └── attune-tauri/                  # bin (脚手架模板，已由 apps/attune-desktop 取代)
│       ├── README.md
│       ├── Cargo.toml.template
│       └── src/main.rs.template      # 模板仅供参考
│
└── tests/
    └── integration_test.rs           # 端到端集成测试

# 桌面应用（独立 Cargo workspace）
apps/attune-desktop/
├── Cargo.toml                        # 独立 workspace，依赖 reqwest 0.12 (rustls-tls + multipart + json)
├── tauri.conf.json
├── src/
│   ├── main.rs                       # Tauri 入口：spawn 内嵌 server + 系统托盘 + upload_dropped_paths command
│   ├── embedded_server.rs            # 子进程启动 attune-server-headless --port 18900
│   └── tray.rs                       # 系统托盘菜单（Show/Hide/Quit）
└── resources/                        # 打包资源（server 二进制等）
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
│  └── Routes: 70+ endpoints (38 route files)       │
├─────────────────────────────────────────────────┤
│  Core Engine (Rust lib)          [attune-core]    │
│  ├── Vault    — 状态机 + DEK 管理 + session       │
│  ├── Crypto   — Argon2id + AES-256-GCM + HMAC    │
│  ├── Store    — rusqlite + 字段级加密            │
│  ├── Index    — tantivy + jieba-rs               │
│  ├── Vectors  — usearch HNSW                     │
│  ├── Search   — RRF 融合 + allocate_budget       │
│  ├── Ingest   — 统一入库 pipeline（SourceConnector + ingest_document）│
│  ├── Chunker  — 滑动窗口 + extract_sections      │
│  ├── Parser   — PDF/DOCX/HTML/EPUB/XLSX/CSV/PPTX/RTF/图片OCR/音频ASR/代码 │
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

## 采集体系（Ingest）

所有入库路径统一走 `attune-core::ingest`：

- **`SourceConnector` trait** — 抽象一个采集源（本地文件夹 / WebDAV / Email IMAP / RSS），
  通过回调 sink 逐个交出 `RawDocument`（含 `domain` / `tags` / `corpus_domain` 字段）。
- **`ingest_document()`** — 唯一入库函数：parse → content_hash 判重 → insert_item（透传
  domain/tags）→ breadcrumbs sidecar → enqueue_embedding（L1 章节 + L2 段落块，
  corpus_domain 注入 `[领域: X]` 前缀）→ set_item_corpus_domain → enqueue_classify，
  返回 `IngestOutcome`（Inserted / Duplicate / Updated / Skipped）。

新增采集源只需实现 `SourceConnector`，不修改 pipeline 内部。
HTTP API（`/api/v1/upload`、`/api/v1/ingest`、`/api/v1/index/*`）对外行为不变。

### 本地文件夹扫描

```
POST /api/v1/index/bind { path, recursive, file_types }
  → bind_directory() → dir_id
  → LocalFolderConnector::run(sink)
    ├── WalkDir → 过滤 file_types
    ├── SHA-256 hash 比对 indexed_files（未变 → Skipped）
    ├── 变更 → delete_item(旧) + enqueue purge
    └── ingest_document_replacing() → Updated / Inserted
  → update_dir_last_scan(dir_id)
```

**只读保证**：`std::fs::File::open(Read)`，永不写入源文件。

### WebDAV 远程目录

WebDAV remote 配置（含加密凭据）持久化在 `webdav_remotes` 表，`password` 走字段级 AES-256-GCM。

```
POST /api/v1/index/bind-remote { url, username, password, ... }
  → upsert_webdav_remote()（password AES-GCM 加密落库）
  → WebDavConnector::run(sink)
    ├── PROPFIND 列目录 → 过滤扩展名
    ├── ETag 比对（未变 → Skipped）
    ├── GET 下载文件字节
    └── ingest_document() / ingest_document_replacing()
```

后台周期同步 worker 每 15 分钟读回凭据，对所有 WebDAV remote 自动增量重扫。

### Email IMAP 采集

Email 账号配置（含加密凭据）持久化在 `email_accounts` 表，`password` 走字段级 AES-256-GCM。

```
GET    /api/v1/index/email-accounts              → 列出已绑定账号
POST   /api/v1/index/bind-email { host, port, username, password, ... }
         → upsert_email_account()（password AES-GCM 加密落库）
         → 首次触发 sync_email_account()
DELETE /api/v1/index/email-accounts/{dir_id}     → 删除账号
POST   /api/v1/index/email-accounts/{dir_id}/sync → 手动触发增量同步

sync_email_account() 内部流程：
  → EmailConnector::fetch_documents(sink)  // 实现 SourceConnector trait
    ├── IMAP LOGIN → 遍历配置文件夹
    ├── UID 比对 since_uid 游标（已索引 → Skipped）
    ├── FETCH 邮件正文（HTML → text strip + plain text）
    ├── 解析附件（支持 Parser 全部格式，逐一 ingest_document）
    └── ingest_document()（邮件正文 + 附件各自入库）
```

后台周期同步 worker 每 15 分钟读回凭据，对所有 Email IMAP 账号自动增量拉取（UID 游标）。

**两层入队**：Level 1（章节）+ Level 2（512 字段落块），向量索引时 metadata 区分层级。

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

## J 系列 RAG Production Quality（W2 batch 1，2026-04-27）

详见 [`docs/superpowers/specs/2026-04-27-w2-rag-quality-batch1-design.md`](../docs/superpowers/specs/2026-04-27-w2-rag-quality-batch1-design.md)。所有抄袭来源登记在 [`ACKNOWLEDGMENTS.md`](../ACKNOWLEDGMENTS.md)。

**3 个核心改造**：

```rust
// J1：chunker 输出带面包屑路径，注入 chunk 文本前
let sections = extract_sections_with_path(content);
for s in &sections {
    let prefixed = s.with_breadcrumb_prefix();  // "> A > B > C\n\n[content]"
    indexer.embed(&prefixed);
}

// J3：search 路径分流 — chat 用 RAG 默认（0.65），通用 search 不过滤
let rag_params = SearchParams::with_defaults_for_rag(5);   // min_score=Some(0.65)
let general_params = SearchParams::with_defaults(5);        // min_score=None

// J5：confidence 解析 + 二次检索（CRAG ambiguous 分支）
let response_1 = run_llm_once(...);
let conf_1 = parse_confidence(&response_1);   // 末尾【置信度: N/5】
if conf_1 < 3 {
    let broader = search(query, Some(0.55));   // 降阈值二次召回
    let response_2 = run_llm_once_with(broader);
}
let display = strip_confidence_marker(&response);  // 用户看不到 marker
```

**B1 后端字段（W3 batch A 已透传真值）**：`Citation.chunk_offset_start/end` + `breadcrumb` 由 indexer 写入 `chunk_breadcrumbs` sidecar 表，search 时 join 填到 `SearchResult` → ChatEngine 映射到 `Citation`。空数据（老 vault / web 来源）优雅降级为 `None` / `vec![]`，serde `skip_serializing_if` 让空字段不出现在 JSON 保持 Chrome 扩展旧客户端契约。Offset 当前是 sidecar 累计 char count（v1 启发式），W5+ 真正按行号映射回原文。

**Sidecar 表 6 步检查清单**（`chunk_breadcrumbs` / `chunk_summaries` / `annotations` 同模式）：
1. `mod.rs` schema 加 `FOREIGN KEY (item_id) REFERENCES items(id) ON DELETE CASCADE`
2. 独立 `store/<table>.rs` 模块写 CRUD（注意 dek 加密敏感字段）
3. 在 `store/items.rs::delete_item` 显式 `DELETE FROM <table> WHERE item_id = ?1`（软删除路径，FK CASCADE 仅硬删除生效）
4. 单元测试覆盖 `fk_cascade_*` + `soft_delete_clears_*` 双场景
5. ingest pipeline（`ingest_document` 统一入口，覆盖 upload / ingest / scanner / webdav 所有路径）写入时同步调 `upsert_*`，错误用 `tracing::warn!` 不阻塞主流程
6. ChatEngine 等读路径优雅降级（无 sidecar 行返回 None / 空 Vec）

## W3 batch A：Web search 缓存（C1, 2026-04-27）

详见 [`docs/superpowers/specs/2026-04-27-w3-batch-a-design.md`](../docs/superpowers/specs/2026-04-27-w3-batch-a-design.md) §3。

```rust
// chat.rs web fallback 流程：
let cached = store.get_web_search_cached(dek, query, now_secs)?;
let results = match cached {
    Some(hits) => hits,                                                       // C1 命中（🆓）
    None => {
        let fresh = ws.search(query, 3)?;                                     // 网络（💰）
        let _ = store.put_web_search_cached(dek, query, &fresh, 30天TTL, now); // 含空结果
        fresh
    }
};
```

**抄袭来源**：[吴师兄 §6](https://mp.weixin.qq.com/s/YNcfSN0uv1c1LsLPzgB0jw) 高频 query 缓存模式。

**Attribution 规范**（强制）：
- 每个抄袭外部 pattern 的代码段必须含 `// per <Source> §<Section>` 内联注释
- 每个 PR 合入前必须更新 `ACKNOWLEDGMENTS.md` 对应条目
- Commit message 含 `Inspired-by: <project>(<URL>)` 行

## 资源治理框架（H1, 2026-04-27）

`attune_core::resource_governor` 提供任务级 CPU/RAM/IO 协作式调度。所有常驻后台 worker 必须接入。详见 [`docs/superpowers/specs/2026-04-27-resource-governor-design.md`](../docs/superpowers/specs/2026-04-27-resource-governor-design.md)。

**关键概念**：`cpu_pct_max` 是**系统全局 CPU 阈值**（不是单进程占用上限）— "系统忙就让让"协作式语义。

### 接入新 worker 的 5 步

```rust
use attune_core::resource_governor::{global_registry, TaskKind};

// 1. 在 worker 启动时注册 governor（同 TaskKind 多次返回同一 Arc）
let governor = global_registry().register(TaskKind::EmbeddingQueue);

std::thread::spawn(move || {
    while running.load(Ordering::SeqCst) {
        // 2. 每次循环顶部 check should_run（被 pause 或全局 CPU 超阈值时返回 false）
        if !governor.should_run() {
            std::thread::sleep(Duration::from_millis(500));
            continue;
        }

        match do_work() {
            Ok(_) => {
                // 3. 工作成功后让 governor 决定退让时长（throttle）
                std::thread::sleep(governor.after_work());
            }
            Err(_) => std::thread::sleep(POLL_INTERVAL),
        }

        // 4. （可选）调 LLM 前 check 速率配额
        // if !governor.allow_llm_call() { continue; }

        // 5. 新增 TaskKind 时，需要在 profiles.rs 三档表 + 30 组合 snapshot 测试同步
    }
});
```

**已 retrofit 的 worker**（W1）：`attune-server::state` 中 `start_classify_worker` / `start_rescan_worker` / `start_queue_worker` / `start_skill_evolver`，均参考 `state.rs` 实际代码。

## A1 Memory Consolidation（2026-04-27）

`attune_core::memory_consolidation` 把 `chunk_summaries` 按时间窗口（默认 1 天）聚合成 episodic memory。设计稿：[`docs/superpowers/specs/2026-04-27-memory-consolidation-design.md`](../docs/superpowers/specs/2026-04-27-memory-consolidation-design.md)。

**三阶段 API（与 skill_evolution 同构）**：

```rust
// Phase 1（持 vault 锁）：扫 chunk_summaries → 按天分桶 → 解密 → 过滤已 consolidated
let bundles = prepare_consolidation_cycle(store, dek, now_secs)?;

// Phase 2（无锁）：每 bundle 单独 LLM 调用 — 生产路径必须用 generate_one + 配额 check
for bundle in &bundles {
    if !governor.allow_llm_call() { break; }
    summaries.push(generate_one_episodic_memory(llm, bundle));
}

// Phase 3（重新持 vault 锁 + 复查 unlocked + 重新取 dek）：幂等 INSERT OR IGNORE
apply_consolidation_result(store, &fresh_dek, &bundles, &summaries, model, now_secs)?;
```

**幂等性保证**：唯一索引 `uq_memories_source(kind, source_chunk_hashes)` — 相同 chunk 集合二次跑 `INSERT OR IGNORE` 返回 0 不重复。

**生产 worker**：`attune-server::state::start_memory_consolidator`（6h 周期）。

**MVP 边界**：仅 episodic、不做 chat 检索集成、不做 conflict detection、CHECK 已预放宽支持 semantic 但 W1 不写入。

**测试 helper**：`Store::__test_seed_chunk_summary` 仅在 `#[cfg(any(test, feature = "test-utils"))]` 下编译。`attune-core` 自 dev-dep 启用 `test-utils`，`cargo test` 无需 `--features` 即可跑集成测试。

## 多层记忆（2026-05-18）

`attune_core::memory` 把 A1 的 episodic memory 接入检索路径，新增语义层（L3）+ tier-aware
上下文装配，让 chat 按 query 形态选最便宜的层应答而非永远 dump 原始 chunk。设计稿：
[`docs/superpowers/plans/2026-05-18-multilayer-memory.md`](../docs/superpowers/plans/2026-05-18-multilayer-memory.md)。

**分层**：L0 raw chunks · L1 chunk summaries · L2 episodic（A1，1d 时间窗口）·
L3 semantic（新增，按主题聚类，跨时间）。

**模块**：

- `memory/retrieval.rs` — `MemoryVectorIndex`（专用 usearch 索引，与文档索引分离）+
  `search_memories`：embed query → 向量排序 live（非冷）记忆 → 可选时间窗口过滤。
- `memory/semantic.rs` — L2→L3 三阶段：`prepare_semantic_cycle`（hdbscan 主题聚类 +
  topic_key 幂等过滤）→ `generate_one_semantic_memory`（每簇 1 次 LLM，配额受控）→
  `apply_semantic_result`（`insert_semantic_memory` + 旧 subset 主题 supersede）。
- `memory/assembler.rs` — `classify_query_shape`（recall/overview/precise 零 LLM 启发式）+
  `assemble_context`（coverage gate：记忆层命中弱 / precise → 退回 L0，无回归）+
  `compact_history`（超窗历史滚动摘要，按 `sha256(dropped)` 缓存进 chunk_summaries）。

**数据模型**（全部增量，老 vault 经幂等 ALTER 升级）：
- `memories` 新增 `topic_key`（L3 去重键）/ `cold`（降级标志）/ `superseded_by`（L3 refresh）。
- 新表 `memory_vectors`（embedding sidecar，让 L2/L3 摘要可向量检索）。

**成本契约**：建库阶段不变（tier 1-2）；L2/L3 摘要属 tier 3，由 `MemoryConsolidation`
配额治理；冷降级是纯 SQL（tier 0）；读路径只 *选择已建好的* 记忆，不触发 LLM。

**生产 worker**：`start_memory_consolidator` 在 episodic pass 后跑 `run_memory_layering`
（embed L2/L3 → L2→L3 语义周期 → 冷降级）。`MemoryVectorIndex` 在 unlock 时从
`memory_vectors` 重建。chat 路由经 `assemble_context` 装配；`memory.tiered_assembler_enabled`
（默认 true）控制，关闭即今日 L0 行为。

**设置键**（`app_settings.memory.*`）：`tiered_assembler_enabled` (bool, 默认 true)、
`memory_confidence` (float, 默认 0.70 — coverage gate 阈值)。

## 插件 capability runtime 契约（跨平台分发，2026-05-31）

> SSOT。attune-pro 各 vertical 迁移 wasm 时引用本节。
> spec:`docs/superpowers/specs/2026-05-31-agent-cross-platform-distribution.md`。

attune-core 通过 `capability_dispatch::dispatch_capability(runtime, &invocation)` 统一分流执行
skill/agent,调用方(`agent_runner` / chat handler)**不感知 runtime 差异**,产物统一
`CapabilityResult{exit_code, stdout, stderr, timed_out}`。

### runtime 取值（plugin.yaml 的 skill/agent 条目 `runtime:` 字段）

| runtime | 执行体字段 | 说明 | 跨平台 |
|---------|-----------|------|--------|
| `rust_binary` | `binary: bin/run_<id>` | 现有 subprocess(平台相关 ELF/PE)。native-only cap(OCR/系统调用)保留 | △ 需平台分包 |
| `wasm` | `wasm: wasm/<id>.wasm` | wasm32-wasip1,内嵌 wasmtime 执行。一份 .wasm 通吃所有平台 | ✅ 一包通吃 |
| `data_only` | （无） | 纯 prompt + JSON schema,逻辑全在宿主 LLM lane | ✅ |
| `python_subprocess` | — | **未实现** → dispatch 返回 `unsupported-runtime` Err | — |

### wasm 契约（spec §5.2）

- **输入**:stdin UTF-8 JSON(与 subprocess agent 同 schema:`{"facts":{...},"context":{...}}`)。
- **输出**:stdout UTF-8 JSON(plugin schema 定义的业务输出)。
- **exit code**:`0` 成功 / `1` 一般错误 / `2` 业务红线(`red_lines_violated`)/ `-1` 超时(`timed_out`)。
  wasm 侧用 `proc_exit(N)` 退出码;trap → 宿主映射 exit 1;epoch 超时 → -1。
- **确定性**:金额/数量计算用整数/定点(避免 f64),保证 wasm/native 输出**逐字节一致**(golden diff=0)。

### `wasi_caps` 白名单（spec §5.1）

`wasi_caps: []` 默认 = 纯计算,无 fs/net。可显式声明:`stdio`(默认隐含)/ `clock` /
`read:<host_path>`(只读 preopen)/ `env:<KEY>`(注入某 env)。**默认无 net、无任意 fs 写**;
未知能力字符串 → 加载期拒载。

### 边界硬约束（spec §7,WasmRunner）

每次调用 fresh `Store`(无跨调用状态泄漏);`StoreLimits` 内存上限 256 MB;
epoch deadline 超时(后台 std::thread ticker,不引 tokio);`Engine` 进程级复用(JIT 摊销)。

### `min_attune_version` gate（spec §10）

`plugin.yaml` 顶层可声明 `min_attune_version: "1.1.0"`。`PluginRegistry::scan` 加载期按 semver
校验:不满足 → skip + `[incompatible]` warning;非法 semver → skip + `[invalid-min-version]`。
marketplace 安装链路把匹配本 plugin 的 incompatible warning 转 409 `plugin-incompatible-version`。
**老包无此字段 → `None` → 视为兼容**(向后兼容)。

### 迁移分类（rust_binary → wasm,分批,不阻塞发版）

- **首批迁 wasm**:纯计算确定性 agent(本息/利率/期限计算、案号结构化、专利权利要求抽取)。
- **保留 rust_binary**:依赖 poppler/OCR 预处理、系统 Chrome、重 native 性能的 cap。
- **data_only**:无计算逻辑、逻辑全在 prompt/LLM 的 agent。
- 每迁一个跑 golden diff=0(其 `agent_golden_gate`)才合入。

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

### Office Helper L1/L2 Gates（v0.7.1 起）

Office helper 模块走 **6 类测试金字塔**（per CLAUDE.md 验证铁律 + spec §6）：

```bash
# L1 准入门 (release gate, 默认跑)
cargo test -p attune-server --test office_happy_path --release          # 7 tests (REST contract smoke)
cargo test -p attune-server --test office_error_contract --release      # 10 tests (kebab error codes)
cargo test -p attune-server --test office_schema_compat --release       # 14 tests (tagged union)
cargo test -p attune-server --test office_ocr_golden_gate --release     # 8 tests (accuracy + speed, skip if no images)
cargo test -p attune-server --test office_asr_golden_gate --release     # 10 tests (WER + DER + RTF, skip if no audio)

# L2 稳定性套件
cargo test -p attune-server --test office_concurrent_test --release     # 4 tests (5 OCR + 2 ASR 并发)
cargo test -p attune-server --test office_cancel_test --release         # 6 tests (Cancel 语义 + 409)
cargo test -p attune-server --test office_failure_recovery_test --release # 5 tests (corrupt PDF / 0 字节)
cargo test -p attune-server --test office_prop_tests --release          # 5 proptest invariants

# 单元测试 (attune-core 内)
cargo test -p attune-core ocr::structured                                # 81 unit tests
cargo test -p attune-core office_job_queue                               # 11 unit tests
```

**Golden 数据集**（`crates/attune-server/tests/golden/office/`）:
- OCR 7 个 scene 目录 + ASR 4 个 语言/会议 目录
- Synthetic 样本生成: `python3 scripts/gen-office-ocr-golden.py` (GB 11643 / Luhn / GB 32100 合规)
- 公开 ASR 数据集下载: `scripts/fetch-office-asr-golden.sh` (LibriSpeech test-clean)
- 内部脱敏样本: 用户手工补 `<id>.png` + `<id>.expected.yaml` 配对，无图样本测试自动 SKIP
- 准确度/速度红线 + 测试基线: [`tests/golden/office/BASELINE_ENV.md`](crates/attune-server/tests/golden/office/BASELINE_ENV.md)

**ENFORCE mode**（六类覆盖门 — D5.5 起）:
```bash
ATTUNE_ENFORCE_OFFICE_FLOOR=1 cargo test -p attune-server --test office_six_category_floor --release
```
强制每个 scene 至少: 5 approved YAML + 3 error case + 3 proptest + 5 boundary + 1 integration. 0 violations 才能 tag GA.

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

# npu-vault

**本地优先、端到端加密的个人知识库引擎。** 跨 Linux / Windows / NAS（HTTPS 远程），通过 Chrome 扩展、本地文件扫描、文件上传自动积累知识，让云端 AI 更懂你。

单一静态 Rust 二进制，零运行时依赖，28 MB 含完整 Web UI、TLS 和加密搜索引擎。

## 功能

- **1Password 式加密** — Master Password + Device Secret → Argon2id(64MB/3轮) → Master Key → 三把 DEK（数据库/全文索引/向量），AES-256-GCM 字段级加密
- **层级语义搜索** — tantivy BM25（中文 jieba 分词）+ usearch HNSW 向量检索 + RRF 融合 + 动态注入预算 2000 字
- **本地文件扫描** — walkdir 全量 + notify 实时增量，SHA-256 hash 变更检测，只读源文件
- **文件直传 API** — multipart 上传 MD / TXT / 代码，自动解析 → 两层分块 → 加密存储 → 入队 embedding
- **Embedding 队列 Worker** — 后台异步通过 Ollama HTTP API 批量 embedding，结果写入 tantivy + usearch
- **NAS 模式** — `--host 0.0.0.0` + rustls TLS + Bearer token 认证，手机浏览器通过 HTTPS Web UI 访问
- **嵌入式 Web UI** — 单页 HTML + vanilla JS，`include_str!` 编译进二进制，响应式移动友好
- **Device Secret 迁移** — 导出/导入 API 支持多设备同步（类似 1Password Secret Key）
- **Chrome 扩展兼容** — 18 个 API 端点全覆盖，扩展改后端地址即可切换（Python 原型 → Rust 商用）
- **跨平台** — 纯 Rust 栈，交叉编译到 x86_64-linux / x86_64-windows / aarch64-android
- **AI 自动分类** — 基于 Ollama 本地 LLM（qwen2.5 等）对每条知识生成 5 核心维度 + 3 通用维度标签，支持编程/法律等行业插件
- **HDBSCAN 智能聚类** — 自动发现知识主题群组，LLM 命名，类似相册的"回忆"功能
- **标签直方图 + 浏览** — Web UI 按维度筛选，点击标签查看所有匹配条目
- **行为画像** — 搜索历史 + 点击追踪 + 热门条目统计
- **画像导出/导入** — `.vault-profile` JSON 格式，支持跨设备迁移
- **WebDAV 远程目录** — 绑定 NAS WebDAV 目录，PROPFIND 列表 + 自动下载入库
- **运行时插件加载** — 用户可在 `{config_dir}/plugins/*.yaml` 放置自定义行业插件

## 快速开始

### 1. 构建

```bash
cd npu-vault
cargo build --release
# 产物：
# target/release/npu-vault         (CLI, 4.1 MB)
# target/release/npu-vault-server  (HTTP Server, 26 MB)
```

### 2. 启动 Ollama（可选，用于语义搜索）

```bash
curl -fsSL https://ollama.com/install.sh | sh
ollama pull bge-m3
```

无 Ollama 时降级为纯全文搜索（tantivy BM25）。

### 3. CLI 模式

```bash
./target/release/npu-vault setup              # 首次设置主密码
./target/release/npu-vault unlock              # 解锁 vault
./target/release/npu-vault insert -t "标题" -c "内容"
./target/release/npu-vault list -l 20
./target/release/npu-vault get <item_id>
./target/release/npu-vault status              # JSON 状态
./target/release/npu-vault lock
```

### 4. HTTP Server 模式

```bash
./target/release/npu-vault-server --port 18900
# 浏览器打开 http://localhost:18900/ 使用 Web UI
# Chrome 扩展改后端地址到 http://localhost:18900 即可对接
```

### 5. NAS 模式（远程 HTTPS + 认证）

```bash
# 生成自签名证书
openssl req -x509 -newkey rsa:2048 \
  -keyout key.pem -out cert.pem \
  -days 365 -nodes -subj "/CN=your-nas.local"

# 启动 HTTPS + Bearer 认证
./target/release/npu-vault-server \
  --host 0.0.0.0 \
  --port 18900 \
  --tls-cert cert.pem \
  --tls-key key.pem \
  --require-auth

# 手机浏览器: https://your-nas.local:18900/
# API 请求需要: Authorization: Bearer <session_token>
```

**NAS 模式必须启用 TLS**：远程访问时请加上 `--tls-cert` 和 `--tls-key` 参数，
否则服务器会在启动日志中输出安全警告。

## 安全模型

### 密钥体系

```
Master Password (用户记忆)  +  Device Secret (设备文件, 256-bit 随机)
                │                       │
                └───────────┬───────────┘
                            ↓
                Argon2id(m=64MB, t=3, p=4)
                → 32-byte Master Key (MK)
                            │
                    ┌───────┼────────┐
                    ↓       ↓        ↓
                  DEK_db  DEK_idx  DEK_vec
```

- **Master Password** — 用户记忆，不落盘
- **Device Secret** — 256-bit 随机，首次 setup 时生成于 `{config_dir}/device.key`（权限 0600）。迁移新设备时需导出
- **Argon2id 参数** — 64 MB 内存、3 轮迭代、4 线程，抗 GPU/ASIC 暴力破解
- **三把 DEK** — 分别加密 SQLite 数据、tantivy 全文索引、usearch 向量文件。改密码只需重新加密 96 字节 DEK，不碰业务数据

### 字段级加密策略

| 字段 | 加密 | 理由 |
|------|------|------|
| `id`, `created_at`, `source_type`, `url`, `domain` | 明文 | 列表/过滤不需解锁 |
| `title` | 明文 | LOCKED 状态下可展示条目名（同 1Password）|
| `content`, `tags`, `metadata` | AES-256-GCM (DEK_db) | 核心敏感数据 |
| tantivy 索引 | 内存持有（DEK_idx 预留）| 全文索引等同明文 |
| usearch 向量 | 文件级加密（DEK_vec 预留）| 向量可反推原文 |

每个加密字段独立 96-bit 随机 nonce，存储格式 `nonce(12B) ‖ ciphertext ‖ tag(16B)`。

### Vault 状态机

```
           ┌─────────┐
 init ──→  │ SEALED  │    (首次运行，无密码)
           └────┬────┘
                │ setup(password) → 生成 device.key + salt + DEK×3
                ↓
           ┌─────────┐
 lock() ─→ │ LOCKED  │ ←── 4h timeout / 手动锁定
           └────┬────┘
                │ unlock(password) → 派生 MK → 解密 DEK → 签发 session token
                ↓
           ┌──────────┐
           │ UNLOCKED │ → 所有 API 可用
           └──────────┘
```

- **Session Token**: HMAC-SHA256(session_id + expires, MK)，4 小时 TTL，携带于 `Authorization: Bearer <token>`
- **Zeroize**: `Key32` 实现 `ZeroizeOnDrop`，lock 时所有密钥从内存抹除

## API 端点

所有端点前缀 `/api/v1/`，localhost 访问免认证，远程需 `--require-auth` + Bearer token。

### Vault 管理

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/vault/status` | vault 状态 (sealed/locked/unlocked) + 条目数 |
| POST | `/vault/setup` | 首次设置密码 |
| POST | `/vault/unlock` | 解锁 vault，返回 session token |
| POST | `/vault/lock` | 手动锁定（清零内存密钥）|
| POST | `/vault/change-password` | 修改主密码（重新加密 DEK）|
| GET | `/vault/device-secret/export` | 导出 device secret（迁移用）|
| POST | `/vault/device-secret/import` | 导入 device secret（新设备）|

### 知识操作

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/ingest` | 知识注入（纯文本 JSON）|
| POST | `/upload` | 文件直传（multipart）|
| GET | `/search?q=&top_k=` | 混合搜索（BM25 + 向量 + RRF）|
| POST | `/search/relevant` | 相关检索（含动态注入预算）|
| GET | `/items?limit=&offset=` | 列出条目 |
| GET | `/items/{id}` | 获取单个条目（解密）|
| PATCH | `/items/{id}` | 更新条目 |
| DELETE | `/items/{id}` | 软删除 |

### 索引与系统

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/index/bind` | 绑定本地目录 + 触发全量扫描 |
| DELETE | `/index/unbind?dir_id=` | 解绑目录 |
| GET | `/index/status` | 绑定目录列表 + pending embedding 数 |
| GET | `/status` | 系统完整状态（含搜索引擎就绪情况）|
| GET | `/status/health` | 健康检查（无需解锁）|
| GET | `/settings` | 获取设置 JSON |
| PATCH | `/settings` | 更新设置（合并语义）|

### 分类与聚类

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/classify/{id}` | 单条重分类（同步，LLM 调用）|
| POST | `/classify/rebuild` | 全量重分类（异步，入队）|
| GET | `/classify/status` | 分类状态（model + pending + classified）|
| GET | `/tags` | 所有维度的直方图（排除 entities）|
| GET | `/tags/{dimension}` | 单维度完整直方图 |
| GET | `/clusters` | 当前聚类快照 |
| GET | `/clusters/{id}` | 单聚类详情 |
| POST | `/clusters/rebuild` | 触发聚类重建 |
| GET | `/plugins` | 列出可用的行业插件 |

### 行为画像

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/behavior/click` | 记录知识条目点击 |
| GET | `/behavior/history` | 最近搜索历史（加密存储） |
| GET | `/behavior/popular` | 热门点击排行 |

### 画像迁移

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/profile/export` | 导出分类结果 + 聚类 + 直方图 |
| POST | `/profile/import` | 导入画像 JSON（合并已有条目） |

### 远程目录

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/index/bind-remote` | 绑定 WebDAV 远程目录并扫描 |

### 分类队列

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/classify/drain` | 手动处理一批分类任务 |

### Web UI

| 路径 | 说明 |
|------|------|
| GET `/` | 嵌入式单页 Web UI（HTML + vanilla JS）|
| GET `/ui` | 同上，别名 |

Web UI 功能：setup / unlock / lock、搜索、录入、条目列表、Device Secret 导出、锁定。

## 数据存储

| 数据 | Linux | Windows |
|------|-------|---------|
| 加密数据库 | `~/.local/share/npu-vault/vault.db` | `%LOCALAPPDATA%\npu-vault\vault.db` |
| Device Secret | `~/.config/npu-vault/device.key` | `%APPDATA%\npu-vault\device.key` |
| 配置 | `~/.config/npu-vault/` | `%APPDATA%\npu-vault\` |

**迁移到新设备**：
1. 备份 `vault.db`（加密，复制即可）
2. 导出 `device.key`（通过 API 或直接复制）
3. 新设备上部署二进制 + 导入 `device.key` + 用原密码 `unlock`

## 二进制

| Binary | 大小 | 用途 |
|--------|------|------|
| npu-vault | 4.2 MB | CLI 管理工具（7 个子命令）|
| npu-vault-server | 28 MB | HTTP API Server（TLS + Web UI + 搜索引擎）|

大小构成：rustls 密码学 + tantivy 全文 + usearch C++ binding + Tokio + Axum。可通过 `strip=true` + `panic=abort` 进一步压缩。

## 测试

```bash
cargo test --workspace    # 120 tests (114 单元 + 6 集成)
```

| 模块 | 测试数 | 覆盖 |
|------|--------|------|
| crypto | 8 | Argon2id, AES-GCM, HMAC, zeroize |
| vault | 16 | 状态机, setup/unlock/lock, session token, change_password, device secret 导出/导入 |
| store | 9 | SQLite schema, 加密 CRUD, FTS 准备 |
| chunker | 6 | 滑动窗口, 章节切割, 中文处理 |
| parser | 6 | MD/TXT/代码解析, SHA-256, bytes 解析 |
| embed | 2 | OllamaProvider 创建, NoopProvider 降级 |
| index | 4 | tantivy 持久化, BM25, jieba 分词 |
| vectors | 5 | usearch HNSW, 增删, save/load |
| search | 5 | RRF 融合, 动态预算 |
| scanner | 5 | 全量扫描, 增量, hash 比对 |
| queue | 2 | Worker 生命周期, NoopProvider 处理 |
| platform + error | 4 | 跨平台路径, 错误类型 |
| llm | 3 | OllamaLlmProvider, MockLlmProvider |
| taxonomy | 6 | 插件 YAML 解析, prompt 构建, validate |
| classifier | 5 | MockLlmProvider 驱动, 解析, 容错 |
| clusterer | 4 | 最小阈值, 序列化, LLM 命名 |
| tag_index | 7 | build, query_and/or, upsert, histogram |
| store 扩展 | 3 | task_type 迁移, update_tags, list_all_item_ids |
| 集成测试 | 3 | 全生命周期, 静态加密, 批量操作 |
| scanner_webdav | 3 | WebDAV PROPFIND XML 解析, 集合过滤 |
| store (behavior) | 3 | 搜索历史加密, 点击统计, 热门排行 |
| crypto (file) | 2 | 加密文件保存/加载, 缺失文件处理 |
| vectors (encrypted) | 1 | save/load_encrypted 往返 |
| taxonomy (user plugins) | 2 | 空目录, YAML 解析 |
| classifier_test (集成) | 3 | e2e 分类 / 重分类 / 锁解锁持久化 |

## 项目结构

```
npu-vault/
├── Cargo.toml                    # workspace
├── crates/
│   ├── vault-core/               # lib: 加密/存储/搜索/扫描（19 模块）
│   ├── vault-server/             # bin: Axum HTTP API + Web UI
│   └── vault-cli/                # bin: 命令行工具
└── tests/                        # 集成测试
```

## Phase 计划

- **Phase 1** ✅ 加密存储引擎 (vault-core + vault-cli)
- **Phase 2a** ✅ Axum API Server + tantivy + usearch + RRF 混合搜索
- **Phase 2b** ✅ 文件扫描 + Embedding 队列 + Upload + Index API
- **搜索集成** ✅ AppState 持有 FulltextIndex/VectorIndex/Ollama，搜索全链路打通
- **Chrome 兼容** ✅ 18 个 API 端点对齐 npu-webhook Python 原型
- **Phase 3** ✅ NAS 模式 (TLS + Bearer) + 嵌入式 Web UI + Device Secret 迁移
- **子系统 A** ✅ AI 自动分类 (qwen2.5 + HDBSCAN + 编程/法律插件 + 最小 UI 集成)
- **子系统 B** ✅ 行为画像（搜索历史 + 点击追踪 + 热门统计）
- **子系统 C** ✅ Web UI MVP（8 标签页：搜索/录入/条目/分类/聚类/远程/历史/设置）
- **子系统 D** ✅ 运行时插件加载（config_dir/plugins/*.yaml）
- **子系统 E** ✅ 画像导出/导入（.vault-profile JSON）
- **F1** ✅ NAS 远程目录（WebDAV PROPFIND + 自动入库）
- **F2** ⏳ Tauri 桌面客户端（脚手架就绪，待 Tauri CLI 激活）
- **F3** ✅ 分类队列 drain（手动触发 /classify/drain）
- **F4** ✅ 索引持久化加密（usearch save/load_encrypted + crypto helpers）

## License

MIT

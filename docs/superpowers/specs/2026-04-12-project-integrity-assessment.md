# npu-webhook 项目完整性评估报告

> 日期: 2026-04-12  
> 状态: 完成  
> 评估范围: Python 原型线 + Rust 商用线（双轨全覆盖）  
> 评估框架: 层次化全维度（战略层 → 功能层 → 横切关注点）  
> 优先级标注: 影响面（P0–P3）× 工作量（S/M/L）

---

## 目录

1. [评估方法](#1-评估方法)
2. [项目现状快照](#2-项目现状快照)
3. [Layer 1：战略层](#3-layer-1战略层)
4. [Layer 2：功能层](#4-layer-2功能层)
5. [Layer 3：横切关注点](#5-layer-3横切关注点)
6. [完整缺口清单](#6-完整缺口清单)
7. [优先级矩阵](#7-优先级矩阵)
8. [快赢建议](#8-快赢建议)

---

## 1. 评估方法

### 评分标准
- **0** — 缺失  
- **1** — 框架/占位符存在  
- **2** — 基本可用  
- **3** — 功能完整  
- **4** — 生产就绪  
- **5** — 超出预期

### 优先级定义
| 级别 | 含义 |
|------|------|
| P0 | 阻塞发布 / 高危安全漏洞 |
| P1 | 重要功能缺失 / 中危安全问题 |
| P2 | 体验问题 / 低危安全隐患 / 功能不完整 |
| P3 | 技术债务 / 优化项 |

### 工作量定义
| 级别 | 估算 |
|------|------|
| S (Small) | < 1 天 |
| M (Medium) | 1–3 天 |
| L (Large) | > 3 天 |

---

## 2. 项目现状快照

### 双轨架构
| 维度 | Python 原型线 | Rust 商用线 |
|------|--------------|------------|
| 版本 | Phase 0–3 完成 | v0.5.0 完成 |
| 测试数 | 78（当前不可运行） | 130（全绿） |
| 代码规模 | ~3,556 行 | ~5,831 行（vault-core） |
| 端点数量 | 26 个（含 3 个占位符） | 30 个 |
| 独有功能 | Reranker、Skill 引擎、硬件检测 | 加密存储、AI 分类、行为画像、聚类、WebDAV |
| 路线图状态 | Phase 4–5 未开始 | v0.6.0–v1.0.0 待实施 |

### 测试健康度
```
Rust vault-core 单元测试：124 个 ✅
Rust 集成测试：         6 个 ✅
Rust 路由层测试：       0 个 ❌
Python 单元+E2E：       78 个 ❌（ModuleNotFoundError）
```

---

## 3. Layer 1：战略层

### 3.1 Python 原型线现状与退出策略

**评分：3/5**（功能稳定但维护成本高）

| 指标 | 现状 |
|------|------|
| 功能完成度 | Phase 0–3 完整；Phase 4（OpenVINO NPU）+ Phase 5（系统集成）未实现 |
| 测试可运行性 | 失效——缺少 `pytest.ini` pythonpath 配置，`ModuleNotFoundError` |
| Python 独有特性 | Reranker 精排、Skill 引擎框架、`/models/check` 硬件检测 |
| 退出阻碍 | 上述 3 项功能尚未迁移到 Rust |

**缺口 S1**：Python 独有特性（Reranker + 硬件检测）未迁移 → P1 / M  
**缺口 S2**：Python 测试环境失效 → P2 / S

### 3.2 Rust 商用线 v0.5.0 → v1.0.0 路线图完成度

| 里程碑 | 状态 | 备注 |
|--------|------|------|
| v0.6.0 Tauri 桌面 | 🔴 0% | 仅有注释模板，缺 `tauri-build`，估计 23–33 小时 |
| v0.7.0 Queue Worker 自启动 + WebSocket | 🟡 部分 | classify worker + rescan worker 已自动启动（`state.rs` 中 unlock 后调用）；**embedding QueueWorker 未自动启动**（ingest 时入队，但无后台消费线程触发）；WebSocket 端点不存在 |
| v0.8.0 云同步 | 🔴 未开始 | — |
| v1.0.0 正式发布 | 🔴 未开始 | CI/CD 矩阵、签名、官网 |

**缺口 S3**：Tauri 桌面 0% 进度，`Cargo.toml.template` 缺 `tauri-build` → P0 / L  
**缺口 S4**：WebSocket 进度推送端点缺失 → P1 / M  
**缺口 S5**：embedding QueueWorker unlock 后是否自动消费待代码确认 → P1 / S

---

## 4. Layer 2：功能层

### 4.1 加密存储引擎

**评分：5/5**（Rust 线）

| 组件 | Python | Rust | 说明 |
|------|--------|------|------|
| 字段级 AES-256-GCM 加密 | ✗ | ✅ | content、tags、query 加密 |
| Argon2id 密钥派生 | ✗ | ✅ | M=64MB / T=3 / P=4，符合 OWASP 2023 |
| DEK 三层管理（db/idx/vec） | ✗ | ✅ | — |
| HMAC-SHA256 会话 token | ✗ | ✅ | 常时间比较，有过期检查 |
| 密码变更（重加密 DEK） | ✗ | ✅ | 业务数据不动 |
| 跨设备迁移（device.key） | ✗ | ✅ | — |

**缺口 F1**：`change_password` 三次 `set_meta()` 无事务包裹，中途失败 DEK 不一致 → P1 / S

### 4.2 混合搜索引擎

**评分：4/5**

| 组件 | Python | Rust | 差异 |
|------|--------|------|------|
| BM25 全文（tantivy/FTS5 + jieba） | ✅ | ✅ | — |
| 向量搜索（usearch HNSW / ChromaDB） | ✅ | ✅ | Rust f16 量化，内存更优 |
| RRF 融合（k=60，权重 0.6/0.4） | ✅ | ✅ | 参数一致 |
| 动态注入预算（`allocate_budget`） | ✅ | ✅ | — |
| Reranker 余弦精排（二次排序） | ✅ | ❌ | Python 独有 |
| LRU 搜索缓存（256 条目） | ✅ | ❌ | Python 独有 |
| Tag / Cluster 过滤 | ❌ | ✅ | Rust 独有 |

**缺口 F2**：Rust 线搜索无 Reranker 精排 → P2 / M  
**缺口 F3**：Rust 线搜索无 LRU 缓存 → P3 / S

### 4.3 文件索引 + Embedding 队列

**评分：4/5**

| 组件 | Python | Rust | 差异 |
|------|--------|------|------|
| watchdog/notify-rs 实时监听 | ✅ | ✅ | — |
| SHA-256 增量去重 | ✅ | ✅ | — |
| 两层入队（Level1 章节 + Level2 块） | ⚠️ schema 有，逻辑未实现 | ✅ | Python 仅有 Level2 |
| Queue Worker 后台线程 | ✅ | ✅ | — |
| Worker unlock 后自动启动 | ✅ lifespan | ❌ 未自动启动 | unlock 只触发 classify/rescan worker |
| WebDAV 远程目录 | ❌ | ✅ | Rust 独有 |

**缺口 F4**：Python Level1 章节入队逻辑实际未实现（只有 schema） → P2 / M  
**缺口 F5**：Rust embedding QueueWorker unlock 后未自动启动，ingest 后 embedding 任务堆积在队列无人消费 → P1 / S

### 4.4 Chrome 扩展兼容协议

**评分：4/5**

核心 API（`/search`、`/ingest`、`/search/relevant`、`/settings`、`/items`、`/status/health`）两条线完全兼容。

**Rust 线缺失的端点（Python 独有）**：
- `GET /items/stale`
- `GET /items/{id}/stats`
- `POST /feedback`
- `GET /models`、`POST /models/check`（硬件检测，Phase 4 功能）
- `GET/POST /skills/*`（Phase 3 功能）

**缺口 F6**：3 个已实现的 Python 独有端点（`/items/stale`、`/items/{id}/stats`、`/feedback`）Rust 未实现 → P2 / M

### 4.5 Web UI（8 标签页）

**评分：4/5**

所有 8 个标签页（搜索/录入/条目/设置/分类/聚类/远程/历史）功能完整。

**缺口 F7**：无 WebSocket 实时进度，扫描/embedding 状态需手动刷新 → P2 / M

### 4.6 AI 分类 + 聚类

**评分：4/5**

LLM 分类、HDBSCAN 聚类、内置插件（编程/法律）、用户自定义 YAML 插件均已实现。

**缺口 F8**：分类队列依赖手动 `POST /classify/drain`，非真正后台自动消费（因 Vault 所有权限制，`Vault` 所有权重构 RELEASE.md v0.7.0 待办） → P2 / L

### 4.7 Chat RAG 子系统

**评分：3/5**

完整的 RAG pipeline（向量+全文搜索 → RRF 融合 → LLM 生成 → 引用返回），但：

**缺口 F9**：`POST /chat` 阻塞等待完整 LLM 回复，无 WebSocket / SSE 流式响应 → P2 / L  
**缺口 F10**：聊天历史复用 `items` 表（`source_type=ai_chat`），无独立会话表、无会话隔离 → P2 / M

### 4.8 Skill 引擎

**评分：1/5**（Python 线）/ **0/5**（Rust 线）

- Python 线：DB schema 就绪，3 个 API 路由均返回 `{"status": "not_implemented"}`，`SkillEngine` 类体仅含 `pass`
- Rust 线：无 Skill 相关实现

**缺口 F11**：Skill 引擎 100% 占位符（Phase 3 未启动） → P2 / L

---

## 5. Layer 3：横切关注点

### 5.1 威胁模型

#### 威胁 T1 — Rust 线 CORS 全开放【P0 / S】
- `main.rs` 使用 `CorsLayer::permissive()`，任意 Origin 均可跨域访问
- UNLOCKED 状态下，恶意网页可通过 JS 读取全部加密内容（含 device.key 导出）
- **修复**：限制为 `chrome-extension://<id>`、`http://localhost`、`http://127.0.0.1`

#### 威胁 T2 — `/device-secret/export` 缺强制 token【P0 / S】
- 该端点为最高敏感端点，但认证依赖全局 `--require-auth` 开关（默认关闭）
- **修复**：该端点独立于全局开关，始终要求 Bearer token

#### 威胁 T3 — Bearer token 默认关闭【P1 / S】
- 默认启动无认证，本地任意进程可无需 token 访问全部 API
- **修复**：默认开启认证，或至少 UNLOCKED 后强制要求 token

#### 威胁 T4 — NAS 模式 TLS 默认关闭【P1 / S】
- NAS 场景下局域网明文传输，同网段可嗅探搜索词和内容片段
- **修复**：NAS 模式将 TLS 设为必选项，文档中强化提示

#### 威胁 T5 — 目录绑定无路径边界验证【P2 / S】
- `POST /index/bind {"path": "/etc"}` 可将系统目录加入索引（Python 和 Rust 均无验证）
- **修复**：绑定目录必须位于用户家目录或配置的白名单路径下

#### 威胁 T6 — `derive_master_key` 中间 Vec 未 Zeroize【P2 / S】
- `crypto.rs` 中 `Vec` 类型的 `input` 变量调用 `drop()` 但非 `Zeroizing<Vec>`，swap/core dump 可能泄露
- **修复**：改用 `zeroize::Zeroizing<Vec<u8>>`

#### 威胁 T7 — Windows device.key ACL 未设置【P2 / M】
- `#[cfg(unix)]` 保护的 `set_permissions(0o600)` 在 Windows 下不生效
- **修复**：Windows 使用 DPAPI 或显式 ACL 设置

#### 威胁 T8 — lock 时 token 无吊销机制【P2 / S】
- lock 操作清除内存密钥，但旧 token 字符串若被截获仍可通过 HMAC 验证
- **修复**：lock 时递增 token 版本号（存入 vault_meta）

#### 威胁 T9 — Python token 比较非恒定时间【P3 / S】
- `main.py` 使用 `token != settings.auth.token`（Python `!=` 非 constant-time）
- **修复**：改用 `hmac.compare_digest(token, settings.auth.token)`

#### 威胁 T10 — AES-GCM nonce 理论碰撞【P3 / M】
- 12 字节随机 nonce 在同一 DEK 下，个人知识库体量实际风险可忽略
- **修复**：如需规范化，升级为 AES-256-GCM-SIV 或计数器 nonce

#### 威胁 T11 — CSP 未显式声明【P3 / S】
- `manifest.json` 无 `content_security_policy` 字段，依赖 MV3 默认策略
- **修复**：显式声明 `"content_security_policy": {"extension_pages": "script-src 'self'; ..."}` 

### 5.2 测试覆盖率

| 测试层 | 数量 | 状态 | 覆盖盲区 |
|--------|------|------|----------|
| Rust vault-core 单元测试 | 124 | ✅ 全绿 | `store.rs` 12 个 pub 函数无测试 |
| Rust 集成测试 | 6 | ✅ 全绿 | — |
| Rust vault-server 路由层 | **0** | ❌ | 42 个 handler 全无测试 |
| Rust vault-cli | 0 | 🟡 可接受 | CLI 无副作用，手动验证即可 |
| Python 单元 + E2E | 78 | ❌ 不可运行 | ModuleNotFoundError |

**store.rs 无测试的 pub 函数**（12 个）：
`bind_directory`、`unbind_directory`、`list_bound_directories`、`update_dir_last_scan`、`get_indexed_file`、`upsert_indexed_file`、`enqueue_embedding`、`dequeue_embeddings`、`mark_embedding_done`、`mark_embedding_failed`、`mark_task_pending`、`checkpoint`

**缺口 Q1**：vault-server 路由层 42 个 handler 零测试 → P1 / L  
**缺口 Q2**：Python 测试环境失效（需添加 `pytest.ini` pythonpath 配置） → P2 / S  
**缺口 Q3**：`store.rs` 12 个目录/队列 pub 函数无测试 → P2 / M

### 5.3 代码健壮性

**缺口 Q4 — 420 处 `.unwrap()` / 12 处 `.expect()` 潜在 panic**

高危点：
| 位置 | 触发条件 | 后果 |
|------|----------|------|
| `platform.rs:4,9` | 容器/沙箱环境无家目录 | 服务启动崩溃 |
| `main.rs:40` | vault 数据库文件损坏 | 无提示崩溃 |
| `main.rs:112` | TLS 证书路径不存在 | 无提示崩溃 |
| 大量 `Mutex::lock().unwrap()` | 任意锁内 panic → 锁中毒 | 整个服务不可恢复 |

影响面 P1 / 工作量 M

### 5.4 可扩展性

**缺口 Q5 — 全量内存索引，10 万条目约需 1.2 GB**

| 组件 | 10 万条估算 |
|------|------------|
| usearch（1024 维 f16） | ~220 MB |
| tantivy 全文索引 | ~500–600 MB |
| 合计 | **~800 MB – 1.2 GB** |

- 重启时 usearch 需解密+解压恢复（耗时），tantivy 直接从磁盘加载（快）
- 个人库（数千条）无风险；NAS 多用户场景需关注
- 影响面 P3 / 工作量 L

### 5.5 文档一致性

5 个已修改文件（`CLAUDE.md`、`DEVELOP.md`、`README.md`、`RELEASE.md`、`extension/src/background/worker.js`、`extension/src/content/index.js`）长期未提交，git 历史断层。

`npu-vault/RELEASE.md` 声称 120 tests，实际已达 130。

**缺口 Q6**：未提交文件 + 文档版本号滞后 → P2 / S

### 5.6 依赖安全

| 结论 | 说明 |
|------|------|
| ✅ 无已知 CVE | 所有 crates.io 依赖版本正常（2026-04-12 检查） |
| ✅ 无 git/path 外部依赖 | 供应链干净 |
| ⚠️ `tokio features = ["full"]` 过度配置 | 增加二进制体积，建议精简 |
| ⚠️ `pdf-extract` 中风险 | 潜在 OCR buffer overflow，非热路径 |
| ⚠️ 无 `cargo audit` CI | 新 CVE 无法自动检测 |

**缺口 Q7**：无 `cargo audit` CI 步骤 → P3 / S

---

## 6. 完整缺口清单

| 编号 | 缺口描述 | 影响面 | 工作量 |
|------|----------|--------|--------|
| T1 | CORS 全开放（Rust 线，`CorsLayer::permissive()`） | **P0** | S |
| T2 | `/device-secret/export` 缺强制 token 保护 | **P0** | S |
| S3 | Tauri 桌面 0% 实现（`Cargo.toml.template` 缺 `tauri-build`） | **P0** | L |
| F1 | `change_password` 无事务，DEK 可不一致（数据损坏风险） | **P1** | S |
| T3 | Bearer token 默认关闭 | **P1** | S |
| T4 | NAS 模式 TLS 默认关闭 | **P1** | S |
| F5 | embedding QueueWorker unlock 后未自动启动，ingest 后 embedding 队列无人消费 | **P1** | S |
| Q1 | vault-server 路由层 42 个 handler 零测试 | **P1** | L |
| Q4 | 锁中毒 + platform 家目录 panic 风险（420 处 unwrap） | **P1** | M |
| S1 | Python 独有特性未迁移（Reranker、硬件检测） | **P1** | M |
| S4 | WebSocket 进度推送端点缺失（扫描/embedding） | **P1** | M |
| T5 | 目录绑定无路径边界验证（可索引 `/etc`） | **P2** | S |
| T6 | `derive_master_key` 中间 Vec 未 Zeroize | **P2** | S |
| T8 | lock 时 token 无吊销机制 | **P2** | S |
| Q2 | Python 测试环境失效（`ModuleNotFoundError`） | **P2** | S |
| Q6 | 5+ 个已修改文件未提交，RELEASE.md 版本号滞后 | **P2** | S |
| T7 | Windows device.key ACL 未设置 | **P2** | M |
| F2 | Rust 搜索无 Reranker 精排 | **P2** | M |
| F4 | Python Level1 章节入队逻辑未实现（schema 有，代码无） | **P2** | M |
| F6 | 3 个已实现 Python 端点 Rust 未实现（`/items/stale` 等） | **P2** | M |
| F7 | Web UI 无 WebSocket 实时进度 | **P2** | M |
| F9 | Chat 无流式响应（阻塞等待完整 LLM 回复） | **P2** | L |
| F10 | Chat 历史无会话隔离（复用 items 表） | **P2** | M |
| F11 | Skill 引擎 100% 占位符（Phase 3 未启动） | **P2** | L |
| F8 | 分类队列依赖手动 drain，非后台自动消费 | **P2** | L |
| Q3 | `store.rs` 12 个目录/队列 pub 函数无测试 | **P2** | M |
| S2 | Python venv 激活文档缺失 | **P2** | S |
| F3 | Rust 搜索无 LRU 缓存 | **P3** | S |
| T9 | Python token 比较非恒定时间 | **P3** | S |
| T11 | CSP 未显式声明（manifest.json） | **P3** | S |
| Q7 | 无 `cargo audit` CI 步骤 | **P3** | S |
| Q5 | 全量内存索引，10 万条目约需 1.2 GB | **P3** | L |
| T10 | AES-GCM nonce 理论碰撞（个人库体量无实际风险） | **P3** | M |
| F1b | 锁定状态数据只读访问模式缺失 | **P3** | L |

**总计：34 项缺口**

---

## 7. 优先级矩阵

```
工作量 ↑
  L   │ S3 Tauri   │ Q1 路由测试  F9 Chat流式  F8 分类auto  F11 Skill │ Q5 内存扩展  F1b 只读模式
      │            │ S4 WebSocket                                     │
  M   │            │ Q4 unwrap   S1 迁移Reranker │ F2 Reranker  Q3 store测试 │ Q5 内存
      │            │ T7 Win ACL  F4 Level1入队  │ F6 端点     F7 UI进度  │
      │            │             F10 Chat历史   │ F10 Chat历史 T10 nonce │
  S   │ T1 CORS    │ T3 token默认 T4 TLS默认    │ T5 路径边界  T6 Zeroize │ F3 LRU  T9 Python token
      │ T2 device-secret│ F5 Worker启动 F1 change_pw│ T8 token吊销 Q2 测试环境 │ T11 CSP  Q7 audit
      │            │             Q6 文档提交    │ S2 venv文档  │
      └────────────┴─────────────────────────────────────────────────────────
                  P0            P1              P2                        P3
                                影响面 →
```

---

## 8. 快赢建议

以下为 **P0 + 工作量 S** 的优先处理项（总耗时 < 1 天，消除最高优先级风险）：

| 序号 | 操作 | 预计时间 |
|------|------|----------|
| 1 | 修复 CORS：`CorsLayer::permissive()` → 白名单限制 | 30 分钟 |
| 2 | `/device-secret/export` 独立强制 Bearer token | 1 小时 |
| 3 | `change_password` 加 SQLite 事务（BEGIN/COMMIT） | 30 分钟 |
| 4 | Bearer token 改为默认开启 | 1 小时 |
| 5 | 提交 5+ 个未提交文件，更新 RELEASE.md 版本号 | 15 分钟 |
| 6 | `pytest.ini` 添加 `pythonpath = src`，修复 Python 测试环境 | 15 分钟 |
| 7 | `derive_master_key` Vec → `Zeroizing<Vec<u8>>` | 20 分钟 |
| 8 | `manifest.json` 显式声明 CSP | 15 分钟 |

**合计：约 4 小时，消除 2 个 P0 安全漏洞 + 1 个 P1 数据完整性风险**

---

*本报告由自动化扫描（代码静态分析 + 手动代码审查）生成，基于 2026-04-12 代码库快照。*

# 搜索质量提升 + 本地推理层 + LLM 抽象设计

## Goal

在 npu-vault Rust 商用线中实现三项协同升级：
1. 本地 ONNX 推理层（替换 Ollama embedding 依赖，新增 cross-encoder reranker）
2. 三阶段搜索管道（initial_k → intermediate_k → top_k，修复向量搜索硬编码 bug）
3. LLM OpenAI-compat 抽象 + 对话 Session 管理

## Architecture

### 整体组件变化

```
vault-core/src/
  embed.rs              → 保留（legacy OllamaProvider，向后兼容）
  infer/                → 新增：本地 ONNX 推理层
    mod.rs              — EmbeddingProvider / RerankProvider trait（替代 embed.rs）
    embedding.rs        — OrtEmbedding（ort + tokenizers）
    reranker.rs         — OrtReranker（ort + tokenizers）
    provider.rs         — EP 自动选择（NPU/iGPU/CUDA/CPU）
    model_store.rs      — 模型下载与缓存管理
  search.rs             → 扩展：三阶段管道 + SearchContext 公共函数
  chat.rs               → 改造：search_for_context 接入公共搜索函数
  llm.rs                → 扩展：统一 OpenAI-compat HTTP 客户端
  store.rs              → 扩展：conversations + conversation_messages 表
  platform.rs           → 扩展：新增 NpuKind 枚举，驱动 EP 选择

vault-server/src/routes/
  search.rs             → 扩展：接收 initial_k / intermediate_k 参数
  chat.rs               → 改造：session_id 支持，接入公共搜索函数
  chat_sessions.rs      → 新增：Session CRUD 路由
```

### 新增 Cargo 依赖

```toml
# vault-core/Cargo.toml
ort = { version = "2", features = ["load-dynamic"] }
tokenizers = "0.21"
hf-hub = "0.3"
```

`ort` 通过 feature flags 支持多 EP：`openvino`（Intel NPU/iGPU）、`directml`（AMD/Intel Windows）、`cuda`（NVIDIA）。`load-dynamic` 模式在运行时动态加载 ort 共享库，二进制体积不膨胀。

---

## 模型选型

| 角色 | 模型 | 格式 | 维度 | 最大 token |
|------|------|------|------|-----------|
| Embedding | `Qwen3-Embedding-0.6B` | INT8 ONNX | 1024 | 32768 |
| Reranker | `bge-reranker-v2-m3` | INT8 ONNX | — | 8192 |

**选型理由（面向专利/法律/标书场景）：**
- Qwen3-Embedding-0.6B CMTEB 多语言 64.33 分（vs bge-m3 的 59.56），中文专业词汇理解更强
- 32K token 上限可完整索引一份专利申请书（通常 5000-20000 字），bge-m3 的 8192 上限会强制截断
- 维度同为 1024，无需重建现有向量索引
- bge-reranker-v2-m3 是最成熟的 ONNX encoder reranker，专为 cross-encoder 精排设计，无需 LLM 推理

---

## Section 1：本地推理层 `vault-core/src/infer/`

### Trait 定义（`mod.rs`）

```rust
pub trait EmbeddingProvider: Send + Sync {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
    fn dimensions(&self) -> usize;
}

pub trait RerankProvider: Send + Sync {
    /// 返回每个文档与 query 的相关性分数 [0.0, 1.0]，顺序与 documents 一致
    fn score(&self, query: &str, documents: &[&str]) -> Result<Vec<f32>>;
}

pub enum EmbeddingModel {
    Qwen3Embedding0_6B,
    BgeM3,
    Custom { model_id: String, dims: usize },
}

pub enum RerankerModel {
    BgeRerankerV2M3,
    Custom { model_id: String },
}
```

### EP 自动选择（`provider.rs`）

```rust
pub fn build_session(model_path: &Path) -> Result<ort::Session> {
    let platform = crate::platform::detect();
    let providers = match platform.npu_kind {
        NpuKind::IntelNpu | NpuKind::IntelIgpu =>
            vec![OpenVINO::default().build(), CPU::default().build()],
        NpuKind::AmdNpu =>
            vec![DirectML::default().build(), CPU::default().build()],
        NpuKind::Cuda =>
            vec![CUDA::default().build(), CPU::default().build()],
        NpuKind::None =>
            vec![CPU::default().build()],
    };
    ort::Session::builder()?
        .with_execution_providers(providers)?
        .commit_from_file(model_path)
}
```

`platform.rs` 新增 `NpuKind` 枚举，基于现有芯片 PCI ID 检测逻辑映射。

### 模型存储（`model_store.rs`）

存储路径：`~/.local/share/npu-vault/models/<model-id>/`

每个模型目录包含：
- `model_int8.onnx` — INT8 量化推理模型
- `tokenizer.json` — HuggingFace tokenizer 配置
- `sha256.txt` — 完整性校验文件（SHA256 写死在代码中）

下载策略：
- 首次使用时懒加载（vault 启动后台线程检测，缺失则从 HuggingFace Hub 下载）
- 支持镜像配置（`HF_ENDPOINT` 环境变量，国内可配 `hf-mirror.com`）
- 下载完成后校验 SHA256，失败则删除重试
- 下载进度通过现有 WebSocket 事件推送到 Web UI

### Fallback 策略

```
有 ONNX 模型（优先）
  → ort 推理，按 EP 优先级自动加速
无 ONNX 模型
  → 检查 OllamaProvider（legacy）
      Ollama 可用 → embedding 降级（无 rerank）
      Ollama 不可用 → 纯 FTS5 搜索（无向量，无 rerank）
```

三层降级保证任意环境可用，质量递减但不中断。

---

## Section 2：三阶段搜索管道

### 参数结构

```rust
// vault-core/src/search.rs

pub struct SearchParams {
    pub top_k: usize,
    pub initial_k: usize,      // 粗召回。默认: top_k * 5，clamp(20, 100)
    pub intermediate_k: usize, // Rerank 前候选数。默认: top_k * 2，clamp(top_k, 40)
}

impl SearchParams {
    pub fn with_defaults(top_k: usize) -> Self {
        Self {
            top_k,
            initial_k: (top_k * 5).clamp(20, 100),
            intermediate_k: (top_k * 2).clamp(top_k, 40),
        }
    }
}
```

### 公共搜索函数

```rust
pub struct SearchContext<'a> {
    pub fulltext: &'a FulltextIndex,
    pub vectors: Option<&'a VectorIndex>,
    pub embedding: Option<&'a dyn EmbeddingProvider>,
    pub reranker: Option<&'a dyn RerankProvider>,
    pub store: &'a Store,
    pub dek: &'a Key32,
}

pub fn search_with_context(
    ctx: &SearchContext,
    query: &str,
    params: &SearchParams,
) -> Result<Vec<SearchResult>>;
```

`search.rs`（GET/POST 搜索端点）和 `chat.rs`（`search_for_context`）均调用此函数，消除重复逻辑。

### 管道流程

```
query
  ├─ VectorSearch(initial_k)     ← 修复：从硬编码 10 改为 initial_k
  └─ FulltextSearch(initial_k)   ← 修复：从 top_k 改为 initial_k
          ↓
    RRF Fusion → 取 intermediate_k 条
          ↓
    Fetch & Decrypt（仅 intermediate_k 条）
          ↓
    RerankProvider::score(query, documents)
      有 OrtReranker  → bge-reranker-v2-m3 精排
      无 Reranker     → 保留当前余弦均值方式
      无向量          → 跳过，按 RRF 分数排序
          ↓
    取 top_k → 返回
```

### Bug 修复

`vault-server/src/routes/chat.rs:117` 中 `content.chars().take(500)` 硬截断删除，改用 `allocate_budget`（与 `chat.rs` 保持一致）。

### API 变更（向后兼容）

`GET /api/v1/search` 新增可选参数：

```
?q=专利侵权&top_k=10&initial_k=50&intermediate_k=20
```

`POST /api/v1/search/relevant` body 新增两个可选字段：

```json
{
  "query": "...",
  "top_k": 5,
  "initial_k": 25,
  "intermediate_k": 10,
  "injection_budget": 2000
}
```

不传则使用 `SearchParams::with_defaults(top_k)` 自动计算。

---

## Section 3：LLM OpenAI-compat 抽象

### 配置结构

```rust
// vault-core/src/llm.rs

pub struct LlmConfig {
    pub endpoint: String,  // "https://api.openai.com/v1" / "http://localhost:11434/v1"
    pub api_key: String,
    pub model: String,     // "gpt-4o-mini" / "qwen2.5:7b" / "claude-3-haiku"
    pub timeout_secs: u64, // 默认 60
}
```

### 三级优先级（启动时自动探测）

1. 配置文件中的 `llm.endpoint` — 用户显式指定，最高优先级
2. 本地 Ollama 健康检查（`GET http://localhost:11434/api/tags`）— 自动接入
3. 无 LLM 可用 — Chat 功能禁用，搜索/知识库正常工作

### LlmClient

现有 `LlmProvider` trait 的实现改为通过 `LlmConfig` 调用标准 OpenAI Chat Completions API（`POST /v1/chat/completions`）。`classifier.rs` 和 `chat.rs` 的 LLM 调用均路由到此统一入口，不改业务逻辑。

---

## Section 4：对话 Session 管理

### 数据模型

```sql
-- vault-core/src/store.rs 中新增 Schema

CREATE TABLE IF NOT EXISTS conversations (
    id          TEXT PRIMARY KEY,
    title       TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS conversation_messages (
    id              TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    role            TEXT NOT NULL CHECK(role IN ('user','assistant','system')),
    content         TEXT NOT NULL,   -- 字段级 AES-256-GCM 加密
    citations       TEXT,            -- JSON: [{"item_id":"...","title":"...","relevance":0.9}]
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_conv_messages_conv_id
    ON conversation_messages(conversation_id);
```

### Store 方法

```rust
// vault-core/src/store.rs

pub fn create_conversation(&self, dek: &Key32, title: &str) -> Result<String>;
pub fn list_conversations(&self, limit: usize, offset: usize) -> Result<Vec<ConversationSummary>>;
pub fn get_conversation_messages(&self, dek: &Key32, conv_id: &str) -> Result<Vec<ConvMessage>>;
pub fn append_message(&self, dek: &Key32, conv_id: &str, role: &str,
                      content: &str, citations: &[Citation]) -> Result<String>;
pub fn delete_conversation(&self, conv_id: &str) -> Result<()>;
```

### API

```
POST   /api/v1/chat
  body:  { "message": "...", "session_id": "uuid-可选" }
  返回:  { "content": "...", "citations": [...],
            "session_id": "uuid", "knowledge_count": 3 }
       ↑ 不传 session_id 时自动创建新会话并返回

GET    /api/v1/chat/sessions?limit=20&offset=0
  返回:  { "sessions": [{"id","title","created_at","updated_at"},...] }

GET    /api/v1/chat/sessions/:id
  返回:  { "session": {"id","title"}, "messages": [...] }

DELETE /api/v1/chat/sessions/:id
  返回:  204 No Content
```

### 与现有实现的迁移

- 现有 `auto_save_conversation`（写入 `source_type=ai_chat` 的 items）保留，兼容旧数据
- 新对话写入 `conversations` + `conversation_messages`，不再写入 items 表
- `GET /api/v1/chat/history` 接口保持不变，内部改为查 `conversations` 表

---

## 错误处理

| 场景 | 处理方式 |
|------|---------|
| 模型文件缺失 | 后台触发下载，当次请求降级到 Ollama/FTS |
| Ollama 不可用 | 降级到纯 FTS 搜索，Chat 返回 503 + hint |
| EP 加载失败 | ort 自动跳过，降级到 CPU |
| SHA256 校验失败 | 删除损坏文件，重新下载 |
| Reranker 超时 | 跳过 rerank，返回 RRF 排序结果 |
| session_id 不存在 | 404 + 创建新 session |

---

## 测试策略

### vault-core 单元测试

- `infer/embedding.rs` — MockEmbeddingProvider（现有）继续用于非 ONNX 测试
- `infer/reranker.rs` — MockRerankProvider：`score()` 返回固定分数列表
- `search.rs` — `search_with_context` 用 mock 组件测试三阶段逻辑
- `store.rs` — conversation CRUD 用 `Store::open_memory()` 测试

### vault-server 集成测试

- `POST /api/v1/chat` 带/不带 `session_id` 各一个 case
- `GET /api/v1/chat/sessions` 返回列表
- `GET /api/v1/chat/sessions/:id` 返回消息历史
- `DELETE /api/v1/chat/sessions/:id` 返回 204

### 不需要真实模型的测试

所有单元和集成测试均使用 Mock provider，CI 无需 GPU/NPU/Ollama。ONNX 推理路径通过手动集成测试验证（在有 Ollama 或模型文件的环境）。

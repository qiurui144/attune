# npu-vault 桌面应用架构设计 + 产品化路线图

---

## 一、系统架构

### 进程模型：Tauri 2 单进程 + 线程隔离

```
+------------------------------------------------------------------+
|                     DESKTOP APP (vault-desktop)                    |
|  +---------------------------+  +------------------------------+  |
|  |    Tauri Shell             |  |   Frontend (React SPA)       |  |
|  |    System Tray + Window    |  |   Chat / Search / Browse     |  |
|  |    Auto-Update / Dialogs   |  |   Taxonomy / Settings        |  |
|  +-----------|---------------+  +-------------|----------------+  |
|              | Tauri IPC (窗口管理)             | HTTP localhost   |
+--------------|---------------------------------|-------------------+
               v                                 v
+------------------------------------------------------------------+
|                   vault-server-lib (Router)                        |
|   Middleware: vault_guard → bearer_auth → CORS                    |
|   Routes: vault / chat(SSE) / search / ingest / items / classify  |
|   AppState: Arc<VaultEngine> + Arc<SearchService> + Arc<AiRouter> |
+------------------------------------------------------------------+
               |
+------------------------------------------------------------------+
|                        vault-core                                 |
|  +================ app/ (编排层) ===============================+ |
|  | VaultEngine  SearchService  AiRouter  ClassifyService         | |
|  | IngestService  QueueWorker  FileWatcher  ClusterService       | |
|  +===============================================================+ |
|  +======= infra/ (适配层) ====+  +==== domain/ (纯逻辑) =======+ |
|  | Store(SQLite+Arc<Mutex>)   |  | types.rs (Item, Result...)   | |
|  | Crypto (Argon2+AES)        |  | search.rs (rrf_fuse)        | |
|  | FulltextIndex (tantivy)    |  | chunker.rs (chunk/sections) | |
|  | VectorIndex (usearch)      |  | taxonomy.rs (dims/plugins)  | |
|  | OllamaProvider             |  | error.rs (VaultError)       | |
|  | OpenAiProvider (NEW)       |  +=============================+ |
|  | Scanner / ScannerWebDav    |                                   |
|  | Parser (PDF/DOCX/MD/TXT)   |                                   |
|  +============================+                                   |
+------------------------------------------------------------------+
```

**关键决策**：
- **单进程**（不是 subprocess）：vault-server-lib 导出 `build_router(state) -> Router`，Tauri 直接调用，不 spawn 子进程
- **Store 用 `Arc<Mutex<Connection>>`**：解决 QueueWorker 并发访问问题（当前致命缺陷）
- **vault-core 三层分离**：domain（纯函数）/ infra（I/O 适配）/ app（编排用例）
- **前端走 HTTP**：不用 Tauri IPC 传数据——API 已完整，IPC 只管窗口/文件对话框/通知

### Crate 结构重构

```
npu-vault/
├── crates/
│   ├── vault-core/           # lib: domain + infra + app 三层
│   ├── vault-server-lib/     # NEW lib: 导出 build_router() + create_app_state()
│   ├── vault-server/          # bin: thin main.rs 调用 vault-server-lib
│   ├── vault-cli/             # bin: 不变
│   └── vault-desktop/         # NEW bin: Tauri 2 shell
├── frontend/                  # NEW: React + Vite + Tailwind SPA
│   ├── src/
│   │   ├── app.tsx
│   │   ├── api/               # HTTP client (fetch → localhost)
│   │   ├── stores/            # Zustand (vault-state, chat-state, ui-state)
│   │   ├── components/
│   │   │   ├── chat/          # Chat Panel + Message Bubble + Context Card
│   │   │   ├── knowledge/     # Document List + Viewer + Taxonomy Filter
│   │   │   ├── vault/         # Unlock Screen + Setup Wizard
│   │   │   └── layout/        # App Shell + Sidebar + Tray Status
│   │   └── hooks/             # use-sse-stream, use-vault-status
│   └── package.json
└── tests/
```

### 系统托盘生命周期

```
OS Login (auto-start)
  → main() → Tauri::Builder
  → setup():
      1. Vault::open_default()
      2. create_app_state(vault)
      3. tokio::spawn(bind Router to 127.0.0.1:0)
      4. 保存动态端口号 → Tauri managed state
      5. 启动后台 Worker（embedding + classify + file watch）
  → Tray Icon 出现 (状态: LOCKED)
      左键 → 打开主窗口 (WebView → http://127.0.0.1:{port}/)
      右键 → 菜单: Unlock / Lock / Pause Indexing / Settings / Quit
  → 关闭窗口 → window.hide()（不退出）
  → Quit → stop workers → vault.lock() → zeroize → exit
```

---

## 二、AI 集成架构

### Provider 抽象 + 降级链

```rust
pub struct AiRouter {
    embedding_chain: Vec<Arc<dyn EmbeddingProvider>>,  // 按优先级排列
    chat_chain: Vec<Arc<dyn LlmProvider>>,
}

// 降级链示例:
// embedding: [Ollama bge-m3] → [OpenAI text-embedding-3] → [NoopProvider]
// chat:      [Ollama qwen2.5] → [OpenAI gpt-4o] → [Anthropic claude] → [Noop]
```

### 支持的后端

| 后端 | Embedding | Chat | 流式 | 离线 |
|------|-----------|------|------|------|
| Ollama (本地) | bge-m3 | qwen2.5/llama3 | ✓ | ✓ |
| OpenAI (云) | text-embedding-3 | gpt-4o | ✓ | ✗ |
| Anthropic (云) | — | claude-sonnet | ✓ | ✗ |
| 自定义 (OpenAI 兼容) | ✓ | ✓ | ✓ | — |
| ONNX Runtime (本地) | bge-m3.onnx | — | ✗ | ✓ |

### RAG Chat 核心流程

```
用户消息 → SearchService.search_relevant(query, top_k=5)
         → 解密并提取 inject_content（动态预算 2000 tokens）
         → build_rag_system_prompt(knowledge_context)
         → AiRouter.chat_stream(system, history, user_msg)
         → SSE 流式返回每个 token
         → 最后返回 citations（引用了哪些知识条目）
         → 对话自动 ingest 入知识库
```

### API Key 存储

加密存于 `vault_meta`（用 DEK_db 加密），仅 UNLOCKED 时可访问：
```
key: "setting:openai_api_key"  →  AES-256-GCM(dek_db, "sk-proj-...")
```

---

## 三、数据流架构

### 文档入库管道

```
文件进入（扫描/上传/WebDAV）
  ↓ [前台 ~50ms]
Parse (PDF/DOCX/MD/TXT) → title + content
  ↓
Store.insert_item (content 加密) → item_id
  ↓
FulltextIndex.add_document (立即可搜索，不依赖 AI)  ← 关键修复
  ↓
Enqueue: embed(level1+2) + classify(level3)
  ↓ [后台线程，异步]
QueueWorker:
  embed → Ollama/OpenAI → VectorIndex.add
  classify → LLM → TagIndex.upsert + Store.update_tags
```

### 异步边界

| 操作 | 执行上下文 | 原因 |
|------|-----------|------|
| HTTP 请求处理 | tokio async task | I/O 密集 |
| 文件解析 | spawn_blocking | CPU 密集 |
| AI API 调用 | spawn_blocking 或独立 runtime | 避免嵌套 runtime |
| Queue 轮询 | 独立 std::thread | 长生命周期 |
| 文件监听 | 独立 std::thread | notify-rs 回调 |
| SSE 流式 | tokio async | 流式响应 |

---

## 四、安全架构

| 层面 | 机制 | 备注 |
|------|------|------|
| 密钥派生 | Argon2id (64MB/3轮/4线程) | 抗 GPU |
| 数据加密 | AES-256-GCM 字段级 | 每值独立 nonce |
| 密钥清零 | zeroize + ZeroizeOnDrop | lock 时内存归零 |
| 内存保护 | memsec::mlock (NEW) | 防止密钥 swap 到磁盘 |
| 文件权限 | Unix 0600 / Windows icacls | 跨平台 |
| 会话 | HMAC-SHA256 token, 4h TTL | NAS 模式 |
| API Key | DEK_db 加密存储 | UNLOCKED 时才可读 |
| 审计日志 | audit_log 表 (NEW) | 加密，write-only |
| 标题加密 | 可选 (NEW) | 律师场景标题含客户名 |

---

## 五、插件架构

### 插件清单 v2（从纯 taxonomy 扩展到 prompt + 解析器）

```yaml
# plugin.yaml
id: law
name: 法律助手
version: "1.0"

# Layer 1: 分类维度 (已有)
dimensions: [...]

# Layer 2: RAG 提示词模板 (NEW)
prompts:
  system_hint: "你是一名资深法律顾问..."
  rag_template: |
    基于以下法律文档：
    {context}
    回答: {query}

# Layer 3: 解析器映射 (NEW)
parsers:
  - extension: ".case"
    handler: "builtin:pdf"

# Layer 4: 首次引导 (NEW)
onboarding:
  suggested_dirs: [~/Documents/Contracts, ~/Documents/Cases]
  search_examples: ["不可抗力条款", "终止条件", "赔偿上限"]
```

**设计原则**：插件只是数据（YAML），不执行任意代码——安全第一。

---

## 六、平台交付

| 平台 | 打包 | 自启动 | 托盘 | 签名 |
|------|------|--------|------|------|
| Windows | NSIS (.msi) | Registry | ✓ | EV cert + signtool |
| Linux | AppImage + .deb | XDG autostart | libappindicator | — |
| macOS | .dmg | launchd | ✓ | notarytool |
| NAS | Docker / systemd | systemd service | 无（headless） | — |
| Mobile | PWA via NAS HTTPS | — | — | — |

### CI/CD 矩阵

```yaml
matrix:
  - { os: ubuntu-latest, target: x86_64-unknown-linux-gnu }
  - { os: ubuntu-latest, target: aarch64-unknown-linux-gnu, cross: true }
  - { os: macos-latest, target: aarch64-apple-darwin }
  - { os: windows-latest, target: x86_64-pc-windows-msvc }
```

---

## 七、记忆系统架构（参照 Claude Code + OpenClaw + Mem0）

### 三层记忆模型

```
┌─────────────────────────────────────────────────────┐
│  Layer 1: 工作记忆 (Working Memory)                   │
│  当前会话的完整上下文，30K tokens 目标窗口              │
│  ├── 最近 7-10 轮对话：100% 完整                       │
│  ├── 10-30 轮：50% 压缩（摘要 + 关键事实）              │
│  └── 30+ 轮：超压缩摘要（~200 tokens）                  │
├─────────────────────────────────────────────────────┤
│  Layer 2: 情景记忆 (Episodic Memory)                   │
│  历史会话的索引化摘要，向量化存储                        │
│  ├── 每次会话结束自动生成 session summary               │
│  ├── 向量索引：新对话开始时搜索相关历史会话              │
│  └── 记忆衰减：按 Ebbinghaus 曲线 + 重要度加权          │
├─────────────────────────────────────────────────────┤
│  Layer 3: 语义记忆 (Semantic Memory)                   │
│  长期沉淀的事实、模式、偏好                             │
│  ├── 用户偏好（沟通风格/领域术语/常用格式）              │
│  ├── 领域知识（从文档提取的结构化事实）                   │
│  ├── 行为模式（搜索习惯/高频操作/关注领域）              │
│  └── 关系图谱（实体间的关联：人→公司→合同→条款）          │
└─────────────────────────────────────────────────────┘
```

### 记忆衰减机制

```rust
// domain/memory.rs

/// Ebbinghaus 遗忘曲线 + 重要度加权
pub fn memory_score(
    semantic_similarity: f32,  // 与当前查询的语义相似度
    age_days: f32,             // 距创建多少天
    importance: Importance,    // 重要度等级
    access_count: u32,         // 被引用/访问次数（强化记忆）
    last_accessed_days: f32,   // 上次访问距今天数
) -> f32 {
    // 衰减时间常数（天）
    let tau = match importance {
        Importance::Critical => 90.0,   // 关键知识：3 个月缓慢衰减
        Importance::High     => 30.0,   // 重要：1 个月
        Importance::Medium   => 14.0,   // 普通：2 周
        Importance::Low      => 7.0,    // 低优：1 周
    };

    // 访问强化：每次访问降低衰减速度
    let reinforcement = 1.0 + (access_count as f32).ln().max(0.0);
    let effective_tau = tau * reinforcement;

    // Ebbinghaus 衰减
    let decay = (-age_days / effective_tau).exp();

    // 最近访问额外加权
    let recency_boost = if last_accessed_days < 1.0 { 1.5 }
                        else if last_accessed_days < 7.0 { 1.2 }
                        else { 1.0 };

    semantic_similarity * decay * recency_boost
}
```

**数据库支持**：`items` 表新增列：
```sql
ALTER TABLE items ADD COLUMN importance INTEGER DEFAULT 2;  -- 0=critical, 1=high, 2=medium, 3=low
ALTER TABLE items ADD COLUMN access_count INTEGER DEFAULT 0;
ALTER TABLE items ADD COLUMN last_accessed_at TEXT;
```

`search_service` 在返回结果前应用 `memory_score` 排序，替代纯 RRF 分数。

### 上下文压缩引擎

```rust
// app/context_manager.rs

pub struct ContextManager {
    llm: Arc<dyn LlmProvider>,
    max_tokens: usize,         // 模型上下文窗口
    target_tokens: usize,      // 目标使用量（max * 0.7）
    compression_threshold: f32, // 0.6 — 60% 时开始压缩
}

impl ContextManager {
    /// 管理对话上下文：根据长度自动压缩
    pub fn prepare_context(
        &self,
        system_prompt: &str,
        rag_context: &str,
        history: &[ChatMessage],
        user_message: &str,
    ) -> Vec<ChatMessage> {
        let total = count_tokens(system_prompt) + count_tokens(rag_context)
                  + count_tokens(user_message) + sum_tokens(history);

        if total < self.target_tokens {
            return full_context(system_prompt, rag_context, history, user_message);
        }

        // 渐进压缩
        let compressed_history = self.compress_history(history);
        // ...
    }

    /// 渐进压缩：最新 N 轮保留，旧的分段摘要
    fn compress_history(&self, history: &[ChatMessage]) -> Vec<ChatMessage> {
        let keep_full = 7;  // 最近 7 轮完整保留
        let (old, recent) = history.split_at(history.len().saturating_sub(keep_full * 2));

        if old.is_empty() {
            return recent.to_vec();
        }

        // 用 LLM 摘要旧对话
        let summary = self.summarize_turns(old);

        let mut result = vec![
            ChatMessage::system(&format!("[以下是此前对话的摘要]\n{summary}\n[摘要结束]")),
        ];
        result.extend_from_slice(recent);
        result
    }

    fn summarize_turns(&self, turns: &[ChatMessage]) -> String {
        let prompt = "请将以下对话压缩为简洁的摘要，保留：\n\
                      1. 关键决策和结论\n\
                      2. 重要的事实和数据\n\
                      3. 用户的偏好和指示\n\
                      丢弃：寒暄、重复、已解决的中间讨论";
        self.llm.chat(prompt, &format_turns(turns))
            .unwrap_or_else(|_| "（摘要生成失败，上下文可能不完整）".into())
    }
}
```

---

## 八、AI 技能模板系统（参照 Claude Code Skills + Cursor Rules）

### 技能定义格式

```yaml
# ~/.config/npu-vault/skills/contract-review.yaml
id: contract-review
name: 合同审查
version: "1.0"
domain: law

# 触发条件：何时自动加载此技能
triggers:
  - keywords: ["合同", "contract", "审查", "review", "条款", "clause"]
  - tags: { domain: "法律" }
  - file_types: [".pdf", ".docx"]

# System prompt 注入（加载此技能时追加到 system prompt）
system_hint: |
  你是一名资深合同审查律师。审查时请关注：
  1. 权利义务是否对等
  2. 违约责任是否明确
  3. 争议解决方式是否合理
  4. 是否存在不合理的免责条款
  5. 合同期限和终止条件

# 工作流模板（用户可一键触发）
workflows:
  - id: risk-analysis
    name: 风险分析
    prompt: |
      请对以下合同进行风险分析，按照低/中/高三个等级标注每个条款的风险程度，
      重点关注：不可抗力、赔偿上限、竞业限制、知识产权归属、保密义务。

  - id: clause-comparison
    name: 条款对比
    prompt: |
      请将以下合同的关键条款与知识库中的类似合同进行对比，
      列出差异项和建议修改点。

  - id: summary
    name: 合同摘要
    prompt: |
      请生成此合同的结构化摘要，包含：
      当事人、标的、金额、期限、关键义务、特殊条款。
```

### 技能路由器

```rust
// app/skill_router.rs

pub struct SkillRouter {
    skills: Vec<Skill>,
    tag_index: Arc<Mutex<TagIndex>>,  // 用于 tags 触发匹配
}

impl SkillRouter {
    /// 根据用户消息 + 上下文自动选择相关技能
    pub fn select_skills(
        &self,
        user_message: &str,
        active_items: &[String],  // 当前对话引用的 item_ids
    ) -> Vec<&Skill> {
        let mut matched = Vec::new();
        for skill in &self.skills {
            // 关键词匹配
            if skill.triggers.iter().any(|t| t.matches(user_message)) {
                matched.push(skill);
                continue;
            }
            // 标签匹配（当前引用的文档是否属于该技能的领域）
            if self.items_match_skill_tags(active_items, skill) {
                matched.push(skill);
            }
        }
        matched
    }

    /// 将选中技能的 system_hint 合并到 prompt
    pub fn build_skill_context(&self, skills: &[&Skill]) -> String {
        skills.iter()
            .map(|s| format!("## 技能: {}\n{}", s.name, s.system_hint))
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}
```

### 预置技能清单

| 技能 ID | 适用角色 | 触发关键词 |
|---------|---------|-----------|
| `contract-review` | 律师 | 合同/审查/条款/违约 |
| `case-research` | 律师 | 案例/判例/法条/法规 |
| `bid-analysis` | 售前 | 标书/投标/方案/响应 |
| `proposal-draft` | 售前 | 起草/方案/报价/技术架构 |
| `code-review` | 开发者 | 代码/审查/重构/性能 |
| `research-summary` | 通用 | 总结/分析/对比/评估 |
| `knowledge-extract` | 通用 | 提取/整理/归纳/关键点 |

---

## 九、系统自学习机制

### 反馈闭环

```
用户行为 → 数据采集 → 模式学习 → 系统优化
   ↑                                    │
   └────────────── 效果验证 ←───────────┘
```

### 学习维度

| 维度 | 数据来源 | 学习内容 | 应用 |
|------|---------|---------|------|
| **搜索偏好** | search_history + click_events | 用户真正关心哪些文档 | 调整 RRF 权重 |
| **分类修正** | 用户手动修改标签 | 纠正 LLM 分类错误 | 微调 taxonomy prompt |
| **技能使用** | 技能触发频率 | 哪些技能最有价值 | 预加载高频技能 |
| **引用质量** | 用户是否点击引用 | RAG 检索质量评估 | 调整 top_k / budget |
| **对话模式** | 会话长度/主题分布 | 用户工作节奏 | 优化压缩时机 |

### 自适应检索权重

```rust
// app/adaptive_search.rs

/// 基于用户历史行为调整搜索参数
pub struct AdaptiveSearch {
    base_vector_weight: f32,    // 0.6
    base_fulltext_weight: f32,  // 0.4
    base_top_k: usize,         // 10
}

impl AdaptiveSearch {
    /// 根据用户点击反馈调整权重
    pub fn adapt(&mut self, feedback: &SearchFeedback) {
        // 如果用户频繁点击全文搜索结果而非向量结果
        // → 降低 vector_weight，提高 fulltext_weight
        let vector_click_rate = feedback.vector_clicks as f32
            / (feedback.total_clicks as f32).max(1.0);

        // 指数移动平均
        self.base_vector_weight = 0.9 * self.base_vector_weight
            + 0.1 * vector_click_rate.max(0.3);
        self.base_fulltext_weight = 1.0 - self.base_vector_weight;
    }
}
```

### 学习结果持久化

学习到的模式存入 `vault_meta`（加密）：
```json
{
  "learned_patterns": {
    "search_weights": { "vector": 0.55, "fulltext": 0.45 },
    "preferred_top_k": 7,
    "active_skills": ["contract-review", "case-research"],
    "classification_corrections": {
      "law_branch": { "合同纠纷": "商事法" }  // 用户纠正过的映射
    },
    "high_value_items": ["item_id_1", "item_id_2"],  // 频繁引用的
    "updated_at": "2026-04-12T10:00:00Z"
  }
}
```

---

## 十、超长文本处理架构

### 文档级处理（入库时）

```
超长文档 (100+ 页 PDF, 50000+ 字)
  ↓
Layer 1: 文档级摘要（LLM 生成 ~500 字概述）
  ↓
Layer 2: 章节切割 (extract_sections, ~1500 字/节)
  → 每章独立向量化 + 分类
  ↓
Layer 3: 段落分块 (chunk, 512 字 / 128 重叠)
  → 精细向量化
  ↓
Layer 4: 关键实体/数据提取
  → 日期、金额、人名、条款编号 → 结构化存储
```

### 对话级处理（聊天时）

```
当前对话 token 使用率
  0%━━━━━━━━━━━━━━━━━━━━━━━━100%

正常区间 (< 60%):
  [system + skills] [RAG context] [full history] [user msg]
  └── 无压缩，完整上下文

预警区间 (60-80%):
  [system + skills] [RAG context] [summary of old turns | recent 7 turns] [user msg]
  └── 旧对话压缩为摘要

危险区间 (80-95%):
  [system] [top-3 RAG] [ultra-summary | recent 3 turns] [user msg]
  └── 激进压缩：减少 RAG 条数，精简系统 prompt

溢出 (> 95%):
  [minimal system] [user msg only]
  └── 紧急模式：新开会话，旧会话存为情景记忆
```

### token 计数

```rust
// infra/tokenizer.rs

pub fn count_tokens(text: &str, model: &str) -> usize {
    match model {
        m if m.starts_with("gpt") => tiktoken_count(text, m),
        m if m.starts_with("claude") => text.len() / 3,  // Anthropic 近似
        _ => text.chars().count() / 2,  // 中文近似 (1 字 ≈ 1.5-2 tokens)
    }
}
```

---

## Context

### 架构方向决策（2026-04-12）

**放弃 webhook/DOM 注入路线，转向方案 C（内置 Chat + 被动捕获）。**

原因：
- DOM 注入脆弱（ChatGPT/Claude/Gemini 每 2-4 周更新 UI，扩展频繁失效）
- 前缀注入被 AI 忽略率高（无法控制 system prompt，只能改 user message）
- 移动端不可用（Chrome 扩展不支持手机浏览器/iOS/Android）
- 唯一能真正 work 的 RAG 方式是**控制完整 prompt**

**新架构**：
```
用户 → npu-vault Chat UI → 搜索本地知识库 → 构建 system+user prompt
                         → 调用 AI API (用户自带 token: OpenAI/Anthropic/Ollama)
                         → 流式返回 + 知识来源标注
                         → 对话自动入库知识库
```

Chrome 扩展退化为**被动收集器**（只捕获对话入库，不注入）。

### 现状问题诊断

npu-vault v0.5.0 加密基座扎实（120 tests, 28 MB），但对真实行业用户**不可用**：

1. **分类 100% 手动** — 用户必须点按钮触发分类
2. **搜索依赖 Ollama** — 全文索引仅在 embedding 成功后才有数据
3. **无 PDF 解析** — `parser.rs` 用 `read_to_string()` 读二进制 PDF 会失败
4. **错误全部静默** — Ollama 不可用时 UI 无任何提示
5. **无对话界面** — 用户必须去 ChatGPT/Claude 官网，再通过扩展"注入"（不可靠）

### 产品定位

**行业 AI 知识助手**：本地加密的个人知识库 + 内置 AI 对话界面。律师/售前/研究员打开就能用——上传文档、搜索知识、和 AI 对话时自动引用自己的专业积累。

**核心价值**：`你的文档 × AI 能力 = 更懂你的专业助手`

---

## Sprint 1：搜索必须可用（无需 AI 也能工作）

**目标**：用户安装后 5 分钟内可搜索自己的文档，零 AI 依赖。

### 1.1 全文索引解耦 — 扫描时直接写 tantivy

**文件**: `crates/vault-core/src/scanner.rs`, `crates/vault-server/src/routes/ingest.rs`, `crates/vault-server/src/routes/upload.rs`

当前问题：全文索引只在 QueueWorker 处理 Level 1 章节 embedding 时才写入 tantivy。如果 Ollama 不可用，tantivy 永远为空。

修复：在 `scanner::process_single_file()` 和 `ingest`/`upload` 路由中，解析文件后**立即**调用 `fulltext.add_document(item_id, title, content, source_type)`。这与 embedding 异步入队并行，不依赖 Ollama。

### 1.2 tantivy 持久化到磁盘

**文件**: `crates/vault-server/src/state.rs`, `crates/vault-core/src/index.rs`

当前问题：`FulltextIndex::open_memory()` 每次重启都丢失索引。

修复：改为 `FulltextIndex::open(&data_dir.join("tantivy"))` 持久化到磁盘。lock 时不删除（索引内容已加密在 items.content 里，tantivy 索引是明文摘要——如果用户在意，可以在 lock 时安全删除目录，unlock 时从 items 重建）。

### 1.3 PDF/DOCX 真实解析

**文件**: `crates/vault-core/Cargo.toml`, `crates/vault-core/src/parser.rs`

当前问题：`parser.rs` 声称支持 PDF/DOCX 但实际用 `read_to_string()` 读取，二进制文件必然失败。

修复：
- 添加依赖 `pdf-extract = "0.8"` 或 `lopdf`
- 添加依赖 `docx-rs = "0.2"` 或 `calamine`
- `parse_file()` 按扩展名分派：`.pdf` → PDF 提取器，`.docx` → DOCX 提取器
- 关键：PDF 提取处理多页、编码、表格文本

### 1.4 诊断端点 + 状态栏

**文件**: `crates/vault-server/src/routes/status.rs`, `crates/vault-server/assets/index.html`

当前问题：Ollama 不可用时一切静默。

修复：
- 新增 `GET /api/v1/status/diagnostics` 返回：`ollama_reachable`, `chat_model_available`, `embed_model_available`, `pending_tasks`, `failed_tasks`
- Web UI 顶部添加状态横幅：
  - 绿色：`AI 就绪 (qwen2.5:3b)`
  - 黄色：`AI 未连接，仅全文搜索可用。安装 Ollama 获取 AI 分类 →`
  - 红色：`后端未响应`

---

## Sprint 2：分类必须自动化

**目标**：文件入库后自动分类，零手动操作。

### 2.1 自动入队分类任务

**文件**: `crates/vault-core/src/scanner.rs`, `crates/vault-server/src/routes/ingest.rs`, `crates/vault-server/src/routes/upload.rs`

修复：在每次 `insert_item()` 成功后，添加一行：
```rust
store.enqueue_classify(&item_id, 3)?;
```
这一行代码让每个新文档自动进入分类队列。

### 2.2 后台分类 Worker

**文件**: `crates/vault-server/src/state.rs`, `crates/vault-server/src/main.rs`

当前问题：`drain_classify_batch()` 需要手动 HTTP 调用。

修复：在 `init_search_engines()` 末尾，如果 classifier 可用，启动一个后台线程：

```rust
if self.classifier.lock().unwrap().is_some() {
    let state_ref = self as *const AppState; // 或用 Arc
    std::thread::spawn(move || {
        loop {
            // 调用 drain_classify_batch(5)
            // 如果处理了 0 条，sleep 5 秒
            // 如果 vault locked，退出循环
        }
    });
}
```

### 2.3 定时重扫目录

**文件**: `crates/vault-server/src/main.rs`

修复：在 server 启动时 spawn 一个 tokio 定时器，每 30 分钟调用 `scan_directory` 对所有 `bound_dirs` 执行增量扫描。

---

## Sprint 3：首次体验

**目标**：非技术用户（律师）下载后能独立完成设置。

### 3.1 首次引导流程

**文件**: `crates/vault-server/assets/index.html`

新增"首次设置向导"：
1. 语言选择（中文/English）
2. 设置主密码
3. 选择职业/行业（律师 / 售前 / 开发者 / 通用）
4. 指定文档目录（输入路径或选择常见路径）
5. AI 后端选择：
   - 本地 Ollama（显示安装说明）
   - 跳过（仅全文搜索，稍后配置）

### 3.2 行业 Starter Kit

**律师版**:
```yaml
# law-starter.yaml
suggested_directories:
  - ~/Documents/Contracts
  - ~/Documents/Cases
  - ~/Documents/Legal-Research
file_types: [pdf, docx, md, txt]
search_examples:
  - "不可抗力条款"
  - "竞业禁止期限"
  - "赔偿责任上限"
  - "合同终止条件"
taxonomy_plugin: law  # 自动启用法律插件
```

**售前版**:
```yaml
# presales-starter.yaml
suggested_directories:
  - ~/Documents/Proposals
  - ~/Documents/RFPs
  - ~/Documents/Case-Studies
file_types: [pdf, docx, pptx, md]
search_examples:
  - "类似项目经验"
  - "技术方案架构"
  - "报价策略"
taxonomy_plugin: tech  # 启用技术插件
```

### 3.3 Ollama 问题的三层方案

**Tier 1（零依赖，开箱即用）**: tantivy BM25 全文搜索。Sprint 1 的 1.1 和 1.2 完成后即可。

**Tier 2（云 API，需 API Key）**: 添加 `OpenAIEmbeddingProvider` 和 `OpenAIChatProvider` 实现 `EmbeddingProvider`/`LlmProvider` trait。用户在设置中输入 API key（加密存储在 vault_meta）。需要隐私提醒。

**Tier 3（本地 Ollama，高级用户）**: 当前实现，保持不变但改善检测和提示。

---

## Sprint 4：内置 Chat + AI API 代理（核心功能）

**目标**：npu-vault 自己就是 AI 对话界面，不再依赖外部 ChatGPT/Claude 网站。

### 4.1 AI API 代理层

**新建**: `crates/vault-core/src/chat.rs`

```rust
pub struct ChatEngine {
    llm: Arc<dyn LlmProvider>,
    search: Arc<SearchEngine>,  // 混合搜索引擎
    store: Arc<Mutex<Store>>,
}

impl ChatEngine {
    /// 处理用户消息：RAG 搜索 → 构建 prompt → 调用 AI → 返回
    pub fn chat(&self, user_message: &str, history: &[ChatMessage]) -> Result<ChatResponse> {
        // 1. 搜索知识库（RRF 混合搜索）
        let knowledge = self.search.search_relevant(user_message, 5)?;
        
        // 2. 构建 system prompt（包含知识上下文）
        let system = build_rag_system_prompt(&knowledge);
        
        // 3. 组装完整对话历史
        let messages = build_messages(&system, history, user_message);
        
        // 4. 调用 AI API（支持 Ollama / OpenAI / Anthropic）
        let response = self.llm.chat_with_history(&messages)?;
        
        // 5. 标注知识来源
        let citations = extract_citations(&response, &knowledge);
        
        // 6. 对话自动入库
        self.auto_ingest(user_message, &response)?;
        
        Ok(ChatResponse { content: response, citations, knowledge_used: knowledge })
    }
}
```

**支持的 AI 后端**（用户在设置中选择）：
- **Ollama 本地** — `http://localhost:11434`（免费，隐私最优）
- **OpenAI** — `https://api.openai.com/v1`（需 API key）
- **Anthropic** — `https://api.anthropic.com/v1`（需 API key）
- **自定义** — 任何 OpenAI 兼容 API（如 DeepSeek、Kimi）

### 4.2 Chat 路由 + WebSocket 流式

**新建**: `crates/vault-server/src/routes/chat.rs`

```
POST /api/v1/chat           → 非流式对话（简单场景）
GET  /api/v1/chat/stream     → WebSocket 流式对话（主力）
GET  /api/v1/chat/history    → 对话历史列表
DELETE /api/v1/chat/history/{id} → 删除某轮对话
```

**WebSocket 流式协议**：
```json
// 客户端发送
{"type": "message", "content": "帮我分析这个合同的风险点", "conversation_id": "uuid"}

// 服务端流式返回
{"type": "knowledge", "items": [{"id": "...", "title": "...", "snippet": "..."}]}
{"type": "token", "content": "根据"}
{"type": "token", "content": "您过往"}
{"type": "token", "content": "的合同"}
{"type": "done", "citations": [{"item_id": "...", "title": "...", "relevance": 0.92}]}
```

### 4.3 Chat UI（Web UI 新标签页）

**修改**: `crates/vault-server/assets/index.html`

新增 **"对话"** 标签页（放在搜索之前，成为默认标签）：

```
┌────────────────────────────────────────┐
│  🔐 npu-vault           [unlocked]     │
│  [对话] [搜索] [条目] [分类] [设置]     │
├────────────────────────────────────────┤
│  ┌──────────────────────────────┐      │
│  │ 💬 AI: 基于你 23 份合同的经验...│      │
│  │    📎 引用: [合同A], [合同B]   │      │
│  └──────────────────────────────┘      │
│  ┌──────────────────────────────┐      │
│  │ 🧑 你: 帮我分析终止条款       │      │
│  └──────────────────────────────┘      │
│                                        │
│  ┌──────────────────────────┐ [发送]   │
│  │ 输入消息...               │          │
│  └──────────────────────────┘          │
│                                        │
│  📚 知识库已检索 5 条相关文档           │
│  AI 后端: Claude (Anthropic API)        │
└────────────────────────────────────────┘
```

**关键 UX**：
- 每条 AI 回复底部显示 `📎 引用: [文档名]` — 点击可查看原文
- 左侧显示 `📚 知识库已检索 X 条` — 透明化 RAG 过程
- 底部状态栏显示当前 AI 后端
- 对话自动保存到知识库（source_type = "ai_chat"）

### 4.4 多 AI 后端支持

**修改**: `crates/vault-core/src/llm.rs`

当前 `LlmProvider` trait 只有 `chat(system, user)` 方法。扩展为：

```rust
pub trait LlmProvider: Send + Sync {
    fn chat(&self, system: &str, user: &str) -> Result<String>;
    
    // 新增: 带历史的对话
    fn chat_with_history(&self, messages: &[ChatMessage]) -> Result<String>;
    
    // 新增: 流式对话
    fn chat_stream(&self, messages: &[ChatMessage], callback: &dyn Fn(&str)) -> Result<()>;
    
    fn is_available(&self) -> bool;
    fn model_name(&self) -> &str;
}
```

**新增**: `OpenAICompatibleProvider` — 兼容 OpenAI API 格式的通用 provider：
```rust
pub struct OpenAICompatibleProvider {
    client: reqwest::Client,
    base_url: String,      // https://api.openai.com/v1 或自定义
    api_key: String,       // 加密存储在 vault_meta
    model: String,         // gpt-4o / claude-3.5-sonnet / deepseek-chat
}
```

一个 struct 兼容：OpenAI、Anthropic（通过 base_url 和 header 适配）、DeepSeek、Kimi、Ollama（OpenAI 兼容模式）。

### 4.5 RAG System Prompt 构建

```rust
fn build_rag_system_prompt(knowledge: &[SearchResult]) -> String {
    let mut prompt = String::from(
        "你是用户的个人知识助手。以下是从用户本地知识库中检索到的相关文档。\n\
         请基于这些知识回答用户的问题。如果引用了某个文档，请标注 [文档标题]。\n\
         如果知识库中没有相关信息，正常回答即可，不要编造引用。\n\n"
    );
    
    prompt.push_str("=== 知识库相关文档 ===\n\n");
    for (i, item) in knowledge.iter().enumerate() {
        prompt.push_str(&format!(
            "[{}] 《{}》(来源: {}, 相关度: {:.0}%)\n{}\n\n",
            i + 1, item.title, item.source_type, 
            item.score * 100.0,
            item.inject_content.as_deref().unwrap_or(&item.content)
        ));
    }
    prompt.push_str("=== 知识库结束 ===\n");
    prompt
}
```

### 4.6 Chrome 扩展瘦身

**修改**: `extension/src/content/injector.js` → 删除整个文件
**修改**: `extension/src/content/index.js` → 移除 injector 引用
**保留**: `capture.js` — 被动捕获对话，自动 ingest 到知识库

扩展从"主动注入"退化为"被动收集"——不再修改 AI 网站的 DOM，只读取对话内容。

---

## Sprint 5：行业 Starter Kit + 移动端

### 5.1 律师 Starter Kit

完整的法律行业套件：
- 增强版 `law.yaml`（中英双语维度）
- 首次引导推荐合同/案例/研究三个目录
- 搜索示例（不可抗力/竞业禁止/终止条款）
- Chat 预设 persona：`你是一名资深法律顾问，专长合同审查和风险评估。`

### 5.2 售前 Starter Kit

- 新建 `presales.yaml` 插件
- 目录推荐：方案/RFP/案例
- Chat 预设 persona：`你是一名资深售前顾问，擅长基于历史方案快速响应客户需求。`

### 5.3 移动优先 UI

- 底部导航 4 按钮（对话/搜索/浏览/设置）
- 对话页面触摸优化
- `@media (max-width: 640px)` 自适应

---

## 关键文件

| 文件 | 修改内容 | Sprint |
|------|---------|--------|
| `vault-core/src/parser.rs` | PDF/DOCX 真实解析 | 1 |
| `vault-core/src/scanner.rs` | 扫描直接写 tantivy + 自动入队 classify | 1-2 |
| `vault-core/src/index.rs` | tantivy 持久化到磁盘 | 1 |
| `vault-server/src/state.rs` | 后台分类 worker + tantivy 持久化初始化 | 1-2 |
| `vault-server/src/routes/status.rs` | 诊断端点 | 1 |
| **`vault-core/src/chat.rs`** | **ChatEngine RAG 核心（新建）** | **4** |
| **`vault-server/src/routes/chat.rs`** | **Chat 路由 + WebSocket 流式（新建）** | **4** |
| **`vault-core/src/llm.rs`** | **扩展 trait + OpenAICompatibleProvider** | **4** |
| `vault-server/assets/index.html` | 引导流程 + Chat UI + 移动端 | 3-5 |
| `vault-core/assets/plugins/law.yaml` | 增强维度 + 英文 | 5 |
| **`vault-core/assets/plugins/presales.yaml`** | **售前插件（新建）** | **5** |

---

## 验证

### Sprint 1 验收
1. 启动 server（无 Ollama），绑定含 10 个 PDF 的目录
2. 扫描完成（< 60 秒），搜索关键词 → 返回 PDF 内容
3. UI 状态栏显示 "AI 未连接，仅全文搜索可用"

### Sprint 2 验收
1. 启动 + Ollama → 上传 3 个文档 → 30 秒后自动分类完成
2. `/tags/domain` 显示正确分布

### Sprint 3 验收
1. 首次启动 → 引导向导 → 选择律师 → 推荐目录
2. "跳过 AI" → 全文搜索立即可用

### Sprint 4 验收（核心）
1. 在 Chat UI 输入 "帮我分析合同终止条款"
2. npu-vault 自动搜索知识库，找到 3 个相关合同
3. 调用 AI API（Ollama 或 OpenAI），流式返回分析
4. 回答底部显示 `📎 引用: [合同A], [合同B]`
5. 对话自动入库知识库
6. 手机浏览器同样体验流畅

### Sprint 5 验收
1. 律师 starter kit 推荐 3 个目录 + 5 个搜索示例
2. Chat persona 对话专业度明显提升
3. 手机底部 4 按钮导航

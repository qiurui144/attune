# AI 自动分类子系统设计文档 (A)

> 日期: 2026-04-11
> 状态: Draft
> 依赖: npu-vault v0.3.0 (Phase 1 + 2a + 2b + 3 已完成)
> 子系统编号: A (首个)

## 1. 背景与定位

在 npu-vault 完成加密存储、混合搜索、文件扫描、Web UI、NAS 模式之后，下一步需要让知识库**可发现、可浏览、可归类**。类似手机相册的"按人物/地点/时间/事件"自动分类能力，本设计为每条知识生成多维度结构化标签 + 智能聚类，让用户在海量私人知识中快速找到自己想要的。

**产品定位升级**：从"本地优先的加密知识库"升级为"**行业 AI 助手的本地知识库基座**"。插件机制让每个行业（编程、法律、医疗、金融）都能声明自己的分类维度和提示词，构建领域特化的 AI 助手。

**本 spec 范围**：子系统 A（AI 自动分类）。不包含：
- B（个人行为画像）
- C（Web UI 全面升级）
- D（运行时插件市场）
- E（画像导出）
- F 系列（NAS 远程、Tauri 桌面、队列自启动等）

## 2. 核心决策摘要

| 决策点 | 选择 | 理由 |
|--------|------|------|
| 分类方式 | 多维度结构化标签 + HDBSCAN 聚类（2 + 4 组合）| 互补，结构化过滤 + 涌现式浏览 |
| 维度 schema | 混合式（维度名固定，值开放）+ 内置插件扩展 | 稳定骨架 + 领域灵活性 |
| LLM 依赖 | Ollama chat 模型硬依赖（无则分类不可用）| 简化降级逻辑，明确价值边界 |
| 首批插件 | 编程/技术 + 法律 | 用户主要场景 |
| 存储层 | 扩展 items.tags (加密 JSON) + 内存 TagIndex | 低复杂度，高性能，安全 |
| 聚类算法 | HDBSCAN | 自动发现聚类数，支持噪声点 |
| 触发时机 | ingest 后异步入队 (priority=3)，Queue Worker 后台处理 | 不阻塞搜索 |
| 作用范围 | 本地目录 + 增量扫描（复用 Phase 2b scanner）| NAS 远程放后续队列 |

## 3. 整体架构

```
┌─────────────────────────────────────────────────────────────┐
│  Chrome Extension / Web UI / CLI                             │
├─────────────────────────────────────────────────────────────┤
│  HTTP API Layer (Axum)                                       │
│  新增端点: /classify/* /tags /clusters /plugins              │
│  扩展端点: /search (支持 tag / cluster 过滤参数)             │
├─────────────────────────────────────────────────────────────┤
│  vault-core 新增 5 个模块                                     │
│  ├── llm.rs          — Ollama chat client (LlmProvider trait)│
│  ├── taxonomy.rs     — 维度定义 + 插件加载 + prompt 构建      │
│  ├── classifier.rs   — 分类 pipeline (批量 LLM 调用)          │
│  ├── clusterer.rs    — HDBSCAN 聚类 + LLM 命名               │
│  └── tag_index.rs    — 内存反向索引 (unlock 构建)             │
│                                                               │
│  已有模块扩展:                                                │
│  ├── store.rs        — task_type 列迁移 + list_all_item_ids  │
│  ├── queue.rs        — 支持 classify task_type               │
│  └── vault.rs        — init_search_engines 加入 tag_index    │
└─────────────────────────────────────────────────────────────┘
```

## 4. 维度设计

### 4.1 核心 5 维（所有用户必选）

| 维度 | 含义 | cardinality | 值类型 |
|------|------|-------------|--------|
| `domain` | 所属行业/专业领域 | Single | Hybrid (候选 10 个 + 开放) |
| `topic` | 具体话题 | Multi(3) | Open |
| `purpose` | 知识的角色（参考/笔记/待办/归档/灵感）| Single | Closed (6 个值) |
| `project` | 上下文归属 | Single | Open (空值允许) |
| `entities` | 命名实体（人/组织/产品）| Multi(10) | Open |

`domain` 候选值（Hybrid，LLM 可生成候选外的值）:
- 技术 / 商业 / 法律 / 医疗 / 金融 / 生活 / 学习 / 科研 / 艺术 / 政策

`purpose` 候选值（Closed，严格限定）:
- 参考资料 / 个人笔记 / 待办草稿 / 问答记录 / 归档 / 灵感

### 4.2 通用扩展 3 维（默认启用）

| 维度 | 含义 | cardinality | 值类型 |
|------|------|-------------|--------|
| `difficulty` | 内容专业深度 | Single | Closed (入门/进阶/专家/N/A) |
| `freshness` | 知识保质期 | Single | Closed (常青/时效性/已过期) |
| `action_type` | 是否需要行动 | Single | Closed (待办/学习/参考/决策依据/纯归档) |

### 4.3 自动维度（系统生成，不用 LLM）

| 维度 | 来源 | 值 |
|------|------|----|
| `created_date_bucket` | `items.created_at` | today / this_week / this_month / this_quarter / older |
| `source_type` | `items.source_type` | webpage / ai_chat / file / note |
| `language` | 简单字符集检测 | zh / en / code / mixed |
| `content_length_bucket` | `items.content.len()` | short(<500) / medium(500-5000) / long(>5000) |

这 4 个维度在 `tag_index.build()` 时从已有字段计算，不消耗 LLM。

### 4.4 内置插件维度

#### 编程/技术 (`tech`)

| 维度 | 含义 | cardinality | 候选值 |
|------|------|-------------|--------|
| `stack_layer` | 技术栈层次 | Multi(3) | 前端 / 后端 / 数据库 / 基础设施 / DevOps / 客户端 / 嵌入式 / AI/ML |
| `language_tech` | 语言/技术 | Multi(5) | Rust / Python / JS/TS / Go / Java / C/C++ / SQL / Shell |
| `design_pattern` | 设计/实践 | Multi(3) | Open |

#### 法律 (`law`)

| 维度 | 含义 | cardinality | 候选值 |
|------|------|-------------|--------|
| `law_branch` | 法律部门 | Single | 民法 / 刑法 / 行政法 / 商事法 / 知识产权法 / 劳动法 / 诉讼法 / 国际私法 / 税法 / 合规 |
| `doc_type` | 文档类型 | Single | Closed (法条引用/判例/合同范本/咨询意见/案情分析/备忘录/政策解读) |
| `jurisdiction` | 管辖区 | Multi(2) | 中国大陆 / 香港 / 澳门 / 台湾 / 美国 / 欧盟 / 英国 / 新加坡 / 国际公约 |
| `risk_level` | 风险等级 | Single | Closed (高/中/低/已规避/不适用) |

两个插件的完整定义以 YAML 形式存于 `crates/vault-core/assets/plugins/tech.yaml` 和 `law.yaml`，通过 `include_str!` 编译进二进制。

## 5. 存储 Schema

### 5.1 SQLite 层改动

**items 表**：不新增字段，扩展 `tags` BLOB 字段的语义。

**embed_queue 表迁移**：
```sql
ALTER TABLE embed_queue ADD COLUMN task_type TEXT NOT NULL DEFAULT 'embed';
-- 'embed' = 已有的 embedding 任务
-- 'classify' = 新增的分类任务
```

迁移脚本在 `Store::open` 时幂等执行（检测列不存在才 ALTER）。

### 5.2 tags 字段新格式（加密 JSON）

```json
{
  "version": 1,
  "classified_at": "2026-04-11T10:23:45Z",
  "model": "qwen2.5:3b",
  "plugins_used": ["tech"],

  "core": {
    "domain": ["技术"],
    "topic": ["Rust 内存安全", "加密存储"],
    "purpose": ["参考资料"],
    "project": ["npu-vault"],
    "entities": ["rustls", "aes-gcm", "zeroize"]
  },
  "universal": {
    "difficulty": "进阶",
    "freshness": "常青",
    "action_type": "学习"
  },
  "plugin": {
    "tech": {
      "stack_layer": ["后端", "基础设施"],
      "language_tech": ["Rust"],
      "design_pattern": ["内存清零"]
    }
  },

  "user_tags": []
}
```

历史数据兼容：`tags` 为数组格式时，迁移到 `user_tags` 字段，其余留空。

### 5.3 vault_meta 聚类快照

```
key = cluster_snapshot
value = 加密 JSON
```

内容:
```json
{
  "version": 1,
  "generated_at": "2026-04-11T10:30:00Z",
  "algorithm": "hdbscan",
  "model": "qwen2.5:3b",
  "clusters": [
    {
      "id": 0,
      "name": "Rust 加密栈研究",
      "summary": "围绕 vault-core 加密模块的 23 条笔记",
      "item_count": 23,
      "item_ids": ["abc123", "def456", "..."],
      "representative_item_id": "abc123"
    }
  ],
  "noise_item_ids": ["xyz789"]
}
```

## 6. TagIndex 内存索引

### 6.1 数据结构

```rust
pub struct TagIndex {
    // (dimension, value) → set of item_ids
    forward: HashMap<(String, String), HashSet<String>>,
    // item_id → list of (dimension, value)
    reverse: HashMap<String, Vec<(String, String)>>,
}
```

### 6.2 生命周期

- `init_search_engines()` 时调用 `TagIndex::build(&store, &dek)`，扫描所有 items，解密 tags，构建索引。
- `clear_search_engines()` 时 `tag_index = None`，所有数据从内存消失。
- `ingest` / `classify` / `reclassify` 后调用 `tag_index.upsert(item_id, tags)`。
- `delete_item` 后调用 `tag_index.remove(item_id)`。

### 6.3 核心方法

```rust
impl TagIndex {
    pub fn build(store: &Store, dek: &Key32) -> Result<Self>;

    pub fn query(&self, dimension: &str, value: &str) -> Vec<&str>;
    pub fn query_and(&self, filters: &[(&str, &str)]) -> Vec<&str>;
    pub fn query_or(&self, filters: &[(&str, &str)]) -> Vec<&str>;

    pub fn upsert(&mut self, item_id: &str, tags: &ClassificationResult);
    pub fn remove(&mut self, item_id: &str);

    pub fn histogram(&self, dimension: &str) -> Vec<(String, usize)>;
    pub fn all_dimensions(&self) -> Vec<&str>;
}
```

### 6.4 性能目标

- 构建时间: < 500 ms @ 10K items
- 单维度单值查询: < 1 μs (HashMap 直接查找)
- 复合过滤: O(min filter size)
- 直方图聚合: O(N) 但只扫描 forward index

## 7. LLM 客户端 (llm.rs)

### 7.1 Trait 定义

```rust
pub trait LlmProvider: Send + Sync {
    fn chat(&self, system: &str, user: &str) -> Result<String>;
    fn is_available(&self) -> bool;
    fn model_name(&self) -> &str;
}
```

### 7.2 OllamaLlmProvider

```rust
pub struct OllamaLlmProvider {
    client: reqwest::Client,
    base_url: String,    // 默认 http://localhost:11434
    model: String,
}

impl OllamaLlmProvider {
    /// 按优先级自动探测: qwen2.5:7b > qwen2.5:3b > llama3.2:3b > phi3:mini
    pub fn auto_detect() -> Result<Self>;

    /// 显式指定模型
    pub fn with_model(model: &str) -> Self;
}
```

HTTP 调用 `POST /api/chat` 使用 `format: "json"` 强制 JSON 输出。与 `embed.rs` 一样使用 `spawn_blocking` 避免嵌套 runtime。

### 7.3 MockLlmProvider (测试专用)

```rust
#[cfg(test)]
pub struct MockLlmProvider {
    responses: Mutex<VecDeque<String>>,
}
```

测试通过 `push_response(json)` 预置响应，验证 classifier / clusterer 的整个 pipeline 不需要真实 Ollama。

## 8. Taxonomy 引擎 (taxonomy.rs)

### 8.1 数据结构

```rust
pub struct Dimension {
    pub name: String,
    pub label: String,              // 中文显示名
    pub description: String,        // 给 LLM 的说明
    pub cardinality: Cardinality,
    pub value_type: ValueType,
    pub suggested_values: Vec<String>,
}

pub enum Cardinality {
    Single,
    Multi(usize),
}

pub enum ValueType {
    Open,
    Closed(Vec<String>),
    Hybrid,  // 候选值 + 允许生成其他
}

pub struct Plugin {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub dimensions: Vec<Dimension>,
    pub prompt_hint: String,
}

pub struct Taxonomy {
    pub core: Vec<Dimension>,
    pub universal: Vec<Dimension>,
    pub plugins: Vec<Plugin>,
}
```

### 8.2 Taxonomy 构建

```rust
impl Taxonomy {
    /// 加载默认 (core + universal)，不含任何插件
    pub fn default() -> Self;

    /// 从内置 YAML 加载所有插件定义
    pub fn load_builtin_plugins() -> Result<Vec<Plugin>>;

    /// 启用某插件
    pub fn with_plugin(mut self, plugin: Plugin) -> Self;

    /// 构建 LLM system prompt (包含所有已启用维度的定义)
    pub fn build_system_prompt(&self) -> String;

    /// 构建 LLM user prompt (批量嵌入 items)
    pub fn build_user_prompt(&self, items: &[(String, String)]) -> String;

    /// 解析 LLM JSON 响应
    pub fn parse_response(&self, json: &str) -> Result<Vec<ClassificationResult>>;

    /// 验证分类结果符合 schema
    pub fn validate(&self, result: &ClassificationResult) -> Result<()>;
}
```

### 8.3 Prompt 模板

**System prompt**：
```
你是一个知识库自动分类助手。给定文本内容，输出严格的 JSON 分类结果。

维度定义：
{DIMENSIONS_JSON}

输出格式（严格遵守此 JSON schema）:
{
  "core": {"domain": [...], "topic": [...], "purpose": [...], "project": [...], "entities": [...]},
  "universal": {"difficulty": "...", "freshness": "...", "action_type": "..."},
  "plugin": {"{plugin_id}": {"{dim_name}": [...]}}
}

规则:
- 数组字段至少 1 个值
- Closed 类型字段只能从候选值中选择
- Hybrid 类型字段优先从候选值中选择，确实不匹配时才生成新值
- 批量输入时，返回 JSON 数组，顺序与输入一致
```

**User prompt**（批量 5 条）：
```
请分类以下 {N} 条内容:

[1]
标题: {title_1}
内容: {content_1_truncated_2000_chars}

[2]
...
```

### 8.4 插件 YAML 格式示例

`crates/vault-core/assets/plugins/tech.yaml`:

```yaml
id: tech
name: 编程/技术
version: "1.0"
description: 软件开发、架构、工程实践相关知识
dimensions:
  - name: stack_layer
    label: 技术栈层次
    description: 内容所属的技术栈层级
    cardinality:
      type: Multi
      max: 3
    value_type:
      type: Hybrid
    suggested_values:
      - 前端
      - 后端
      - 数据库
      - 基础设施
      - DevOps / CI/CD
      - 客户端（桌面/移动）
      - 嵌入式
      - AI/ML

  - name: language_tech
    label: 语言/技术
    description: 涉及的编程语言、框架、工具
    cardinality:
      type: Multi
      max: 5
    value_type:
      type: Hybrid
    suggested_values:
      - Rust
      - Python
      - JavaScript / TypeScript
      - Go
      - Java / Kotlin
      - C / C++
      - SQL
      - Shell / Bash

  - name: design_pattern
    label: 设计/实践
    description: 涉及的架构模式、设计模式、工程实践
    cardinality:
      type: Multi
      max: 3
    value_type:
      type: Open
    suggested_values: []

prompt_hint: |
  这是软件工程相关的内容。请识别技术栈层次（前端/后端/基础设施等）、
  涉及的具体语言或框架、以及相关的架构或实践模式。
```

`crates/vault-core/assets/plugins/law.yaml` 结构同上，定义 `law_branch` / `doc_type` / `jurisdiction` / `risk_level` 四个维度。

## 9. 分类 Pipeline (classifier.rs)

```rust
pub struct Classifier {
    taxonomy: Arc<Taxonomy>,
    llm: Arc<dyn LlmProvider>,
    batch_size: usize,  // 默认 5
}

impl Classifier {
    pub fn classify_one(&self, title: &str, content: &str) -> Result<ClassificationResult>;
    pub fn classify_batch(&self, items: &[(String, String)]) -> Result<Vec<ClassificationResult>>;
}

pub struct ClassificationResult {
    pub core: CoreTags,
    pub universal: UniversalTags,
    pub plugin: HashMap<String, serde_json::Value>,
    pub classified_at: String,
    pub model: String,
    pub plugins_used: Vec<String>,
}
```

**批量策略**：一次 LLM 调用处理最多 5 条，摊薄首 token 开销。Prompt 构造 + 解析在 taxonomy 模块。

**容错**：解析 JSON 失败时（例如 LLM 返回了额外文字），尝试提取 JSON 代码块。依然失败则：
- `classify_one`: 返回错误
- `classify_batch`: 跳过该条，其余正常处理，返回 `Vec<Result<...>>` 让调用方决定

**Schema 验证**：`Taxonomy::validate` 检查：
- 所有必需维度存在（core + universal + 启用的插件）
- cardinality 限制（Single 只能 1 值，Multi(N) 最多 N 值）
- Closed 值必须在候选集内
- 不合规时裁剪到合规状态（而非报错）

## 10. 聚类 Pipeline (clusterer.rs)

### 10.1 算法选择

HDBSCAN (`hdbscan` crate)，理由：
- 自动发现聚类数（k-means 需预定 k）
- 支持噪声点（那些不属于任何聚类的孤立 item）
- 对密度变化的聚类友好

### 10.2 流程

```
rebuild()
  ↓
1. 收集所有 Level 1 (章节级) 向量作为代表
  ↓
2. 向量数量检查: if < MIN_ITEMS_FOR_CLUSTERING (默认 20) → 返回空快照
  ↓
3. HDBSCAN(vectors, min_cluster_size=max(3, n/30), min_samples=1)
   → 返回每个向量的 label (-1 = 噪声, 0+ = 聚类 ID)
  ↓
4. 按 label 分组
  ↓
5. 对每个非噪声聚类:
   a. 找到质心最近的 3 个代表样本
   b. 解密代表样本的 content
   c. 调用 LLM: "给这个聚类起名 + 写一句话摘要"
   d. 得到 {name, summary}
  ↓
6. 包装为 ClusterSnapshot，加密存储到 vault_meta
```

### 10.3 LLM 命名 Prompt

```
SYSTEM: 你是一个知识库聚类命名助手。给定一组相关的知识片段，生成简洁的主题名和一句话摘要。

USER:
以下是一个聚类中的 3 个代表样本:

- {title_1}: {content_1_truncated_300}
- {title_2}: {content_2_truncated_300}
- {title_3}: {content_3_truncated_300}

请输出 JSON:
{
  "name": "主题名 (8-15 字)",
  "summary": "一句话摘要 (20-40 字)"
}
```

### 10.4 性能

- HDBSCAN @ 1000 向量: < 100 ms
- HDBSCAN @ 10000 向量: < 1 s
- LLM 命名: 每聚类 1-2 秒 (qwen2.5:3b)
- 假设 10K 条产生 30 个聚类，总耗时约 60 秒

### 10.5 触发方式

- **手动**: `POST /clusters/rebuild`
- **定期**: Queue Worker 每 24h 检查 `cluster_snapshot.generated_at`，超时则自动触发

## 11. API 端点

### 11.1 新增端点

```
POST   /api/v1/classify/{item_id}       单条重分类
POST   /api/v1/classify/rebuild         全量重分类（异步，返回 job_id）
GET    /api/v1/classify/status          当前分类进度 (pending/processing/done counts)

GET    /api/v1/tags                     所有维度的直方图 (不含 entities)
GET    /api/v1/tags/{dimension}         某维度所有值 + 对应 item 数量

POST   /api/v1/clusters/rebuild         手动重跑聚类
GET    /api/v1/clusters                 当前聚类快照 (列表)
GET    /api/v1/clusters/{id}            某聚类详情 + 全部 item_ids

GET    /api/v1/plugins                  列出所有可用插件 (built-in)
POST   /api/v1/plugins/{id}/enable      启用插件 (触发 reclassify_all)
POST   /api/v1/plugins/{id}/disable     禁用插件
```

### 11.2 扩展端点

**`/items` 增加 tag 过滤参数**:
```
GET /items?tag=domain:技术&tag=purpose:参考资料&limit=20
```
多个 `tag` 参数为 AND 关系。

**`/search` 增加 tag / cluster 过滤参数**:
```
GET /search?q=加密&tag=domain:技术&cluster=7&top_k=10
```
过滤顺序: tag AND cluster → 在交集内执行 BM25 + 向量 + RRF。

### 11.3 settings 新增字段

```json
{
  "classify": {
    "enabled": true,
    "model": "auto",
    "batch_size": 5,
    "enabled_plugins": ["tech", "law"],
    "auto_cluster_interval_hours": 24,
    "min_items_for_clustering": 20
  }
}
```

## 12. Queue Worker 扩展

`embed_queue.task_type` 列区分 `embed` / `classify`。Worker 在 `process_batch` 中分派:

```rust
fn process_batch(&self) -> Result<usize> {
    let tasks = store.dequeue_tasks(BATCH_SIZE)?;
    let (embed_tasks, classify_tasks): (Vec<_>, Vec<_>) =
        tasks.into_iter().partition(|t| t.task_type == "embed");

    if !embed_tasks.is_empty() {
        self.process_embed_batch(embed_tasks)?;
    }
    if !classify_tasks.is_empty() {
        self.process_classify_batch(classify_tasks)?;
    }
    Ok(total)
}

fn process_classify_batch(&self, tasks: Vec<Task>) -> Result<()> {
    // 1. 批量读取 content (每 5 条一组)
    // 2. Classifier::classify_batch
    // 3. 对每条: store.update_tags(item_id, result), tag_index.upsert
    // 4. 标记 done
}
```

**优先级**: classify task `priority=3`, reclassify task `priority=4`。低于 embed 的 `priority=2`，确保 ingest 后搜索立即可用。

## 13. Web UI 最小集成

### 13.1 新增两个标签页

`crates/vault-server/assets/index.html` 扩展：

```
[搜索] [录入] [条目] [分类] [聚类] [设置]
                      ^^^^^^ ^^^^^^
                      新增    新增
```

### 13.2 分类标签页

左右布局:
- **左**: 维度选择器 (dropdown): `domain / topic / purpose / project / difficulty / ... / [插件维度]`
- **右**: 直方图渲染 (每个值一行，值 + count + 进度条)
- **点击某个值**: 右下区域显示该值下的 items 列表
- **顶部**: 已选过滤器标签 (AND 组合), "清空" 按钮
- **底部**: "重新分类全部" 按钮 + 进度条 (轮询 /classify/status)

### 13.3 聚类标签页

- **顶部**: "上次聚类: {timestamp}" + "重新聚类" 按钮
- **主区**: 聚类卡片网格
  - 标题: `🔬 {cluster.name}`
  - 摘要: {cluster.summary}
  - 数量: {item_count} 条
  - 代表标题列表 (前 3 条)
- **点击卡片**: 展开显示该聚类下全部 items

### 13.4 录入页增强

ingest/upload 完成后:
- Toast: "已入库 (ID: xxx)，分类中..."
- 轮询 `/classify/status`，完成后 toast "分类完成，已自动标记为 domain=技术 ..."

## 14. 配置与部署

### 14.1 新用户

启动 vault-server 时:
1. 连接 Ollama, `GET /api/tags` 获取已下载模型
2. 筛选 chat 模型 (排除 bge-m3 等 embedding 模型)
3. 按优先级选择 (qwen2.5:7b > qwen2.5:3b > llama3.2:3b > phi3:mini)
4. 若无可用 chat 模型:
   - 启动成功，但 `/classify/*` 和 `/clusters/*` 返回 503 + 提示
   - 提示内容: "请安装 chat 模型: `ollama pull qwen2.5:3b`"

### 14.2 现有用户升级

首次启动检测:
1. `embed_queue` 缺少 `task_type` 列 → 运行 ALTER TABLE
2. items 存在但 tags 为空 → UI 显示"点击开始自动分类您的 X 条知识"按钮
3. 用户点击 → `POST /classify/rebuild` → 后台异步处理

### 14.3 模型切换

用户在 settings 修改 `classify.model`:
- 从 `auto` 改为指定模型: 立即生效
- 新分类任务使用新模型
- 旧分类结果保留 (可以触发 reclassify_all 强制重做)

### 14.4 插件启用/禁用

启用插件:
- 更新 settings.classify.enabled_plugins
- 自动触发 reclassify_all (priority=4, 不阻塞)
- 进度可在 /classify/status 查看

禁用插件:
- 更新 settings.classify.enabled_plugins
- 从所有 items.tags.plugin 中移除该插件的维度 (同步操作)
- 不需要重新 LLM 调用

## 15. 安全考量

### 15.1 LLM 请求内容

- 分类请求会将知识的 **明文 title + content** 发送给 Ollama
- Ollama 本地运行，内容不离开本机
- NAS 模式下 Ollama 应在同一台 NAS 上运行（或通过内网访问）
- 日志配置: 分类 prompt 默认不写入日志，避免泄露到 log 文件

### 15.2 tags 存储

- tags 作为加密 JSON 存于 items.tags BLOB
- 与 content 使用相同 DEK 加密
- lock 状态下无法读取 tags (但 items.title 明文可见)

### 15.3 TagIndex 内存安全

- TagIndex 只在 UNLOCKED 状态存在
- lock 时直接 drop，释放所有 tag 内容
- tag 值是明文字符串（HashMap key），attacker 访问进程内存可以看到
- 这是已接受的 trade-off（为了查询性能）

## 16. 测试策略

### 16.1 单元测试目标

| 模块 | 关键测试 | 数量 |
|------|---------|------|
| `llm.rs` | OllamaLlmProvider 创建, auto_detect 逻辑, NoopLlmProvider | 3 |
| `taxonomy.rs` | 插件 YAML 解析, prompt 构建, JSON 解析, validate, cardinality 裁剪 | 6 |
| `classifier.rs` | classify_one, classify_batch, Mock LLM, 解析失败容错, 部分结果 | 5 |
| `clusterer.rs` | 少于阈值不跑, HDBSCAN 正确性, 噪声点处理, LLM 命名 | 4 |
| `tag_index.rs` | build, query, query_and, query_or, upsert, remove, histogram | 7 |
| `store.rs` 扩展 | task_type 迁移幂等性, list_all_item_ids, enqueue_classify | 3 |

**小计新增**: 28 单元测试

### 16.2 集成测试 (tests/classifier_test.rs)

- `e2e_classify_flow`: setup → ingest 5 条 → Mock LLM 返回预设 JSON → classify → 验证 tag_index 正确
- `e2e_reclassify_all`: 启用插件 → reclassify → 验证 items.tags 全部更新
- `e2e_cluster_snapshot`: ingest 30 条 → rebuild clusters → 验证 vault_meta 快照

**小计新增**: 3 集成测试

### 16.3 Smoke 测试

`tests/SMOKE_CLASSIFICATION.md` 手动步骤:
1. 启动 Ollama + qwen2.5:3b
2. `npu-vault-server` 启动
3. setup + ingest 真实中英文混合内容 10 条
4. 等待分类完成
5. `GET /tags/domain` 验证直方图
6. `POST /clusters/rebuild` + `GET /clusters` 验证聚类结果

### 16.4 测试数量小结

Baseline: 78 tests (75 unit + 3 integration)
New: 28 unit + 3 integration = 31
Total: **106 tests** after A 子系统完成

## 17. 非功能需求

| 指标 | 目标 | 备注 |
|------|------|------|
| 单条分类延迟 | < 3 秒 (qwen2.5:3b CPU) | 批量 5 条总计 < 10 秒 |
| 批量分类吞吐 | > 30 items/min (CPU), > 200 items/min (GPU) | |
| TagIndex 构建时间 | < 500 ms @ 10K items | 在 init_search_engines 中执行 |
| 单维度直方图查询 | < 10 ms | 内存 HashMap 聚合 |
| 聚类完整运行 | < 60 s @ 10K items | 含所有 LLM 命名调用 |
| 二进制大小增量 | < 5 MB | hdbscan ~200 KB + 插件 YAML ~10 KB + LLM client 复用现有 reqwest |

## 18. 非目标 (Out of Scope)

以下内容**不在** A 子系统范围内，避免范围蔓延:

- **运行时插件加载** — 插件 YAML 编译时嵌入，不支持用户加载外部 .yaml (D 子系统处理)
- **插件市场** — 不提供下载/发现机制 (D)
- **行为画像** — 不追踪搜索历史、点击、用户偏好 (B)
- **完整 UI 重构** — 只在现有最小 UI 加 2 个标签页，不升级整体布局 (C)
- **画像导出** — 不提供 tag/cluster 结果的独立导出格式 (E)
- **NAS 远程目录扫描** — 仅支持本地目录 (F1)
- **多语言 UI** — Web UI 仅中文 (未来可选)
- **OCR / 图片分类** — 不处理图像内容，仅处理文本

## 19. 后续方向队列 (依赖本 spec 完成)

按优先级列出后续子系统，每个都会走 brainstorming → spec → plan → implementation 循环:

| 优先级 | 子系统 | 简述 |
|--------|--------|------|
| 2 | **B — 个人行为画像** | 搜索历史埋点、点击权重、偏好模型 |
| 3 | **C — Web UI 全面升级** | 升级为完整 SPA, 分类浏览, 批量操作 |
| 4 | **D — 运行时插件系统** | 用户导入 .yaml 插件、插件市场 |
| 5 | **E — 画像/分类导出** | `.vault-profile` 可迁移文件 |
| 6 | **F1 — NAS 远程目录** | SMB / NFS / S3 / WebDAV 挂载 |
| 7 | **F2 — Tauri 桌面客户端** | 系统托盘 + 原生窗口 |
| 8 | **F3 — Queue Worker 自启动** | server 启动时自动消费队列 |
| 9 | **F4 — 索引持久化加密** | 启用 DEK_idx 和 DEK_vec |
| 10 | **F5 — 云同步** | E2E 加密备份到对象存储 |

## 20. 交付里程碑

A 子系统分为 4 个实现阶段，在单个 plan 中按顺序完成:

1. **基础设施** — error 扩展, llm.rs, taxonomy.rs, 插件 YAML, store 迁移
2. **核心 Pipeline** — classifier.rs, clusterer.rs, tag_index.rs
3. **API + Queue 集成** — routes/{classify,tags,clusters,plugins}, queue 扩展, AppState 扩展
4. **Web UI + 文档** — index.html 增加 2 个标签页, README/DEVELOP/RELEASE 更新

每个阶段完成后运行 `cargo test` 验证通过才进入下一阶段。

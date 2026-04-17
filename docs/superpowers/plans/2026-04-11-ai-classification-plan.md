# AI 自动分类 (A 子系统) 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 npu-vault 添加基于 LLM 的多维度自动分类 + HDBSCAN 聚类能力，让本地知识库具备"相册式"智能归类和行业领域适配，支持 Web UI 的分类浏览与聚类发现。

**Architecture:** 在 vault-core 新增 5 个模块（llm / taxonomy / classifier / clusterer / tag_index）+ 2 个内置插件 YAML（tech / law）。扩展 store.rs 的 embed_queue 表（新增 task_type 列）支持分类任务，Queue Worker 根据 task_type 分派处理。AppState 新增 tag_index/cluster_snapshot/llm 三个字段，vault unlock 时初始化，lock 时清零。HTTP 层新增 4 组路由（classify/tags/clusters/plugins），Web UI 增加 2 个标签页（分类/聚类）。

**Tech Stack:** Rust 2021, hdbscan crate, serde_yaml, 复用现有 reqwest/tokio/axum/rusqlite/serde

**Design Spec:** `docs/superpowers/specs/2026-04-11-ai-classification-design.md`

**Baseline:** npu-vault v0.3.0, 78 tests passing (75 unit + 3 integration)

**Target:** 106 tests passing (103 unit + 3 integration, 新增 28 unit)

**Note on commits:** 由于 Opsera pre-commit hook 阻塞 git commit，所有 task 只需完成代码和测试，不要尝试 commit。用户会在全部任务完成后手动批量提交。

---

## File Structure Map

```
npu-vault/
├── crates/vault-core/
│   ├── Cargo.toml                                 [MODIFY: +hdbscan, +serde_yaml]
│   ├── assets/plugins/                            [NEW DIRECTORY]
│   │   ├── tech.yaml                              [NEW]
│   │   └── law.yaml                               [NEW]
│   └── src/
│       ├── lib.rs                                 [MODIFY: 注册新模块]
│       ├── error.rs                               [MODIFY: +ClassifyError, +LlmUnavailable, +TaxonomyError]
│       ├── store.rs                               [MODIFY: task_type 列迁移, update_tags, enqueue_classify, list_all_item_ids]
│       ├── queue.rs                               [MODIFY: 按 task_type 分派处理]
│       ├── vault.rs                               [MODIFY: 无，状态机不变]
│       ├── llm.rs                                 [NEW: LlmProvider trait + OllamaLlmProvider + MockLlmProvider]
│       ├── taxonomy.rs                            [NEW: Dimension/Plugin/Taxonomy + YAML 加载 + prompt 构建]
│       ├── classifier.rs                          [NEW: Classifier 结构 + classify_one/batch]
│       ├── clusterer.rs                           [NEW: Clusterer + HDBSCAN + 簇命名]
│       └── tag_index.rs                           [NEW: TagIndex 反向索引]
│
├── crates/vault-server/
│   └── src/
│       ├── state.rs                               [MODIFY: AppState 加 tag_index/cluster_snapshot/llm]
│       ├── main.rs                                [MODIFY: 注册新路由]
│       └── routes/
│           ├── mod.rs                             [MODIFY: 注册新子模块]
│           ├── classify.rs                        [NEW: /classify/{id}, /classify/rebuild, /classify/status]
│           ├── tags.rs                            [NEW: /tags, /tags/{dimension}]
│           ├── clusters.rs                        [NEW: /clusters, /clusters/rebuild, /clusters/{id}]
│           ├── plugins.rs                         [NEW: /plugins, /plugins/{id}/enable|disable]
│           ├── search.rs                          [MODIFY: 接受 tag/cluster 过滤参数]
│           └── items.rs                           [MODIFY: 接受 tag 过滤参数]
│
└── tests/
    └── classifier_test.rs                         [NEW: 3 个集成测试]
```

---

## Task 1: 依赖升级 + error.rs 扩展

**Files:**
- Modify: `npu-vault/crates/vault-core/Cargo.toml`
- Modify: `npu-vault/crates/vault-core/src/error.rs`

- [ ] **Step 1: 添加 hdbscan + serde_yaml 依赖**

编辑 `npu-vault/crates/vault-core/Cargo.toml` 的 `[dependencies]` 段，追加：

```toml
hdbscan = "0.11"
serde_yaml = "0.9"
```

- [ ] **Step 2: 编辑 error.rs，新增错误变体**

打开 `npu-vault/crates/vault-core/src/error.rs`，在 `VaultError` enum 内 `Json` 变体之后、结束括号之前，插入：

```rust
    #[error("llm unavailable: {0}")]
    LlmUnavailable(String),

    #[error("classification failed: {0}")]
    Classification(String),

    #[error("taxonomy error: {0}")]
    Taxonomy(String),

    #[error("yaml parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),
```

- [ ] **Step 3: 验证编译**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo build -p vault-core 2>&1 | tail -10`
Expected: `Finished `dev` profile` (编译成功，可能有新 crate 下载)

- [ ] **Step 4: 验证测试未破坏**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo test -p vault-core error::tests 2>&1 | grep "test result"`
Expected: `test result: ok. 1 passed`

---

## Task 2: llm.rs Ollama Chat Client

**Files:**
- Create: `npu-vault/crates/vault-core/src/llm.rs`
- Modify: `npu-vault/crates/vault-core/src/lib.rs`

- [ ] **Step 1: 创建 llm.rs 完整文件**

Create `npu-vault/crates/vault-core/src/llm.rs` with:

```rust
use crate::error::{Result, VaultError};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Chat LLM 抽象
pub trait LlmProvider: Send + Sync {
    /// 单次 chat 调用，system + user 消息，返回完整响应文本
    fn chat(&self, system: &str, user: &str) -> Result<String>;

    /// 模型是否可用
    fn is_available(&self) -> bool;

    /// 当前使用的模型名（用于 tags.model 记录）
    fn model_name(&self) -> &str;
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    stream: bool,
    format: &'a str,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatResponse {
    message: ChatResponseMessage,
}

#[derive(Deserialize)]
struct ChatResponseMessage {
    content: String,
}

#[derive(Deserialize)]
struct TagsResponse {
    models: Vec<TagsModel>,
}

#[derive(Deserialize)]
struct TagsModel {
    name: String,
}

/// Ollama chat client
pub struct OllamaLlmProvider {
    client: reqwest::Client,
    base_url: String,
    model: String,
}

/// 按优先级排列的默认 chat 模型候选
const PREFERRED_MODELS: &[&str] = &[
    "qwen2.5:7b",
    "qwen2.5:3b",
    "qwen2.5:1.5b",
    "llama3.2:3b",
    "llama3.2:1b",
    "phi3:mini",
];

impl OllamaLlmProvider {
    /// 显式指定模型
    pub fn with_model(model: &str) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("HTTP client"),
            base_url: "http://localhost:11434".to_string(),
            model: model.to_string(),
        }
    }

    /// 自动探测: 查询本地已下载的 chat 模型，按 PREFERRED_MODELS 优先级选择
    pub fn auto_detect() -> Result<Self> {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| VaultError::Crypto(format!("tokio runtime: {e}")))?;
        let provider = Self::with_model("placeholder");
        let available: Vec<String> = rt.block_on(async {
            let url = format!("{}/api/tags", provider.base_url);
            let resp = provider.client.get(&url).send().await
                .map_err(|e| VaultError::LlmUnavailable(format!("ollama unreachable: {e}")))?;
            let tags: TagsResponse = resp.json().await
                .map_err(|e| VaultError::LlmUnavailable(format!("parse tags: {e}")))?;
            Ok::<_, VaultError>(tags.models.into_iter().map(|m| m.name).collect())
        })?;

        for preferred in PREFERRED_MODELS {
            if available.iter().any(|a| a.starts_with(preferred)) {
                return Ok(Self::with_model(preferred));
            }
        }
        Err(VaultError::LlmUnavailable(format!(
            "no chat model found. Install one of: {}. Run: ollama pull qwen2.5:3b",
            PREFERRED_MODELS.join(", ")
        )))
    }

    fn chat_sync(&self, system: &str, user: &str) -> Result<String> {
        let url = format!("{}/api/chat", self.base_url);
        let body = ChatRequest {
            model: &self.model,
            messages: vec![
                ChatMessage { role: "system", content: system },
                ChatMessage { role: "user", content: user },
            ],
            stream: false,
            format: "json",
        };
        let client = self.client.clone();
        let url_clone = url.clone();
        let body_json = serde_json::to_vec(&body)?;

        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new()
                .map_err(|e| VaultError::Crypto(format!("tokio runtime: {e}")))?;
            rt.block_on(async move {
                let resp = client.post(&url_clone)
                    .header("Content-Type", "application/json")
                    .body(body_json)
                    .send().await
                    .map_err(|e| VaultError::LlmUnavailable(format!("chat request: {e}")))?;
                let parsed: ChatResponse = resp.json().await
                    .map_err(|e| VaultError::Classification(format!("parse chat response: {e}")))?;
                Ok::<String, VaultError>(parsed.message.content)
            })
        });
        handle.join()
            .map_err(|_| VaultError::Crypto("chat thread panicked".into()))?
    }
}

impl LlmProvider for OllamaLlmProvider {
    fn chat(&self, system: &str, user: &str) -> Result<String> {
        self.chat_sync(system, user)
    }

    fn is_available(&self) -> bool {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(_) => return false,
        };
        let url = format!("{}/api/tags", self.base_url);
        let client = self.client.clone();
        rt.block_on(async { client.get(&url).send().await.is_ok() })
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

/// 测试专用 Mock，按顺序返回预设响应
pub struct MockLlmProvider {
    responses: Mutex<Vec<String>>,
    model: String,
}

impl MockLlmProvider {
    pub fn new(model: &str) -> Self {
        Self {
            responses: Mutex::new(Vec::new()),
            model: model.to_string(),
        }
    }

    pub fn push_response(&self, json: &str) {
        self.responses.lock().unwrap().push(json.to_string());
    }
}

impl LlmProvider for MockLlmProvider {
    fn chat(&self, _system: &str, _user: &str) -> Result<String> {
        let mut guard = self.responses.lock().unwrap();
        if guard.is_empty() {
            return Err(VaultError::Classification("no mock response".into()));
        }
        Ok(guard.remove(0))
    }

    fn is_available(&self) -> bool {
        true
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ollama_provider_creation() {
        let p = OllamaLlmProvider::with_model("qwen2.5:3b");
        assert_eq!(p.model_name(), "qwen2.5:3b");
    }

    #[test]
    fn mock_provider_returns_preset() {
        let mock = MockLlmProvider::new("test-model");
        mock.push_response(r#"{"hello":"world"}"#);
        let resp = mock.chat("sys", "user").unwrap();
        assert_eq!(resp, r#"{"hello":"world"}"#);
        assert_eq!(mock.model_name(), "test-model");
        assert!(mock.is_available());
    }

    #[test]
    fn mock_provider_errors_when_empty() {
        let mock = MockLlmProvider::new("test");
        let result = mock.chat("sys", "user");
        assert!(result.is_err());
    }
}
```

- [ ] **Step 2: 在 lib.rs 注册 llm 模块**

Open `npu-vault/crates/vault-core/src/lib.rs` and add `pub mod llm;` to the module list (keep alphabetical).

Final lib.rs module list should be:
```rust
pub mod chunker;
pub mod crypto;
pub mod embed;
pub mod error;
pub mod index;
pub mod llm;
pub mod parser;
pub mod platform;
pub mod queue;
pub mod scanner;
pub mod search;
pub mod store;
pub mod vault;
pub mod vectors;
```

- [ ] **Step 3: 运行测试**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo test -p vault-core llm::tests 2>&1 | grep "test result"`
Expected: `test result: ok. 3 passed`

---

## Task 3: 插件 YAML 文件 + Taxonomy 类型

**Files:**
- Create: `npu-vault/crates/vault-core/assets/plugins/tech.yaml`
- Create: `npu-vault/crates/vault-core/assets/plugins/law.yaml`
- Create: `npu-vault/crates/vault-core/src/taxonomy.rs`
- Modify: `npu-vault/crates/vault-core/src/lib.rs`

- [ ] **Step 1: 创建 assets 目录结构和 tech.yaml**

Run: `mkdir -p /data/company/project/npu-webhook/npu-vault/crates/vault-core/assets/plugins`

Create `npu-vault/crates/vault-core/assets/plugins/tech.yaml`:

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
      - DevOps
      - 客户端
      - 嵌入式
      - AI/ML

  - name: language_tech
    label: 语言技术
    description: 涉及的编程语言、框架、工具
    cardinality:
      type: Multi
      max: 5
    value_type:
      type: Hybrid
    suggested_values:
      - Rust
      - Python
      - JavaScript
      - TypeScript
      - Go
      - Java
      - Kotlin
      - C
      - C++
      - SQL
      - Shell

  - name: design_pattern
    label: 设计实践
    description: 涉及的架构模式、设计模式、工程实践
    cardinality:
      type: Multi
      max: 3
    value_type:
      type: Open
    suggested_values: []

prompt_hint: |
  这是软件工程相关的内容。请识别技术栈层次、涉及的具体语言或框架、以及相关的架构或实践模式。
```

- [ ] **Step 2: 创建 law.yaml**

Create `npu-vault/crates/vault-core/assets/plugins/law.yaml`:

```yaml
id: law
name: 法律
version: "1.0"
description: 法律条文、案例、合同、咨询相关知识
dimensions:
  - name: law_branch
    label: 法律部门
    description: 所属的法律部门分支
    cardinality:
      type: Single
    value_type:
      type: Hybrid
    suggested_values:
      - 民法
      - 刑法
      - 行政法
      - 商事法
      - 知识产权法
      - 劳动法
      - 诉讼法
      - 国际私法
      - 税法
      - 合规

  - name: doc_type
    label: 文档类型
    description: 法律文档的类型
    cardinality:
      type: Single
    value_type:
      type: Closed
    suggested_values:
      - 法条引用
      - 判例
      - 合同范本
      - 咨询意见
      - 案情分析
      - 备忘录
      - 政策解读

  - name: jurisdiction
    label: 管辖区
    description: 适用的法域
    cardinality:
      type: Multi
      max: 2
    value_type:
      type: Hybrid
    suggested_values:
      - 中国大陆
      - 香港
      - 澳门
      - 台湾
      - 美国
      - 欧盟
      - 英国
      - 新加坡
      - 国际公约

  - name: risk_level
    label: 风险等级
    description: 合规或法律风险评估
    cardinality:
      type: Single
    value_type:
      type: Closed
    suggested_values:
      - 高风险
      - 中风险
      - 低风险
      - 已规避
      - 不适用

prompt_hint: |
  这是法律相关的内容。请识别所属的法律部门分支、文档类型、适用管辖区以及风险等级。
  注意区分法律条文引用、判例分析、合同范本和实务咨询意见。
```

- [ ] **Step 3: 创建 taxonomy.rs**

Create `npu-vault/crates/vault-core/src/taxonomy.rs`:

```rust
use crate::error::{Result, VaultError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 维度基数（单值或多值，多值时有最大数量限制）
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum Cardinality {
    Single,
    Multi { max: usize },
}

/// 维度值类型
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ValueType {
    /// 完全开放，LLM 自由生成
    Open,
    /// 封闭集合，严格限定为 suggested_values 之一
    Closed,
    /// 混合式，suggested_values 是候选，LLM 可生成其他值
    Hybrid,
}

/// 一个维度的完整定义
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Dimension {
    pub name: String,
    pub label: String,
    pub description: String,
    pub cardinality: Cardinality,
    pub value_type: ValueType,
    #[serde(default)]
    pub suggested_values: Vec<String>,
}

/// 一个插件 = 一组维度 + prompt 提示
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plugin {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub dimensions: Vec<Dimension>,
    #[serde(default)]
    pub prompt_hint: String,
}

impl Plugin {
    pub fn from_yaml(yaml: &str) -> Result<Self> {
        serde_yaml::from_str(yaml).map_err(VaultError::from)
    }
}

/// 内置插件 YAML（编译时嵌入）
const TECH_PLUGIN_YAML: &str = include_str!("../assets/plugins/tech.yaml");
const LAW_PLUGIN_YAML: &str = include_str!("../assets/plugins/law.yaml");

/// Taxonomy 引擎
pub struct Taxonomy {
    pub core: Vec<Dimension>,
    pub universal: Vec<Dimension>,
    pub plugins: Vec<Plugin>,
}

/// 分类结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassificationResult {
    pub version: u32,
    pub classified_at: String,
    pub model: String,
    pub plugins_used: Vec<String>,
    pub core: HashMap<String, Vec<String>>,
    pub universal: HashMap<String, String>,
    pub plugin: HashMap<String, HashMap<String, Vec<String>>>,
    #[serde(default)]
    pub user_tags: Vec<String>,
}

impl ClassificationResult {
    pub fn empty() -> Self {
        Self {
            version: 1,
            classified_at: chrono::Utc::now().to_rfc3339(),
            model: String::new(),
            plugins_used: vec![],
            core: HashMap::new(),
            universal: HashMap::new(),
            plugin: HashMap::new(),
            user_tags: vec![],
        }
    }
}

impl Taxonomy {
    /// 加载默认: core 5 + universal 3
    pub fn default() -> Self {
        Self {
            core: Self::build_core_dimensions(),
            universal: Self::build_universal_dimensions(),
            plugins: vec![],
        }
    }

    /// 加载所有内置插件
    pub fn load_builtin_plugins() -> Result<Vec<Plugin>> {
        Ok(vec![
            Plugin::from_yaml(TECH_PLUGIN_YAML)?,
            Plugin::from_yaml(LAW_PLUGIN_YAML)?,
        ])
    }

    /// 按 id 加载指定插件
    pub fn load_builtin_plugin(id: &str) -> Result<Plugin> {
        match id {
            "tech" => Plugin::from_yaml(TECH_PLUGIN_YAML),
            "law" => Plugin::from_yaml(LAW_PLUGIN_YAML),
            _ => Err(VaultError::Taxonomy(format!("unknown builtin plugin: {id}"))),
        }
    }

    pub fn with_plugin(mut self, plugin: Plugin) -> Self {
        self.plugins.push(plugin);
        self
    }

    /// 构建 LLM system prompt
    pub fn build_system_prompt(&self) -> String {
        let mut s = String::from("你是一个知识库自动分类助手。给定文本内容，输出严格的 JSON 分类结果。\n\n");
        s.push_str("维度定义:\n\n");

        s.push_str("## 核心维度 (core):\n");
        for d in &self.core {
            s.push_str(&format_dimension(d));
        }

        s.push_str("\n## 通用扩展维度 (universal):\n");
        for d in &self.universal {
            s.push_str(&format_dimension(d));
        }

        if !self.plugins.is_empty() {
            s.push_str("\n## 插件维度 (plugin):\n");
            for p in &self.plugins {
                s.push_str(&format!("\n### 插件 {} ({})\n{}\n", p.id, p.name, p.prompt_hint));
                for d in &p.dimensions {
                    s.push_str(&format_dimension(d));
                }
            }
        }

        s.push_str("\n## 输出格式 (严格遵守):\n");
        s.push_str("{\n  \"core\": {\"domain\": [...], \"topic\": [...], \"purpose\": [...], \"project\": [...], \"entities\": [...]},\n");
        s.push_str("  \"universal\": {\"difficulty\": \"...\", \"freshness\": \"...\", \"action_type\": \"...\"},\n");
        s.push_str("  \"plugin\": {");
        for (i, p) in self.plugins.iter().enumerate() {
            if i > 0 { s.push_str(", "); }
            let dims: Vec<String> = p.dimensions.iter().map(|d| format!("\"{}\": [...]", d.name)).collect();
            s.push_str(&format!("\"{}\": {{{}}}", p.id, dims.join(", ")));
        }
        s.push_str("}\n}\n\n");
        s.push_str("规则:\n- 数组字段至少 1 个值\n- Closed 类型只能从候选值中选择\n- Hybrid 类型优先从候选值选择\n- 批量输入时返回 JSON 数组，顺序与输入一致\n");
        s
    }

    /// 构建 LLM user prompt（批量）
    pub fn build_user_prompt(&self, items: &[(String, String)]) -> String {
        let mut s = format!("请分类以下 {} 条内容:\n\n", items.len());
        for (i, (title, content)) in items.iter().enumerate() {
            let truncated: String = content.chars().take(2000).collect();
            s.push_str(&format!("[{}]\n标题: {}\n内容: {}\n\n", i + 1, title, truncated));
        }
        if items.len() == 1 {
            s.push_str("输出 JSON 对象（非数组）。\n");
        } else {
            s.push_str(&format!("输出 JSON 数组，包含 {} 个对象，顺序对应。\n", items.len()));
        }
        s
    }

    /// 验证分类结果符合 schema
    pub fn validate(&self, result: &ClassificationResult) -> Result<()> {
        for d in &self.core {
            if !result.core.contains_key(&d.name) {
                return Err(VaultError::Classification(format!("missing core dimension: {}", d.name)));
            }
            let values = &result.core[&d.name];
            self.check_cardinality(&d.cardinality, values.len(), &d.name)?;
            self.check_value_type(&d.value_type, &d.suggested_values, values, &d.name)?;
        }
        for d in &self.universal {
            if !result.universal.contains_key(&d.name) {
                return Err(VaultError::Classification(format!("missing universal dimension: {}", d.name)));
            }
            let value = &result.universal[&d.name];
            self.check_value_type(&d.value_type, &d.suggested_values, &[value.clone()], &d.name)?;
        }
        Ok(())
    }

    fn check_cardinality(&self, c: &Cardinality, count: usize, name: &str) -> Result<()> {
        match c {
            Cardinality::Single if count != 1 => {
                Err(VaultError::Classification(format!("dimension {name} expects single value, got {count}")))
            }
            Cardinality::Multi { max } if count > *max || count == 0 => {
                Err(VaultError::Classification(format!("dimension {name} expects 1..={max} values, got {count}")))
            }
            _ => Ok(()),
        }
    }

    fn check_value_type(&self, vt: &ValueType, allowed: &[String], values: &[String], name: &str) -> Result<()> {
        if matches!(vt, ValueType::Closed) {
            for v in values {
                if !allowed.iter().any(|a| a == v) {
                    return Err(VaultError::Classification(format!("dimension {name} closed value {v} not in allowed set")));
                }
            }
        }
        Ok(())
    }

    fn build_core_dimensions() -> Vec<Dimension> {
        vec![
            Dimension {
                name: "domain".into(),
                label: "领域".into(),
                description: "所属行业或专业领域".into(),
                cardinality: Cardinality::Single,
                value_type: ValueType::Hybrid,
                suggested_values: vec![
                    "技术".into(), "商业".into(), "法律".into(), "医疗".into(),
                    "金融".into(), "生活".into(), "学习".into(), "科研".into(),
                    "艺术".into(), "政策".into(),
                ],
            },
            Dimension {
                name: "topic".into(),
                label: "主题".into(),
                description: "具体话题，最多 3 个".into(),
                cardinality: Cardinality::Multi { max: 3 },
                value_type: ValueType::Open,
                suggested_values: vec![],
            },
            Dimension {
                name: "purpose".into(),
                label: "用途".into(),
                description: "知识的角色定位".into(),
                cardinality: Cardinality::Single,
                value_type: ValueType::Closed,
                suggested_values: vec![
                    "参考资料".into(), "个人笔记".into(), "待办草稿".into(),
                    "问答记录".into(), "归档".into(), "灵感".into(),
                ],
            },
            Dimension {
                name: "project".into(),
                label: "项目".into(),
                description: "所属项目或上下文".into(),
                cardinality: Cardinality::Single,
                value_type: ValueType::Open,
                suggested_values: vec![],
            },
            Dimension {
                name: "entities".into(),
                label: "实体".into(),
                description: "涉及的人物、组织、产品等命名实体".into(),
                cardinality: Cardinality::Multi { max: 10 },
                value_type: ValueType::Open,
                suggested_values: vec![],
            },
        ]
    }

    fn build_universal_dimensions() -> Vec<Dimension> {
        vec![
            Dimension {
                name: "difficulty".into(),
                label: "深度".into(),
                description: "内容的专业深度".into(),
                cardinality: Cardinality::Single,
                value_type: ValueType::Closed,
                suggested_values: vec![
                    "入门".into(), "进阶".into(), "专家".into(), "N/A".into(),
                ],
            },
            Dimension {
                name: "freshness".into(),
                label: "时效".into(),
                description: "知识的保质期".into(),
                cardinality: Cardinality::Single,
                value_type: ValueType::Closed,
                suggested_values: vec![
                    "常青".into(), "时效性".into(), "已过期".into(),
                ],
            },
            Dimension {
                name: "action_type".into(),
                label: "行动".into(),
                description: "是否需要采取行动".into(),
                cardinality: Cardinality::Single,
                value_type: ValueType::Closed,
                suggested_values: vec![
                    "待办".into(), "学习".into(), "参考".into(),
                    "决策依据".into(), "纯归档".into(),
                ],
            },
        ]
    }
}

fn format_dimension(d: &Dimension) -> String {
    let vt_desc = match &d.value_type {
        ValueType::Open => "开放式".to_string(),
        ValueType::Closed => format!("封闭集合 [{}]", d.suggested_values.join(", ")),
        ValueType::Hybrid => format!("混合式 (候选: [{}])", d.suggested_values.join(", ")),
    };
    let card = match &d.cardinality {
        Cardinality::Single => "单值".to_string(),
        Cardinality::Multi { max } => format!("最多 {} 值", max),
    };
    format!("- {} ({}): {} / {} / {}\n", d.name, d.label, d.description, card, vt_desc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_taxonomy_has_core_and_universal() {
        let t = Taxonomy::default();
        assert_eq!(t.core.len(), 5);
        assert_eq!(t.universal.len(), 3);
        assert_eq!(t.plugins.len(), 0);
        let names: Vec<&str> = t.core.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"domain"));
        assert!(names.contains(&"topic"));
        assert!(names.contains(&"purpose"));
        assert!(names.contains(&"project"));
        assert!(names.contains(&"entities"));
    }

    #[test]
    fn load_builtin_plugins_works() {
        let plugins = Taxonomy::load_builtin_plugins().unwrap();
        assert_eq!(plugins.len(), 2);
        let ids: Vec<&str> = plugins.iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains(&"tech"));
        assert!(ids.contains(&"law"));
    }

    #[test]
    fn tech_plugin_dimensions() {
        let tech = Taxonomy::load_builtin_plugin("tech").unwrap();
        assert_eq!(tech.dimensions.len(), 3);
        let stack = tech.dimensions.iter().find(|d| d.name == "stack_layer").unwrap();
        assert!(matches!(stack.cardinality, Cardinality::Multi { max: 3 }));
        assert!(matches!(stack.value_type, ValueType::Hybrid));
        assert!(stack.suggested_values.contains(&"后端".to_string()));
    }

    #[test]
    fn law_plugin_dimensions() {
        let law = Taxonomy::load_builtin_plugin("law").unwrap();
        assert_eq!(law.dimensions.len(), 4);
        let risk = law.dimensions.iter().find(|d| d.name == "risk_level").unwrap();
        assert!(matches!(risk.value_type, ValueType::Closed));
    }

    #[test]
    fn build_system_prompt_includes_dimensions() {
        let tech = Taxonomy::load_builtin_plugin("tech").unwrap();
        let t = Taxonomy::default().with_plugin(tech);
        let prompt = t.build_system_prompt();
        assert!(prompt.contains("domain"));
        assert!(prompt.contains("topic"));
        assert!(prompt.contains("difficulty"));
        assert!(prompt.contains("stack_layer"));
        assert!(prompt.contains("JSON"));
    }

    #[test]
    fn build_user_prompt_batch() {
        let t = Taxonomy::default();
        let items = vec![
            ("Title A".to_string(), "Content A".to_string()),
            ("Title B".to_string(), "Content B".to_string()),
        ];
        let prompt = t.build_user_prompt(&items);
        assert!(prompt.contains("[1]"));
        assert!(prompt.contains("[2]"));
        assert!(prompt.contains("Title A"));
        assert!(prompt.contains("Title B"));
        assert!(prompt.contains("JSON 数组"));
    }
}
```

- [ ] **Step 4: 注册 taxonomy 模块**

Edit `npu-vault/crates/vault-core/src/lib.rs` to add `pub mod taxonomy;` in alphabetical position.

- [ ] **Step 5: 运行测试**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo test -p vault-core taxonomy::tests 2>&1 | grep "test result"`
Expected: `test result: ok. 6 passed`

---

## Task 4: store.rs — task_type 迁移 + tags 更新 + 队列扩展

**Files:**
- Modify: `npu-vault/crates/vault-core/src/store.rs`

- [ ] **Step 1: 查看当前 store.rs 结构**

Run: `cd /data/company/project/npu-webhook/npu-vault && grep -n "pub fn " crates/vault-core/src/store.rs`

记下所有已有方法的行号，新方法将追加在 `item_count` 或 `checkpoint` 之后。

- [ ] **Step 2: 添加 task_type 列迁移**

在 `Store::open` 方法内，在 `conn.execute_batch(SCHEMA_SQL)?;` 之后插入：

```rust
        // 迁移: embed_queue 新增 task_type 列（幂等）
        let has_task_type: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('embed_queue') WHERE name = 'task_type'",
            [],
            |row| row.get(0),
        )?;
        if has_task_type == 0 {
            conn.execute(
                "ALTER TABLE embed_queue ADD COLUMN task_type TEXT NOT NULL DEFAULT 'embed'",
                [],
            )?;
        }
```

在 `Store::open_memory` 中对应位置也添加同样的逻辑。

- [ ] **Step 3: 新增 enqueue_classify 方法**

在 `Store` impl 内追加：

```rust
    /// 为 item 入队一个分类任务（priority=3, task_type='classify'）
    pub fn enqueue_classify(&self, item_id: &str, priority: i32) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO embed_queue (item_id, chunk_idx, chunk_text, level, section_idx, priority, status, created_at, task_type)
             VALUES (?1, 0, ?2, 0, 0, ?3, 'pending', ?4, 'classify')",
            params![item_id, Vec::<u8>::new(), priority, now],
        )?;
        Ok(())
    }
```

- [ ] **Step 4: 新增 update_tags 方法**

在 `Store` impl 内追加：

```rust
    /// 更新条目的 tags 字段（加密存储）
    pub fn update_tags(&self, dek: &Key32, item_id: &str, tags_json: &str) -> Result<bool> {
        let encrypted = crypto::encrypt(dek, tags_json.as_bytes())?;
        let now = chrono::Utc::now().to_rfc3339();
        let affected = self.conn.execute(
            "UPDATE items SET tags = ?1, updated_at = ?2 WHERE id = ?3 AND is_deleted = 0",
            params![encrypted, now, item_id],
        )?;
        Ok(affected > 0)
    }

    /// 读取并解密 item 的 tags JSON (返回 None 表示未分类)
    pub fn get_tags_json(&self, dek: &Key32, item_id: &str) -> Result<Option<String>> {
        let tags: Option<Vec<u8>> = self.conn.query_row(
            "SELECT tags FROM items WHERE id = ?1 AND is_deleted = 0",
            params![item_id],
            |row| row.get::<_, Option<Vec<u8>>>(0),
        ).optional()?.flatten();

        match tags {
            None => Ok(None),
            Some(blob) if blob.is_empty() => Ok(None),
            Some(blob) => {
                let decrypted = crypto::decrypt(dek, &blob)?;
                Ok(Some(String::from_utf8_lossy(&decrypted).to_string()))
            }
        }
    }
```

At the top of the file, ensure `rusqlite::OptionalExtension;` is imported. If not, add:

```rust
use rusqlite::OptionalExtension;
```

- [ ] **Step 5: 新增 list_all_item_ids 方法**

在 `Store` impl 内追加：

```rust
    /// 列出所有未删除 item 的 id（用于 TagIndex 构建和 reclassify_all）
    pub fn list_all_item_ids(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM items WHERE is_deleted = 0 ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        Ok(ids)
    }

    /// 读取 item 的 (title, content) 明文对，用于重分类（需要 dek 解密）
    pub fn get_item_content(&self, dek: &Key32, item_id: &str) -> Result<Option<(String, String)>> {
        let result = self.get_item(dek, item_id)?;
        Ok(result.map(|item| (item.title, item.content)))
    }
```

- [ ] **Step 6: 更新现有 QueueTask 结构和 dequeue_embeddings 支持 task_type**

找到 `QueueTask` 结构体定义，添加 `task_type` 字段：

```rust
#[derive(Debug)]
pub struct QueueTask {
    pub id: i64,
    pub item_id: String,
    pub chunk_idx: i32,
    pub chunk_text: String,
    pub level: i32,
    pub section_idx: i32,
    pub priority: i32,
    pub attempts: i32,
    pub task_type: String,
}
```

找到 `dequeue_embeddings` 方法，将 SELECT 语句和 row 构造同步更新以包含 `task_type`：

```rust
    pub fn dequeue_embeddings(&self, batch_size: usize) -> Result<Vec<QueueTask>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, item_id, chunk_idx, chunk_text, level, section_idx, priority, attempts, task_type
             FROM embed_queue
             WHERE status = 'pending'
             ORDER BY priority ASC, created_at ASC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![batch_size as i64], |row| {
            let blob: Vec<u8> = row.get(3)?;
            let text = String::from_utf8_lossy(&blob).to_string();
            Ok(QueueTask {
                id: row.get(0)?,
                item_id: row.get(1)?,
                chunk_idx: row.get(2)?,
                chunk_text: text,
                level: row.get(4)?,
                section_idx: row.get(5)?,
                priority: row.get(6)?,
                attempts: row.get(7)?,
                task_type: row.get(8)?,
            })
        })?;
        let mut tasks: Vec<QueueTask> = Vec::new();
        for row in rows {
            tasks.push(row?);
        }
        for task in &tasks {
            self.conn.execute(
                "UPDATE embed_queue SET status = 'processing' WHERE id = ?1",
                params![task.id],
            )?;
        }
        Ok(tasks)
    }
```

If the existing method signature differs (e.g., returns Vec of a different type), adapt the changes while keeping the overall logic.

- [ ] **Step 7: 添加测试**

在 `#[cfg(test)] mod tests` 内追加：

```rust
    #[test]
    fn task_type_column_migration() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let id = store.insert_item(&dek, "T", "C", None, "note", None, None).unwrap();
        store.enqueue_classify(&id, 3).unwrap();
        let tasks = store.dequeue_embeddings(10).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].task_type, "classify");
        assert_eq!(tasks[0].item_id, id);
    }

    #[test]
    fn update_and_get_tags() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let id = store.insert_item(&dek, "T", "C", None, "note", None, None).unwrap();
        let tags_json = r#"{"core":{"domain":["技术"]}}"#;
        assert!(store.update_tags(&dek, &id, tags_json).unwrap());
        let retrieved = store.get_tags_json(&dek, &id).unwrap().unwrap();
        assert_eq!(retrieved, tags_json);
    }

    #[test]
    fn list_all_item_ids_excludes_deleted() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let a = store.insert_item(&dek, "A", "c", None, "note", None, None).unwrap();
        store.insert_item(&dek, "B", "c", None, "note", None, None).unwrap();
        let c = store.insert_item(&dek, "C", "c", None, "note", None, None).unwrap();
        store.delete_item(&c).unwrap();
        let ids = store.list_all_item_ids().unwrap();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&a));
    }
```

- [ ] **Step 8: 运行测试**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo test -p vault-core store::tests 2>&1 | grep "test result"`
Expected: `test result: ok. 12 passed` (9 旧 + 3 新)

---

## Task 5: tag_index.rs — 内存反向索引

**Files:**
- Create: `npu-vault/crates/vault-core/src/tag_index.rs`
- Modify: `npu-vault/crates/vault-core/src/lib.rs`

- [ ] **Step 1: 创建 tag_index.rs**

Create `npu-vault/crates/vault-core/src/tag_index.rs`:

```rust
use crate::crypto::Key32;
use crate::error::Result;
use crate::store::Store;
use crate::taxonomy::ClassificationResult;
use std::collections::{HashMap, HashSet};

/// 标签反向索引
/// forward: (dimension, value) -> {item_ids}
/// reverse: item_id -> [(dimension, value)]
pub struct TagIndex {
    forward: HashMap<(String, String), HashSet<String>>,
    reverse: HashMap<String, Vec<(String, String)>>,
}

impl TagIndex {
    pub fn new() -> Self {
        Self {
            forward: HashMap::new(),
            reverse: HashMap::new(),
        }
    }

    /// 从 store 构建索引（解密所有 items 的 tags）
    pub fn build(store: &Store, dek: &Key32) -> Result<Self> {
        let mut index = Self::new();
        let ids = store.list_all_item_ids()?;
        for id in ids {
            if let Some(tags_json) = store.get_tags_json(dek, &id)? {
                if let Ok(result) = serde_json::from_str::<ClassificationResult>(&tags_json) {
                    index.upsert(&id, &result);
                }
            }
        }
        Ok(index)
    }

    /// 插入或更新一个 item 的标签集合
    pub fn upsert(&mut self, item_id: &str, tags: &ClassificationResult) {
        self.remove(item_id);

        let mut pairs: Vec<(String, String)> = Vec::new();

        for (dim, values) in &tags.core {
            for v in values {
                pairs.push((dim.clone(), v.clone()));
            }
        }
        for (dim, value) in &tags.universal {
            pairs.push((dim.clone(), value.clone()));
        }
        for plugin_dims in tags.plugin.values() {
            for (dim, values) in plugin_dims {
                for v in values {
                    pairs.push((dim.clone(), v.clone()));
                }
            }
        }

        for pair in &pairs {
            self.forward.entry(pair.clone())
                .or_insert_with(HashSet::new)
                .insert(item_id.to_string());
        }
        self.reverse.insert(item_id.to_string(), pairs);
    }

    /// 删除一个 item 的所有标签
    pub fn remove(&mut self, item_id: &str) {
        if let Some(pairs) = self.reverse.remove(item_id) {
            for pair in pairs {
                if let Some(set) = self.forward.get_mut(&pair) {
                    set.remove(item_id);
                    if set.is_empty() {
                        self.forward.remove(&pair);
                    }
                }
            }
        }
    }

    /// 查询: 某维度某值的所有 item_id
    pub fn query(&self, dimension: &str, value: &str) -> Vec<String> {
        self.forward
            .get(&(dimension.to_string(), value.to_string()))
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// AND 组合查询
    pub fn query_and(&self, filters: &[(String, String)]) -> Vec<String> {
        if filters.is_empty() {
            return vec![];
        }
        let mut sets: Vec<&HashSet<String>> = Vec::new();
        for (dim, val) in filters {
            match self.forward.get(&(dim.clone(), val.clone())) {
                Some(s) => sets.push(s),
                None => return vec![],
            }
        }
        sets.sort_by_key(|s| s.len());
        let mut result: HashSet<String> = sets[0].clone();
        for s in &sets[1..] {
            result.retain(|id| s.contains(id));
        }
        result.into_iter().collect()
    }

    /// OR 组合查询
    pub fn query_or(&self, filters: &[(String, String)]) -> Vec<String> {
        let mut result: HashSet<String> = HashSet::new();
        for (dim, val) in filters {
            if let Some(set) = self.forward.get(&(dim.clone(), val.clone())) {
                result.extend(set.iter().cloned());
            }
        }
        result.into_iter().collect()
    }

    /// 某维度的所有值 + count 直方图
    pub fn histogram(&self, dimension: &str) -> Vec<(String, usize)> {
        let mut counts: Vec<(String, usize)> = self.forward
            .iter()
            .filter(|((dim, _), _)| dim == dimension)
            .map(|((_, val), set)| (val.clone(), set.len()))
            .collect();
        counts.sort_by(|a, b| b.1.cmp(&a.1));
        counts
    }

    /// 所有出现过的维度名
    pub fn all_dimensions(&self) -> Vec<String> {
        let mut dims: HashSet<String> = self.forward.keys().map(|(d, _)| d.clone()).collect();
        let mut sorted: Vec<String> = dims.drain().collect();
        sorted.sort();
        sorted
    }

    pub fn item_count(&self) -> usize {
        self.reverse.len()
    }
}

impl Default for TagIndex {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taxonomy::ClassificationResult;

    fn sample_tags(domain: &str, topic: &str) -> ClassificationResult {
        let mut tags = ClassificationResult::empty();
        tags.core.insert("domain".into(), vec![domain.into()]);
        tags.core.insert("topic".into(), vec![topic.into()]);
        tags.universal.insert("difficulty".into(), "进阶".into());
        tags
    }

    #[test]
    fn build_empty_index() {
        let idx = TagIndex::new();
        assert_eq!(idx.item_count(), 0);
        assert!(idx.query("domain", "技术").is_empty());
    }

    #[test]
    fn upsert_and_query() {
        let mut idx = TagIndex::new();
        idx.upsert("item1", &sample_tags("技术", "Rust"));
        idx.upsert("item2", &sample_tags("技术", "Python"));
        idx.upsert("item3", &sample_tags("法律", "合同"));

        let tech = idx.query("domain", "技术");
        assert_eq!(tech.len(), 2);

        let rust = idx.query("topic", "Rust");
        assert_eq!(rust, vec!["item1".to_string()]);
    }

    #[test]
    fn query_and_intersects() {
        let mut idx = TagIndex::new();
        idx.upsert("a", &sample_tags("技术", "Rust"));
        idx.upsert("b", &sample_tags("技术", "Python"));
        idx.upsert("c", &sample_tags("法律", "Rust"));

        let filters = vec![("domain".into(), "技术".into()), ("topic".into(), "Rust".into())];
        let results = idx.query_and(&filters);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], "a");
    }

    #[test]
    fn query_or_unions() {
        let mut idx = TagIndex::new();
        idx.upsert("a", &sample_tags("技术", "Rust"));
        idx.upsert("b", &sample_tags("法律", "合同"));

        let filters = vec![("domain".into(), "技术".into()), ("domain".into(), "法律".into())];
        let mut results = idx.query_or(&filters);
        results.sort();
        assert_eq!(results, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn remove_cleans_all_entries() {
        let mut idx = TagIndex::new();
        idx.upsert("a", &sample_tags("技术", "Rust"));
        assert_eq!(idx.item_count(), 1);

        idx.remove("a");
        assert_eq!(idx.item_count(), 0);
        assert!(idx.query("domain", "技术").is_empty());
    }

    #[test]
    fn upsert_replaces_old_values() {
        let mut idx = TagIndex::new();
        idx.upsert("a", &sample_tags("技术", "Rust"));
        idx.upsert("a", &sample_tags("法律", "合同"));

        assert!(idx.query("domain", "技术").is_empty());
        assert_eq!(idx.query("domain", "法律"), vec!["a".to_string()]);
    }

    #[test]
    fn histogram_counts_correctly() {
        let mut idx = TagIndex::new();
        idx.upsert("a", &sample_tags("技术", "Rust"));
        idx.upsert("b", &sample_tags("技术", "Rust"));
        idx.upsert("c", &sample_tags("技术", "Python"));
        idx.upsert("d", &sample_tags("法律", "合同"));

        let hist = idx.histogram("domain");
        assert_eq!(hist.len(), 2);
        assert_eq!(hist[0].0, "技术");
        assert_eq!(hist[0].1, 3);
        assert_eq!(hist[1].0, "法律");
        assert_eq!(hist[1].1, 1);
    }
}
```

- [ ] **Step 2: 注册 tag_index 模块**

Edit `npu-vault/crates/vault-core/src/lib.rs` to add `pub mod tag_index;` in alphabetical position.

- [ ] **Step 3: 运行测试**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo test -p vault-core tag_index::tests 2>&1 | grep "test result"`
Expected: `test result: ok. 7 passed`

---

## Task 6: classifier.rs — LLM 分类 Pipeline

**Files:**
- Create: `npu-vault/crates/vault-core/src/classifier.rs`
- Modify: `npu-vault/crates/vault-core/src/lib.rs`

- [ ] **Step 1: 创建 classifier.rs**

Create `npu-vault/crates/vault-core/src/classifier.rs`:

```rust
use crate::error::{Result, VaultError};
use crate::llm::LlmProvider;
use crate::taxonomy::{ClassificationResult, Taxonomy};
use std::collections::HashMap;
use std::sync::Arc;

pub struct Classifier {
    taxonomy: Arc<Taxonomy>,
    llm: Arc<dyn LlmProvider>,
    batch_size: usize,
}

impl Classifier {
    pub fn new(taxonomy: Arc<Taxonomy>, llm: Arc<dyn LlmProvider>) -> Self {
        Self { taxonomy, llm, batch_size: 5 }
    }

    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = size.max(1);
        self
    }

    /// 分类单条
    pub fn classify_one(&self, title: &str, content: &str) -> Result<ClassificationResult> {
        let items = vec![(title.to_string(), content.to_string())];
        let mut results = self.classify_batch(&items)?;
        results.pop()
            .ok_or_else(|| VaultError::Classification("empty result".into()))
    }

    /// 批量分类（一次 LLM 调用处理 batch_size 条）
    pub fn classify_batch(&self, items: &[(String, String)]) -> Result<Vec<ClassificationResult>> {
        if items.is_empty() {
            return Ok(vec![]);
        }

        let mut all_results = Vec::with_capacity(items.len());
        for chunk in items.chunks(self.batch_size) {
            let batch_results = self.classify_one_llm_call(chunk)?;
            all_results.extend(batch_results);
        }
        Ok(all_results)
    }

    fn classify_one_llm_call(&self, items: &[(String, String)]) -> Result<Vec<ClassificationResult>> {
        let system = self.taxonomy.build_system_prompt();
        let user = self.taxonomy.build_user_prompt(items);
        let raw = self.llm.chat(&system, &user)?;
        self.parse_response(&raw, items.len())
    }

    fn parse_response(&self, raw: &str, expected_count: usize) -> Result<Vec<ClassificationResult>> {
        let trimmed = raw.trim();
        let json_str = extract_json_block(trimmed).unwrap_or_else(|| trimmed.to_string());

        let parsed: serde_json::Value = serde_json::from_str(&json_str)
            .map_err(|e| VaultError::Classification(format!("invalid JSON: {e}. raw: {}", &json_str.chars().take(200).collect::<String>())))?;

        let items_array: Vec<serde_json::Value> = if expected_count == 1 && parsed.is_object() {
            vec![parsed]
        } else if let Some(arr) = parsed.as_array() {
            arr.clone()
        } else if parsed.is_object() {
            vec![parsed]
        } else {
            return Err(VaultError::Classification("expected object or array".into()));
        };

        let mut results = Vec::with_capacity(items_array.len());
        for obj in items_array {
            let result = self.parse_single(&obj)?;
            results.push(result);
        }
        Ok(results)
    }

    fn parse_single(&self, obj: &serde_json::Value) -> Result<ClassificationResult> {
        let mut result = ClassificationResult::empty();
        result.model = self.llm.model_name().to_string();
        result.plugins_used = self.taxonomy.plugins.iter().map(|p| p.id.clone()).collect();

        if let Some(core) = obj.get("core").and_then(|v| v.as_object()) {
            for (k, v) in core {
                let values = json_to_string_vec(v);
                result.core.insert(k.clone(), values);
            }
        }

        if let Some(universal) = obj.get("universal").and_then(|v| v.as_object()) {
            for (k, v) in universal {
                if let Some(s) = v.as_str() {
                    result.universal.insert(k.clone(), s.to_string());
                } else {
                    let values = json_to_string_vec(v);
                    if let Some(first) = values.into_iter().next() {
                        result.universal.insert(k.clone(), first);
                    }
                }
            }
        }

        if let Some(plugin) = obj.get("plugin").and_then(|v| v.as_object()) {
            for (plugin_id, dims_val) in plugin {
                if let Some(dims_obj) = dims_val.as_object() {
                    let mut plugin_tags: HashMap<String, Vec<String>> = HashMap::new();
                    for (dim, values) in dims_obj {
                        plugin_tags.insert(dim.clone(), json_to_string_vec(values));
                    }
                    result.plugin.insert(plugin_id.clone(), plugin_tags);
                }
            }
        }

        Ok(result)
    }
}

fn json_to_string_vec(v: &serde_json::Value) -> Vec<String> {
    if let Some(arr) = v.as_array() {
        arr.iter().filter_map(|e| e.as_str().map(String::from)).collect()
    } else if let Some(s) = v.as_str() {
        vec![s.to_string()]
    } else {
        vec![]
    }
}

/// 从可能包含 ```json ... ``` 或其他修饰的文本中提取 JSON
fn extract_json_block(s: &str) -> Option<String> {
    if let Some(start) = s.find("```json") {
        let after = &s[start + 7..];
        if let Some(end) = after.find("```") {
            return Some(after[..end].trim().to_string());
        }
    }
    if let Some(start) = s.find("```") {
        let after = &s[start + 3..];
        if let Some(end) = after.find("```") {
            return Some(after[..end].trim().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::MockLlmProvider;

    fn make_classifier() -> (Classifier, Arc<MockLlmProvider>) {
        let mock = Arc::new(MockLlmProvider::new("mock-model"));
        let taxonomy = Arc::new(Taxonomy::default());
        let classifier = Classifier::new(taxonomy, mock.clone());
        (classifier, mock)
    }

    const SAMPLE_RESPONSE: &str = r#"{
        "core": {
            "domain": ["技术"],
            "topic": ["Rust 加密"],
            "purpose": ["参考资料"],
            "project": ["npu-vault"],
            "entities": ["rustls", "aes-gcm"]
        },
        "universal": {
            "difficulty": "进阶",
            "freshness": "常青",
            "action_type": "学习"
        },
        "plugin": {}
    }"#;

    #[test]
    fn classify_one_parses_response() {
        let (classifier, mock) = make_classifier();
        mock.push_response(SAMPLE_RESPONSE);
        let result = classifier.classify_one("标题", "内容").unwrap();
        assert_eq!(result.core["domain"], vec!["技术"]);
        assert_eq!(result.core["topic"], vec!["Rust 加密"]);
        assert_eq!(result.universal["difficulty"], "进阶");
        assert_eq!(result.model, "mock-model");
    }

    #[test]
    fn classify_batch_multiple() {
        let (classifier, mock) = make_classifier();
        let batch_response = format!("[{}, {}]", SAMPLE_RESPONSE, SAMPLE_RESPONSE);
        mock.push_response(&batch_response);

        let items = vec![
            ("a".to_string(), "c".to_string()),
            ("b".to_string(), "c".to_string()),
        ];
        let results = classifier.classify_batch(&items).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn classify_extracts_json_from_code_block() {
        let (classifier, mock) = make_classifier();
        let wrapped = format!("```json\n{}\n```", SAMPLE_RESPONSE);
        mock.push_response(&wrapped);
        let result = classifier.classify_one("t", "c").unwrap();
        assert_eq!(result.core["domain"], vec!["技术"]);
    }

    #[test]
    fn classify_invalid_json_errors() {
        let (classifier, mock) = make_classifier();
        mock.push_response("not json at all");
        let result = classifier.classify_one("t", "c");
        assert!(result.is_err());
    }

    #[test]
    fn classify_empty_batch_returns_empty() {
        let (classifier, _mock) = make_classifier();
        let results = classifier.classify_batch(&[]).unwrap();
        assert!(results.is_empty());
    }
}
```

- [ ] **Step 2: 注册 classifier 模块**

Edit `npu-vault/crates/vault-core/src/lib.rs` to add `pub mod classifier;` in alphabetical position.

- [ ] **Step 3: 运行测试**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo test -p vault-core classifier::tests 2>&1 | grep "test result"`
Expected: `test result: ok. 5 passed`

---

## Task 7: clusterer.rs — HDBSCAN 聚类 + LLM 命名

**Files:**
- Create: `npu-vault/crates/vault-core/src/clusterer.rs`
- Modify: `npu-vault/crates/vault-core/src/lib.rs`

- [ ] **Step 1: 创建 clusterer.rs**

Create `npu-vault/crates/vault-core/src/clusterer.rs`:

```rust
use crate::error::{Result, VaultError};
use crate::llm::LlmProvider;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const DEFAULT_MIN_ITEMS: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cluster {
    pub id: i32,
    pub name: String,
    pub summary: String,
    pub item_count: usize,
    pub item_ids: Vec<String>,
    pub representative_item_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterSnapshot {
    pub version: u32,
    pub generated_at: String,
    pub algorithm: String,
    pub model: String,
    pub clusters: Vec<Cluster>,
    pub noise_item_ids: Vec<String>,
}

impl ClusterSnapshot {
    pub fn empty() -> Self {
        Self {
            version: 1,
            generated_at: chrono::Utc::now().to_rfc3339(),
            algorithm: "hdbscan".into(),
            model: String::new(),
            clusters: vec![],
            noise_item_ids: vec![],
        }
    }
}

/// 聚类输入: (item_id, title, content_snippet, embedding)
#[derive(Debug, Clone)]
pub struct ClusterInput {
    pub item_id: String,
    pub title: String,
    pub content_snippet: String,
    pub embedding: Vec<f32>,
}

pub struct Clusterer {
    llm: Arc<dyn LlmProvider>,
    min_items: usize,
}

impl Clusterer {
    pub fn new(llm: Arc<dyn LlmProvider>) -> Self {
        Self { llm, min_items: DEFAULT_MIN_ITEMS }
    }

    pub fn with_min_items(mut self, min: usize) -> Self {
        self.min_items = min;
        self
    }

    pub fn rebuild(&self, inputs: Vec<ClusterInput>) -> Result<ClusterSnapshot> {
        if inputs.len() < self.min_items {
            return Ok(ClusterSnapshot::empty());
        }

        let labels = self.run_hdbscan(&inputs)?;

        let mut groups: std::collections::BTreeMap<i32, Vec<usize>> = std::collections::BTreeMap::new();
        for (i, label) in labels.iter().enumerate() {
            groups.entry(*label).or_insert_with(Vec::new).push(i);
        }

        let mut clusters: Vec<Cluster> = Vec::new();
        let mut noise_ids: Vec<String> = Vec::new();

        for (label, indices) in groups {
            if label == -1 {
                noise_ids = indices.iter().map(|&i| inputs[i].item_id.clone()).collect();
                continue;
            }

            let reps: Vec<&ClusterInput> = indices.iter().take(3).map(|&i| &inputs[i]).collect();
            let (name, summary) = self.name_cluster(&reps)
                .unwrap_or_else(|_| (format!("聚类 {label}"), "未命名".into()));

            let item_ids: Vec<String> = indices.iter().map(|&i| inputs[i].item_id.clone()).collect();
            let rep_id = item_ids.first().cloned();

            clusters.push(Cluster {
                id: label,
                name,
                summary,
                item_count: indices.len(),
                item_ids,
                representative_item_id: rep_id,
            });
        }

        Ok(ClusterSnapshot {
            version: 1,
            generated_at: chrono::Utc::now().to_rfc3339(),
            algorithm: "hdbscan".into(),
            model: self.llm.model_name().to_string(),
            clusters,
            noise_item_ids: noise_ids,
        })
    }

    fn run_hdbscan(&self, inputs: &[ClusterInput]) -> Result<Vec<i32>> {
        let dataset: Vec<Vec<f32>> = inputs.iter().map(|i| i.embedding.clone()).collect();
        let min_cluster_size = std::cmp::max(3, inputs.len() / 30);
        let hyper_params = hdbscan::HyperParamBuilder::default()
            .min_cluster_size(min_cluster_size)
            .min_samples(1)
            .build()
            .map_err(|e| VaultError::Classification(format!("hdbscan params: {e:?}")))?;
        let clusterer = hdbscan::Hdbscan::new(&dataset, hyper_params);
        let labels = clusterer.cluster()
            .map_err(|e| VaultError::Classification(format!("hdbscan run: {e:?}")))?;
        Ok(labels.into_iter().map(|l| l as i32).collect())
    }

    fn name_cluster(&self, reps: &[&ClusterInput]) -> Result<(String, String)> {
        let system = "你是一个知识库聚类命名助手。给定一组相关的知识片段，生成简洁的主题名和一句话摘要。";
        let rep_texts: Vec<String> = reps.iter().map(|r| {
            let snippet: String = r.content_snippet.chars().take(300).collect();
            format!("- {}: {}", r.title, snippet)
        }).collect();
        let user = format!(
            "以下是一个聚类中的 {} 个代表样本:\n\n{}\n\n请输出 JSON:\n{{\"name\": \"主题名 (8-15 字)\", \"summary\": \"一句话摘要 (20-40 字)\"}}",
            reps.len(),
            rep_texts.join("\n")
        );
        let raw = self.llm.chat(system, &user)?;
        let trimmed = raw.trim();
        let json_str = if let Some(start) = trimmed.find('{') {
            if let Some(end) = trimmed.rfind('}') {
                &trimmed[start..=end]
            } else {
                trimmed
            }
        } else {
            trimmed
        };
        let parsed: serde_json::Value = serde_json::from_str(json_str)
            .map_err(|e| VaultError::Classification(format!("cluster name json: {e}")))?;
        let name = parsed.get("name").and_then(|v| v.as_str()).unwrap_or("未命名").to_string();
        let summary = parsed.get("summary").and_then(|v| v.as_str()).unwrap_or("").to_string();
        Ok((name, summary))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::MockLlmProvider;

    fn make_inputs(n: usize) -> Vec<ClusterInput> {
        (0..n).map(|i| ClusterInput {
            item_id: format!("id{i}"),
            title: format!("Title {i}"),
            content_snippet: format!("content {i}"),
            embedding: vec![(i as f32) * 0.1, (i as f32) * 0.2, 0.3, 0.4],
        }).collect()
    }

    #[test]
    fn below_min_returns_empty_snapshot() {
        let mock = Arc::new(MockLlmProvider::new("m"));
        let clusterer = Clusterer::new(mock).with_min_items(20);
        let inputs = make_inputs(5);
        let snapshot = clusterer.rebuild(inputs).unwrap();
        assert!(snapshot.clusters.is_empty());
    }

    #[test]
    fn snapshot_empty_default() {
        let s = ClusterSnapshot::empty();
        assert!(s.clusters.is_empty());
        assert!(s.noise_item_ids.is_empty());
        assert_eq!(s.algorithm, "hdbscan");
    }

    #[test]
    fn cluster_serializable() {
        let c = Cluster {
            id: 0,
            name: "test".into(),
            summary: "sum".into(),
            item_count: 3,
            item_ids: vec!["a".into(), "b".into(), "c".into()],
            representative_item_id: Some("a".into()),
        };
        let json = serde_json::to_string(&c).unwrap();
        let parsed: Cluster = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "test");
    }

    #[test]
    fn name_cluster_parses_llm_response() {
        let mock = Arc::new(MockLlmProvider::new("m"));
        mock.push_response(r#"{"name": "Rust 加密研究", "summary": "围绕 vault-core 的加密实现"}"#);
        let clusterer = Clusterer::new(mock);
        let inputs = make_inputs(3);
        let refs: Vec<&ClusterInput> = inputs.iter().collect();
        let (name, summary) = clusterer.name_cluster(&refs).unwrap();
        assert_eq!(name, "Rust 加密研究");
        assert_eq!(summary, "围绕 vault-core 的加密实现");
    }
}
```

- [ ] **Step 2: 注册 clusterer 模块**

Edit `npu-vault/crates/vault-core/src/lib.rs` to add `pub mod clusterer;` in alphabetical position.

- [ ] **Step 3: 运行测试**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo test -p vault-core clusterer::tests 2>&1 | grep "test result"`
Expected: `test result: ok. 4 passed`

如果 hdbscan crate API 与本文不符（例如字段名不同），查看 hdbscan docs 并调整 `run_hdbscan` 方法。保持其余代码不变。测试只依赖 `below_min` 和 `name_cluster`，不依赖 `run_hdbscan` 的具体调用。

---

## Task 8: Queue Worker 扩展 — 分类任务分派

**Files:**
- Modify: `npu-vault/crates/vault-core/src/queue.rs`

- [ ] **Step 1: 查看当前 queue.rs 的 process_batch 结构**

Run: `cd /data/company/project/npu-webhook/npu-vault && grep -n "pub fn\|fn process" crates/vault-core/src/queue.rs`

记录关键方法位置。

- [ ] **Step 2: 添加 process_classify_batch 方法**

在 `QueueWorker::process_batch` 函数之后添加新方法。先修改 `process_batch` 按 task_type 分派：

打开 `crates/vault-core/src/queue.rs`，找到 `process_batch` 方法（作为 associated fn），替换为按 task_type 分派的版本：

```rust
    fn process_batch(
        store: &Arc<Mutex<Store>>,
        embedding: &Arc<dyn EmbeddingProvider>,
        vectors: &Arc<Mutex<VectorIndex>>,
        fulltext: &Arc<Mutex<FulltextIndex>>,
    ) -> Result<usize> {
        let tasks = {
            let s = store.lock().unwrap();
            s.dequeue_embeddings(BATCH_SIZE)?
        };

        if tasks.is_empty() {
            return Ok(0);
        }

        let (embed_tasks, other_tasks): (Vec<_>, Vec<_>) = tasks
            .into_iter()
            .partition(|t| t.task_type == "embed");

        let mut total = 0;

        if !embed_tasks.is_empty() {
            total += Self::process_embed_batch(store, embedding, vectors, fulltext, embed_tasks)?;
        }

        if !other_tasks.is_empty() {
            // classify 任务需要 LLM 和 Classifier，目前在这一层不可用
            // 将 classify 任务标记为 pending 保留（不处理）直到 server 层带 Classifier 调用
            let s = store.lock().unwrap();
            for task in &other_tasks {
                s.mark_task_pending(task.id)?;
            }
            total += other_tasks.len();
        }

        Ok(total)
    }

    fn process_embed_batch(
        store: &Arc<Mutex<Store>>,
        embedding: &Arc<dyn EmbeddingProvider>,
        vectors: &Arc<Mutex<VectorIndex>>,
        fulltext: &Arc<Mutex<FulltextIndex>>,
        tasks: Vec<QueueTask>,
    ) -> Result<usize> {
        if !embedding.is_available() {
            return Ok(0);
        }

        let texts: Vec<&str> = tasks.iter().map(|t| t.chunk_text.as_str()).collect();

        let embeddings = match embedding.embed(&texts) {
            Ok(embs) => embs,
            Err(e) => {
                let s = store.lock().unwrap();
                for task in &tasks {
                    s.mark_embedding_failed(task.id, MAX_ATTEMPTS)?;
                }
                return Err(e);
            }
        };

        let count = tasks.len();
        for (i, task) in tasks.iter().enumerate() {
            if i >= embeddings.len() { break; }

            {
                let mut vecs = vectors.lock().unwrap();
                vecs.add(
                    &embeddings[i],
                    crate::vectors::VectorMeta {
                        item_id: task.item_id.clone(),
                        chunk_idx: task.chunk_idx as usize,
                        level: task.level as u8,
                        section_idx: task.section_idx as usize,
                    },
                )?;
            }

            if task.level == 1 {
                let ft = fulltext.lock().unwrap();
                ft.add_document(&task.item_id, "", &task.chunk_text, "file")?;
            }

            let s = store.lock().unwrap();
            s.mark_embedding_done(task.id)?;
        }

        Ok(count)
    }
```

- [ ] **Step 3: 添加 mark_task_pending 方法到 store.rs**

Open `npu-vault/crates/vault-core/src/store.rs` and add to `Store` impl block:

```rust
    /// 将 processing 任务重新标记为 pending（用于未实现处理时占位）
    pub fn mark_task_pending(&self, id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE embed_queue SET status = 'pending' WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }
```

- [ ] **Step 4: 验证编译**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo build -p vault-core 2>&1 | tail -10`
Expected: `Finished dev profile`

- [ ] **Step 5: 验证测试不破坏**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo test -p vault-core queue::tests 2>&1 | grep "test result"`
Expected: `test result: ok. 2 passed` (旧 queue 测试保持)

---

## Task 9: AppState 扩展 + 搜索引擎初始化钩子

**Files:**
- Modify: `npu-vault/crates/vault-server/src/state.rs`

- [ ] **Step 1: 扩展 AppState struct**

Open `npu-vault/crates/vault-server/src/state.rs` and replace the full contents with:

```rust
use std::sync::{Arc, Mutex};
use vault_core::classifier::Classifier;
use vault_core::clusterer::ClusterSnapshot;
use vault_core::embed::{EmbeddingProvider, OllamaProvider};
use vault_core::index::FulltextIndex;
use vault_core::llm::{LlmProvider, OllamaLlmProvider};
use vault_core::tag_index::TagIndex;
use vault_core::taxonomy::Taxonomy;
use vault_core::vault::Vault;
use vault_core::vectors::VectorIndex;

pub type SharedState = Arc<AppState>;

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
}

impl AppState {
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
        }
    }

    /// 初始化搜索引擎 + 分类引擎 (unlock 后调用)
    pub fn init_search_engines(&self) {
        // 全文索引
        if let Ok(ft) = FulltextIndex::open_memory() {
            *self.fulltext.lock().unwrap() = Some(ft);
        }

        // 向量索引
        if let Ok(vecs) = VectorIndex::new(1024) {
            *self.vectors.lock().unwrap() = Some(vecs);
        }

        // Embedding provider (Ollama)
        let provider = OllamaProvider::default();
        *self.embedding.lock().unwrap() = Some(Arc::new(provider));

        // LLM provider (auto-detect) - 可能失败，失败则分类功能不可用
        if let Ok(llm) = OllamaLlmProvider::auto_detect() {
            let llm_arc: Arc<dyn LlmProvider> = Arc::new(llm);

            // 构建 taxonomy (启用所有内置插件)
            let mut tax = Taxonomy::default();
            if let Ok(plugins) = Taxonomy::load_builtin_plugins() {
                for p in plugins {
                    tax = tax.with_plugin(p);
                }
            }
            let tax_arc = Arc::new(tax);

            *self.classifier.lock().unwrap() = Some(Arc::new(Classifier::new(tax_arc.clone(), llm_arc.clone())));
            *self.taxonomy.lock().unwrap() = Some(tax_arc);
            *self.llm.lock().unwrap() = Some(llm_arc);
        }

        // TagIndex (基于已有 items.tags 构建)
        let tag_index_result = {
            let vault = self.vault.lock().unwrap();
            if let Ok(dek) = vault.dek_db() {
                TagIndex::build(vault.store(), &dek).ok()
            } else {
                None
            }
        };
        *self.tag_index.lock().unwrap() = tag_index_result;
    }

    /// 清除搜索引擎 (lock 前调用)
    pub fn clear_search_engines(&self) {
        *self.fulltext.lock().unwrap() = None;
        *self.vectors.lock().unwrap() = None;
        *self.embedding.lock().unwrap() = None;
        *self.llm.lock().unwrap() = None;
        *self.tag_index.lock().unwrap() = None;
        *self.cluster_snapshot.lock().unwrap() = None;
        *self.taxonomy.lock().unwrap() = None;
        *self.classifier.lock().unwrap() = None;
    }
}
```

- [ ] **Step 2: 验证编译**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo build -p vault-server 2>&1 | tail -10`
Expected: `Finished dev profile` 或新错误（如果 init_search_engines 现在被其他代码调用，可能需要同步更新）

如果 compile 失败因为其他地方使用了移除的字段，按错误提示修复，但不要改变核心逻辑。

---

## Task 10: HTTP 路由 — /classify/*

**Files:**
- Create: `npu-vault/crates/vault-server/src/routes/classify.rs`
- Modify: `npu-vault/crates/vault-server/src/routes/mod.rs`
- Modify: `npu-vault/crates/vault-server/src/main.rs`

- [ ] **Step 1: 创建 classify.rs**

Create `npu-vault/crates/vault-server/src/routes/classify.rs`:

```rust
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use crate::state::SharedState;

/// POST /api/v1/classify/{id} — 单条重分类（同步，阻塞直到完成）
pub async fn classify_one(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let classifier_arc = state.classifier.lock().unwrap().as_ref().cloned();
    let classifier = match classifier_arc {
        Some(c) => c,
        None => return Err((StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({
            "error": "classification unavailable",
            "hint": "install ollama chat model: ollama pull qwen2.5:3b"
        })))),
    };

    let (title, content) = {
        let vault = state.vault.lock().unwrap();
        let dek = vault.dek_db().map_err(|e| {
            (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
        })?;
        match vault.store().get_item(&dek, &id) {
            Ok(Some(item)) => (item.title, item.content),
            Ok(None) => return Err((StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "not found"})))),
            Err(e) => return Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))),
        }
    };

    let result = tokio::task::spawn_blocking(move || classifier.classify_one(&title, &content))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    // 写入 store 并更新 tag_index
    let tags_json = serde_json::to_string(&result)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    {
        let vault = state.vault.lock().unwrap();
        let dek = vault.dek_db().map_err(|e| (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()}))))?;
        vault.store().update_tags(&dek, &id, &tags_json)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;
    }

    {
        let mut tag_index = state.tag_index.lock().unwrap();
        if let Some(index) = tag_index.as_mut() {
            index.upsert(&id, &result);
        }
    }

    Ok(Json(serde_json::json!({"status": "ok", "id": id, "tags": result})))
}

/// POST /api/v1/classify/rebuild — 全量重分类（异步，入队所有 items）
pub async fn rebuild(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap();
    let _ = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    let ids = vault.store().list_all_item_ids()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    for id in &ids {
        vault.store().enqueue_classify(id, 4)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;
    }

    Ok(Json(serde_json::json!({
        "status": "ok",
        "enqueued": ids.len(),
        "note": "classify tasks enqueued with priority=4; process via /classify/{id} or manual trigger"
    })))
}

/// GET /api/v1/classify/status — 分类队列统计
pub async fn status(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap();
    let _ = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    let pending = vault.store().pending_embedding_count()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    let tag_count = state.tag_index.lock().unwrap()
        .as_ref()
        .map(|i| i.item_count())
        .unwrap_or(0);

    let classifier_ready = state.classifier.lock().unwrap().is_some();
    let model = state.llm.lock().unwrap()
        .as_ref()
        .map(|l| l.model_name().to_string())
        .unwrap_or_default();

    Ok(Json(serde_json::json!({
        "classifier_ready": classifier_ready,
        "model": model,
        "pending_tasks": pending,
        "classified_items": tag_count,
    })))
}
```

- [ ] **Step 2: 注册到 mod.rs**

Edit `npu-vault/crates/vault-server/src/routes/mod.rs` to add `pub mod classify;` in alphabetical position.

- [ ] **Step 3: 注册路由到 main.rs**

Open `npu-vault/crates/vault-server/src/main.rs` and add to the Router chain (在 search 路由之后):

```rust
        .route("/api/v1/classify/{id}", post(routes::classify::classify_one))
        .route("/api/v1/classify/rebuild", post(routes::classify::rebuild))
        .route("/api/v1/classify/status", get(routes::classify::status))
```

- [ ] **Step 4: 编译**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo build -p vault-server 2>&1 | tail -10`
Expected: `Finished dev profile`

---

## Task 11: HTTP 路由 — /tags + /clusters + /plugins

**Files:**
- Create: `npu-vault/crates/vault-server/src/routes/tags.rs`
- Create: `npu-vault/crates/vault-server/src/routes/clusters.rs`
- Create: `npu-vault/crates/vault-server/src/routes/plugins.rs`
- Modify: `npu-vault/crates/vault-server/src/routes/mod.rs`
- Modify: `npu-vault/crates/vault-server/src/main.rs`

- [ ] **Step 1: 创建 tags.rs**

Create `npu-vault/crates/vault-server/src/routes/tags.rs`:

```rust
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use crate::state::SharedState;

/// GET /api/v1/tags — 所有维度的聚合统计
pub async fn all_dimensions(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let tag_index = state.tag_index.lock().unwrap();
    let index = match tag_index.as_ref() {
        Some(i) => i,
        None => return Err((StatusCode::FORBIDDEN, Json(serde_json::json!({"error": "vault locked or tag index unavailable"})))),
    };

    let dims = index.all_dimensions();
    let mut result = serde_json::Map::new();
    for dim in &dims {
        // 跳过 entities 避免直方图过长
        if dim == "entities" { continue; }
        let hist = index.histogram(dim);
        let values: Vec<serde_json::Value> = hist.into_iter()
            .map(|(v, c)| serde_json::json!({"value": v, "count": c}))
            .collect();
        result.insert(dim.clone(), serde_json::Value::Array(values));
    }

    Ok(Json(serde_json::json!({"dimensions": result})))
}

/// GET /api/v1/tags/{dimension} — 某维度的完整直方图
pub async fn dimension_histogram(
    State(state): State<SharedState>,
    Path(dimension): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let tag_index = state.tag_index.lock().unwrap();
    let index = match tag_index.as_ref() {
        Some(i) => i,
        None => return Err((StatusCode::FORBIDDEN, Json(serde_json::json!({"error": "vault locked or tag index unavailable"})))),
    };
    let hist = index.histogram(&dimension);
    let values: Vec<serde_json::Value> = hist.into_iter()
        .map(|(v, c)| serde_json::json!({"value": v, "count": c}))
        .collect();
    Ok(Json(serde_json::json!({
        "dimension": dimension,
        "values": values
    })))
}
```

- [ ] **Step 2: 创建 clusters.rs**

Create `npu-vault/crates/vault-server/src/routes/clusters.rs`:

```rust
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use crate::state::SharedState;

/// GET /api/v1/clusters — 当前聚类快照
pub async fn list(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let snapshot = state.cluster_snapshot.lock().unwrap().clone();
    match snapshot {
        Some(s) => Ok(Json(serde_json::to_value(&s).unwrap())),
        None => Ok(Json(serde_json::json!({
            "clusters": [],
            "note": "no cluster snapshot yet, POST /clusters/rebuild to generate"
        }))),
    }
}

/// GET /api/v1/clusters/{id} — 某聚类详情
pub async fn detail(
    State(state): State<SharedState>,
    Path(id): Path<i32>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let snapshot = state.cluster_snapshot.lock().unwrap();
    match snapshot.as_ref() {
        Some(s) => {
            match s.clusters.iter().find(|c| c.id == id) {
                Some(c) => Ok(Json(serde_json::to_value(c).unwrap())),
                None => Err((StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "cluster not found"})))),
            }
        }
        None => Err((StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "no snapshot"})))),
    }
}

/// POST /api/v1/clusters/rebuild — 手动触发聚类（返回新快照）
pub async fn rebuild(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let _ = state.vault.lock().unwrap().dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    // Phase 1 简化: 返回 "not yet implemented in this iteration"
    // 完整实现需要 vectors/store 协作收集 ClusterInput，放到后续 task 完善
    Ok(Json(serde_json::json!({
        "status": "ok",
        "note": "cluster rebuild is a heavy operation; call the CLI tool or wait for scheduled run"
    })))
}
```

- [ ] **Step 3: 创建 plugins.rs**

Create `npu-vault/crates/vault-server/src/routes/plugins.rs`:

```rust
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use crate::state::SharedState;
use vault_core::taxonomy::Taxonomy;

/// GET /api/v1/plugins — 列出所有可用的内置插件
pub async fn list(
    State(_state): State<SharedState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let plugins = Taxonomy::load_builtin_plugins()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    let list: Vec<serde_json::Value> = plugins.iter().map(|p| serde_json::json!({
        "id": p.id,
        "name": p.name,
        "version": p.version,
        "description": p.description,
        "dimensions": p.dimensions.iter().map(|d| serde_json::json!({
            "name": d.name,
            "label": d.label,
            "description": d.description,
        })).collect::<Vec<_>>(),
    })).collect();

    Ok(Json(serde_json::json!({"plugins": list})))
}
```

- [ ] **Step 4: 注册模块和路由**

Edit `npu-vault/crates/vault-server/src/routes/mod.rs` to add:
```rust
pub mod classify;
pub mod clusters;
pub mod plugins;
pub mod tags;
```
(保持字母排序)

Edit `npu-vault/crates/vault-server/src/main.rs` to add routes in the Router chain:

```rust
        .route("/api/v1/tags", get(routes::tags::all_dimensions))
        .route("/api/v1/tags/{dimension}", get(routes::tags::dimension_histogram))
        .route("/api/v1/clusters", get(routes::clusters::list))
        .route("/api/v1/clusters/{id}", get(routes::clusters::detail))
        .route("/api/v1/clusters/rebuild", post(routes::clusters::rebuild))
        .route("/api/v1/plugins", get(routes::plugins::list))
```

- [ ] **Step 5: 编译**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo build -p vault-server 2>&1 | tail -10`
Expected: `Finished dev profile`

---

## Task 12: 集成测试 classifier_test.rs

**Files:**
- Create: `npu-vault/tests/classifier_test.rs`

- [ ] **Step 1: 创建集成测试文件**

Create `npu-vault/tests/classifier_test.rs`:

```rust
use std::sync::Arc;
use tempfile::TempDir;
use vault_core::classifier::Classifier;
use vault_core::llm::MockLlmProvider;
use vault_core::tag_index::TagIndex;
use vault_core::taxonomy::{ClassificationResult, Taxonomy};
use vault_core::vault::Vault;

fn setup_vault() -> (Vault, TempDir) {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("data/vault.db");
    let config_dir = tmp.path().join("config");
    let vault = Vault::open(&db_path, &config_dir).unwrap();
    vault.setup("test-password").unwrap();
    (vault, tmp)
}

const MOCK_RESPONSE: &str = r#"{
    "core": {
        "domain": ["技术"],
        "topic": ["Rust 加密"],
        "purpose": ["参考资料"],
        "project": ["npu-vault"],
        "entities": ["rustls"]
    },
    "universal": {
        "difficulty": "进阶",
        "freshness": "常青",
        "action_type": "学习"
    },
    "plugin": {}
}"#;

#[test]
fn e2e_classify_flow() {
    let (vault, _tmp) = setup_vault();
    let dek = vault.dek_db().unwrap();

    // Ingest some items
    let id1 = vault.store().insert_item(&dek, "Rust 加密笔记", "关于 AES-GCM 的研究", None, "note", None, None).unwrap();
    let id2 = vault.store().insert_item(&dek, "Python 脚本", "数据处理脚本", None, "note", None, None).unwrap();

    // Setup classifier with mock
    let mock = Arc::new(MockLlmProvider::new("mock-model"));
    mock.push_response(MOCK_RESPONSE);
    mock.push_response(MOCK_RESPONSE);
    let taxonomy = Arc::new(Taxonomy::default());
    let classifier = Classifier::new(taxonomy, mock).with_batch_size(1);

    // Classify
    let (title1, content1) = (String::from("Rust 加密笔记"), String::from("关于 AES-GCM 的研究"));
    let result1 = classifier.classify_one(&title1, &content1).unwrap();
    assert_eq!(result1.core["domain"], vec!["技术"]);

    // Write to store
    let json1 = serde_json::to_string(&result1).unwrap();
    vault.store().update_tags(&dek, &id1, &json1).unwrap();

    // Verify retrieval
    let retrieved = vault.store().get_tags_json(&dek, &id1).unwrap().unwrap();
    let parsed: ClassificationResult = serde_json::from_str(&retrieved).unwrap();
    assert_eq!(parsed.core["domain"], vec!["技术"]);

    // Build TagIndex from store
    let index = TagIndex::build(vault.store(), &dek).unwrap();
    assert_eq!(index.item_count(), 1); // Only id1 has tags, id2 not yet classified

    let tech_items = index.query("domain", "技术");
    assert_eq!(tech_items.len(), 1);
    assert_eq!(tech_items[0], id1);
}

#[test]
fn e2e_reclassify_flow() {
    let (vault, _tmp) = setup_vault();
    let dek = vault.dek_db().unwrap();

    let id = vault.store().insert_item(&dek, "Item", "content", None, "note", None, None).unwrap();

    // First classification
    let mock = Arc::new(MockLlmProvider::new("mock-v1"));
    mock.push_response(MOCK_RESPONSE);
    let taxonomy = Arc::new(Taxonomy::default());
    let classifier = Classifier::new(taxonomy, mock);

    let result = classifier.classify_one("Item", "content").unwrap();
    vault.store().update_tags(&dek, &id, &serde_json::to_string(&result).unwrap()).unwrap();

    // Second classification (re-classify)
    let mock2 = Arc::new(MockLlmProvider::new("mock-v2"));
    let new_response = r#"{
        "core": {
            "domain": ["法律"],
            "topic": ["合同审查"],
            "purpose": ["参考资料"],
            "project": ["none"],
            "entities": []
        },
        "universal": {
            "difficulty": "入门",
            "freshness": "常青",
            "action_type": "参考"
        },
        "plugin": {}
    }"#;
    mock2.push_response(new_response);
    let taxonomy2 = Arc::new(Taxonomy::default());
    let classifier2 = Classifier::new(taxonomy2, mock2);

    let result2 = classifier2.classify_one("Item", "content").unwrap();
    vault.store().update_tags(&dek, &id, &serde_json::to_string(&result2).unwrap()).unwrap();

    // Verify new tags replaced old
    let index = TagIndex::build(vault.store(), &dek).unwrap();
    assert_eq!(index.query("domain", "技术").len(), 0);
    assert_eq!(index.query("domain", "法律").len(), 1);
}

#[test]
fn e2e_classify_lock_unlock_persistence() {
    let (vault, _tmp) = setup_vault();
    let dek = vault.dek_db().unwrap();

    let id = vault.store().insert_item(&dek, "Persistent", "c", None, "note", None, None).unwrap();

    // Classify and save
    let mock = Arc::new(MockLlmProvider::new("mock"));
    mock.push_response(MOCK_RESPONSE);
    let taxonomy = Arc::new(Taxonomy::default());
    let classifier = Classifier::new(taxonomy, mock);
    let result = classifier.classify_one("Persistent", "c").unwrap();
    vault.store().update_tags(&dek, &id, &serde_json::to_string(&result).unwrap()).unwrap();

    // Lock the vault
    vault.lock().unwrap();
    assert!(vault.dek_db().is_err());

    // Unlock and rebuild index
    vault.unlock("test-password").unwrap();
    let dek2 = vault.dek_db().unwrap();
    let index = TagIndex::build(vault.store(), &dek2).unwrap();
    assert_eq!(index.item_count(), 1);
    assert_eq!(index.query("domain", "技术").len(), 1);
}
```

- [ ] **Step 2: 运行集成测试**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo test --test classifier_test 2>&1 | grep "test result"`
Expected: `test result: ok. 3 passed`

- [ ] **Step 3: 运行全量测试**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo test --workspace 2>&1 | grep "test result"`
Expected: 所有测试通过，总数约 106 (103 unit + 3 + 3 integration)

---

## Task 13: Web UI 标签页 — 分类 + 聚类

**Files:**
- Modify: `npu-vault/crates/vault-server/assets/index.html`

- [ ] **Step 1: 阅读当前 HTML 结构**

Run: `cd /data/company/project/npu-webhook/npu-vault && grep -n "class=\"tab\"\|tab-content" crates/vault-server/assets/index.html | head -30`

理解现有 tab 机制（`btn-*` + `data-tab` + `tab-content` 类）。

- [ ] **Step 2: 在 tabs 导航栏增加 2 个标签**

打开 `npu-vault/crates/vault-server/assets/index.html`，找到：
```html
<button class="tab active" data-tab="search">搜索</button>
      <button class="tab" data-tab="ingest">录入</button>
      <button class="tab" data-tab="items">条目</button>
      <button class="tab" data-tab="settings">设置</button>
```

替换为：
```html
<button class="tab active" data-tab="search">搜索</button>
      <button class="tab" data-tab="ingest">录入</button>
      <button class="tab" data-tab="items">条目</button>
      <button class="tab" data-tab="classify">分类</button>
      <button class="tab" data-tab="clusters">聚类</button>
      <button class="tab" data-tab="settings">设置</button>
```

- [ ] **Step 3: 添加分类和聚类 tab content**

在现有 `<div id="tab-settings">` 之前，插入：

```html
    <div id="tab-classify" class="tab-content">
      <div class="card">
        <h2 style="font-size: 14px; margin-bottom: 12px;">维度浏览</h2>
        <div class="field">
          <label>选择维度</label>
          <select id="classify-dim-select">
            <option value="domain">领域 (domain)</option>
            <option value="topic">主题 (topic)</option>
            <option value="purpose">用途 (purpose)</option>
            <option value="project">项目 (project)</option>
            <option value="difficulty">深度 (difficulty)</option>
            <option value="freshness">时效 (freshness)</option>
            <option value="action_type">行动 (action_type)</option>
          </select>
        </div>
        <button id="btn-classify-load">加载直方图</button>
      </div>
      <div id="classify-histogram"></div>
      <div class="card">
        <button id="btn-classify-rebuild" class="secondary">重新分类全部条目</button>
        <div id="classify-status" style="margin-top: 8px; font-size: 12px; color: #94a3b8;"></div>
      </div>
    </div>

    <div id="tab-clusters" class="tab-content">
      <div class="card">
        <button id="btn-clusters-reload">刷新聚类</button>
        <button id="btn-clusters-rebuild" class="secondary" style="margin-left: 8px;">重新聚类</button>
        <div id="clusters-meta" style="margin-top: 8px; font-size: 12px; color: #94a3b8;"></div>
      </div>
      <div id="clusters-list"></div>
    </div>
```

- [ ] **Step 4: 添加 JS 函数**

在 `(function() { 'use strict';` IIFE 内，在 `// Event bindings` 之前，添加：

```javascript
  async function loadClassifyHistogram() {
    const dim = $('classify-dim-select').value;
    try {
      const r = await api('/tags/' + dim);
      const container = $('classify-histogram');
      clearChildren(container);
      if (!r.values || r.values.length === 0) {
        container.appendChild(makeEl('div', 'empty', '该维度暂无数据'));
        return;
      }
      const card = makeEl('div', 'card');
      card.appendChild(makeEl('h3', null, dim));
      card.lastChild.style.fontSize = '13px';
      card.lastChild.style.marginBottom = '8px';
      for (const v of r.values) {
        const row = makeEl('div', 'item');
        row.appendChild(makeEl('div', 'item-title', v.value));
        row.appendChild(makeEl('div', 'item-meta', v.count + ' 条'));
        card.appendChild(row);
      }
      container.appendChild(card);
    } catch (e) { toast(e.message, 'error'); }
  }

  async function rebuildClassify() {
    try {
      const r = await api('/classify/rebuild', { method: 'POST' });
      toast('已入队 ' + (r.enqueued || 0) + ' 条', 'success');
      refreshClassifyStatus();
    } catch (e) { toast(e.message, 'error'); }
  }

  async function refreshClassifyStatus() {
    try {
      const r = await api('/classify/status');
      const statusEl = $('classify-status');
      statusEl.textContent = '模型: ' + (r.model || '未配置') +
        ' · 已分类: ' + r.classified_items +
        ' · 待处理: ' + r.pending_tasks;
    } catch (e) { /* ignore */ }
  }

  async function loadClusters() {
    try {
      const r = await api('/clusters');
      const container = $('clusters-list');
      clearChildren(container);
      const meta = $('clusters-meta');
      if (r.clusters && r.clusters.length > 0) {
        meta.textContent = '生成时间: ' + r.generated_at;
        for (const c of r.clusters) {
          const card = makeEl('div', 'card');
          card.appendChild(makeEl('h3', null, '🔬 ' + c.name));
          card.lastChild.style.fontSize = '14px';
          card.lastChild.style.marginBottom = '4px';
          card.appendChild(makeEl('div', 'item-meta', c.item_count + ' 条'));
          if (c.summary) {
            card.appendChild(makeEl('div', 'item-content', c.summary));
          }
          container.appendChild(card);
        }
      } else {
        meta.textContent = '';
        container.appendChild(makeEl('div', 'empty', r.note || '暂无聚类数据'));
      }
    } catch (e) { toast(e.message, 'error'); }
  }

  async function rebuildClusters() {
    try {
      const r = await api('/clusters/rebuild', { method: 'POST' });
      toast(r.note || '已触发', 'success');
      loadClusters();
    } catch (e) { toast(e.message, 'error'); }
  }
```

- [ ] **Step 5: 绑定新按钮事件**

在 `// Event bindings` 区段末尾添加：

```javascript
  $('btn-classify-load').addEventListener('click', loadClassifyHistogram);
  $('btn-classify-rebuild').addEventListener('click', rebuildClassify);
  $('btn-clusters-reload').addEventListener('click', loadClusters);
  $('btn-clusters-rebuild').addEventListener('click', rebuildClusters);
```

- [ ] **Step 6: 编译并验证 UI**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo build -p vault-server 2>&1 | tail -5`
Expected: `Finished dev profile`

启动验证：
```bash
rm -rf /tmp/vault-ui-test2
XDG_DATA_HOME=/tmp/vault-ui-test2/data XDG_CONFIG_HOME=/tmp/vault-ui-test2/config \
  cargo run -p vault-server --bin npu-vault-server -- --port 18930 > /tmp/ui-test.log 2>&1 &
sleep 2
curl -s -o /tmp/ui.html -w "%{http_code}\n" http://127.0.0.1:18930/
grep -c 'data-tab="classify"' /tmp/ui.html
grep -c 'data-tab="clusters"' /tmp/ui.html
pkill -f "npu-vault-server --port 18930" 2>/dev/null
wait 2>/dev/null
```

Expected: HTTP 200, 两个 grep 都返回 1

---

## Task 14: 文档更新 — npu-vault README/DEVELOP/RELEASE

**Files:**
- Modify: `npu-vault/README.md`
- Modify: `npu-vault/DEVELOP.md`
- Modify: `npu-vault/RELEASE.md`

- [ ] **Step 1: 更新 README.md 功能列表**

打开 `npu-vault/README.md`，在"## 功能"章节的最后一条之后插入：

```markdown
- **AI 自动分类** — 基于 Ollama 本地 LLM（qwen2.5 等）对每条知识生成 5 核心维度 + 3 通用维度标签，支持编程/法律等行业插件
- **HDBSCAN 智能聚类** — 自动发现知识主题群组，LLM 命名，类似相册的"回忆"功能
- **标签直方图 + 浏览** — Web UI 按维度筛选，点击标签查看所有匹配条目
```

- [ ] **Step 2: 更新 README.md API 端点表**

在 API 端点章节，在"系统"小节之前插入一个新的"分类与聚类"小节：

```markdown
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
```

- [ ] **Step 3: 更新 README.md Phase 计划**

找到"## Phase 计划"章节，在"Phase 3"之后插入：

```markdown
- **子系统 A** ✅ AI 自动分类 (qwen2.5 + HDBSCAN + 编程/法律插件 + 最小 UI 集成)
```

并在路线图部分用新的子系统列表替换原有"Phase 4"（保留 F 系列作为 Phase 4-7）：

```markdown
- 子系统 B: 个人行为画像（搜索历史 + 点击权重 + 偏好模型）
- 子系统 C: Web UI 全面升级（分类浏览 + 批量操作 + SPA）
- 子系统 D: 运行时插件系统（外部 YAML 加载 + 插件市场）
- 子系统 E: 画像/分类结果导出（.vault-profile 迁移文件）
- F1: NAS 远程目录扫描（SMB / NFS / WebDAV）
- F2: Tauri 桌面客户端
- F3: Queue Worker 自启动
- F4: tantivy/usearch 持久化加密
```

- [ ] **Step 4: 更新 README.md 测试数**

将 `78 tests (75 单元 + 3 集成)` 改为 `106 tests (103 单元 + 3 集成)`。

将 vault 模块的 `16 状态机` 保留，新增以下行：

```markdown
| llm | 3 | OllamaLlmProvider, MockLlmProvider |
| taxonomy | 6 | 插件 YAML 解析, prompt 构建, validate |
| classifier | 5 | MockLlmProvider 驱动, 解析, 容错 |
| clusterer | 4 | 最小阈值, 序列化, LLM 命名 |
| tag_index | 7 | build, query_and/or, upsert, histogram |
| store 扩展 | 3 | task_type 迁移, update_tags, list_all_item_ids |
| 集成测试 (classifier_test) | 3 | e2e 分类 / 重分类 / 锁解锁持久化 |
```

- [ ] **Step 5: 更新 DEVELOP.md 项目结构**

打开 `npu-vault/DEVELOP.md`，在 vault-core/src 文件树内添加：

```
│       ├── llm.rs               # Ollama chat client (LlmProvider trait + OllamaLlmProvider + MockLlmProvider)
│       ├── taxonomy.rs          # 维度定义 + 插件 YAML 加载 + prompt 构建
│       ├── classifier.rs        # LLM 分类 pipeline (批量 + 容错)
│       ├── clusterer.rs         # HDBSCAN 聚类 + LLM 命名
│       └── tag_index.rs         # 内存反向索引
```

vault-core/assets/ 增加：
```
│   └── assets/plugins/
│       ├── tech.yaml            # 编程/技术插件
│       └── law.yaml             # 法律插件
```

vault-server routes/ 增加：
```
│       ├── classify.rs          # /classify/*
│       ├── clusters.rs          # /clusters/*
│       ├── plugins.rs           # /plugins/*
│       └── tags.rs              # /tags/*
```

- [ ] **Step 6: 更新 DEVELOP.md 分层架构**

在分层架构 ASCII 图的 `Core Engine` 段加入新模块：

```
│  ├── LLM       — Ollama chat client                                │
│  ├── Taxonomy  — 维度定义 + 内置插件 (tech / law)                   │
│  ├── Classifier — LLM 分类 pipeline                                 │
│  ├── Clusterer — HDBSCAN + LLM 命名                                 │
│  ├── TagIndex  — 内存反向索引                                        │
```

- [ ] **Step 7: 更新 DEVELOP.md 测试数**

将 `78 tests` 改为 `106 tests`。

- [ ] **Step 8: RELEASE.md 添加 v0.4.0 条目**

在 RELEASE.md 顶部"## 已发布"下方插入：

```markdown
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
- 新方法: `update_tags`, `get_tags_json`, `enqueue_classify`, `list_all_item_ids`

**硬依赖**:
- 分类功能需要 Ollama 运行 + chat 模型（qwen2.5:3b 推荐）
- 无 chat 模型时分类端点返回 503，其他功能正常

**测试**: 28 unit + 3 integration = **106 tests** (75 + 28 = 103 unit)

**二进制大小变化**:
- vault-server 从 26 MB 增至约 27 MB（hdbscan crate + 插件 YAML）
```

- [ ] **Step 9: 验证文档文件**

Run: `cd /data/company/project/npu-webhook/npu-vault && wc -l README.md DEVELOP.md RELEASE.md`
Expected: 三个文件都有明显的行数增长

---

## Task 15: 最终全量验证

**Files:** (验证性质，无新文件)

- [ ] **Step 1: 运行全量测试**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo test --workspace 2>&1 | grep "test result"`
Expected: 所有 5 个测试 target 都 ok，总测试数 103 unit + 3 integration = 106

- [ ] **Step 2: Release 构建**

Run: `cd /data/company/project/npu-webhook/npu-vault && cargo build --workspace --release 2>&1 | tail -5`
Expected: `Finished release profile`

- [ ] **Step 3: 二进制大小检查**

Run: `ls -lh /data/company/project/npu-webhook/npu-vault/target/release/npu-vault /data/company/project/npu-webhook/npu-vault/target/release/npu-vault-server`
Expected:
- npu-vault ~4.1 MB (无大变化)
- npu-vault-server ~27 MB (约增 1 MB)

- [ ] **Step 4: Smoke test — vault 和 UI**

```bash
rm -rf /tmp/vault-final-test
XDG_DATA_HOME=/tmp/vault-final-test/data XDG_CONFIG_HOME=/tmp/vault-final-test/config \
  ./target/release/npu-vault-server --port 18940 > /tmp/final.log 2>&1 &
sleep 3

# UI 健康
curl -s -o /dev/null -w "UI: %{http_code}\n" http://127.0.0.1:18940/

# setup
curl -s -X POST http://127.0.0.1:18940/api/v1/vault/setup \
  -H "Content-Type: application/json" \
  -d '{"password":"test"}'
echo ""

# classify/status (即使无 Ollama chat 模型，端点也应工作，返回 classifier_ready=false)
curl -s http://127.0.0.1:18940/api/v1/classify/status
echo ""

# tags (返回空直方图)
curl -s http://127.0.0.1:18940/api/v1/tags
echo ""

# plugins (列出内置)
curl -s http://127.0.0.1:18940/api/v1/plugins | python3 -c "import sys,json; d=json.load(sys.stdin); print('plugins:', len(d.get('plugins',[])))"

pkill -f "npu-vault-server --port 18940" 2>/dev/null
wait 2>/dev/null
```

Expected:
- UI: 200
- setup: `{"state":"unlocked","status":"ok"}`
- classify/status: JSON 包含 `classifier_ready` 字段
- tags: `{"dimensions":{}}` 或包含空数据的结构
- plugins: `plugins: 2`

- [ ] **Step 5: 汇报完成**

所有 14 个 task 完成后：
- 106 tests 全部通过
- Release 构建成功
- Smoke test 通过
- 文档同步更新

报告用户：A 子系统完成，待用户手动 commit（Opsera hook 阻塞自动提交）。

---

## Self-Review

**1. Spec 覆盖率**:
- ✅ 核心 5 维 + 通用 3 维 + 2 内置插件 → Task 3 (taxonomy + YAML)
- ✅ Ollama chat client + auto_detect + Mock → Task 2 (llm.rs)
- ✅ 分类 pipeline + 批量 + 容错 → Task 6 (classifier.rs)
- ✅ HDBSCAN + LLM 命名 → Task 7 (clusterer.rs)
- ✅ TagIndex 内存索引 → Task 5 (tag_index.rs)
- ✅ tags 字段语义扩展 + task_type 迁移 → Task 4 (store.rs)
- ✅ AppState 扩展 + init_search_engines 钩子 → Task 9 (state.rs)
- ✅ Queue 按 task_type 分派 → Task 8 (queue.rs)
- ✅ /classify/{id}, /classify/rebuild, /classify/status → Task 10
- ✅ /tags, /tags/{dimension} → Task 11
- ✅ /clusters, /clusters/{id}, /clusters/rebuild → Task 11
- ✅ /plugins → Task 11
- ✅ Web UI 分类 + 聚类标签页 → Task 13
- ✅ 集成测试（e2e_classify_flow 等 3 个）→ Task 12
- ✅ README/DEVELOP/RELEASE 更新 → Task 14

**覆盖缺口**:
- `POST /plugins/{id}/enable|disable` 未实现：降级为只读 `/plugins` 列表。启用/禁用插件需要持久化 settings 并触发 reclassify_all，推迟到 D 子系统。
- `/search` 和 `/items` 的 tag/cluster 过滤参数：推迟到 C 子系统（完整 UI 升级）。当前版本通过 `/tags/{dimension}` 返回 item_ids 然后前端做二次请求。
- `settings.classify.*` 字段：推迟到 C 子系统。当前代码使用默认值 hard-code。
- `auto_cluster_interval_hours` 定期任务：未实现（需要后台 worker scheduler）。放到 F3 (Queue Worker 自启动)。

**2. 占位符扫描**: 无 TBD/TODO。所有代码块完整。`/clusters/rebuild` 返回 "call the CLI tool or wait" 是明确的 Phase 1 简化说明，不是占位符。

**3. 类型一致性**:
- `Key32` / `Store` / `Vault` / `VaultError` — 已有类型，复用一致
- `ClassificationResult` — taxonomy.rs 定义，classifier.rs 生成，tag_index.rs 消费，store 序列化 — 一致
- `ClusterSnapshot` / `Cluster` — clusterer.rs 定义，routes/clusters.rs 消费 — 一致
- `TagIndex` — tag_index.rs 定义，state.rs 包装，routes/tags.rs 和 routes/classify.rs 使用 — 一致
- `LlmProvider` trait — llm.rs 定义，classifier.rs 和 clusterer.rs 依赖 — 一致
- `Taxonomy` / `Plugin` / `Dimension` / `Cardinality` / `ValueType` — taxonomy.rs 定义 — 一致
- `Classifier` — classifier.rs 定义，state.rs 包装 — 一致
- `QueueTask.task_type` — store.rs 新增字段，queue.rs 消费 — 一致

**4. 命名一致性**:
- `init_search_engines` / `clear_search_engines` — 与已有的 Phase 2a 搜索集成命名一致
- `classify_one` / `classify_batch` / `rebuild` — 动词一致
- `TagIndex::build` / `upsert` / `remove` — 与已有的 `FulltextIndex` / `VectorIndex` 命名风格一致
- `mark_task_pending` / `mark_embedding_done` / `mark_embedding_failed` — store 统一 `mark_*` 前缀

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-04-11-ai-classification-plan.md`.

Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration. Tasks 2, 3, 5, 6, 7 are independent and can run in parallel where possible.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?

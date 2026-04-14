# 搜索质量提升 Plan A：推理层 + 三阶段管道 + LLM 抽象

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 用 ort ONNX 本地推理替换 Ollama embedding 依赖，新增 cross-encoder reranker，将搜索管道扩展为 initial_k→intermediate_k→top_k 三阶段，LLM 统一为 OpenAI-compat HTTP 接口。

**Architecture:** 在 `vault-core` 新增 `infer/` 模块，实现 `OrtEmbeddingProvider` 和 `OrtRerankProvider`；重构 `search.rs` 提取公共 `search_with_context` 函数供 search 和 chat 共用；`llm.rs` 新增 `OpenAiLlmProvider` 支持任意 OpenAI-compat 后端；`state.rs` 新增 `reranker` 字段并更新初始化逻辑。

**Tech Stack:** `ort 2.x`（ONNX Runtime Rust 绑定）、`tokenizers 0.21`（HuggingFace tokenizer）、`hf-hub 0.3`（模型下载）、`ndarray 0.16`、Qwen3-Embedding-0.6B INT8 ONNX、bge-reranker-v2-m3 INT8 ONNX

---

## 文件结构

```
新建：
  npu-vault/crates/vault-core/src/infer/mod.rs
  npu-vault/crates/vault-core/src/infer/embedding.rs
  npu-vault/crates/vault-core/src/infer/reranker.rs
  npu-vault/crates/vault-core/src/infer/provider.rs
  npu-vault/crates/vault-core/src/infer/model_store.rs

修改：
  npu-vault/crates/vault-core/Cargo.toml          — 新增 ort/tokenizers/hf-hub/ndarray
  npu-vault/crates/vault-core/src/lib.rs           — 新增 pub mod infer
  npu-vault/crates/vault-core/src/platform.rs      — 新增 models_dir() + NpuKind + detect_npu()
  npu-vault/crates/vault-core/src/search.rs        — 新增 SearchParams/SearchContext/search_with_context
  npu-vault/crates/vault-core/src/chat.rs          — 重构 search_for_context 用 search_with_context
  npu-vault/crates/vault-core/src/llm.rs           — 新增 OpenAiLlmProvider
  npu-vault/crates/vault-server/src/state.rs       — 新增 reranker 字段，更新 init_search_engines
  npu-vault/crates/vault-server/src/routes/search.rs — 接收 initial_k/intermediate_k 参数
  npu-vault/crates/vault-server/src/routes/chat.rs   — 删除 500 字符截断 bug
```

---

### Task 1：Cargo 依赖 + infer/ 模块骨架（traits + mocks）

**Files:**
- Modify: `npu-vault/crates/vault-core/Cargo.toml`
- Create: `npu-vault/crates/vault-core/src/infer/mod.rs`
- Modify: `npu-vault/crates/vault-core/src/lib.rs`

- [ ] **Step 1: 在 vault-core/Cargo.toml 新增依赖**

打开 `npu-vault/crates/vault-core/Cargo.toml`，在 `[dependencies]` 末尾添加：

```toml
ort = { version = "2.0", features = ["cuda"] }
tokenizers = { version = "0.21", default-features = false }
hf-hub = "0.3"
ndarray = "0.16"
```

- [ ] **Step 2: 验证依赖可解析**

```bash
cd npu-vault && cargo check -p vault-core 2>&1 | head -20
```

期望：仅警告，无 error。若有版本冲突按提示调整。

- [ ] **Step 3: 写失败测试（RerankProvider trait 不存在）**

新建 `npu-vault/crates/vault-core/src/infer/mod.rs`：

```rust
// 暂时写一个会失败的测试
#[cfg(test)]
mod tests {
    #[test]
    fn rerank_provider_exists() {
        // 此测试会在 trait 定义后通过
        let _: Option<Box<dyn super::RerankProvider>> = None;
    }
}
```

- [ ] **Step 4: 运行确认测试失败**

```bash
cd npu-vault && cargo test -p vault-core infer 2>&1 | tail -10
```

期望：编译失败，`RerankProvider` 未定义。

- [ ] **Step 5: 实现 infer/mod.rs（trait 定义 + mock）**

将 `npu-vault/crates/vault-core/src/infer/mod.rs` 替换为完整实现：

```rust
// npu-vault/crates/vault-core/src/infer/mod.rs

pub mod embedding;
pub mod model_store;
pub mod provider;
pub mod reranker;

use crate::error::Result;

/// Cross-encoder reranker：对每个 (query, document) 对输出相关性分数
pub trait RerankProvider: Send + Sync {
    /// 返回分数列表 [0.0, 1.0]，顺序与 `documents` 一致
    fn score(&self, query: &str, documents: &[&str]) -> Result<Vec<f32>>;
}

/// 测试用 mock，返回预设分数
pub struct MockRerankProvider {
    scores: std::sync::Mutex<Vec<f32>>,
}

impl MockRerankProvider {
    pub fn new(scores: Vec<f32>) -> Self {
        Self { scores: std::sync::Mutex::new(scores) }
    }
}

impl RerankProvider for MockRerankProvider {
    fn score(&self, _query: &str, documents: &[&str]) -> Result<Vec<f32>> {
        let preset = self.scores.lock().unwrap();
        // 循环复用 preset 分数填充文档数量
        let result = (0..documents.len())
            .map(|i| *preset.get(i % preset.len().max(1)).unwrap_or(&0.5))
            .collect();
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rerank_provider_exists() {
        let _: Option<Box<dyn RerankProvider>> = None;
    }

    #[test]
    fn mock_reranker_returns_preset_scores() {
        let mock = MockRerankProvider::new(vec![0.9, 0.3, 0.7]);
        let docs = ["doc1", "doc2", "doc3"];
        let scores = mock.score("query", &docs).unwrap();
        assert_eq!(scores, vec![0.9, 0.3, 0.7]);
    }

    #[test]
    fn mock_reranker_cycles_when_fewer_presets_than_docs() {
        let mock = MockRerankProvider::new(vec![0.8]);
        let docs = ["a", "b", "c"];
        let scores = mock.score("q", &docs).unwrap();
        assert_eq!(scores.len(), 3);
        assert!((scores[0] - 0.8).abs() < 1e-5);
    }
}
```

- [ ] **Step 6: 在 lib.rs 注册模块**

打开 `npu-vault/crates/vault-core/src/lib.rs`，在 `pub mod embed;` 后添加：

```rust
pub mod infer;
```

- [ ] **Step 7: 运行测试确认通过**

```bash
cd npu-vault && cargo test -p vault-core infer 2>&1 | tail -15
```

期望：`3 tests passed`。

- [ ] **Step 8: 为四个子模块创建空文件（避免编译报错）**

```bash
touch npu-vault/crates/vault-core/src/infer/embedding.rs
touch npu-vault/crates/vault-core/src/infer/reranker.rs
touch npu-vault/crates/vault-core/src/infer/provider.rs
touch npu-vault/crates/vault-core/src/infer/model_store.rs
```

- [ ] **Step 9: 确认整体编译**

```bash
cd npu-vault && cargo check -p vault-core 2>&1 | grep "^error" | head -10
```

期望：无 error。

- [ ] **Step 10: 提交**

```bash
cd npu-vault && git add crates/vault-core/Cargo.toml crates/vault-core/src/lib.rs crates/vault-core/src/infer/
git commit -m "feat(infer): add RerankProvider trait, MockRerankProvider, infer/ module skeleton"
```

---

### Task 2：platform.rs 新增 models_dir() + NpuKind + detect_npu()

**Files:**
- Modify: `npu-vault/crates/vault-core/src/platform.rs`

- [ ] **Step 1: 写失败测试**

打开 `npu-vault/crates/vault-core/src/platform.rs`，在 `#[cfg(test)]` 内添加：

```rust
#[test]
fn models_dir_inside_data_dir() {
    let md = models_dir();
    assert!(md.starts_with(data_dir()));
    assert!(md.ends_with("models"));
}

#[test]
fn detect_npu_returns_valid_variant() {
    let npu = detect_npu();
    // 任意变体均合法，只要能编译返回
    let _ = format!("{:?}", npu);
}
```

- [ ] **Step 2: 确认测试失败**

```bash
cd npu-vault && cargo test -p vault-core platform 2>&1 | tail -5
```

期望：编译失败，`models_dir` 和 `detect_npu` 未定义。

- [ ] **Step 3: 实现**

将 `npu-vault/crates/vault-core/src/platform.rs` 替换为：

```rust
// npu-vault/crates/vault-core/src/platform.rs

use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    let base = dirs::data_local_dir().expect("cannot determine data directory");
    base.join("npu-vault")
}

pub fn config_dir() -> PathBuf {
    let base = dirs::config_dir().expect("cannot determine config directory");
    base.join("npu-vault")
}

pub fn db_path() -> PathBuf {
    data_dir().join("vault.db")
}

pub fn device_secret_path() -> PathBuf {
    config_dir().join("device.key")
}

/// 模型缓存目录：~/.local/share/npu-vault/models/
pub fn models_dir() -> PathBuf {
    data_dir().join("models")
}

/// 可用的硬件加速后端
#[derive(Debug, Clone, PartialEq)]
pub enum NpuKind {
    IntelNpu,
    IntelIgpu,
    AmdNpu,
    Cuda,
    None,
}

/// 探测本机最优 Execution Provider
///
/// 优先级：NPU_VAULT_EP 环境变量 > CUDA > CPU fallback
/// OpenVINO（Intel NPU/iGPU）和 DirectML（AMD）可在 ort feature flags
/// 启用后通过环境变量 NPU_VAULT_EP=openvino/directml 开启。
pub fn detect_npu() -> NpuKind {
    match std::env::var("NPU_VAULT_EP").as_deref() {
        Ok("openvino") => NpuKind::IntelNpu,
        Ok("directml") => NpuKind::AmdNpu,
        Ok("cuda") => NpuKind::Cuda,
        Ok("cpu") | Ok("none") => NpuKind::None,
        _ => {
            // 自动探测：Linux 下检查 CUDA 设备节点
            if std::path::Path::new("/dev/nvidia0").exists() {
                NpuKind::Cuda
            } else {
                NpuKind::None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_end_with_npu_vault() {
        let dd = data_dir();
        let cd = config_dir();
        assert!(dd.ends_with("npu-vault"), "data_dir should end with npu-vault: {:?}", dd);
        assert!(cd.ends_with("npu-vault"), "config_dir should end with npu-vault: {:?}", cd);
    }

    #[test]
    fn db_path_inside_data_dir() {
        let db = db_path();
        assert!(db.starts_with(data_dir()));
        assert_eq!(db.file_name().unwrap(), "vault.db");
    }

    #[test]
    fn device_secret_inside_config_dir() {
        let ds = device_secret_path();
        assert!(ds.starts_with(config_dir()));
        assert_eq!(ds.file_name().unwrap(), "device.key");
    }

    #[test]
    fn models_dir_inside_data_dir() {
        let md = models_dir();
        assert!(md.starts_with(data_dir()));
        assert!(md.to_str().unwrap().ends_with("models"));
    }

    #[test]
    fn detect_npu_returns_valid_variant() {
        let npu = detect_npu();
        let _ = format!("{:?}", npu);
    }

    #[test]
    fn detect_npu_respects_env_var() {
        std::env::set_var("NPU_VAULT_EP", "cuda");
        assert_eq!(detect_npu(), NpuKind::Cuda);
        std::env::set_var("NPU_VAULT_EP", "cpu");
        assert_eq!(detect_npu(), NpuKind::None);
        std::env::remove_var("NPU_VAULT_EP");
    }
}
```

- [ ] **Step 4: 运行测试**

```bash
cd npu-vault && cargo test -p vault-core platform 2>&1 | tail -10
```

期望：`5 tests passed`。

- [ ] **Step 5: 提交**

```bash
cd npu-vault && git add crates/vault-core/src/platform.rs
git commit -m "feat(platform): add models_dir(), NpuKind, detect_npu()"
```

---

### Task 3：infer/model_store.rs — 模型下载与缓存

**Files:**
- Modify: `npu-vault/crates/vault-core/src/infer/model_store.rs`

- [ ] **Step 1: 写失败测试**

在 `npu-vault/crates/vault-core/src/infer/model_store.rs` 中写：

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn model_cache_dir_for_repo() {
        let dir = super::model_cache_dir("Qwen/Qwen3-Embedding-0.6B");
        assert!(dir.to_str().unwrap().contains("Qwen_Qwen3-Embedding-0.6B"));
    }
}
```

- [ ] **Step 2: 确认失败**

```bash
cd npu-vault && cargo test -p vault-core model_store 2>&1 | tail -5
```

期望：编译失败，`model_cache_dir` 未定义。

- [ ] **Step 3: 实现 model_store.rs**

```rust
// npu-vault/crates/vault-core/src/infer/model_store.rs

use crate::error::{Result, VaultError};
use std::path::PathBuf;

/// 给定 HuggingFace repo_id，返回本地缓存目录路径
/// repo_id 中的 '/' 替换为 '_'，避免目录层级问题
pub fn model_cache_dir(repo_id: &str) -> PathBuf {
    crate::platform::models_dir().join(repo_id.replace('/', "_"))
}

/// 确保 model_filename 和 tokenizer_filename 两个文件已缓存在本地
///
/// 若文件不存在则从 HuggingFace Hub 下载（支持 HF_ENDPOINT 环境变量镜像）。
/// 返回 (model_path, tokenizer_path)。
pub fn ensure_models(
    repo_id: &str,
    model_filename: &str,
    tokenizer_filename: &str,
) -> Result<(PathBuf, PathBuf)> {
    let cache_dir = model_cache_dir(repo_id);
    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| VaultError::Crypto(format!("create model dir: {e}")))?;

    // 取文件名末段（model_filename 可能含路径如 "onnx/model_quantized.onnx"）
    let model_basename = model_filename.rsplit('/').next().unwrap_or(model_filename);
    let tokenizer_basename = tokenizer_filename.rsplit('/').next().unwrap_or(tokenizer_filename);

    let model_path = cache_dir.join(model_basename);
    let tokenizer_path = cache_dir.join(tokenizer_basename);

    if model_path.exists() && tokenizer_path.exists() {
        return Ok((model_path, tokenizer_path));
    }

    let api = hf_hub::api::sync::Api::new()
        .map_err(|e| VaultError::Crypto(format!("hf-hub init: {e}")))?;
    let repo = api.model(repo_id.to_string());

    if !model_path.exists() {
        let src = repo.get(model_filename)
            .map_err(|e| VaultError::Crypto(format!("download {model_filename}: {e}")))?;
        std::fs::copy(&src, &model_path)
            .map_err(|e| VaultError::Crypto(format!("copy model file: {e}")))?;
    }

    if !tokenizer_path.exists() {
        let src = repo.get(tokenizer_filename)
            .map_err(|e| VaultError::Crypto(format!("download {tokenizer_filename}: {e}")))?;
        std::fs::copy(&src, &tokenizer_path)
            .map_err(|e| VaultError::Crypto(format!("copy tokenizer file: {e}")))?;
    }

    Ok((model_path, tokenizer_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_cache_dir_for_repo() {
        let dir = model_cache_dir("Qwen/Qwen3-Embedding-0.6B");
        assert!(dir.to_str().unwrap().contains("Qwen_Qwen3-Embedding-0.6B"));
    }

    #[test]
    fn model_cache_dir_replaces_slash() {
        let dir = model_cache_dir("BAAI/bge-reranker-v2-m3");
        let s = dir.to_str().unwrap();
        assert!(!s.contains("BAAI/bge"), "slash should be replaced");
        assert!(s.contains("BAAI_bge-reranker-v2-m3"));
    }
}
```

- [ ] **Step 4: 运行测试**

```bash
cd npu-vault && cargo test -p vault-core model_store 2>&1 | tail -8
```

期望：`2 tests passed`（不联网，仅测 dir 逻辑）。

- [ ] **Step 5: 提交**

```bash
cd npu-vault && git add crates/vault-core/src/infer/model_store.rs
git commit -m "feat(infer): add model_store with ensure_models() and hf-hub download"
```

---

### Task 4：infer/provider.rs — ort Session 构建 + EP 选择

**Files:**
- Modify: `npu-vault/crates/vault-core/src/infer/provider.rs`

- [ ] **Step 1: 实现 provider.rs**

```rust
// npu-vault/crates/vault-core/src/infer/provider.rs

use crate::error::{Result, VaultError};
use crate::platform::NpuKind;
use ort::{
    execution_providers::{CPUExecutionProvider, CUDAExecutionProvider},
    Session,
};
use std::path::Path;

/// 根据平台检测结果，构建带最优 Execution Provider 的 ort Session
///
/// EP 优先级：CUDA > CPU（其余 EP 通过 ort feature flags 和 NPU_VAULT_EP 环境变量启用）
pub fn build_session(model_path: &Path) -> Result<Session> {
    let npu = crate::platform::detect_npu();

    let builder = Session::builder()
        .map_err(|e| VaultError::Crypto(format!("ort Session::builder: {e}")))?;

    let session = match npu {
        NpuKind::Cuda => builder
            .with_execution_providers([
                CUDAExecutionProvider::default().build(),
                CPUExecutionProvider::default().build(),
            ])
            .map_err(|e| VaultError::Crypto(format!("ort with_execution_providers: {e}")))?
            .commit_from_file(model_path)
            .map_err(|e| VaultError::Crypto(format!("ort commit_from_file: {e}")))?,
        _ => builder
            .with_execution_providers([CPUExecutionProvider::default().build()])
            .map_err(|e| VaultError::Crypto(format!("ort with_execution_providers: {e}")))?
            .commit_from_file(model_path)
            .map_err(|e| VaultError::Crypto(format!("ort commit_from_file: {e}")))?,
    };

    Ok(session)
}

// provider.rs 本身不含可单元测试的纯逻辑，集成测试在有模型文件的环境中运行
```

- [ ] **Step 2: 确认编译**

```bash
cd npu-vault && cargo check -p vault-core 2>&1 | grep "^error"
```

期望：无 error。

- [ ] **Step 3: 提交**

```bash
cd npu-vault && git add crates/vault-core/src/infer/provider.rs
git commit -m "feat(infer): add build_session() with CUDA/CPU EP auto-selection"
```

---

### Task 5：infer/embedding.rs — OrtEmbeddingProvider

**Files:**
- Modify: `npu-vault/crates/vault-core/src/infer/embedding.rs`

- [ ] **Step 1: 写失败测试**

在 `npu-vault/crates/vault-core/src/infer/embedding.rs` 写：

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn ort_embedding_provider_implements_trait() {
        // 确认 OrtEmbeddingProvider 实现了 EmbeddingProvider
        fn assert_impl<T: crate::embed::EmbeddingProvider>() {}
        assert_impl::<super::OrtEmbeddingProvider>();
    }
}
```

- [ ] **Step 2: 确认失败**

```bash
cd npu-vault && cargo test -p vault-core embedding 2>&1 | tail -5
```

期望：编译失败，`OrtEmbeddingProvider` 未定义。

- [ ] **Step 3: 实现 embedding.rs**

```rust
// npu-vault/crates/vault-core/src/infer/embedding.rs

use crate::embed::EmbeddingProvider;
use crate::error::{Result, VaultError};
use ndarray::Array2;
use std::path::Path;
use tokenizers::Tokenizer;

const MAX_SEQ_LEN: usize = 512;

pub struct OrtEmbeddingProvider {
    session: ort::Session,
    tokenizer: Tokenizer,
    dims: usize,
}

impl OrtEmbeddingProvider {
    pub fn new(model_path: &Path, tokenizer_path: &Path, dims: usize) -> Result<Self> {
        let session = super::provider::build_session(model_path)?;
        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| VaultError::Crypto(format!("load tokenizer: {e}")))?;
        Ok(Self { session, tokenizer, dims })
    }

    /// 便捷构造：自动下载 Qwen3-Embedding-0.6B 并加载
    pub fn qwen3_embedding_0_6b() -> Result<Self> {
        let (model_path, tokenizer_path) = super::model_store::ensure_models(
            "Qwen/Qwen3-Embedding-0.6B",
            "onnx/model_quantized.onnx",
            "tokenizer.json",
        )?;
        Self::new(&model_path, &tokenizer_path, 1024)
    }

    fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        // 1. Tokenize（截断到 MAX_SEQ_LEN）
        let encoding = self.tokenizer
            .encode(text, false)
            .map_err(|e| VaultError::Crypto(format!("tokenize: {e}")))?;

        let seq_len = encoding.get_ids().len().min(MAX_SEQ_LEN);
        let ids: Vec<i64> = encoding.get_ids()[..seq_len]
            .iter().map(|&x| x as i64).collect();
        let masks: Vec<i64> = encoding.get_attention_mask()[..seq_len]
            .iter().map(|&x| x as i64).collect();

        // 2. ONNX 推理
        let ids_arr = Array2::from_shape_vec((1, seq_len), ids)
            .map_err(|e| VaultError::Crypto(format!("reshape ids: {e}")))?;
        let masks_arr = Array2::from_shape_vec((1, seq_len), masks)
            .map_err(|e| VaultError::Crypto(format!("reshape masks: {e}")))?;

        let outputs = self.session
            .run(ort::inputs![
                "input_ids" => ids_arr.view(),
                "attention_mask" => masks_arr.view()
            ]
            .map_err(|e| VaultError::Crypto(format!("ort inputs: {e}")))?)
            .map_err(|e| VaultError::Crypto(format!("ort run: {e}")))?;

        // 3. 取 last_hidden_state 并做有效 token 均值池化
        let tensor = outputs["last_hidden_state"]
            .try_extract_tensor::<f32>()
            .map_err(|e| VaultError::Crypto(format!("extract tensor: {e}")))?;
        let view = tensor.view(); // shape [1, seq_len, hidden_dim]
        let hidden_dim = view.shape()[2];

        let mut mean = vec![0.0f32; hidden_dim];
        let attn = encoding.get_attention_mask();
        let valid: f32 = attn[..seq_len].iter().filter(|&&m| m == 1).count()
            .max(1) as f32;

        for t in 0..seq_len {
            if attn[t] == 1 {
                for d in 0..hidden_dim {
                    mean[d] += view[[0, t, d]];
                }
            }
        }
        for v in &mut mean { *v /= valid; }

        // 4. L2 归一化
        let norm: f32 = mean.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 1e-8 { for v in &mut mean { *v /= norm; } }

        Ok(mean)
    }
}

impl EmbeddingProvider for OrtEmbeddingProvider {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        texts.iter().map(|t| self.embed_one(t)).collect()
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    fn is_available(&self) -> bool {
        true // 模型文件已加载则始终可用
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ort_embedding_provider_implements_trait() {
        fn assert_impl<T: crate::embed::EmbeddingProvider>() {}
        assert_impl::<OrtEmbeddingProvider>();
    }
}
```

- [ ] **Step 4: 运行测试**

```bash
cd npu-vault && cargo test -p vault-core infer::embedding 2>&1 | tail -8
```

期望：`1 test passed`（trait bound 编译测试）。

- [ ] **Step 5: 提交**

```bash
cd npu-vault && git add crates/vault-core/src/infer/embedding.rs
git commit -m "feat(infer): add OrtEmbeddingProvider with mean-pool + L2-norm"
```

---

### Task 6：infer/reranker.rs — OrtRerankProvider

**Files:**
- Modify: `npu-vault/crates/vault-core/src/infer/reranker.rs`

- [ ] **Step 1: 写失败测试**

```rust
// npu-vault/crates/vault-core/src/infer/reranker.rs
#[cfg(test)]
mod tests {
    #[test]
    fn ort_reranker_implements_trait() {
        fn assert_impl<T: crate::infer::RerankProvider>() {}
        assert_impl::<super::OrtRerankProvider>();
    }
}
```

- [ ] **Step 2: 确认失败**

```bash
cd npu-vault && cargo test -p vault-core infer::reranker 2>&1 | tail -5
```

- [ ] **Step 3: 实现 reranker.rs**

```rust
// npu-vault/crates/vault-core/src/infer/reranker.rs

use crate::error::{Result, VaultError};
use crate::infer::RerankProvider;
use ndarray::Array2;
use std::path::Path;
use tokenizers::Tokenizer;

const MAX_SEQ_LEN: usize = 512;

pub struct OrtRerankProvider {
    session: ort::Session,
    tokenizer: Tokenizer,
}

impl OrtRerankProvider {
    pub fn new(model_path: &Path, tokenizer_path: &Path) -> Result<Self> {
        let session = super::provider::build_session(model_path)?;
        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| VaultError::Crypto(format!("load reranker tokenizer: {e}")))?;
        Ok(Self { session, tokenizer })
    }

    /// 便捷构造：自动下载 bge-reranker-v2-m3 并加载
    pub fn bge_reranker_v2_m3() -> Result<Self> {
        let (model_path, tokenizer_path) = super::model_store::ensure_models(
            "BAAI/bge-reranker-v2-m3",
            "onnx/model_quantized.onnx",
            "tokenizer.json",
        )?;
        Self::new(&model_path, &tokenizer_path)
    }

    fn score_one(&self, query: &str, document: &str) -> Result<f32> {
        // Cross-encoder: encode (query, document) pair
        let encoding = self.tokenizer
            .encode((query, document), true)
            .map_err(|e| VaultError::Crypto(format!("tokenize pair: {e}")))?;

        let seq_len = encoding.get_ids().len().min(MAX_SEQ_LEN);
        let ids: Vec<i64> = encoding.get_ids()[..seq_len]
            .iter().map(|&x| x as i64).collect();
        let masks: Vec<i64> = encoding.get_attention_mask()[..seq_len]
            .iter().map(|&x| x as i64).collect();

        let ids_arr = Array2::from_shape_vec((1, seq_len), ids)
            .map_err(|e| VaultError::Crypto(format!("reshape ids: {e}")))?;
        let masks_arr = Array2::from_shape_vec((1, seq_len), masks)
            .map_err(|e| VaultError::Crypto(format!("reshape masks: {e}")))?;

        let outputs = self.session
            .run(ort::inputs![
                "input_ids" => ids_arr.view(),
                "attention_mask" => masks_arr.view()
            ]
            .map_err(|e| VaultError::Crypto(format!("ort inputs: {e}")))?)
            .map_err(|e| VaultError::Crypto(format!("ort run: {e}")))?;

        // logits: [1, 1]，取标量后 sigmoid
        let tensor = outputs["logits"]
            .try_extract_tensor::<f32>()
            .map_err(|e| VaultError::Crypto(format!("extract logits: {e}")))?;
        let logit = tensor.view()[[0, 0]];

        // sigmoid(logit) → [0, 1]
        Ok(1.0 / (1.0 + (-logit).exp()))
    }
}

impl RerankProvider for OrtRerankProvider {
    fn score(&self, query: &str, documents: &[&str]) -> Result<Vec<f32>> {
        documents.iter().map(|doc| self.score_one(query, doc)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ort_reranker_implements_trait() {
        fn assert_impl<T: crate::infer::RerankProvider>() {}
        assert_impl::<OrtRerankProvider>();
    }

    #[test]
    fn sigmoid_range() {
        // 验证 sigmoid 公式在极值下收敛
        let big_pos = 1.0f32 / (1.0 + (-10.0f32).exp());
        let big_neg = 1.0f32 / (1.0 + (10.0f32).exp());
        assert!(big_pos > 0.99);
        assert!(big_neg < 0.01);
    }
}
```

- [ ] **Step 4: 运行测试**

```bash
cd npu-vault && cargo test -p vault-core infer::reranker 2>&1 | tail -8
```

期望：`2 tests passed`。

- [ ] **Step 5: 提交**

```bash
cd npu-vault && git add crates/vault-core/src/infer/reranker.rs
git commit -m "feat(infer): add OrtRerankProvider with cross-encoder sigmoid scoring"
```

---

### Task 7：search.rs — SearchParams + SearchContext + search_with_context

**Files:**
- Modify: `npu-vault/crates/vault-core/src/search.rs`

- [ ] **Step 1: 写失败测试**

在 `search.rs` 的 `#[cfg(test)]` 模块末尾加：

```rust
#[test]
fn search_params_defaults_clamp_correctly() {
    let p = SearchParams::with_defaults(5);
    assert_eq!(p.top_k, 5);
    assert_eq!(p.initial_k, 25);   // 5*5=25，在 [20,100] 内
    assert_eq!(p.intermediate_k, 10); // 5*2=10，在 [5,40] 内

    let p2 = SearchParams::with_defaults(1);
    assert_eq!(p2.initial_k, 20);  // min clamp
    assert_eq!(p2.intermediate_k, 2); // max(1, min(2, 40))

    let p3 = SearchParams::with_defaults(30);
    assert_eq!(p3.initial_k, 100); // max clamp
    assert_eq!(p3.intermediate_k, 40); // max clamp
}
```

- [ ] **Step 2: 确认失败**

```bash
cd npu-vault && cargo test -p vault-core search::tests::search_params 2>&1 | tail -5
```

- [ ] **Step 3: 在 search.rs 中添加 SearchParams 和 SearchContext**

在 `search.rs` 文件顶部的 `use` 之后，`RRF_K` 常量之前，添加：

```rust
use std::sync::Arc;
use crate::crypto::Key32;
use crate::embed::EmbeddingProvider;
use crate::index::FulltextIndex;
use crate::infer::RerankProvider;
use crate::store::Store;
use crate::vectors::VectorIndex;

/// 三阶段搜索参数
#[derive(Debug, Clone)]
pub struct SearchParams {
    pub top_k: usize,
    /// 粗召回数量（向量+全文各取此数量后 RRF 融合）
    pub initial_k: usize,
    /// Reranker 入口前的候选数量
    pub intermediate_k: usize,
}

impl SearchParams {
    pub fn with_defaults(top_k: usize) -> Self {
        let initial_k = (top_k * 5).clamp(20, 100);
        let intermediate_k = (top_k * 2).clamp(top_k, 40);
        Self { top_k, initial_k, intermediate_k }
    }
}

/// 搜索上下文：持有所有搜索所需组件的引用
pub struct SearchContext<'a> {
    pub fulltext: Option<&'a FulltextIndex>,
    pub vectors: Option<&'a VectorIndex>,
    pub embedding: Option<Arc<dyn EmbeddingProvider>>,
    pub reranker: Option<Arc<dyn RerankProvider>>,
    pub store: &'a Store,
    pub dek: &'a Key32,
}
```

- [ ] **Step 4: 添加 search_with_context 函数**

在 `rerank` 函数之后，`#[cfg(test)]` 之前插入：

```rust
/// 三阶段搜索：initial_k 粗召回 → intermediate_k RRF 融合 → Rerank → top_k 返回
///
/// 同时被 search 端点和 chat 引擎调用，避免重复逻辑。
pub fn search_with_context(
    ctx: &SearchContext<'_>,
    query: &str,
    params: &SearchParams,
) -> crate::error::Result<Vec<SearchResult>> {
    // 1. 全文搜索（initial_k）
    let ft_results = ctx.fulltext
        .map(|ft| ft.search(query, params.initial_k).unwrap_or_default())
        .unwrap_or_default();

    // 2. 向量搜索（initial_k）
    let (vec_results, query_vec): (Vec<(String, f32)>, Option<Vec<f32>>) =
        match (&ctx.embedding, &ctx.vectors) {
            (Some(emb), Some(vecs)) => {
                match emb.embed(&[query]) {
                    Ok(e) if !e.is_empty() => {
                        let qv = e[0].clone();
                        let vr = vecs.search(&qv, params.initial_k)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|(meta, score)| (meta.item_id, score))
                            .collect();
                        (vr, Some(qv))
                    }
                    _ => (vec![], None),
                }
            }
            _ => (vec![], None),
        };

    // 3. RRF 融合 → intermediate_k
    let fused = rrf_fuse(&vec_results, &ft_results, DEFAULT_VECTOR_WEIGHT, DEFAULT_FULLTEXT_WEIGHT, params.intermediate_k);

    // 4. 获取并解密 items
    let mut results: Vec<SearchResult> = Vec::new();
    for (item_id, score) in &fused {
        if let Ok(Some(item)) = ctx.store.get_item(ctx.dek, item_id) {
            results.push(SearchResult {
                item_id: item.id,
                score: *score,
                title: item.title,
                content: item.content,
                source_type: item.source_type,
                inject_content: None,
            });
        }
    }

    // 5. Rerank（有 Reranker 时用 cross-encoder；有 query 向量时用余弦；否则跳过）
    if params.intermediate_k <= RERANK_TOP_K_THRESHOLD {
        if let Some(reranker) = &ctx.reranker {
            let docs: Vec<&str> = results.iter().map(|r| r.content.as_str()).collect();
            if let Ok(scores) = reranker.score(query, &docs) {
                for (r, s) in results.iter_mut().zip(scores.iter()) {
                    r.score = *s;
                }
                results.sort_by(|a, b| b.score.partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal));
            }
        } else if let Some(qvec) = &query_vec {
            if let Some(vecs) = ctx.vectors {
                rerank(qvec, &mut results, vecs);
            }
        }
    }

    // 6. 截取 top_k
    results.truncate(params.top_k);
    Ok(results)
}
```

- [ ] **Step 5: 运行测试**

```bash
cd npu-vault && cargo test -p vault-core search 2>&1 | tail -15
```

期望：所有原有测试 + 新增 `search_params_defaults_clamp_correctly` 通过。

- [ ] **Step 6: 提交**

```bash
cd npu-vault && git add crates/vault-core/src/search.rs
git commit -m "feat(search): add SearchParams, SearchContext, search_with_context (three-stage pipeline)"
```

---

### Task 8：重构 chat.rs + 修复 routes/chat.rs 截断 bug

**Files:**
- Modify: `npu-vault/crates/vault-core/src/chat.rs`
- Modify: `npu-vault/crates/vault-server/src/routes/chat.rs`

- [ ] **Step 1: 写测试确认 chat 现有测试通过**

```bash
cd npu-vault && cargo test -p vault-core chat 2>&1 | tail -10
```

记录当前测试数量，重构后需保持全部通过。

- [ ] **Step 2: 重构 chat.rs 的 search_for_context 使用 search_with_context**

将 `npu-vault/crates/vault-core/src/chat.rs` 中 `search_for_context` 方法替换为：

```rust
fn search_for_context(&self, query: &str, dek: &Key32, top_k: usize) -> Result<Vec<SearchResult>> {
    let ft_guard = self.fulltext.lock().unwrap();
    let vec_guard = self.vectors.lock().unwrap();
    let emb_guard = self.embedding.lock().unwrap();

    let ctx = crate::search::SearchContext {
        fulltext: ft_guard.as_ref(),
        vectors: vec_guard.as_ref(),
        embedding: emb_guard.clone(),
        reranker: None, // ChatEngine 构造时无 reranker（server 层已处理）
        store: &self.store.lock().unwrap(),
        dek,
    };
    let params = crate::search::SearchParams::with_defaults(top_k);
    let mut results = crate::search::search_with_context(&ctx, query, &params)?;
    allocate_budget(&mut results, INJECTION_BUDGET);
    Ok(results)
}
```

同时在 `ChatEngine` 结构体和 `new()` 中添加 `reranker` 字段：

```rust
pub struct ChatEngine {
    llm: Arc<dyn LlmProvider>,
    store: Arc<Mutex<Store>>,
    fulltext: Arc<Mutex<Option<FulltextIndex>>>,
    vectors: Arc<Mutex<Option<VectorIndex>>>,
    embedding: Arc<Mutex<Option<Arc<dyn crate::embed::EmbeddingProvider>>>>,
    reranker: Arc<Mutex<Option<Arc<dyn crate::infer::RerankProvider>>>>,
}

impl ChatEngine {
    pub fn new(
        llm: Arc<dyn LlmProvider>,
        store: Arc<Mutex<Store>>,
        fulltext: Arc<Mutex<Option<FulltextIndex>>>,
        vectors: Arc<Mutex<Option<VectorIndex>>>,
        embedding: Arc<Mutex<Option<Arc<dyn crate::embed::EmbeddingProvider>>>>,
        reranker: Arc<Mutex<Option<Arc<dyn crate::infer::RerankProvider>>>>,
    ) -> Self {
        Self { llm, store, fulltext, vectors, embedding, reranker }
    }
    // ... 其余方法不变
}
```

在 `search_for_context` 中使用 `self.reranker.lock().unwrap().clone()` 传入 `ctx.reranker`。

- [ ] **Step 3: 修复 routes/chat.rs 中 500 字符截断 bug**

打开 `npu-vault/crates/vault-server/src/routes/chat.rs`，找到第 117 行左右：

```rust
// 删除这行（500 字符截断）：
"content": item.content.chars().take(500).collect::<String>(),
// 替换为（完整内容，由 chat.rs 的 allocate_budget 控制长度）：
"content": item.content,
```

- [ ] **Step 4: 更新 chat 测试中 ChatEngine::new 调用（新增 reranker 参数）**

在 `chat.rs` 的 `#[cfg(test)]` 中，所有 `ChatEngine::new(mock, store, fulltext, vectors, embedding)` 调用末尾添加：

```rust
Arc::new(Mutex::new(None::<Arc<dyn crate::infer::RerankProvider>>))
```

- [ ] **Step 5: 运行全部 chat 测试**

```bash
cd npu-vault && cargo test -p vault-core chat 2>&1 | tail -10
```

期望：原有测试数量全部通过。

- [ ] **Step 6: 提交**

```bash
cd npu-vault && git add crates/vault-core/src/chat.rs crates/vault-server/src/routes/chat.rs
git commit -m "refactor(chat): use search_with_context, fix 500-char truncation bug, add reranker field"
```

---

### Task 9：llm.rs — 新增 OpenAiLlmProvider

**Files:**
- Modify: `npu-vault/crates/vault-core/src/llm.rs`

- [ ] **Step 1: 写失败测试**

在 `llm.rs` 的 `#[cfg(test)]` 末尾加：

```rust
#[test]
fn openai_provider_creation() {
    let p = OpenAiLlmProvider::new("https://api.openai.com/v1", "sk-test", "gpt-4o-mini");
    assert_eq!(p.model_name(), "gpt-4o-mini");
    assert!(p.is_available()); // 构造后始终返回 true（可用性由健康检查决定）
}
```

- [ ] **Step 2: 确认失败**

```bash
cd npu-vault && cargo test -p vault-core llm::tests::openai 2>&1 | tail -5
```

- [ ] **Step 3: 在 llm.rs 末尾（MockLlmProvider 之前）添加 OpenAiLlmProvider**

在 `llm.rs` 中的 `/// 测试专用 Mock` 注释前插入：

```rust
/// OpenAI-compatible LLM client
///
/// 支持任意兼容 OpenAI Chat Completions API 的后端：
///   - OpenAI:     endpoint = "https://api.openai.com/v1"
///   - Ollama v1:  endpoint = "http://localhost:11434/v1"
///   - LM Studio:  endpoint = "http://localhost:1234/v1"
///   - vLLM:       endpoint = "http://localhost:8000/v1"
pub struct OpenAiLlmProvider {
    client: reqwest::Client,
    endpoint: String,   // 末尾无 '/'
    api_key: String,
    model: String,
}

#[derive(Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
}

#[derive(Deserialize)]
struct OpenAiMessage {
    content: String,
}

impl OpenAiLlmProvider {
    pub fn new(endpoint: &str, api_key: &str, model: &str) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("HTTP client"),
            endpoint: endpoint.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            model: model.to_string(),
        }
    }

    fn chat_sync_impl(&self, messages: &[ChatMessage]) -> Result<String> {
        let url = format!("{}/chat/completions", self.endpoint);
        let body = serde_json::json!({
            "model": &self.model,
            "messages": messages,
            "stream": false,
        });
        let client = self.client.clone();
        let body_bytes = serde_json::to_vec(&body)?;
        let api_key = self.api_key.clone();

        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new()
                .map_err(|e| VaultError::Crypto(format!("tokio: {e}")))?;
            rt.block_on(async move {
                let resp = client
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .header("Authorization", format!("Bearer {api_key}"))
                    .body(body_bytes)
                    .send().await
                    .map_err(|e| VaultError::LlmUnavailable(format!("openai request: {e}")))?;
                let parsed: OpenAiResponse = resp.json().await
                    .map_err(|e| VaultError::Classification(format!("parse openai response: {e}")))?;
                parsed.choices.into_iter().next()
                    .map(|c| c.message.content)
                    .ok_or_else(|| VaultError::Classification("empty choices".into()))
            })
        });
        handle.join().map_err(|_| VaultError::Crypto("thread panic".into()))?
    }
}

impl LlmProvider for OpenAiLlmProvider {
    fn chat(&self, system: &str, user: &str) -> Result<String> {
        let messages = vec![
            ChatMessage::system(system),
            ChatMessage::user(user),
        ];
        self.chat_sync_impl(&messages)
    }

    fn chat_with_history(&self, messages: &[ChatMessage]) -> Result<String> {
        self.chat_sync_impl(messages)
    }

    fn is_available(&self) -> bool {
        true // 可用性在调用时由 HTTP 错误体现，构造时不检查
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}
```

- [ ] **Step 4: 运行 llm 测试**

```bash
cd npu-vault && cargo test -p vault-core llm 2>&1 | tail -12
```

期望：所有原有测试 + `openai_provider_creation` 通过。

- [ ] **Step 5: 提交**

```bash
cd npu-vault && git add crates/vault-core/src/llm.rs
git commit -m "feat(llm): add OpenAiLlmProvider for OpenAI-compat endpoint (Ollama/OpenAI/LM Studio/vLLM)"
```

---

### Task 10：state.rs + routes/search.rs — 接入 reranker 字段与三阶段参数

**Files:**
- Modify: `npu-vault/crates/vault-server/src/state.rs`
- Modify: `npu-vault/crates/vault-server/src/routes/search.rs`

- [ ] **Step 1: 在 state.rs 中添加 reranker 字段**

在 `AppState` 结构体中，`embedding` 字段后插入：

```rust
pub reranker: Mutex<Option<Arc<dyn vault_core::infer::RerankProvider>>>,
```

在 `AppState::new()` 的字段列表中对应加：

```rust
reranker: Mutex::new(None),
```

在 `clear_search_engines()` 中加：

```rust
*self.reranker.lock().unwrap() = None;
```

- [ ] **Step 2: 在 init_search_engines 中尝试加载 OrtEmbeddingProvider**

在 `init_search_engines` 中，当前 `OllamaProvider::default()` 那段替换为：

```rust
// 优先使用本地 ONNX embedding；文件不存在时降级到 Ollama
if let Ok(mut guard) = self.embedding.lock() {
    let provider: Arc<dyn vault_core::embed::EmbeddingProvider> =
        match vault_core::infer::embedding::OrtEmbeddingProvider::qwen3_embedding_0_6b() {
            Ok(p) => {
                tracing::info!("Embedding: OrtEmbeddingProvider (Qwen3-Embedding-0.6B)");
                Arc::new(p)
            }
            Err(e) => {
                tracing::info!("ONNX embedding unavailable ({e}), falling back to Ollama bge-m3");
                Arc::new(vault_core::embed::OllamaProvider::default())
            }
        };
    *guard = Some(provider);
}

// 尝试加载 OrtRerankProvider
if let Ok(mut guard) = self.reranker.lock() {
    match vault_core::infer::reranker::OrtRerankProvider::bge_reranker_v2_m3() {
        Ok(r) => {
            tracing::info!("Reranker: OrtRerankProvider (bge-reranker-v2-m3)");
            *guard = Some(Arc::new(r));
        }
        Err(e) => {
            tracing::info!("Reranker unavailable ({e}), will use vector cosine fallback");
        }
    }
}
```

- [ ] **Step 3: 更新 routes/search.rs 的 SearchQuery 加 initial_k / intermediate_k**

打开 `npu-vault/crates/vault-server/src/routes/search.rs`，在 `SearchQuery` 结构体中添加两个可选字段：

```rust
#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: String,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    pub initial_k: Option<usize>,
    pub intermediate_k: Option<usize>,
}
```

在 `search` 函数中，`rrf_fuse` 调用前添加：

```rust
let params = {
    let top_k = params.top_k;
    let mut p = vault_core::search::SearchParams::with_defaults(top_k);
    if let Some(ik) = params.initial_k { p.initial_k = ik; }
    if let Some(imk) = params.intermediate_k { p.intermediate_k = imk; }
    p
};
```

将 `search` 函数的搜索主逻辑替换为调用 `search_with_context`：

```rust
let dek = {
    let vault = state.vault.lock().map_err(|_| err_500("vault lock poisoned"))?;
    vault.dek_db().map_err(|e| (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()}))))?
};

let reranker = state.reranker.lock().map_err(|_| err_500("reranker lock"))?.clone();
let emb = state.embedding.lock().map_err(|_| err_500("emb lock"))?.clone();

let results = {
    let ft_guard = state.fulltext.lock().map_err(|_| err_500("ft lock"))?;
    let vec_guard = state.vectors.lock().map_err(|_| err_500("vec lock"))?;
    let vault_guard = state.vault.lock().map_err(|_| err_500("vault lock"))?;

    let ctx = vault_core::search::SearchContext {
        fulltext: ft_guard.as_ref(),
        vectors: vec_guard.as_ref(),
        embedding: emb,
        reranker,
        store: vault_guard.store(),
        dek: &dek,
    };
    vault_core::search::search_with_context(&ctx, &search_q.q, &params)
        .map_err(|e| err_500(&e.to_string()))?
};
```

同样更新 `search_relevant` 函数中的 `RelevantRequest` 和调用逻辑（添加 `initial_k`/`intermediate_k` 字段并传入 `SearchParams`）。

- [ ] **Step 4: 运行全量测试**

```bash
cd npu-vault && cargo test 2>&1 | tail -20
```

期望：所有测试通过（197 个测试 + 新增的）。

- [ ] **Step 5: 提交**

```bash
cd npu-vault && git add crates/vault-server/src/state.rs crates/vault-server/src/routes/search.rs
git commit -m "feat(server): wire reranker into AppState, add initial_k/intermediate_k to search API"
```

---

### Task 11：全量测试 + 文档更新

**Files:**
- Modify: `npu-vault/RELEASE.md`

- [ ] **Step 1: 运行全量测试**

```bash
cd npu-vault && cargo test 2>&1 | tail -30
```

期望：所有测试通过，无回归。

- [ ] **Step 2: 运行 clippy**

```bash
cd npu-vault && cargo clippy -p vault-core -p vault-server -- -D warnings 2>&1 | head -30
```

修复所有 warning。

- [ ] **Step 3: 更新 RELEASE.md**

在 `npu-vault/RELEASE.md` 最新版本的变更记录末尾添加：

```markdown
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
- `routes/chat.rs`: 修复 500 字符截断 bug
```

- [ ] **Step 4: 最终提交**

```bash
cd npu-vault && git add npu-vault/RELEASE.md
git commit -m "docs: update RELEASE.md for Plan A (search quality + infer layer)"
```

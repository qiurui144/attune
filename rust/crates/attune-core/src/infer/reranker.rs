// npu-vault/crates/vault-core/src/infer/reranker.rs

use crate::edge_cloud::capacity::{DEFAULT_PROBE_TIMEOUT, DEFAULT_SCHEDULER_BASE};
use crate::edge_cloud::scheduler::{
    LocalSchedulerClient, SchedulerJobStatus, SchedulerKbTaskResponse,
};
use crate::error::{Result, VaultError};
use crate::infer::RerankProvider;
#[cfg(feature = "local-inference")]
use ort::value::Tensor;
#[cfg(feature = "local-inference")]
use std::path::Path;
#[cfg(feature = "local-inference")]
use std::sync::Mutex;
use std::time::{Duration, Instant};
#[cfg(feature = "local-inference")]
use tokenizers::Tokenizer;

/// BGE-reranker-base (BAAI 官方) / Xenova/bge-reranker-base — 均基于 XLM-RoBERTa-base，
/// position_embeddings 维度 = 514（max 实际可用 token 数 = 512，扣 2 个特殊 token）。
/// 之前设 2048 会触发 ONNX `Gather: indices element out of data bounds, idx=514 ...`
/// 错误，在长文档检索中 reranker 100% 静默失败、永远 fallback 到 RRF，是 v0.6/v0.7 隐藏
/// ranking quality 杀手（2026-05-24 50-query rust-book benchmark 发现）。
///
/// 注意：bge-reranker-v2-m3（多语言，max 8192）尚未默认启用，若未来切到 v2-m3 这条路径
/// 时需要把这里改回 8192 或通过 env var 区分。
#[cfg(feature = "local-inference")]
const MAX_SEQ_LEN: usize = 512;

#[cfg(feature = "local-inference")]
pub struct OrtRerankProvider {
    session: Mutex<ort::session::Session>,
    tokenizer: Tokenizer,
}

#[cfg(feature = "local-inference")]
impl OrtRerankProvider {
    pub fn new(model_path: &Path, tokenizer_path: &Path) -> Result<Self> {
        let session = super::provider::build_session(model_path)?;
        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| VaultError::Crypto(format!("load reranker tokenizer: {e}")))?;
        Ok(Self {
            session: Mutex::new(session),
            tokenizer,
        })
    }

    /// 便捷构造：自动下载 ONNX reranker。
    ///
    /// 模型来源选择历程（记录以防未来又需要切换）：
    /// - 原定：`BAAI/bge-reranker-v2-m3` + `onnx/model_quantized.onnx` ——
    ///   HF 上官方仓库没有任何 ONNX 文件（只有 safetensors），404 失败。
    /// - 现选：`Xenova/bge-reranker-base` + `onnx/model_quantized.onnx` ——
    ///   Xenova 专注 transformers.js 的 ONNX 转换镜像，成熟可靠，
    ///   提供 `model_quantized.onnx` 约 110MB，下载 + 加载都快。
    /// - 降级：`BAAI/bge-reranker-base` + `onnx/model.onnx` ——
    ///   官方仓库有完整 ONNX（330MB），Xenova 若失联则用。
    ///
    /// 多语言（中文）支持：
    ///   bge-reranker-base 主训练英文，跨语言能力一般；下面的 multilingual 版本
    ///   `jinaai/jina-reranker-v2-base-multilingual` 可通过 env var
    ///   `ATTUNE_RERANKER_MODEL` 切换启用。
    pub fn bge_reranker_v2_m3() -> Result<Self> {
        // v0.6 Phase B fix：默认切到 BAAI 官方 bge-reranker-base ONNX。
        // 原默认 Xenova/bge-reranker-base 量化版有 known issue：某些中文长文档触发
        // ONNX `Expand node invalid shape` 错误（见 server log），让 reranker 永久
        // 退化到 RRF 排序，是法律 / 中文检索的隐藏 ranking 杀手。
        // BAAI 官方 model.onnx (330MB full precision) 不量化，没这个 bug。
        // 也提供 jina-v2-multilingual 作为多语言可选（中文支持更好）。
        let (repo, file) = match std::env::var("ATTUNE_RERANKER_MODEL").as_deref() {
            Ok("jina-v2-multilingual") => (
                "jinaai/jina-reranker-v2-base-multilingual",
                "onnx/model_quantized.onnx",
            ),
            Ok("xenova-quantized") => ("Xenova/bge-reranker-base", "onnx/model_quantized.onnx"),
            _ => (
                // 默认：BAAI 官方 ONNX (full precision, 330MB) — 稳定，无 Expand bug
                "BAAI/bge-reranker-base",
                "onnx/model.onnx",
            ),
        };
        let (model_path, tokenizer_path) =
            super::model_store::ensure_models(repo, file, "tokenizer.json")?;
        Self::new(&model_path, &tokenizer_path)
    }

    fn score_one(&self, query: &str, document: &str) -> Result<f32> {
        // 1. Tokenize pair (query, document) with special tokens
        let encoding = self
            .tokenizer
            .encode((query, document), true)
            .map_err(|e| VaultError::Crypto(format!("tokenize pair: {e}")))?;

        let seq_len = encoding.get_ids().len().min(MAX_SEQ_LEN);
        let ids: Vec<i64> = encoding.get_ids()[..seq_len]
            .iter()
            .map(|&x| x as i64)
            .collect();
        let masks: Vec<i64> = encoding.get_attention_mask()[..seq_len]
            .iter()
            .map(|&x| x as i64)
            .collect();
        let type_ids: Vec<i64> = encoding.get_type_ids()[..seq_len]
            .iter()
            .map(|&x| x as i64)
            .collect();

        // 2. 构建 ort Tensor
        let ids_tensor = Tensor::<i64>::from_array((vec![1usize, seq_len], ids))
            .map_err(|e| VaultError::Crypto(format!("ids tensor: {e}")))?;

        let masks_tensor = Tensor::<i64>::from_array((vec![1usize, seq_len], masks))
            .map_err(|e| VaultError::Crypto(format!("masks tensor: {e}")))?;

        let token_type_tensor = Tensor::<i64>::from_array((vec![1usize, seq_len], type_ids))
            .map_err(|e| VaultError::Crypto(format!("token_type tensor: {e}")))?;

        // 3. ONNX 推理
        // 部分 reranker 变体（如 DeBERTa 系列）不包含 token_type_ids 输入，
        // 根据 session.inputs 动态决定是否传入，避免 OrtError: unknown input name
        let mut session = self
            .session
            .lock()
            .map_err(|_| VaultError::Crypto("session mutex poisoned".into()))?;
        let has_token_type_ids = session
            .inputs()
            .iter()
            .any(|i| i.name() == "token_type_ids");
        let mut outputs = if has_token_type_ids {
            session
                .run(ort::inputs! {
                    "input_ids" => ids_tensor,
                    "attention_mask" => masks_tensor,
                    "token_type_ids" => token_type_tensor
                })
                .map_err(|e| VaultError::Crypto(format!("ort run: {e}")))?
        } else {
            session
                .run(ort::inputs! {
                    "input_ids" => ids_tensor,
                    "attention_mask" => masks_tensor
                })
                .map_err(|e| VaultError::Crypto(format!("ort run (no token_type_ids): {e}")))?
        };

        // 4. 取 logits 输出（bge-reranker-v2-m3 标准输出名为 "logits"），shape: [1, 1]
        // 不使用 keys().next() 以避免 HashMap 迭代顺序不确定问题
        let output_value = outputs
            .remove("logits")
            .ok_or_else(|| VaultError::Crypto("ort output 'logits' missing".into()))?;

        let (_shape, flat) = output_value
            .try_extract_tensor::<f32>()
            .map_err(|e| VaultError::Crypto(format!("extract tensor: {e}")))?;

        // 5. sigmoid(logit)
        let logit = flat
            .first()
            .copied()
            .ok_or_else(|| VaultError::Crypto("empty logits tensor".into()))?;
        let score = 1.0f32 / (1.0 + (-logit).exp());
        Ok(score)
    }
}

#[cfg(feature = "local-inference")]
impl RerankProvider for OrtRerankProvider {
    fn score(&self, query: &str, documents: &[&str]) -> Result<Vec<f32>> {
        documents
            .iter()
            .map(|doc| self.score_one(query, doc))
            .collect()
    }
}

/// Scheduler-native reranker.
///
/// Attune submits rerank as an application task and never loads an ORT session in
/// the server process. The scheduler is responsible for selecting ORT, llama.cpp,
/// GPU EPs, or any platform-specific implementation.
pub struct LocalSchedulerRerankProvider {
    client: LocalSchedulerClient,
    task: String,
    poll_timeout: Duration,
    max_document_chars: usize,
}

impl LocalSchedulerRerankProvider {
    pub fn new(base_url: &str, task: &str, poll_timeout_ms: u64) -> Self {
        let task = task.trim();
        Self {
            client: LocalSchedulerClient::with_base(
                if base_url.trim().is_empty() {
                    DEFAULT_SCHEDULER_BASE
                } else {
                    base_url
                },
                DEFAULT_PROBE_TIMEOUT,
            ),
            task: if task.is_empty() {
                "kb.query.rerank".to_string()
            } else {
                task.to_string()
            },
            poll_timeout: Duration::from_millis(poll_timeout_ms.max(1_000)),
            max_document_chars: env_usize_any(
                &[
                    "ATTUNE_RERANK_MAX_INPUT_CHARS",
                    "ATTUNE_SCHEDULER_RERANK_MAX_INPUT_CHARS",
                    "ATTUNE_LOCAL_RERANK_MAX_INPUT_CHARS",
                ],
                1024,
            )
            .clamp(128, 8192),
        }
    }

    fn final_outputs(&self, response: SchedulerKbTaskResponse) -> Result<serde_json::Value> {
        let is_async =
            response.job_id.is_some() || response.scheduled_as.eq_ignore_ascii_case("async");
        if !is_async {
            return Ok(response.outputs);
        }
        let job_id = response.job_id.ok_or_else(|| {
            VaultError::LlmUnavailable("local scheduler rerank missing job_id".into())
        })?;
        let deadline = Instant::now() + self.poll_timeout;
        let mut last_poll_error: Option<String> = None;
        loop {
            if Instant::now() >= deadline {
                let detail = last_poll_error
                    .map(|err| format!("; last poll error: {err}"))
                    .unwrap_or_default();
                return Err(VaultError::LlmUnavailable(format!(
                    "local scheduler rerank job {job_id} timed out after {} ms{detail}",
                    self.poll_timeout.as_millis(),
                )));
            }
            std::thread::sleep(Duration::from_millis(100));
            let job = match self.client.job(&job_id) {
                Ok(job) => job,
                Err(err) => {
                    last_poll_error = Some(err.to_string());
                    continue;
                }
            };
            if scheduler_job_done(&job) {
                return Ok(job.outputs);
            }
            if scheduler_job_failed(&job) {
                return Err(VaultError::LlmUnavailable(format!(
                    "local scheduler rerank job failed: {}",
                    job.error.or(job.detail).unwrap_or_else(|| job.status)
                )));
            }
        }
    }

    fn extract_scores(value: &serde_json::Value) -> Option<Vec<f32>> {
        for pointer in [
            "/scores",
            "/outputs/scores",
            "/results",
            "/outputs/results",
            "/data",
            "/outputs/data",
        ] {
            if let Some(scores) = value.pointer(pointer).and_then(scores_from_array) {
                return Some(scores);
            }
        }
        value.get("outputs").and_then(Self::extract_scores)
    }
}

impl RerankProvider for LocalSchedulerRerankProvider {
    fn score(&self, query: &str, documents: &[&str]) -> Result<Vec<f32>> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }
        let bounded_documents: Vec<String> = documents
            .iter()
            .map(|doc| bounded_text(doc, self.max_document_chars))
            .collect();
        let body = serde_json::json!({
            "query": query,
            "documents": bounded_documents,
        });
        let response = self.client.submit_kb_task(&self.task, &body, false)?;
        let outputs = self.final_outputs(response)?;
        let scores = Self::extract_scores(&outputs).ok_or_else(|| {
            VaultError::LlmUnavailable(format!(
                "local scheduler rerank response missing scores: {outputs}"
            ))
        })?;
        if scores.len() != documents.len() {
            return Err(VaultError::LlmUnavailable(format!(
                "local scheduler rerank returned {} scores for {} documents",
                scores.len(),
                documents.len()
            )));
        }
        Ok(scores)
    }
}

fn scores_from_array(value: &serde_json::Value) -> Option<Vec<f32>> {
    let arr = value.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    let mut indexed_out = vec![None; arr.len()];
    let mut saw_index = false;
    for item in arr {
        let score = score_from_value(item)?;
        if let Some(index) = item
            .get("index")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
        {
            if index >= indexed_out.len() || indexed_out[index].is_some() {
                return None;
            }
            saw_index = true;
            indexed_out[index] = Some(score);
        } else if saw_index {
            return None;
        } else {
            out.push(score);
        }
    }
    if saw_index {
        if !out.is_empty() {
            return None;
        }
        indexed_out.into_iter().collect()
    } else {
        Some(out)
    }
}

fn score_from_value(item: &serde_json::Value) -> Option<f32> {
    if let Some(n) = item.as_f64() {
        return Some(n as f32);
    }
    item.get("score")
        .or_else(|| item.get("relevance"))
        .or_else(|| item.get("rerank_score"))
        .or_else(|| item.get("relevance_score"))
        .and_then(|v| v.as_f64())
        .map(|n| n as f32)
}

fn scheduler_job_done(job: &SchedulerJobStatus) -> bool {
    matches!(
        job.status.to_ascii_lowercase().as_str(),
        "done" | "completed" | "complete" | "success" | "succeeded"
    )
}

fn scheduler_job_failed(job: &SchedulerJobStatus) -> bool {
    matches!(
        job.status.to_ascii_lowercase().as_str(),
        "error" | "failed" | "failure" | "canceled" | "cancelled" | "expired"
    )
}

fn env_usize_any(keys: &[&str], default: usize) -> usize {
    keys.iter()
        .find_map(|key| {
            std::env::var(key)
                .ok()
                .and_then(|v| v.trim().parse::<usize>().ok())
                .filter(|v| *v > 0)
        })
        .unwrap_or(default)
}

fn bounded_text(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    let total = trimmed.chars().count();
    if total <= max_chars {
        return trimmed.to_string();
    }
    const ELLIPSIS: &str = "\n...\n";
    if max_chars <= ELLIPSIS.chars().count() + 2 {
        return trimmed.chars().take(max_chars).collect();
    }
    let body_budget = max_chars - ELLIPSIS.chars().count();
    let head_budget = body_budget / 2 + body_budget % 2;
    let tail_budget = body_budget / 2;
    let head: String = trimmed.chars().take(head_budget).collect();
    let tail: String = trimmed
        .chars()
        .rev()
        .take(tail_budget)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{}{}{}", head.trim_end(), ELLIPSIS, tail.trim_start())
}

#[cfg(test)]
mod scheduler_rerank_tests {
    use super::*;

    #[test]
    fn bounded_text_keeps_head_and_tail() {
        let text = format!("{}MID{}", "a".repeat(2000), "z".repeat(2000));
        let out = bounded_text(&text, 512);
        assert!(out.chars().count() <= 512);
        assert!(out.contains("\n...\n"));
        assert!(out.starts_with('a'));
        assert!(out.ends_with('z'));
    }

    #[test]
    fn scores_from_array_accepts_scheduler_relevance_score() {
        let value = serde_json::json!([
            {"index": 2, "relevance_score": 0.7},
            {"index": 0, "relevance_score": 0.3},
            {"index": 1, "relevance_score": 0.5}
        ]);
        assert_eq!(scores_from_array(&value), Some(vec![0.3, 0.5, 0.7]));
    }

    #[test]
    fn scores_from_array_rejects_duplicate_scheduler_index() {
        let value = serde_json::json!([
            {"index": 0, "relevance_score": 0.3},
            {"index": 0, "relevance_score": 0.5}
        ]);
        assert_eq!(scores_from_array(&value), None);
    }
}

#[cfg(all(test, feature = "local-inference"))]
mod tests {
    use super::*;

    #[test]
    fn ort_reranker_implements_trait() {
        fn assert_impl<T: crate::infer::RerankProvider>() {}
        assert_impl::<OrtRerankProvider>();
    }

    #[test]
    fn sigmoid_range() {
        let big_pos = 1.0f32 / (1.0 + (-10.0f32).exp());
        let big_neg = 1.0f32 / (1.0 + (10.0f32).exp());
        assert!(big_pos > 0.99);
        assert!(big_neg < 0.01);
    }

    /// Regression: BGE-reranker-base / Xenova/bge-reranker-base 都基于 XLM-RoBERTa-base，
    /// 其 position_embeddings dim 是 514（max 实际 token = 512）。
    /// 之前 MAX_SEQ_LEN=2048 让 reranker 在长文档上 100% 静默失败
    /// （ONNX `Gather: indices element out of data bounds idx=514`），
    /// fallback 到 RRF，是 v0.6/v0.7 隐藏的 ranking quality 杀手。
    /// 修复见 docs/superpowers/specs/2026-05-24-knowledge-base-deepseek-rag-audit.md §B1。
    #[test]
    fn max_seq_len_within_bge_reranker_position_embedding_bound() {
        // BGE-reranker-base position_embeddings weight: dims=[514, 768]
        // 加上 RoBERTa 的 padding_idx + 1 offset，实际安全 token 数 = 512
        // 编译期常量断言：若切换 bge-reranker-v2-m3 等 8192-position 模型，需区分常量。
        const _: () = assert!(MAX_SEQ_LEN <= 512);
    }
}

// npu-vault/crates/vault-core/src/infer/reranker.rs

use crate::error::{Result, VaultError};
use crate::infer::RerankProvider;
use ort::value::Tensor;
use std::path::Path;
use std::sync::Mutex;
use tokenizers::Tokenizer;

/// BGE-reranker-base (BAAI 官方) / Xenova/bge-reranker-base — 均基于 XLM-RoBERTa-base，
/// position_embeddings 维度 = 514（max 实际可用 token 数 = 512，扣 2 个特殊 token）。
/// 之前设 2048 会触发 ONNX `Gather: indices element out of data bounds, idx=514 ...`
/// 错误，在长文档检索中 reranker 100% 静默失败、永远 fallback 到 RRF，是 v0.6/v0.7 隐藏
/// ranking quality 杀手（2026-05-24 50-query rust-book benchmark 发现）。
///
/// 注意：bge-reranker-v2-m3（多语言，max 8192）尚未默认启用，若未来切到 v2-m3 这条路径
/// 时需要把这里改回 8192 或通过 env var 区分。
const MAX_SEQ_LEN: usize = 512;

pub struct OrtRerankProvider {
    session: Mutex<ort::session::Session>,
    tokenizer: Tokenizer,
}

impl OrtRerankProvider {
    pub fn new(model_path: &Path, tokenizer_path: &Path) -> Result<Self> {
        let session = super::provider::build_session(model_path)?;
        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| VaultError::Crypto(format!("load reranker tokenizer: {e}")))?;
        Ok(Self { session: Mutex::new(session), tokenizer })
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
            Ok("xenova-quantized") => (
                "Xenova/bge-reranker-base",
                "onnx/model_quantized.onnx",
            ),
            _ => (
                // 默认：BAAI 官方 ONNX (full precision, 330MB) — 稳定，无 Expand bug
                "BAAI/bge-reranker-base",
                "onnx/model.onnx",
            ),
        };
        let (model_path, tokenizer_path) = super::model_store::ensure_models(
            repo,
            file,
            "tokenizer.json",
        )?;
        Self::new(&model_path, &tokenizer_path)
    }

    fn score_one(&self, query: &str, document: &str) -> Result<f32> {
        // 1. Tokenize pair (query, document) with special tokens
        let encoding = self.tokenizer
            .encode((query, document), true)
            .map_err(|e| VaultError::Crypto(format!("tokenize pair: {e}")))?;

        let seq_len = encoding.get_ids().len().min(MAX_SEQ_LEN);
        let ids: Vec<i64> = encoding.get_ids()[..seq_len]
            .iter().map(|&x| x as i64).collect();
        let masks: Vec<i64> = encoding.get_attention_mask()[..seq_len]
            .iter().map(|&x| x as i64).collect();
        let type_ids: Vec<i64> = encoding.get_type_ids()[..seq_len]
            .iter().map(|&x| x as i64).collect();

        // 2. 构建 ort Tensor
        let ids_tensor = Tensor::<i64>::from_array(
            (vec![1usize, seq_len], ids)
        ).map_err(|e| VaultError::Crypto(format!("ids tensor: {e}")))?;

        let masks_tensor = Tensor::<i64>::from_array(
            (vec![1usize, seq_len], masks)
        ).map_err(|e| VaultError::Crypto(format!("masks tensor: {e}")))?;

        let token_type_tensor = Tensor::<i64>::from_array(
            (vec![1usize, seq_len], type_ids)
        ).map_err(|e| VaultError::Crypto(format!("token_type tensor: {e}")))?;

        // 3. ONNX 推理
        // 部分 reranker 变体（如 DeBERTa 系列）不包含 token_type_ids 输入，
        // 根据 session.inputs 动态决定是否传入，避免 OrtError: unknown input name
        let mut session = self.session.lock()
            .map_err(|_| VaultError::Crypto("session mutex poisoned".into()))?;
        let has_token_type_ids = session.inputs().iter().any(|i| i.name() == "token_type_ids");
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
        let output_value = outputs.remove("logits")
            .ok_or_else(|| VaultError::Crypto("ort output 'logits' missing".into()))?;

        let (_shape, flat) = output_value
            .try_extract_tensor::<f32>()
            .map_err(|e| VaultError::Crypto(format!("extract tensor: {e}")))?;

        // 5. sigmoid(logit)
        let logit = flat.first()
            .copied()
            .ok_or_else(|| VaultError::Crypto("empty logits tensor".into()))?;
        let score = 1.0f32 / (1.0 + (-logit).exp());
        Ok(score)
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

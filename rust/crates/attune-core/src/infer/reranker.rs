// npu-vault/crates/vault-core/src/infer/reranker.rs

use crate::error::{Result, VaultError};
use crate::infer::RerankProvider;
use ort::value::Tensor;
use std::path::Path;
use std::sync::Mutex;
use tokenizers::Tokenizer;

/// bge-reranker-v2-m3 最大支持 8192 tokens；设 2048 作为安全默认值，
/// 在精度与推理内存之间取得平衡。
const MAX_SEQ_LEN: usize = 2048;

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

    /// 便捷构造：自动下载 BAAI/bge-reranker-v2-m3 并加载
    pub fn bge_reranker_v2_m3() -> Result<Self> {
        let (model_path, tokenizer_path) = super::model_store::ensure_models(
            "BAAI/bge-reranker-v2-m3",
            "onnx/model_quantized.onnx",
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
}

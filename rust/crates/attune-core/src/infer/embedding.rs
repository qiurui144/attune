// npu-vault/crates/vault-core/src/infer/embedding.rs

use crate::embed::EmbeddingProvider;
use crate::error::{Result, VaultError};
use ort::value::Tensor;
use std::path::Path;
use std::sync::Mutex;
use tokenizers::Tokenizer;

/// Qwen3-Embedding-0.6B 最大支持 32768 tokens；设 2048 作为安全默认值，
/// 覆盖绝大多数文档而不超出 ORT 推理内存预算。
/// 如有需要可通过 NPU_VAULT_EMBED_MAX_SEQ_LEN 环境变量覆盖。
const MAX_SEQ_LEN: usize = 2048;

pub struct OrtEmbeddingProvider {
    session: Mutex<ort::session::Session>,
    tokenizer: Tokenizer,
    dims: usize,
}

impl OrtEmbeddingProvider {
    pub fn new(model_path: &Path, tokenizer_path: &Path, dims: usize) -> Result<Self> {
        let session = super::provider::build_session(model_path)?;
        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| VaultError::Crypto(format!("load tokenizer: {e}")))?;
        Ok(Self { session: Mutex::new(session), tokenizer, dims })
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

        // 2. 构建 ort Tensor，使用 (shape_vec, data_vec) 形式
        let ids_tensor = Tensor::<i64>::from_array(
            (vec![1usize, seq_len], ids)
        ).map_err(|e| VaultError::Crypto(format!("ids tensor: {e}")))?;

        // clone before move: masks 后续用于均值池化
        let masks_tensor = Tensor::<i64>::from_array(
            (vec![1usize, seq_len], masks.clone())
        ).map_err(|e| VaultError::Crypto(format!("masks tensor: {e}")))?;

        // 3. ONNX 推理
        let mut session = self.session.lock()
            .map_err(|_| VaultError::Crypto("session mutex poisoned".into()))?;
        let mut outputs = session
            .run(ort::inputs! {
                "input_ids" => ids_tensor,
                "attention_mask" => masks_tensor
            })
            .map_err(|e| VaultError::Crypto(format!("ort run: {e}")))?;

        // 4. 取 last_hidden_state 输出（Qwen3-Embedding 标准输出名），shape: [1, seq_len, hidden_dim]
        // 不使用 keys().next() 以避免 HashMap 迭代顺序不确定问题
        let output_value = outputs.remove("last_hidden_state")
            .ok_or_else(|| VaultError::Crypto("ort output 'last_hidden_state' missing".into()))?;

        let (shape, flat) = output_value
            .try_extract_tensor::<f32>()
            .map_err(|e| VaultError::Crypto(format!("extract tensor: {e}")))?;

        // shape: [batch=1, seq_len, hidden_dim]  (Shape deref to [i64])
        if shape.len() < 3 {
            return Err(VaultError::Crypto(
                format!("unexpected tensor rank {}", shape.len())
            ));
        }
        let hidden_dim = shape[2] as usize;

        // 5. 有效 token 均值池化（flat 是 row-major: offset = t * hidden_dim + d）
        // 复用已截断的 masks（Vec<i64>），避免再次从 encoding 取全长 mask 造成歧义
        let mut mean = vec![0.0f32; hidden_dim];
        let valid: f32 = masks.iter().filter(|&&m| m == 1).count().max(1) as f32;
        // 用 model_seq_len（shape[1]）限制迭代范围，防止 flat 越界（ONNX 模型可能在内部截断）
        let model_seq_len = shape[1] as usize;
        let iter_len = masks.len().min(model_seq_len);

        for (t, &mask) in masks[..iter_len].iter().enumerate() {
            if mask == 1 {
                let offset = t * hidden_dim;
                for d in 0..hidden_dim {
                    mean[d] += flat[offset + d];
                }
            }
        }
        for v in &mut mean { *v /= valid; }

        // 6. L2 归一化
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
        true
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

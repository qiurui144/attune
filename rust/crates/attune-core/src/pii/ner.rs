//! L2 ONNX NER scaffold (Phase A.5.6, partial)
//!
//! 状态：trait + stub 实现就位；ONNX 模型集成排到 v0.7。
//!
//! ## 计划
//!
//! - 模型：`hfl/chinese-roberta-wwm-ext-large` 或等价中文 RoBERTa
//!   - Xenova 镜像或 BAAI 系列的 NER finetune
//!   - 标签：BIO 格式 (B-PER / I-PER / B-LOC / I-LOC / B-ORG / I-ORG / O)
//!   - 量化 ONNX 大小 ~280MB，按硬件 tier (T1 Low+) 自动下载
//!
//! - 集成：
//!   - 复用 `attune-core::infer::model_store::ensure_models` 下载 + 缓存
//!   - 复用 `tokenizers::Tokenizer` 处理（与 OrtEmbeddingProvider 同模式）
//!   - BIO 标签序列 → 实体跨度（合并连续 B/I 块）
//!   - 输出 `Vec<NerEntity>` → 与 L1 Redactor 输出格式对齐
//!
//! - 触发：
//!   - 用户在 Settings 启用 "L2 NER" toggle（默认关闭）
//!   - 启用后下载模型（首次有进度条）
//!   - 出网前 redactor 链式调用：L1 (regex/dict) → L2 (NER)
//!
//! ## 当前 stub 行为
//!
//! `OrtNerProvider::detect_named_entities` 返回空 Vec，让上游代码可以提前接入接口
//! 而不阻塞下游编译。v0.7 替换为真 ONNX 推理。

use serde::{Deserialize, Serialize};

/// 识别出的命名实体。span 是 byte offset。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NerEntity {
    /// 实体类型："PER" (人名) / "LOC" (地点) / "ORG" (组织) / "MISC"
    pub label: String,
    pub text: String,
    pub byte_start: usize,
    pub byte_end: usize,
    /// NER 模型置信度（0..1）
    pub score: f32,
}

/// L2 NER 抽取器接口。v0.7 由 OrtNerProvider 实现。
pub trait NerProvider: Send + Sync {
    /// 输入文本 → 命名实体列表。空文本返回空 Vec。
    fn detect_named_entities(&self, text: &str) -> Vec<NerEntity>;

    /// 模型名（用于 settings 显示 + audit log）
    fn model_name(&self) -> &str;
}

/// Stub L2 NER provider — 永远返回空 Vec。
///
/// v0.6 出货版本不下载模型也能编译；v0.7 替换为 OrtNerProvider 实际推理。
pub struct StubNerProvider;

impl NerProvider for StubNerProvider {
    fn detect_named_entities(&self, _text: &str) -> Vec<NerEntity> {
        Vec::new()
    }

    fn model_name(&self) -> &str {
        "stub-not-loaded"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_returns_empty() {
        let p = StubNerProvider;
        let r = p.detect_named_entities("张三在上海工作");
        assert!(r.is_empty(), "stub should return empty");
        assert_eq!(p.model_name(), "stub-not-loaded");
    }

    #[test]
    fn ner_entity_serializes() {
        let e = NerEntity {
            label: "PER".into(),
            text: "张三".into(),
            byte_start: 0,
            byte_end: 6,
            score: 0.95,
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"PER\""));
        assert!(json.contains("\"score\":0.95"));
    }
}

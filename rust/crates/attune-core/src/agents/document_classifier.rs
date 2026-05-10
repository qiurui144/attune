//! document_classifier_agent — 文档分类 + 简单理解.
//!
//! 编排 3 个内部 skill: classify_chunk_kind / extract_entities / parse_chinese_date.
//! 输入: 一组文档 (text + filename), 输出: 每份的 ClassifiedEvidence.
//!
//! 不做行业精解 (借条第几条 / 法条引用) — 行业 agent 负责.

use crate::skills::{classify_chunk_kind, extract_entities};
use serde::{Deserialize, Serialize};

/// 单份文档分类结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassifiedEvidence {
    pub file: String,
    /// classify_chunk_kind 输出 ("borrowing_doc" / "contract" / ...)
    pub kind: String,
    pub confidence: f64,
    pub matched_keywords: Vec<String>,
    /// extract_entities 输出 (人 / 日期 / 金额 / 地点 / 组织)
    pub entities: extract_entities::Entities,
    /// chunk text 长度 (字符数)
    pub text_length: usize,
}

/// 输入: 一份文档
pub struct DocumentInput<'a> {
    pub file: &'a str,
    pub text: &'a str,
}

/// agent 业务输出
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassificationOutput {
    pub classified: Vec<ClassifiedEvidence>,
    /// 按 kind 聚合的统计 (kind → count)
    pub kind_summary: std::collections::BTreeMap<String, usize>,
}

pub type Output = super::AgentOutput<ClassificationOutput>;

/// 主入口: 批量分类 + 实体抽取
pub fn run(inputs: &[DocumentInput<'_>]) -> Output {
    let mut classified = Vec::with_capacity(inputs.len());
    let mut kind_summary = std::collections::BTreeMap::new();
    let mut audit_lines = Vec::new();
    let mut low_confidence_count = 0usize;

    audit_lines.push(format!("[document_classifier_agent] 处理 {} 份文档", inputs.len()));

    for input in inputs {
        let cls = classify_chunk_kind::classify(input.text);
        let ents = extract_entities::extract(input.text);
        let kind_str = cls.kind.as_str().to_string();

        *kind_summary.entry(kind_str.clone()).or_insert(0) += 1;
        if cls.confidence < 0.3 {
            low_confidence_count += 1;
        }

        audit_lines.push(format!(
            "  [{}] kind={} conf={:.2} entities=(p:{}, d:{}, a:{}, l:{}, o:{})",
            input.file, kind_str, cls.confidence,
            ents.persons.len(), ents.dates.len(), ents.amounts.len(),
            ents.locations.len(), ents.organizations.len()
        ));

        classified.push(ClassifiedEvidence {
            file: input.file.to_string(),
            kind: kind_str,
            confidence: cls.confidence,
            matched_keywords: cls.matched_keywords,
            entities: ents,
            text_length: input.text.chars().count(),
        });
    }

    let mut missing = Vec::new();
    let mut followups = Vec::new();
    if low_confidence_count > 0 {
        missing.push(format!(
            "{} 份文档分类置信度 < 0.3, 可能未识别正确类型", low_confidence_count
        ));
        followups.push("低置信度文档建议人工核对或追加 LLM 二次分类".into());
    }

    let overall_conf = if classified.is_empty() {
        0.0
    } else {
        classified.iter().map(|c| c.confidence).sum::<f64>() / classified.len() as f64
    };

    Output {
        computation: ClassificationOutput { classified, kind_summary },
        audit_trail: audit_lines.join("\n"),
        red_lines_violated: vec![],   // 文档分类无硬红线
        missing_evidence: missing,
        followups,
        confidence: overall_conf,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc<'a>(file: &'a str, text: &'a str) -> DocumentInput<'a> {
        DocumentInput { file, text }
    }

    #[test]
    fn classifies_mixed_evidence_pool() {
        let docs = vec![
            doc("借条.pdf", "借条\n出借人: 张三\n借款人: 李四\n借款本金: 人民币伍拾万元整\n月利率 1%"),
            doc("流水.pdf", "交易日期 2023-01-15  对方户名 李四  交易金额 +500000  余额 1000000  汇入"),
            doc("微信.png", "[微信] 张三: 借款已转 [图片]\n李四: 收到 嗯嗯"),
            doc("收据.pdf", "收据\n今收到张三还款人民币贰万元整\n开票日期: 2023-06-15"),
            doc("判决.pdf", "(2024)京01民初1号民事判决书\n本院查明: ...\n判决如下: ..."),
        ];
        let out = run(&docs);
        assert_eq!(out.computation.classified.len(), 5);

        let kinds: Vec<&str> = out.computation.classified.iter().map(|c| c.kind.as_str()).collect();
        assert!(kinds.contains(&"borrowing_doc"));
        assert!(kinds.contains(&"bank_statement"));
        assert!(kinds.contains(&"chat"));
        assert!(kinds.contains(&"receipt"));
        assert!(kinds.contains(&"judgment"));

        // kind_summary 含 5 个不同 kind 各 1 份
        assert_eq!(out.computation.kind_summary.len(), 5);
    }

    #[test]
    fn extracts_entities_per_document() {
        let docs = vec![doc(
            "借条.pdf",
            "原告张三 (北京市海淀区) 与被告李四 (上海市浦东新区) 因借款 500000 元纠纷, 2023年1月15日签订",
        )];
        let out = run(&docs);
        let c = &out.computation.classified[0];
        assert!(c.entities.persons.iter().any(|p| p.starts_with("张三")));
        assert!(c.entities.persons.iter().any(|p| p.starts_with("李四")));
        assert!(c.entities.dates.contains(&"2023-01-15".to_string()));
        assert!(c.entities.amounts.iter().any(|a| (a.value - 500_000.0).abs() < 0.01));
        assert!(c.entities.locations.iter().any(|l| l.contains("海淀区")));
    }

    #[test]
    fn empty_input_returns_empty_output() {
        let out = run(&[]);
        assert!(out.computation.classified.is_empty());
        assert_eq!(out.confidence, 0.0);
        assert!(out.red_lines_violated.is_empty());
    }

    #[test]
    fn low_confidence_triggers_followup() {
        let docs = vec![doc("noise.txt", "今天天气不错")];
        let out = run(&docs);
        assert_eq!(out.computation.classified[0].kind, "other");
        assert!(out.confidence < 0.3);
        assert!(out.missing_evidence.iter().any(|m| m.contains("置信度")));
        assert!(out.followups.iter().any(|f| f.contains("人工核对")));
    }

    #[test]
    fn confidence_aggregation_averages_per_doc() {
        let docs = vec![
            doc("strong.pdf", "借条 出借人 借款人 本金 月利率 利息 还款期限"),
            doc("weak.pdf", "随便写点东西"),
        ];
        let out = run(&docs);
        // 一强一弱, 平均落在中间
        assert!(out.confidence > 0.0);
        assert!(out.confidence < 0.7);
    }

    #[test]
    fn audit_trail_lists_each_doc() {
        let docs = vec![
            doc("a.pdf", "借条 借款人 出借人"),
            doc("b.pdf", "交易日期 余额"),
        ];
        let out = run(&docs);
        assert!(out.audit_trail.contains("a.pdf"));
        assert!(out.audit_trail.contains("b.pdf"));
        assert!(out.audit_trail.contains("处理 2 份文档"));
    }
}

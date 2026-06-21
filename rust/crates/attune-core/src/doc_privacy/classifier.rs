//! Document classifier — confidential-keyword detection + sensitivity grading.
//!
//! Borrows the *design* of `kvm-info-privacy::classifier` (keyword → Classified
//! fail-closed block; high PII density → SensitivePartial) but is reimplemented
//! natively. kvm used an Aho-Corasick automaton; attune uses a small
//! case-insensitive substring scan to avoid pulling a new direct dependency
//! (the confidential-keyword list is short, so O(n·m) is fine and keeps the
//! cross-platform / WASM surface clean).
//!
//! **OSS boundary** (CLAUDE.md): the default keyword set is *generic* only
//! (绝密 / 机密 / confidential / …). Industry-specific markers (律所"案卷密" /
//! 医院"病历") are injected by attune-pro plugins via [`DocClassifier::with_keywords`],
//! never hard-coded here.

use crate::doc_privacy::models::{Classification, DocEntity};

/// Generic confidential markers valid for any individual user (零行业绑定).
/// Industry markers come from pro plugins, not this list.
pub const DEFAULT_CONFIDENTIAL_KEYWORDS: &[&str] = &[
    "绝密",
    "机密",
    "秘密",
    "内部资料",
    "禁止外传",
    "不得外传",
    "confidential",
    "top secret",
    "classified",
    "do not distribute",
    "internal only",
];

/// When this many `high`-security PII entities appear, the doc is graded
/// `SensitivePartial` (advisory, not blocking). Mirrors kvm's threshold.
const SENSITIVE_PARTIAL_THRESHOLD: usize = 5;

pub struct DocClassifier {
    /// Lower-cased keywords for case-insensitive matching.
    keywords_lc: Vec<String>,
    /// Original-cased keywords (for the human-readable block reason).
    keywords_orig: Vec<String>,
}

impl Default for DocClassifier {
    fn default() -> Self {
        Self::with_keywords(DEFAULT_CONFIDENTIAL_KEYWORDS.iter().map(|s| s.to_string()))
    }
}

impl DocClassifier {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build from an explicit keyword set (pro plugins extend the default set
    /// by passing default ++ their own markers).
    pub fn with_keywords<I: IntoIterator<Item = String>>(keywords: I) -> Self {
        let keywords_orig: Vec<String> = keywords.into_iter().collect();
        let keywords_lc = keywords_orig.iter().map(|k| k.to_lowercase()).collect();
        Self { keywords_lc, keywords_orig }
    }

    /// Classify a document from its extracted text + detected PII entities.
    ///
    /// Order matters (fail-closed first): a confidential marker anywhere ⇒
    /// `Classified` + `blocked=true`, regardless of PII density.
    pub fn classify(&self, text: &str, entities: &[DocEntity]) -> ClassificationResult {
        let text_lc = text.to_lowercase();
        for (i, kw_lc) in self.keywords_lc.iter().enumerate() {
            if !kw_lc.is_empty() && text_lc.contains(kw_lc.as_str()) {
                let kw = &self.keywords_orig[i];
                return ClassificationResult {
                    classification: Classification::Classified,
                    blocked: true,
                    block_reason: Some(format!("检测到保密标记：\u{201c}{kw}\u{201d}")),
                    warning: None,
                };
            }
        }

        // High PII density → advisory SensitivePartial (count "high" security
        // kinds: id_card / bank_card / credit_card carry the highest risk).
        let high_risk = entities.iter().filter(|e| is_high_risk(&e.kind)).count();
        if high_risk >= SENSITIVE_PARTIAL_THRESHOLD {
            return ClassificationResult {
                classification: Classification::SensitivePartial,
                blocked: false,
                block_reason: None,
                warning: Some(format!(
                    "文档含 {high_risk} 处高风险隐私实体，建议全量脱敏后再导出"
                )),
            };
        }

        ClassificationResult {
            classification: Classification::Normal,
            blocked: false,
            block_reason: None,
            warning: None,
        }
    }
}

fn is_high_risk(kind: &crate::pii::PiiKind) -> bool {
    use crate::pii::PiiKind::*;
    matches!(kind, IdCard | BankCard | CreditCard)
}

#[derive(Debug, Clone)]
pub struct ClassificationResult {
    pub classification: Classification,
    pub blocked: bool,
    pub block_reason: Option<String>,
    pub warning: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pii::PiiKind;

    fn ent(kind: PiiKind) -> DocEntity {
        DocEntity { kind, page: 0, layer: "text".into(), byte_start: Some(0), byte_end: Some(1) }
    }

    #[test]
    fn confidential_keyword_blocks_fail_closed() {
        let c = DocClassifier::new();
        let r = c.classify("本文件属于公司绝密资料，请勿外传", &[]);
        assert_eq!(r.classification, Classification::Classified);
        assert!(r.blocked, "classified doc must be fail-closed blocked");
        assert!(r.block_reason.unwrap().contains("绝密"));
    }

    #[test]
    fn english_confidential_marker_case_insensitive() {
        let c = DocClassifier::new();
        let r = c.classify("This document is CONFIDENTIAL.", &[]);
        assert_eq!(r.classification, Classification::Classified);
        assert!(r.blocked);
    }

    #[test]
    fn keyword_beats_pii_density() {
        // Even with lots of PII, a confidential marker forces Classified (block),
        // never the weaker SensitivePartial.
        let c = DocClassifier::new();
        let ents: Vec<_> = (0..10).map(|_| ent(PiiKind::IdCard)).collect();
        let r = c.classify("机密：客户名单", &ents);
        assert_eq!(r.classification, Classification::Classified);
        assert!(r.blocked);
    }

    #[test]
    fn high_pii_density_is_sensitive_partial() {
        let c = DocClassifier::new();
        let ents: Vec<_> = (0..6).map(|_| ent(PiiKind::IdCard)).collect();
        let r = c.classify("客户资料汇总表", &ents);
        assert_eq!(r.classification, Classification::SensitivePartial);
        assert!(!r.blocked, "sensitive_partial is advisory, not blocking");
        assert!(r.warning.is_some());
    }

    #[test]
    fn low_pii_clean_doc_is_normal() {
        let c = DocClassifier::new();
        let r = c.classify("普通文档，含一个手机号", &[ent(PiiKind::Phone)]);
        assert_eq!(r.classification, Classification::Normal);
        assert!(!r.blocked);
    }

    #[test]
    fn empty_doc_is_normal() {
        let c = DocClassifier::new();
        let r = c.classify("", &[]);
        assert_eq!(r.classification, Classification::Normal);
    }

    #[test]
    fn plugin_keyword_injection_extends_default() {
        // pro plugin adds an industry marker on top of the generic defaults.
        let kws: Vec<String> = DEFAULT_CONFIDENTIAL_KEYWORDS
            .iter()
            .map(|s| s.to_string())
            .chain(std::iter::once("案卷密".to_string()))
            .collect();
        let c = DocClassifier::with_keywords(kws);
        let r = c.classify("本卷宗为案卷密级", &[]);
        assert_eq!(r.classification, Classification::Classified);
        // generic default still works too
        let r2 = c.classify("机密文件", &[]);
        assert_eq!(r2.classification, Classification::Classified);
    }
}

//! 案件库 (CaseVault) metadata schema — 与 Project 关联.
//!
//! 一个 Project 可绑定一个 case_metadata, 含案件类型 + parties + 自动分类后的证据列表.
//! evidence_pool 即 Project.files (现有 Project 文件夹模型), 不重复存储.

use crate::agents::document_classifier::ClassifiedEvidence;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CaseMetadata {
    /// 案件类型 id (如 "civil-loan", "civil-marriage", "criminal-defense").
    /// 由付费插件 registers_case_kinds 提供; OSS 裸装 = None.
    pub kind: Option<String>,
    /// 双方角色
    #[serde(default)]
    pub parties: Vec<Party>,
    /// 案号 (如有)
    pub case_no: Option<String>,
    /// AI 自动分类后的证据列表 (document_classifier_agent 输出)
    #[serde(default)]
    pub classified_evidence: Vec<ClassifiedEvidence>,
    /// 案件备注 (用户/律师手写)
    #[serde(default)]
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Party {
    pub name: String,
    /// "plaintiff" / "defendant" / "third_party"
    pub role: String,
    /// 是否我方代理
    #[serde(default)]
    pub is_our_client: bool,
}

impl CaseMetadata {
    pub fn new(kind: impl Into<Option<String>>) -> Self {
        Self {
            kind: kind.into(),
            ..Default::default()
        }
    }

    pub fn add_party(mut self, name: &str, role: &str, is_our_client: bool) -> Self {
        self.parties.push(Party {
            name: name.to_string(),
            role: role.to_string(),
            is_our_client,
        });
        self
    }

    /// 我方 (代理方) 姓名
    pub fn our_client_name(&self) -> Option<&str> {
        self.parties
            .iter()
            .find(|p| p.is_our_client)
            .map(|p| p.name.as_str())
    }

    /// 对方姓名 (相对我方)
    pub fn opposing_party_name(&self) -> Option<&str> {
        self.parties.iter().find(|p| !p.is_our_client).map(|p| p.name.as_str())
    }

    /// 序列化到 JSON 文件 (供 Project 持久化)
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }

    /// 替换分类结果 (event: evidence_classified 触发后调用)
    pub fn update_classified(&mut self, evidence: Vec<ClassifiedEvidence>) {
        self.classified_evidence = evidence;
    }

    /// 按 kind 过滤已分类证据
    pub fn evidence_by_kind(&self, kind: &str) -> Vec<&ClassifiedEvidence> {
        self.classified_evidence.iter().filter(|e| e.kind == kind).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_with_parties() {
        let m = CaseMetadata::new(Some("civil-loan".into()))
            .add_party("张三", "plaintiff", true)
            .add_party("李四", "defendant", false);
        assert_eq!(m.kind.as_deref(), Some("civil-loan"));
        assert_eq!(m.our_client_name(), Some("张三"));
        assert_eq!(m.opposing_party_name(), Some("李四"));
    }

    #[test]
    fn json_roundtrip() {
        let m = CaseMetadata::new(Some("civil-loan".into()))
            .add_party("A", "plaintiff", true);
        let s = m.to_json().expect("ser");
        let back = CaseMetadata::from_json(&s).expect("de");
        assert_eq!(back.kind, m.kind);
        assert_eq!(back.parties.len(), 1);
    }

    #[test]
    fn empty_metadata_is_default() {
        let m = CaseMetadata::default();
        assert!(m.kind.is_none());
        assert!(m.parties.is_empty());
        assert!(m.classified_evidence.is_empty());
    }

    #[test]
    fn evidence_by_kind_filters() {
        let mut m = CaseMetadata::default();
        m.classified_evidence = vec![
            ClassifiedEvidence {
                file: "a.pdf".into(),
                kind: "borrowing_doc".into(),
                confidence: 0.9,
                matched_keywords: vec![],
                entities: Default::default(),
                text_length: 100,
            },
            ClassifiedEvidence {
                file: "b.pdf".into(),
                kind: "bank_statement".into(),
                confidence: 0.8,
                matched_keywords: vec![],
                entities: Default::default(),
                text_length: 200,
            },
        ];
        assert_eq!(m.evidence_by_kind("borrowing_doc").len(), 1);
        assert_eq!(m.evidence_by_kind("bank_statement").len(), 1);
        assert_eq!(m.evidence_by_kind("contract").len(), 0);
    }
}

//! `card_v1` — 名片字段抽取 (6 字段: name/company/job_title/phone/email/address).
//!
//! Spec §4.2: 启发式 + 关键词字典 + 正则. 高标杆场景 (Z 方案, 92% 红线).
//! D2.4 实施.

use super::{FieldValue, StructuredFields};
use crate::ocr::RawLine;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CardFields {
    pub name: FieldValue,
    pub company: FieldValue,
    pub job_title: FieldValue,
    pub phone: FieldValue,
    pub email: FieldValue,
    pub address: FieldValue,
}

/// D2.1 stub — 所有字段返 None. D2.4 实施.
pub fn extract(_lines: &[RawLine]) -> StructuredFields {
    StructuredFields::CardV1 {
        fields: CardFields::default(),
        unrecognized_fields: vec![
            "name".into(),
            "company".into(),
            "job_title".into(),
            "phone".into(),
            "email".into(),
            "address".into(),
        ],
        validation_warnings: vec![],
    }
}

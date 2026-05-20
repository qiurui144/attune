//! `id_card_cn_v1` / `bank_card_v1` / `business_license_v1` — 3 子类型卡证抽取.
//!
//! Spec §4.2: subtype 由调用方显式指定, OCR 不猜 (per "不确定就问律师"原则).
//! 高准确度红线 (95%, 卡证字段位置固定).
//! D2.5 实施.

use super::{FieldValue, StructuredFields};
use crate::ocr::RawLine;
use serde::{Deserialize, Serialize};

// ─── 居民身份证 (id_card_cn_v1) ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IdCardCnFields {
    pub name: FieldValue,
    pub gender: FieldValue,
    pub nationality: FieldValue,
    pub birth_date: FieldValue,
    pub address: FieldValue,
    pub id_number: FieldValue,
}

// ─── 银行卡 (bank_card_v1) ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BankCardFields {
    pub card_number: FieldValue,
    pub bank_name: FieldValue,
    pub card_type: FieldValue,
    pub valid_thru: FieldValue,
}

// ─── 营业执照 (business_license_v1) ──────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BusinessLicenseFields {
    pub registration_no: FieldValue,
    pub company_name: FieldValue,
    pub legal_rep: FieldValue,
    pub registered_capital: FieldValue,
    pub established_date: FieldValue,
    pub scope: FieldValue,
}

/// 入口路由 — 按 subtype 选 schema. 未知 subtype 返 None.
pub fn extract(_lines: &[RawLine], subtype: &str) -> Option<StructuredFields> {
    match subtype {
        "id_card_cn" => Some(StructuredFields::IdCardCnV1 {
            fields: IdCardCnFields::default(),
            unrecognized_fields: vec![
                "name".into(),
                "gender".into(),
                "nationality".into(),
                "birth_date".into(),
                "address".into(),
                "id_number".into(),
            ],
            validation_warnings: vec![],
        }),
        "bank_card" => Some(StructuredFields::BankCardV1 {
            fields: BankCardFields::default(),
            unrecognized_fields: vec![
                "card_number".into(),
                "bank_name".into(),
                "card_type".into(),
                "valid_thru".into(),
            ],
            validation_warnings: vec![],
        }),
        "business_license" => Some(StructuredFields::BusinessLicenseV1 {
            fields: BusinessLicenseFields::default(),
            unrecognized_fields: vec![
                "registration_no".into(),
                "company_name".into(),
                "legal_rep".into(),
                "registered_capital".into(),
                "established_date".into(),
                "scope".into(),
            ],
            validation_warnings: vec![],
        }),
        _ => None,
    }
}

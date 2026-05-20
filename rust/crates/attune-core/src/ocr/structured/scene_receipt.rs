//! `receipt_v1` — 发票/收据 7 字段抽取.
//!
//! Spec §4.2: invoice_no, issue_date, seller, buyer, amount_total, tax_amount, amount_chinese.
//! 锚点正则 + bbox 邻近 + 校验 (normalize_date / normalize_amount).
//! D2.2 实施.

use super::{FieldValue, StructuredFields};
use crate::ocr::RawLine;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReceiptFields {
    pub invoice_no: FieldValue,
    pub issue_date: FieldValue,
    pub seller: FieldValue,
    pub buyer: FieldValue,
    pub amount_total: FieldValue,
    pub tax_amount: FieldValue,
    pub amount_chinese: FieldValue,
}

/// D2.1 stub — 所有字段返 None. D2.2 实施.
pub fn extract(_lines: &[RawLine]) -> StructuredFields {
    StructuredFields::ReceiptV1 {
        fields: ReceiptFields::default(),
        unrecognized_fields: vec![
            "invoice_no".into(),
            "issue_date".into(),
            "seller".into(),
            "buyer".into(),
            "amount_total".into(),
            "tax_amount".into(),
            "amount_chinese".into(),
        ],
        validation_warnings: vec![],
    }
}

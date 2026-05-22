//! D5.4 — L2 property-based tests (proptest invariants).
//!
//! Spec §6.3:
//!   1. prop_field_value_confidence_in_range — FieldValue.confidence ∈ [0, 1]
//!   2. prop_schema_serde_roundtrip — StructuredFields serde round-trip preserves variant
//!   3. prop_bbox_serde_no_panic — arbitrary u32 quadruples for BBox always serde clean
//!
//! 不依赖 HTTP server (避免每个 proptest case 启服务太慢).

use attune_core::ocr::structured::scene_card::CardFields;
use attune_core::ocr::structured::scene_document::DocumentFields;
use attune_core::ocr::structured::scene_id_card::{
    BankCardFields, BusinessLicenseFields, IdCardCnFields,
};
use attune_core::ocr::structured::scene_receipt::ReceiptFields;
use attune_core::ocr::structured::scene_table::TableFields;
use attune_core::ocr::structured::{FieldValue, StructuredFields};
use attune_core::ocr::{BBox, RawLine};
use proptest::prelude::*;

// ─── Strategies ─────────────────────────────────────────────────────────────

fn arb_bbox() -> impl Strategy<Value = BBox> {
    (any::<u32>(), any::<u32>(), any::<u32>(), any::<u32>())
        .prop_map(|(x, y, w, h)| BBox { x, y, w, h })
}

fn arb_raw_line() -> impl Strategy<Value = RawLine> {
    (".*", arb_bbox(), 0.0f32..=1.0f32).prop_map(|(text, bbox, confidence)| RawLine {
        text,
        bbox,
        confidence,
    })
}

fn arb_field_value() -> impl Strategy<Value = FieldValue> {
    (
        proptest::option::of(".*"),
        0.0f32..=1.0f32,
        proptest::option::of(arb_bbox()),
        proptest::option::of(any::<usize>()),
    )
        .prop_map(|(value, confidence, bbox, source_line_idx)| FieldValue {
            value,
            confidence,
            bbox,
            source_line_idx,
        })
}

fn arb_structured_fields() -> impl Strategy<Value = StructuredFields> {
    prop_oneof![
        Just(StructuredFields::DocumentV1 {
            fields: DocumentFields::default(),
            unrecognized_fields: vec![],
            validation_warnings: vec![],
        }),
        arb_field_value().prop_map(|fv| {
            StructuredFields::ReceiptV1 {
                fields: ReceiptFields { invoice_no: fv, ..Default::default() },
                unrecognized_fields: vec![],
                validation_warnings: vec![],
            }
        }),
        Just(StructuredFields::TableV1 {
            fields: TableFields::default(),
            unrecognized_fields: vec![],
            validation_warnings: vec![],
        }),
        arb_field_value().prop_map(|fv| {
            StructuredFields::CardV1 {
                fields: CardFields { name: fv, ..Default::default() },
                unrecognized_fields: vec![],
                validation_warnings: vec![],
            }
        }),
        arb_field_value().prop_map(|fv| {
            StructuredFields::IdCardCnV1 {
                fields: IdCardCnFields { name: fv, ..Default::default() },
                unrecognized_fields: vec![],
                validation_warnings: vec![],
            }
        }),
        arb_field_value().prop_map(|fv| {
            StructuredFields::BankCardV1 {
                fields: BankCardFields { card_number: fv, ..Default::default() },
                unrecognized_fields: vec![],
                validation_warnings: vec![],
            }
        }),
        arb_field_value().prop_map(|fv| {
            StructuredFields::BusinessLicenseV1 {
                fields: BusinessLicenseFields { company_name: fv, ..Default::default() },
                unrecognized_fields: vec![],
                validation_warnings: vec![],
            }
        }),
    ]
}

// ─── Invariants ─────────────────────────────────────────────────────────────

proptest! {
    /// FieldValue 序列化后 confidence 必须仍 ∈ [0, 1]
    #[test]
    fn prop_field_value_confidence_in_range(fv in arb_field_value()) {
        prop_assert!(fv.confidence >= 0.0 && fv.confidence <= 1.0);
        let json = serde_json::to_string(&fv).expect("ser");
        let de: FieldValue = serde_json::from_str(&json).expect("de");
        prop_assert!(de.confidence >= 0.0 && de.confidence <= 1.0);
        prop_assert_eq!(de.value, fv.value);
    }

    /// StructuredFields ser → str → de → eq (variant 保持)
    #[test]
    fn prop_schema_serde_roundtrip(sf in arb_structured_fields()) {
        let json = serde_json::to_string(&sf).expect("ser");
        let de: StructuredFields = serde_json::from_str(&json).expect("de");
        // Variant tag must round-trip — check via debug
        let orig_tag = match &sf {
            StructuredFields::DocumentV1 { .. } => "DocumentV1",
            StructuredFields::ReceiptV1 { .. } => "ReceiptV1",
            StructuredFields::TableV1 { .. } => "TableV1",
            StructuredFields::CardV1 { .. } => "CardV1",
            StructuredFields::IdCardCnV1 { .. } => "IdCardCnV1",
            StructuredFields::BankCardV1 { .. } => "BankCardV1",
            StructuredFields::BusinessLicenseV1 { .. } => "BusinessLicenseV1",
        };
        let de_tag = match &de {
            StructuredFields::DocumentV1 { .. } => "DocumentV1",
            StructuredFields::ReceiptV1 { .. } => "ReceiptV1",
            StructuredFields::TableV1 { .. } => "TableV1",
            StructuredFields::CardV1 { .. } => "CardV1",
            StructuredFields::IdCardCnV1 { .. } => "IdCardCnV1",
            StructuredFields::BankCardV1 { .. } => "BankCardV1",
            StructuredFields::BusinessLicenseV1 { .. } => "BusinessLicenseV1",
        };
        prop_assert_eq!(orig_tag, de_tag);
    }

    /// BBox 任意 u32 数据组合 serde clean (溢出/NaN 不存在 — u32 整数)
    #[test]
    fn prop_bbox_serde_no_panic(bbox in arb_bbox()) {
        let json = serde_json::to_string(&bbox).expect("ser");
        let de: BBox = serde_json::from_str(&json).expect("de");
        prop_assert_eq!(de.x, bbox.x);
        prop_assert_eq!(de.y, bbox.y);
        prop_assert_eq!(de.w, bbox.w);
        prop_assert_eq!(de.h, bbox.h);
    }

    /// RawLine 任意输入 serde 不 panic + confidence 不变
    #[test]
    fn prop_raw_line_serde_roundtrip(rl in arb_raw_line()) {
        let json = serde_json::to_string(&rl).expect("ser");
        let de: RawLine = serde_json::from_str(&json).expect("de");
        prop_assert_eq!(de.text, rl.text);
        prop_assert!((de.confidence - rl.confidence).abs() < 1e-6);
    }

    /// extract() 对任意 RawLine 列表不 panic (路由到所有 B-档 scene)
    #[test]
    fn prop_extract_no_panic_for_b_layer_scenes(
        lines in proptest::collection::vec(arb_raw_line(), 0..20)
    ) {
        for profile in ["document", "receipt", "table", "card"] {
            let _ = attune_core::ocr::structured::extract(profile, &lines, None);
        }
        for subtype in ["id_card_cn", "bank_card", "business_license"] {
            let _ = attune_core::ocr::structured::extract("id_card", &lines, Some(subtype));
        }
    }
}

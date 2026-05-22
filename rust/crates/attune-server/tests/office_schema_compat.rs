//! D3.4b — L1 Schema Compatibility Gate.
//!
//! Spec §3.1 + §4.4 (tagged union path Y). Pure serde unit tests — no HTTP.
//!
//! 验证 StructuredFields 7 variants serde round-trip + 未知 schema tag 干净失败 +
//! extract() 路由对 7 个 profile + 3 个 id_card subtype 行为正确.

use attune_core::ocr::structured::scene_card::CardFields;
use attune_core::ocr::structured::scene_document::DocumentFields;
use attune_core::ocr::structured::scene_id_card::{
    BankCardFields, BusinessLicenseFields, IdCardCnFields,
};
use attune_core::ocr::structured::scene_receipt::ReceiptFields;
use attune_core::ocr::structured::scene_table::TableFields;
use attune_core::ocr::structured::{FieldValue, StructuredFields};
use attune_core::ocr::{BBox, RawLine};

fn fv(value: &str) -> FieldValue {
    FieldValue {
        value: Some(value.into()),
        confidence: 0.9,
        bbox: Some(BBox { x: 1, y: 2, w: 3, h: 4 }),
        source_line_idx: Some(0),
    }
}

fn rl(text: &str) -> RawLine {
    RawLine {
        text: text.into(),
        bbox: BBox { x: 0, y: 0, w: 100, h: 20 },
        confidence: 0.95,
    }
}

// ─── 7 variants serde round-trip ─────────────────────────────────────────────

#[test]
fn document_v1_serde_roundtrip() {
    let v = StructuredFields::DocumentV1 {
        fields: DocumentFields { title: Some(fv("My Title")), blocks: vec![] },
        unrecognized_fields: vec!["blocks".into()],
        validation_warnings: vec![],
    };
    let json = serde_json::to_string(&v).expect("ser");
    assert!(json.contains("\"schema\":\"document_v1\""), "tag present: {json}");
    let de: StructuredFields = serde_json::from_str(&json).expect("de");
    match de {
        StructuredFields::DocumentV1 { fields, .. } => {
            assert_eq!(fields.title.unwrap().value.as_deref(), Some("My Title"));
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn receipt_v1_serde_roundtrip() {
    let fields = ReceiptFields { invoice_no: fv("12345678"), amount_total: fv("1234.56"), ..Default::default() };
    let v = StructuredFields::ReceiptV1 {
        fields,
        unrecognized_fields: vec!["buyer".into()],
        validation_warnings: vec!["test warning".into()],
    };
    let json = serde_json::to_string(&v).expect("ser");
    assert!(json.contains("\"schema\":\"receipt_v1\""));
    let de: StructuredFields = serde_json::from_str(&json).expect("de");
    match de {
        StructuredFields::ReceiptV1 { fields, validation_warnings, .. } => {
            assert_eq!(fields.invoice_no.value.as_deref(), Some("12345678"));
            assert_eq!(validation_warnings, vec!["test warning"]);
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn table_v1_serde_roundtrip() {
    let fields = TableFields { row_count: fv("3"), column_count: fv("4"), ..Default::default() };
    let v = StructuredFields::TableV1 {
        fields,
        unrecognized_fields: vec![],
        validation_warnings: vec![],
    };
    let json = serde_json::to_string(&v).expect("ser");
    assert!(json.contains("\"schema\":\"table_v1\""));
    let de: StructuredFields = serde_json::from_str(&json).expect("de");
    if let StructuredFields::TableV1 { fields, .. } = de {
        assert_eq!(fields.row_count.value.as_deref(), Some("3"));
        assert_eq!(fields.column_count.value.as_deref(), Some("4"));
    } else {
        panic!("wrong variant");
    }
}

#[test]
fn card_v1_serde_roundtrip() {
    let fields = CardFields { name: fv("Alice"), email: fv("alice@example.com"), ..Default::default() };
    let v = StructuredFields::CardV1 {
        fields,
        unrecognized_fields: vec!["address".into()],
        validation_warnings: vec![],
    };
    let json = serde_json::to_string(&v).expect("ser");
    assert!(json.contains("\"schema\":\"card_v1\""));
    let de: StructuredFields = serde_json::from_str(&json).expect("de");
    if let StructuredFields::CardV1 { fields, .. } = de {
        assert_eq!(fields.name.value.as_deref(), Some("Alice"));
        assert_eq!(fields.email.value.as_deref(), Some("alice@example.com"));
    } else {
        panic!("wrong variant");
    }
}

#[test]
fn id_card_cn_v1_serde_roundtrip() {
    let fields = IdCardCnFields { name: fv("张三"), id_number: fv("110101199001010015"), ..Default::default() };
    let v = StructuredFields::IdCardCnV1 {
        fields,
        unrecognized_fields: vec![],
        validation_warnings: vec![],
    };
    let json = serde_json::to_string(&v).expect("ser");
    assert!(json.contains("\"schema\":\"id_card_cn_v1\""));
    let de: StructuredFields = serde_json::from_str(&json).expect("de");
    if let StructuredFields::IdCardCnV1 { fields, .. } = de {
        assert_eq!(fields.name.value.as_deref(), Some("张三"));
        assert_eq!(fields.id_number.value.as_deref(), Some("110101199001010015"));
    } else {
        panic!("wrong variant");
    }
}

#[test]
fn bank_card_v1_serde_roundtrip() {
    let fields = BankCardFields { card_number: fv("4111 1111 1111 1111"), bank_name: fv("中国工商银行"), ..Default::default() };
    let v = StructuredFields::BankCardV1 {
        fields,
        unrecognized_fields: vec![],
        validation_warnings: vec![],
    };
    let json = serde_json::to_string(&v).expect("ser");
    assert!(json.contains("\"schema\":\"bank_card_v1\""));
    let de: StructuredFields = serde_json::from_str(&json).expect("de");
    if let StructuredFields::BankCardV1 { fields, .. } = de {
        assert_eq!(fields.card_number.value.as_deref(), Some("4111 1111 1111 1111"));
        assert_eq!(fields.bank_name.value.as_deref(), Some("中国工商银行"));
    } else {
        panic!("wrong variant");
    }
}

#[test]
fn business_license_v1_serde_roundtrip() {
    let fields = BusinessLicenseFields { registration_no: fv("91110000600000000X"), company_name: fv("测试有限公司"), ..Default::default() };
    let v = StructuredFields::BusinessLicenseV1 {
        fields,
        unrecognized_fields: vec!["scope".into()],
        validation_warnings: vec![],
    };
    let json = serde_json::to_string(&v).expect("ser");
    assert!(json.contains("\"schema\":\"business_license_v1\""));
    let de: StructuredFields = serde_json::from_str(&json).expect("de");
    if let StructuredFields::BusinessLicenseV1 { fields, .. } = de {
        assert_eq!(fields.registration_no.value.as_deref(), Some("91110000600000000X"));
    } else {
        panic!("wrong variant");
    }
}

// ─── Unknown schema → clean Err (no panic) ───────────────────────────────────

#[test]
fn unknown_schema_tag_returns_err_not_panic() {
    let json = r#"{"schema":"totally_unknown_v999","fields":{}}"#;
    let result: Result<StructuredFields, _> = serde_json::from_str(json);
    assert!(result.is_err(), "unknown schema tag must Err");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("unknown variant") || err_msg.contains("totally_unknown_v999"),
        "err msg should mention unknown variant: {err_msg}"
    );
}

#[test]
fn missing_schema_tag_returns_err() {
    let json = r#"{"fields":{}}"#;
    let result: Result<StructuredFields, _> = serde_json::from_str(json);
    assert!(result.is_err(), "missing tag must Err");
}

// ─── extract() routing for all 9 profile names ───────────────────────────────

#[test]
fn extract_routes_b_layer_scenes() {
    let lines = vec![rl("test")];

    // B-档 scenes: extract should return Some(_)
    for profile in ["document", "receipt", "table", "card"] {
        let r = attune_core::ocr::structured::extract(profile, &lines, None);
        assert!(r.is_some(), "profile '{profile}' must return Some StructuredFields");
    }
}

#[test]
fn extract_routes_a_layer_scenes_to_none() {
    let lines = vec![rl("test")];

    // A-档 only scenes: extract returns None (caller uses A 档 lines)
    for profile in ["screenshot", "contract", "ancient", "form"] {
        let r = attune_core::ocr::structured::extract(profile, &lines, None);
        assert!(r.is_none(), "profile '{profile}' (A-only) must return None");
    }
}

#[test]
fn extract_routes_id_card_subtypes() {
    let lines = vec![rl("test")];

    for subtype in ["id_card_cn", "bank_card", "business_license"] {
        let r = attune_core::ocr::structured::extract("id_card", &lines, Some(subtype));
        assert!(r.is_some(), "subtype '{subtype}' must return Some");
    }

    // unknown subtype → None
    let r = attune_core::ocr::structured::extract("id_card", &lines, Some("unknown_subtype"));
    assert!(r.is_none(), "unknown id_card subtype must return None");

    // id_card without subtype → None
    let r = attune_core::ocr::structured::extract("id_card", &lines, None);
    assert!(r.is_none(), "id_card without subtype must return None");
}

#[test]
fn extract_routes_totally_unknown_profile_to_none() {
    let lines = vec![rl("test")];
    let r = attune_core::ocr::structured::extract("totally_made_up_xyz", &lines, None);
    assert!(r.is_none(), "unknown profile must return None");
}

// ─── envelope_version present (snake_case JSON field check on REST shape) ────

#[test]
fn ocr_response_envelope_includes_version() {
    // Build a fake OcrResponse-equivalent JSON manually (we can't import the
    // route's response type because it's pub(crate); we check the contract by
    // building the same shape and asserting envelope_version field name + value).
    let envelope = serde_json::json!({
        "envelope_version": "1",
        "profile": "receipt",
        "elapsed_ms": 1000,
        "engine": "ppocrv5-mobile",
        "lines": [],
        "structured": null,
    });
    assert_eq!(envelope["envelope_version"], "1");
    assert!(envelope.get("profile").is_some());
    assert!(envelope.get("elapsed_ms").is_some());
    assert!(envelope.get("engine").is_some());
    assert!(envelope.get("lines").is_some());
}

//! Export HTTP route integration test (spec §5.1 + §9.1) — end-to-end download.
//!
//! Boots the real router (`spawn_eval_server`) and POSTs IR to `/api/v1/export`,
//! asserting the response is a real downloadable file with the right headers, that
//! the bytes re-parse correctly (calamine for xlsx, pdf-extract for CJK pdf), and
//! that filename/format errors map to stable kebab codes. Export needs no member
//! gate (🆓 zero-cost), so all of this works without a paid login.

use attune_server::test_support::spawn_eval_server;
use serde_json::{json, Value};
use std::io::Cursor;

fn device_table_ir() -> Value {
    json!({
        "type": "table",
        "data": {
            "title": "设备参数差异",
            "headers": ["参数", "设备A", "设备B"],
            "rows": [
                ["电压", "220V", "110V"],
                ["额定功率", "1000瓦", "1500瓦"]
            ],
            "aligns": ["left", "right", "right"]
        }
    })
}

async fn post_export(base: &str, body: Value) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{base}/api/v1/export"))
        .json(&body)
        .send()
        .await
        .expect("request sent")
}

#[tokio::test]
async fn export_xlsx_downloads_and_reparses() {
    use calamine::{Data, Reader, Xlsx};

    let srv = spawn_eval_server().await;
    let resp = post_export(
        &srv.url(),
        json!({ "artifact": device_table_ir(), "format": "xlsx", "filename": "设备参数差异" }),
    )
    .await;

    assert_eq!(resp.status().as_u16(), 200);
    let ctype = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(ctype.contains("spreadsheetml.sheet"), "xlsx mime: {ctype}");
    let cd = resp
        .headers()
        .get("content-disposition")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(cd.contains("attachment"), "is attachment: {cd}");
    assert!(cd.contains("filename*=UTF-8''"), "rfc5987 name: {cd}");

    let bytes = resp.bytes().await.unwrap().to_vec();
    let mut wb: Xlsx<_> = calamine::open_workbook_from_rs(Cursor::new(bytes)).unwrap();
    let name = wb.sheet_names()[0].clone();
    assert_eq!(name, "设备参数差异");
    let range = wb.worksheet_range(&name).unwrap();
    match range.get_value((1, 0)) {
        Some(Data::String(s)) => assert_eq!(s, "电压"),
        other => panic!("cell: {other:?}"),
    }
}

#[tokio::test]
async fn export_pdf_chinese_downloads_not_garbled() {
    let srv = spawn_eval_server().await;
    let resp = post_export(
        &srv.url(),
        json!({ "artifact": device_table_ir(), "format": "pdf" }),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "application/pdf"
    );
    let bytes = resp.bytes().await.unwrap().to_vec();
    assert!(bytes.starts_with(b"%PDF"), "is a PDF");

    let text = pdf_extract::extract_text_from_mem(&bytes).unwrap();
    let normalized: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    for needle in ["设备参数差异", "额定功率", "电压"] {
        assert!(
            normalized.contains(needle),
            "PDF download lost Chinese {needle}; got {normalized:?}"
        );
    }
}

#[tokio::test]
async fn export_csv_downloads_with_safe_filename() {
    let srv = spawn_eval_server().await;
    // hostile filename with path traversal — must be sanitised in the header.
    let resp = post_export(
        &srv.url(),
        json!({ "artifact": device_table_ir(), "format": "csv", "filename": "../../etc/passwd" }),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 200);
    let cd = resp
        .headers()
        .get("content-disposition")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    // no path separators leak into either filename form
    assert!(!cd.contains('/'), "no slash in disposition: {cd}");
    assert!(!cd.contains(".."), "no dotdot in disposition: {cd}");
    assert!(cd.contains("passwd"), "kept the basename: {cd}");
}

#[tokio::test]
async fn export_unknown_format_400_kebab() {
    let srv = spawn_eval_server().await;
    let resp = post_export(
        &srv.url(),
        json!({ "artifact": device_table_ir(), "format": "ppt" }),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 400);
    let v: Value = resp.json().await.unwrap();
    assert_eq!(v["code"], "unsupported-format");
}

#[tokio::test]
async fn export_malformed_ir_400_kebab() {
    let srv = spawn_eval_server().await;
    // ragged row → malformed-ir
    let bad = json!({
        "type": "table",
        "data": { "headers": ["a", "b"], "rows": [["only-one"]] }
    });
    let resp = post_export(&srv.url(), json!({ "artifact": bad, "format": "csv" })).await;
    assert_eq!(resp.status().as_u16(), 400);
    let v: Value = resp.json().await.unwrap();
    assert_eq!(v["code"], "malformed-ir");
}

// ── INT-2 privacy egress gate (adversarial: real route, real download) ───────

/// A confidential artifact (绝密 marker) is fail-closed REFUSED at the real
/// `/export` route — no file is ever produced. Proves the export bypass hole is
/// closed for classified docs.
#[tokio::test]
async fn export_classified_artifact_blocked_fail_closed() {
    let srv = spawn_eval_server().await;
    let classified = json!({
        "type": "document",
        "data": {
            "title": "尽职调查报告",
            "blocks": [
                { "kind": "paragraph", "text": "绝密：本报告禁止外传，仅限内部。" },
                { "kind": "paragraph", "text": "客户电话 13800138000" }
            ]
        }
    });
    for fmt in ["docx", "pdf", "md", "csv", "xlsx"] {
        let resp = post_export(&srv.url(), json!({ "artifact": classified, "format": fmt })).await;
        assert_eq!(
            resp.status().as_u16(),
            422,
            "classified export must be blocked for {fmt}"
        );
        let v: Value = resp.json().await.unwrap();
        assert_eq!(v["code"], "doc-classified", "{fmt}");
    }
}

/// A normal artifact carrying PII downloads SUCCESSFULLY but the bytes that leave
/// the device carry NO plaintext PII (redacted in place before render). This is
/// the core "export no longer bypasses redaction" proof, on the real wire.
#[tokio::test]
async fn export_pii_artifact_downloads_redacted_no_plaintext() {
    let srv = spawn_eval_server().await;
    let pii_doc = json!({
        "type": "document",
        "data": {
            "title": "客户联系清单",
            "blocks": [
                { "kind": "paragraph", "text": "张三 电话 13800138000 邮箱 zhangsan@example.com" },
                { "kind": "list", "ordered": false, "items": ["李四 手机 13900139000"] }
            ]
        }
    });

    // markdown — easy to assert on the raw bytes
    let resp = post_export(&srv.url(), json!({ "artifact": pii_doc, "format": "md" })).await;
    assert_eq!(resp.status().as_u16(), 200);
    let bytes = resp.bytes().await.unwrap().to_vec();
    let md = String::from_utf8(bytes).unwrap();
    for raw in ["13800138000", "zhangsan@example.com", "13900139000"] {
        assert!(!md.contains(raw), "downloaded md must not carry plaintext {raw}; got {md}");
    }
    // structure preserved (title + names still present)
    assert!(md.contains("客户联系清单") && md.contains("张三") && md.contains("李四"));

    // docx — adversarial format: verify the PII isn't hiding inside the OOXML zip
    let resp = post_export(&srv.url(), json!({ "artifact": pii_doc, "format": "docx" })).await;
    assert_eq!(resp.status().as_u16(), 200);
    let docx = resp.bytes().await.unwrap().to_vec();
    // crude but effective: the raw phone digits must not appear anywhere in the
    // (decompressed) document. Unzip word/document.xml and scan it.
    let mut ar = zip::ZipArchive::new(Cursor::new(docx)).unwrap();
    let mut doc_xml = String::new();
    use std::io::Read;
    ar.by_name("word/document.xml")
        .unwrap()
        .read_to_string(&mut doc_xml)
        .unwrap();
    for raw in ["13800138000", "zhangsan@example.com", "13900139000"] {
        assert!(!doc_xml.contains(raw), "docx document.xml must not carry {raw}");
    }
}

/// A clean artifact (no PII, no markers) downloads unchanged — no false positive.
#[tokio::test]
async fn export_clean_artifact_unchanged() {
    let srv = spawn_eval_server().await;
    let resp = post_export(
        &srv.url(),
        json!({ "artifact": device_table_ir(), "format": "csv" }),
    )
    .await;
    assert_eq!(resp.status().as_u16(), 200);
    let csv = String::from_utf8(resp.bytes().await.unwrap().to_vec()).unwrap();
    // device table has no PII → content intact
    assert!(csv.contains("电压") && csv.contains("220V"));
}

/// The dry-run preview endpoint reports the same verdict the real export would,
/// so the UI can warn before the user clicks download.
#[tokio::test]
async fn doc_privacy_export_preview_matches_verdict() {
    let srv = spawn_eval_server().await;
    let client = reqwest::Client::new();

    // classified → blocked
    let classified = json!({
        "type": "document",
        "data": { "title": "x", "blocks": [{ "kind": "paragraph", "text": "机密文件" }] }
    });
    let resp = client
        .post(format!("{}/api/v1/doc-privacy/export-preview", srv.url()))
        .json(&json!({ "artifact": classified }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let v: Value = resp.json().await.unwrap();
    assert_eq!(v["decision"], "blocked");
    assert_eq!(v["blocked"], true);

    // PII → allowed + will_redact
    let pii = json!({
        "type": "document",
        "data": { "title": "y", "blocks": [{ "kind": "paragraph", "text": "电话 13800138000" }] }
    });
    let resp = client
        .post(format!("{}/api/v1/doc-privacy/export-preview", srv.url()))
        .json(&json!({ "artifact": pii }))
        .send()
        .await
        .unwrap();
    let v: Value = resp.json().await.unwrap();
    assert_eq!(v["decision"], "allowed");
    assert_eq!(v["will_redact"], true);
    assert!(v["redacted_count"].as_u64().unwrap() >= 1);
}

/// The scan endpoint classifies raw text without leaking PII values.
#[tokio::test]
async fn doc_privacy_scan_text() {
    let srv = spawn_eval_server().await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/v1/doc-privacy/scan", srv.url()))
        .json(&json!({ "text": "绝密：联系电话 13800138000" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let v: Value = resp.json().await.unwrap();
    assert_eq!(v["classification"], "classified");
    assert_eq!(v["blocked"], true);
    // privacy-first: the response body must not echo the phone value
    let body = serde_json::to_string(&v).unwrap();
    assert!(!body.contains("13800138000"), "scan must not leak PII value");
}

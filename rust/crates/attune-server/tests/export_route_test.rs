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

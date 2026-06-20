//! Artifact export route (spec §5.1) — turn an [`Artifact`] IR into a downloadable
//! office file (xlsx / csv / md / docx / pdf).
//!
//! `POST /api/v1/export` with body
//! `{ "artifact": { "type": "table", "data": {…} }, "format": "xlsx",
//! "filename": "设备参数差异" }` → the rendered file as an attachment download.
//!
//! **Cost contract (CLAUDE.md §成本契约)**: export is **tier-🆓** — pure-Rust
//! rendering, no LLM, no member gate, no privacy egress. Anyone (even with the
//! vault locked) can render IR they already hold. The route never touches an
//! `LlmProvider`.
//!
//! **Security**: the download filename is sanitised against path traversal
//! (`attune_core::export::sanitize::download_filename`) and RFC-5987-encoded so a
//! CJK name is safe in the `Content-Disposition` header. Formula/CSV-injection is
//! handled inside the renderers.

use crate::error::AppError;
use crate::routes::errors::internal;
use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use attune_core::export::sanitize::download_filename;
use attune_core::export::{Artifact, ExportFormat};

#[derive(Debug, Deserialize)]
pub struct ExportRequest {
    /// The stable export IR (Table or Document).
    pub artifact: Artifact,
    /// Output format: xlsx | csv | md | docx | pdf (case-insensitive).
    pub format: String,
    /// Optional caller-supplied download name (extension is derived from format).
    #[serde(default)]
    pub filename: Option<String>,
}

/// RFC 5987 `filename*` value: percent-encode everything that isn't an
/// attr-char so a UTF-8 (e.g. Chinese) filename is transported safely.
fn rfc5987(name: &str) -> String {
    let mut out = String::with_capacity(name.len() * 3);
    for b in name.as_bytes() {
        let c = *b;
        // attr-char per RFC 5987: ALPHA / DIGIT / !#$&+-.^_`|~
        let safe = c.is_ascii_alphanumeric()
            || matches!(
                c,
                b'!' | b'#' | b'$' | b'&' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
            );
        if safe {
            out.push(c as char);
        } else {
            out.push('%');
            out.push_str(&format!("{c:02X}"));
        }
    }
    out
}

/// Build the `Content-Disposition: attachment` value with both an ASCII-fallback
/// `filename=` and an RFC-5987 `filename*=` for non-ASCII names.
fn content_disposition(filename: &str) -> String {
    // ASCII fallback: replace non-ascii with '_' so old clients still get a name.
    let ascii: String = filename
        .chars()
        .map(|c| if c.is_ascii() { c } else { '_' })
        .collect();
    format!(
        "attachment; filename=\"{}\"; filename*=UTF-8''{}",
        ascii,
        rfc5987(filename)
    )
}

/// POST /api/v1/export — render IR → downloadable file.
pub async fn export_artifact(Json(req): Json<ExportRequest>) -> Response {
    let format = match ExportFormat::parse(&req.format) {
        Some(f) => f,
        None => {
            return AppError::detailed(
                StatusCode::BAD_REQUEST,
                json!({
                    "error": format!("unknown export format {:?} (xlsx|csv|md|docx|pdf)", req.format),
                    "code": "unsupported-format"
                }),
            )
            .into_response();
        }
    };

    let bytes = match req.artifact.render(format) {
        Ok(b) => b,
        Err(e) => {
            let status = match attune_core::export::http_status_for(e.code()) {
                400 => StatusCode::BAD_REQUEST,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            return AppError::detailed(
                status,
                json!({ "error": e.to_string(), "code": e.code() }),
            )
            .into_response();
        }
    };

    // Derive a safe download name: caller filename → IR title → "export".
    let raw_name = req
        .filename
        .as_deref()
        .or_else(|| req.artifact.title())
        .unwrap_or("export");
    let filename = download_filename(raw_name, format.extension());

    match Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, format.mime())
        .header(header::CONTENT_DISPOSITION, content_disposition(&filename))
        .header(header::CONTENT_LENGTH, bytes.len().to_string())
        .header("X-Export-Cost-Tier", "free")
        .body(Body::from(bytes))
    {
        Ok(resp) => resp,
        Err(e) => internal("build export response", e).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc5987_encodes_cjk_and_keeps_ascii() {
        assert_eq!(rfc5987("report-2026.xlsx"), "report-2026.xlsx");
        // CJK bytes are percent-encoded (UTF-8, uppercase hex)
        let enc = rfc5987("设备.xlsx");
        assert!(enc.contains('%') && enc.ends_with(".xlsx"));
        assert!(!enc.contains('设'));
    }

    #[test]
    fn content_disposition_has_both_forms() {
        let cd = content_disposition("设备参数差异.xlsx");
        assert!(cd.contains("attachment; filename=\""));
        assert!(cd.contains("filename*=UTF-8''"));
        // ascii fallback must not carry raw CJK
        let ascii_part = cd.split("filename*").next().unwrap();
        assert!(!ascii_part.contains('设'));
    }
}
